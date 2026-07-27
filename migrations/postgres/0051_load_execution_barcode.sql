TRUNCATE TABLE loads RESTART IDENTITY CASCADE;

ALTER TABLE loads
    ADD COLUMN execution_barcode TEXT NOT NULL,
    ADD CONSTRAINT loads_execution_barcode_format_check CHECK (
        execution_barcode = upper(btrim(execution_barcode))
        AND octet_length(execution_barcode) BETWEEN 1 AND 200
        AND execution_barcode ~ '^[A-Z0-9][A-Z0-9._:-]{0,199}$'
    ),
    ADD CONSTRAINT loads_tenant_execution_barcode_unique
        UNIQUE (tenant_id, execution_barcode);

CREATE FUNCTION enforce_load_execution_barcode_immutable()
RETURNS TRIGGER
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF NEW.execution_barcode IS DISTINCT FROM OLD.execution_barcode THEN
        RAISE EXCEPTION 'load execution barcode is immutable'
            USING ERRCODE = '23514';
    END IF;
    RETURN NEW;
END
$$;

CREATE TRIGGER trg_load_execution_barcode_immutable
BEFORE UPDATE OF execution_barcode ON loads
FOR EACH ROW
EXECUTE FUNCTION enforce_load_execution_barcode_immutable();

REVOKE ALL ON FUNCTION enforce_load_execution_barcode_immutable()
FROM PUBLIC, wareboxes_app;
