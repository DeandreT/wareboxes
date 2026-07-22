#!/usr/bin/env bash
# Create tenant-scoped inventory journal and balance data for operational testing.
set -euo pipefail
cd "$(dirname "$0")/.."

count=400

usage() {
  cat <<'USAGE'
Usage:
  scripts/seed-inventory.sh                 create up to 400 inventory positions
  scripts/seed-inventory.sh --count 1000    create up to a specific number of positions

The target database must contain at least one active tenant. The script uses the
oldest active tenant and is replay-safe: existing WB-SEED-INV-* commands are left
unchanged. Set DATABASE_URL to use local psql; otherwise Docker Compose is used.
USAGE
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --count|-n)
      count="${2:-}"
      shift 2
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
    psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -v inventory_count="$count"
    return
  fi

  if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    echo "DATABASE_URL+psql or an available Docker daemon is required." >&2
    exit 1
  fi
  docker compose exec -T postgres psql -U wareboxes -d wareboxes \
    -v ON_ERROR_STOP=1 -v inventory_count="$count"
}

run_psql <<'SQL'
SELECT set_config('wareboxes.seed_inventory_count', :'inventory_count', false);

DO $$
DECLARE
  seed_count integer := current_setting('wareboxes.seed_inventory_count')::integer;
  tenant bigint;
  owner bigint;
  facility bigint;
  actor bigint;
  item bigint;
  location bigint;
  batch bigint;
  plate bigint;
  transaction bigint;
  item_ids bigint[] := ARRAY[]::bigint[];
  location_ids bigint[] := ARRAY[]::bigint[];
  item_names text[] := ARRAY[
    'Seed Canned Beans', 'Seed Paper Towels', 'Seed Sparkling Water',
    'Seed Pet Food', 'Seed Protein Bars', 'Seed Olive Oil',
    'Seed Pasta Sauce', 'Seed Granola', 'Seed Laundry Detergent',
    'Seed Breakfast Cereal', 'Seed Printer Paper', 'Seed Coffee Pods'
  ];
  location_codes text[] := ARRAY[
    'SEED-RECV-01', 'SEED-A-01-01', 'SEED-A-01-02', 'SEED-A-02-01',
    'SEED-B-01-01', 'SEED-B-01-02', 'SEED-PICK-01', 'SEED-PICK-02',
    'SEED-QC-01', 'SEED-DAMAGE-01'
  ];
  statuses text[] := ARRAY['available', 'hold', 'damaged', 'quarantine'];
  position_no text;
  item_index integer;
  location_index integer;
  quantity bigint;
  inventory_status text;
  i integer;
