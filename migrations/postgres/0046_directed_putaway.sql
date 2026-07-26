ALTER TABLE work_tasks
    DROP CONSTRAINT work_tasks_task_type_check,
    ADD CONSTRAINT work_tasks_task_type_check CHECK (
        task_type IN (
            'cycle_count_item_location',
            'cycle_count_location',
            'break_master_pack',
            'unpack_cancelled_order',
            'putaway'
        )
    ),
    DROP CONSTRAINT work_tasks_required_dimensions_check,
    ADD CONSTRAINT work_tasks_required_dimensions_check CHECK (
        (
            task_type IN (
                'cycle_count_item_location',
                'unpack_cancelled_order',
                'putaway'
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
            'putaway_confirmed'
        )
    );

CREATE TABLE putaway_tasks (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    source_inventory_balance_id BIGINT NOT NULL,
    source_location_id BIGINT NOT NULL,
    destination_location_id BIGINT NOT NULL,
    item_batch_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    inventory_status TEXT NOT NULL,
    planned_quantity BIGINT NOT NULL,
    closed_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, task_id),
    UNIQUE (
        tenant_id,
        task_id,
        inventory_owner_id,
        facility_id,
        source_inventory_balance_id,
        source_location_id,
        destination_location_id,
        item_batch_id,
        item_id,
        inventory_status,
        planned_quantity
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
        source_inventory_balance_id
    )
        REFERENCES inventory_balances(
            tenant_id,
            inventory_owner_id,
            facility_id,
            id
        ),
    FOREIGN KEY (tenant_id, facility_id, source_location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, facility_id, destination_location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id)
        REFERENCES item_batches(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, item_id)
        REFERENCES items(tenant_id, id),
    CHECK (source_location_id <> destination_location_id),
    CHECK (inventory_status IN ('available', 'hold', 'damaged', 'quarantine')),
    CHECK (planned_quantity > 0)
);

CREATE UNIQUE INDEX putaway_tasks_one_active_source_idx
    ON putaway_tasks(
        tenant_id,
        inventory_owner_id,
        source_inventory_balance_id
    )
    WHERE closed_at IS NULL;

CREATE INDEX putaway_tasks_destination_idx
    ON putaway_tasks(
        tenant_id,
        inventory_owner_id,
        facility_id,
        destination_location_id
    )
    WHERE closed_at IS NULL;

CREATE TABLE putaway_results (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    source_inventory_balance_id BIGINT NOT NULL,
    destination_inventory_balance_id BIGINT NOT NULL,
    source_location_id BIGINT NOT NULL,
    destination_location_id BIGINT NOT NULL,
    item_batch_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    inventory_status TEXT NOT NULL,
    quantity BIGINT NOT NULL,
    inventory_transaction_id BIGINT NOT NULL,
    confirmed_by BIGINT NOT NULL,
    confirmed_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, task_id),
    UNIQUE (tenant_id, inventory_owner_id, inventory_transaction_id),
    FOREIGN KEY (
        tenant_id,
        task_id,
        inventory_owner_id,
        facility_id,
        source_inventory_balance_id,
        source_location_id,
        destination_location_id,
        item_batch_id,
        item_id,
        inventory_status,
        quantity
    )
        REFERENCES putaway_tasks(
            tenant_id,
            task_id,
            inventory_owner_id,
            facility_id,
            source_inventory_balance_id,
            source_location_id,
            destination_location_id,
            item_batch_id,
            item_id,
            inventory_status,
            planned_quantity
        ),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        source_inventory_balance_id
    )
        REFERENCES inventory_balances(
            tenant_id,
            inventory_owner_id,
            facility_id,
            id
        ),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        destination_inventory_balance_id
    )
        REFERENCES inventory_balances(
            tenant_id,
            inventory_owner_id,
            facility_id,
            id
        ),
    FOREIGN KEY (tenant_id, inventory_owner_id, inventory_transaction_id)
        REFERENCES inventory_transactions(
            tenant_id,
            inventory_owner_id,
            id
        ),
    FOREIGN KEY (tenant_id, confirmed_by)
        REFERENCES tenant_memberships(tenant_id, user_id),
    CHECK (
        source_inventory_balance_id <> destination_inventory_balance_id
    ),
    CHECK (inventory_status IN ('available', 'hold', 'damaged', 'quarantine')),
    CHECK (quantity > 0)
);

