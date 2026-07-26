ALTER TABLE loads ENABLE ROW LEVEL SECURITY;
ALTER TABLE loads FORCE ROW LEVEL SECURITY;

CREATE POLICY loads_tenant_isolation
ON loads
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE load_lines ENABLE ROW LEVEL SECURITY;
ALTER TABLE load_lines FORCE ROW LEVEL SECURITY;

CREATE POLICY load_lines_tenant_isolation
ON load_lines
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE load_notes ENABLE ROW LEVEL SECURITY;
ALTER TABLE load_notes FORCE ROW LEVEL SECURITY;

CREATE POLICY load_notes_tenant_isolation
ON load_notes
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE load_files ENABLE ROW LEVEL SECURITY;
ALTER TABLE load_files FORCE ROW LEVEL SECURITY;

CREATE POLICY load_files_tenant_isolation
ON load_files
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE load_orders ENABLE ROW LEVEL SECURITY;
ALTER TABLE load_orders FORCE ROW LEVEL SECURITY;

CREATE POLICY load_orders_tenant_isolation
ON load_orders
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

REVOKE ALL ON loads, load_lines, load_notes, load_files, load_orders
FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT, UPDATE ON loads, load_lines, load_notes, load_files
TO wareboxes_app;
GRANT SELECT ON load_orders TO wareboxes_app;

REVOKE ALL ON SEQUENCE
    loads_id_seq,
    load_lines_id_seq,
    load_notes_id_seq,
    load_files_id_seq,
    load_orders_id_seq
FROM PUBLIC, wareboxes_app;

GRANT USAGE ON SEQUENCE
    loads_id_seq,
    load_lines_id_seq,
    load_notes_id_seq,
    load_files_id_seq
TO wareboxes_app;