BEGIN
  SELECT id INTO tenant
  FROM tenants
  WHERE deleted IS NULL AND status = 'active'
  ORDER BY id
  LIMIT 1;
  IF tenant IS NULL THEN
    RAISE EXCEPTION 'seed-inventory requires at least one active tenant';
  END IF;

  SELECT user_id INTO actor
  FROM tenant_memberships
  WHERE tenant_id = tenant AND deleted IS NULL
  ORDER BY is_default DESC, id
  LIMIT 1;

  INSERT INTO inventory_owners (tenant_id, created, name, email)
  VALUES (tenant, now(), 'Seed Inventory Owner', 'seed.inventory.owner@example.test')
  ON CONFLICT (tenant_id, name) DO UPDATE SET deleted = NULL, email = EXCLUDED.email
  RETURNING id INTO owner;

  INSERT INTO facilities (tenant_id, created, name)
  VALUES (tenant, now(), 'Seed Distribution Center')
  ON CONFLICT (tenant_id, name) DO UPDATE SET deleted = NULL
  RETURNING id INTO facility;

  INSERT INTO inventory_owner_facilities
      (tenant_id, created, inventory_owner_id, facility_id)
  VALUES (tenant, now(), owner, facility)
  ON CONFLICT (tenant_id, inventory_owner_id, facility_id)
  DO UPDATE SET deleted = NULL;

  FOREACH inventory_status IN ARRAY location_codes LOOP
    INSERT INTO locations
        (tenant_id, created, facility_id, barcode, name, type, active, pickable, receivable)
    VALUES (
      tenant, now(), facility, inventory_status, inventory_status,
      CASE
        WHEN inventory_status LIKE '%RECV%' THEN 'staging'
        WHEN inventory_status LIKE '%QC%' THEN 'hold'
        WHEN inventory_status LIKE '%DAMAGE%' THEN 'damage'
        WHEN inventory_status LIKE '%PICK%' THEN 'pick'
        ELSE 'rack'
      END,
      true,
      inventory_status NOT LIKE '%RECV%' AND inventory_status NOT LIKE '%QC%' AND inventory_status NOT LIKE '%DAMAGE%',
      inventory_status LIKE '%RECV%'
    )
    ON CONFLICT (tenant_id, barcode) DO UPDATE
    SET deleted = NULL, active = true, name = EXCLUDED.name
    RETURNING id INTO location;
    location_ids := array_append(location_ids, location);
  END LOOP;

  FOREACH inventory_status IN ARRAY item_names LOOP
    SELECT id INTO item
    FROM items
    WHERE tenant_id = tenant AND description = inventory_status AND deleted IS NULL
    ORDER BY id
    LIMIT 1;
    IF item IS NULL THEN
      INSERT INTO items
          (tenant_id, created, description, notes, packaging_unit, pallet_hi, pallet_ti, inner_units)
      VALUES (tenant, now(), inventory_status, 'Inventory seed item', 'case', 5, 8, 12)
      RETURNING id INTO item;
    END IF;
    INSERT INTO inventory_owner_items
        (tenant_id, created, inventory_owner_id, item_id)
    VALUES (tenant, now(), owner, item)
    ON CONFLICT (tenant_id, inventory_owner_id, item_id) DO UPDATE SET deleted = NULL;
    item_ids := array_append(item_ids, item);
  END LOOP;

  FOR i IN 1..seed_count LOOP
    position_no := lpad(i::text, greatest(length(i::text), 6), '0');
    IF EXISTS (
      SELECT 1 FROM command_idempotency_records
      WHERE tenant_id = tenant
        AND operation = 'seed_inventory'
        AND idempotency_key = 'WB-SEED-INV-' || position_no
    ) THEN
      CONTINUE;
    END IF;

    item_index := ((i - 1) % array_length(item_ids, 1)) + 1;
    location_index := ((i * 7 - 1) % array_length(location_ids, 1)) + 1;
    item := item_ids[item_index];
    location := location_ids[location_index];
    inventory_status := statuses[((i - 1) % array_length(statuses, 1)) + 1];
    quantity := 12 + ((i * 17) % 240);

    INSERT INTO item_batches
        (tenant_id, inventory_owner_id, created, item_id, uom, lot, expiration)
    VALUES (
      tenant, owner, now() - ((i % 45) || ' days')::interval, item, 'case',
      'WB-SEED-INV-' || position_no,
      CASE WHEN item_index % 6 = 0 THEN now() + ((30 + location_index) || ' days')::interval END
    )
    RETURNING id INTO batch;

    plate := NULL;
    IF i % 8 = 0 THEN
      INSERT INTO license_plates
          (tenant_id, inventory_owner_id, created, barcode, location_id)
      VALUES (tenant, owner, now(), 'WB-SEED-LP-' || position_no, location)
      ON CONFLICT (tenant_id, barcode) DO UPDATE
      SET deleted = NULL, location_id = EXCLUDED.location_id
      RETURNING id INTO plate;
    END IF;

    INSERT INTO inventory_transactions
        (tenant_id, inventory_owner_id, created, actor_user_id, transaction_type,
         reason, reference_type, reference_id, operation, idempotency_key, request_hash)
    VALUES (
      tenant, owner, now(), actor, 'receive', 'seed inventory receive',
      'seed', batch, 'seed_inventory', 'WB-SEED-INV-' || position_no,
      md5('WB-SEED-INV-' || position_no)
    )
    RETURNING id INTO transaction;

    INSERT INTO command_idempotency_records
        (tenant_id, created, operation, idempotency_key, request_hash,
         result_json, inventory_transaction_id)
    VALUES (
      tenant, now(), 'seed_inventory', 'WB-SEED-INV-' || position_no,
      md5('WB-SEED-INV-' || position_no), to_jsonb(transaction), transaction
    );

    INSERT INTO inventory_entries
        (tenant_id, inventory_owner_id, transaction_id, created, facility_id,
         location_id, license_plate_id, item_batch_id, item_id, uom, lot,
         expiration, serial, status, quantity_delta)
    SELECT tenant, owner, transaction, now(), facility, location, plate, batch,
           b.item_id, b.uom, b.lot, b.expiration, b.serial, inventory_status, quantity
    FROM item_batches b WHERE b.id = batch;

    IF plate IS NULL THEN
      INSERT INTO inventory_balances
          (tenant_id, inventory_owner_id, created, modified, facility_id, location_id,
           license_plate_id, item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved)
      VALUES (
        tenant, owner, now(), now(), facility, location, NULL, batch, item, 'case',
        inventory_status, quantity, 0
      );
    ELSE
      INSERT INTO inventory_balances
          (tenant_id, inventory_owner_id, created, modified, facility_id, location_id,
           license_plate_id, item_batch_id, item_id, uom, status, qty_on_hand, qty_reserved)
      VALUES (
        tenant, owner, now(), now(), facility, location, plate, batch, item, 'case',
        inventory_status, quantity, 0
      );
    END IF;
  END LOOP;

  RAISE NOTICE 'Seed inventory now contains up to % positions for tenant %.', seed_count, tenant;
END $$;
SQL
