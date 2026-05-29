CREATE TABLE order_tracking_numbers (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    order_id BIGINT NOT NULL REFERENCES orders(id),
    tracking_number TEXT NOT NULL,
    carrier TEXT,
    service TEXT,
    UNIQUE (order_id, tracking_number)
);

CREATE INDEX idx_order_tracking_numbers_order_id
    ON order_tracking_numbers(order_id)
    WHERE deleted IS NULL;
