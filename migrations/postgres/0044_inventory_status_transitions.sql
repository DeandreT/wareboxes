ALTER TABLE inventory_transactions
    DROP CONSTRAINT inventory_transactions_transaction_type_check,
    ADD CONSTRAINT inventory_transactions_transaction_type_check
    CHECK (
        transaction_type IN (
            'receive',
            'move',
            'adjust',
            'ship',
            'status_change'
        )
    );

CREATE TABLE inventory_status_transitions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    transaction_id BIGINT NOT NULL,
    source_balance_id BIGINT NOT NULL,
    destination_balance_id BIGINT NOT NULL,
    from_status TEXT NOT NULL,
    to_status TEXT NOT NULL,
    qty BIGINT NOT NULL,
    reason_code TEXT NOT NULL,
    reason_note TEXT,
    reference_type TEXT,
    reference_id BIGINT,
    created_by BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT statement_timestamp(),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    UNIQUE (tenant_id, inventory_owner_id, transaction_id),
    FOREIGN KEY (tenant_id, inventory_owner_id, transaction_id)
        REFERENCES inventory_transactions(
            tenant_id,
            inventory_owner_id,
            id
        ),
    FOREIGN KEY (tenant_id, created_by)
        REFERENCES tenant_memberships(tenant_id, user_id),
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id)
        REFERENCES inventory_owner_facilities(
            tenant_id,
            inventory_owner_id,
            facility_id
        ),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        source_balance_id
    )
        REFERENCES inventory_balances(
            tenant_id,
            inventory_owner_id,
            facility_id,
            id
        ),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        destination_balance_id
    )
        REFERENCES inventory_balances(
            tenant_id,
            inventory_owner_id,
            facility_id,
            id
        ),
    CHECK (source_balance_id <> destination_balance_id),
    CHECK (
        from_status IN ('available', 'hold', 'damaged', 'quarantine')
    ),
    CHECK (
        to_status IN ('available', 'hold', 'damaged', 'quarantine')
    ),
    CHECK (from_status <> to_status),
    CHECK (qty > 0),
    CHECK (
        reason_code IN (
            'quality_inspection',
            'damage_suspected',
            'damage_confirmed',
            'inspection_passed',
            'inventory_discrepancy',
            'discrepancy_resolved',
            'regulatory_restriction',
            'regulatory_release',
            'customer_request',
            'customer_release',
            'other'
        )
    ),
    CHECK (
        (
            reason_code IN (
                'quality_inspection',
                'damage_suspected',
                'inventory_discrepancy',
                'regulatory_restriction',
                'customer_request'
            )
            AND to_status IN ('hold', 'quarantine')
        )
        OR (
            reason_code = 'damage_confirmed'
            AND to_status = 'damaged'
        )
        OR (
            reason_code IN (
                'inspection_passed',
                'discrepancy_resolved',
                'regulatory_release',
                'customer_release'
            )
            AND to_status = 'available'
        )
        OR reason_code = 'other'
    ),
    CHECK (
        reason_note IS NULL
        OR (
            btrim(reason_note) <> ''
            AND reason_note = btrim(reason_note)
            AND char_length(reason_note) <= 1000
        )
    ),
    CHECK (
        reason_code <> 'other'
        OR (
            reason_note IS NOT NULL
            AND btrim(reason_note) <> ''
        )
    ),
    CHECK (
        (reference_type IS NULL AND reference_id IS NULL)
        OR (
            reference_type IS NOT NULL
            AND btrim(reference_type) <> ''
            AND reference_type = btrim(reference_type)
            AND char_length(reference_type) <= 100
            AND reference_id IS NOT NULL
            AND reference_id > 0
        )
    )
);

CREATE INDEX inventory_status_transitions_source_balance_idx
    ON inventory_status_transitions(
        tenant_id,
        inventory_owner_id,
        source_balance_id,
        created
    );

CREATE INDEX inventory_status_transitions_destination_balance_idx
    ON inventory_status_transitions(
        tenant_id,
        inventory_owner_id,
        destination_balance_id,
        created
    );

