CREATE TABLE work_tasks (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    modified TIMESTAMPTZ,
    deleted TIMESTAMPTZ,
    task_type TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'open',
    required_permission TEXT NOT NULL DEFAULT 'wms',
    priority BIGINT NOT NULL DEFAULT 0,
    title TEXT NOT NULL,
    instructions TEXT,
    assigned_user_id BIGINT,
    created_by BIGINT,
    completed_by BIGINT,
    scheduled_for TIMESTAMPTZ,
    due_at TIMESTAMPTZ,
    started_at TIMESTAMPTZ,
    lease_expires_at TIMESTAMPTZ,
    task_timeout_seconds BIGINT NOT NULL DEFAULT 1800,
    last_released_at TIMESTAMPTZ,
    release_count BIGINT NOT NULL DEFAULT 0,
    completed_at TIMESTAMPTZ,
    metadata_json TEXT,
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, assigned_user_id) REFERENCES tenant_memberships(tenant_id, user_id),
    FOREIGN KEY (tenant_id, created_by) REFERENCES tenant_memberships(tenant_id, user_id),
    FOREIGN KEY (tenant_id, completed_by) REFERENCES tenant_memberships(tenant_id, user_id),
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
    ON work_tasks(tenant_id, status, required_permission, scheduled_for, priority DESC, created)
    WHERE deleted IS NULL;
CREATE INDEX idx_work_tasks_expiring_leases
    ON work_tasks(tenant_id, lease_expires_at)
    WHERE deleted IS NULL AND status IN ('assigned', 'in_progress') AND lease_expires_at IS NOT NULL;
CREATE INDEX idx_work_tasks_assigned
    ON work_tasks(tenant_id, assigned_user_id, status, scheduled_for)
    WHERE deleted IS NULL AND assigned_user_id IS NOT NULL;
CREATE UNIQUE INDEX idx_work_tasks_one_active_per_user
    ON work_tasks(tenant_id, assigned_user_id)
    WHERE deleted IS NULL
      AND assigned_user_id IS NOT NULL
      AND status IN ('assigned', 'in_progress');

CREATE TABLE cycle_count_item_location_tasks (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    location_id BIGINT NOT NULL,
    item_id BIGINT NOT NULL,
    inventory_balance_id BIGINT,
    order_id BIGINT,
    order_item_id BIGINT,
    source TEXT,
    note TEXT,
    PRIMARY KEY (tenant_id, task_id),
    FOREIGN KEY (tenant_id, task_id) REFERENCES work_tasks(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, facility_id) REFERENCES facilities(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id, location_id) REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_balance_id) REFERENCES inventory_balances(tenant_id, id),
    FOREIGN KEY (tenant_id, order_id) REFERENCES orders(tenant_id, id),
    FOREIGN KEY (tenant_id, order_item_id) REFERENCES order_items(tenant_id, id)
);
CREATE INDEX idx_cycle_count_item_location_tasks_location
    ON cycle_count_item_location_tasks(tenant_id, location_id, item_id);

CREATE TABLE cycle_count_location_tasks (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    location_id BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, task_id),
    FOREIGN KEY (tenant_id, task_id) REFERENCES work_tasks(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, facility_id) REFERENCES facilities(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id, location_id) REFERENCES locations(tenant_id, facility_id, id)
);
CREATE INDEX idx_cycle_count_location_tasks_location
    ON cycle_count_location_tasks(tenant_id, location_id);

CREATE TABLE break_master_pack_tasks (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    location_id BIGINT NOT NULL,
    master_item_id BIGINT NOT NULL,
    single_item_id BIGINT NOT NULL,
    master_qty BIGINT NOT NULL CHECK (master_qty > 0),
    master_qty_completed BIGINT NOT NULL DEFAULT 0 CHECK (master_qty_completed >= 0),
    inner_qty_snapshot BIGINT NOT NULL CHECK (inner_qty_snapshot > 1),
    PRIMARY KEY (tenant_id, task_id),
    FOREIGN KEY (tenant_id, task_id) REFERENCES work_tasks(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, facility_id) REFERENCES facilities(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id, location_id) REFERENCES locations(tenant_id, facility_id, id),
    FOREIGN KEY (tenant_id, master_item_id) REFERENCES items(tenant_id, id),
    FOREIGN KEY (tenant_id, single_item_id) REFERENCES items(tenant_id, id),
    CHECK (master_qty_completed <= master_qty)
);
CREATE INDEX idx_break_master_pack_tasks_location
    ON break_master_pack_tasks(tenant_id, location_id);
