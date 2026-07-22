-- Remaining domains ported from the Drizzle schema (items, inventory,
-- locations, license plates, employees, loads, audits). Conventions match 0001:
-- TIMESTAMPTZ timestamps, native booleans, enums as TEXT.

CREATE TABLE locations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    warehouse_id BIGINT NOT NULL,
    parent_location_id BIGINT,
    barcode TEXT,
    name TEXT,
    type TEXT NOT NULL,
    active BOOLEAN NOT NULL DEFAULT true,
    pickable BOOLEAN NOT NULL DEFAULT false,
    receivable BOOLEAN NOT NULL DEFAULT false,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, barcode),
    FOREIGN KEY (tenant_id, warehouse_id) REFERENCES warehouses(tenant_id, id),
    FOREIGN KEY (tenant_id, parent_location_id) REFERENCES locations(tenant_id, id)
);
CREATE INDEX idx_locations_warehouse_id ON locations(tenant_id, warehouse_id);
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
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    description TEXT,
    notes TEXT,
    packaging_unit TEXT NOT NULL,
    dims_id BIGINT REFERENCES dims(id),
    pallet_hi BIGINT,
    pallet_ti BIGINT,
    inner_units BIGINT,
    CHECK (pallet_hi IS NULL OR pallet_hi > 0),
    CHECK (pallet_ti IS NULL OR pallet_ti > 0),
    CHECK (inner_units IS NULL OR inner_units > 0)
);

CREATE TABLE skus (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT NOT NULL,
    item_id BIGINT NOT NULL REFERENCES items(id) ON DELETE CASCADE,
    notes TEXT,
    UNIQUE (item_id, name)
);

CREATE TABLE barcodes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT NOT NULL UNIQUE,
    type TEXT NOT NULL,
    item_id BIGINT NOT NULL REFERENCES items(id),
    notes TEXT
);

CREATE TABLE account_items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    account_id BIGINT NOT NULL REFERENCES accounts(id),
    item_id BIGINT NOT NULL REFERENCES items(id)
);

CREATE TABLE loads (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    warehouse_id BIGINT NOT NULL REFERENCES warehouses(id),
    account_id BIGINT NOT NULL REFERENCES accounts(id),
    status TEXT NOT NULL DEFAULT 'planned',
    type TEXT NOT NULL,
    reference_number TEXT,
    invoice_number TEXT,
    carrier TEXT,
    trailer_number TEXT,
    seal_number TEXT,
    dock_door_location_id BIGINT REFERENCES locations(id),
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
    CHECK (status IN ('planned', 'scheduled', 'arrived', 'receiving', 'received', 'rejected', 'closed', 'cancelled')),
    CHECK (type IN ('inbound', 'outbound'))
);
CREATE INDEX idx_loads_status ON loads(status);
CREATE INDEX idx_loads_dock_door ON loads(dock_door_location_id);
CREATE INDEX idx_loads_active_work ON loads(warehouse_id, account_id, status, appointment_time)
    WHERE deleted IS NULL AND status NOT IN ('closed', 'cancelled');

CREATE TABLE load_orders (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    load_id BIGINT REFERENCES loads(id),
    order_id BIGINT NOT NULL REFERENCES orders(id)
);
CREATE INDEX idx_load_orders_load_id ON load_orders(load_id);
CREATE INDEX idx_load_orders_order_id ON load_orders(order_id);

CREATE TABLE load_notes (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    load_id BIGINT REFERENCES loads(id),
    note TEXT NOT NULL
);
CREATE INDEX idx_load_notes_load_id ON load_notes(load_id) WHERE deleted IS NULL;

CREATE TABLE load_files (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    load_id BIGINT REFERENCES loads(id),
    original_name TEXT NOT NULL,
    name TEXT NOT NULL,
    path TEXT NOT NULL,
    content_type TEXT,
    category TEXT NOT NULL DEFAULT 'general',
    CHECK (category IN ('general', 'invoice'))
);
CREATE INDEX idx_load_files_load_id ON load_files(load_id) WHERE deleted IS NULL;

