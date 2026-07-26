ALTER TABLE locations
    DROP CONSTRAINT locations_tenant_id_parent_location_id_fkey,
    ADD CONSTRAINT locations_parent_same_facility_fkey
        FOREIGN KEY (tenant_id, facility_id, parent_location_id)
        REFERENCES locations(tenant_id, facility_id, id);

ALTER TABLE facilities ENABLE ROW LEVEL SECURITY;
ALTER TABLE facilities FORCE ROW LEVEL SECURITY;

CREATE POLICY facilities_tenant_isolation
ON facilities
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE locations ENABLE ROW LEVEL SECURITY;
ALTER TABLE locations FORCE ROW LEVEL SECURITY;

CREATE POLICY locations_tenant_isolation
ON locations
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE inventory_owners ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_owners FORCE ROW LEVEL SECURITY;

CREATE POLICY inventory_owners_tenant_isolation
ON inventory_owners
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE inventory_owner_facilities ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_owner_facilities FORCE ROW LEVEL SECURITY;

CREATE POLICY inventory_owner_facilities_tenant_isolation
ON inventory_owner_facilities
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

REVOKE ALL ON
    facilities,
    locations,
    inventory_owners,
    inventory_owner_facilities
FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT ON facilities TO wareboxes_app;
GRANT SELECT, INSERT, UPDATE ON locations TO wareboxes_app;
GRANT SELECT, INSERT, UPDATE ON inventory_owners TO wareboxes_app;
GRANT SELECT ON inventory_owner_facilities TO wareboxes_app;

REVOKE ALL ON SEQUENCE
    facilities_id_seq,
    locations_id_seq,
    inventory_owners_id_seq,
    inventory_owner_facilities_id_seq
FROM PUBLIC, wareboxes_app;

GRANT USAGE ON SEQUENCE
    facilities_id_seq,
    locations_id_seq,
    inventory_owners_id_seq
TO wareboxes_app;
