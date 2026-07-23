CREATE OR REPLACE FUNCTION assert_employee_active_facility(
    checked_tenant_id BIGINT,
    checked_employee_id BIGINT
)
RETURNS void AS $$
BEGIN
    PERFORM pg_advisory_xact_lock(
        hashtextextended(
            'employee-facility:' || checked_tenant_id::TEXT || ':' || checked_employee_id::TEXT,
            0
        )
    );

    IF EXISTS (
        SELECT 1
        FROM employees employee
        WHERE employee.tenant_id = checked_tenant_id
          AND employee.id = checked_employee_id
          AND employee.deleted IS NULL
    ) AND NOT EXISTS (
        SELECT 1
        FROM employee_facilities employee_facility
        INNER JOIN facilities facility
            ON facility.tenant_id = employee_facility.tenant_id
           AND facility.id = employee_facility.facility_id
           AND facility.deleted IS NULL
        WHERE employee_facility.tenant_id = checked_tenant_id
          AND employee_facility.employee_id = checked_employee_id
          AND employee_facility.deleted IS NULL
    ) THEN
        RAISE EXCEPTION 'active employee requires an active facility'
            USING ERRCODE = '23514';
    END IF;

    RETURN;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION enforce_employee_active_facility()
RETURNS trigger AS $$
BEGIN
    IF TG_TABLE_NAME = 'employees' THEN
        PERFORM assert_employee_active_facility(NEW.tenant_id, NEW.id);
    ELSIF TG_OP = 'DELETE' THEN
        PERFORM assert_employee_active_facility(OLD.tenant_id, OLD.employee_id);
    ELSIF TG_OP = 'INSERT' THEN
        PERFORM assert_employee_active_facility(NEW.tenant_id, NEW.employee_id);
    ELSE
        IF (NEW.tenant_id, NEW.employee_id) < (OLD.tenant_id, OLD.employee_id) THEN
            PERFORM assert_employee_active_facility(NEW.tenant_id, NEW.employee_id);
            PERFORM assert_employee_active_facility(OLD.tenant_id, OLD.employee_id);
        ELSE
            PERFORM assert_employee_active_facility(OLD.tenant_id, OLD.employee_id);
            IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
                OR NEW.employee_id IS DISTINCT FROM OLD.employee_id
            THEN
                PERFORM assert_employee_active_facility(NEW.tenant_id, NEW.employee_id);
            END IF;
        END IF;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER employee_requires_active_facility
    AFTER INSERT OR UPDATE OF deleted ON employees
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION enforce_employee_active_facility();

CREATE CONSTRAINT TRIGGER employee_facility_preserves_assignment
    AFTER INSERT OR UPDATE OR DELETE ON employee_facilities
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION enforce_employee_active_facility();

CREATE OR REPLACE FUNCTION retire_deleted_facility_employee_assignments()
RETURNS trigger AS $$
DECLARE
    checked_employee_id BIGINT;
BEGIN
    IF NEW.deleted IS NOT NULL AND OLD.deleted IS NULL THEN
        FOR checked_employee_id IN
            UPDATE employee_facilities
            SET deleted = NEW.deleted
            WHERE tenant_id = NEW.tenant_id
              AND facility_id = NEW.id
              AND deleted IS NULL
            RETURNING employee_id
        LOOP
            PERFORM assert_employee_active_facility(NEW.tenant_id, checked_employee_id);
        END LOOP;
    END IF;

    RETURN NULL;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER facility_deletion_preserves_employee_assignment
    AFTER UPDATE OF deleted ON facilities
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION retire_deleted_facility_employee_assignments();