CREATE TABLE load_lines (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    load_id BIGINT NOT NULL REFERENCES loads(id),
    item_id BIGINT NOT NULL REFERENCES items(id),
    sku_id BIGINT REFERENCES skus(id),
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
    CHECK (status IN ('pending', 'partial', 'received', 'rejected', 'missing')),
    CHECK (received_qty + rejected_qty + missing_qty <= expected_qty),
    CHECK (
        (missing_qty = 0 AND missing_confirmed_by IS NULL AND missing_confirmed_at IS NULL)
        OR (missing_qty > 0 AND missing_confirmed_by IS NOT NULL AND missing_confirmed_at IS NOT NULL)
    )
);
CREATE INDEX idx_load_lines_load ON load_lines(load_id);
CREATE INDEX idx_load_lines_item ON load_lines(item_id);
CREATE INDEX idx_load_lines_open ON load_lines(load_id, status)
    WHERE deleted IS NULL AND status IN ('pending', 'partial');

CREATE TABLE load_activity (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    load_id BIGINT REFERENCES loads(id),
    user_id BIGINT REFERENCES users(id),
    action TEXT NOT NULL,
    message TEXT,
    metadata_json TEXT
);
CREATE INDEX idx_load_activity_load_id ON load_activity(load_id);

CREATE TABLE item_batches (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    item_id BIGINT NOT NULL REFERENCES items(id),
    lot TEXT,
    load_id BIGINT REFERENCES loads(id),
    order_id BIGINT REFERENCES orders(id),
    expiration TIMESTAMPTZ,
    serial TEXT
);
ALTER TABLE order_items
    ADD CONSTRAINT order_items_item_id_fkey FOREIGN KEY (item_id) REFERENCES items(id),
    ADD CONSTRAINT order_items_item_batch_id_fkey FOREIGN KEY (item_batch_id) REFERENCES item_batches(id);
CREATE INDEX idx_item_batches_item_id ON item_batches(item_id) WHERE deleted IS NULL;
CREATE INDEX idx_item_batches_load_id ON item_batches(load_id) WHERE deleted IS NULL;

CREATE TABLE license_plates (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    barcode TEXT UNIQUE,
    location_id BIGINT REFERENCES locations(id),
    dims_id BIGINT REFERENCES dims(id)
);
CREATE INDEX idx_license_plates_location_id ON license_plates(location_id) WHERE deleted IS NULL;

CREATE TABLE inventory_balances (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    modified TIMESTAMPTZ,
    deleted TIMESTAMPTZ,
    warehouse_id BIGINT NOT NULL REFERENCES warehouses(id),
    location_id BIGINT NOT NULL REFERENCES locations(id),
    license_plate_id BIGINT REFERENCES license_plates(id),
    item_batch_id BIGINT NOT NULL REFERENCES item_batches(id),
    status TEXT NOT NULL DEFAULT 'available',
    qty_on_hand BIGINT NOT NULL DEFAULT 0,
    qty_reserved BIGINT NOT NULL DEFAULT 0,
    CHECK (qty_on_hand >= 0),
    CHECK (qty_reserved >= 0),
    CHECK (status IN ('available', 'hold', 'damaged', 'quarantine')),
    CHECK (qty_reserved <= qty_on_hand)
);
CREATE INDEX idx_inventory_balances_batch ON inventory_balances(item_batch_id);
CREATE INDEX idx_inventory_balances_location ON inventory_balances(location_id);
CREATE INDEX idx_inventory_balances_lookup ON inventory_balances(location_id, status, item_batch_id)
    WHERE deleted IS NULL;
CREATE UNIQUE INDEX idx_inventory_balances_loose_unique
    ON inventory_balances(location_id, item_batch_id, status)
    WHERE license_plate_id IS NULL;
CREATE UNIQUE INDEX idx_inventory_balances_lp_unique
    ON inventory_balances(location_id, license_plate_id, item_batch_id, status)
    WHERE license_plate_id IS NOT NULL;
CREATE INDEX idx_inventory_balances_license_plate
    ON inventory_balances(license_plate_id)
    WHERE deleted IS NULL AND license_plate_id IS NOT NULL;