CREATE INDEX inventory_status_transitions_reference_idx
    ON inventory_status_transitions(
        tenant_id,
        inventory_owner_id,
        reference_type,
        reference_id
    )
    WHERE reference_type IS NOT NULL;

ALTER TABLE inventory_status_transitions ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_status_transitions FORCE ROW LEVEL SECURITY;

CREATE POLICY inventory_status_transitions_tenant_isolation
    ON inventory_status_transitions
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

CREATE TRIGGER inventory_status_transitions_are_immutable
    BEFORE UPDATE OR DELETE
    ON inventory_status_transitions
    FOR EACH ROW
    EXECUTE FUNCTION reject_inventory_journal_mutation();

CREATE FUNCTION reject_direct_inventory_balance_status_update()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF NEW.status IS DISTINCT FROM OLD.status THEN
        RAISE EXCEPTION
            'inventory balance status changes require a status-change transaction'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER inventory_balances_reject_direct_status_update
    BEFORE UPDATE OF status
    ON inventory_balances
    FOR EACH ROW
    EXECUTE FUNCTION reject_direct_inventory_balance_status_update();

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

    IF NEW.transaction_type = 'status_change' AND EXISTS (
        SELECT 1
        FROM public.inventory_entries entry
        WHERE entry.tenant_id = NEW.tenant_id
          AND entry.inventory_owner_id = NEW.inventory_owner_id
          AND entry.transaction_id = NEW.id
        GROUP BY entry.inventory_owner_id, entry.item_batch_id, entry.item_id,
                 entry.uom, entry.lot, entry.expiration, entry.serial
        HAVING SUM(entry.quantity_delta) <> 0
    ) THEN
        RAISE EXCEPTION
            'inventory status-change entries must conserve quantity'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.transaction_type = 'status_change' AND (
        SELECT COUNT(DISTINCT entry.facility_id)
        FROM public.inventory_entries entry
        WHERE entry.tenant_id = NEW.tenant_id
          AND entry.inventory_owner_id = NEW.inventory_owner_id
          AND entry.transaction_id = NEW.id
    ) > 1 THEN
        RAISE EXCEPTION 'inventory status changes cannot span facilities'
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

CREATE FUNCTION enforce_inventory_status_transition()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    target_tenant_id BIGINT;
    target_inventory_owner_id BIGINT;
    target_transaction_id BIGINT;
    target_transaction_type TEXT;
    target_actor_user_id BIGINT;
    target_reason TEXT;
    target_reference_type TEXT;
    target_reference_id BIGINT;
    transition_count BIGINT;
    entry_count BIGINT;
    transition_row public.inventory_status_transitions%ROWTYPE;
    source_balance public.inventory_balances%ROWTYPE;
    destination_balance public.inventory_balances%ROWTYPE;
BEGIN
    IF TG_TABLE_NAME = 'inventory_transactions' THEN
        target_tenant_id := NEW.tenant_id;
        target_inventory_owner_id := NEW.inventory_owner_id;
        target_transaction_id := NEW.id;
        target_transaction_type := NEW.transaction_type;
        target_actor_user_id := NEW.actor_user_id;
        target_reason := NEW.reason;
        target_reference_type := NEW.reference_type;
        target_reference_id := NEW.reference_id;
    ELSE
        target_tenant_id := NEW.tenant_id;
        target_inventory_owner_id := NEW.inventory_owner_id;
        target_transaction_id := NEW.transaction_id;

        SELECT transaction.transaction_type,
               transaction.actor_user_id,
               transaction.reason,
               transaction.reference_type,
               transaction.reference_id
        INTO target_transaction_type,
             target_actor_user_id,
             target_reason,
             target_reference_type,
             target_reference_id
        FROM public.inventory_transactions transaction
        WHERE transaction.tenant_id = target_tenant_id
          AND transaction.inventory_owner_id = target_inventory_owner_id
          AND transaction.id = target_transaction_id;

        IF NOT FOUND THEN
            RAISE EXCEPTION
                'inventory status transition transaction does not exist'
                USING ERRCODE = '23503';
        END IF;
    END IF;

    SELECT COUNT(*)
    INTO transition_count
    FROM public.inventory_status_transitions transition
    WHERE transition.tenant_id = target_tenant_id
      AND transition.inventory_owner_id = target_inventory_owner_id
      AND transition.transaction_id = target_transaction_id;

    IF target_transaction_type <> 'status_change' THEN
        IF transition_count > 0 THEN
            RAISE EXCEPTION
                'inventory status transition audit requires a status-change transaction'
                USING ERRCODE = '23514';
        END IF;

        RETURN NEW;
    END IF;

    IF transition_count <> 1 THEN
        RAISE EXCEPTION
            'inventory status-change transaction requires exactly one audit row'
            USING ERRCODE = '23514';
    END IF;

    SELECT transition.*
    INTO transition_row
    FROM public.inventory_status_transitions transition
    WHERE transition.tenant_id = target_tenant_id
      AND transition.inventory_owner_id = target_inventory_owner_id
      AND transition.transaction_id = target_transaction_id;

    IF transition_row.created_by IS DISTINCT FROM target_actor_user_id
       OR transition_row.reason_code IS DISTINCT FROM target_reason
       OR transition_row.reference_type IS DISTINCT FROM
            target_reference_type
       OR transition_row.reference_id IS DISTINCT FROM target_reference_id
    THEN
        RAISE EXCEPTION
            'inventory status-change audit does not match its transaction metadata'
            USING ERRCODE = '23514';
    END IF;

    SELECT balance.*
    INTO source_balance
    FROM public.inventory_balances balance
    WHERE balance.tenant_id = transition_row.tenant_id
      AND balance.inventory_owner_id =
          transition_row.inventory_owner_id
      AND balance.facility_id = transition_row.facility_id
      AND balance.id = transition_row.source_balance_id
      AND balance.deleted IS NULL;

    IF NOT FOUND THEN
        RAISE EXCEPTION
            'inventory status-change source balance is not active'
            USING ERRCODE = '23514';
    END IF;

    SELECT balance.*
    INTO destination_balance
    FROM public.inventory_balances balance
    WHERE balance.tenant_id = transition_row.tenant_id
      AND balance.inventory_owner_id =
          transition_row.inventory_owner_id
      AND balance.facility_id = transition_row.facility_id
      AND balance.id = transition_row.destination_balance_id
      AND balance.deleted IS NULL;

    IF NOT FOUND THEN
        RAISE EXCEPTION
            'inventory status-change destination balance is not active'
            USING ERRCODE = '23514';
    END IF;

    IF source_balance.status IS DISTINCT FROM transition_row.from_status
       OR destination_balance.status IS DISTINCT FROM transition_row.to_status
    THEN
        RAISE EXCEPTION
            'inventory status-change balances do not match the audited statuses'
            USING ERRCODE = '23514';
    END IF;

    IF source_balance.location_id IS DISTINCT FROM
            destination_balance.location_id
       OR source_balance.license_plate_id IS DISTINCT FROM
            destination_balance.license_plate_id
       OR source_balance.item_batch_id IS DISTINCT FROM
            destination_balance.item_batch_id
       OR source_balance.item_id IS DISTINCT FROM
            destination_balance.item_id
       OR source_balance.uom IS DISTINCT FROM destination_balance.uom
    THEN
        RAISE EXCEPTION
            'inventory status-change balances may differ only by status'
            USING ERRCODE = '23514';
    END IF;

    SELECT COUNT(*)
    INTO entry_count
    FROM public.inventory_entries entry
    WHERE entry.tenant_id = target_tenant_id
      AND entry.inventory_owner_id = target_inventory_owner_id
      AND entry.transaction_id = target_transaction_id;

    IF entry_count <> 2 THEN
        RAISE EXCEPTION
            'inventory status-change transaction requires exactly two entries'
            USING ERRCODE = '23514';
    END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM public.inventory_entries entry
        WHERE entry.tenant_id = target_tenant_id
          AND entry.inventory_owner_id = target_inventory_owner_id
          AND entry.transaction_id = target_transaction_id
          AND entry.facility_id = transition_row.facility_id
          AND entry.location_id = source_balance.location_id
          AND entry.license_plate_id IS NOT DISTINCT FROM
              source_balance.license_plate_id
          AND entry.item_batch_id = source_balance.item_batch_id
          AND entry.item_id = source_balance.item_id
          AND entry.uom = source_balance.uom
          AND entry.status = transition_row.from_status
          AND entry.quantity_delta = -transition_row.qty
    ) THEN
        RAISE EXCEPTION
            'inventory status-change transaction is missing its source entry'
            USING ERRCODE = '23514';
    END IF;

    IF NOT EXISTS (
        SELECT 1
        FROM public.inventory_entries entry
        WHERE entry.tenant_id = target_tenant_id
          AND entry.inventory_owner_id = target_inventory_owner_id
          AND entry.transaction_id = target_transaction_id
          AND entry.facility_id = transition_row.facility_id
          AND entry.location_id = destination_balance.location_id
          AND entry.license_plate_id IS NOT DISTINCT FROM
              destination_balance.license_plate_id
          AND entry.item_batch_id = destination_balance.item_batch_id
          AND entry.item_id = destination_balance.item_id
          AND entry.uom = destination_balance.uom
          AND entry.status = transition_row.to_status
          AND entry.quantity_delta = transition_row.qty
    ) THEN
        RAISE EXCEPTION
            'inventory status-change transaction is missing its destination entry'
            USING ERRCODE = '23514';
    END IF;

    IF EXISTS (
        SELECT 1
        FROM public.inventory_reconciliation reconciliation
        WHERE reconciliation.tenant_id = target_tenant_id
          AND reconciliation.inventory_owner_id =
              target_inventory_owner_id
          AND reconciliation.facility_id = transition_row.facility_id
          AND reconciliation.location_id = source_balance.location_id
          AND reconciliation.license_plate_id IS NOT DISTINCT FROM
              source_balance.license_plate_id
          AND reconciliation.item_batch_id = source_balance.item_batch_id
          AND reconciliation.item_id = source_balance.item_id
          AND reconciliation.uom = source_balance.uom
          AND reconciliation.status IN (
              transition_row.from_status,
              transition_row.to_status
          )
    ) THEN
        RAISE EXCEPTION
            'inventory status-change balances do not reconcile with the journal'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE CONSTRAINT TRIGGER inventory_status_change_transaction_shape
    AFTER INSERT
    ON inventory_transactions
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION enforce_inventory_status_transition();

