ALTER TABLE order_tracking_numbers ENABLE ROW LEVEL SECURITY;
ALTER TABLE order_tracking_numbers FORCE ROW LEVEL SECURITY;

CREATE POLICY order_tracking_numbers_tenant_isolation
    ON order_tracking_numbers
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );
