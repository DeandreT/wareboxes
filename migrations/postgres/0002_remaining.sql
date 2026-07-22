-- Remaining domains ported from the Drizzle schema (items, inventory,
-- locations, license plates, employees, loads, audits). Conventions match 0001:
-- TIMESTAMPTZ timestamps, native booleans, enums as TEXT.

CREATE TABLE locations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    facility_id BIGINT NOT NULL,
    parent_location_id BIGINT,
    barcode TEXT,
    name TEXT,
    type TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    pickable BOOLEAN NOT NULL DEFAULT false,
    receivable BOOLEAN NOT NULL DEFAULT false,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, facility_id, id),
    UNIQUE (tenant_id, barcode),
    FOREIGN KEY (tenant_id, facility_id) REFERENCES facilities(tenant_id, id),
    FOREIGN KEY (tenant_id, parent_location_id) REFERENCES locations(tenant_id, id)
);
CREATE INDEX idx_locations_facility_id ON locations(tenant_id, facility_id);
CREATE INDEX idx_locations_parent_location_id ON locations(tenant_id, parent_location_id);

CREATE TABLE dims (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    length BIGINT,
    width BIGINT,
    height BIGINT,
    length_uom TEXT,
    weight BIGINT,
    weight_uom TEXT,
    CHECK (length IS NULL OR length > 0),
    CHECK (width IS NULL OR width > 0),
    CHECK (height IS NULL OR height > 0),
    CHECK (weight IS NULL OR weight > 0)
);

CREATE TABLE items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    description TEXT,
    notes TEXT,
    packaging_unit TEXT NOT NULL,
    dims_id BIGINT REFERENCES dims(id),
    pallet_hi BIGINT,
    pallet_ti BIGINT,
    inner_units BIGINT,
    UNIQUE (tenant_id, id),
    CHECK (pallet_hi IS NULL OR pallet_hi > 0),
    CHECK (pallet_ti IS NULL OR pallet_ti > 0),
    CHECK (inner_units IS NULL OR inner_units > 0)
);

CREATE TABLE skus (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT NOT NULL,
    item_id BIGINT NOT NULL,
    notes TEXT,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, item_id, id),
    UNIQUE (tenant_id, item_id, name),
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE barcodes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT NOT NULL,
    type TEXT NOT NULL,
    item_id BIGINT NOT NULL,
    notes TEXT,
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id)
);

CREATE TABLE inventory_owner_items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    inventory_owner_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, item_id)
);

CREATE TABLE loads (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    facility_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    status TEXT NOT NULL DEFAULT 'planned',
    type TEXT NOT NULL,
    reference_number TEXT,
    invoice_number TEXT,
    carrier TEXT,
    trailer_number TEXT,
    seal_number TEXT,
    dock_door_location_id BIGINT,
    expected_time TIMESTAMPTZ,
    appointment_time TIMESTAMPTZ,
    actual_time TIMESTAMPTZ,
    arrival TIMESTAMPTZ,
    departure TIMESTAMPTZ,
    rejected TIMESTAMPTZ,
    receive_completed BOOLEAN NOT NULL DEFAULT false,
    closed TIMESTAMPTZ,
    checked_in_by BIGINT REFERENCES users(id),
    closed_by BIGINT REFERENCES users(id),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, facility_id) REFERENCES facilities(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id, dock_door_location_id) REFERENCES locations(tenant_id, facility_id, id),
    CHECK (status IN ('planned', 'scheduled', 'arrived', 'receiving', 'received', 'rejected', 'closed', 'cancelled')),
    CHECK (type IN ('inbound', 'outbound'))
);
CREATE INDEX idx_loads_status ON loads(tenant_id, status);
CREATE INDEX idx_loads_dock_door ON loads(tenant_id, dock_door_location_id);
CREATE INDEX idx_loads_active_work ON loads(tenant_id, facility_id, inventory_owner_id, status, appointment_time)
    WHERE deleted IS NULL AND status NOT IN ('closed', 'cancelled');

CREATE TABLE load_orders (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    load_id BIGINT NOT NULL,
    order_id BIGINT NOT NULL,
    FOREIGN KEY (tenant_id, inventory_owner_id, load_id) REFERENCES loads(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, order_id) REFERENCES orders(tenant_id, inventory_owner_id, id)
);
CREATE INDEX idx_load_orders_load_id ON load_orders(tenant_id, inventory_owner_id, load_id);
CREATE INDEX idx_load_orders_order_id ON load_orders(tenant_id, inventory_owner_id, order_id);

