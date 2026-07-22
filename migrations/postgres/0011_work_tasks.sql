CREATE TABLE work_tasks (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    modified TIMESTAMPTZ,
    deleted TIMESTAMPTZ,
    task_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    required_permission TEXT NOT NULL DEFAULT 'wms',
    priority BIGINT NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    instructions TEXT,
    assigned_user_id BIGINT REFERENCES users(id),
    created_by BIGINT REFERENCES users(id),
    completed_by BIGINT REFERENCES users(id),
    scheduled_for TIMESTAMPTZ,
    due_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    lease_expires_at TIMESTAMPTZ,
    task_timeout_seconds BIGINT NOT NULL DEFAULT 1800,
    last_released_at TIMESTAMPTZ,
    release_count BIGINT NOT NULL DEFAULT 0,
    completed_at TIMESTAMPTZ,
    metadata_json TEXT,
    CHECK (task_type IN (
        'cycle_count_item_location',
        'cycle_count_location',
        'break_master_pack',
        'unpack_cancelled_order'
    )),
    CHECK (status IN ('open', 'assigned', 'in_progress', 'completed', 'cancelled')),
    CHECK (priority >= 0),
    CHECK (task_timeout_seconds > 0),
    CHECK (release_count >= 0),
    CHECK (lease_expires_at IS NULL OR status IN ('assigned', 'in_progress')),
    CHECK (completed_at IS NULL OR status IN ('completed', 'cancelled')),
    CHECK (started_at IS NULL OR status IN ('in_progress', 'completed', 'cancelled'))
);

CREATE INDEX idx_work_tasks_status_schedule
    ON work_tasks(status, required_permission, scheduled_for, priority DESC, created)
    WHERE deleted IS NULL;
CREATE INDEX idx_work_tasks_expiring_leases
    ON work_tasks(lease_expires_at)
    WHERE deleted IS NULL AND status IN ('assigned', 'in_progress') AND lease_expires_at IS NOT NULL;
CREATE INDEX idx_work_tasks_assigned
    ON work_tasks(assigned_user_id, status, scheduled_for)
    WHERE deleted IS NULL AND assigned_user_id IS NOT NULL;

CREATE TABLE cycle_count_item_location_tasks (
    task_id BIGINT PRIMARY KEY REFERENCES work_tasks(id) ON DELETE CASCADE,
    facility_id BIGINT NOT NULL REFERENCES facilities(id),
    location_id BIGINT NOT NULL REFERENCES locations(id),
    item_id BIGINT NOT NULL REFERENCES items(id),
    inventory_balance_id BIGINT REFERENCES inventory_balances(id),
    order_id BIGINT REFERENCES orders(id),
    order_item_id BIGINT REFERENCES order_items(id),
    source TEXT,
    note TEXT
);
CREATE INDEX idx_cycle_count_item_location_tasks_location
    ON cycle_count_item_location_tasks(location_id, item_id);

CREATE TABLE cycle_count_location_tasks (
    task_id BIGINT PRIMARY KEY REFERENCES work_tasks(id) ON DELETE CASCADE,
    facility_id BIGINT NOT NULL REFERENCES facilities(id),
    location_id BIGINT NOT NULL REFERENCES locations(id)
);
CREATE INDEX idx_cycle_count_location_tasks_location
    ON cycle_count_location_tasks(location_id);

CREATE TABLE break_master_pack_tasks (
    task_id BIGINT PRIMARY KEY REFERENCES work_tasks(id) ON DELETE CASCADE,
    facility_id BIGINT NOT NULL REFERENCES facilities(id),
    location_id BIGINT NOT NULL REFERENCES locations(id),
    master_item_id BIGINT NOT NULL REFERENCES items(id),
    single_item_id BIGINT NOT NULL REFERENCES items(id),
    master_qty BIGINT NOT NULL CHECK (master_qty > 0),
    master_qty_completed BIGINT NOT NULL DEFAULT 0 CHECK (master_qty_completed >= 0),
    inner_qty_snapshot BIGINT NOT NULL CHECK (inner_qty_snapshot > 1),
    CHECK (master_qty_completed <= master_qty)
);
CREATE INDEX idx_break_master_pack_tasks_location
    ON break_master_pack_tasks(location_id);
CREATE INDEX idx_break_master_pack_tasks_items
    ON break_master_pack_tasks(master_item_id, single_item_id);

CREATE TABLE unpack_cancelled_order_tasks (
    task_id BIGINT PRIMARY KEY REFERENCES work_tasks(id) ON DELETE CASCADE,
    order_id BIGINT NOT NULL REFERENCES orders(id)
);
CREATE INDEX idx_unpack_cancelled_order_tasks_order
    ON unpack_cancelled_order_tasks(order_id);

CREATE TABLE unpack_cancelled_order_task_lines (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    task_id BIGINT NOT NULL REFERENCES unpack_cancelled_order_tasks(task_id) ON DELETE CASCADE,
    order_item_id BIGINT REFERENCES order_items(id),
    item_id BIGINT NOT NULL REFERENCES items(id),
    item_batch_id BIGINT REFERENCES item_batches(id),
    inventory_balance_id BIGINT REFERENCES inventory_balances(id),
    license_plate_id BIGINT REFERENCES license_plates(id),
    source_location_id BIGINT REFERENCES locations(id),
    destination_location_id BIGINT REFERENCES locations(id),
    expected_qty BIGINT NOT NULL CHECK (expected_qty > 0),
    unpacked_qty BIGINT NOT NULL DEFAULT 0 CHECK (unpacked_qty >= 0),
    missing_qty BIGINT NOT NULL DEFAULT 0 CHECK (missing_qty >= 0),
    damaged_qty BIGINT NOT NULL DEFAULT 0 CHECK (damaged_qty >= 0),
    status TEXT NOT NULL DEFAULT 'open',
    CHECK (status IN ('open', 'partial', 'completed', 'exception')),
    CHECK (unpacked_qty + missing_qty + damaged_qty <= expected_qty)
);
CREATE INDEX idx_unpack_cancelled_order_task_lines_task
    ON unpack_cancelled_order_task_lines(task_id, status);

CREATE TABLE work_task_progress (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    task_id BIGINT NOT NULL REFERENCES work_tasks(id) ON DELETE CASCADE,
    task_line_id BIGINT REFERENCES unpack_cancelled_order_task_lines(id),
    user_id BIGINT REFERENCES users(id),
    action TEXT NOT NULL,
    qty_delta BIGINT,
    from_location_id BIGINT REFERENCES locations(id),
    to_location_id BIGINT REFERENCES locations(id),
    note TEXT,
    metadata_json TEXT,
    CHECK (action IN ('started', 'aborted', 'expired', 'completed', 'progress', 'unpacked', 'missing', 'damaged', 'moved')),
    CHECK (qty_delta IS NULL OR qty_delta > 0)
);
CREATE INDEX idx_work_task_progress_task
    ON work_task_progress(task_id, created);
