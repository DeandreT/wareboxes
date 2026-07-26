ALTER TABLE work_tasks
    DROP CONSTRAINT work_tasks_task_type_check,
    ADD CONSTRAINT work_tasks_task_type_check CHECK (
        task_type IN (
            'cycle_count_item_location',
            'cycle_count_location',
            'break_master_pack',
            'unpack_cancelled_order',
            'putaway',
            'license_plate_putaway'
        )
    ),
    DROP CONSTRAINT work_tasks_required_dimensions_check,
    ADD CONSTRAINT work_tasks_required_dimensions_check CHECK (
        (
            task_type IN (
                'cycle_count_item_location',
                'unpack_cancelled_order',
                'putaway',
                'license_plate_putaway'
            )
            AND facility_id IS NOT NULL
            AND inventory_owner_id IS NOT NULL
        )
        OR (
            task_type IN (
                'cycle_count_location',
                'break_master_pack'
            )
            AND facility_id IS NOT NULL
        )
    );

ALTER TABLE work_task_progress
    DROP CONSTRAINT work_task_progress_action_check,
    ADD CONSTRAINT work_task_progress_action_check CHECK (
        action IN (
            'started',
            'aborted',
            'expired',
            'scope_revoked',
            'completed',
            'cancelled',
            'progress',
            'unpacked',
            'missing',
            'damaged',
            'moved',
            'cycle_count_confirmed',
            'putaway_confirmed',
            'license_plate_putaway_confirmed'
        )
    );

CREATE TABLE license_plate_putaway_tasks (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    license_plate_id BIGINT NOT NULL,
    source_location_id BIGINT NOT NULL,
    destination_location_id BIGINT NOT NULL,
    planned_balance_count BIGINT NOT NULL,
    closed_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, task_id),
    UNIQUE (
        tenant_id,
        task_id,
        inventory_owner_id,
        facility_id,
        license_plate_id
    ),
    UNIQUE (
        tenant_id,
        task_id,
        inventory_owner_id,
        facility_id,
        license_plate_id,
        source_location_id,
        destination_location_id,
        planned_balance_count
    ),
    FOREIGN KEY (tenant_id, facility_id, task_id)
        REFERENCES work_tasks(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, task_id)
        REFERENCES work_tasks(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(
            tenant_id,
            inventory_owner_id,
            facility_id
        ),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        license_plate_id
    )
        REFERENCES license_plates(
            tenant_id,
            inventory_owner_id,
            facility_id,
            id
        ),
    FOREIGN KEY (tenant_id, facility_id, source_location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, facility_id, destination_location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    CHECK (source_location_id <> destination_location_id),
    CHECK (planned_balance_count > 0)
);

CREATE UNIQUE INDEX license_plate_putaway_tasks_one_active_plate_idx
    ON license_plate_putaway_tasks(
        tenant_id,
        inventory_owner_id,
        license_plate_id
    )
    WHERE closed_at IS NULL;

CREATE INDEX license_plate_putaway_tasks_destination_idx
    ON license_plate_putaway_tasks(
        tenant_id,
        inventory_owner_id,
        facility_id,
        destination_location_id
    )
    WHERE closed_at IS NULL;

CREATE TABLE license_plate_putaway_task_contents (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    license_plate_id BIGINT NOT NULL,
    inventory_balance_id BIGINT NOT NULL,
    item_batch_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    uom TEXT NOT NULL,
    inventory_status TEXT NOT NULL,
    planned_quantity BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, task_id, inventory_balance_id),
    FOREIGN KEY (
        tenant_id,
        task_id,
        inventory_owner_id,
        facility_id,
        license_plate_id
    )
        REFERENCES license_plate_putaway_tasks(
            tenant_id,
            task_id,
            inventory_owner_id,
            facility_id,
            license_plate_id
        ),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        inventory_balance_id
    )
        REFERENCES inventory_balances(
            tenant_id,
            inventory_owner_id,
            facility_id,
            id
        ),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id)
        REFERENCES item_batches(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, item_id)
        REFERENCES items(tenant_id, id),
    CHECK (btrim(uom) <> ''),
    CHECK (inventory_status = 'available'),
    CHECK (planned_quantity > 0)
);

