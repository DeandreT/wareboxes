ALTER TABLE putaway_results
    ADD COLUMN destination_location_barcode TEXT NOT NULL,
    ADD CONSTRAINT putaway_results_destination_barcode_nonblank CHECK (
        btrim(destination_location_barcode) <> ''
    );

CREATE INDEX work_tasks_putaway_open_queue_idx
    ON work_tasks(
        tenant_id,
        task_type,
        priority DESC,
        due_at,
        scheduled_for,
        created,
        id
    )
    WHERE deleted IS NULL
      AND status = 'open'
      AND task_type IN ('putaway', 'license_plate_putaway');

CREATE OR REPLACE FUNCTION validate_putaway_task()
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

    IF NOT destination_is_active OR NOT owner_facility_is_active THEN
        RAISE EXCEPTION
            'putaway destination and owner facility assignment must be active and scannable'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE OR REPLACE FUNCTION validate_putaway_result()
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
       OR task_row.deleted IS NOT NULL
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
           balance.deleted,
           destination.barcode AS location_barcode,
           destination.active AS location_is_active,
           destination.receivable AS location_is_receivable,
           destination.deleted AS location_deleted
    INTO destination_row
    FROM public.inventory_balances balance
    INNER JOIN public.locations destination
        ON destination.tenant_id = balance.tenant_id
       AND destination.facility_id = balance.facility_id
       AND destination.id = balance.location_id
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
       OR destination_row.location_deleted IS NOT NULL
       OR NOT destination_row.location_is_active
       OR destination_row.location_is_receivable
       OR destination_row.location_barcode
            IS DISTINCT FROM NEW.destination_location_barcode
    THEN
        RAISE EXCEPTION
            'putaway result does not match its inventory balances and destination'
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

REVOKE ALL ON FUNCTION
    validate_putaway_task(),
    validate_putaway_result()
FROM PUBLIC, wareboxes_app;
