#!/usr/bin/env bash
# Create tenant-scoped orders for local development and hosted demos.
set -euo pipefail
cd "$(dirname "$0")/.."

count=18

while [ "$#" -gt 0 ]; do
  case "$1" in
    --count|-n)
      count="${2:-}"
      shift 2
      ;;
    --help|-h)
      echo "usage: scripts/seed-orders.sh [--count N]"
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
  if [ -n "${MIGRATION_DATABASE_URL:-}" ] && command -v psql >/dev/null 2>&1; then
    psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 -v order_count="$count"
    return
  fi

  if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    echo "MIGRATION_DATABASE_URL+psql or an available Docker daemon is required." >&2
    exit 1
  fi
  docker compose exec -T postgres psql -U wareboxes_admin -d wareboxes \
    -v ON_ERROR_STOP=1 -v order_count="$count"
}

run_psql <<'SQL'
SELECT set_config('wareboxes.seed_order_count', :'order_count', false);

DO $$
DECLARE
  seed_count integer := current_setting('wareboxes.seed_order_count')::integer;
  tenant bigint;
  owner bigint;
  address bigint;
  item bigint;
  order_id bigint;
  item_ids bigint[] := ARRAY[]::bigint[];
  statuses text[] := ARRAY['open', 'open', 'held', 'open', 'shipped', 'cancelled'];
  order_status text;
  order_number text;
  i integer;
  j integer;
BEGIN
  SELECT id INTO tenant
  FROM tenants
  WHERE deleted IS NULL AND status = 'active'
  ORDER BY id
  LIMIT 1;
  IF tenant IS NULL THEN
    RAISE EXCEPTION 'seed-orders requires at least one active tenant';
  END IF;

  SELECT id INTO owner
  FROM inventory_owners
  WHERE tenant_id = tenant AND name = 'Seed Inventory Owner' AND deleted IS NULL
  ORDER BY id
  LIMIT 1;
  IF owner IS NULL THEN
    RAISE EXCEPTION 'seed-orders requires scripts/seed-inventory.sh to run first';
  END IF;

  SELECT array_agg(item_id ORDER BY item_id) INTO item_ids
  FROM inventory_owner_items
  WHERE tenant_id = tenant AND inventory_owner_id = owner AND deleted IS NULL;
  IF coalesce(array_length(item_ids, 1), 0) = 0 THEN
    RAISE EXCEPTION 'seed-orders requires inventory-owner items';
  END IF;

  FOR i IN 1..seed_count LOOP
    order_number := lpad(i::text, greatest(length(i::text), 5), '0');
    IF EXISTS (
      SELECT 1 FROM orders
      WHERE tenant_id = tenant
        AND inventory_owner_id = owner
        AND order_key = 'WB-DEMO-ORDER-' || order_number
    ) THEN
      CONTINUE;
    END IF;

    order_status := statuses[((i - 1) % array_length(statuses, 1)) + 1];
    INSERT INTO addresses
        (tenant_id, created, name, company, line1, postal_code, country, state, city)
    VALUES (
      tenant,
      now() - ((i % 12) || ' days')::interval,
      'Demo Customer ' || i,
      CASE WHEN i % 3 = 0 THEN 'Northstar Retail' ELSE 'Demo Customer' END,
      (100 + i) || ' Market Street',
      '9720' || (i % 10),
      'US',
      'OR',
      CASE WHEN i % 2 = 0 THEN 'Portland' ELSE 'Beaverton' END
    )
    RETURNING id INTO address;

    INSERT INTO orders
        (tenant_id, inventory_owner_id, order_key, created, rush, status,
         address_id, confirmed, closed, ship_by)
    VALUES (
      tenant,
      owner,
      'WB-DEMO-ORDER-' || order_number,
      now() - ((i % 12) || ' days')::interval,
      i % 5 = 0,
      order_status,
      address,
      CASE WHEN order_status = 'shipped' THEN now() - interval '2 days' END,
      CASE WHEN order_status = 'shipped' THEN now() - interval '1 day' END,
      now() + ((1 + i % 7) || ' days')::interval
    )
    RETURNING id INTO order_id;

    FOR j IN 1..(2 + i % 2) LOOP
      item := item_ids[((i + j - 2) % array_length(item_ids, 1)) + 1];
      INSERT INTO order_items
          (tenant_id, inventory_owner_id, created, qty, item_id, order_id)
      VALUES (tenant, owner, now(), 2 + ((i * j) % 9), item, order_id);
    END LOOP;

    INSERT INTO order_activity
        (tenant_id, inventory_owner_id, created, order_id, action)
    VALUES (tenant, owner, now(), order_id, 'demo order created');

    IF order_status = 'shipped' THEN
      INSERT INTO order_tracking_numbers
          (tenant_id, inventory_owner_id, created, order_id, tracking_number, carrier, service)
      VALUES (
        tenant, owner, now(), order_id,
        '1ZWBDEMO' || order_number, 'Demo Parcel', 'Ground'
      );
    END IF;
  END LOOP;

  RAISE NOTICE 'Seeded up to % demo orders for tenant %.', seed_count, tenant;
END $$;
SQL