CREATE INDEX license_plate_putaway_task_contents_plate_idx
    ON license_plate_putaway_task_contents(
        tenant_id,
        inventory_owner_id,
        facility_id,
        license_plate_id
    );

CREATE TABLE license_plate_putaway_results (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    license_plate_id BIGINT NOT NULL,
    license_plate_barcode TEXT NOT NULL,
    source_location_id BIGINT NOT NULL,
    destination_location_id BIGINT NOT NULL,
    destination_location_barcode TEXT NOT NULL,
    inventory_transaction_id BIGINT NOT NULL,
    moved_balance_count BIGINT NOT NULL,
    confirmed_by BIGINT NOT NULL,
    confirmed_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, task_id),
    UNIQUE (tenant_id, inventory_owner_id, inventory_transaction_id),
    FOREIGN KEY (
        tenant_id,
        task_id,
        inventory_owner_id,
        facility_id,
        license_plate_id,
        source_location_id,
        destination_location_id,
        moved_balance_count
    )
        REFERENCES license_plate_putaway_tasks(
            tenant_id,
            task_id,
            inventory_owner_id,
            facility_id,
            license_plate_id,
            source_location_id,
            destination_location_id,
            planned_balance_count
        ),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        license_plate_id
    )
        REFERENCES license_plates(
            tenant_id,
            inventory_owner_id,
            facility_id,
            id
        ),
    FOREIGN KEY (tenant_id, facility_id, source_location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, facility_id, destination_location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, inventory_transaction_id)
        REFERENCES inventory_transactions(
            tenant_id,
            inventory_owner_id,
            id
        ),
    FOREIGN KEY (tenant_id, confirmed_by)
        REFERENCES tenant_memberships(tenant_id, user_id),
    CHECK (btrim(license_plate_barcode) <> ''),
    CHECK (btrim(destination_location_barcode) <> ''),
    CHECK (source_location_id <> destination_location_id),
    CHECK (moved_balance_count > 0)
);

ALTER TABLE license_plate_putaway_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE license_plate_putaway_tasks FORCE ROW LEVEL SECURITY;

CREATE POLICY license_plate_putaway_tasks_tenant_isolation
    ON license_plate_putaway_tasks
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE license_plate_putaway_task_contents ENABLE ROW LEVEL SECURITY;
ALTER TABLE license_plate_putaway_task_contents FORCE ROW LEVEL SECURITY;

CREATE POLICY license_plate_putaway_task_contents_tenant_isolation
    ON license_plate_putaway_task_contents
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE license_plate_putaway_results ENABLE ROW LEVEL SECURITY;
ALTER TABLE license_plate_putaway_results FORCE ROW LEVEL SECURITY;

CREATE POLICY license_plate_putaway_results_tenant_isolation
    ON license_plate_putaway_results
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

CREATE FUNCTION validate_license_plate_putaway_task()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    task_row RECORD;
    plate_row RECORD;
    source_is_active BOOLEAN;
    destination_is_active BOOLEAN;
    owner_facility_is_active BOOLEAN;
    current_balance_count BIGINT;
    snapshot_balance_count BIGINT;
