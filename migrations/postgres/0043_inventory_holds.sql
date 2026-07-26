ALTER TABLE inventory_balances
    ADD COLUMN qty_held BIGINT NOT NULL DEFAULT 0,
    ADD CONSTRAINT inventory_balances_qty_held_nonnegative
        CHECK (qty_held >= 0),
    ADD CONSTRAINT inventory_balances_commitments_within_on_hand
        CHECK (qty_reserved + qty_held <= qty_on_hand);

ALTER TABLE cycle_count_item_location_results
    ADD COLUMN system_qty_held BIGINT NOT NULL,
    DROP CONSTRAINT cycle_count_item_location_results_check,
    DROP CONSTRAINT cycle_count_item_location_results_check1,
    ADD CONSTRAINT cycle_count_results_system_qty_held_nonnegative
        CHECK (system_qty_held >= 0),
    ADD CONSTRAINT cycle_count_results_commitments_within_on_hand
        CHECK (
            system_qty_reserved + system_qty_held <= system_qty_on_hand
        ),
    ADD CONSTRAINT cycle_count_results_counted_at_least_commitments
        CHECK (
            counted_qty >= system_qty_reserved + system_qty_held
        );

CREATE TABLE inventory_holds (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    modified TIMESTAMPTZ,
    deleted TIMESTAMPTZ,
    created_by BIGINT NOT NULL,
    released_by BIGINT,
    released_at TIMESTAMPTZ,
    inventory_balance_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    location_id BIGINT NOT NULL,
    license_plate_id BIGINT,
    item_batch_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    uom TEXT NOT NULL,
    inventory_status TEXT NOT NULL,
    qty BIGINT NOT NULL CHECK (qty > 0),
    reason_code TEXT NOT NULL,
    note TEXT,
    reference_type TEXT,
    reference_id BIGINT,
    status TEXT NOT NULL DEFAULT 'active',
    CHECK (status IN ('active', 'released')),
    CHECK (inventory_status IN ('available', 'hold', 'damaged', 'quarantine')),
    CHECK (BTRIM(uom) <> ''),
    CHECK (
        reason_code IN (
            'quality_inspection',
            'damage_suspected',
            'inventory_discrepancy',
            'regulatory',
            'customer_request',
            'other'
        )
    ),
    CHECK (
        reason_code <> 'other'
        OR (note IS NOT NULL AND BTRIM(note) <> '')
    ),
    CHECK (
        (reference_type IS NULL AND reference_id IS NULL)
        OR (
            reference_type IS NOT NULL
            AND BTRIM(reference_type) <> ''
            AND reference_id IS NOT NULL
            AND reference_id > 0
        )
    ),
    CHECK (
        (
            status = 'active'
            AND deleted IS NULL
            AND released_by IS NULL
            AND released_at IS NULL
        )
        OR (
            status = 'released'
            AND deleted IS NOT NULL
            AND released_by IS NOT NULL
            AND released_at IS NOT NULL
            AND modified = released_at
            AND deleted = released_at
        )
    ),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, created_by)
        REFERENCES tenant_memberships(tenant_id, user_id),
    FOREIGN KEY (tenant_id, released_by)
        REFERENCES tenant_memberships(tenant_id, user_id),
    FOREIGN KEY (tenant_id, inventory_owner_id)
        REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(
            tenant_id,
            inventory_owner_id,
            facility_id
        ),
    FOREIGN KEY (tenant_id, inventory_owner_id, inventory_balance_id)
        REFERENCES inventory_balances(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, facility_id, location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        license_plate_id
    ) REFERENCES license_plates(
        tenant_id,
        inventory_owner_id,
        facility_id,
        id
    ),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id)
        REFERENCES item_batches(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, item_id)
        REFERENCES items(tenant_id, id)
);

CREATE INDEX inventory_holds_balance_idx
    ON inventory_holds(
        tenant_id,
        inventory_owner_id,
        inventory_balance_id,
        status
    );
CREATE INDEX inventory_holds_owner_facility_idx
    ON inventory_holds(
        tenant_id,
        inventory_owner_id,
        facility_id,
        status
    );
