CREATE TABLE outbox_event_keys (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    event_key TEXT NOT NULL,
    created TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, event_key),
    CHECK (btrim(event_key) <> '')
);

CREATE TABLE outbox_aggregate_sequences (
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    ordering_key TEXT NOT NULL,
    last_sequence BIGINT NOT NULL,
    updated TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, ordering_key),
    CHECK (btrim(ordering_key) <> ''),
    CHECK (last_sequence > 0)
);

CREATE TABLE outbox_events (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    tenant_id BIGINT NOT NULL REFERENCES tenants(id),
    inventory_owner_id BIGINT,
    facility_id BIGINT,
    actor_user_id BIGINT,
    created TIMESTAMPTZ NOT NULL,
    event_key TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT NOT NULL,
    ordering_key TEXT NOT NULL,
    aggregate_sequence BIGINT NOT NULL,
    event_type TEXT NOT NULL,
    schema_version INTEGER NOT NULL,
    payload JSONB NOT NULL,
    occurred_at TIMESTAMPTZ NOT NULL,
    available_at TIMESTAMPTZ NOT NULL,
    claimed_at TIMESTAMPTZ,
    claimed_by TEXT,
    lease_expires_at TIMESTAMPTZ,
    claim_version BIGINT NOT NULL DEFAULT 0,
    attempts INTEGER NOT NULL DEFAULT 0,
    last_error TEXT,
    dead_lettered_at TIMESTAMPTZ,
    replay_count INTEGER NOT NULL DEFAULT 0,
    discarded_at TIMESTAMPTZ,
    discard_reason TEXT,
    discarded_by_user_id BIGINT,
    published_at TIMESTAMPTZ,
    UNIQUE (tenant_id, id),
    UNIQUE (tenant_id, event_key),
    UNIQUE (tenant_id, ordering_key, aggregate_sequence),
    FOREIGN KEY (tenant_id, inventory_owner_id)
        REFERENCES inventory_owners(tenant_id, id),
    FOREIGN KEY (tenant_id, facility_id)
        REFERENCES facilities(tenant_id, id),
    FOREIGN KEY (tenant_id, actor_user_id)
        REFERENCES tenant_memberships(tenant_id, user_id),
    FOREIGN KEY (tenant_id, discarded_by_user_id)
        REFERENCES tenant_memberships(tenant_id, user_id),
    CHECK (btrim(event_key) <> ''),
    CHECK (btrim(aggregate_type) <> ''),
    CHECK (btrim(aggregate_id) <> ''),
    CHECK (btrim(ordering_key) <> ''),
    CHECK (aggregate_sequence > 0),
    CHECK (btrim(event_type) <> ''),
    CHECK (schema_version > 0),
    CHECK (jsonb_typeof(payload) = 'object'),
    CHECK (attempts >= 0),
    CHECK (claim_version >= 0),
    CHECK (replay_count >= 0),
    CHECK ((claimed_at IS NULL) = (claimed_by IS NULL)),
    CHECK ((claimed_at IS NULL) = (lease_expires_at IS NULL)),
    CHECK (claimed_by IS NULL OR btrim(claimed_by) <> ''),
    CHECK (last_error IS NULL OR btrim(last_error) <> ''),
    CHECK (discard_reason IS NULL OR btrim(discard_reason) <> ''),
    CHECK (
        (discarded_at IS NULL AND discard_reason IS NULL AND discarded_by_user_id IS NULL)
        OR (discarded_at IS NOT NULL AND discard_reason IS NOT NULL AND discarded_by_user_id IS NOT NULL)
    ),
    CHECK (
        dead_lettered_at IS NULL
        OR (
            claimed_at IS NULL
            AND claimed_by IS NULL
            AND lease_expires_at IS NULL
            AND published_at IS NULL
        )
    ),
    CHECK (
        published_at IS NULL
        OR (
            claimed_at IS NULL
            AND claimed_by IS NULL
            AND lease_expires_at IS NULL
            AND last_error IS NULL
            AND dead_lettered_at IS NULL
            AND discarded_at IS NULL
        )
    )
);

CREATE INDEX outbox_events_ready_idx
    ON outbox_events(available_at, id)
    WHERE published_at IS NULL AND dead_lettered_at IS NULL;
CREATE INDEX outbox_events_tenant_history_idx
    ON outbox_events(tenant_id, created, id);

CREATE OR REPLACE FUNCTION protect_outbox_event_envelope()
RETURNS trigger AS $$
BEGIN
    IF NEW.id IS DISTINCT FROM OLD.id
        OR NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
        OR NEW.inventory_owner_id IS DISTINCT FROM OLD.inventory_owner_id
        OR NEW.facility_id IS DISTINCT FROM OLD.facility_id
        OR NEW.actor_user_id IS DISTINCT FROM OLD.actor_user_id
        OR NEW.created IS DISTINCT FROM OLD.created
        OR NEW.event_key IS DISTINCT FROM OLD.event_key
        OR NEW.aggregate_type IS DISTINCT FROM OLD.aggregate_type
        OR NEW.aggregate_id IS DISTINCT FROM OLD.aggregate_id
        OR NEW.ordering_key IS DISTINCT FROM OLD.ordering_key
        OR NEW.aggregate_sequence IS DISTINCT FROM OLD.aggregate_sequence
        OR NEW.event_type IS DISTINCT FROM OLD.event_type
        OR NEW.schema_version IS DISTINCT FROM OLD.schema_version
        OR NEW.payload IS DISTINCT FROM OLD.payload
        OR NEW.occurred_at IS DISTINCT FROM OLD.occurred_at
    THEN
        RAISE EXCEPTION 'outbox event envelopes are immutable'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER outbox_event_envelopes_are_immutable
    BEFORE UPDATE ON outbox_events
    FOR EACH ROW EXECUTE FUNCTION protect_outbox_event_envelope();
