-- A location may not hold the same item across mixed lot/expiration groups.
-- Operators can still combine multiple batches when lot and expiration match.

CREATE OR REPLACE FUNCTION prevent_mixed_item_lot_expiration_per_location()
RETURNS trigger AS $$
DECLARE
    incoming_item_id BIGINT;
BEGIN
    IF NEW.deleted IS NULL AND NEW.qty_on_hand > 0 THEN
        IF TG_OP <> 'INSERT' THEN
            IF OLD.location_id IS NOT DISTINCT FROM NEW.location_id
               AND OLD.item_batch_id IS NOT DISTINCT FROM NEW.item_batch_id
               AND OLD.deleted IS NOT DISTINCT FROM NEW.deleted
               AND NEW.qty_on_hand <= OLD.qty_on_hand
            THEN
                RETURN NEW;
            END IF;
        END IF;

        SELECT item_id INTO incoming_item_id
        FROM item_batches
        WHERE id = NEW.item_batch_id;

        PERFORM pg_advisory_xact_lock(hashtextextended(
            'inventory-location-item:' || NEW.location_id::text || ':' || incoming_item_id::text,
            0
        ));

        IF EXISTS (
            SELECT 1
            FROM inventory_balances ib
            INNER JOIN item_batches existing_batch ON existing_batch.id = ib.item_batch_id
            INNER JOIN item_batches incoming_batch ON incoming_batch.id = NEW.item_batch_id
            WHERE ib.id <> COALESCE(NEW.id, -1)
              AND ib.location_id = NEW.location_id
              AND ib.deleted IS NULL
              AND ib.qty_on_hand > 0
              AND existing_batch.item_id = incoming_item_id
              AND (
                  existing_batch.lot IS DISTINCT FROM incoming_batch.lot
                  OR existing_batch.expiration IS DISTINCT FROM incoming_batch.expiration
              )
            LIMIT 1
        ) THEN
            RAISE EXCEPTION 'location already contains this item with a different lot or expiration'
                USING ERRCODE = '23514';
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS inventory_balances_prevent_mixed_item_lot_expiration
    ON inventory_balances;

CREATE TRIGGER inventory_balances_prevent_mixed_item_lot_expiration
    BEFORE INSERT OR UPDATE OF location_id, item_batch_id, qty_on_hand, deleted
    ON inventory_balances
    FOR EACH ROW
    EXECUTE FUNCTION prevent_mixed_item_lot_expiration_per_location();
