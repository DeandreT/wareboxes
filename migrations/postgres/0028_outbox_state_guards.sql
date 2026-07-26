CREATE OR REPLACE FUNCTION protect_outbox_event_key_state()
RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'outbox event keys are permanent'
            USING ERRCODE = '55000';
    END IF;

    IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
        OR NEW.event_key IS DISTINCT FROM OLD.event_key
        OR NEW.created IS DISTINCT FROM OLD.created
    THEN
        RAISE EXCEPTION 'outbox event keys are immutable'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER outbox_event_keys_are_permanent
    BEFORE UPDATE OR DELETE ON outbox_event_keys
    FOR EACH ROW EXECUTE FUNCTION protect_outbox_event_key_state();

CREATE OR REPLACE FUNCTION protect_outbox_aggregate_sequence_state()
RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'outbox aggregate sequences are permanent'
            USING ERRCODE = '55000';
    END IF;

    IF NEW.tenant_id IS DISTINCT FROM OLD.tenant_id
        OR NEW.ordering_key IS DISTINCT FROM OLD.ordering_key
        OR NEW.last_sequence <> OLD.last_sequence + 1
        OR NEW.updated < OLD.updated
    THEN
        RAISE EXCEPTION 'outbox aggregate sequences must advance by one'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER outbox_aggregate_sequences_are_monotonic
    BEFORE UPDATE OR DELETE ON outbox_aggregate_sequences
    FOR EACH ROW EXECUTE FUNCTION protect_outbox_aggregate_sequence_state();

CREATE OR REPLACE FUNCTION protect_unpublished_outbox_event_deletion()
RETURNS trigger AS $$
BEGIN
    IF OLD.published_at IS NULL AND OLD.discarded_at IS NULL THEN
        RAISE EXCEPTION 'only published or discarded outbox events may be deleted'
            USING ERRCODE = '55000';
    END IF;

    RETURN OLD;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER unpublished_outbox_events_cannot_be_deleted
    BEFORE DELETE ON outbox_events
    FOR EACH ROW EXECUTE FUNCTION protect_unpublished_outbox_event_deletion();
