CREATE TABLE edge_cloud_deliveries (
    cloud_command_id INTEGER PRIMARY KEY CHECK (cloud_command_id > 0),
    cloud_device_id INTEGER NOT NULL CHECK (cloud_device_id > 0),
    local_command_id TEXT NOT NULL UNIQUE
        REFERENCES edge_commands(command_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
    delivery_token TEXT NOT NULL
        CHECK (length(delivery_token) BETWEEN 32 AND 200),
    delivery_revision INTEGER NOT NULL CHECK (delivery_revision > 0),
    acknowledgement_revision INTEGER,
    acknowledged_at_ms INTEGER,
    reported_revision INTEGER,
    reported_status TEXT
        CHECK (reported_status IS NULL OR reported_status IN (
            'succeeded', 'failed', 'manual_review'
        )),
    reported_at_ms INTEGER,
    last_cloud_error TEXT,
    updated_at_ms INTEGER NOT NULL,
    CHECK (
        (acknowledgement_revision IS NULL AND acknowledged_at_ms IS NULL)
        OR
        (acknowledgement_revision > 0 AND acknowledged_at_ms IS NOT NULL)
    ),
    CHECK (
        (reported_revision IS NULL AND reported_status IS NULL AND reported_at_ms IS NULL)
        OR
        (reported_revision > 0 AND reported_status IS NOT NULL AND reported_at_ms IS NOT NULL)
    )
);

CREATE INDEX edge_cloud_deliveries_pending_ack
    ON edge_cloud_deliveries (acknowledged_at_ms, cloud_command_id);

CREATE INDEX edge_cloud_deliveries_pending_report
    ON edge_cloud_deliveries (reported_at_ms, cloud_command_id);

CREATE TRIGGER edge_cloud_delivery_identity_immutable
BEFORE UPDATE ON edge_cloud_deliveries
WHEN OLD.cloud_command_id IS NOT NEW.cloud_command_id
  OR OLD.cloud_device_id IS NOT NEW.cloud_device_id
  OR OLD.local_command_id IS NOT NEW.local_command_id
BEGIN
    SELECT RAISE(ABORT, 'edge cloud delivery identity is immutable');
END;

CREATE TRIGGER edge_cloud_deliveries_immutable_delete
BEFORE DELETE ON edge_cloud_deliveries
BEGIN
    SELECT RAISE(ABORT, 'edge cloud deliveries cannot be deleted');
END;
