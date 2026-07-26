CREATE OR REPLACE FUNCTION inventory_reservation_matches_balance()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
DECLARE
    balance_item_batch_id BIGINT;
    balance_location_id BIGINT;
    balance_facility_id BIGINT;
    order_inventory_owner_id BIGINT;
BEGIN
    SELECT item_batch_id, location_id, facility_id
    INTO balance_item_batch_id, balance_location_id, balance_facility_id
    FROM public.inventory_balances
    WHERE tenant_id = NEW.tenant_id
      AND inventory_owner_id = NEW.inventory_owner_id
      AND id = NEW.inventory_balance_id
    FOR SHARE;

    IF balance_item_batch_id IS NULL THEN
        RAISE EXCEPTION 'reservation inventory balance does not exist'
            USING ERRCODE = '23503';
    END IF;

    IF NEW.item_batch_id IS DISTINCT FROM balance_item_batch_id
       OR NEW.location_id IS DISTINCT FROM balance_location_id
       OR NEW.facility_id IS DISTINCT FROM balance_facility_id
    THEN
        RAISE EXCEPTION
            'reservation item batch/location must match inventory balance'
            USING ERRCODE = '23514';
    END IF;

    SELECT inventory_owner_id
    INTO order_inventory_owner_id
    FROM public.orders
    WHERE tenant_id = NEW.tenant_id
      AND id = NEW.order_id
      AND deleted IS NULL;

    IF order_inventory_owner_id IS NULL
       OR NEW.inventory_owner_id IS DISTINCT FROM order_inventory_owner_id
    THEN
        RAISE EXCEPTION
            'reservation order must match the inventory owner scope'
            USING ERRCODE = '23514';
    END IF;

    IF NEW.order_item_id IS NOT NULL AND NOT EXISTS (
        SELECT 1
        FROM public.order_items order_line
        INNER JOIN public.item_batches batch
            ON batch.tenant_id = NEW.tenant_id
           AND batch.inventory_owner_id = NEW.inventory_owner_id
           AND batch.id = NEW.item_batch_id
        WHERE order_line.id = NEW.order_item_id
          AND order_line.tenant_id = NEW.tenant_id
          AND order_line.inventory_owner_id = NEW.inventory_owner_id
          AND order_line.order_id = NEW.order_id
          AND order_line.item_id = batch.item_id
          AND order_line.deleted IS NULL
    ) THEN
        RAISE EXCEPTION
            'reservation order line must match the order and inventory item'
            USING ERRCODE = '23514';
    END IF;

    RETURN NEW;
END;
$$;

CREATE FUNCTION guard_active_reservation_balance_dimensions()
RETURNS trigger
LANGUAGE plpgsql
SET search_path = pg_catalog, public
AS $$
BEGIN
    IF NEW.tenant_id IS NOT DISTINCT FROM OLD.tenant_id
       AND NEW.inventory_owner_id IS NOT DISTINCT FROM OLD.inventory_owner_id
       AND NEW.facility_id IS NOT DISTINCT FROM OLD.facility_id
       AND NEW.location_id IS NOT DISTINCT FROM OLD.location_id
       AND NEW.item_batch_id IS NOT DISTINCT FROM OLD.item_batch_id
    THEN
        RETURN NEW;
    END IF;

    PERFORM 1
    FROM public.inventory_reservations reservation
    WHERE reservation.tenant_id = OLD.tenant_id
      AND reservation.inventory_owner_id = OLD.inventory_owner_id
      AND reservation.inventory_balance_id = OLD.id
      AND reservation.deleted IS NULL
      AND reservation.status = 'reserved'
    LIMIT 1;

    IF FOUND THEN
        RAISE EXCEPTION
            'inventory balance dimensions cannot change while active reservations reference it'
            USING ERRCODE = '55000';
    END IF;

    RETURN NEW;
END;
$$;

CREATE TRIGGER inventory_balances_guard_active_reservation_dimensions
    BEFORE UPDATE OF tenant_id, inventory_owner_id, facility_id, location_id,
        item_batch_id
    ON inventory_balances
    FOR EACH ROW
    EXECUTE FUNCTION guard_active_reservation_balance_dimensions();

CREATE CONSTRAINT TRIGGER inventory_balances_verify_active_reservation_dimensions
    AFTER UPDATE
    ON inventory_balances
    DEFERRABLE INITIALLY DEFERRED
    FOR EACH ROW
    EXECUTE FUNCTION guard_active_reservation_balance_dimensions();

REVOKE ALL ON FUNCTION
    inventory_reservation_matches_balance(),
    guard_active_reservation_balance_dimensions()
FROM PUBLIC, wareboxes_app;