CREATE INDEX inventory_holds_active_reference_idx
    ON inventory_holds(tenant_id, reference_type, reference_id)
    WHERE deleted IS NULL
      AND status = 'active'
      AND reference_type IS NOT NULL;

ALTER TABLE inventory_holds ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_holds FORCE ROW LEVEL SECURITY;
CREATE POLICY inventory_holds_tenant_isolation
    ON inventory_holds
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

DROP TRIGGER inventory_balances_guard_allocation_projection
    ON inventory_balances;
DROP FUNCTION guard_inventory_balance_allocation_projection();

CREATE FUNCTION guard_inventory_balance_commitments()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    allocated_qty BIGINT;
    held_qty BIGINT;
BEGIN
    SELECT COALESCE(SUM(allocation.qty), 0)::BIGINT
    INTO allocated_qty
    FROM public.inventory_allocations allocation
    WHERE allocation.tenant_id = NEW.tenant_id
      AND allocation.inventory_owner_id = NEW.inventory_owner_id
      AND allocation.inventory_balance_id = NEW.id
      AND allocation.deleted IS NULL
      AND allocation.status = 'allocated';

    SELECT COALESCE(SUM(hold.qty), 0)::BIGINT
    INTO held_qty
    FROM public.inventory_holds hold
    WHERE hold.tenant_id = NEW.tenant_id
      AND hold.inventory_owner_id = NEW.inventory_owner_id
      AND hold.inventory_balance_id = NEW.id
      AND hold.deleted IS NULL
      AND hold.status = 'active';

    IF NEW.qty_reserved IS DISTINCT FROM allocated_qty THEN
        RAISE EXCEPTION
            'inventory balance reserved projection does not match allocations'
            USING ERRCODE = '55000';
    END IF;

    IF NEW.qty_held IS DISTINCT FROM held_qty THEN
        RAISE EXCEPTION
            'inventory balance held projection does not match active holds'
            USING ERRCODE = '55000';
    END IF;

    IF allocated_qty + held_qty > NEW.qty_on_hand THEN
        RAISE EXCEPTION
            'inventory balance commitments exceed on-hand quantity'
            USING ERRCODE = '55000';
    END IF;

    IF allocated_qty + held_qty > 0
       AND (
           NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
           OR NEW.inventory_owner_id IS DISTINCT FROM
               OLD.inventory_owner_id
           OR NEW.facility_id IS DISTINCT FROM OLD.facility_id
           OR NEW.location_id IS DISTINCT FROM OLD.location_id
           OR NEW.license_plate_id IS DISTINCT FROM OLD.license_plate_id
           OR NEW.item_batch_id IS DISTINCT FROM OLD.item_batch_id
           OR NEW.item_id IS DISTINCT FROM OLD.item_id
           OR NEW.uom IS DISTINCT FROM OLD.uom
           OR NEW.status IS DISTINCT FROM OLD.status
           OR NEW.deleted IS DISTINCT FROM OLD.deleted
       )
    THEN
        RAISE EXCEPTION
            'committed inventory balance dimensions are immutable'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER inventory_balances_guard_commitments
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, facility_id,
        location_id, license_plate_id, item_batch_id, item_id, uom, status,
        qty_on_hand, qty_reserved, qty_held, deleted
    ON inventory_balances
    FOR EACH ROW
    EXECUTE FUNCTION guard_inventory_balance_commitments();

