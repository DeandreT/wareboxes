CREATE TABLE edge_devices (
    device_id TEXT PRIMARY KEY
        CHECK (length(device_id) BETWEEN 1 AND 128),
    tenant_id TEXT NOT NULL
        CHECK (length(tenant_id) BETWEEN 1 AND 128),
    facility_id TEXT NOT NULL
        CHECK (length(facility_id) BETWEEN 1 AND 128),
    device_class TEXT NOT NULL
        CHECK (device_class IN ('plc', 'conveyor', 'robotics', 'sortation', 'printer', 'scale')),
    display_name TEXT NOT NULL
        CHECK (length(trim(display_name)) BETWEEN 1 AND 200),
    control_mode TEXT NOT NULL
        CHECK (control_mode IN ('disabled', 'automatic', 'manual_fallback')),
    control_reason TEXT NOT NULL
        CHECK (length(trim(control_reason)) BETWEEN 1 AND 1000),
    control_actor TEXT NOT NULL
        CHECK (length(control_actor) BETWEEN 1 AND 128),
    control_changed_at_ms INTEGER NOT NULL,
    health_state TEXT NOT NULL DEFAULT 'unknown'
        CHECK (health_state IN ('unknown', 'healthy', 'degraded', 'offline', 'faulted')),
    health_message TEXT,
    last_heartbeat_at_ms INTEGER,
    consecutive_health_failures INTEGER NOT NULL DEFAULT 0
        CHECK (consecutive_health_failures >= 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    UNIQUE (tenant_id, facility_id, device_id)
);

CREATE TABLE edge_commands (
    command_id TEXT PRIMARY KEY
        CHECK (length(command_id) BETWEEN 1 AND 128),
    tenant_id TEXT NOT NULL,
    facility_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    correlation_id TEXT NOT NULL
        CHECK (length(correlation_id) BETWEEN 1 AND 128),
    idempotency_key TEXT NOT NULL
        CHECK (length(idempotency_key) BETWEEN 1 AND 200),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    device_class TEXT NOT NULL
        CHECK (device_class IN ('plc', 'conveyor', 'robotics', 'sortation', 'printer', 'scale')),
    recovery_policy TEXT NOT NULL
        CHECK (recovery_policy IN (
            'device_deduplicated_replay',
            'probe_then_retry',
            'manual_review'
        )),
    request_hash BLOB NOT NULL CHECK (length(request_hash) = 32),
    request_json BLOB NOT NULL,
    state TEXT NOT NULL
        CHECK (state IN (
            'queued',
            'executing',
            'retry_wait',
            'recovery_wait',
            'succeeded',
            'failed',
            'manual_review',
            'resolved_manually',
            'cancelled'
        )),
    attempt_count INTEGER NOT NULL DEFAULT 0 CHECK (attempt_count >= 0),
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    next_attempt_at_ms INTEGER NOT NULL,
    lease_token TEXT,
    lease_until_ms INTEGER,
    result_json BLOB,
    last_error TEXT,
    resolution_note TEXT,
    FOREIGN KEY (tenant_id, facility_id, device_id)
        REFERENCES edge_devices (tenant_id, facility_id, device_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT,
    UNIQUE (tenant_id, facility_id, device_id, correlation_id),
    UNIQUE (tenant_id, facility_id, device_id, idempotency_key),
    CHECK (
        (state = 'executing' AND lease_token IS NOT NULL AND lease_until_ms IS NOT NULL)
        OR
        (state <> 'executing' AND lease_token IS NULL AND lease_until_ms IS NULL)
    ),
    CHECK (state <> 'succeeded' OR result_json IS NOT NULL)
);

CREATE UNIQUE INDEX edge_commands_one_executing_per_device
    ON edge_commands (device_id)
    WHERE state = 'executing';

CREATE INDEX edge_commands_due
    ON edge_commands (state, next_attempt_at_ms, created_at_ms);

CREATE TABLE edge_command_attempts (
    attempt_id TEXT PRIMARY KEY,
    command_id TEXT NOT NULL
        REFERENCES edge_commands(command_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
    sequence INTEGER NOT NULL CHECK (sequence > 0),
    execution_attempt INTEGER NOT NULL CHECK (execution_attempt >= 0),
    attempt_kind TEXT NOT NULL CHECK (attempt_kind IN ('execute', 'recovery_probe')),
    state TEXT NOT NULL CHECK (state IN (
        'active',
        'succeeded',
        'retryable_failure',
        'permanent_failure',
        'ambiguous',
        'not_found',
        'still_processing',
        'manual_review',
        'abandoned'
    )),
    started_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER,
    message TEXT,
    result_json BLOB,
    CHECK (
        (state = 'active' AND finished_at_ms IS NULL)
        OR
        (state <> 'active' AND finished_at_ms IS NOT NULL)
    ),
    UNIQUE (command_id, sequence)
);

CREATE TABLE edge_command_events (
    event_id INTEGER PRIMARY KEY,
    command_id TEXT NOT NULL
        REFERENCES edge_commands(command_id) ON UPDATE RESTRICT ON DELETE RESTRICT,
    from_state TEXT,
    to_state TEXT NOT NULL,
    actor TEXT,
    reason TEXT NOT NULL,
    occurred_at_ms INTEGER NOT NULL
);

CREATE TABLE edge_control_events (
    event_id INTEGER PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    facility_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    from_mode TEXT,
    to_mode TEXT NOT NULL,
    actor TEXT NOT NULL,
    reason TEXT NOT NULL,
    occurred_at_ms INTEGER NOT NULL,
    FOREIGN KEY (tenant_id, facility_id, device_id)
        REFERENCES edge_devices (tenant_id, facility_id, device_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
);

CREATE TABLE edge_heartbeat_events (
    event_id INTEGER PRIMARY KEY,
    tenant_id TEXT NOT NULL,
    facility_id TEXT NOT NULL,
    device_id TEXT NOT NULL,
    health_state TEXT NOT NULL,
    message TEXT,
    alarm_codes_json BLOB NOT NULL,
    observed_at_ms INTEGER NOT NULL,
    FOREIGN KEY (tenant_id, facility_id, device_id)
        REFERENCES edge_devices (tenant_id, facility_id, device_id)
        ON UPDATE RESTRICT ON DELETE RESTRICT
);

CREATE TRIGGER edge_devices_scope_immutable
BEFORE UPDATE ON edge_devices
WHEN OLD.device_id IS NOT NEW.device_id
  OR OLD.tenant_id IS NOT NEW.tenant_id
  OR OLD.facility_id IS NOT NEW.facility_id
  OR OLD.device_class IS NOT NEW.device_class
BEGIN
    SELECT RAISE(ABORT, 'edge device identity and scope are immutable');
END;

CREATE TRIGGER edge_commands_identity_immutable
BEFORE UPDATE ON edge_commands
WHEN OLD.command_id IS NOT NEW.command_id
  OR OLD.tenant_id IS NOT NEW.tenant_id
  OR OLD.facility_id IS NOT NEW.facility_id
  OR OLD.device_id IS NOT NEW.device_id
  OR OLD.correlation_id IS NOT NEW.correlation_id
  OR OLD.idempotency_key IS NOT NEW.idempotency_key
  OR OLD.schema_version IS NOT NEW.schema_version
  OR OLD.device_class IS NOT NEW.device_class
  OR OLD.recovery_policy IS NOT NEW.recovery_policy
  OR OLD.request_hash IS NOT NEW.request_hash
  OR OLD.request_json IS NOT NEW.request_json
  OR OLD.created_at_ms IS NOT NEW.created_at_ms
BEGIN
    SELECT RAISE(ABORT, 'edge command identity and request are immutable');
END;

CREATE TRIGGER edge_command_attempts_immutable_update
BEFORE UPDATE ON edge_command_attempts
WHEN OLD.state <> 'active'
BEGIN
    SELECT RAISE(ABORT, 'completed edge command attempts are immutable');
END;

CREATE TRIGGER edge_command_attempts_immutable_delete
BEFORE DELETE ON edge_command_attempts
BEGIN
    SELECT RAISE(ABORT, 'edge command attempts are immutable');
END;

CREATE TRIGGER edge_command_events_immutable_update
BEFORE UPDATE ON edge_command_events
BEGIN
    SELECT RAISE(ABORT, 'edge command events are immutable');
END;

CREATE TRIGGER edge_command_events_immutable_delete
BEFORE DELETE ON edge_command_events
BEGIN
    SELECT RAISE(ABORT, 'edge command events are immutable');
END;

CREATE TRIGGER edge_control_events_immutable_update
BEFORE UPDATE ON edge_control_events
BEGIN
    SELECT RAISE(ABORT, 'edge control events are immutable');
END;

CREATE TRIGGER edge_control_events_immutable_delete
BEFORE DELETE ON edge_control_events
BEGIN
    SELECT RAISE(ABORT, 'edge control events are immutable');
END;

CREATE TRIGGER edge_heartbeat_events_immutable_update
BEFORE UPDATE ON edge_heartbeat_events
BEGIN
    SELECT RAISE(ABORT, 'edge heartbeat events are immutable');
END;

CREATE TRIGGER edge_heartbeat_events_immutable_delete
BEFORE DELETE ON edge_heartbeat_events
BEGIN
    SELECT RAISE(ABORT, 'edge heartbeat events are immutable');
END;

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
