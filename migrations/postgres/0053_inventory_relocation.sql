ALTER TABLE work_tasks
    DROP CONSTRAINT work_tasks_task_type_check,
    ADD CONSTRAINT work_tasks_task_type_check CHECK (
        task_type IN (
            'cycle_count_item_location',
            'cycle_count_location',
            'break_master_pack',
            'unpack_cancelled_order',
            'putaway',
            'license_plate_putaway',
            'inventory_relocation'
        )
    ),
    DROP CONSTRAINT work_tasks_required_dimensions_check,
    ADD CONSTRAINT work_tasks_required_dimensions_check CHECK (
        (
            task_type IN (
                'cycle_count_item_location',
                'unpack_cancelled_order',
                'putaway',
                'license_plate_putaway',
                'inventory_relocation'
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
            'license_plate_putaway_confirmed',
            'putaway_heartbeat',
            'putaway_released',
            'inventory_relocation_confirmed',
            'inventory_relocation_heartbeat',
            'inventory_relocation_released'
        )
    );

CREATE TABLE inventory_relocation_tasks (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    workflow TEXT NOT NULL,
    source_inventory_balance_id BIGINT,
    license_plate_id BIGINT,
    source_location_id BIGINT NOT NULL,
    destination_location_id BIGINT NOT NULL,
    item_batch_id BIGINT,
    item_id BIGINT,
    uom TEXT,
    inventory_status TEXT,
    planned_quantity BIGINT,
    planned_balance_count BIGINT,
    closed_at TIMESTAMPTZ,
    PRIMARY KEY (tenant_id, task_id),
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
    FOREIGN KEY (tenant_id, facility_id, source_location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, facility_id, destination_location_id)
        REFERENCES locations(tenant_id, facility_id, id),
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
        license_plate_id
    )
        REFERENCES license_plates(
            tenant_id,
            inventory_owner_id,
            facility_id,
            id
        ),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id)
        REFERENCES item_batches(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, item_id)
        REFERENCES items(tenant_id, id),
    CHECK (source_location_id <> destination_location_id),
    CHECK (workflow IN ('loose_balance', 'license_plate')),
    CHECK (
        (
            workflow = 'loose_balance'
            AND source_inventory_balance_id IS NOT NULL
            AND license_plate_id IS NULL
            AND item_batch_id IS NOT NULL
            AND item_id IS NOT NULL
            AND uom IS NOT NULL
            AND inventory_status IN (
                'available',
                'hold',
                'damaged',
                'quarantine'
            )
            AND planned_quantity > 0
            AND planned_balance_count IS NULL
        )
        OR (
            workflow = 'license_plate'
            AND source_inventory_balance_id IS NULL
            AND license_plate_id IS NOT NULL
            AND item_batch_id IS NULL
            AND item_id IS NULL
            AND uom IS NULL
            AND inventory_status IS NULL
            AND planned_quantity IS NULL
            AND planned_balance_count > 0
        )
    )
);

CREATE UNIQUE INDEX inventory_relocation_one_active_balance_idx
    ON inventory_relocation_tasks(
        tenant_id,
        inventory_owner_id,
        source_inventory_balance_id
    )
    WHERE closed_at IS NULL
      AND workflow = 'loose_balance';

CREATE UNIQUE INDEX inventory_relocation_one_active_plate_idx
    ON inventory_relocation_tasks(
        tenant_id,
        inventory_owner_id,
        license_plate_id
    )
    WHERE closed_at IS NULL
      AND workflow = 'license_plate';

CREATE TABLE inventory_relocation_task_contents (
    tenant_id BIGINT NOT NULL,
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
    FOREIGN KEY (tenant_id, task_id)
        REFERENCES inventory_relocation_tasks(tenant_id, task_id),
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
    CHECK (inventory_status IN (
        'available',
        'hold',
        'damaged',
        'quarantine'
    )),
    CHECK (planned_quantity > 0)
);

