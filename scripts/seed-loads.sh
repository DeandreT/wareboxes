#!/usr/bin/env bash
# Create tenant-scoped load data for operational and pagination testing.
set -euo pipefail
cd "$(dirname "$0")/.."

count=40
clear_seed=true

usage() {
  cat <<'USAGE'
Usage:
  scripts/seed-loads.sh                 create 40 loads
  scripts/seed-loads.sh --count 1000    create a specific number of loads
  scripts/seed-loads.sh --keep-existing retain existing WB-SEED-LOAD-* loads

The target database must contain at least one active tenant. The script uses the
oldest active tenant. Set DATABASE_URL to use local psql; otherwise Docker Compose
is used.
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
    psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -v load_count="$count" -v clear_seed="$clear_seed"
    return
  fi

  if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    echo "DATABASE_URL+psql or an available Docker daemon is required." >&2
    exit 1
  fi
  docker compose exec -T postgres psql -U wareboxes -d wareboxes \
    -v ON_ERROR_STOP=1 -v load_count="$count" -v clear_seed="$clear_seed"
}

run_psql <<'SQL'
SELECT set_config('wareboxes.seed_load_count', :'load_count', false);
SELECT set_config('wareboxes.seed_clear_loads', :'clear_seed', false);

DO $$
DECLARE
  seed_count integer := current_setting('wareboxes.seed_load_count')::integer;
  clear_seed boolean := current_setting('wareboxes.seed_clear_loads')::boolean;
  tenant bigint;
  facility bigint;
  dock bigint;
  actor bigint;
  owner bigint;
  item bigint;
  v_load_id bigint;
  owner_ids bigint[] := ARRAY[]::bigint[];
  item_ids bigint[] := ARRAY[]::bigint[];
  owner_names text[] := ARRAY[
    'Seed Grocery Owner', 'Seed Retail Owner', 'Seed Wholesale Owner', 'Seed Food Owner'
  ];
  item_names text[] := ARRAY[
    'Seed Canned Beans', 'Seed Paper Towels', 'Seed Sparkling Water',
    'Seed Pet Food', 'Seed Protein Bars', 'Seed Olive Oil',
    'Seed Pasta Sauce', 'Seed Granola'
  ];
  statuses text[] := ARRAY[
    'planned', 'scheduled', 'arrived', 'receiving',
    'received', 'rejected', 'closed', 'cancelled'
  ];
  load_status text;
  load_type text;
  created_at timestamptz;
  expected_qty bigint;
  received_qty bigint;
  rejected_qty bigint;
  missing_qty bigint;
  line_status text;
  seed_no text;
  i integer;
  j integer;