BEGIN
    SELECT task.task_type,
           task.status,
           task.deleted,
           task.inventory_owner_id,
           task.facility_id
    INTO task_row
    FROM public.work_tasks task
    WHERE task.tenant_id = NEW.tenant_id
      AND task.id = NEW.task_id
    FOR SHARE;

    IF NOT FOUND
       OR task_row.task_type <> 'license_plate_putaway'
       OR task_row.status NOT IN ('open', 'assigned')
       OR task_row.deleted IS NOT NULL
       OR task_row.inventory_owner_id
            IS DISTINCT FROM NEW.inventory_owner_id
       OR task_row.facility_id IS DISTINCT FROM NEW.facility_id
       OR NEW.closed_at IS NOT NULL
    THEN
        RAISE EXCEPTION
            'license plate putaway detail does not match an active task'
            USING ERRCODE = '55000';
    END IF;

    SELECT plate.inventory_owner_id,
           plate.facility_id,
           plate.location_id,
           plate.barcode,
           plate.deleted
    INTO plate_row
    FROM public.license_plates plate
    WHERE plate.tenant_id = NEW.tenant_id
      AND plate.id = NEW.license_plate_id
    FOR SHARE;

    IF NOT FOUND
       OR plate_row.deleted IS NOT NULL
       OR plate_row.inventory_owner_id
            IS DISTINCT FROM NEW.inventory_owner_id
       OR plate_row.facility_id IS DISTINCT FROM NEW.facility_id
       OR plate_row.location_id IS DISTINCT FROM NEW.source_location_id
       OR plate_row.barcode IS NULL
       OR btrim(plate_row.barcode) = ''
    THEN
        RAISE EXCEPTION
            'license plate putaway source plate is not active at its source'
            USING ERRCODE = '55000';
    END IF;

    SELECT EXISTS (
        SELECT 1
        FROM public.locations location
        WHERE location.tenant_id = NEW.tenant_id
          AND location.facility_id = NEW.facility_id
          AND location.id = NEW.source_location_id
          AND location.deleted IS NULL
          AND location.active
          AND location.receivable
    )
    INTO source_is_active;

    SELECT EXISTS (
        SELECT 1
        FROM public.locations location
        WHERE location.tenant_id = NEW.tenant_id
          AND location.facility_id = NEW.facility_id
          AND location.id = NEW.destination_location_id
          AND location.deleted IS NULL
          AND location.active
          AND NOT location.receivable
          AND location.barcode IS NOT NULL
          AND btrim(location.barcode) <> ''
    )
    INTO destination_is_active;

    SELECT EXISTS (
        SELECT 1
        FROM public.inventory_owner_facilities assignment
        WHERE assignment.tenant_id = NEW.tenant_id
          AND assignment.inventory_owner_id = NEW.inventory_owner_id
          AND assignment.facility_id = NEW.facility_id
          AND assignment.deleted IS NULL
    )
    INTO owner_facility_is_active;

    IF NOT source_is_active
       OR NOT destination_is_active
       OR NOT owner_facility_is_active
    THEN
        RAISE EXCEPTION
            'license plate putaway locations and owner facility assignment must be active'
            USING ERRCODE = '55000';
    END IF;

    SELECT COUNT(*)
    INTO current_balance_count
    FROM public.inventory_balances balance
    WHERE balance.tenant_id = NEW.tenant_id
      AND balance.inventory_owner_id = NEW.inventory_owner_id
      AND balance.facility_id = NEW.facility_id
      AND balance.license_plate_id = NEW.license_plate_id
      AND balance.deleted IS NULL
      AND balance.qty_on_hand > 0;

    SELECT COUNT(*)
    INTO snapshot_balance_count
    FROM public.license_plate_putaway_task_contents content
    WHERE content.tenant_id = NEW.tenant_id
      AND content.task_id = NEW.task_id;

    IF current_balance_count <> NEW.planned_balance_count
       OR snapshot_balance_count <> NEW.planned_balance_count
       OR EXISTS (
            SELECT 1
            FROM public.inventory_balances balance
            WHERE balance.tenant_id = NEW.tenant_id
              AND balance.inventory_owner_id = NEW.inventory_owner_id
              AND balance.facility_id = NEW.facility_id
              AND balance.license_plate_id = NEW.license_plate_id
              AND balance.deleted IS NULL
              AND balance.qty_on_hand > 0
              AND (
                  balance.location_id <> NEW.source_location_id
                  OR balance.status <> 'available'
                  OR balance.qty_reserved <> 0
                  OR balance.qty_held <> 0
                  OR NOT EXISTS (
                      SELECT 1
                      FROM public.license_plate_putaway_task_contents content
                      WHERE content.tenant_id = NEW.tenant_id
                        AND content.task_id = NEW.task_id
                        AND content.inventory_owner_id =
                            NEW.inventory_owner_id
                        AND content.facility_id = NEW.facility_id
                        AND content.license_plate_id =
                            NEW.license_plate_id
                        AND content.inventory_balance_id = balance.id
                        AND content.item_batch_id = balance.item_batch_id
                        AND content.item_id = balance.item_id
                        AND content.uom = balance.uom
                        AND content.inventory_status = balance.status
                        AND content.planned_quantity =
                            balance.qty_on_hand
                  )
              )
       )
       OR EXISTS (
            SELECT 1
            FROM public.license_plate_putaway_task_contents content
            WHERE content.tenant_id = NEW.tenant_id
              AND content.task_id = NEW.task_id
              AND (
                  content.inventory_owner_id <>
                      NEW.inventory_owner_id
                  OR content.facility_id <> NEW.facility_id
                  OR content.license_plate_id <>
                      NEW.license_plate_id
                  OR NOT EXISTS (
                      SELECT 1
                      FROM public.inventory_balances balance
                      WHERE balance.tenant_id = NEW.tenant_id
                        AND balance.inventory_owner_id =
                            NEW.inventory_owner_id
                        AND balance.facility_id = NEW.facility_id
                        AND balance.license_plate_id =
                            NEW.license_plate_id
                        AND balance.id =
                            content.inventory_balance_id
                        AND balance.location_id =
                            NEW.source_location_id
                        AND balance.item_batch_id =
                            content.item_batch_id
                        AND balance.item_id = content.item_id
                        AND balance.uom = content.uom
                        AND balance.status =
                            content.inventory_status
                        AND balance.qty_on_hand =
                            content.planned_quantity
                        AND balance.qty_reserved = 0
                        AND balance.qty_held = 0
                        AND balance.deleted IS NULL
                  )
              )
       )
    THEN
        RAISE EXCEPTION
            'license plate putaway snapshot must exactly match movable plate contents'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE CONSTRAINT TRIGGER license_plate_putaway_tasks_are_valid
    AFTER INSERT ON license_plate_putaway_tasks
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION validate_license_plate_putaway_task();

