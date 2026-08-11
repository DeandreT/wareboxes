#!/usr/bin/env bash
# Seed a coherent local demo dataset and verify the promised workspace coverage.
set -euo pipefail

cd "$(dirname "$0")/.."

profile=full
inventory_count=400
order_count=125
load_count=125
verify_only=false

usage() {
  cat <<'USAGE'
Usage:
  scripts/seed-demo.sh [--profile core|full]
                       [--inventory-count N] [--order-count N] [--load-count N]
                       [--verify-only]

Profiles:
  core  Catalog, inventory, orders, and legacy inbound/outbound load history.
  full  Core data plus executable fulfillment, shipping, outbound-load,
        replenishment, cycle-count, putaway, hold, and integration scenarios.

The command is replay-safe. Set DATABASE_URL and MIGRATION_DATABASE_URL to target
a database outside the Docker Compose default. Full seeding uses the bootstrap
user selected by SEED_USER_EMAIL, or the oldest active tenant administrator.
USAGE
}

positive_integer() {
  [[ "$1" =~ ^[0-9]+$ ]] && [ "$1" -gt 0 ]
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --profile) profile="${2:-}"; shift 2 ;;
    --inventory-count) inventory_count="${2:-}"; shift 2 ;;
    --order-count) order_count="${2:-}"; shift 2 ;;
    --load-count) load_count="${2:-}"; shift 2 ;;
    --verify-only) verify_only=true; shift ;;
    --help|-h) usage; exit 0 ;;
    *) echo "unknown argument: $1" >&2; usage >&2; exit 2 ;;
  esac
done

if [ "$profile" != core ] && [ "$profile" != full ]; then
  echo "--profile must be core or full" >&2
  exit 2
fi
for value in "$inventory_count" "$order_count" "$load_count"; do
  if ! positive_integer "$value"; then
    echo "seed counts must be positive integers" >&2
    exit 2
  fi
done

run_psql() {
  if [ -n "${MIGRATION_DATABASE_URL:-}" ] && command -v psql >/dev/null 2>&1; then
    psql "$MIGRATION_DATABASE_URL" -v ON_ERROR_STOP=1 "$@"
    return
  fi
  if ! command -v docker >/dev/null 2>&1 || ! docker info >/dev/null 2>&1; then
    echo "MIGRATION_DATABASE_URL+psql or an available Docker daemon is required." >&2
    exit 1
  fi
  docker compose exec -T postgres psql -U wareboxes_admin -d wareboxes "$@"
}

verify_core() {
  run_psql -At <<'SQL'
DO $$
DECLARE
  missing text[] := ARRAY[]::text[];
BEGIN
  IF NOT EXISTS (SELECT 1 FROM inventory_owners WHERE deleted IS NULL) THEN missing := array_append(missing, 'clients'); END IF;
  IF NOT EXISTS (SELECT 1 FROM facilities WHERE deleted IS NULL) THEN missing := array_append(missing, 'facilities'); END IF;
  IF NOT EXISTS (SELECT 1 FROM locations WHERE deleted IS NULL) THEN missing := array_append(missing, 'locations'); END IF;
  IF NOT EXISTS (SELECT 1 FROM items WHERE deleted IS NULL) THEN missing := array_append(missing, 'items'); END IF;
  IF NOT EXISTS (SELECT 1 FROM inventory_balances WHERE deleted IS NULL) THEN missing := array_append(missing, 'inventory'); END IF;
  IF NOT EXISTS (SELECT 1 FROM orders WHERE deleted IS NULL) THEN missing := array_append(missing, 'orders'); END IF;
  IF NOT EXISTS (SELECT 1 FROM loads WHERE deleted IS NULL) THEN missing := array_append(missing, 'legacy loads'); END IF;
  IF NOT EXISTS (
    SELECT 1 FROM loads
    WHERE deleted IS NULL AND type='inbound' AND status='planned' AND appointment_time IS NULL
  ) THEN missing := array_append(missing, 'schedulable inbound load'); END IF;
  IF EXISTS (
    SELECT 1 FROM loads load
    WHERE load.deleted IS NULL AND load.type='inbound' AND load.status='cancelled'
      AND NOT EXISTS (
        SELECT 1 FROM inbound_load_cancellations cancellation
        WHERE cancellation.tenant_id=load.tenant_id AND cancellation.load_id=load.id
      )
  ) THEN missing := array_append(missing, 'typed inbound cancellation evidence'); END IF;
  IF EXISTS (
    SELECT 1 FROM loads load
    WHERE load.deleted IS NULL AND load.type='inbound' AND load.status='rejected'
      AND NOT EXISTS (
        SELECT 1 FROM inbound_load_rejections rejection
        WHERE rejection.tenant_id=load.tenant_id AND rejection.load_id=load.id
      )
  ) THEN missing := array_append(missing, 'typed inbound rejection evidence'); END IF;
  IF cardinality(missing) > 0 THEN
    RAISE EXCEPTION 'core demo coverage is incomplete: %', array_to_string(missing, ', ');
  END IF;
END $$;
SQL
}

