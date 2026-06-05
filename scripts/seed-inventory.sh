#!/usr/bin/env bash
# Create visual test data for the Inventory screen.
set -euo pipefail
cd "$(dirname "$0")/.."

count=400
clear_seed=true

usage() {
  cat <<'USAGE'
Usage:
  scripts/seed-inventory.sh                 create 400 inventory balance rows
  scripts/seed-inventory.sh --count 1000    create a specific number of balances
  scripts/seed-inventory.sh --keep-existing keep existing WB-SEED-INV-* inventory

The script prefers DATABASE_URL with local psql. If DATABASE_URL is unset, it
uses docker compose exec against the postgres service.
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --count|-n)
      count="${2:-}"
      shift 2
      ;;
    --keep-existing)
      clear_seed=false
      shift
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

if ! [[ "$count" =~ ^[0-9]+$ ]] || [ "$count" -lt 1 ]; then
  echo "--count must be a positive integer" >&2
  exit 2
fi

run_psql() {
  if [ -n "${DATABASE_URL:-}" ] && command -v psql >/dev/null 2>&1; then
    psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -v inventory_count="$count" -v clear_seed="$clear_seed"
    return
  fi

  if ! command -v docker >/dev/null 2>&1; then
    echo "docker not found and DATABASE_URL+psql were not available." >&2
    exit 127
  fi
  if ! docker info >/dev/null 2>&1; then
    echo "docker is not available to this user. Start Docker and make sure your user can access /var/run/docker.sock." >&2
    echo "On Linux: sudo usermod -aG docker \"$USER\", then log out and back in." >&2
    exit 1
  fi
  docker compose exec -T postgres psql -U wareboxes -d wareboxes \
    -v ON_ERROR_STOP=1 -v inventory_count="$count" -v clear_seed="$clear_seed"
}

run_psql <<'SQL'
SELECT set_config('wareboxes.seed_inventory_count', :'inventory_count', false);
SELECT set_config('wareboxes.seed_clear_seed', :'clear_seed', false);

DO $$
DECLARE
  seed_count integer := current_setting('wareboxes.seed_inventory_count')::integer;
  clear_seed boolean := current_setting('wareboxes.seed_clear_seed')::boolean;
  warehouse_names text[] := ARRAY['Seed Warehouse', 'Seed Overflow Warehouse', 'Seed Returns DC'];
  item_names text[] := ARRAY[
    'Seed Canned Beans',
    'Seed Paper Towels',
    'Seed Sparkling Water',
    'Seed Dog Food',
    'Seed Protein Bars',
    'Seed Olive Oil',
    'Seed Pasta Sauce',
    'Seed Granola',
    'Seed Laundry Detergent',
    'Seed Breakfast Cereal',
    'Seed Printer Paper',
    'Seed Coffee Pods'
  ];
  statuses text[] := ARRAY['available', 'hold', 'damaged', 'quarantine'];
  location_templates text[][] := ARRAY[
    ARRAY['DOCK-01', 'Dock Door 1', 'dock', 'false', 'true'],
    ARRAY['RECV-01', 'Receiving Staging', 'staging', 'false', 'true'],
    ARRAY['STAGE-01', 'Outbound Staging', 'staging', 'false', 'true'],
    ARRAY['A-01-01', 'Aisle A Bay 01', 'rack', 'true', 'false'],
    ARRAY['A-01-02', 'Aisle A Bay 02', 'rack', 'true', 'false'],
    ARRAY['A-02-01', 'Aisle A Bay 03', 'rack', 'true', 'false'],
    ARRAY['B-01-01', 'Aisle B Bay 01', 'rack', 'true', 'false'],
    ARRAY['B-01-02', 'Aisle B Bay 02', 'rack', 'true', 'false'],
    ARRAY['PICK-01', 'Forward Pick 01', 'pick', 'true', 'false'],
    ARRAY['PICK-02', 'Forward Pick 02', 'pick', 'true', 'false'],
    ARRAY['QC-01', 'Quality Hold', 'hold', 'false', 'false'],
    ARRAY['DAM-01', 'Damage Cage', 'damage', 'false', 'false']
  ];
  v_wh_id bigint;
  v_wh_ids bigint[] := ARRAY[]::bigint[];
  v_item_id bigint;
  v_item_ids bigint[] := ARRAY[]::bigint[];
  v_loc_id bigint;
  v_loc_ids bigint[] := ARRAY[]::bigint[];
  v_batch_id bigint;
  v_lp_id bigint;
  v_user_id bigint;
  v_loc_wh_id bigint;
  v_loc_index integer;
  v_item_index integer;
  v_status text;
  v_qty bigint;
  v_reserved bigint;
  v_lot text;
  v_expiration timestamptz;
  v_barcode text;
  v_created timestamptz;
  i integer;
