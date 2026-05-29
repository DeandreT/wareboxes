ALTER TABLE barcodes
    DROP CONSTRAINT IF EXISTS barcodes_type_check;

ALTER TABLE barcodes
    ADD CONSTRAINT barcodes_type_check CHECK (type IN ('code128', 'gs1-128', 'upc-a', 'qr'));