ALTER TABLE putaway_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE putaway_tasks FORCE ROW LEVEL SECURITY;

CREATE POLICY putaway_tasks_tenant_isolation
    ON putaway_tasks
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE putaway_results ENABLE ROW LEVEL SECURITY;
ALTER TABLE putaway_results FORCE ROW LEVEL SECURITY;

CREATE POLICY putaway_results_tenant_isolation
    ON putaway_results
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

CREATE FUNCTION validate_putaway_task()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    task_row RECORD;
    source_row RECORD;
    destination_is_active BOOLEAN;
    owner_facility_is_active BOOLEAN;
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
       OR task_row.task_type <> 'putaway'
       OR task_row.status NOT IN ('open', 'assigned')
       OR task_row.deleted IS NOT NULL
       OR task_row.inventory_owner_id
            IS DISTINCT FROM NEW.inventory_owner_id
       OR task_row.facility_id IS DISTINCT FROM NEW.facility_id
       OR NEW.closed_at IS NOT NULL
    THEN
        RAISE EXCEPTION
            'putaway detail does not match an active putaway task'
            USING ERRCODE = '55000';
    END IF;

    SELECT balance.inventory_owner_id,
           balance.facility_id,
           balance.location_id,
           balance.license_plate_id,
           balance.item_batch_id,
           balance.item_id,
           balance.status,
           balance.qty_on_hand,
           balance.qty_reserved,
           balance.qty_held,
           balance.deleted AS balance_deleted,
           source_location.deleted AS location_deleted,
           source_location.active AS location_is_active,
           source_location.receivable AS location_is_receivable
    INTO source_row
    FROM public.inventory_balances balance
    INNER JOIN public.locations source_location
        ON source_location.tenant_id = balance.tenant_id
       AND source_location.facility_id = balance.facility_id
       AND source_location.id = balance.location_id
    WHERE balance.tenant_id = NEW.tenant_id
      AND balance.id = NEW.source_inventory_balance_id
    FOR SHARE;

    IF NOT FOUND
       OR NEW.inventory_status <> 'available'
       OR source_row.balance_deleted IS NOT NULL
       OR source_row.location_deleted IS NOT NULL
       OR NOT source_row.location_is_active
       OR NOT source_row.location_is_receivable
       OR source_row.license_plate_id IS NOT NULL
       OR source_row.inventory_owner_id
            IS DISTINCT FROM NEW.inventory_owner_id
       OR source_row.facility_id IS DISTINCT FROM NEW.facility_id
       OR source_row.location_id IS DISTINCT FROM NEW.source_location_id
       OR source_row.item_batch_id IS DISTINCT FROM NEW.item_batch_id
       OR source_row.item_id IS DISTINCT FROM NEW.item_id
       OR source_row.status IS DISTINCT FROM NEW.inventory_status
       OR source_row.qty_on_hand
            - source_row.qty_reserved
            - source_row.qty_held < NEW.planned_quantity
    THEN
        RAISE EXCEPTION
            'putaway detail does not match available loose source inventory'
            USING ERRCODE = '55000';
    END IF;

    SELECT EXISTS (
        SELECT 1
        FROM public.locations location
        WHERE location.tenant_id = NEW.tenant_id
          AND location.facility_id = NEW.facility_id
          AND location.id = NEW.destination_location_id
          AND location.deleted IS NULL
          AND location.active
          AND NOT location.receivable
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

    IF NOT destination_is_active OR NOT owner_facility_is_active THEN
        RAISE EXCEPTION
            'putaway destination and owner facility assignment must be active'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER putaway_tasks_are_valid
    BEFORE INSERT ON putaway_tasks
    FOR EACH ROW
    EXECUTE FUNCTION validate_putaway_task();

CREATE FUNCTION guard_putaway_task_mutation()
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
       OR OLD.source_inventory_balance_id
            IS DISTINCT FROM NEW.source_inventory_balance_id
       OR OLD.source_location_id IS DISTINCT FROM NEW.source_location_id
       OR OLD.destination_location_id
            IS DISTINCT FROM NEW.destination_location_id
       OR OLD.item_batch_id IS DISTINCT FROM NEW.item_batch_id
       OR OLD.item_id IS DISTINCT FROM NEW.item_id
       OR OLD.inventory_status IS DISTINCT FROM NEW.inventory_status
       OR OLD.planned_quantity IS DISTINCT FROM NEW.planned_quantity
       OR OLD.closed_at IS NOT NULL
       OR NEW.closed_at IS NULL
    THEN
        RAISE EXCEPTION 'putaway task snapshots are immutable'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER putaway_tasks_guard_mutation
    BEFORE UPDATE OR DELETE ON putaway_tasks
    FOR EACH ROW
    EXECUTE FUNCTION guard_putaway_task_mutation();

CREATE FUNCTION close_putaway_task_detail()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF NEW.task_type = 'putaway'
       AND (
           NEW.deleted IS NOT NULL
           OR NEW.status IN ('completed', 'cancelled')
       )
    THEN
        UPDATE public.putaway_tasks
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

CREATE TRIGGER work_tasks_close_putaway_detail
    AFTER UPDATE OF status, deleted ON work_tasks
    FOR EACH ROW
    EXECUTE FUNCTION close_putaway_task_detail();

CREATE FUNCTION reject_putaway_result_mutation()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    RAISE EXCEPTION 'putaway results are immutable'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER putaway_results_are_immutable
    BEFORE UPDATE OR DELETE ON putaway_results
    FOR EACH ROW
    EXECUTE FUNCTION reject_putaway_result_mutation();

CREATE FUNCTION validate_putaway_result()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    task_row RECORD;
    source_row RECORD;
    destination_row RECORD;
    transaction_matches BOOLEAN;
    transaction_entry_count BIGINT;
    source_entry_count BIGINT;
    destination_entry_count BIGINT;
BEGIN
    SELECT task.status,
           task.deleted,
           task.assigned_user_id,
           task.completed_by,
           task.completed_at,
           detail.closed_at
    INTO task_row
    FROM public.work_tasks task
    INNER JOIN public.putaway_tasks detail
        ON detail.tenant_id = task.tenant_id
       AND detail.task_id = task.id
    WHERE task.tenant_id = NEW.tenant_id
      AND task.id = NEW.task_id
      AND task.task_type = 'putaway';

    IF NOT FOUND
       OR task_row.status <> 'completed'
       OR task_row.assigned_user_id IS DISTINCT FROM NEW.confirmed_by
       OR task_row.completed_by IS DISTINCT FROM NEW.confirmed_by
       OR task_row.completed_at IS DISTINCT FROM NEW.confirmed_at
       OR task_row.closed_at IS DISTINCT FROM NEW.confirmed_at
    THEN
        RAISE EXCEPTION
            'putaway result does not match its completed task'
            USING ERRCODE = '55000';
    END IF;

    SELECT balance.location_id,
           balance.license_plate_id,
           balance.item_batch_id,
           balance.item_id,
           balance.status,
           balance.deleted
    INTO source_row
    FROM public.inventory_balances balance
    WHERE balance.tenant_id = NEW.tenant_id
      AND balance.inventory_owner_id = NEW.inventory_owner_id
      AND balance.facility_id = NEW.facility_id
      AND balance.id = NEW.source_inventory_balance_id;

    SELECT balance.location_id,
           balance.license_plate_id,
           balance.item_batch_id,
           balance.item_id,
           balance.status,
           balance.qty_on_hand,
           balance.deleted
    INTO destination_row
    FROM public.inventory_balances balance
    WHERE balance.tenant_id = NEW.tenant_id
      AND balance.inventory_owner_id = NEW.inventory_owner_id
      AND balance.facility_id = NEW.facility_id
      AND balance.id = NEW.destination_inventory_balance_id;

    IF source_row.location_id IS DISTINCT FROM NEW.source_location_id
       OR source_row.license_plate_id IS NOT NULL
       OR source_row.item_batch_id IS DISTINCT FROM NEW.item_batch_id
       OR source_row.item_id IS DISTINCT FROM NEW.item_id
       OR source_row.status IS DISTINCT FROM NEW.inventory_status
       OR source_row.deleted IS NOT NULL
       OR destination_row.location_id
            IS DISTINCT FROM NEW.destination_location_id
       OR destination_row.license_plate_id IS NOT NULL
       OR destination_row.item_batch_id IS DISTINCT FROM NEW.item_batch_id
       OR destination_row.item_id IS DISTINCT FROM NEW.item_id
       OR destination_row.status IS DISTINCT FROM NEW.inventory_status
       OR destination_row.qty_on_hand < NEW.quantity
       OR destination_row.deleted IS NOT NULL
    THEN
        RAISE EXCEPTION
            'putaway result does not match its inventory balances'
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
          AND transaction.reference_type = 'putaway_task'
          AND transaction.reference_id = NEW.task_id
          AND transaction.operation = 'task.confirm_putaway.v1'
    )
    INTO transaction_matches;

    SELECT COUNT(*),
           COUNT(*) FILTER (
               WHERE entry.facility_id = NEW.facility_id
                 AND entry.location_id = NEW.source_location_id
                 AND entry.license_plate_id IS NULL
                 AND entry.item_batch_id = NEW.item_batch_id
                 AND entry.item_id = NEW.item_id
                 AND entry.status = NEW.inventory_status
                 AND entry.quantity_delta = -NEW.quantity
           ),
           COUNT(*) FILTER (
               WHERE entry.facility_id = NEW.facility_id
                 AND entry.location_id = NEW.destination_location_id
                 AND entry.license_plate_id IS NULL
                 AND entry.item_batch_id = NEW.item_batch_id
                 AND entry.item_id = NEW.item_id
                 AND entry.status = NEW.inventory_status
                 AND entry.quantity_delta = NEW.quantity
           )
    INTO
        transaction_entry_count,
        source_entry_count,
        destination_entry_count
    FROM public.inventory_entries entry
    WHERE entry.tenant_id = NEW.tenant_id
      AND entry.inventory_owner_id = NEW.inventory_owner_id
      AND entry.transaction_id = NEW.inventory_transaction_id;

    IF NOT transaction_matches
       OR transaction_entry_count <> 2
       OR source_entry_count <> 1
       OR destination_entry_count <> 1
    THEN
        RAISE EXCEPTION
            'putaway move transaction does not match its result'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE CONSTRAINT TRIGGER putaway_results_are_valid
    AFTER INSERT ON putaway_results
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION validate_putaway_result();