CREATE FUNCTION require_license_plate_putaway_task_detail()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    detail_count BIGINT;
BEGIN
    IF NEW.task_type = 'license_plate_putaway' THEN
        SELECT COUNT(*)
        INTO detail_count
        FROM public.license_plate_putaway_tasks detail
        WHERE detail.tenant_id = NEW.tenant_id
          AND detail.task_id = NEW.id
          AND detail.inventory_owner_id =
              NEW.inventory_owner_id
          AND detail.facility_id = NEW.facility_id;

        IF detail_count <> 1 THEN
            RAISE EXCEPTION
                'license plate putaway work task requires exactly one scoped detail'
                USING ERRCODE = '55000';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

CREATE CONSTRAINT TRIGGER work_tasks_require_license_plate_putaway_detail
    AFTER INSERT OR UPDATE ON work_tasks
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION require_license_plate_putaway_task_detail();

CREATE FUNCTION guard_license_plate_putaway_task_mutation()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF TG_OP = 'DELETE'
       OR OLD.tenant_id IS DISTINCT FROM NEW.tenant_id
       OR OLD.task_id IS DISTINCT FROM NEW.task_id
       OR OLD.inventory_owner_id IS DISTINCT FROM NEW.inventory_owner_id
       OR OLD.facility_id IS DISTINCT FROM NEW.facility_id
       OR OLD.license_plate_id IS DISTINCT FROM NEW.license_plate_id
       OR OLD.source_location_id IS DISTINCT FROM NEW.source_location_id
       OR OLD.destination_location_id
            IS DISTINCT FROM NEW.destination_location_id
       OR OLD.planned_balance_count
            IS DISTINCT FROM NEW.planned_balance_count
       OR OLD.closed_at IS NOT NULL
       OR NEW.closed_at IS NULL
    THEN
        RAISE EXCEPTION
            'license plate putaway task snapshots are immutable'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER license_plate_putaway_tasks_guard_mutation
    BEFORE UPDATE OR DELETE ON license_plate_putaway_tasks
    FOR EACH ROW
    EXECUTE FUNCTION guard_license_plate_putaway_task_mutation();

CREATE FUNCTION reject_license_plate_putaway_content_mutation()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    RAISE EXCEPTION
        'license plate putaway content snapshots are immutable'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER license_plate_putaway_task_contents_are_immutable
    BEFORE UPDATE OR DELETE ON license_plate_putaway_task_contents
    FOR EACH ROW
    EXECUTE FUNCTION reject_license_plate_putaway_content_mutation();

