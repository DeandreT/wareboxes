ALTER TABLE audit_waves ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_waves FORCE ROW LEVEL SECURITY;

CREATE POLICY audit_waves_tenant_isolation
ON audit_waves
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE audit_location_counts ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_location_counts FORCE ROW LEVEL SECURITY;

CREATE POLICY audit_location_counts_tenant_isolation
ON audit_location_counts
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE audit_wave_items ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_wave_items FORCE ROW LEVEL SECURITY;

CREATE POLICY audit_wave_items_tenant_isolation
ON audit_wave_items
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE audit_wave_inventory_owners ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_wave_inventory_owners FORCE ROW LEVEL SECURITY;

CREATE POLICY audit_wave_inventory_owners_tenant_isolation
ON audit_wave_inventory_owners
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE audit_wave_locations ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_wave_locations FORCE ROW LEVEL SECURITY;

CREATE POLICY audit_wave_locations_tenant_isolation
ON audit_wave_locations
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE audit_wave_assignments ENABLE ROW LEVEL SECURITY;
ALTER TABLE audit_wave_assignments FORCE ROW LEVEL SECURITY;

CREATE POLICY audit_wave_assignments_tenant_isolation
ON audit_wave_assignments
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

REVOKE ALL ON
    audit_waves,
    audit_location_counts,
    audit_wave_items,
    audit_wave_inventory_owners,
    audit_wave_locations,
    audit_wave_assignments
FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT, UPDATE ON audit_waves, audit_location_counts
TO wareboxes_app;

REVOKE ALL ON SEQUENCE
    audit_waves_id_seq,
    audit_location_counts_id_seq,
    audit_wave_items_id_seq,
    audit_wave_inventory_owners_id_seq,
    audit_wave_locations_id_seq,
    audit_wave_assignments_id_seq
FROM PUBLIC, wareboxes_app;

GRANT USAGE ON SEQUENCE audit_waves_id_seq, audit_location_counts_id_seq
TO wareboxes_app;
