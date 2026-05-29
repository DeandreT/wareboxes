-- Wareboxes WMS schema (PostgreSQL). Ported from the Drizzle schema in
-- app/utils/types/db/*.ts. Timestamps use TIMESTAMPTZ;
-- booleans use native BOOLEAN.

CREATE TABLE addresses (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT,
    company TEXT,
    line1 TEXT NOT NULL,
    line2 TEXT,
    postal_code TEXT,
    country TEXT NOT NULL,
    phone TEXT,
    email TEXT,
    state TEXT,
    county TEXT,
    city TEXT,
    territory TEXT,
    district TEXT,
    validated TIMESTAMPTZ
);

CREATE TABLE users (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    first_name TEXT,
    last_name TEXT,
    email TEXT NOT NULL UNIQUE,
    nick_name TEXT,
    phone TEXT
);

CREATE TABLE user_credentials (
    user_id BIGINT PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    password_hash TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL
);

CREATE TABLE sessions (
    token TEXT PRIMARY KEY,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created TIMESTAMPTZ NOT NULL,
    expires TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_sessions_user_id ON sessions(user_id);
CREATE INDEX idx_sessions_expires ON sessions(expires);

CREATE TABLE roles (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT NOT NULL UNIQUE,
    description TEXT,
    parent_id BIGINT REFERENCES roles(id)
);
CREATE INDEX roles_parent_id_key ON roles(parent_id);

CREATE TABLE user_roles (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    role_id BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    UNIQUE (user_id, role_id)
);
CREATE INDEX idx_user_roles_role_id ON user_roles(role_id);
CREATE INDEX idx_user_roles_user_id ON user_roles(user_id);

CREATE TABLE permissions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT NOT NULL UNIQUE,
    description TEXT
);

CREATE TABLE role_permissions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    role_id BIGINT NOT NULL REFERENCES roles(id) ON DELETE CASCADE,
    permission_id BIGINT NOT NULL REFERENCES permissions(id) ON DELETE CASCADE,
    UNIQUE (role_id, permission_id)
);
CREATE INDEX idx_role_permissions_permission_id ON role_permissions(permission_id);
CREATE INDEX idx_role_permissions_role_id ON role_permissions(role_id);

CREATE TABLE accounts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT NOT NULL UNIQUE,
    email TEXT NOT NULL
);

CREATE TABLE warehouses (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT,
    address_id BIGINT REFERENCES addresses(id)
);

CREATE TABLE account_warehouses (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    account_id BIGINT NOT NULL REFERENCES accounts(id),
    warehouse_id BIGINT NOT NULL REFERENCES warehouses(id),
    UNIQUE (account_id, warehouse_id)
);
CREATE INDEX idx_account_warehouses_account_id ON account_warehouses(account_id);
CREATE INDEX idx_account_warehouses_warehouse_id ON account_warehouses(warehouse_id);

CREATE TABLE user_accounts (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    account_id BIGINT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
    is_primary BOOLEAN NOT NULL DEFAULT false,
    UNIQUE (user_id, account_id)
);
CREATE INDEX idx_user_accounts_account_id ON user_accounts(account_id);
CREATE UNIQUE INDEX idx_user_accounts_one_primary
    ON user_accounts(user_id)
    WHERE is_primary AND deleted IS NULL;

CREATE TABLE pick_waves (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT
);

CREATE TABLE orders (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    order_key TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    rush BOOLEAN NOT NULL DEFAULT false,
    status TEXT NOT NULL DEFAULT 'open',
    address_id BIGINT NOT NULL,
    confirmed TIMESTAMPTZ,
    closed TIMESTAMPTZ,
    ship_by TIMESTAMPTZ,
    wave_id BIGINT REFERENCES pick_waves(id),
    account_id BIGINT REFERENCES accounts(id),
    CHECK (status IN ('awaiting shipment', 'shipped', 'cancelled', 'held', 'processing', 'open', 'void')),
    UNIQUE (order_key, account_id)
);
CREATE INDEX idx_orders_account_status ON orders(account_id, status) WHERE deleted IS NULL;
CREATE INDEX idx_orders_status_created ON orders(status, created DESC) WHERE deleted IS NULL;
CREATE INDEX idx_orders_ship_by ON orders(ship_by) WHERE deleted IS NULL AND ship_by IS NOT NULL;

CREATE TABLE order_items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    qty BIGINT NOT NULL CHECK (qty > 0),
    item_id BIGINT NOT NULL,
    order_id BIGINT NOT NULL REFERENCES orders(id),
    item_batch_id BIGINT
);
CREATE INDEX idx_order_items_order_id ON order_items(order_id);
CREATE INDEX idx_order_items_item_id ON order_items(item_id);

CREATE TABLE order_activity (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    order_id BIGINT REFERENCES orders(id),
    action TEXT NOT NULL
);
CREATE INDEX idx_order_activity_order_id ON order_activity(order_id);