CREATE OR REPLACE FUNCTION validate_inventory_allocation()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    reservation_facility_id BIGINT;
    reservation_item_id BIGINT;
    reservation_uom TEXT;
    reservation_qty BIGINT;
    reservation_status TEXT;
    balance_facility_id BIGINT;
    balance_location_id BIGINT;
    balance_license_plate_id BIGINT;
    balance_item_batch_id BIGINT;
    balance_item_id BIGINT;
    balance_uom TEXT;
    balance_status TEXT;
    balance_on_hand BIGINT;
    balance_reserved BIGINT;
    balance_held BIGINT;
    allocated_qty BIGINT;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
           OR NEW.inventory_owner_id IS DISTINCT FROM OLD.inventory_owner_id
           OR NEW.created IS DISTINCT FROM OLD.created
           OR NEW.created_by IS DISTINCT FROM OLD.created_by
           OR NEW.reservation_id IS DISTINCT FROM OLD.reservation_id
           OR NEW.inventory_balance_id IS DISTINCT FROM
               OLD.inventory_balance_id
           OR NEW.facility_id IS DISTINCT FROM OLD.facility_id
           OR NEW.location_id IS DISTINCT FROM OLD.location_id
           OR NEW.license_plate_id IS DISTINCT FROM OLD.license_plate_id
           OR NEW.item_batch_id IS DISTINCT FROM OLD.item_batch_id
           OR NEW.item_id IS DISTINCT FROM OLD.item_id
           OR NEW.uom IS DISTINCT FROM OLD.uom
           OR NEW.inventory_status IS DISTINCT FROM OLD.inventory_status
           OR NEW.qty IS DISTINCT FROM OLD.qty
        THEN
            RAISE EXCEPTION 'inventory allocation dimensions are immutable'
                USING ERRCODE = '55000';
        END IF;

        IF OLD.status <> 'allocated'
           AND (
               NEW.status IS DISTINCT FROM OLD.status
               OR NEW.deleted IS DISTINCT FROM OLD.deleted
               OR NEW.modified IS DISTINCT FROM OLD.modified
           )
        THEN
            RAISE EXCEPTION 'terminal inventory allocation is immutable'
                USING ERRCODE = '55000';
        END IF;
    ELSIF NEW.status <> 'allocated' OR NEW.deleted IS NOT NULL THEN
        RAISE EXCEPTION 'new inventory allocation must be active'
            USING ERRCODE = '23514';
    END IF;

    SELECT reservation.facility_id, reservation.item_id, reservation.uom,
           reservation.qty, reservation.status
    INTO reservation_facility_id, reservation_item_id, reservation_uom,
         reservation_qty, reservation_status
    FROM public.inventory_reservations reservation
    WHERE reservation.tenant_id = NEW.tenant_id
      AND reservation.inventory_owner_id = NEW.inventory_owner_id
      AND reservation.id = NEW.reservation_id
    FOR UPDATE;

    IF reservation_qty IS NULL THEN
        RAISE EXCEPTION 'inventory reservation does not exist'
            USING ERRCODE = '23503';
    END IF;

    SELECT balance.facility_id, balance.location_id,
           balance.license_plate_id, balance.item_batch_id, balance.item_id,
           balance.uom, balance.status, balance.qty_on_hand,
           balance.qty_reserved, balance.qty_held
    INTO balance_facility_id, balance_location_id,
         balance_license_plate_id, balance_item_batch_id, balance_item_id,
         balance_uom, balance_status, balance_on_hand, balance_reserved,
         balance_held
    FROM public.inventory_balances balance
    WHERE balance.tenant_id = NEW.tenant_id
      AND balance.inventory_owner_id = NEW.inventory_owner_id
      AND balance.id = NEW.inventory_balance_id
      AND balance.deleted IS NULL
    FOR UPDATE;

    IF balance_on_hand IS NULL THEN
        RAISE EXCEPTION 'allocation inventory balance does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.facility_id IS DISTINCT FROM reservation_facility_id
       OR NEW.facility_id IS DISTINCT FROM balance_facility_id
       OR NEW.location_id IS DISTINCT FROM balance_location_id
       OR NEW.license_plate_id IS DISTINCT FROM balance_license_plate_id
       OR NEW.item_batch_id IS DISTINCT FROM balance_item_batch_id
       OR NEW.item_id IS DISTINCT FROM reservation_item_id
       OR NEW.item_id IS DISTINCT FROM balance_item_id
       OR NEW.uom IS DISTINCT FROM reservation_uom
       OR NEW.uom IS DISTINCT FROM balance_uom
       OR NEW.inventory_status IS DISTINCT FROM balance_status
       OR NEW.inventory_status <> 'available'
    THEN
        RAISE EXCEPTION
            'allocation dimensions must match reservation and available balance'
            USING ERRCODE = '23514';
    END IF;

    IF TG_OP = 'INSERT' THEN
        IF reservation_status <> 'active' THEN
            RAISE EXCEPTION 'inventory reservation is not active'
                USING ERRCODE = '55000';
        END IF;

        SELECT COALESCE(SUM(allocation.qty), 0)::BIGINT
        INTO allocated_qty
        FROM public.inventory_allocations allocation
        WHERE allocation.tenant_id = NEW.tenant_id
          AND allocation.inventory_owner_id = NEW.inventory_owner_id
          AND allocation.reservation_id = NEW.reservation_id
          AND allocation.deleted IS NULL
          AND allocation.status = 'allocated';

        IF allocated_qty + NEW.qty > reservation_qty THEN
            RAISE EXCEPTION
                'active allocations exceed reservation demand'
                USING ERRCODE = '23514';
        END IF;

        IF balance_on_hand - balance_reserved - balance_held < NEW.qty THEN
            RAISE EXCEPTION
                'insufficient available inventory to allocate'
                USING ERRCODE = '55000';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION validate_item_location_cycle_count_result()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    task_row RECORD;
    balance_row RECORD;
    transaction_matches BOOLEAN;
    matching_entry_count BIGINT;
    transaction_entry_count BIGINT;