CREATE TABLE load_notes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    load_id BIGINT NOT NULL,
    note TEXT NOT NULL,
    FOREIGN KEY (tenant_id, load_id) REFERENCES loads(tenant_id, id)
);
CREATE INDEX idx_load_notes_load_id ON load_notes(tenant_id, load_id) WHERE deleted IS NULL;

CREATE TABLE load_files (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    load_id BIGINT NOT NULL,
    original_name TEXT NOT NULL,
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    content_type TEXT,
    category TEXT NOT NULL DEFAULT 'general',
    FOREIGN KEY (tenant_id, load_id) REFERENCES loads(tenant_id, id),
    CHECK (category IN ('general', 'invoice'))
);
CREATE INDEX idx_load_files_load_id ON load_files(tenant_id, load_id) WHERE deleted IS NULL;

CREATE TABLE load_lines (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    load_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    sku_id BIGINT,
    expected_qty BIGINT NOT NULL CHECK (expected_qty > 0),
    received_qty BIGINT NOT NULL DEFAULT 0 CHECK (received_qty >= 0),
    rejected_qty BIGINT NOT NULL DEFAULT 0 CHECK (rejected_qty >= 0),
    missing_qty BIGINT NOT NULL DEFAULT 0 CHECK (missing_qty >= 0),
    missing_confirmed_by BIGINT REFERENCES users(id),
    missing_confirmed_at TIMESTAMPTZ,
    lot TEXT,
    serial TEXT,
    expiration TIMESTAMPTZ,
    status TEXT NOT NULL DEFAULT 'pending',
    FOREIGN KEY (tenant_id, load_id) REFERENCES loads(tenant_id, id),
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id),
    FOREIGN KEY (tenant_id, item_id, sku_id) REFERENCES skus(tenant_id, item_id, id),
    CHECK (status IN ('pending', 'partial', 'received', 'rejected', 'missing')),
    CHECK (received_qty + rejected_qty + missing_qty <= expected_qty),
    CHECK (
        (missing_qty = 0 AND missing_confirmed_by IS NULL AND missing_confirmed_at IS NULL)
        OR (missing_qty > 0 AND missing_confirmed_by IS NOT NULL AND missing_confirmed_at IS NOT NULL)
    )
);
CREATE INDEX idx_load_lines_load ON load_lines(tenant_id, load_id);
CREATE INDEX idx_load_lines_item ON load_lines(tenant_id, item_id);
CREATE INDEX idx_load_lines_open ON load_lines(tenant_id, load_id, status)
    WHERE deleted IS NULL AND status IN ('pending', 'partial');

CREATE TABLE load_activity (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    load_id BIGINT NOT NULL,
    user_id BIGINT REFERENCES users(id),
    action TEXT NOT NULL,
    message TEXT,
    metadata_json TEXT,
    FOREIGN KEY (tenant_id, load_id) REFERENCES loads(tenant_id, id)
);
CREATE INDEX idx_load_activity_load_id ON load_activity(tenant_id, load_id);

CREATE TABLE item_batches (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    item_id BIGINT NOT NULL,
    uom TEXT NOT NULL,
    lot TEXT,
    load_id BIGINT,
    order_id BIGINT,
    expiration TIMESTAMPTZ,
    serial TEXT,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_id) REFERENCES inventory_owner_items(tenant_id, inventory_owner_id, item_id),
    FOREIGN KEY (tenant_id, inventory_owner_id, load_id) REFERENCES loads(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, order_id) REFERENCES orders(tenant_id, inventory_owner_id, id),
    CHECK (btrim(uom) <> '')
);
ALTER TABLE order_items
    ADD CONSTRAINT order_items_item_id_fkey FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id),
    ADD CONSTRAINT order_items_item_batch_id_fkey FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id) REFERENCES item_batches(tenant_id, inventory_owner_id, id);
CREATE INDEX idx_item_batches_item_id ON item_batches(item_id) WHERE deleted IS NULL;
CREATE INDEX idx_item_batches_load_id ON item_batches(load_id) WHERE deleted IS NULL;