CREATE INDEX idx_break_master_pack_tasks_items
    ON break_master_pack_tasks(tenant_id, master_item_id, single_item_id);

CREATE TABLE unpack_cancelled_order_tasks (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    order_id BIGINT NOT NULL,
    PRIMARY KEY (tenant_id, task_id),
    FOREIGN KEY (tenant_id, task_id) REFERENCES work_tasks(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, order_id) REFERENCES orders(tenant_id, id)
);
CREATE INDEX idx_unpack_cancelled_order_tasks_order
    ON unpack_cancelled_order_tasks(tenant_id, order_id);

CREATE TABLE unpack_cancelled_order_task_lines (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    task_id BIGINT NOT NULL,
    order_item_id BIGINT,
    item_id BIGINT NOT NULL,
    item_batch_id BIGINT,
    inventory_balance_id BIGINT,
    license_plate_id BIGINT,
    source_location_id BIGINT,
    destination_location_id BIGINT,
    expected_qty BIGINT NOT NULL CHECK (expected_qty > 0),
    unpacked_qty BIGINT NOT NULL DEFAULT 0 CHECK (unpacked_qty >= 0),
    missing_qty BIGINT NOT NULL DEFAULT 0 CHECK (missing_qty >= 0),
    damaged_qty BIGINT NOT NULL DEFAULT 0 CHECK (damaged_qty >= 0),
    status TEXT NOT NULL DEFAULT 'open',
    UNIQUE (tenant_id, id),
    FOREIGN KEY (tenant_id, task_id) REFERENCES unpack_cancelled_order_tasks(tenant_id, task_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, order_item_id) REFERENCES order_items(tenant_id, id),
    FOREIGN KEY (tenant_id, item_id) REFERENCES items(tenant_id, id),
    FOREIGN KEY (tenant_id, item_batch_id) REFERENCES item_batches(tenant_id, id),
    FOREIGN KEY (tenant_id, inventory_balance_id) REFERENCES inventory_balances(tenant_id, id),
    FOREIGN KEY (tenant_id, license_plate_id) REFERENCES license_plates(tenant_id, id),
    FOREIGN KEY (tenant_id, source_location_id) REFERENCES locations(tenant_id, id),
    FOREIGN KEY (tenant_id, destination_location_id) REFERENCES locations(tenant_id, id),
    CHECK (status IN ('open', 'partial', 'completed', 'exception')),
    CHECK (unpacked_qty + missing_qty + damaged_qty <= expected_qty)
);
CREATE INDEX idx_unpack_cancelled_order_task_lines_task
    ON unpack_cancelled_order_task_lines(tenant_id, task_id, status);

CREATE TABLE work_task_progress (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    task_id BIGINT NOT NULL,
    task_line_id BIGINT,
    user_id BIGINT,
    action TEXT NOT NULL,
    qty_delta BIGINT,
    from_location_id BIGINT,
    to_location_id BIGINT,
    note TEXT,
    metadata_json TEXT,
    FOREIGN KEY (tenant_id, task_id) REFERENCES work_tasks(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, task_line_id) REFERENCES unpack_cancelled_order_task_lines(tenant_id, id),
    FOREIGN KEY (tenant_id, user_id) REFERENCES tenant_memberships(tenant_id, user_id),
    FOREIGN KEY (tenant_id, from_location_id) REFERENCES locations(tenant_id, id),
    FOREIGN KEY (tenant_id, to_location_id) REFERENCES locations(tenant_id, id),
    CHECK (action IN ('started', 'aborted', 'expired', 'completed', 'cancelled', 'progress', 'unpacked', 'missing', 'damaged', 'moved')),
    CHECK (qty_delta IS NULL OR qty_delta > 0)
);
CREATE INDEX idx_work_task_progress_task
    ON work_task_progress(tenant_id, task_id, created);
