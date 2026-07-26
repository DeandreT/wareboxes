CREATE TABLE inventory_projection_changes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    transaction_id BIGINT NOT NULL,
    inventory_balance_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL DEFAULT statement_timestamp(),
    facility_id BIGINT NOT NULL,
    location_id BIGINT NOT NULL,
    license_plate_id BIGINT,
    item_batch_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    uom TEXT NOT NULL,
    lot TEXT,
    expiration TIMESTAMPTZ,
    serial TEXT,
    status TEXT NOT NULL,
    quantity_delta BIGINT NOT NULL,
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, transaction_id)
        REFERENCES inventory_transactions(
            tenant_id,
            inventory_owner_id,
            id
        ),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id)
        REFERENCES item_batches(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, facility_id, location_id)
        REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (
        tenant_id,
        inventory_owner_id,
        facility_id,
        license_plate_id
    )
        REFERENCES license_plates(
            tenant_id,
            inventory_owner_id,
            facility_id,
            id
        ),
    FOREIGN KEY (tenant_id, item_id)
        REFERENCES items(tenant_id, id),
    CHECK (inventory_balance_id > 0),
    CHECK (btrim(uom) <> ''),
    CHECK (status IN ('available', 'hold', 'damaged', 'quarantine')),
    CHECK (quantity_delta <> 0)
);

CREATE INDEX inventory_projection_changes_transaction_idx
    ON inventory_projection_changes(
        tenant_id,
        inventory_owner_id,
        transaction_id,
        id
    );

CREATE INDEX inventory_projection_changes_balance_idx
    ON inventory_projection_changes(
        tenant_id,
        inventory_owner_id,
        inventory_balance_id,
        id
    );

ALTER TABLE inventory_projection_changes ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_projection_changes FORCE ROW LEVEL SECURITY;