CREATE FUNCTION close_license_plate_putaway_task_detail()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF NEW.task_type = 'license_plate_putaway'
       AND (
           NEW.deleted IS NOT NULL
           OR NEW.status IN ('completed', 'cancelled')
       )
    THEN
        UPDATE public.license_plate_putaway_tasks
        SET closed_at = COALESCE(
            NEW.completed_at,
            NEW.deleted,
            statement_timestamp()
        )
        WHERE tenant_id = NEW.tenant_id
          AND task_id = NEW.id
          AND closed_at IS NULL;
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER work_tasks_close_license_plate_putaway_detail
    AFTER UPDATE OF status, deleted ON work_tasks
    FOR EACH ROW
    EXECUTE FUNCTION close_license_plate_putaway_task_detail();

CREATE FUNCTION reject_license_plate_putaway_result_mutation()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    RAISE EXCEPTION 'license plate putaway results are immutable'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER license_plate_putaway_results_are_immutable
    BEFORE UPDATE OR DELETE ON license_plate_putaway_results
    FOR EACH ROW
    EXECUTE FUNCTION reject_license_plate_putaway_result_mutation();

CREATE FUNCTION validate_license_plate_putaway_result()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    task_row RECORD;
    plate_row RECORD;
    destination_row RECORD;
    plate_found BOOLEAN;
    destination_found BOOLEAN;
    transaction_matches BOOLEAN;
    snapshot_balance_count BIGINT;
    transaction_entry_count BIGINT;