CREATE TABLE license_plates (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    barcode TEXT,
    facility_id BIGINT NOT NULL,
    location_id BIGINT,
    dims_id BIGINT REFERENCES dims(id),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    UNIQUE (tenant_id, inventory_owner_id, facility_id, id),
    UNIQUE (tenant_id, barcode),
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id) REFERENCES facilities(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id, location_id) REFERENCES locations(tenant_id, facility_id, id)
);
CREATE INDEX idx_license_plates_facility_location
    ON license_plates(tenant_id, facility_id, location_id)
    WHERE deleted IS NULL;

CREATE TABLE inventory_balances (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    modified TIMESTAMPTZ,
    deleted TIMESTAMPTZ,
    facility_id BIGINT NOT NULL,
    location_id BIGINT NOT NULL,
    license_plate_id BIGINT,
    item_batch_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    uom TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'available',
    qty_on_hand BIGINT NOT NULL DEFAULT 0,
    qty_reserved BIGINT NOT NULL DEFAULT 0,
    CHECK (qty_on_hand >= 0),
    CHECK (qty_reserved >= 0),
    CHECK (status IN ('available', 'hold', 'damaged', 'quarantine')),
    CHECK (qty_reserved <= qty_on_hand),
    CHECK (btrim(uom) <> ''),
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id, location_id) REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, license_plate_id) REFERENCES license_plates(tenant_id, inventory_owner_id, facility_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id) REFERENCES item_batches(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id)
);
CREATE INDEX idx_inventory_balances_batch ON inventory_balances(tenant_id, inventory_owner_id, item_batch_id);
CREATE INDEX idx_inventory_balances_location ON inventory_balances(tenant_id, facility_id, location_id);
CREATE INDEX idx_inventory_balances_lookup ON inventory_balances(tenant_id, inventory_owner_id, location_id, status, item_batch_id)
    WHERE deleted IS NULL;
CREATE UNIQUE INDEX idx_inventory_balances_loose_unique
    ON inventory_balances(tenant_id, inventory_owner_id, location_id, item_batch_id, uom, status)
    WHERE license_plate_id IS NULL;
CREATE UNIQUE INDEX idx_inventory_balances_lp_unique
    ON inventory_balances(tenant_id, inventory_owner_id, location_id, license_plate_id, item_batch_id, uom, status)
    WHERE license_plate_id IS NOT NULL;
CREATE INDEX idx_inventory_balances_license_plate
    ON inventory_balances(tenant_id, license_plate_id)
    WHERE deleted IS NULL AND license_plate_id IS NOT NULL;

CREATE TABLE inventory_transactions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    actor_user_id BIGINT REFERENCES users(id),
    transaction_type TEXT NOT NULL,
    reason TEXT,
    reference_type TEXT,
    reference_id BIGINT,
    correlation_id TEXT,
    operation TEXT NOT NULL,
    idempotency_key TEXT,
    request_hash TEXT NOT NULL,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    CHECK (transaction_type IN ('receive', 'move', 'adjust', 'ship')),
    CHECK (btrim(operation) <> ''),
    CHECK (btrim(request_hash) <> '')
);
CREATE INDEX idx_inventory_transactions_reference
    ON inventory_transactions(tenant_id, inventory_owner_id, reference_type, reference_id)
    WHERE reference_type IS NOT NULL AND reference_id IS NOT NULL;

CREATE TABLE command_idempotency_records (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    operation TEXT NOT NULL,
    idempotency_key TEXT NOT NULL,
    request_hash TEXT NOT NULL,
    result_json JSONB NOT NULL,
    inventory_transaction_id BIGINT,
    UNIQUE (tenant_id, operation, idempotency_key),
    FOREIGN KEY (tenant_id, inventory_transaction_id) REFERENCES inventory_transactions(tenant_id, id),
    CHECK (btrim(operation) <> ''),
    CHECK (btrim(idempotency_key) <> ''),
    CHECK (btrim(request_hash) <> '')
);

CREATE TABLE inventory_entries (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    transaction_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
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
    FOREIGN KEY (tenant_id, inventory_owner_id, transaction_id) REFERENCES inventory_transactions(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id) REFERENCES item_batches(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, facility_id, location_id) REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, facility_id, license_plate_id) REFERENCES license_plates(tenant_id, inventory_owner_id, facility_id, id),
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id),
    CHECK (quantity_delta <> 0),
    CHECK (status IN ('available', 'hold', 'damaged', 'quarantine')),
    CHECK (btrim(uom) <> '')
);
CREATE INDEX idx_inventory_entries_transaction
    ON inventory_entries(tenant_id, transaction_id, id);
