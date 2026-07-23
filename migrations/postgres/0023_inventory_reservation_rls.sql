ALTER TABLE inventory_reservations ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_reservations FORCE ROW LEVEL SECURITY;

CREATE POLICY inventory_reservations_tenant_isolation
    ON inventory_reservations
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );
