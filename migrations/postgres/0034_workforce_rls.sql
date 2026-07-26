ALTER TABLE employees ENABLE ROW LEVEL SECURITY;
ALTER TABLE employees FORCE ROW LEVEL SECURITY;

CREATE POLICY employees_tenant_isolation
ON employees
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE employee_facilities ENABLE ROW LEVEL SECURITY;
ALTER TABLE employee_facilities FORCE ROW LEVEL SECURITY;

CREATE POLICY employee_facilities_tenant_isolation
ON employee_facilities
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

REVOKE ALL ON employees, employee_facilities FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT, UPDATE ON employees, employee_facilities TO wareboxes_app;

REVOKE ALL ON SEQUENCE employees_id_seq, employee_facilities_id_seq
FROM PUBLIC, wareboxes_app;

GRANT USAGE ON SEQUENCE employees_id_seq, employee_facilities_id_seq
TO wareboxes_app;

REVOKE ALL ON FUNCTION
    assert_employee_active_facility(BIGINT, BIGINT),
    enforce_employee_active_facility(),
    retire_deleted_facility_employee_assignments()
FROM PUBLIC, wareboxes_app;

GRANT EXECUTE ON FUNCTION assert_employee_active_facility(BIGINT, BIGINT)
TO wareboxes_app;
