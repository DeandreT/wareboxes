ALTER TABLE work_tasks
    ADD COLUMN facility_id BIGINT,
    ADD COLUMN inventory_owner_id BIGINT;

ALTER TABLE cycle_count_item_location_tasks
    ADD COLUMN inventory_owner_id BIGINT;

ALTER TABLE unpack_cancelled_order_tasks
    ADD COLUMN inventory_owner_id BIGINT;

ALTER TABLE unpack_cancelled_order_task_lines
    ADD COLUMN inventory_owner_id BIGINT;

ALTER TABLE work_tasks
    ADD CONSTRAINT work_tasks_facility_fkey
        FOREIGN KEY (tenant_id, facility_id) REFERENCES facilities(tenant_id, id),
    ADD CONSTRAINT work_tasks_inventory_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    ADD CONSTRAINT work_tasks_required_dimensions_check CHECK (
        (
            task_type = 'unpack_cancelled_order'
            AND facility_id IS NOT NULL
            AND inventory_owner_id IS NOT NULL
        )
        OR (
            task_type IN (
                'cycle_count_item_location',
                'cycle_count_location',
                'break_master_pack'
            )
            AND facility_id IS NOT NULL
        )
    ),
    ADD CONSTRAINT work_tasks_tenant_facility_id_unique
        UNIQUE (tenant_id, facility_id, id),
    ADD CONSTRAINT work_tasks_tenant_owner_id_unique
        UNIQUE (tenant_id, inventory_owner_id, id);

ALTER TABLE inventory_balances
    ADD CONSTRAINT inventory_balances_tenant_owner_facility_id_unique
        UNIQUE (tenant_id, inventory_owner_id, facility_id, id);

ALTER TABLE order_items
    ADD CONSTRAINT order_items_tenant_owner_order_id_unique
        UNIQUE (tenant_id, inventory_owner_id, order_id, id);

ALTER TABLE cycle_count_item_location_tasks
    ADD CONSTRAINT cycle_count_item_location_tasks_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id)
        REFERENCES inventory_owners(tenant_id, id),
    ADD CONSTRAINT cycle_count_item_location_tasks_task_facility_fkey
        FOREIGN KEY (tenant_id, facility_id, task_id)
        REFERENCES work_tasks(tenant_id, facility_id, id),
    ADD CONSTRAINT cycle_count_item_location_tasks_task_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, task_id)
        REFERENCES work_tasks(tenant_id, inventory_owner_id, id),
    ADD CONSTRAINT cycle_count_item_location_tasks_balance_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, inventory_balance_id)
        REFERENCES inventory_balances(tenant_id, inventory_owner_id, facility_id, id),
    ADD CONSTRAINT cycle_count_item_location_tasks_order_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, order_id)
        REFERENCES orders(tenant_id, inventory_owner_id, id),
    ADD CONSTRAINT cycle_count_item_location_tasks_order_item_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, order_item_id)
        REFERENCES order_items(tenant_id, inventory_owner_id, id),
    ADD CONSTRAINT cycle_count_item_location_tasks_scoped_references_check CHECK (
        inventory_owner_id IS NOT NULL
        OR (
            inventory_balance_id IS NULL
            AND order_id IS NULL
            AND order_item_id IS NULL
        )
    );

ALTER TABLE cycle_count_location_tasks
    ADD CONSTRAINT cycle_count_location_tasks_task_facility_fkey
        FOREIGN KEY (tenant_id, facility_id, task_id)
        REFERENCES work_tasks(tenant_id, facility_id, id);

ALTER TABLE break_master_pack_tasks
    ADD CONSTRAINT break_master_pack_tasks_task_facility_fkey
        FOREIGN KEY (tenant_id, facility_id, task_id)
        REFERENCES work_tasks(tenant_id, facility_id, id);

ALTER TABLE unpack_cancelled_order_tasks
    ADD COLUMN facility_id BIGINT NOT NULL,
    ALTER COLUMN inventory_owner_id SET NOT NULL,
    ADD CONSTRAINT unpack_cancelled_order_tasks_facility_fkey
        FOREIGN KEY (tenant_id, facility_id)
        REFERENCES facilities(tenant_id, id),
    ADD CONSTRAINT unpack_cancelled_order_tasks_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id)
        REFERENCES inventory_owners(tenant_id, id),
    ADD CONSTRAINT unpack_cancelled_order_tasks_task_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, task_id)
        REFERENCES work_tasks(tenant_id, inventory_owner_id, id),
    ADD CONSTRAINT unpack_cancelled_order_tasks_task_facility_fkey
        FOREIGN KEY (tenant_id, facility_id, task_id)
        REFERENCES work_tasks(tenant_id, facility_id, id),
    ADD CONSTRAINT unpack_cancelled_order_tasks_order_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, order_id)
        REFERENCES orders(tenant_id, inventory_owner_id, id),
    ADD CONSTRAINT unpack_cancelled_order_tasks_tenant_owner_facility_task_unique
        UNIQUE (tenant_id, inventory_owner_id, facility_id, task_id),
    ADD CONSTRAINT unpack_cancelled_order_tasks_tenant_order_unique
        UNIQUE (tenant_id, order_id);

