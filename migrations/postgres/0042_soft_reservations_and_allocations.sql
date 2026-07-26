DROP TRIGGER IF EXISTS inventory_balances_guard_active_reservation_dimensions
    ON inventory_balances;
DROP TRIGGER IF EXISTS inventory_balances_verify_active_reservation_dimensions
    ON inventory_balances;
DROP TRIGGER IF EXISTS inventory_reservations_match_balance
    ON inventory_reservations;
DROP FUNCTION IF EXISTS guard_active_reservation_balance_dimensions();
DROP FUNCTION IF EXISTS inventory_reservation_matches_balance();

DROP TABLE inventory_reservations CASCADE;

CREATE TABLE inventory_reservations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    modified TIMESTAMPTZ,
    deleted TIMESTAMPTZ,
    created_by BIGINT NOT NULL,
    order_id BIGINT NOT NULL,
    order_item_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    uom TEXT NOT NULL,
    qty BIGINT NOT NULL CHECK (qty > 0),
    status TEXT NOT NULL DEFAULT 'active',
    CHECK (status IN ('active', 'cancelled', 'fulfilled')),
    CHECK (BTRIM(uom) <> ''),
    CHECK (
        (status = 'active' AND deleted IS NULL)
        OR (status IN ('cancelled', 'fulfilled') AND deleted IS NOT NULL)
    ),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, created_by)
        REFERENCES tenant_memberships(tenant_id, user_id),
    FOREIGN KEY (tenant_id, inventory_owner_id)
        REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(
            tenant_id,
            inventory_owner_id,
            facility_id
        ),
    FOREIGN KEY (tenant_id, inventory_owner_id, order_id)
        REFERENCES orders(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        order_id,
        order_item_id
    ) REFERENCES order_items(
        tenant_id,
        inventory_owner_id,
        order_id,
        id
    ),
    FOREIGN KEY (tenant_id, facility_id)
        REFERENCES facilities(tenant_id, id),
    FOREIGN KEY (tenant_id, item_id)
        REFERENCES items(tenant_id, id)
);

CREATE INDEX inventory_reservations_order_line_idx
    ON inventory_reservations(
        tenant_id,
        inventory_owner_id,
        order_id,
        order_item_id
    );
CREATE INDEX inventory_reservations_active_demand_idx
    ON inventory_reservations(
        tenant_id,
        inventory_owner_id,
        facility_id,
        item_id,
        uom
    )
    WHERE deleted IS NULL AND status = 'active';