CREATE TABLE stock_movements (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    user_id BIGINT REFERENCES users(id),
    item_batch_id BIGINT NOT NULL REFERENCES item_batches(id),
    license_plate_id BIGINT REFERENCES license_plates(id),
    from_location_id BIGINT REFERENCES locations(id),
    to_location_id BIGINT REFERENCES locations(id),
    qty BIGINT NOT NULL CHECK (qty > 0),
    movement_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'available',
    reason TEXT,
    reference_type TEXT,
    reference_id BIGINT,
    idempotency_key TEXT UNIQUE,
    CHECK (movement_type IN ('receive', 'move', 'reserve', 'adjust')),
    CHECK (status IN ('available', 'hold', 'damaged', 'quarantine'))
);
CREATE INDEX idx_stock_movements_batch ON stock_movements(item_batch_id);
CREATE INDEX idx_stock_movements_locations ON stock_movements(from_location_id, to_location_id);
CREATE INDEX idx_stock_movements_reference ON stock_movements(reference_type, reference_id)
    WHERE reference_type IS NOT NULL AND reference_id IS NOT NULL;

CREATE TABLE inventory_reservations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    modified TIMESTAMPTZ,
    deleted TIMESTAMPTZ,
    order_id BIGINT NOT NULL REFERENCES orders(id),
    order_item_id BIGINT REFERENCES order_items(id),
    item_batch_id BIGINT NOT NULL REFERENCES item_batches(id),
    location_id BIGINT NOT NULL REFERENCES locations(id),
    qty BIGINT NOT NULL CHECK (qty > 0),
    status TEXT NOT NULL DEFAULT 'reserved',
    CHECK (status IN ('reserved', 'cancelled', 'fulfilled'))
);
CREATE INDEX idx_inventory_reservations_order ON inventory_reservations(order_id);
CREATE INDEX idx_inventory_reservations_batch_location ON inventory_reservations(item_batch_id, location_id);
CREATE INDEX idx_inventory_reservations_open ON inventory_reservations(order_id, status)
    WHERE deleted IS NULL AND status = 'reserved';

CREATE TABLE picks (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    location_id BIGINT REFERENCES locations(id),
    item_batch_id BIGINT REFERENCES item_batches(id),
    reservation_id BIGINT REFERENCES inventory_reservations(id),
    qty BIGINT NOT NULL DEFAULT 1 CHECK (qty > 0)
);

CREATE TABLE employees (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    user_id BIGINT REFERENCES users(id),
    first_name TEXT NOT NULL,
    last_name TEXT NOT NULL,
    email TEXT,
    phone TEXT,
    title TEXT NOT NULL,
    type TEXT NOT NULL,
    hired TIMESTAMPTZ NOT NULL,
    terminated TIMESTAMPTZ,
    CHECK (terminated IS NULL OR terminated >= hired)
);

CREATE TABLE employee_warehouses (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    employee_id BIGINT NOT NULL REFERENCES employees(id),
    warehouse_id BIGINT NOT NULL REFERENCES warehouses(id),
    UNIQUE (employee_id, warehouse_id)
);

CREATE TABLE audit_waves (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT,
    description TEXT
);

CREATE TABLE audit_wave_items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    item_id BIGINT NOT NULL REFERENCES items(id),
    audit_wave_id BIGINT NOT NULL REFERENCES audit_waves(id)
);

CREATE TABLE audit_wave_accounts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    account_id BIGINT NOT NULL REFERENCES accounts(id),
    audit_wave_id BIGINT NOT NULL REFERENCES audit_waves(id)
);

CREATE TABLE audit_wave_locations (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    started TIMESTAMPTZ,
    ended TIMESTAMPTZ,
    location_id BIGINT NOT NULL REFERENCES locations(id),
    audit_wave_id BIGINT NOT NULL REFERENCES audit_waves(id),
    auditor_id BIGINT REFERENCES users(id)
);

CREATE TABLE audit_location_counts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    started TIMESTAMPTZ,
    ended TIMESTAMPTZ,
    audit_id BIGINT NOT NULL REFERENCES audit_waves(id),
    location_id BIGINT NOT NULL REFERENCES locations(id),
    item_id BIGINT NOT NULL REFERENCES items(id),
    lot TEXT,
    expiration TIMESTAMPTZ,
    serial TEXT,
    on_hand BIGINT NOT NULL,
    count BIGINT NOT NULL,
    approval_status TEXT NOT NULL,
    CHECK (on_hand >= 0),
    CHECK (count >= 0),
    CHECK (approval_status IN ('pending', 'approved', 'rejected'))
);

CREATE TABLE audit_wave_assignments (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    audit_wave_id BIGINT NOT NULL REFERENCES audit_waves(id),
    auditor_id BIGINT NOT NULL REFERENCES users(id)
);
