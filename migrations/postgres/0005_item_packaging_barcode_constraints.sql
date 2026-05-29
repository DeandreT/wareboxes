ALTER TABLE items
    ADD CONSTRAINT items_packaging_unit_check CHECK (packaging_unit IN ('each', 'case'));

ALTER TABLE barcodes
    ADD CONSTRAINT barcodes_type_check CHECK (type IN ('code128', 'qr'));