BEGIN
  SELECT id INTO tenant
  FROM tenants
  WHERE deleted IS NULL AND status = 'active'
  ORDER BY id
  LIMIT 1;
  IF tenant IS NULL THEN
    RAISE EXCEPTION 'seed-loads requires at least one active tenant';
  END IF;

  SELECT user_id INTO actor
  FROM tenant_memberships
  WHERE tenant_id = tenant AND deleted IS NULL
  ORDER BY is_default DESC, id
  LIMIT 1;

  IF clear_seed THEN
    DELETE FROM load_orders
    WHERE tenant_id = tenant AND load_id IN (
      SELECT id FROM loads WHERE tenant_id = tenant AND reference_number LIKE 'WB-SEED-LOAD-%'
    );
    DELETE FROM load_activity
    WHERE tenant_id = tenant AND load_id IN (
      SELECT id FROM loads WHERE tenant_id = tenant AND reference_number LIKE 'WB-SEED-LOAD-%'
    );
    DELETE FROM load_files
    WHERE tenant_id = tenant AND load_id IN (
      SELECT id FROM loads WHERE tenant_id = tenant AND reference_number LIKE 'WB-SEED-LOAD-%'
    );
    DELETE FROM load_notes
    WHERE tenant_id = tenant AND load_id IN (
      SELECT id FROM loads WHERE tenant_id = tenant AND reference_number LIKE 'WB-SEED-LOAD-%'
    );
    DELETE FROM load_lines
    WHERE tenant_id = tenant AND load_id IN (
      SELECT id FROM loads WHERE tenant_id = tenant AND reference_number LIKE 'WB-SEED-LOAD-%'
    );
    DELETE FROM loads WHERE tenant_id = tenant AND reference_number LIKE 'WB-SEED-LOAD-%';
  END IF;

  INSERT INTO facilities (tenant_id, created, name)
  VALUES (tenant, now(), 'Seed Distribution Center')
  ON CONFLICT (tenant_id, name) DO UPDATE SET deleted = NULL
  RETURNING id INTO facility;

  INSERT INTO locations
      (tenant_id, created, facility_id, barcode, name, type, active, pickable, receivable)
  VALUES (tenant, now(), facility, 'SEED-DOCK-01', 'Seed Dock 01', 'dock', true, false, true)
  ON CONFLICT (tenant_id, barcode) DO UPDATE
  SET deleted = NULL, active = true, receivable = true
  RETURNING id INTO dock;

  FOREACH load_status IN ARRAY owner_names LOOP
    INSERT INTO inventory_owners (tenant_id, created, name, email)
    VALUES (
      tenant, now(), load_status,
      lower(replace(load_status, ' ', '.')) || '@example.test'
    )
    ON CONFLICT (tenant_id, name) DO UPDATE SET deleted = NULL, email = EXCLUDED.email
    RETURNING id INTO owner;
    INSERT INTO inventory_owner_facilities
        (tenant_id, created, inventory_owner_id, facility_id)
    VALUES (tenant, now(), owner, facility)
    ON CONFLICT (tenant_id, inventory_owner_id, facility_id) DO UPDATE SET deleted = NULL;
    owner_ids := array_append(owner_ids, owner);
  END LOOP;

  FOREACH load_status IN ARRAY item_names LOOP
    SELECT id INTO item
    FROM items
    WHERE tenant_id = tenant AND description = load_status AND deleted IS NULL
    ORDER BY id
    LIMIT 1;
    IF item IS NULL THEN
      INSERT INTO items
          (tenant_id, created, description, notes, packaging_unit, pallet_hi, pallet_ti, inner_units)
      VALUES (tenant, now(), load_status, 'Load seed item', 'case', 5, 8, 12)
      RETURNING id INTO item;
    END IF;
    item_ids := array_append(item_ids, item);
  END LOOP;

  FOR i IN 1..seed_count LOOP
    seed_no := lpad(i::text, greatest(length(i::text), 6), '0');
    IF EXISTS (
      SELECT 1 FROM loads
      WHERE tenant_id = tenant AND reference_number = 'WB-SEED-LOAD-' || seed_no
    ) THEN
      CONTINUE;
    END IF;

    load_status := statuses[((i - 1) % array_length(statuses, 1)) + 1];
    load_type := CASE WHEN i % 6 = 0 THEN 'outbound' ELSE 'inbound' END;
    created_at := now() - ((i % 90) || ' days')::interval;

    INSERT INTO loads
        (tenant_id, created, facility_id, inventory_owner_id, status, type, reference_number,
         invoice_number, carrier, trailer_number, seal_number, dock_door_location_id,
         expected_time, appointment_time, actual_time, arrival, rejected,
         receive_completed, closed, checked_in_by, closed_by)
    VALUES (
      tenant,
      created_at,
      facility,
      owner_ids[((i - 1) % array_length(owner_ids, 1)) + 1],
      load_status,
      load_type,
      'WB-SEED-LOAD-' || seed_no,
      'WB-SEED-INVOICE-' || seed_no,
      CASE (i % 4) WHEN 0 THEN 'North Freight' WHEN 1 THEN 'West Freight' WHEN 2 THEN 'Parcel Freight' ELSE 'Regional Freight' END,
      'TRL-' || (7000 + i),
      'SEAL-' || (9000 + i),
      dock,
      created_at + interval '18 hours',
      created_at + interval '20 hours',
      CASE WHEN load_status IN ('arrived', 'receiving', 'received', 'rejected', 'closed') THEN created_at + interval '21 hours' END,
      CASE WHEN load_status IN ('arrived', 'receiving', 'received', 'rejected', 'closed') THEN created_at + interval '21 hours' END,
      CASE WHEN load_status = 'rejected' THEN created_at + interval '22 hours' END,
      load_status IN ('received', 'closed'),
      CASE WHEN load_status = 'closed' THEN created_at + interval '24 hours' END,
      CASE WHEN load_status IN ('arrived', 'receiving', 'received', 'rejected', 'closed') THEN actor END,
      CASE WHEN load_status = 'closed' THEN actor END
    )
    RETURNING id INTO v_load_id;

    INSERT INTO load_notes (tenant_id, created, load_id, note)
    VALUES (tenant, created_at, v_load_id, 'Seed load');

    FOR j IN 1..(1 + (i % 4)) LOOP
      expected_qty := 12 + ((i * j * 7) % 96);
      received_qty := CASE WHEN load_status IN ('received', 'closed') THEN expected_qty ELSE 0 END;
      rejected_qty := CASE WHEN load_status = 'rejected' THEN expected_qty ELSE 0 END;
      missing_qty := 0;
      line_status := CASE
        WHEN received_qty = expected_qty THEN 'received'
        WHEN rejected_qty = expected_qty THEN 'rejected'
        ELSE 'pending'
      END;

      INSERT INTO load_lines
          (tenant_id, created, load_id, item_id, expected_qty, received_qty, rejected_qty,
           missing_qty, lot, status)
      VALUES (
        tenant, created_at, v_load_id,
        item_ids[((i + j - 2) % array_length(item_ids, 1)) + 1],
        expected_qty, received_qty, rejected_qty, missing_qty,
        'WB-SEED-LOT-' || seed_no || '-' || j,
        line_status
      );
    END LOOP;

    INSERT INTO load_activity (tenant_id, created, load_id, user_id, action, message)
    VALUES (tenant, created_at, v_load_id, actor, 'load_created', 'Seed load created');
  END LOOP;

  RAISE NOTICE 'Seeded up to % loads for tenant %.', seed_count, tenant;
END $$;
SQL