CREATE CONSTRAINT TRIGGER inventory_status_transition_audit_shape
    AFTER INSERT
    ON inventory_status_transitions
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION enforce_inventory_status_transition();

REVOKE ALL ON inventory_status_transitions FROM PUBLIC, wareboxes_app;
GRANT SELECT, INSERT ON inventory_status_transitions TO wareboxes_app;

REVOKE ALL ON SEQUENCE inventory_status_transitions_id_seq
FROM PUBLIC, wareboxes_app;
GRANT USAGE ON SEQUENCE inventory_status_transitions_id_seq TO wareboxes_app;

REVOKE ALL ON inventory_balances FROM PUBLIC, wareboxes_app;
GRANT SELECT, INSERT, UPDATE ON inventory_balances TO wareboxes_app;

REVOKE ALL ON inventory_transactions FROM PUBLIC, wareboxes_app;
GRANT SELECT, INSERT ON inventory_transactions TO wareboxes_app;

REVOKE ALL ON inventory_entries FROM PUBLIC, wareboxes_app;
GRANT SELECT, INSERT ON inventory_entries TO wareboxes_app;

REVOKE ALL ON inventory_reconciliation FROM PUBLIC, wareboxes_app;
GRANT SELECT ON inventory_reconciliation TO wareboxes_app;

REVOKE ALL ON SEQUENCE
    inventory_balances_id_seq,
    inventory_transactions_id_seq,
    inventory_entries_id_seq
FROM PUBLIC, wareboxes_app;
GRANT USAGE ON SEQUENCE
    inventory_balances_id_seq,
    inventory_transactions_id_seq,
    inventory_entries_id_seq
TO wareboxes_app;

REVOKE ALL ON FUNCTION
    reject_direct_inventory_balance_status_update(),
    enforce_inventory_transaction_conservation(),
    enforce_inventory_status_transition()
FROM PUBLIC, wareboxes_app;
