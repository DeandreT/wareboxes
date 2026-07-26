ALTER TABLE barcodes
    ADD CONSTRAINT barcodes_active_name_nonblank CHECK (
        deleted IS NOT NULL OR btrim(name) <> ''
    );

DROP TRIGGER IF EXISTS trg_barcodes_single_active_item ON barcodes;
DROP FUNCTION IF EXISTS enforce_barcode_single_active_item();

DROP INDEX IF EXISTS idx_barcodes_active_item_name_type_unique;
CREATE UNIQUE INDEX barcodes_active_scanner_identity_unique_idx
    ON barcodes (tenant_id, lower(name))
    WHERE deleted IS NULL;
