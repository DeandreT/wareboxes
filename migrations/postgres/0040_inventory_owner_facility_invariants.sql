ALTER TABLE inventory_balances
    ADD CONSTRAINT inventory_balances_owner_facility_fkey
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
    REFERENCES inventory_owner_facilities(
        tenant_id,
        inventory_owner_id,
        facility_id
    );

ALTER TABLE inventory_entries
    ADD CONSTRAINT inventory_entries_owner_facility_fkey
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
    REFERENCES inventory_owner_facilities(
        tenant_id,
        inventory_owner_id,
        facility_id
    );

ALTER TABLE license_plates
    ADD CONSTRAINT license_plates_owner_facility_fkey
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
    REFERENCES inventory_owner_facilities(
        tenant_id,
        inventory_owner_id,
        facility_id
    );

ALTER TABLE inventory_reservations
    ADD CONSTRAINT inventory_reservations_owner_facility_fkey
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
    REFERENCES inventory_owner_facilities(
        tenant_id,
        inventory_owner_id,
        facility_id
    );

ALTER TABLE outbox_events
    ADD CONSTRAINT outbox_events_owner_facility_fkey
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
    REFERENCES inventory_owner_facilities(
        tenant_id,
        inventory_owner_id,
        facility_id
    );

ALTER TABLE inventory_transactions
    DROP CONSTRAINT inventory_transactions_actor_user_id_fkey,
    ADD CONSTRAINT inventory_transactions_actor_membership_fkey
    FOREIGN KEY (tenant_id, actor_user_id)
    REFERENCES tenant_memberships(tenant_id, user_id);

CREATE INDEX inventory_reservations_owner_facility_idx
    ON inventory_reservations(tenant_id, inventory_owner_id, facility_id);

CREATE INDEX outbox_events_owner_facility_idx
    ON outbox_events(tenant_id, inventory_owner_id, facility_id);

CREATE FUNCTION require_active_inventory_owner_facility()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
BEGIN
    PERFORM 1
    FROM public.inventory_owner_facilities assignment
    INNER JOIN public.inventory_owners owner
        ON owner.tenant_id = assignment.tenant_id
       AND owner.id = assignment.inventory_owner_id
       AND owner.deleted IS NULL
    INNER JOIN public.facilities facility
        ON facility.tenant_id = assignment.tenant_id
       AND facility.id = assignment.facility_id
       AND facility.deleted IS NULL
    WHERE assignment.tenant_id = NEW.tenant_id
      AND assignment.inventory_owner_id = NEW.inventory_owner_id
      AND assignment.facility_id = NEW.facility_id
      AND assignment.deleted IS NULL
    FOR SHARE OF assignment;

    IF NOT FOUND THEN
        RAISE EXCEPTION
            'inventory owner is not active at the selected facility'
            USING ERRCODE = '23503';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER inventory_balances_require_active_owner_facility
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, facility_id
    ON inventory_balances
    FOR EACH ROW
    EXECUTE FUNCTION require_active_inventory_owner_facility();

CREATE TRIGGER inventory_entries_require_active_owner_facility
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, facility_id
    ON inventory_entries
    FOR EACH ROW
    EXECUTE FUNCTION require_active_inventory_owner_facility();

CREATE TRIGGER license_plates_require_active_owner_facility
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, facility_id
    ON license_plates
    FOR EACH ROW
    EXECUTE FUNCTION require_active_inventory_owner_facility();

CREATE TRIGGER inventory_reservations_require_active_owner_facility
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, facility_id
    ON inventory_reservations
    FOR EACH ROW
    EXECUTE FUNCTION require_active_inventory_owner_facility();

CREATE FUNCTION protect_inventory_owner_facility_assignment()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    referenced_tenant_id BIGINT := OLD.tenant_id;
    referenced_inventory_owner_id BIGINT := OLD.inventory_owner_id;
    referenced_facility_id BIGINT := OLD.facility_id;
