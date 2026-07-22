CREATE INDEX idx_inventory_reservations_balance
    ON inventory_reservations(tenant_id, inventory_owner_id, inventory_balance_id)
    WHERE deleted IS NULL;

CREATE OR REPLACE FUNCTION inventory_reservation_matches_balance()
RETURNS trigger AS $$
DECLARE
    balance_item_batch_id BIGINT;
    balance_location_id BIGINT;
    balance_facility_id BIGINT;
    order_inventory_owner_id BIGINT;
BEGIN
    SELECT item_batch_id, location_id, facility_id
    INTO balance_item_batch_id, balance_location_id, balance_facility_id
    FROM inventory_balances
    WHERE tenant_id = NEW.tenant_id
      AND inventory_owner_id = NEW.inventory_owner_id
      AND id = NEW.inventory_balance_id;

    IF balance_item_batch_id IS NULL THEN
        RAISE EXCEPTION 'reservation inventory balance does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.item_batch_id IS DISTINCT FROM balance_item_batch_id
       OR NEW.location_id IS DISTINCT FROM balance_location_id
       OR NEW.facility_id IS DISTINCT FROM balance_facility_id
    THEN
        RAISE EXCEPTION 'reservation item batch/location must match inventory balance'
            USING ERRCODE = '23514';
    END IF;

    SELECT inventory_owner_id INTO order_inventory_owner_id
    FROM orders
    WHERE id = NEW.order_id AND deleted IS NULL;

    IF order_inventory_owner_id IS NULL
       OR NEW.inventory_owner_id IS DISTINCT FROM order_inventory_owner_id THEN
        RAISE EXCEPTION 'reservation order must match the inventory owner scope'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.order_item_id IS NOT NULL AND NOT EXISTS (
        SELECT 1
        FROM order_items order_line
        INNER JOIN item_batches batch
            ON batch.tenant_id = NEW.tenant_id
           AND batch.inventory_owner_id = NEW.inventory_owner_id
           AND batch.id = NEW.item_batch_id
        WHERE order_line.id = NEW.order_item_id
          AND order_line.order_id = NEW.order_id
          AND order_line.item_id = batch.item_id
          AND order_line.deleted IS NULL
    ) THEN
        RAISE EXCEPTION 'reservation order line must match the order and inventory item'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

CREATE TRIGGER inventory_reservations_match_balance
    BEFORE INSERT OR UPDATE OF tenant_id, inventory_owner_id, order_id, order_item_id, inventory_balance_id, facility_id, item_batch_id, location_id
    ON inventory_reservations
    FOR EACH ROW
    EXECUTE FUNCTION inventory_reservation_matches_balance();
