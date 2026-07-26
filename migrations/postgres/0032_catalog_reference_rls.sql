ALTER TABLE dims ENABLE ROW LEVEL SECURITY;
ALTER TABLE dims FORCE ROW LEVEL SECURITY;

CREATE POLICY dims_tenant_isolation
ON dims
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE items ENABLE ROW LEVEL SECURITY;
ALTER TABLE items FORCE ROW LEVEL SECURITY;

CREATE POLICY items_tenant_isolation
ON items
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE skus ENABLE ROW LEVEL SECURITY;
ALTER TABLE skus FORCE ROW LEVEL SECURITY;

CREATE POLICY skus_tenant_isolation
ON skus
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE barcodes ENABLE ROW LEVEL SECURITY;
ALTER TABLE barcodes FORCE ROW LEVEL SECURITY;

CREATE POLICY barcodes_tenant_isolation
ON barcodes
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE item_pack_links ENABLE ROW LEVEL SECURITY;
ALTER TABLE item_pack_links FORCE ROW LEVEL SECURITY;

CREATE POLICY item_pack_links_tenant_isolation
ON item_pack_links
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE inventory_owner_items ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_owner_items FORCE ROW LEVEL SECURITY;

CREATE POLICY inventory_owner_items_tenant_isolation
ON inventory_owner_items
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE item_batches ENABLE ROW LEVEL SECURITY;
ALTER TABLE item_batches FORCE ROW LEVEL SECURITY;

CREATE POLICY item_batches_tenant_isolation
ON item_batches
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

REVOKE ALL ON
    dims,
    items,
    skus,
    barcodes,
    item_pack_links,
    inventory_owner_items,
    item_batches
FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT ON dims TO wareboxes_app;
GRANT SELECT, INSERT, UPDATE ON items TO wareboxes_app;
GRANT SELECT, INSERT ON skus TO wareboxes_app;
GRANT SELECT, INSERT, UPDATE ON barcodes TO wareboxes_app;
GRANT SELECT, INSERT, UPDATE ON item_pack_links TO wareboxes_app;
GRANT SELECT, INSERT, UPDATE ON inventory_owner_items TO wareboxes_app;
GRANT SELECT, INSERT, UPDATE ON item_batches TO wareboxes_app;

REVOKE ALL ON SEQUENCE
    dims_id_seq,
    items_id_seq,
    skus_id_seq,
    barcodes_id_seq,
    item_pack_links_id_seq,
    inventory_owner_items_id_seq,
    item_batches_id_seq
FROM PUBLIC, wareboxes_app;

GRANT USAGE ON SEQUENCE
    dims_id_seq,
    items_id_seq,
    skus_id_seq,
    barcodes_id_seq,
    item_pack_links_id_seq,
    inventory_owner_items_id_seq,
    item_batches_id_seq
TO wareboxes_app;
