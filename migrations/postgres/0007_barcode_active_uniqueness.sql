ALTER TABLE barcodes
    DROP CONSTRAINT IF EXISTS barcodes_name_key;

DROP INDEX IF EXISTS idx_barcodes_active_item_name_type_unique;
CREATE UNIQUE INDEX idx_barcodes_active_item_name_type_unique
    ON barcodes (tenant_id, item_id, lower(name), type)
    WHERE deleted IS NULL;

CREATE OR REPLACE FUNCTION enforce_barcode_single_active_item()
RETURNS trigger AS $$
BEGIN
    IF NEW.deleted IS NULL AND EXISTS (
        SELECT 1
        FROM barcodes b
        WHERE b.deleted IS NULL
          AND b.tenant_id = NEW.tenant_id
          AND lower(b.name) = lower(NEW.name)
          AND b.item_id <> NEW.item_id
          AND (TG_OP = 'INSERT' OR b.id <> NEW.id)
    ) THEN
        RAISE EXCEPTION 'barcode value is already assigned to another item'
            USING ERRCODE = '23505', CONSTRAINT = 'barcodes_active_name_single_item';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS trg_barcodes_single_active_item ON barcodes;
CREATE TRIGGER trg_barcodes_single_active_item
    BEFORE INSERT OR UPDATE OF tenant_id, name, item_id, deleted ON barcodes
    FOR EACH ROW
    EXECUTE FUNCTION enforce_barcode_single_active_item();