ALTER TABLE unpack_cancelled_order_task_lines
    ADD COLUMN facility_id BIGINT NOT NULL,
    ALTER COLUMN inventory_owner_id SET NOT NULL,
    ADD CONSTRAINT unpack_cancelled_order_task_lines_task_scope_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, task_id)
        REFERENCES unpack_cancelled_order_tasks(
            tenant_id,
            inventory_owner_id,
            facility_id,
            task_id
        ),
    ADD CONSTRAINT unpack_cancelled_order_task_lines_scope_id_unique
        UNIQUE (tenant_id, inventory_owner_id, facility_id, task_id, id),
    ADD CONSTRAINT unpack_cancelled_order_task_lines_order_item_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, order_item_id)
        REFERENCES order_items(tenant_id, inventory_owner_id, id),
    ADD CONSTRAINT unpack_cancelled_order_task_lines_batch_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id)
        REFERENCES item_batches(tenant_id, inventory_owner_id, id),
    ADD CONSTRAINT unpack_cancelled_order_task_lines_balance_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, inventory_balance_id)
        REFERENCES inventory_balances(tenant_id, inventory_owner_id, facility_id, id),
    ADD CONSTRAINT unpack_cancelled_order_task_lines_license_plate_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, license_plate_id)
        REFERENCES license_plates(tenant_id, inventory_owner_id, facility_id, id),
    ADD CONSTRAINT unpack_cancelled_order_task_lines_source_facility_fkey
        FOREIGN KEY (tenant_id, facility_id, source_location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    ADD CONSTRAINT unpack_cancelled_order_task_lines_destination_facility_fkey
        FOREIGN KEY (tenant_id, facility_id, destination_location_id)
        REFERENCES locations(tenant_id, facility_id, id);

ALTER TABLE work_task_progress
    ADD COLUMN facility_id BIGINT NOT NULL,
    ADD COLUMN inventory_owner_id BIGINT,
    ADD CONSTRAINT work_task_progress_task_line_owner_check CHECK (
        task_line_id IS NULL OR inventory_owner_id IS NOT NULL
    ),
    ADD CONSTRAINT work_task_progress_task_facility_fkey
        FOREIGN KEY (tenant_id, facility_id, task_id)
        REFERENCES work_tasks(tenant_id, facility_id, id),
    ADD CONSTRAINT work_task_progress_task_owner_fkey
        FOREIGN KEY (tenant_id, inventory_owner_id, task_id)
        REFERENCES work_tasks(tenant_id, inventory_owner_id, id),
    ADD CONSTRAINT work_task_progress_task_line_scope_fkey
        FOREIGN KEY (
            tenant_id,
            inventory_owner_id,
            facility_id,
            task_id,
            task_line_id
        )
        REFERENCES unpack_cancelled_order_task_lines(
            tenant_id,
            inventory_owner_id,
            facility_id,
            task_id,
            id
        ),
    ADD CONSTRAINT work_task_progress_from_location_facility_fkey
        FOREIGN KEY (tenant_id, facility_id, from_location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    ADD CONSTRAINT work_task_progress_to_location_facility_fkey
        FOREIGN KEY (tenant_id, facility_id, to_location_id)
        REFERENCES locations(tenant_id, facility_id, id);

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
            'moved'
        )
    );

CREATE INDEX work_tasks_scope_queue_idx
    ON work_tasks(
        tenant_id,
        facility_id,
        inventory_owner_id,
        status,
        required_permission,
        priority DESC,
        created
    )
    WHERE deleted IS NULL;

CREATE OR REPLACE FUNCTION protect_work_task_dimensions()
RETURNS trigger AS $$
BEGIN
    IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
        OR NEW.facility_id IS DISTINCT FROM OLD.facility_id
        OR NEW.inventory_owner_id IS DISTINCT FROM OLD.inventory_owner_id
        OR NEW.task_type IS DISTINCT FROM OLD.task_type
    THEN
        RAISE EXCEPTION 'work task dimensions are immutable'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER work_task_dimensions_are_immutable
    BEFORE UPDATE OF tenant_id, facility_id, inventory_owner_id, task_type ON work_tasks
    FOR EACH ROW EXECUTE FUNCTION protect_work_task_dimensions();
