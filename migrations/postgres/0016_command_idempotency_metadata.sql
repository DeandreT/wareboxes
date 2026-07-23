ALTER TABLE command_idempotency_records
    ADD COLUMN actor_user_id BIGINT,
    ADD COLUMN request_id TEXT,
    ADD COLUMN result_schema_version INTEGER NOT NULL DEFAULT 1,
    ADD CONSTRAINT command_idempotency_records_actor_fkey
        FOREIGN KEY (tenant_id, actor_user_id)
        REFERENCES tenant_memberships(tenant_id, user_id),
    ADD CONSTRAINT command_idempotency_records_key_length_check CHECK (
        char_length(idempotency_key) BETWEEN 1 AND 200
    ),
    ADD CONSTRAINT command_idempotency_records_request_id_check CHECK (
        request_id IS NULL
        OR (btrim(request_id) <> '' AND char_length(request_id) <= 128)
    ),
    ADD CONSTRAINT command_idempotency_records_result_schema_version_check CHECK (
        result_schema_version > 0
    );