BEGIN
    SELECT task.status,
           task.assigned_user_id,
           task.deleted,
           task.lease_expires_at,
           detail.inventory_owner_id,
           detail.facility_id,
           detail.location_id,
           detail.item_id,
           detail.inventory_balance_id
    INTO task_row
    FROM public.work_tasks task
    JOIN public.cycle_count_item_location_tasks detail
      ON detail.tenant_id = task.tenant_id
     AND detail.task_id = task.id
    WHERE task.tenant_id = NEW.tenant_id
      AND task.id = NEW.task_id
      AND task.task_type = 'cycle_count_item_location'
    FOR UPDATE OF task, detail;

    IF NOT FOUND
       OR task_row.deleted IS NOT NULL
       OR task_row.status <> 'in_progress'
       OR task_row.assigned_user_id IS DISTINCT FROM NEW.confirmed_by
       OR task_row.lease_expires_at IS NULL
       OR task_row.lease_expires_at <= statement_timestamp()
       OR task_row.inventory_owner_id IS DISTINCT FROM NEW.inventory_owner_id
       OR task_row.facility_id IS DISTINCT FROM NEW.facility_id
       OR task_row.location_id IS DISTINCT FROM NEW.location_id
       OR task_row.item_id IS DISTINCT FROM NEW.item_id
       OR task_row.inventory_balance_id
            IS DISTINCT FROM NEW.inventory_balance_id
    THEN
        RAISE EXCEPTION
            'cycle count result does not match an active task claim'
            USING ERRCODE = '55000';
    END IF;

    SELECT balance.item_batch_id,
           balance.license_plate_id,
           balance.uom,
           batch.lot,
           batch.expiration,
           batch.serial,
           balance.status,
           balance.qty_on_hand,
           balance.qty_reserved,
           balance.qty_held,
           balance.deleted
    INTO balance_row
    FROM public.inventory_balances balance
    JOIN public.item_batches batch
      ON batch.tenant_id = balance.tenant_id
     AND batch.inventory_owner_id = balance.inventory_owner_id
     AND batch.id = balance.item_batch_id
    WHERE balance.tenant_id = NEW.tenant_id
      AND balance.inventory_owner_id = NEW.inventory_owner_id
      AND balance.facility_id = NEW.facility_id
      AND balance.location_id = NEW.location_id
      AND balance.item_id = NEW.item_id
      AND balance.id = NEW.inventory_balance_id
    FOR UPDATE;

    IF NOT FOUND
       OR balance_row.deleted IS NOT NULL
       OR balance_row.item_batch_id IS DISTINCT FROM NEW.item_batch_id
       OR balance_row.license_plate_id IS DISTINCT FROM NEW.license_plate_id
       OR balance_row.uom IS DISTINCT FROM NEW.uom
       OR balance_row.lot IS DISTINCT FROM NEW.lot
       OR balance_row.expiration IS DISTINCT FROM NEW.expiration
       OR balance_row.serial IS DISTINCT FROM NEW.serial
       OR balance_row.status IS DISTINCT FROM NEW.status
       OR balance_row.qty_on_hand IS DISTINCT FROM NEW.counted_qty
       OR balance_row.qty_reserved IS DISTINCT FROM NEW.system_qty_reserved
       OR balance_row.qty_held IS DISTINCT FROM NEW.system_qty_held
    THEN
        RAISE EXCEPTION
            'cycle count result does not match the adjusted balance'
            USING ERRCODE = '55000';
    END IF;

    IF NEW.variance_qty <> 0 THEN
        SELECT EXISTS (
            SELECT 1
            FROM public.inventory_transactions transaction
            WHERE transaction.tenant_id = NEW.tenant_id
              AND transaction.inventory_owner_id = NEW.inventory_owner_id
              AND transaction.id = NEW.inventory_transaction_id
              AND transaction.transaction_type = 'adjust'
              AND transaction.actor_user_id = NEW.confirmed_by
              AND transaction.reference_type =
                  'cycle_count_item_location_task'
              AND transaction.reference_id = NEW.task_id
              AND transaction.operation =
                  'task.confirm_item_location_cycle_count.v1'
        )
        INTO transaction_matches;

        SELECT COUNT(*),
               COUNT(*) FILTER (
                   WHERE entry.facility_id = NEW.facility_id
                     AND entry.location_id = NEW.location_id
                     AND entry.item_batch_id = NEW.item_batch_id
                     AND entry.item_id = NEW.item_id
                     AND entry.license_plate_id
                           IS NOT DISTINCT FROM NEW.license_plate_id
                     AND entry.uom = NEW.uom
                     AND entry.status = NEW.status
                     AND entry.quantity_delta = NEW.variance_qty
               )
        INTO transaction_entry_count, matching_entry_count
        FROM public.inventory_entries entry
        WHERE entry.tenant_id = NEW.tenant_id
          AND entry.inventory_owner_id = NEW.inventory_owner_id
          AND entry.transaction_id = NEW.inventory_transaction_id;

        IF NOT transaction_matches
           OR transaction_entry_count <> 1
           OR matching_entry_count <> 1
        THEN
            RAISE EXCEPTION
                'cycle count adjustment does not match its result'
                USING ERRCODE = '55000';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

