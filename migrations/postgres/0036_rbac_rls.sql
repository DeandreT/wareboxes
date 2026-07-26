ALTER TABLE roles ENABLE ROW LEVEL SECURITY;
ALTER TABLE roles FORCE ROW LEVEL SECURITY;

CREATE POLICY roles_tenant_isolation
ON roles
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE permissions ENABLE ROW LEVEL SECURITY;
ALTER TABLE permissions FORCE ROW LEVEL SECURITY;

CREATE POLICY permissions_tenant_isolation
ON permissions
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE user_roles ENABLE ROW LEVEL SECURITY;
ALTER TABLE user_roles FORCE ROW LEVEL SECURITY;

CREATE POLICY user_roles_tenant_isolation
ON user_roles
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE role_permissions ENABLE ROW LEVEL SECURITY;
ALTER TABLE role_permissions FORCE ROW LEVEL SECURITY;

CREATE POLICY role_permissions_tenant_isolation
ON role_permissions
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

CREATE FUNCTION guard_role_hierarchy()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(
        hashtextextended('role-hierarchy:' || NEW.tenant_id::TEXT, 0)
    );

    IF NEW.parent_id IS NULL THEN
        RETURN NEW;
    END IF;
    IF NEW.parent_id = NEW.id THEN
        RAISE EXCEPTION 'a role cannot be its own parent'
            USING ERRCODE = '23514';
    END IF;
    IF EXISTS (
        WITH RECURSIVE ancestors(id, parent_id) AS (
            SELECT role.id, role.parent_id
            FROM roles role
            WHERE role.tenant_id = NEW.tenant_id
              AND role.id = NEW.parent_id
            UNION
            SELECT role.id, role.parent_id
            FROM roles role
            INNER JOIN ancestors ancestor ON ancestor.parent_id = role.id
            WHERE role.tenant_id = NEW.tenant_id
        )
        SELECT 1 FROM ancestors WHERE id = NEW.id
    ) THEN
        RAISE EXCEPTION 'role hierarchy cannot contain a cycle'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER roles_hierarchy_guard
BEFORE INSERT OR UPDATE OF tenant_id, parent_id ON roles
FOR EACH ROW EXECUTE FUNCTION guard_role_hierarchy();

CREATE FUNCTION guard_self_role()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF OLD.self_user_id IS NULL AND NEW.self_user_id IS NOT NULL THEN
        RAISE EXCEPTION 'a regular role cannot become a self role'
            USING ERRCODE = '23514';
    END IF;
    IF OLD.self_user_id IS NOT NULL AND (
        NEW.self_user_id IS DISTINCT FROM OLD.self_user_id
        OR NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
        OR NEW.parent_id IS NOT NULL
        OR NEW.deleted IS NOT NULL
        OR NEW.description IS DISTINCT FROM 'Self role'
    ) THEN
        RAISE EXCEPTION 'self role invariants cannot be changed'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER roles_self_role_guard
BEFORE UPDATE ON roles
FOR EACH ROW EXECUTE FUNCTION guard_self_role();

CREATE FUNCTION guard_self_user_role()
RETURNS TRIGGER
LANGUAGE plpgsql
AS $$
BEGIN
    IF (
        NEW.deleted IS NOT NULL
        OR NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
        OR NEW.user_id IS DISTINCT FROM OLD.user_id
        OR NEW.role_id IS DISTINCT FROM OLD.role_id
    ) AND EXISTS (
        SELECT 1
        FROM roles role
        WHERE role.tenant_id = OLD.tenant_id
          AND role.id = OLD.role_id
          AND role.self_user_id = OLD.user_id
    ) THEN
        RAISE EXCEPTION 'self role assignment cannot be changed'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER user_roles_self_role_guard
BEFORE UPDATE ON user_roles
FOR EACH ROW EXECUTE FUNCTION guard_self_user_role();

REVOKE ALL ON roles, permissions, user_roles, role_permissions
FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT, UPDATE ON roles, permissions, user_roles, role_permissions
TO wareboxes_app;

REVOKE ALL ON SEQUENCE
    roles_id_seq,
    permissions_id_seq,
    user_roles_id_seq,
    role_permissions_id_seq
FROM PUBLIC, wareboxes_app;

GRANT USAGE ON SEQUENCE
    roles_id_seq,
    permissions_id_seq,
    user_roles_id_seq,
    role_permissions_id_seq
TO wareboxes_app;

REVOKE ALL ON FUNCTION
    guard_role_hierarchy(),
    guard_self_role(),
    guard_self_user_role()
FROM PUBLIC, wareboxes_app;