verify_full() {
  run_psql -At <<'SQL'
DO $$
DECLARE
  missing text[] := ARRAY[]::text[];
BEGIN
  IF NOT EXISTS (SELECT 1 FROM packing_sessions) THEN missing := array_append(missing, 'packing'); END IF;
  IF NOT EXISTS (SELECT 1 FROM shipments) THEN missing := array_append(missing, 'shipping'); END IF;
  IF NOT EXISTS (SELECT 1 FROM outbound_loads) THEN missing := array_append(missing, 'outbound loads'); END IF;
  IF NOT EXISTS (SELECT 1 FROM pick_waves) THEN missing := array_append(missing, 'pick waves'); END IF;
  IF NOT EXISTS (SELECT 1 FROM replenishment_policies) THEN missing := array_append(missing, 'replenishment policies'); END IF;
  IF NOT EXISTS (SELECT 1 FROM replenishment_tasks) THEN missing := array_append(missing, 'replenishment work'); END IF;
  IF NOT EXISTS (SELECT 1 FROM cycle_count_location_tasks)
     AND NOT EXISTS (SELECT 1 FROM cycle_count_item_location_tasks) THEN
    missing := array_append(missing, 'cycle counts');
  END IF;
  IF NOT EXISTS (SELECT 1 FROM putaway_tasks) THEN missing := array_append(missing, 'putaway'); END IF;
  IF NOT EXISTS (SELECT 1 FROM inventory_holds) THEN missing := array_append(missing, 'inventory holds'); END IF;
  IF NOT EXISTS (SELECT 1 FROM integration_inbox_receipts) THEN missing := array_append(missing, 'integration monitor'); END IF;
  IF EXISTS (
    SELECT 1
    FROM loads load
    INNER JOIN load_lines line
      ON line.tenant_id=load.tenant_id AND line.load_id=load.id
    WHERE load.type='inbound'
      AND load.status IN ('planned','scheduled')
      AND load.deleted IS NULL
      AND (
        load.dock_door_location_id IS NULL
        OR NOT EXISTS (
          SELECT 1 FROM locations location
          WHERE location.tenant_id=load.tenant_id
            AND location.id=load.dock_door_location_id
            AND location.deleted IS NULL
            AND location.active AND location.receivable
            AND NULLIF(btrim(location.barcode),'') IS NOT NULL
        )
        OR NOT EXISTS (
          SELECT 1 FROM inventory_owner_items owner_item
          WHERE owner_item.tenant_id=load.tenant_id
            AND owner_item.inventory_owner_id=load.inventory_owner_id
            AND owner_item.item_id=line.item_id
            AND owner_item.deleted IS NULL
        )
        OR NOT EXISTS (
          SELECT 1 FROM barcodes barcode
          WHERE barcode.tenant_id=load.tenant_id
            AND barcode.item_id=line.item_id
            AND barcode.deleted IS NULL
            AND NULLIF(btrim(barcode.name),'') IS NOT NULL
        )
      )
  ) THEN missing := array_append(missing, 'executable planned inbound loads'); END IF;
  IF cardinality(missing) > 0 THEN
    RAISE EXCEPTION 'full demo coverage is incomplete: %', array_to_string(missing, ', ');
  END IF;
END $$;
SQL
}

report_coverage() {
  run_psql -P pager=off -c "
    SELECT workspace, records
    FROM (VALUES
      ('Inventory', (SELECT COUNT(*) FROM inventory_balances WHERE deleted IS NULL)),
      ('Orders', (SELECT COUNT(*) FROM orders WHERE deleted IS NULL)),
      ('Legacy loads', (SELECT COUNT(*) FROM loads WHERE deleted IS NULL)),
      ('Pick waves', (SELECT COUNT(*) FROM pick_waves)),
      ('Packing sessions', (SELECT COUNT(*) FROM packing_sessions)),
      ('Shipments', (SELECT COUNT(*) FROM shipments)),
      ('Outbound loads', (SELECT COUNT(*) FROM outbound_loads)),
      ('Putaway work', (SELECT COUNT(*) FROM putaway_tasks)),
      ('Cycle-count work', (SELECT COUNT(*) FROM cycle_count_location_tasks)
                           + (SELECT COUNT(*) FROM cycle_count_item_location_tasks)),
      ('Inventory holds', (SELECT COUNT(*) FROM inventory_holds)),
      ('Replenishment policies', (SELECT COUNT(*) FROM replenishment_policies)),
      ('Replenishment work', (SELECT COUNT(*) FROM replenishment_tasks)),
      ('Integration receipts', (SELECT COUNT(*) FROM integration_inbox_receipts))
    ) AS coverage(workspace, records)
    ORDER BY workspace;"
}

if ! $verify_only; then
  scripts/seed-inventory.sh --count "$inventory_count"
  scripts/seed-orders.sh --count "$order_count"
  scripts/seed-loads.sh --count "$load_count" --keep-existing

  if [ "$profile" = full ]; then
    if [ -f "$HOME/.cargo/env" ]; then
      # shellcheck disable=SC1090
      . "$HOME/.cargo/env"
    fi
    CARGO_BUILD_JOBS=1 cargo run --locked -p wareboxes-server --example seed_demo --
  fi
fi

verify_core
if [ "$profile" = full ]; then
  verify_full
fi
report_coverage

echo "Demo seed profile '$profile' is ready."