CREATE FUNCTION validate_inventory_hold()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    balance_facility_id BIGINT;
    balance_location_id BIGINT;
    balance_license_plate_id BIGINT;
    balance_item_batch_id BIGINT;
    balance_item_id BIGINT;
    balance_uom TEXT;
    balance_status TEXT;
    balance_on_hand BIGINT;
    balance_reserved BIGINT;
    balance_held BIGINT;
    active_hold_qty BIGINT;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
           OR NEW.inventory_owner_id IS DISTINCT FROM OLD.inventory_owner_id
           OR NEW.created IS DISTINCT FROM OLD.created
           OR NEW.created_by IS DISTINCT FROM OLD.created_by
           OR NEW.inventory_balance_id IS DISTINCT FROM
               OLD.inventory_balance_id
           OR NEW.facility_id IS DISTINCT FROM OLD.facility_id
           OR NEW.location_id IS DISTINCT FROM OLD.location_id
           OR NEW.license_plate_id IS DISTINCT FROM OLD.license_plate_id
           OR NEW.item_batch_id IS DISTINCT FROM OLD.item_batch_id
           OR NEW.item_id IS DISTINCT FROM OLD.item_id
           OR NEW.uom IS DISTINCT FROM OLD.uom
           OR NEW.inventory_status IS DISTINCT FROM OLD.inventory_status
           OR NEW.qty IS DISTINCT FROM OLD.qty
           OR NEW.reason_code IS DISTINCT FROM OLD.reason_code
           OR NEW.note IS DISTINCT FROM OLD.note
           OR NEW.reference_type IS DISTINCT FROM OLD.reference_type
           OR NEW.reference_id IS DISTINCT FROM OLD.reference_id
        THEN
            RAISE EXCEPTION 'inventory hold dimensions are immutable'
                USING ERRCODE = '55000';
        END IF;

        IF OLD.status = 'released' THEN
            RAISE EXCEPTION 'released inventory hold is immutable'
                USING ERRCODE = '55000';
        END IF;

        IF NEW.status = 'active'
           AND (
               NEW.modified IS DISTINCT FROM OLD.modified
               OR NEW.deleted IS DISTINCT FROM OLD.deleted
               OR NEW.released_by IS DISTINCT FROM OLD.released_by
               OR NEW.released_at IS DISTINCT FROM OLD.released_at
           )
        THEN
            RAISE EXCEPTION 'active inventory hold lifecycle is immutable'
                USING ERRCODE = '55000';
        ELSIF NEW.status = 'released'
              AND (
                  NEW.modified IS NULL
                  OR NEW.deleted IS DISTINCT FROM NEW.modified
                  OR NEW.released_at IS DISTINCT FROM NEW.modified
                  OR NEW.released_by IS NULL
              )
        THEN
            RAISE EXCEPTION 'invalid inventory hold release'
                USING ERRCODE = '55000';
        ELSIF NEW.status NOT IN ('active', 'released') THEN
            RAISE EXCEPTION 'invalid inventory hold transition'
                USING ERRCODE = '55000';
        END IF;
    ELSIF NEW.status <> 'active'
          OR NEW.deleted IS NOT NULL
          OR NEW.released_by IS NOT NULL
          OR NEW.released_at IS NOT NULL
    THEN
        RAISE EXCEPTION 'new inventory hold must be active'
            USING ERRCODE = '23514';
    END IF;

    SELECT balance.facility_id, balance.location_id,
           balance.license_plate_id, balance.item_batch_id, balance.item_id,
           balance.uom, balance.status, balance.qty_on_hand,
           balance.qty_reserved, balance.qty_held
    INTO balance_facility_id, balance_location_id,
         balance_license_plate_id, balance_item_batch_id, balance_item_id,
         balance_uom, balance_status, balance_on_hand, balance_reserved,
         balance_held
    FROM public.inventory_balances balance
    WHERE balance.tenant_id = NEW.tenant_id
      AND balance.inventory_owner_id = NEW.inventory_owner_id
      AND balance.id = NEW.inventory_balance_id
      AND balance.deleted IS NULL
    FOR UPDATE;

    IF balance_on_hand IS NULL THEN
        RAISE EXCEPTION 'hold inventory balance does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.facility_id IS DISTINCT FROM balance_facility_id
       OR NEW.location_id IS DISTINCT FROM balance_location_id
       OR NEW.license_plate_id IS DISTINCT FROM balance_license_plate_id
       OR NEW.item_batch_id IS DISTINCT FROM balance_item_batch_id
       OR NEW.item_id IS DISTINCT FROM balance_item_id
       OR NEW.uom IS DISTINCT FROM balance_uom
       OR NEW.inventory_status IS DISTINCT FROM balance_status
    THEN
        RAISE EXCEPTION
            'hold dimensions must match its inventory balance'
            USING ERRCODE = '23514';
    END IF;

    SELECT COALESCE(SUM(hold.qty), 0)::BIGINT
    INTO active_hold_qty
    FROM public.inventory_holds hold
    WHERE hold.tenant_id = NEW.tenant_id
      AND hold.inventory_owner_id = NEW.inventory_owner_id
      AND hold.inventory_balance_id = NEW.inventory_balance_id
      AND hold.deleted IS NULL
      AND hold.status = 'active';

    IF balance_held IS DISTINCT FROM active_hold_qty THEN
        RAISE EXCEPTION
            'inventory balance held projection does not match active holds'
            USING ERRCODE = '55000';
    END IF;

    IF TG_OP = 'INSERT'
       AND balance_on_hand - balance_reserved - balance_held < NEW.qty
    THEN
        RAISE EXCEPTION
            'insufficient uncommitted inventory to hold'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE FUNCTION apply_inventory_hold_projection()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    projection_delta BIGINT := 0;
    affected_rows BIGINT;