CREATE INDEX idx_inventory_entries_dimensions
    ON inventory_entries(tenant_id, inventory_owner_id, facility_id, location_id, item_id, uom, status);

CREATE OR REPLACE FUNCTION reject_inventory_journal_mutation()
RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'inventory journal rows are immutable'
        USING ERRCODE = '55000';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER inventory_transactions_are_immutable
    BEFORE UPDATE OR DELETE ON inventory_transactions
    FOR EACH ROW EXECUTE FUNCTION reject_inventory_journal_mutation();

CREATE TRIGGER inventory_entries_are_immutable
    BEFORE UPDATE OR DELETE ON inventory_entries
    FOR EACH ROW EXECUTE FUNCTION reject_inventory_journal_mutation();

CREATE TRIGGER command_idempotency_records_are_immutable
    BEFORE UPDATE OR DELETE ON command_idempotency_records
    FOR EACH ROW EXECUTE FUNCTION reject_inventory_journal_mutation();

CREATE OR REPLACE FUNCTION require_open_inventory_transaction()
RETURNS trigger AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM inventory_transactions
        WHERE tenant_id = NEW.tenant_id
          AND inventory_owner_id = NEW.inventory_owner_id
          AND id = NEW.transaction_id
          AND xmin::TEXT = pg_current_xact_id()::TEXT
    ) THEN
        RAISE EXCEPTION 'inventory entries may only be appended while creating their transaction'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER inventory_entries_require_open_transaction
    BEFORE INSERT ON inventory_entries
    FOR EACH ROW EXECUTE FUNCTION require_open_inventory_transaction();

CREATE OR REPLACE FUNCTION enforce_inventory_transaction_conservation()
RETURNS trigger AS $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM inventory_entries WHERE transaction_id = NEW.id
    ) THEN
        RAISE EXCEPTION 'inventory transaction must contain at least one entry'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.transaction_type = 'move' AND EXISTS (
        SELECT 1
        FROM inventory_entries
        WHERE transaction_id = NEW.id
        GROUP BY inventory_owner_id, item_id, uom, lot, expiration, serial, status
        HAVING SUM(quantity_delta) <> 0
    ) THEN
        RAISE EXCEPTION 'inventory move entries must conserve quantity for every stock dimension'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.transaction_type = 'receive' AND EXISTS (
        SELECT 1 FROM inventory_entries
        WHERE transaction_id = NEW.id AND quantity_delta <= 0
    ) THEN
        RAISE EXCEPTION 'inventory receipt entries must be positive'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.transaction_type = 'ship' AND EXISTS (
        SELECT 1 FROM inventory_entries
        WHERE transaction_id = NEW.id AND quantity_delta >= 0
    ) THEN
        RAISE EXCEPTION 'inventory shipment entries must be negative'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE CONSTRAINT TRIGGER inventory_transactions_conserve_quantity
    AFTER INSERT ON inventory_transactions
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW EXECUTE FUNCTION enforce_inventory_transaction_conservation();

CREATE OR REPLACE FUNCTION inventory_row_matches_batch()
RETURNS trigger AS $$
DECLARE
    batch_item_id BIGINT;
    batch_uom TEXT;
BEGIN
    SELECT item_id, uom
    INTO batch_item_id, batch_uom
    FROM item_batches
    WHERE tenant_id = NEW.tenant_id
      AND inventory_owner_id = NEW.inventory_owner_id
      AND id = NEW.item_batch_id;

    IF batch_item_id IS NULL THEN
        RAISE EXCEPTION 'inventory item batch does not exist in this tenant and owner scope'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.item_id IS DISTINCT FROM batch_item_id
       OR NEW.uom IS DISTINCT FROM batch_uom THEN
        RAISE EXCEPTION 'inventory dimensions must match the item batch'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE OR REPLACE FUNCTION inventory_entry_matches_batch()
RETURNS trigger AS $$
DECLARE
    batch_item_id BIGINT;
    batch_uom TEXT;
    batch_lot TEXT;
    batch_expiration TIMESTAMPTZ;
    batch_serial TEXT;