CREATE POLICY inventory_projection_changes_tenant_isolation
    ON inventory_projection_changes
    USING (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id =
            NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

CREATE TRIGGER inventory_projection_changes_are_immutable
    BEFORE UPDATE OR DELETE
    ON inventory_projection_changes
    FOR EACH ROW
    EXECUTE FUNCTION reject_inventory_journal_mutation();

CREATE FUNCTION capture_inventory_projection_change()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
SET search_path = pg_catalog, public
AS $$
DECLARE
    context_transaction_id BIGINT;
    target_tenant_id BIGINT;
    target_inventory_owner_id BIGINT;
    old_effective_quantity BIGINT := 0;
    new_effective_quantity BIGINT := 0;
    dimensions_match BOOLEAN := FALSE;
BEGIN
    IF TG_OP IN ('UPDATE', 'DELETE') AND OLD.deleted IS NULL THEN
        old_effective_quantity := OLD.qty_on_hand;
    END IF;

    IF TG_OP IN ('INSERT', 'UPDATE') AND NEW.deleted IS NULL THEN
        new_effective_quantity := NEW.qty_on_hand;
    END IF;

    IF TG_OP = 'INSERT' AND new_effective_quantity = 0 THEN
        RETURN NEW;
    ELSIF TG_OP = 'DELETE' AND old_effective_quantity = 0 THEN
        RETURN OLD;
    ELSIF TG_OP = 'UPDATE' THEN
        dimensions_match :=
            OLD.tenant_id IS NOT DISTINCT FROM NEW.tenant_id
            AND OLD.inventory_owner_id IS NOT DISTINCT FROM
                NEW.inventory_owner_id
            AND OLD.facility_id IS NOT DISTINCT FROM NEW.facility_id
            AND OLD.location_id IS NOT DISTINCT FROM NEW.location_id
            AND OLD.license_plate_id IS NOT DISTINCT FROM
                NEW.license_plate_id
            AND OLD.item_batch_id IS NOT DISTINCT FROM NEW.item_batch_id
            AND OLD.item_id IS NOT DISTINCT FROM NEW.item_id
            AND OLD.uom IS NOT DISTINCT FROM NEW.uom
            AND OLD.status IS NOT DISTINCT FROM NEW.status;

        IF dimensions_match
           AND old_effective_quantity = new_effective_quantity
        THEN
            RETURN NEW;
        END IF;
    END IF;

    target_tenant_id := CASE
        WHEN TG_OP = 'DELETE' THEN OLD.tenant_id
        ELSE NEW.tenant_id
    END;
    target_inventory_owner_id := CASE
        WHEN TG_OP = 'DELETE' THEN OLD.inventory_owner_id
        ELSE NEW.inventory_owner_id
    END;
    context_transaction_id :=
        NULLIF(
            current_setting(
                'wareboxes.inventory_transaction_id',
                true
            ),
            ''
        )::BIGINT;

    IF context_transaction_id IS NULL OR NOT EXISTS (
        SELECT 1
        FROM public.inventory_transactions transaction
        WHERE transaction.tenant_id = target_tenant_id
          AND transaction.inventory_owner_id =
              target_inventory_owner_id
          AND transaction.id = context_transaction_id
          AND transaction.xmin::TEXT = pg_current_xact_id()::TEXT
    ) THEN
        RAISE EXCEPTION
            'on-hand inventory changes require a journal transaction created in the same database transaction'
            USING ERRCODE = '55000';
    END IF;

    IF TG_OP = 'UPDATE' AND dimensions_match THEN
        INSERT INTO public.inventory_projection_changes (
            tenant_id,
            inventory_owner_id,
            transaction_id,
            inventory_balance_id,
            facility_id,
            location_id,
            license_plate_id,
            item_batch_id,
            item_id,
            uom,
            lot,
            expiration,
            serial,
            status,
            quantity_delta
        )
        SELECT
            NEW.tenant_id,
            NEW.inventory_owner_id,
            context_transaction_id,
            NEW.id,
            NEW.facility_id,
            NEW.location_id,
            NEW.license_plate_id,
            NEW.item_batch_id,
            NEW.item_id,
            NEW.uom,
            batch.lot,
            batch.expiration,
            batch.serial,
            NEW.status,
            new_effective_quantity - old_effective_quantity
        FROM public.item_batches batch
        WHERE batch.tenant_id = NEW.tenant_id
          AND batch.inventory_owner_id = NEW.inventory_owner_id
          AND batch.id = NEW.item_batch_id;

        RETURN NEW;
    END IF;

    IF old_effective_quantity <> 0 THEN
        INSERT INTO public.inventory_projection_changes (
            tenant_id,
            inventory_owner_id,
            transaction_id,
            inventory_balance_id,
            facility_id,
            location_id,
            license_plate_id,
            item_batch_id,
            item_id,
            uom,
            lot,
            expiration,
            serial,
            status,
            quantity_delta
        )
        SELECT
            OLD.tenant_id,
            OLD.inventory_owner_id,
            context_transaction_id,
            OLD.id,
            OLD.facility_id,
            OLD.location_id,
            OLD.license_plate_id,
            OLD.item_batch_id,
            OLD.item_id,
            OLD.uom,
            batch.lot,
            batch.expiration,
            batch.serial,
            OLD.status,
            -old_effective_quantity
        FROM public.item_batches batch
        WHERE batch.tenant_id = OLD.tenant_id
          AND batch.inventory_owner_id = OLD.inventory_owner_id
          AND batch.id = OLD.item_batch_id;
    END IF;

    IF new_effective_quantity <> 0 THEN
        INSERT INTO public.inventory_projection_changes (
            tenant_id,
            inventory_owner_id,
            transaction_id,
            inventory_balance_id,
            facility_id,
            location_id,
            license_plate_id,
            item_batch_id,
            item_id,
            uom,
            lot,
            expiration,
            serial,
            status,
            quantity_delta
        )
        SELECT
            NEW.tenant_id,
            NEW.inventory_owner_id,
            context_transaction_id,
            NEW.id,
            NEW.facility_id,
            NEW.location_id,
            NEW.license_plate_id,
            NEW.item_batch_id,
            NEW.item_id,
            NEW.uom,
            batch.lot,
            batch.expiration,
            batch.serial,
            NEW.status,
            new_effective_quantity
        FROM public.item_batches batch
        WHERE batch.tenant_id = NEW.tenant_id
          AND batch.inventory_owner_id = NEW.inventory_owner_id
          AND batch.id = NEW.item_batch_id;
    END IF;

    IF TG_OP = 'DELETE' THEN
        RETURN OLD;
    END IF;
    RETURN NEW;
END;
$$;

CREATE TRIGGER inventory_balances_capture_projection_change
    AFTER INSERT OR UPDATE OR DELETE
    ON inventory_balances
    FOR EACH ROW
    EXECUTE FUNCTION capture_inventory_projection_change();

CREATE OR REPLACE FUNCTION enforce_inventory_transaction_conservation()
RETURNS trigger
LANGUAGE plpgsql
SECURITY DEFINER
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

    IF EXISTS (
        WITH projection AS (
            SELECT
                change.facility_id,
                change.location_id,
                change.license_plate_id,
                change.item_batch_id,
                change.item_id,
                change.uom,
                change.lot,
                change.expiration,
                change.serial,
                change.status,
                SUM(change.quantity_delta) AS quantity_delta
            FROM public.inventory_projection_changes change
            WHERE change.tenant_id = NEW.tenant_id
              AND change.inventory_owner_id = NEW.inventory_owner_id
              AND change.transaction_id = NEW.id
            GROUP BY
                change.facility_id,
                change.location_id,
                change.license_plate_id,
                change.item_batch_id,
                change.item_id,
                change.uom,
                change.lot,
                change.expiration,
                change.serial,
                change.status
        ),
        journal AS (
            SELECT
                entry.facility_id,
                entry.location_id,
                entry.license_plate_id,
                entry.item_batch_id,
                entry.item_id,
                entry.uom,
                entry.lot,
                entry.expiration,
                entry.serial,
                entry.status,
                SUM(entry.quantity_delta) AS quantity_delta
            FROM public.inventory_entries entry
            WHERE entry.tenant_id = NEW.tenant_id
              AND entry.inventory_owner_id = NEW.inventory_owner_id
              AND entry.transaction_id = NEW.id
            GROUP BY
                entry.facility_id,
                entry.location_id,
                entry.license_plate_id,
                entry.item_batch_id,
                entry.item_id,
                entry.uom,
                entry.lot,
                entry.expiration,
                entry.serial,
                entry.status
        )
        (
            SELECT * FROM projection
            EXCEPT
            SELECT * FROM journal
        )
        UNION ALL
        (
            SELECT * FROM journal
            EXCEPT
            SELECT * FROM projection
        )
    ) THEN
        RAISE EXCEPTION
            'inventory journal entries must exactly match on-hand projection changes'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

REVOKE ALL ON inventory_projection_changes
FROM PUBLIC, wareboxes_app;
GRANT SELECT ON inventory_projection_changes TO wareboxes_app;

REVOKE ALL ON SEQUENCE inventory_projection_changes_id_seq
FROM PUBLIC, wareboxes_app;

REVOKE ALL ON FUNCTION
    capture_inventory_projection_change(),
    enforce_inventory_transaction_conservation()
FROM PUBLIC, wareboxes_app;