CREATE TABLE inventory_relocation_results (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    workflow TEXT NOT NULL,
    source_location_id BIGINT NOT NULL,
    destination_location_id BIGINT NOT NULL,
    destination_location_barcode TEXT NOT NULL,
    inventory_transaction_id BIGINT NOT NULL,
    source_inventory_balance_id BIGINT,
    destination_inventory_balance_id BIGINT,
    license_plate_id BIGINT,
    license_plate_barcode TEXT,
    item_batch_id BIGINT,
    item_id BIGINT,
    uom TEXT,
    inventory_status TEXT,
    quantity BIGINT,
    moved_balance_count BIGINT,
    confirmed_by BIGINT NOT NULL,
    confirmed_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, task_id),
    UNIQUE (tenant_id, inventory_owner_id, inventory_transaction_id),
    FOREIGN KEY (tenant_id, task_id)
        REFERENCES inventory_relocation_tasks(tenant_id, task_id),
    FOREIGN KEY (tenant_id, inventory_owner_id, inventory_transaction_id)
        REFERENCES inventory_transactions(
            tenant_id,
            inventory_owner_id,
            id
        ),
    FOREIGN KEY (tenant_id, confirmed_by)
        REFERENCES tenant_memberships(tenant_id, user_id),
    CHECK (source_location_id <> destination_location_id),
    CHECK (length(trim(destination_location_barcode)) > 0),
    CHECK (
        (
            workflow = 'loose_balance'
            AND source_inventory_balance_id IS NOT NULL
            AND destination_inventory_balance_id IS NOT NULL
            AND source_inventory_balance_id <>
                destination_inventory_balance_id
            AND license_plate_id IS NULL
            AND license_plate_barcode IS NULL
            AND item_batch_id IS NOT NULL
            AND item_id IS NOT NULL
            AND uom IS NOT NULL
            AND inventory_status IN (
                'available',
                'hold',
                'damaged',
                'quarantine'
            )
            AND quantity > 0
            AND moved_balance_count IS NULL
        )
        OR (
            workflow = 'license_plate'
            AND source_inventory_balance_id IS NULL
            AND destination_inventory_balance_id IS NULL
            AND license_plate_id IS NOT NULL
            AND length(trim(license_plate_barcode)) > 0
            AND item_batch_id IS NULL
            AND item_id IS NULL
            AND uom IS NULL
            AND inventory_status IS NULL
            AND quantity IS NULL
            AND moved_balance_count > 0
        )
    )
);

ALTER TABLE inventory_relocation_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_relocation_tasks FORCE ROW LEVEL SECURITY;
CREATE POLICY inventory_relocation_tasks_tenant_isolation
    ON inventory_relocation_tasks
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE inventory_relocation_task_contents ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_relocation_task_contents FORCE ROW LEVEL SECURITY;
CREATE POLICY inventory_relocation_task_contents_tenant_isolation
    ON inventory_relocation_task_contents
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE inventory_relocation_results ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_relocation_results FORCE ROW LEVEL SECURITY;
CREATE POLICY inventory_relocation_results_tenant_isolation
    ON inventory_relocation_results
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

GRANT SELECT, INSERT ON
    inventory_relocation_tasks,
    inventory_relocation_task_contents,
    inventory_relocation_results
TO wareboxes_app;

CREATE FUNCTION close_inventory_relocation_task_detail()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF NEW.task_type = 'inventory_relocation'
       AND (
           NEW.deleted IS NOT NULL
           OR NEW.status IN ('completed', 'cancelled')
       )
    THEN
        UPDATE public.inventory_relocation_tasks
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

CREATE TRIGGER work_tasks_close_inventory_relocation_detail
    AFTER UPDATE OF status, deleted ON work_tasks
    FOR EACH ROW
    EXECUTE FUNCTION close_inventory_relocation_task_detail();

CREATE FUNCTION reject_inventory_relocation_result_mutation()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    RAISE EXCEPTION 'inventory relocation results are immutable'
        USING ERRCODE = '55000';
END;
$$;

CREATE TRIGGER inventory_relocation_results_are_immutable
    BEFORE UPDATE OR DELETE ON inventory_relocation_results
    FOR EACH ROW
    EXECUTE FUNCTION reject_inventory_relocation_result_mutation();