BEGIN
    SELECT task.status,
           task.deleted,
           task.assigned_user_id,
           task.completed_by,
           task.completed_at,
           detail.closed_at
    INTO task_row
    FROM public.work_tasks task
    INNER JOIN public.license_plate_putaway_tasks detail
        ON detail.tenant_id = task.tenant_id
       AND detail.task_id = task.id
    WHERE task.tenant_id = NEW.tenant_id
      AND task.id = NEW.task_id
      AND task.task_type = 'license_plate_putaway';

    IF NOT FOUND
       OR task_row.status <> 'completed'
       OR task_row.deleted IS NOT NULL
       OR task_row.assigned_user_id IS DISTINCT FROM NEW.confirmed_by
       OR task_row.completed_by IS DISTINCT FROM NEW.confirmed_by
       OR task_row.completed_at IS DISTINCT FROM NEW.confirmed_at
       OR task_row.closed_at IS DISTINCT FROM NEW.confirmed_at
    THEN
        RAISE EXCEPTION
            'license plate putaway result does not match its completed task'
            USING ERRCODE = '55000';
    END IF;

    SELECT plate.inventory_owner_id,
           plate.facility_id,
           plate.location_id,
           plate.barcode,
           plate.deleted
    INTO plate_row
    FROM public.license_plates plate
    WHERE plate.tenant_id = NEW.tenant_id
      AND plate.id = NEW.license_plate_id;
    plate_found := FOUND;

    SELECT location.facility_id,
           location.barcode,
           location.active,
           location.receivable,
           location.deleted
    INTO destination_row
    FROM public.locations location
    WHERE location.tenant_id = NEW.tenant_id
      AND location.id = NEW.destination_location_id;
    destination_found := FOUND;

    IF NOT plate_found
       OR NOT destination_found
       OR plate_row.deleted IS NOT NULL
       OR plate_row.inventory_owner_id
            IS DISTINCT FROM NEW.inventory_owner_id
       OR plate_row.facility_id IS DISTINCT FROM NEW.facility_id
       OR plate_row.location_id
            IS DISTINCT FROM NEW.destination_location_id
       OR plate_row.barcode IS DISTINCT FROM NEW.license_plate_barcode
       OR destination_row.deleted IS NOT NULL
       OR NOT destination_row.active
       OR destination_row.receivable
       OR destination_row.facility_id IS DISTINCT FROM NEW.facility_id
       OR destination_row.barcode
            IS DISTINCT FROM NEW.destination_location_barcode
    THEN
        RAISE EXCEPTION
            'license plate putaway result does not match its destination'
            USING ERRCODE = '55000';
    END IF;

    SELECT COUNT(*)
    INTO snapshot_balance_count
    FROM public.license_plate_putaway_task_contents content
    WHERE content.tenant_id = NEW.tenant_id
      AND content.task_id = NEW.task_id;

    IF snapshot_balance_count <> NEW.moved_balance_count
       OR EXISTS (
            SELECT 1
            FROM public.license_plate_putaway_task_contents content
            WHERE content.tenant_id = NEW.tenant_id
              AND content.task_id = NEW.task_id
              AND NOT EXISTS (
                  SELECT 1
                  FROM public.inventory_balances balance
                  WHERE balance.tenant_id = NEW.tenant_id
                    AND balance.inventory_owner_id =
                        NEW.inventory_owner_id
                    AND balance.facility_id = NEW.facility_id
                    AND balance.license_plate_id =
                        NEW.license_plate_id
                    AND balance.id = content.inventory_balance_id
                    AND balance.location_id =
                        NEW.destination_location_id
                    AND balance.item_batch_id =
                        content.item_batch_id
                    AND balance.item_id = content.item_id
                    AND balance.uom = content.uom
                    AND balance.status =
                        content.inventory_status
                    AND balance.qty_on_hand =
                        content.planned_quantity
                    AND balance.qty_reserved = 0
                    AND balance.qty_held = 0
                    AND balance.deleted IS NULL
              )
       )
       OR EXISTS (
            SELECT 1
            FROM public.inventory_balances balance
            WHERE balance.tenant_id = NEW.tenant_id
              AND balance.inventory_owner_id = NEW.inventory_owner_id
              AND balance.facility_id = NEW.facility_id
              AND balance.license_plate_id = NEW.license_plate_id
              AND balance.deleted IS NULL
              AND balance.qty_on_hand > 0
              AND NOT EXISTS (
                  SELECT 1
                  FROM public.license_plate_putaway_task_contents content
                  WHERE content.tenant_id = NEW.tenant_id
                    AND content.task_id = NEW.task_id
                    AND content.inventory_balance_id = balance.id
                    AND content.item_batch_id = balance.item_batch_id
                    AND content.item_id = balance.item_id
                    AND content.uom = balance.uom
                    AND content.inventory_status = balance.status
                    AND content.planned_quantity =
                        balance.qty_on_hand
              )
       )
    THEN
        RAISE EXCEPTION
            'license plate putaway result does not match its content snapshot'
            USING ERRCODE = '55000';
    END IF;

    SELECT EXISTS (
        SELECT 1
        FROM public.inventory_transactions transaction
        WHERE transaction.tenant_id = NEW.tenant_id
          AND transaction.inventory_owner_id = NEW.inventory_owner_id
          AND transaction.id = NEW.inventory_transaction_id
          AND transaction.transaction_type = 'move'
          AND transaction.actor_user_id = NEW.confirmed_by
          AND transaction.reference_type =
              'license_plate_putaway_task'
          AND transaction.reference_id = NEW.task_id
          AND transaction.operation =
              'task.confirm_license_plate_putaway.v1'
    )
    INTO transaction_matches;

    SELECT COUNT(*)
    INTO transaction_entry_count
    FROM public.inventory_entries entry
    WHERE entry.tenant_id = NEW.tenant_id
      AND entry.inventory_owner_id = NEW.inventory_owner_id
      AND entry.transaction_id = NEW.inventory_transaction_id;

    IF NOT transaction_matches
       OR transaction_entry_count <> 2 * NEW.moved_balance_count
       OR EXISTS (
            SELECT 1
            FROM public.license_plate_putaway_task_contents content
            WHERE content.tenant_id = NEW.tenant_id
              AND content.task_id = NEW.task_id
              AND (
                  (
                      SELECT COUNT(*)
                      FROM public.inventory_entries entry
                      WHERE entry.tenant_id = NEW.tenant_id
                        AND entry.inventory_owner_id =
                            NEW.inventory_owner_id
                        AND entry.transaction_id =
                            NEW.inventory_transaction_id
                        AND entry.facility_id = NEW.facility_id
                        AND entry.location_id =
                            NEW.source_location_id
                        AND entry.license_plate_id =
                            NEW.license_plate_id
                        AND entry.item_batch_id =
                            content.item_batch_id
                        AND entry.item_id = content.item_id
                        AND entry.uom = content.uom
                        AND entry.status =
                            content.inventory_status
                        AND entry.quantity_delta =
                            -content.planned_quantity
                  ) <> 1
                  OR (
                      SELECT COUNT(*)
                      FROM public.inventory_entries entry
                      WHERE entry.tenant_id = NEW.tenant_id
                        AND entry.inventory_owner_id =
                            NEW.inventory_owner_id
                        AND entry.transaction_id =
                            NEW.inventory_transaction_id
                        AND entry.facility_id = NEW.facility_id
                        AND entry.location_id =
                            NEW.destination_location_id
                        AND entry.license_plate_id =
                            NEW.license_plate_id
                        AND entry.item_batch_id =
                            content.item_batch_id
                        AND entry.item_id = content.item_id
                        AND entry.uom = content.uom
                        AND entry.status =
                            content.inventory_status
                        AND entry.quantity_delta =
                            content.planned_quantity
                  ) <> 1
              )
       )
    THEN
        RAISE EXCEPTION
            'license plate putaway move transaction does not match its snapshot'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE CONSTRAINT TRIGGER license_plate_putaway_results_are_valid
    AFTER INSERT ON license_plate_putaway_results
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION validate_license_plate_putaway_result();