BEGIN
    SELECT item_id, uom, lot, expiration, serial
    INTO batch_item_id, batch_uom, batch_lot, batch_expiration, batch_serial
    FROM item_batches
    WHERE tenant_id = NEW.tenant_id
      AND inventory_owner_id = NEW.inventory_owner_id
      AND id = NEW.item_batch_id;

    IF batch_item_id IS NULL THEN
        RAISE EXCEPTION 'inventory item batch does not exist in this tenant and owner scope'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.item_id IS DISTINCT FROM batch_item_id
       OR NEW.uom IS DISTINCT FROM batch_uom
       OR NEW.lot IS DISTINCT FROM batch_lot
       OR NEW.expiration IS DISTINCT FROM batch_expiration
       OR NEW.serial IS DISTINCT FROM batch_serial THEN
        RAISE EXCEPTION 'inventory dimensions must match the item batch'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER inventory_balances_match_batch
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, item_batch_id, item_id, uom
    ON inventory_balances
    FOR EACH ROW EXECUTE FUNCTION inventory_row_matches_batch();

CREATE TRIGGER inventory_entries_match_batch
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, item_batch_id, item_id, uom, lot, expiration, serial
    ON inventory_entries
    FOR EACH ROW EXECUTE FUNCTION inventory_entry_matches_batch();

CREATE VIEW inventory_reconciliation AS
WITH journal AS (
    SELECT tenant_id, inventory_owner_id, facility_id, location_id,
           license_plate_id, item_batch_id, item_id, uom, status,
           SUM(quantity_delta)::BIGINT AS journal_qty
    FROM inventory_entries
    GROUP BY tenant_id, inventory_owner_id, facility_id, location_id,
             license_plate_id, item_batch_id, item_id, uom, status
), projection AS (
    SELECT tenant_id, inventory_owner_id, facility_id, location_id,
           license_plate_id, item_batch_id, item_id, uom, status,
           SUM(qty_on_hand)::BIGINT AS projected_qty
    FROM inventory_balances
    WHERE deleted IS NULL
    GROUP BY tenant_id, inventory_owner_id, facility_id, location_id,
             license_plate_id, item_batch_id, item_id, uom, status
)
SELECT COALESCE(journal.tenant_id, projection.tenant_id) AS tenant_id,
       COALESCE(journal.inventory_owner_id, projection.inventory_owner_id) AS inventory_owner_id,
       COALESCE(journal.facility_id, projection.facility_id) AS facility_id,
       COALESCE(journal.location_id, projection.location_id) AS location_id,
       COALESCE(journal.license_plate_id, projection.license_plate_id) AS license_plate_id,
       COALESCE(journal.item_batch_id, projection.item_batch_id) AS item_batch_id,
       COALESCE(journal.item_id, projection.item_id) AS item_id,
       COALESCE(journal.uom, projection.uom) AS uom,
       COALESCE(journal.status, projection.status) AS status,
       COALESCE(journal.journal_qty, 0)::BIGINT AS journal_qty,
       COALESCE(projection.projected_qty, 0)::BIGINT AS projected_qty,
       (COALESCE(projection.projected_qty, 0) - COALESCE(journal.journal_qty, 0))::BIGINT AS variance
FROM journal
FULL OUTER JOIN projection
    ON projection.tenant_id = journal.tenant_id
   AND projection.inventory_owner_id = journal.inventory_owner_id
   AND projection.facility_id = journal.facility_id
   AND projection.location_id = journal.location_id
   AND projection.license_plate_id IS NOT DISTINCT FROM journal.license_plate_id
   AND projection.item_batch_id = journal.item_batch_id
   AND projection.item_id = journal.item_id
   AND projection.uom = journal.uom
   AND projection.status = journal.status
WHERE COALESCE(projection.projected_qty, 0) <> COALESCE(journal.journal_qty, 0);

CREATE TABLE inventory_reservations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    modified TIMESTAMPTZ,
    deleted TIMESTAMPTZ,
    order_id BIGINT NOT NULL,
    order_item_id BIGINT,
    inventory_balance_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    item_batch_id BIGINT NOT NULL,
    location_id BIGINT NOT NULL,
    qty BIGINT NOT NULL CHECK (qty > 0),
    status TEXT NOT NULL DEFAULT 'reserved',
    CHECK (status IN ('reserved', 'cancelled', 'fulfilled')),
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, order_id) REFERENCES orders(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, order_item_id) REFERENCES order_items(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, inventory_balance_id) REFERENCES inventory_balances(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_batch_id) REFERENCES item_batches(tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, facility_id, location_id) REFERENCES locations(tenant_id, facility_id, id)
);
CREATE INDEX idx_inventory_reservations_order ON inventory_reservations(tenant_id, inventory_owner_id, order_id);
CREATE INDEX idx_inventory_reservations_batch_location ON inventory_reservations(tenant_id, inventory_owner_id, item_batch_id, location_id);
CREATE INDEX idx_inventory_reservations_open ON inventory_reservations(tenant_id, inventory_owner_id, order_id, status)
    WHERE deleted IS NULL AND status = 'reserved';

