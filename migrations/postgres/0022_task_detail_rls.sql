ALTER TABLE cycle_count_item_location_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE cycle_count_item_location_tasks FORCE ROW LEVEL SECURITY;

CREATE POLICY cycle_count_item_location_tasks_tenant_isolation
    ON cycle_count_item_location_tasks
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE cycle_count_location_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE cycle_count_location_tasks FORCE ROW LEVEL SECURITY;

CREATE POLICY cycle_count_location_tasks_tenant_isolation
    ON cycle_count_location_tasks
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE break_master_pack_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE break_master_pack_tasks FORCE ROW LEVEL SECURITY;

CREATE POLICY break_master_pack_tasks_tenant_isolation
    ON break_master_pack_tasks
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE unpack_cancelled_order_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE unpack_cancelled_order_tasks FORCE ROW LEVEL SECURITY;

CREATE POLICY unpack_cancelled_order_tasks_tenant_isolation
    ON unpack_cancelled_order_tasks
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE unpack_cancelled_order_task_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE unpack_cancelled_order_task_lines FORCE ROW LEVEL SECURITY;

CREATE POLICY unpack_cancelled_order_task_lines_tenant_isolation
    ON unpack_cancelled_order_task_lines
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );
