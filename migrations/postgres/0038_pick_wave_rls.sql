ALTER TABLE pick_waves ENABLE ROW LEVEL SECURITY;
ALTER TABLE pick_waves FORCE ROW LEVEL SECURITY;

CREATE POLICY pick_waves_tenant_isolation
ON pick_waves
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

REVOKE ALL ON pick_waves FROM PUBLIC, wareboxes_app;
REVOKE ALL ON SEQUENCE pick_waves_id_seq FROM PUBLIC, wareboxes_app;
