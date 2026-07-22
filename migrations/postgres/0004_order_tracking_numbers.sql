CREATE TABLE order_tracking_numbers (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    order_id BIGINT NOT NULL,
    tracking_number TEXT NOT NULL,
    carrier TEXT,
    service TEXT,
    FOREIGN KEY (tenant_id, inventory_owner_id, order_id) REFERENCES orders(tenant_id, inventory_owner_id, id),
    UNIQUE (tenant_id, inventory_owner_id, order_id, tracking_number)
);

CREATE INDEX idx_order_tracking_numbers_order_id
    ON order_tracking_numbers(tenant_id, inventory_owner_id, order_id)
    WHERE deleted IS NULL;
