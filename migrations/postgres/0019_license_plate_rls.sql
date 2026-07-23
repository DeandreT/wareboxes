ALTER TABLE license_plates ENABLE ROW LEVEL SECURITY;
ALTER TABLE license_plates FORCE ROW LEVEL SECURITY;

CREATE POLICY license_plates_tenant_isolation
    ON license_plates
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );
