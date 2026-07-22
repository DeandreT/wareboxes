-- Wareboxes WMS schema (PostgreSQL). Ported from the Drizzle schema in
-- app/utils/types/db/*.ts. Timestamps use TIMESTAMPTZ;
-- booleans use native BOOLEAN.

CREATE TABLE tenants (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted TIMESTAMPTZ,
    slug TEXT NOT NULL,
    name TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'active',
    CONSTRAINT tenants_slug_unique UNIQUE (slug),
    CONSTRAINT tenants_slug_not_blank CHECK (btrim(slug) <> ''),
    CONSTRAINT tenants_name_not_blank CHECK (btrim(name) <> ''),
    CONSTRAINT tenants_status_valid CHECK (status IN ('active', 'suspended'))
);

CREATE TABLE addresses (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
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
    validated TIMESTAMPTZ,
    UNIQUE (tenant_id, id)
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

CREATE TABLE tenant_memberships (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    user_id BIGINT NOT NULL REFERENCES users(id),
    created TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,
    deleted TIMESTAMPTZ,
    is_default BOOLEAN NOT NULL DEFAULT FALSE,
    CONSTRAINT tenant_memberships_tenant_user_unique UNIQUE (tenant_id, user_id)
);
CREATE INDEX tenant_memberships_user_active_idx
    ON tenant_memberships (user_id, tenant_id)
    WHERE deleted IS NULL;
CREATE UNIQUE INDEX tenant_memberships_one_default_per_user_idx
    ON tenant_memberships (user_id)
    WHERE deleted IS NULL AND is_default;

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
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT NOT NULL,
    description TEXT,
    parent_id BIGINT,
    self_user_id BIGINT,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, name),
    UNIQUE (tenant_id, self_user_id),
    FOREIGN KEY (tenant_id, parent_id) REFERENCES roles(tenant_id, id),
    FOREIGN KEY (tenant_id, self_user_id) REFERENCES tenant_memberships(tenant_id, user_id),
    CHECK (self_user_id IS NULL OR description = 'Self role')
);
CREATE INDEX roles_parent_id_key ON roles(tenant_id, parent_id);

CREATE TABLE user_roles (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    user_id BIGINT NOT NULL,
    role_id BIGINT NOT NULL,
    FOREIGN KEY (tenant_id, user_id) REFERENCES tenant_memberships(tenant_id, user_id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, role_id) REFERENCES roles(tenant_id, id) ON DELETE CASCADE,
    UNIQUE (tenant_id, user_id, role_id)
);
CREATE INDEX idx_user_roles_role_id ON user_roles(tenant_id, role_id);
CREATE INDEX idx_user_roles_user_id ON user_roles(tenant_id, user_id);

CREATE TABLE permissions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT NOT NULL,
    description TEXT,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, name)
);

CREATE TABLE role_permissions (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    role_id BIGINT NOT NULL,
    permission_id BIGINT NOT NULL,
    FOREIGN KEY (tenant_id, role_id) REFERENCES roles(tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, permission_id) REFERENCES permissions(tenant_id, id) ON DELETE CASCADE,
    UNIQUE (tenant_id, role_id, permission_id)
);
CREATE INDEX idx_role_permissions_permission_id ON role_permissions(tenant_id, permission_id);
CREATE INDEX idx_role_permissions_role_id ON role_permissions(tenant_id, role_id);

CREATE TABLE inventory_owners (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT NOT NULL,
    email TEXT NOT NULL,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, name)
);

CREATE TABLE facilities (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT,
    address_id BIGINT,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, name),
    FOREIGN KEY (tenant_id, address_id) REFERENCES addresses(tenant_id, id)
);

CREATE TABLE inventory_owner_facilities (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    inventory_owner_id BIGINT NOT NULL,
    facility_id BIGINT NOT NULL,
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id) REFERENCES facilities(tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, facility_id)
);
CREATE INDEX idx_inventory_owner_facilities_inventory_owner_id ON inventory_owner_facilities(inventory_owner_id);
CREATE INDEX idx_inventory_owner_facilities_facility_id ON inventory_owner_facilities(facility_id);

CREATE TABLE user_inventory_owners (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    user_id BIGINT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    inventory_owner_id BIGINT NOT NULL,
    is_primary BOOLEAN NOT NULL DEFAULT false,
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id) ON DELETE CASCADE,
    UNIQUE (tenant_id, user_id, inventory_owner_id)
);
CREATE INDEX idx_user_inventory_owners_inventory_owner_id ON user_inventory_owners(inventory_owner_id);
CREATE UNIQUE INDEX idx_user_inventory_owners_one_primary
    ON user_inventory_owners(tenant_id, user_id)
    WHERE is_primary AND deleted IS NULL;

CREATE TABLE pick_waves (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    name TEXT,
    UNIQUE (tenant_id, id)
);

CREATE TABLE orders (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    order_key TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    rush BOOLEAN NOT NULL DEFAULT false,
    status TEXT NOT NULL DEFAULT 'open',
    address_id BIGINT NOT NULL,
    confirmed TIMESTAMPTZ,
    closed TIMESTAMPTZ,
    ship_by TIMESTAMPTZ,
    wave_id BIGINT,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id) REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, address_id) REFERENCES addresses(tenant_id, id),
    FOREIGN KEY (tenant_id, wave_id) REFERENCES pick_waves(tenant_id, id),
    CHECK (status IN ('awaiting shipment', 'shipped', 'cancelled', 'held', 'processing', 'open', 'void')),
    UNIQUE (tenant_id, inventory_owner_id, order_key)
);
CREATE INDEX idx_orders_inventory_owner_status ON orders(tenant_id, inventory_owner_id, status) WHERE deleted IS NULL;
CREATE INDEX idx_orders_status_created ON orders(tenant_id, status, created DESC) WHERE deleted IS NULL;
CREATE INDEX idx_orders_ship_by ON orders(tenant_id, ship_by) WHERE deleted IS NULL AND ship_by IS NOT NULL;

CREATE TABLE order_items (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    qty BIGINT NOT NULL CHECK (qty > 0),
    item_id BIGINT NOT NULL,
    order_id BIGINT NOT NULL,
    item_batch_id BIGINT,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, inventory_owner_id, id),
    FOREIGN KEY (tenant_id, inventory_owner_id, order_id) REFERENCES orders(tenant_id, inventory_owner_id, id)
);
CREATE INDEX idx_order_items_order_id ON order_items(tenant_id, inventory_owner_id, order_id);
CREATE INDEX idx_order_items_item_id ON order_items(tenant_id, item_id);

CREATE TABLE order_activity (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    order_id BIGINT NOT NULL,
    action TEXT NOT NULL,
    FOREIGN KEY (tenant_id, inventory_owner_id, order_id) REFERENCES orders(tenant_id, inventory_owner_id, id)
);
CREATE INDEX idx_order_activity_order_id ON order_activity(tenant_id, inventory_owner_id, order_id);