BEGIN
    IF TG_OP = 'INSERT' THEN
        projection_delta := NEW.qty;
    ELSIF OLD.status = 'active'
          AND OLD.deleted IS NULL
          AND NEW.status = 'released'
          AND NEW.deleted IS NOT NULL
    THEN
        projection_delta := -OLD.qty;
    END IF;

    IF projection_delta <> 0 THEN
        UPDATE public.inventory_balances
        SET qty_held = qty_held + projection_delta,
            modified = COALESCE(NEW.modified, NEW.created)
        WHERE tenant_id = NEW.tenant_id
          AND inventory_owner_id = NEW.inventory_owner_id
          AND id = NEW.inventory_balance_id
          AND qty_held + projection_delta >= 0
          AND qty_reserved + qty_held + projection_delta <= qty_on_hand;
        GET DIAGNOSTICS affected_rows = ROW_COUNT;
        IF affected_rows <> 1 THEN
            RAISE EXCEPTION
                'inventory hold projection could not be updated'
                USING ERRCODE = '55000';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

CREATE FUNCTION inventory_hold_cannot_be_deleted()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
BEGIN
    RAISE EXCEPTION 'inventory holds are retained permanently'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER inventory_holds_require_active_owner_facility
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, facility_id
    ON inventory_holds
    FOR EACH ROW
    EXECUTE FUNCTION require_active_inventory_owner_facility();