BEGIN
  IF clear_seed THEN
    DELETE FROM inventory_reservations ir
    WHERE ir.item_batch_id IN (SELECT ib.id FROM item_batches ib WHERE ib.lot LIKE 'WB-SEED-INV-%');

    DELETE FROM stock_movements sm
    WHERE sm.idempotency_key LIKE 'WB-SEED-INV-%'
       OR sm.item_batch_id IN (SELECT ib.id FROM item_batches ib WHERE ib.lot LIKE 'WB-SEED-INV-%');

    DELETE FROM inventory_balances ibal
    WHERE ibal.item_batch_id IN (SELECT ib.id FROM item_batches ib WHERE ib.lot LIKE 'WB-SEED-INV-%')
       OR ibal.license_plate_id IN (SELECT lp.id FROM license_plates lp WHERE lp.barcode LIKE 'WB-SEED-LP-%');

    DELETE FROM license_plates lp WHERE lp.barcode LIKE 'WB-SEED-LP-%';
    DELETE FROM item_batches ib WHERE ib.lot LIKE 'WB-SEED-INV-%';
  END IF;

  INSERT INTO users (created, first_name, last_name, email)
  VALUES (now(), 'Seed', 'Inventory', 'seed.inventory@example.test')
  ON CONFLICT (email) DO UPDATE SET first_name = EXCLUDED.first_name
  RETURNING id INTO v_user_id;

  FOR i IN 1..array_length(warehouse_names, 1) LOOP
    SELECT id INTO v_wh_id FROM warehouses WHERE name = warehouse_names[i] ORDER BY id LIMIT 1;
    IF v_wh_id IS NULL THEN
      INSERT INTO warehouses (created, name)
      VALUES (now(), warehouse_names[i])
      RETURNING id INTO v_wh_id;
    END IF;
    v_wh_ids := array_append(v_wh_ids, v_wh_id);

    FOR v_loc_index IN 1..array_length(location_templates, 1) LOOP
      v_barcode := 'WB-SEED-' || upper(replace(warehouse_names[i], ' ', '-')) || '-' || location_templates[v_loc_index][1];
      INSERT INTO locations (
        created, warehouse_id, barcode, name, type, active, pickable, receivable
      ) VALUES (
        now(),
        v_wh_id,
        v_barcode,
        warehouse_names[i] || ' ' || location_templates[v_loc_index][2],
        location_templates[v_loc_index][3],
        true,
        location_templates[v_loc_index][4]::boolean,
        location_templates[v_loc_index][5]::boolean
      )
      ON CONFLICT (barcode) DO UPDATE
      SET name = EXCLUDED.name,
          type = EXCLUDED.type,
          active = true,
          pickable = EXCLUDED.pickable,
          receivable = EXCLUDED.receivable
      RETURNING id INTO v_loc_id;
      v_loc_ids := array_append(v_loc_ids, v_loc_id);
    END LOOP;
  END LOOP;

  FOR i IN 1..array_length(item_names, 1) LOOP
    SELECT id INTO v_item_id FROM items WHERE description = item_names[i] ORDER BY id LIMIT 1;
    IF v_item_id IS NULL THEN
      INSERT INTO items (created, description, notes, packaging_unit, pallet_hi, pallet_ti, inner_units)
      VALUES (now(), item_names[i], 'Seed inventory visualization item', CASE WHEN i % 4 = 0 THEN 'each' ELSE 'case' END, 5, 8, 12)
      RETURNING id INTO v_item_id;
    END IF;
    v_item_ids := array_append(v_item_ids, v_item_id);
  END LOOP;

  FOR i IN 1..seed_count LOOP
    v_item_index := ((i - 1) % array_length(v_item_ids, 1)) + 1;
    v_loc_index := ((i * 7 - 1) % array_length(v_loc_ids, 1)) + 1;
    SELECT warehouse_id INTO v_loc_wh_id FROM locations WHERE id = v_loc_ids[v_loc_index];
    v_status := statuses[((i - 1) % array_length(statuses, 1)) + 1];
    v_qty := 12 + ((i * 17) % 240);
    v_reserved := CASE
      WHEN v_status = 'available' AND i % 5 = 0 THEN least(v_qty - 1, 1 + ((i * 3) % 24))
      ELSE 0
    END;
    v_lot := 'WB-SEED-INV-ITEM-' || lpad(v_item_index::text, 2, '0') || '-LOC-' || lpad(v_loc_index::text, 3, '0');
    v_expiration := CASE
      WHEN v_item_index % 6 = 0 THEN now() + ((30 + v_loc_index) || ' days')::interval
      ELSE NULL
    END;
    v_created := now() - ((i % 45) || ' days')::interval;

    INSERT INTO item_batches (created, item_id, lot, expiration)
    VALUES (
      v_created,
      v_item_ids[v_item_index],
      v_lot,
      v_expiration
    )
    RETURNING id INTO v_batch_id;

    IF i % 8 = 0 THEN
      v_barcode := 'WB-SEED-LP-' || lpad(i::text, greatest(length(i::text), 6), '0');
      INSERT INTO license_plates (created, barcode, location_id)
      VALUES (v_created, v_barcode, v_loc_ids[v_loc_index])
      RETURNING id INTO v_lp_id;
    ELSE
      v_lp_id := NULL;
    END IF;

    INSERT INTO inventory_balances (
      created, modified, warehouse_id, location_id, license_plate_id, item_batch_id,
      status, qty_on_hand, qty_reserved
    ) VALUES (
      v_created,
      now(),
      v_loc_wh_id,
      v_loc_ids[v_loc_index],
      v_lp_id,
      v_batch_id,
      v_status,
      v_qty,
      v_reserved
    );

    INSERT INTO stock_movements (
      created, user_id, item_batch_id, license_plate_id, from_location_id, to_location_id,
      qty, movement_type, status, reason, reference_type, reference_id, idempotency_key
    ) VALUES (
      v_created,
      v_user_id,
      v_batch_id,
      v_lp_id,
      NULL,
      v_loc_ids[v_loc_index],
      v_qty,
      'receive',
      v_status,
      'seed inventory receive',
      'seed',
      v_batch_id,
      'WB-SEED-INV-RECEIVE-' || v_batch_id
    );
  END LOOP;

  RAISE NOTICE 'Seeded % inventory balances across % warehouses and % locations.',
    seed_count, array_length(v_wh_ids, 1), array_length(v_loc_ids, 1);
END $$;
SQL
