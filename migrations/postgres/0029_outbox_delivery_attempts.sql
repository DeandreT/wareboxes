CREATE TABLE outbox_delivery_attempts (
    tenant_id BIGINT NOT NULL,
    outbox_event_id BIGINT NOT NULL,
    event_key TEXT NOT NULL,
    event_type TEXT NOT NULL,
    claim_version BIGINT NOT NULL,
    replay_count INTEGER NOT NULL,
    attempt_number INTEGER NOT NULL,
    worker_id TEXT NOT NULL,
    publisher_name TEXT NOT NULL,
    claimed_at TIMESTAMPTZ NOT NULL,
    lease_expires_at TIMESTAMPTZ NOT NULL,
    PRIMARY KEY (tenant_id, outbox_event_id, claim_version),
    UNIQUE (tenant_id, event_key, claim_version),
    UNIQUE (tenant_id, event_key, replay_count, attempt_number),
    FOREIGN KEY (tenant_id, event_key)
        REFERENCES outbox_event_keys(tenant_id, event_key),
    CHECK (outbox_event_id > 0),
    CHECK (btrim(event_key) <> ''),
    CHECK (btrim(event_type) <> ''),
    CHECK (claim_version > 0),
    CHECK (replay_count >= 0),
    CHECK (attempt_number > 0),
    CHECK (btrim(worker_id) <> '' AND char_length(worker_id) <= 200),
    CHECK (btrim(publisher_name) <> '' AND char_length(publisher_name) <= 200),
    CHECK (lease_expires_at > claimed_at)
);

CREATE TABLE outbox_delivery_attempt_results (
    tenant_id BIGINT NOT NULL,
    outbox_event_id BIGINT NOT NULL,
    claim_version BIGINT NOT NULL,
    outcome TEXT NOT NULL,
    completed_at TIMESTAMPTZ NOT NULL,
    error TEXT,
    retry_after_seconds BIGINT,
    PRIMARY KEY (tenant_id, outbox_event_id, claim_version),
    FOREIGN KEY (tenant_id, outbox_event_id, claim_version)
        REFERENCES outbox_delivery_attempts(
            tenant_id,
            outbox_event_id,
            claim_version
        ),
    CHECK (
        outcome IN (
            'published',
            'retry_scheduled',
            'permanent_failure',
            'retry_exhausted',
            'lease_lost'
        )
    ),
    CHECK (error IS NULL OR (btrim(error) <> '' AND char_length(error) <= 4000)),
    CHECK (retry_after_seconds IS NULL OR retry_after_seconds >= 0),
    CHECK (
        (
            outcome = 'published'
            AND error IS NULL
            AND retry_after_seconds IS NULL
        )
        OR (
            outcome = 'retry_scheduled'
            AND error IS NOT NULL
            AND retry_after_seconds IS NOT NULL
        )
        OR (
            outcome IN ('permanent_failure', 'retry_exhausted')
            AND error IS NOT NULL
            AND retry_after_seconds IS NULL
        )
        OR (
            outcome = 'lease_lost'
            AND retry_after_seconds IS NULL
        )
    )
);

CREATE INDEX outbox_delivery_attempts_tenant_history_idx
    ON outbox_delivery_attempts(
        tenant_id,
        claimed_at,
        event_key,
        claim_version
    );

CREATE INDEX outbox_delivery_attempt_results_tenant_history_idx
    ON outbox_delivery_attempt_results(
        tenant_id,
        completed_at,
        outbox_event_id,
        claim_version
    );

ALTER TABLE outbox_delivery_attempts ENABLE ROW LEVEL SECURITY;
ALTER TABLE outbox_delivery_attempts FORCE ROW LEVEL SECURITY;

CREATE POLICY outbox_delivery_attempts_tenant_isolation
ON outbox_delivery_attempts
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

ALTER TABLE outbox_delivery_attempt_results ENABLE ROW LEVEL SECURITY;
ALTER TABLE outbox_delivery_attempt_results FORCE ROW LEVEL SECURITY;

CREATE POLICY outbox_delivery_attempt_results_tenant_isolation
ON outbox_delivery_attempt_results
USING (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
)
WITH CHECK (
    tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
);

CREATE OR REPLACE FUNCTION protect_outbox_delivery_attempt_history()
RETURNS trigger AS $$
BEGIN
    RAISE EXCEPTION 'outbox delivery attempt history is append-only'
        USING ERRCODE = '55000';
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER outbox_delivery_attempts_are_append_only
    BEFORE UPDATE OR DELETE ON outbox_delivery_attempts
    FOR EACH ROW EXECUTE FUNCTION protect_outbox_delivery_attempt_history();

CREATE TRIGGER outbox_delivery_attempt_results_are_append_only
    BEFORE UPDATE OR DELETE ON outbox_delivery_attempt_results
    FOR EACH ROW EXECUTE FUNCTION protect_outbox_delivery_attempt_history();

REVOKE ALL ON outbox_delivery_attempts FROM PUBLIC, wareboxes_app;
REVOKE ALL ON outbox_delivery_attempt_results FROM PUBLIC, wareboxes_app;

GRANT SELECT, INSERT ON outbox_delivery_attempts TO wareboxes_app;
GRANT SELECT, INSERT ON outbox_delivery_attempt_results TO wareboxes_app;