CREATE TRIGGER inventory_holds_validate
    BEFORE INSERT OR UPDATE
    ON inventory_holds
    FOR EACH ROW
    EXECUTE FUNCTION validate_inventory_hold();

CREATE TRIGGER inventory_holds_apply_projection
    AFTER INSERT OR UPDATE
    ON inventory_holds
    FOR EACH ROW
    EXECUTE FUNCTION apply_inventory_hold_projection();

CREATE TRIGGER inventory_holds_prevent_delete
    BEFORE DELETE
    ON inventory_holds
    FOR EACH ROW
    EXECUTE FUNCTION inventory_hold_cannot_be_deleted();

CREATE OR REPLACE FUNCTION protect_inventory_owner_facility_assignment()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    referenced_tenant_id BIGINT := OLD.tenant_id;
    referenced_inventory_owner_id BIGINT := OLD.inventory_owner_id;
    referenced_facility_id BIGINT := OLD.facility_id;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
            OR NEW.inventory_owner_id IS DISTINCT FROM OLD.inventory_owner_id
            OR NEW.facility_id IS DISTINCT FROM OLD.facility_id
        THEN
            RAISE EXCEPTION 'inventory owner facility dimensions are immutable'
                USING ERRCODE = '55000';
        END IF;

        IF OLD.deleted IS NOT NULL OR NEW.deleted IS NULL THEN
            RETURN NEW;
        END IF;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.inventory_holds hold
        WHERE hold.tenant_id = referenced_tenant_id
          AND hold.inventory_owner_id = referenced_inventory_owner_id
          AND hold.facility_id = referenced_facility_id
          AND hold.deleted IS NULL
          AND hold.status = 'active'
    ) THEN
        RAISE EXCEPTION
            'inventory owner facility assignment has active holds'
            USING ERRCODE = '55000';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.inventory_balances balance
        WHERE balance.tenant_id = referenced_tenant_id
          AND balance.inventory_owner_id = referenced_inventory_owner_id
          AND balance.facility_id = referenced_facility_id
          AND (
              balance.qty_on_hand > 0
              OR balance.qty_reserved > 0
              OR balance.qty_held > 0
          )
    ) THEN
        RAISE EXCEPTION
            'inventory owner facility assignment has committed inventory'
            USING ERRCODE = '55000';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.inventory_reservations reservation
        WHERE reservation.tenant_id = referenced_tenant_id
          AND reservation.inventory_owner_id =
              referenced_inventory_owner_id
          AND reservation.facility_id = referenced_facility_id
          AND reservation.deleted IS NULL
          AND reservation.status = 'active'
    ) THEN
        RAISE EXCEPTION
            'inventory owner facility assignment has active reservations'
            USING ERRCODE = '55000';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.license_plates license_plate
        WHERE license_plate.tenant_id = referenced_tenant_id
          AND license_plate.inventory_owner_id =
              referenced_inventory_owner_id
          AND license_plate.facility_id = referenced_facility_id
          AND license_plate.deleted IS NULL
    ) THEN
        RAISE EXCEPTION
            'inventory owner facility assignment has active license plates'
            USING ERRCODE = '55000';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.work_tasks task
        WHERE task.tenant_id = referenced_tenant_id
          AND task.inventory_owner_id = referenced_inventory_owner_id
          AND task.facility_id = referenced_facility_id
          AND task.deleted IS NULL
          AND task.status IN ('open', 'assigned', 'in_progress')
    ) THEN
        RAISE EXCEPTION
            'inventory owner facility assignment has executable work'
            USING ERRCODE = '55000';
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE VIEW inventory_hold_reconciliation
WITH (security_invoker = true)
AS
SELECT balance.id AS inventory_balance_id,
       balance.tenant_id,
       balance.inventory_owner_id,
       balance.facility_id,
       balance.location_id,
       balance.license_plate_id,
       balance.item_batch_id,
       balance.item_id,
       balance.uom,
       balance.status AS inventory_status,
       balance.qty_on_hand,
       balance.qty_reserved,
       allocation_projection.allocated_qty,
       balance.qty_held,
       hold_projection.held_qty,
       GREATEST(
           allocation_projection.allocated_qty + hold_projection.held_qty
               - balance.qty_on_hand,
           0
       ) AS overcommitted_qty,
       ARRAY_REMOVE(
           ARRAY[
               CASE
                   WHEN balance.qty_reserved IS DISTINCT FROM
                       allocation_projection.allocated_qty
                   THEN 'allocation_projection_mismatch'
               END,
               CASE
                   WHEN balance.qty_held IS DISTINCT FROM
                       hold_projection.held_qty
                   THEN 'hold_projection_mismatch'
               END,
               CASE
                   WHEN allocation_projection.allocated_qty
                            + hold_projection.held_qty >
                        balance.qty_on_hand
                   THEN 'commitments_exceed_on_hand'
               END
           ],
           NULL
       ) AS issue_codes