CREATE TABLE employees (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    user_id BIGINT,
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    email TEXT,
    phone TEXT,
    title TEXT NOT NULL,
    type TEXT NOT NULL,
    hired TIMESTAMPTZ NOT NULL,
    terminated TIMESTAMPTZ,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, email),
    FOREIGN KEY (tenant_id, user_id) REFERENCES tenant_memberships(tenant_id, user_id),
    CHECK (btrim(first_name) <> ''),
    CHECK (btrim(last_name) <> ''),
    CHECK (btrim(title) <> ''),
    CHECK (btrim(type) <> ''),
    CHECK (terminated IS NULL OR terminated >= hired)
);
CREATE INDEX idx_employees_user ON employees(tenant_id, user_id) WHERE user_id IS NOT NULL;

CREATE TABLE employee_facilities (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    employee_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    FOREIGN KEY (tenant_id, employee_id) REFERENCES employees(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id) REFERENCES facilities(tenant_id, id),
    UNIQUE (tenant_id, employee_id, facility_id)
);
CREATE INDEX idx_employee_facilities_facility ON employee_facilities(tenant_id, facility_id);

CREATE TABLE audit_waves (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT,
    description TEXT,
    created_by BIGINT NOT NULL,
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, created_by) REFERENCES tenant_memberships(tenant_id, user_id)
);

CREATE TABLE audit_wave_items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    item_id BIGINT NOT NULL,
    audit_wave_id BIGINT NOT NULL,
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id),
    FOREIGN KEY (tenant_id, audit_wave_id) REFERENCES audit_waves(tenant_id, id),
    UNIQUE (tenant_id, audit_wave_id, item_id)
);

CREATE TABLE audit_wave_inventory_owners (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    inventory_owner_id BIGINT NOT NULL,
    audit_wave_id BIGINT NOT NULL,
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, audit_wave_id) REFERENCES audit_waves(tenant_id, id),
    UNIQUE (tenant_id, audit_wave_id, inventory_owner_id)
);

CREATE TABLE audit_wave_locations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    started TIMESTAMPTZ,
    ended TIMESTAMPTZ,
    location_id BIGINT NOT NULL,
    audit_wave_id BIGINT NOT NULL,
    auditor_id BIGINT,
    FOREIGN KEY (tenant_id, location_id) REFERENCES locations(tenant_id, id),
    FOREIGN KEY (tenant_id, audit_wave_id) REFERENCES audit_waves(tenant_id, id),
    FOREIGN KEY (tenant_id, auditor_id) REFERENCES tenant_memberships(tenant_id, user_id),
    UNIQUE (tenant_id, audit_wave_id, location_id),
    CHECK (ended IS NULL OR started IS NULL OR ended >= started)
);

CREATE TABLE audit_location_counts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    started TIMESTAMPTZ,
    ended TIMESTAMPTZ,
    audit_id BIGINT NOT NULL,
    inventory_owner_id BIGINT NOT NULL,
    location_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    lot TEXT,
    expiration TIMESTAMPTZ,
    serial TEXT,
    on_hand BIGINT NOT NULL,
    count BIGINT NOT NULL,
    approval_status TEXT NOT NULL,
    FOREIGN KEY (tenant_id, audit_id) REFERENCES audit_waves(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, location_id) REFERENCES locations(tenant_id, id),
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, item_id) REFERENCES inventory_owner_items(tenant_id, inventory_owner_id, item_id),
    CHECK (ended IS NULL OR started IS NULL OR ended >= started),
    CHECK (on_hand >= 0),
    CHECK (count >= 0),
    CHECK (approval_status IN ('pending', 'approved', 'rejected'))
);

CREATE TABLE audit_wave_assignments (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    audit_wave_id BIGINT NOT NULL,
    auditor_id BIGINT NOT NULL,
    FOREIGN KEY (tenant_id, audit_wave_id) REFERENCES audit_waves(tenant_id, id),
    FOREIGN KEY (tenant_id, auditor_id) REFERENCES tenant_memberships(tenant_id, user_id),
    UNIQUE (tenant_id, audit_wave_id, auditor_id)
);
