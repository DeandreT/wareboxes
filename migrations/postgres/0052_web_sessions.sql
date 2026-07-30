ALTER TABLE sessions
    ADD COLUMN purpose TEXT NOT NULL DEFAULT 'api',
    ADD COLUMN active_tenant_id BIGINT,
    ADD COLUMN last_seen_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP;

ALTER TABLE sessions
    ADD CONSTRAINT sessions_purpose_check
        CHECK (purpose IN ('api', 'web')),
    ADD CONSTRAINT sessions_web_context_check
        CHECK (
            (purpose = 'api' AND active_tenant_id IS NULL)
            OR purpose = 'web'
        ),
    ADD CONSTRAINT sessions_active_membership_fk
        FOREIGN KEY (active_tenant_id, user_id)
        REFERENCES tenant_memberships(tenant_id, user_id);

CREATE INDEX sessions_web_expiry_idx
    ON sessions (expires, last_seen_at)
    WHERE purpose = 'web';

CREATE FUNCTION api_session_user_id(token_hash TEXT)
RETURNS BIGINT
LANGUAGE SQL
STABLE
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    SELECT session.user_id
    FROM public.sessions session
    WHERE session.token = token_hash
      AND session.purpose = 'api'
      AND session.expires > CURRENT_TIMESTAMP
$$;

CREATE FUNCTION create_web_session_record(
    p_token_hash TEXT,
    p_user_id BIGINT,
    p_absolute_ttl_seconds INTEGER
)
RETURNS VOID
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF p_absolute_ttl_seconds < 300 OR p_absolute_ttl_seconds > 86400 THEN
        RAISE EXCEPTION 'web session TTL must be between 300 and 86400 seconds'
            USING ERRCODE = '22023';
    END IF;

    INSERT INTO public.sessions (
        token,
        user_id,
        created,
        expires,
        purpose,
        active_tenant_id,
        last_seen_at
    )
    VALUES (
        p_token_hash,
        p_user_id,
        CURRENT_TIMESTAMP,
        CURRENT_TIMESTAMP + p_absolute_ttl_seconds * INTERVAL '1 second',
        'web',
        NULL,
        CURRENT_TIMESTAMP
    );
END;
$$;

CREATE FUNCTION web_session_identity(
    p_token_hash TEXT,
    p_idle_ttl_seconds INTEGER
)
RETURNS TABLE(user_id BIGINT, tenant_id BIGINT)
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF p_idle_ttl_seconds < 60 OR p_idle_ttl_seconds > 86400 THEN
        RAISE EXCEPTION 'web session idle TTL must be between 60 and 86400 seconds'
            USING ERRCODE = '22023';
    END IF;

    RETURN QUERY
    UPDATE public.sessions session
    SET last_seen_at = CURRENT_TIMESTAMP
    FROM public.tenant_memberships membership,
         public.tenants tenant
    WHERE session.token = p_token_hash
      AND session.purpose = 'web'
      AND session.expires > CURRENT_TIMESTAMP
      AND session.last_seen_at >
          CURRENT_TIMESTAMP - p_idle_ttl_seconds * INTERVAL '1 second'
      AND session.active_tenant_id IS NOT NULL
      AND membership.tenant_id = session.active_tenant_id
      AND membership.user_id = session.user_id
      AND membership.deleted IS NULL
      AND tenant.id = membership.tenant_id
      AND tenant.deleted IS NULL
      AND tenant.status = 'active'
    RETURNING session.user_id, session.active_tenant_id;
END;
$$;

CREATE FUNCTION select_web_session_tenant(
    p_token_hash TEXT,
    p_selected_tenant_id BIGINT
)
RETURNS BOOLEAN
LANGUAGE SQL
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
    WITH selected AS (
        UPDATE public.sessions session
        SET active_tenant_id = p_selected_tenant_id,
            last_seen_at = CURRENT_TIMESTAMP
        WHERE session.token = p_token_hash
          AND session.purpose = 'web'
          AND session.expires > CURRENT_TIMESTAMP
          AND EXISTS (
              SELECT 1
              FROM public.tenant_memberships membership
              JOIN public.tenants tenant
                ON tenant.id = membership.tenant_id
              WHERE membership.tenant_id = p_selected_tenant_id
                AND membership.user_id = session.user_id
                AND membership.deleted IS NULL
                AND tenant.deleted IS NULL
                AND tenant.status = 'active'
          )
        RETURNING 1
    )
    SELECT EXISTS(SELECT 1 FROM selected)
$$;

REVOKE ALL ON FUNCTION
    api_session_user_id(TEXT),
    create_web_session_record(TEXT, BIGINT, INTEGER),
    web_session_identity(TEXT, INTEGER),
    select_web_session_tenant(TEXT, BIGINT)
FROM PUBLIC, wareboxes_app;

GRANT EXECUTE ON FUNCTION
    api_session_user_id(TEXT),
    create_web_session_record(TEXT, BIGINT, INTEGER),
    web_session_identity(TEXT, INTEGER),
    select_web_session_tenant(TEXT, BIGINT)
TO wareboxes_app;