FROM inventory_balances balance
CROSS JOIN LATERAL (
    SELECT COALESCE(SUM(allocation.qty), 0)::BIGINT AS allocated_qty
    FROM inventory_allocations allocation
    WHERE allocation.tenant_id = balance.tenant_id
      AND allocation.inventory_owner_id = balance.inventory_owner_id
      AND allocation.inventory_balance_id = balance.id
      AND allocation.deleted IS NULL
      AND allocation.status = 'allocated'
) allocation_projection
CROSS JOIN LATERAL (
    SELECT COALESCE(SUM(hold.qty), 0)::BIGINT AS held_qty
    FROM inventory_holds hold
    WHERE hold.tenant_id = balance.tenant_id
      AND hold.inventory_owner_id = balance.inventory_owner_id
      AND hold.inventory_balance_id = balance.id
      AND hold.deleted IS NULL
      AND hold.status = 'active'
) hold_projection
WHERE balance.qty_reserved IS DISTINCT FROM
          allocation_projection.allocated_qty
   OR balance.qty_held IS DISTINCT FROM hold_projection.held_qty
   OR allocation_projection.allocated_qty + hold_projection.held_qty >
          balance.qty_on_hand;

REVOKE ALL ON inventory_holds FROM PUBLIC, wareboxes_app;
GRANT SELECT, INSERT, UPDATE ON inventory_holds TO wareboxes_app;

REVOKE ALL ON SEQUENCE inventory_holds_id_seq
FROM PUBLIC, wareboxes_app;
GRANT USAGE ON SEQUENCE inventory_holds_id_seq TO wareboxes_app;

REVOKE ALL ON inventory_hold_reconciliation
FROM PUBLIC, wareboxes_app;
GRANT SELECT ON inventory_hold_reconciliation TO wareboxes_app;

REVOKE ALL ON FUNCTION
    guard_inventory_balance_commitments(),
    validate_inventory_allocation(),
    validate_item_location_cycle_count_result(),
    validate_inventory_hold(),
    apply_inventory_hold_projection(),
    inventory_hold_cannot_be_deleted(),
    protect_inventory_owner_facility_assignment()
FROM PUBLIC, wareboxes_app;
