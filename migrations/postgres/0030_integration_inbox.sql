CREATE TABLE integration_inbox_receipts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT,
    facility_id BIGINT,
    received_at TIMESTAMPTZ NOT NULL,
    source_key TEXT NOT NULL,
    deduplication_key TEXT NOT NULL,
    content_type TEXT NOT NULL,
    raw_payload BYTEA NOT NULL,
    payload_sha256 BYTEA NOT NULL,
    request_id TEXT,
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id)
        REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id)
        REFERENCES facilities(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(
            tenant_id,
            inventory_owner_id,
            facility_id
        ),
    CHECK (
        source_key = btrim(source_key)
        AND char_length(source_key) BETWEEN 1 AND 200
    ),
    CHECK (
        deduplication_key = btrim(deduplication_key)
        AND char_length(deduplication_key) BETWEEN 1 AND 500
    ),
    CHECK (
        content_type = btrim(content_type)
        AND char_length(content_type) BETWEEN 1 AND 255
    ),
    CHECK (octet_length(raw_payload) <= 16777216),
    CHECK (octet_length(payload_sha256) = 32),
    CHECK (
        request_id IS NULL
        OR (
            request_id = btrim(request_id)
            AND char_length(request_id) BETWEEN 1 AND 128
        )
    )
);

CREATE TABLE integration_inbox_keys (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    source_key TEXT NOT NULL,
    deduplication_key TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL,
    receipt_id BIGINT NOT NULL,
    inventory_owner_id BIGINT,
    facility_id BIGINT,
    content_type TEXT NOT NULL,
    payload_sha256 BYTEA NOT NULL,
    PRIMARY KEY (tenant_id, source_key, deduplication_key),
    UNIQUE (tenant_id, receipt_id),
    FOREIGN KEY (tenant_id, inventory_owner_id)
        REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id)
        REFERENCES facilities(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(
            tenant_id,
            inventory_owner_id,
            facility_id
        ),
    CHECK (
        source_key = btrim(source_key)
        AND char_length(source_key) BETWEEN 1 AND 200
    ),
    CHECK (
        deduplication_key = btrim(deduplication_key)
        AND char_length(deduplication_key) BETWEEN 1 AND 500
    ),
    CHECK (receipt_id > 0),
    CHECK (
        content_type = btrim(content_type)
        AND char_length(content_type) BETWEEN 1 AND 255
    ),
    CHECK (octet_length(payload_sha256) = 32)
);

CREATE INDEX integration_inbox_receipts_tenant_history_idx
    ON integration_inbox_receipts(tenant_id, received_at, id);

ALTER TABLE integration_inbox_receipts ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration_inbox_receipts FORCE ROW LEVEL SECURITY;

CREATE POLICY integration_inbox_receipts_tenant_isolation
ON integration_inbox_receipts
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE integration_inbox_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE integration_inbox_keys FORCE ROW LEVEL SECURITY;

CREATE POLICY integration_inbox_keys_tenant_isolation
ON integration_inbox_keys
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

CREATE OR REPLACE FUNCTION require_integration_inbox_receipt_for_key()
RETURNS trigger AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM integration_inbox_receipts receipt
        WHERE receipt.tenant_id = NEW.tenant_id
          AND receipt.id = NEW.receipt_id
          AND receipt.source_key = NEW.source_key
          AND receipt.deduplication_key = NEW.deduplication_key
          AND receipt.inventory_owner_id
                IS NOT DISTINCT FROM NEW.inventory_owner_id
          AND receipt.facility_id IS NOT DISTINCT FROM NEW.facility_id
          AND receipt.content_type = NEW.content_type
          AND receipt.payload_sha256 = NEW.payload_sha256
    ) THEN
        RAISE EXCEPTION 'integration inbox key must match its receipt envelope'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER integration_inbox_keys_require_receipt
    BEFORE INSERT ON integration_inbox_keys
    FOR EACH ROW EXECUTE FUNCTION require_integration_inbox_receipt_for_key();

CREATE OR REPLACE FUNCTION protect_integration_inbox_receipt_envelope()
RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'integration inbox receipt envelopes are immutable'
        USING ERRCODE = '55000';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER integration_inbox_receipts_are_immutable
    BEFORE UPDATE ON integration_inbox_receipts
    FOR EACH ROW EXECUTE FUNCTION protect_integration_inbox_receipt_envelope();

CREATE OR REPLACE FUNCTION protect_integration_inbox_key()
RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'integration inbox keys are permanent'
        USING ERRCODE = '55000';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER integration_inbox_keys_are_permanent
    BEFORE UPDATE OR DELETE ON integration_inbox_keys
    FOR EACH ROW EXECUTE FUNCTION protect_integration_inbox_key();

REVOKE ALL ON integration_inbox_receipts FROM PUBLIC, wareboxes_app;
REVOKE ALL ON integration_inbox_keys FROM PUBLIC, wareboxes_app;
REVOKE ALL ON SEQUENCE integration_inbox_receipts_id_seq
    FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT ON integration_inbox_receipts TO wareboxes_app;
GRANT SELECT, INSERT ON integration_inbox_keys TO wareboxes_app;
GRANT USAGE ON SEQUENCE integration_inbox_receipts_id_seq TO wareboxes_app;
