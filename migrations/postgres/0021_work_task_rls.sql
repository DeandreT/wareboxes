ALTER TABLE work_tasks ENABLE ROW LEVEL SECURITY;
ALTER TABLE work_tasks FORCE ROW LEVEL SECURITY;

CREATE POLICY work_tasks_tenant_isolation
    ON work_tasks
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE work_task_progress ENABLE ROW LEVEL SECURITY;
ALTER TABLE work_task_progress FORCE ROW LEVEL SECURITY;

CREATE POLICY work_task_progress_tenant_isolation
    ON work_task_progress
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );
