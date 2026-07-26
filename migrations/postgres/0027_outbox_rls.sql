ALTER TABLE outbox_event_keys ENABLE ROW LEVEL SECURITY;
ALTER TABLE outbox_event_keys FORCE ROW LEVEL SECURITY;

CREATE POLICY outbox_event_keys_tenant_isolation
ON outbox_event_keys
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE outbox_aggregate_sequences ENABLE ROW LEVEL SECURITY;
ALTER TABLE outbox_aggregate_sequences FORCE ROW LEVEL SECURITY;

CREATE POLICY outbox_aggregate_sequences_tenant_isolation
ON outbox_aggregate_sequences
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE outbox_events ENABLE ROW LEVEL SECURITY;
ALTER TABLE outbox_events FORCE ROW LEVEL SECURITY;

CREATE POLICY outbox_events_tenant_isolation
ON outbox_events
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

DROP INDEX outbox_events_ready_idx;
CREATE INDEX outbox_events_tenant_ready_idx
    ON outbox_events(tenant_id, available_at, id)
    WHERE published_at IS NULL
      AND dead_lettered_at IS NULL
      AND discarded_at IS NULL;

CREATE INDEX outbox_events_tenant_ordering_blocker_idx
    ON outbox_events(tenant_id, ordering_key, aggregate_sequence)
    WHERE published_at IS NULL AND discarded_at IS NULL;

CREATE INDEX outbox_events_tenant_terminal_idx
    ON outbox_events(
        tenant_id,
        COALESCE(published_at, discarded_at),
        id
    )
    WHERE published_at IS NOT NULL OR discarded_at IS NOT NULL;