CREATE FUNCTION require_license_plate_putaway_result()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    result_row RECORD;
BEGIN
    IF NEW.task_type = 'license_plate_putaway'
       AND NEW.status = 'completed'
    THEN
        SELECT result.confirmed_by,
               result.confirmed_at
        INTO result_row
        FROM public.license_plate_putaway_results result
        WHERE result.tenant_id = NEW.tenant_id
          AND result.task_id = NEW.id;

        IF NOT FOUND
           OR NEW.completed_by
                IS DISTINCT FROM result_row.confirmed_by
           OR NEW.completed_at
                IS DISTINCT FROM result_row.confirmed_at
        THEN
            RAISE EXCEPTION
                'license plate putaway completion requires its matching result'
                USING ERRCODE = '55000';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

CREATE CONSTRAINT TRIGGER work_tasks_require_license_plate_putaway_result
    AFTER INSERT OR UPDATE ON work_tasks
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION require_license_plate_putaway_result();

CREATE FUNCTION assert_license_plate_location_consistency(
    target_tenant_id BIGINT,
    target_license_plate_id BIGINT
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    plate_row RECORD;
BEGIN
    IF target_license_plate_id IS NULL
       OR NOT EXISTS (
            SELECT 1
            FROM public.inventory_balances balance
            WHERE balance.tenant_id = target_tenant_id
              AND balance.license_plate_id =
                  target_license_plate_id
              AND balance.deleted IS NULL
              AND balance.qty_on_hand > 0
       )
    THEN
        RETURN;
    END IF;

    SELECT plate.inventory_owner_id,
           plate.facility_id,
           plate.location_id,
           plate.deleted
    INTO plate_row
    FROM public.license_plates plate
    WHERE plate.tenant_id = target_tenant_id
      AND plate.id = target_license_plate_id;

    IF NOT FOUND
       OR plate_row.deleted IS NOT NULL
       OR plate_row.location_id IS NULL
       OR EXISTS (
            SELECT 1
            FROM public.inventory_balances balance
            WHERE balance.tenant_id = target_tenant_id
              AND balance.license_plate_id =
                  target_license_plate_id
              AND balance.deleted IS NULL
              AND balance.qty_on_hand > 0
              AND (
                  balance.inventory_owner_id
                      IS DISTINCT FROM plate_row.inventory_owner_id
                  OR balance.facility_id
                      IS DISTINCT FROM plate_row.facility_id
                  OR balance.location_id
                      IS DISTINCT FROM plate_row.location_id
              )
       )
    THEN
        RAISE EXCEPTION
            'license plate header and positive inventory balances must share one location'
            USING ERRCODE = '55000';
    END IF;
END;
$$;

CREATE FUNCTION validate_license_plate_location_from_plate()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') THEN
        PERFORM public.assert_license_plate_location_consistency(
            OLD.tenant_id,
            OLD.id
        );
    END IF;

    IF TG_OP IN ('INSERT', 'UPDATE')
       AND (
           TG_OP = 'INSERT'
           OR NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
           OR NEW.id IS DISTINCT FROM OLD.id
       )
    THEN
        PERFORM public.assert_license_plate_location_consistency(
            NEW.tenant_id,
            NEW.id
        );
    ELSIF TG_OP = 'UPDATE' THEN
        PERFORM public.assert_license_plate_location_consistency(
            NEW.tenant_id,
            NEW.id
        );
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE CONSTRAINT TRIGGER license_plates_location_is_consistent
    AFTER INSERT OR UPDATE OR DELETE ON license_plates
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION validate_license_plate_location_from_plate();

CREATE FUNCTION assert_inventory_balance_license_plate_location_consistency(
    target_tenant_id BIGINT,
    target_inventory_balance_id BIGINT
)
RETURNS void
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    balance_row RECORD;
BEGIN
    SELECT balance.inventory_owner_id,
           balance.facility_id,
           balance.location_id,
           balance.license_plate_id,
           balance.qty_on_hand,
           balance.deleted
    INTO balance_row
    FROM public.inventory_balances balance
    WHERE balance.tenant_id = target_tenant_id
      AND balance.id = target_inventory_balance_id;

    IF NOT FOUND
       OR balance_row.deleted IS NOT NULL
       OR balance_row.qty_on_hand <= 0
       OR balance_row.license_plate_id IS NULL
    THEN
        RETURN;
    END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM public.license_plates plate
        WHERE plate.tenant_id = target_tenant_id
          AND plate.id = balance_row.license_plate_id
          AND plate.inventory_owner_id =
              balance_row.inventory_owner_id
          AND plate.facility_id = balance_row.facility_id
          AND plate.location_id = balance_row.location_id
          AND plate.deleted IS NULL
    ) THEN
        RAISE EXCEPTION
            'positive license plate inventory balance must match its active plate location'
            USING ERRCODE = '55000';
    END IF;