CREATE TABLE inventory_allocations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    modified TIMESTAMPTZ,
    deleted TIMESTAMPTZ,
    created_by BIGINT NOT NULL,
    reservation_id BIGINT NOT NULL,
    inventory_balance_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    location_id BIGINT NOT NULL,
    license_plate_id BIGINT,
    item_batch_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    uom TEXT NOT NULL,
    inventory_status TEXT NOT NULL,
    qty BIGINT NOT NULL CHECK (qty > 0),
    status TEXT NOT NULL DEFAULT 'allocated',
    CHECK (status IN ('allocated', 'released', 'fulfilled')),
    CHECK (inventory_status IN ('available', 'hold', 'damaged', 'quarantine')),
    CHECK (BTRIM(uom) <> ''),
    CHECK (
        (status = 'allocated' AND deleted IS NULL)
        OR (status IN ('released', 'fulfilled') AND deleted IS NOT NULL)
    ),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, created_by)
        REFERENCES tenant_memberships(tenant_id, user_id),
    FOREIGN KEY (tenant_id, inventory_owner_id)
        REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(
            tenant_id,
            inventory_owner_id,
            facility_id
        ),
    FOREIGN KEY (tenant_id, inventory_owner_id, reservation_id)
        REFERENCES inventory_reservations(
            tenant_id,
            inventory_owner_id,
            id
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

CREATE INDEX inventory_allocations_reservation_idx
    ON inventory_allocations(
        tenant_id,
        inventory_owner_id,
        reservation_id,
        status
    );
CREATE INDEX inventory_allocations_balance_idx
    ON inventory_allocations(
        tenant_id,
        inventory_owner_id,
        inventory_balance_id,
        status
    );
CREATE UNIQUE INDEX inventory_allocations_active_balance_reservation_key
    ON inventory_allocations(
        tenant_id,
        inventory_owner_id,
        reservation_id,
        inventory_balance_id
    )
    WHERE deleted IS NULL AND status = 'allocated';

ALTER TABLE inventory_reservations ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_reservations FORCE ROW LEVEL SECURITY;
CREATE POLICY inventory_reservations_tenant_isolation
    ON inventory_reservations
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE inventory_allocations ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_allocations FORCE ROW LEVEL SECURITY;
CREATE POLICY inventory_allocations_tenant_isolation
    ON inventory_allocations
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

CREATE FUNCTION validate_inventory_reservation()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    line_qty BIGINT;
    line_item_id BIGINT;
    line_uom TEXT;
    active_demand BIGINT;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
           OR NEW.inventory_owner_id IS DISTINCT FROM OLD.inventory_owner_id
           OR NEW.created IS DISTINCT FROM OLD.created
           OR NEW.created_by IS DISTINCT FROM OLD.created_by
           OR NEW.order_id IS DISTINCT FROM OLD.order_id
           OR NEW.order_item_id IS DISTINCT FROM OLD.order_item_id
           OR NEW.facility_id IS DISTINCT FROM OLD.facility_id
           OR NEW.item_id IS DISTINCT FROM OLD.item_id
           OR NEW.uom IS DISTINCT FROM OLD.uom
           OR NEW.qty IS DISTINCT FROM OLD.qty
        THEN
            RAISE EXCEPTION 'inventory reservation demand is immutable'
                USING ERRCODE = '55000';
        END IF;

        IF OLD.status <> 'active'
           AND (
               NEW.status IS DISTINCT FROM OLD.status
               OR NEW.deleted IS DISTINCT FROM OLD.deleted
               OR NEW.modified IS DISTINCT FROM OLD.modified
           )
        THEN
            RAISE EXCEPTION 'terminal inventory reservation is immutable'
                USING ERRCODE = '55000';
        END IF;

        IF OLD.status = 'active'
           AND NEW.status IN ('cancelled', 'fulfilled')
           AND EXISTS (
               SELECT 1
               FROM public.inventory_allocations allocation
               WHERE allocation.tenant_id = OLD.tenant_id
                 AND allocation.inventory_owner_id =
                     OLD.inventory_owner_id
                 AND allocation.reservation_id = OLD.id
                 AND allocation.deleted IS NULL
                 AND allocation.status = 'allocated'
           )
        THEN
            RAISE EXCEPTION
                'active allocations must be released before reservation closure'
                USING ERRCODE = '55000';
        END IF;

        RETURN NEW;
    END IF;

    IF NEW.status <> 'active' OR NEW.deleted IS NOT NULL THEN
        RAISE EXCEPTION 'new inventory reservation must be active'
            USING ERRCODE = '23514';
    END IF;

    SELECT order_line.qty, order_line.item_id, item.packaging_unit
    INTO line_qty, line_item_id, line_uom
    FROM public.order_items order_line
    INNER JOIN public.orders customer_order
        ON customer_order.tenant_id = order_line.tenant_id
       AND customer_order.inventory_owner_id =
           order_line.inventory_owner_id
       AND customer_order.id = order_line.order_id
       AND customer_order.deleted IS NULL
       AND customer_order.status NOT IN ('shipped', 'cancelled', 'void')
    INNER JOIN public.items item
        ON item.tenant_id = order_line.tenant_id
       AND item.id = order_line.item_id
       AND item.deleted IS NULL
    INNER JOIN public.inventory_owner_facilities assignment
        ON assignment.tenant_id = order_line.tenant_id
       AND assignment.inventory_owner_id =
           order_line.inventory_owner_id
       AND assignment.facility_id = NEW.facility_id
       AND assignment.deleted IS NULL
    WHERE order_line.tenant_id = NEW.tenant_id
      AND order_line.inventory_owner_id = NEW.inventory_owner_id
      AND order_line.order_id = NEW.order_id
      AND order_line.id = NEW.order_item_id
      AND order_line.deleted IS NULL
    FOR UPDATE OF order_line;

    IF line_qty IS NULL THEN
        RAISE EXCEPTION
            'reservation order line or owner-facility assignment is unavailable'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.item_id IS DISTINCT FROM line_item_id
       OR NEW.uom IS DISTINCT FROM line_uom
    THEN
        RAISE EXCEPTION
            'reservation item and UOM must match its order line'
            USING ERRCODE = '23514';
    END IF;

    SELECT COALESCE(SUM(reservation.qty), 0)::BIGINT
    INTO active_demand
    FROM public.inventory_reservations reservation
    WHERE reservation.tenant_id = NEW.tenant_id
      AND reservation.inventory_owner_id = NEW.inventory_owner_id
      AND reservation.order_id = NEW.order_id
      AND reservation.order_item_id = NEW.order_item_id
      AND reservation.deleted IS NULL
      AND reservation.status = 'active';

    IF active_demand + NEW.qty > line_qty THEN
        RAISE EXCEPTION
            'active reservations exceed order-line demand'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE FUNCTION validate_inventory_allocation()
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
           balance.qty_reserved
    INTO balance_facility_id, balance_location_id,
         balance_license_plate_id, balance_item_batch_id, balance_item_id,
         balance_uom, balance_status, balance_on_hand, balance_reserved
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

        IF balance_on_hand - balance_reserved < NEW.qty THEN
            RAISE EXCEPTION
                'insufficient available inventory to allocate'
                USING ERRCODE = '23514';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

CREATE FUNCTION apply_inventory_allocation_projection()
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
    ELSIF OLD.status = 'allocated'
          AND OLD.deleted IS NULL
          AND NEW.status IN ('released', 'fulfilled')
          AND NEW.deleted IS NOT NULL
    THEN
        projection_delta := -OLD.qty;
    END IF;

    IF projection_delta <> 0 THEN
        UPDATE public.inventory_balances
        SET qty_reserved = qty_reserved + projection_delta,
            modified = COALESCE(NEW.modified, NEW.created)
        WHERE tenant_id = NEW.tenant_id
          AND inventory_owner_id = NEW.inventory_owner_id
          AND id = NEW.inventory_balance_id
          AND qty_reserved + projection_delta >= 0
          AND qty_reserved + projection_delta <= qty_on_hand;
        GET DIAGNOSTICS affected_rows = ROW_COUNT;
        IF affected_rows <> 1 THEN
            RAISE EXCEPTION
                'inventory allocation projection could not be updated'
                USING ERRCODE = '55000';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

CREATE FUNCTION guard_inventory_balance_allocation_projection()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    allocated_qty BIGINT;
BEGIN
    SELECT COALESCE(SUM(allocation.qty), 0)::BIGINT
    INTO allocated_qty
    FROM public.inventory_allocations allocation
    WHERE allocation.tenant_id = NEW.tenant_id
      AND allocation.inventory_owner_id = NEW.inventory_owner_id
      AND allocation.inventory_balance_id = NEW.id
      AND allocation.deleted IS NULL
      AND allocation.status = 'allocated';

    IF NEW.qty_reserved IS DISTINCT FROM allocated_qty THEN
        RAISE EXCEPTION
            'inventory balance reserved projection does not match allocations'
            USING ERRCODE = '55000';
    END IF;

    IF allocated_qty > 0
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
            'allocated inventory balance dimensions are immutable'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE FUNCTION inventory_allocation_cannot_be_deleted()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
BEGIN
    RAISE EXCEPTION 'inventory allocations are retained permanently'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER inventory_reservations_validate
    BEFORE INSERT OR UPDATE
    ON inventory_reservations
    FOR EACH ROW
    EXECUTE FUNCTION validate_inventory_reservation();

CREATE TRIGGER inventory_reservations_require_active_owner_facility
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, facility_id
    ON inventory_reservations
    FOR EACH ROW
    EXECUTE FUNCTION require_active_inventory_owner_facility();

CREATE TRIGGER inventory_allocations_validate
    BEFORE INSERT OR UPDATE
    ON inventory_allocations
    FOR EACH ROW
    EXECUTE FUNCTION validate_inventory_allocation();

CREATE TRIGGER inventory_allocations_require_active_owner_facility
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, facility_id
    ON inventory_allocations
    FOR EACH ROW
    EXECUTE FUNCTION require_active_inventory_owner_facility();

CREATE TRIGGER inventory_allocations_apply_projection
    AFTER INSERT OR UPDATE
    ON inventory_allocations
    FOR EACH ROW
    EXECUTE FUNCTION apply_inventory_allocation_projection();

CREATE TRIGGER inventory_allocations_prevent_delete
    BEFORE DELETE
    ON inventory_allocations
    FOR EACH ROW
    EXECUTE FUNCTION inventory_allocation_cannot_be_deleted();

CREATE TRIGGER inventory_balances_guard_allocation_projection
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, facility_id,
        location_id, license_plate_id, item_batch_id, item_id, uom, status,
        qty_reserved, deleted
    ON inventory_balances
    FOR EACH ROW
    EXECUTE FUNCTION guard_inventory_balance_allocation_projection();

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
        FROM public.inventory_balances balance
        WHERE balance.tenant_id = referenced_tenant_id
          AND balance.inventory_owner_id = referenced_inventory_owner_id
          AND balance.facility_id = referenced_facility_id
          AND (balance.qty_on_hand > 0 OR balance.qty_reserved > 0)
    ) THEN
        RAISE EXCEPTION
            'inventory owner facility assignment has positive or reserved inventory'
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

REVOKE ALL ON inventory_reservations, inventory_allocations
FROM PUBLIC, wareboxes_app;
GRANT SELECT, INSERT, UPDATE
ON inventory_reservations, inventory_allocations
TO wareboxes_app;

REVOKE ALL ON SEQUENCE
    inventory_reservations_id_seq,
    inventory_allocations_id_seq
FROM PUBLIC, wareboxes_app;
GRANT USAGE ON SEQUENCE
    inventory_reservations_id_seq,
    inventory_allocations_id_seq
TO wareboxes_app;

REVOKE ALL ON FUNCTION
    validate_inventory_reservation(),
    validate_inventory_allocation(),
    apply_inventory_allocation_projection(),
    guard_inventory_balance_allocation_projection(),
    inventory_allocation_cannot_be_deleted()
FROM PUBLIC, wareboxes_app;
