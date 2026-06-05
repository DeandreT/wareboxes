-- Reservations should target one exact balance row. The denormalized
-- item_batch_id/location_id columns remain for reporting and simple joins.

ALTER TABLE inventory_reservations
    ADD COLUMN inventory_balance_id BIGINT REFERENCES inventory_balances(id);

UPDATE inventory_reservations r
SET inventory_balance_id = (
    SELECT ib.id
    FROM inventory_balances ib
    WHERE ib.item_batch_id = r.item_batch_id
      AND ib.location_id = r.location_id
      AND ib.status = 'available'
      AND ib.deleted IS NULL
    ORDER BY CASE WHEN ib.license_plate_id IS NULL THEN 0 ELSE 1 END, ib.id
    LIMIT 1
)
WHERE r.inventory_balance_id IS NULL;

ALTER TABLE inventory_reservations
    ALTER COLUMN inventory_balance_id SET NOT NULL;

CREATE INDEX idx_inventory_reservations_balance
    ON inventory_reservations(inventory_balance_id)
    WHERE deleted IS NULL;

CREATE OR REPLACE FUNCTION inventory_reservation_matches_balance()
RETURNS trigger AS $$
DECLARE
    balance_item_batch_id BIGINT;
    balance_location_id BIGINT;
BEGIN
    SELECT item_batch_id, location_id
    INTO balance_item_batch_id, balance_location_id
    FROM inventory_balances
    WHERE id = NEW.inventory_balance_id;

    IF balance_item_batch_id IS NULL THEN
        RAISE EXCEPTION 'reservation inventory balance does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.item_batch_id IS DISTINCT FROM balance_item_batch_id
       OR NEW.location_id IS DISTINCT FROM balance_location_id
    THEN
        RAISE EXCEPTION 'reservation item batch/location must match inventory balance'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER inventory_reservations_match_balance
    BEFORE INSERT OR UPDATE OF inventory_balance_id, item_batch_id, location_id
    ON inventory_reservations
    FOR EACH ROW
    EXECUTE FUNCTION inventory_reservation_matches_balance();