END;
$$;

CREATE FUNCTION validate_license_plate_location_from_balance()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        PERFORM public.assert_inventory_balance_license_plate_location_consistency(
            OLD.tenant_id,
            OLD.id
        );
        RETURN OLD;
    END IF;

    PERFORM public.assert_inventory_balance_license_plate_location_consistency(
        NEW.tenant_id,
        NEW.id
    );

    IF TG_OP = 'UPDATE'
       AND (
           OLD.tenant_id IS DISTINCT FROM NEW.tenant_id
           OR OLD.id IS DISTINCT FROM NEW.id
       )
    THEN
        PERFORM public.assert_inventory_balance_license_plate_location_consistency(
            OLD.tenant_id,
            OLD.id
        );
    END IF;

    RETURN NEW;
END;
$$;

CREATE CONSTRAINT TRIGGER inventory_balances_license_plate_location_is_consistent
    AFTER INSERT OR UPDATE OR DELETE ON inventory_balances
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION validate_license_plate_location_from_balance();

REVOKE ALL ON
    license_plate_putaway_tasks,
    license_plate_putaway_task_contents,
    license_plate_putaway_results
FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT ON
    license_plate_putaway_tasks,
    license_plate_putaway_task_contents,
    license_plate_putaway_results
TO wareboxes_app;

REVOKE ALL ON FUNCTION
    validate_license_plate_putaway_task(),
    require_license_plate_putaway_task_detail(),
    guard_license_plate_putaway_task_mutation(),
    reject_license_plate_putaway_content_mutation(),
    close_license_plate_putaway_task_detail(),
    reject_license_plate_putaway_result_mutation(),
    validate_license_plate_putaway_result(),
    require_license_plate_putaway_result(),
    assert_license_plate_location_consistency(BIGINT, BIGINT),
    validate_license_plate_location_from_plate(),
    assert_inventory_balance_license_plate_location_consistency(BIGINT, BIGINT),
    validate_license_plate_location_from_balance()
FROM PUBLIC, wareboxes_app;
