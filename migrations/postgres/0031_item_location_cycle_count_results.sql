ALTER TABLE work_tasks
    DROP CONSTRAINT work_tasks_required_dimensions_check,
    ADD CONSTRAINT work_tasks_required_dimensions_check CHECK (
        (
            task_type IN (
                'cycle_count_item_location',
                'unpack_cancelled_order'
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

ALTER TABLE cycle_count_item_location_tasks
    ALTER COLUMN inventory_owner_id SET NOT NULL,
    ALTER COLUMN inventory_balance_id SET NOT NULL,
    ADD CONSTRAINT cycle_count_item_location_tasks_target_unique
        UNIQUE (
            tenant_id,
            inventory_owner_id,
            facility_id,
            location_id,
            item_id,
            task_id,
            inventory_balance_id
        );

ALTER TABLE inventory_balances
    ADD CONSTRAINT inventory_balances_cycle_count_target_unique
        UNIQUE (
            tenant_id,
            inventory_owner_id,
            facility_id,
            location_id,
            item_id,
            id
        );

ALTER TABLE cycle_count_item_location_tasks
    ADD CONSTRAINT cycle_count_item_location_tasks_exact_balance_fkey
        FOREIGN KEY (
            tenant_id,
            inventory_owner_id,
            facility_id,
            location_id,
            item_id,
            inventory_balance_id
        )
        REFERENCES inventory_balances(
            tenant_id,
            inventory_owner_id,
            facility_id,
            location_id,
            item_id,
            id
        );

CREATE TABLE cycle_count_item_location_results (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    location_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    inventory_balance_id BIGINT NOT NULL,
    item_batch_id BIGINT NOT NULL,
    license_plate_id BIGINT,
    uom TEXT NOT NULL,
    lot TEXT,
    expiration TIMESTAMPTZ,
    serial TEXT,
    status TEXT NOT NULL,
    system_qty_on_hand BIGINT NOT NULL,
    system_qty_reserved BIGINT NOT NULL,
    counted_qty BIGINT NOT NULL,
    variance_qty BIGINT NOT NULL,
    inventory_transaction_id BIGINT,
    confirmed_by BIGINT NOT NULL,
    confirmed_at TIMESTAMPTZ NOT NULL,
    note TEXT,
    PRIMARY KEY (tenant_id, task_id),
    UNIQUE (tenant_id, inventory_owner_id, inventory_transaction_id),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        location_id,
        item_id,
        task_id,
        inventory_balance_id
    )
        REFERENCES cycle_count_item_location_tasks(
            tenant_id,
            inventory_owner_id,
            facility_id,
            location_id,
            item_id,
            task_id,
            inventory_balance_id
        ),
    FOREIGN KEY (tenant_id, inventory_owner_id)
        REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(
            tenant_id,
            inventory_owner_id,
            facility_id
        ),
    FOREIGN KEY (tenant_id, facility_id, location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, item_id)
        REFERENCES items(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id)
        REFERENCES item_batches(tenant_id, inventory_owner_id, id),
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
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        location_id,
        item_id,
        inventory_balance_id
    )
        REFERENCES inventory_balances(
            tenant_id,
            inventory_owner_id,
            facility_id,
            location_id,
            item_id,
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
    CHECK (system_qty_on_hand >= 0),
    CHECK (system_qty_reserved >= 0),
    CHECK (system_qty_reserved <= system_qty_on_hand),
    CHECK (counted_qty >= system_qty_reserved),
    CHECK (variance_qty = counted_qty - system_qty_on_hand),
    CHECK ((variance_qty = 0) = (inventory_transaction_id IS NULL)),
    CHECK (btrim(uom) <> ''),
    CHECK (status IN ('available', 'hold', 'damaged', 'quarantine')),
    CHECK (
        note IS NULL
        OR (
            note = btrim(note)
            AND char_length(note) BETWEEN 1 AND 1000
        )
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
            'cycle_count_confirmed'
        )
    );

ALTER TABLE cycle_count_item_location_results ENABLE ROW LEVEL SECURITY;
ALTER TABLE cycle_count_item_location_results FORCE ROW LEVEL SECURITY;

CREATE POLICY cycle_count_item_location_results_tenant_isolation
ON cycle_count_item_location_results
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

CREATE OR REPLACE FUNCTION validate_item_location_cycle_count_result()
RETURNS trigger AS $$
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
    FROM work_tasks task
    JOIN cycle_count_item_location_tasks detail
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
        RAISE EXCEPTION 'cycle count result does not match an active task claim'
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
           balance.deleted
    INTO balance_row
    FROM inventory_balances balance
    JOIN item_batches batch
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
    THEN
        RAISE EXCEPTION 'cycle count result does not match the adjusted balance'
            USING ERRCODE = '55000';
    END IF;

    IF NEW.variance_qty <> 0 THEN
        SELECT EXISTS (
            SELECT 1
            FROM inventory_transactions transaction
            WHERE transaction.tenant_id = NEW.tenant_id
              AND transaction.inventory_owner_id = NEW.inventory_owner_id
              AND transaction.id = NEW.inventory_transaction_id
              AND transaction.transaction_type = 'adjust'
              AND transaction.actor_user_id = NEW.confirmed_by
              AND transaction.reference_type = 'cycle_count_item_location_task'
              AND transaction.reference_id = NEW.task_id
              AND transaction.operation = 'task.confirm_item_location_cycle_count.v1'
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
        FROM inventory_entries entry
        WHERE entry.tenant_id = NEW.tenant_id
          AND entry.inventory_owner_id = NEW.inventory_owner_id
          AND entry.transaction_id = NEW.inventory_transaction_id;

        IF NOT transaction_matches
           OR transaction_entry_count <> 1
           OR matching_entry_count <> 1
        THEN
            RAISE EXCEPTION 'cycle count adjustment does not match its result'
                USING ERRCODE = '55000';
        END IF;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER cycle_count_item_location_results_are_valid
    BEFORE INSERT ON cycle_count_item_location_results
    FOR EACH ROW EXECUTE FUNCTION validate_item_location_cycle_count_result();

CREATE OR REPLACE FUNCTION reject_cycle_count_result_mutation()
RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'cycle count results are immutable'
        USING ERRCODE = '55000';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER cycle_count_item_location_results_are_immutable
    BEFORE UPDATE OR DELETE ON cycle_count_item_location_results
    FOR EACH ROW EXECUTE FUNCTION reject_cycle_count_result_mutation();

CREATE OR REPLACE FUNCTION require_item_location_cycle_count_result()
RETURNS trigger AS $$
DECLARE
    result_row RECORD;
BEGIN
    IF NEW.task_type = 'cycle_count_item_location'
       AND NEW.status = 'completed'
       AND OLD.status IS DISTINCT FROM NEW.status
    THEN
        SELECT confirmed_by, confirmed_at
        INTO result_row
        FROM cycle_count_item_location_results
        WHERE tenant_id = NEW.tenant_id
          AND task_id = NEW.id;

        IF NOT FOUND
           OR NEW.completed_by IS DISTINCT FROM result_row.confirmed_by
           OR NEW.completed_at IS DISTINCT FROM result_row.confirmed_at
        THEN
            RAISE EXCEPTION 'item-location cycle count completion requires its result'
                USING ERRCODE = '55000';
        END IF;
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER work_tasks_require_item_location_cycle_count_result
    BEFORE UPDATE OF status ON work_tasks
    FOR EACH ROW EXECUTE FUNCTION require_item_location_cycle_count_result();

REVOKE ALL ON cycle_count_item_location_results
    FROM PUBLIC, wareboxes_app;
GRANT SELECT, INSERT ON cycle_count_item_location_results TO wareboxes_app;
