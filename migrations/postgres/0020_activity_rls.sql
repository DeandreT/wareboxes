ALTER TABLE order_activity ENABLE ROW LEVEL SECURITY;
ALTER TABLE order_activity FORCE ROW LEVEL SECURITY;

CREATE POLICY order_activity_tenant_isolation
    ON order_activity
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE load_activity ENABLE ROW LEVEL SECURITY;
ALTER TABLE load_activity FORCE ROW LEVEL SECURITY;

CREATE POLICY load_activity_tenant_isolation
    ON load_activity
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );
