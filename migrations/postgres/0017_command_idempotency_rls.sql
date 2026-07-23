ALTER TABLE command_idempotency_records ENABLE ROW LEVEL SECURITY;
ALTER TABLE command_idempotency_records FORCE ROW LEVEL SECURITY;

CREATE POLICY command_idempotency_records_tenant_isolation
    ON command_idempotency_records
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

REVOKE CREATE ON SCHEMA public FROM PUBLIC;
DO $$
BEGIN
    EXECUTE format(
        'REVOKE TEMPORARY ON DATABASE %I FROM PUBLIC',
        current_database()
    );
END
$$;

GRANT USAGE ON SCHEMA public TO wareboxes_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA public TO wareboxes_app;
GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA public TO wareboxes_app;
REVOKE ALL ON TABLE public._sqlx_migrations FROM wareboxes_app;