CREATE FUNCTION require_putaway_result()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    task_row RECORD;
    result_row RECORD;
BEGIN
    SELECT task.status,
           task.completed_by,
           task.completed_at
    INTO task_row
    FROM public.work_tasks task
    WHERE task.tenant_id = NEW.tenant_id
      AND task.id = NEW.id
      AND task.task_type = 'putaway';

    IF FOUND AND task_row.status = 'completed' THEN
        SELECT result.confirmed_by,
               result.confirmed_at
        INTO result_row
        FROM public.putaway_results result
        WHERE result.tenant_id = NEW.tenant_id
          AND result.task_id = NEW.id;

        IF NOT FOUND
           OR task_row.completed_by
                IS DISTINCT FROM result_row.confirmed_by
           OR task_row.completed_at
                IS DISTINCT FROM result_row.confirmed_at
        THEN
            RAISE EXCEPTION
                'putaway completion requires its matching result'
                USING ERRCODE = '55000';
        END IF;
    END IF;

    RETURN NEW;
END;
$$;

CREATE CONSTRAINT TRIGGER work_tasks_require_putaway_result
    AFTER INSERT OR UPDATE ON work_tasks
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION require_putaway_result();

REVOKE ALL ON putaway_tasks, putaway_results
FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT ON putaway_tasks, putaway_results
TO wareboxes_app;

REVOKE ALL ON FUNCTION
    validate_putaway_task(),
    guard_putaway_task_mutation(),
    close_putaway_task_detail(),
    reject_putaway_result_mutation(),
    validate_putaway_result(),
    require_putaway_result()
FROM PUBLIC, wareboxes_app;
