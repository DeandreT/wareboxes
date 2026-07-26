CREATE FUNCTION session_user_id(token_hash TEXT)
RETURNS BIGINT
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT session.user_id
    FROM public.sessions session
    WHERE session.token = token_hash
      AND session.expires > CURRENT_TIMESTAMP
$$;

CREATE FUNCTION create_session_record(
    token_hash TEXT,
    user_id BIGINT
)
RETURNS VOID
LANGUAGE SQL
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    INSERT INTO public.sessions (token, user_id, created, expires)
    VALUES (
        token_hash,
        user_id,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP + INTERVAL '30 days'
    )
$$;

CREATE FUNCTION destroy_session_record(token_hash TEXT)
RETURNS VOID
LANGUAGE SQL
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    DELETE FROM public.sessions
    WHERE token = token_hash
$$;

ALTER TABLE tenant_memberships ENABLE ROW LEVEL SECURITY;
ALTER TABLE tenant_memberships FORCE ROW LEVEL SECURITY;

CREATE POLICY tenant_memberships_tenant_isolation
ON tenant_memberships
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

CREATE POLICY tenant_memberships_session_visibility
ON tenant_memberships
FOR SELECT
USING (
    user_id = session_user_id(
        NULLIF(current_setting('wareboxes.session_token_hash', true), '')
    )
);

ALTER TABLE user_facilities ENABLE ROW LEVEL SECURITY;
ALTER TABLE user_facilities FORCE ROW LEVEL SECURITY;

CREATE POLICY user_facilities_tenant_isolation
ON user_facilities
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

CREATE POLICY user_facilities_session_visibility
ON user_facilities
FOR SELECT
USING (
    user_id = session_user_id(
        NULLIF(current_setting('wareboxes.session_token_hash', true), '')
    )
);

ALTER TABLE user_inventory_owners ENABLE ROW LEVEL SECURITY;
ALTER TABLE user_inventory_owners FORCE ROW LEVEL SECURITY;

CREATE POLICY user_inventory_owners_tenant_isolation
ON user_inventory_owners
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

CREATE POLICY user_inventory_owners_session_visibility
ON user_inventory_owners
FOR SELECT
USING (
    user_id = session_user_id(
        NULLIF(current_setting('wareboxes.session_token_hash', true), '')
    )
);

REVOKE ALL ON sessions FROM PUBLIC, wareboxes_app;
REVOKE ALL ON
    tenant_memberships,
    user_facilities,
    user_inventory_owners
FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT, UPDATE ON
    tenant_memberships,
    user_facilities,
    user_inventory_owners
TO wareboxes_app;

REVOKE ALL ON SEQUENCE
    tenant_memberships_id_seq,
    user_facilities_id_seq,
    user_inventory_owners_id_seq
FROM PUBLIC, wareboxes_app;

GRANT USAGE ON SEQUENCE
    tenant_memberships_id_seq,
    user_facilities_id_seq,
    user_inventory_owners_id_seq
TO wareboxes_app;

REVOKE ALL ON FUNCTION
    session_user_id(TEXT),
    create_session_record(TEXT, BIGINT),
    destroy_session_record(TEXT)
FROM PUBLIC, wareboxes_app;

GRANT EXECUTE ON FUNCTION
    session_user_id(TEXT),
    create_session_record(TEXT, BIGINT),
    destroy_session_record(TEXT)
TO wareboxes_app;