BEGIN
    IF TG_OP = 'UPDATE' THEN
        IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
            OR NEW.inventory_owner_id IS DISTINCT FROM OLD.inventory_owner_id
            OR NEW.facility_id IS DISTINCT FROM OLD.facility_id
        THEN
            RAISE EXCEPTION 'inventory owner facility dimensions are immutable'
                USING ERRCODE = '55000';
        END IF;

        IF OLD.deleted IS NOT NULL OR NEW.deleted IS NULL THEN
            RETURN NEW;
        END IF;
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.inventory_balances balance
        WHERE balance.tenant_id = referenced_tenant_id
          AND balance.inventory_owner_id = referenced_inventory_owner_id
          AND balance.facility_id = referenced_facility_id
          AND (balance.qty_on_hand > 0 OR balance.qty_reserved > 0)
    ) THEN
        RAISE EXCEPTION
            'inventory owner facility assignment has positive or reserved inventory'
            USING ERRCODE = '55000';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.inventory_reservations reservation
        WHERE reservation.tenant_id = referenced_tenant_id
          AND reservation.inventory_owner_id = referenced_inventory_owner_id
          AND reservation.facility_id = referenced_facility_id
          AND reservation.deleted IS NULL
          AND reservation.status = 'reserved'
    ) THEN
        RAISE EXCEPTION
            'inventory owner facility assignment has active reservations'
            USING ERRCODE = '55000';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.license_plates license_plate
        WHERE license_plate.tenant_id = referenced_tenant_id
          AND license_plate.inventory_owner_id =
              referenced_inventory_owner_id
          AND license_plate.facility_id = referenced_facility_id
          AND license_plate.deleted IS NULL
    ) THEN
        RAISE EXCEPTION
            'inventory owner facility assignment has active license plates'
            USING ERRCODE = '55000';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.work_tasks task
        WHERE task.tenant_id = referenced_tenant_id
          AND task.inventory_owner_id = referenced_inventory_owner_id
          AND task.facility_id = referenced_facility_id
          AND task.deleted IS NULL
          AND task.status IN ('open', 'assigned', 'in_progress')
    ) THEN
        RAISE EXCEPTION
            'inventory owner facility assignment has executable work'
            USING ERRCODE = '55000';
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER inventory_owner_facilities_protect_active_references
    BEFORE UPDATE OF tenant_id, inventory_owner_id, facility_id, deleted
        OR DELETE
    ON inventory_owner_facilities
    FOR EACH ROW
    EXECUTE FUNCTION protect_inventory_owner_facility_assignment();

CREATE CONSTRAINT TRIGGER inventory_owner_facilities_verify_retirement
    AFTER UPDATE OR DELETE
    ON inventory_owner_facilities
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION protect_inventory_owner_facility_assignment();

CREATE OR REPLACE FUNCTION enforce_inventory_transaction_conservation()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM public.inventory_entries entry
        WHERE entry.tenant_id = NEW.tenant_id
          AND entry.inventory_owner_id = NEW.inventory_owner_id
          AND entry.transaction_id = NEW.id
    ) THEN
        RAISE EXCEPTION 'inventory transaction must contain at least one entry'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.transaction_type = 'move' AND EXISTS (
        SELECT 1
        FROM public.inventory_entries entry
        WHERE entry.tenant_id = NEW.tenant_id
          AND entry.inventory_owner_id = NEW.inventory_owner_id
          AND entry.transaction_id = NEW.id
        GROUP BY entry.inventory_owner_id, entry.item_id, entry.uom, entry.lot,
                 entry.expiration, entry.serial, entry.status
        HAVING SUM(entry.quantity_delta) <> 0
    ) THEN
        RAISE EXCEPTION
            'inventory move entries must conserve quantity for every stock dimension'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.transaction_type = 'move' AND (
        SELECT COUNT(DISTINCT entry.facility_id)
        FROM public.inventory_entries entry
        WHERE entry.tenant_id = NEW.tenant_id
          AND entry.inventory_owner_id = NEW.inventory_owner_id
          AND entry.transaction_id = NEW.id
    ) > 1 THEN
        RAISE EXCEPTION 'inventory moves cannot span facilities'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.transaction_type = 'receive' AND EXISTS (
        SELECT 1
        FROM public.inventory_entries entry
        WHERE entry.tenant_id = NEW.tenant_id
          AND entry.inventory_owner_id = NEW.inventory_owner_id
          AND entry.transaction_id = NEW.id
          AND entry.quantity_delta <= 0
    ) THEN
        RAISE EXCEPTION 'inventory receipt entries must be positive'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.transaction_type = 'ship' AND EXISTS (
        SELECT 1
        FROM public.inventory_entries entry
        WHERE entry.tenant_id = NEW.tenant_id
          AND entry.inventory_owner_id = NEW.inventory_owner_id
          AND entry.transaction_id = NEW.id
          AND entry.quantity_delta >= 0
    ) THEN
        RAISE EXCEPTION 'inventory shipment entries must be negative'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

REVOKE ALL ON FUNCTION
    require_active_inventory_owner_facility(),
    protect_inventory_owner_facility_assignment(),
    enforce_inventory_transaction_conservation()
FROM PUBLIC, wareboxes_app;

REVOKE ALL ON inventory_owner_facilities FROM PUBLIC, wareboxes_app;
GRANT SELECT, INSERT, UPDATE ON inventory_owner_facilities TO wareboxes_app;

REVOKE ALL ON SEQUENCE inventory_owner_facilities_id_seq
FROM PUBLIC, wareboxes_app;
GRANT USAGE ON SEQUENCE inventory_owner_facilities_id_seq TO wareboxes_app;
