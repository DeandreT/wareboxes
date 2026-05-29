#!/usr/bin/env bash
# Create visual test data for the Loads screen.
set -euo pipefail
cd "$(dirname "$0")/.."

count=500000
clear_seed=true

usage() {
  cat <<'USAGE'
Usage:
  scripts/seed-loads.sh                 create 40 visual test loads
  scripts/seed-loads.sh --count 80      create a specific number of loads
  scripts/seed-loads.sh --keep-existing keep existing WB-SEED-* loads

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
    psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -v load_count="$count" -v clear_seed="$clear_seed"
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
    -v ON_ERROR_STOP=1 -v load_count="$count" -v clear_seed="$clear_seed"
}

run_psql <<'SQL'
SELECT set_config('wareboxes.seed_load_count', :'load_count', false);
SELECT set_config('wareboxes.seed_clear_seed', :'clear_seed', false);

CREATE TABLE IF NOT EXISTS order_tracking_numbers (
    id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,
    created TIMESTAMPTZ NOT NULL,
    deleted TIMESTAMPTZ,
    order_id BIGINT NOT NULL REFERENCES orders(id),
    tracking_number TEXT NOT NULL,
    carrier TEXT,
    service TEXT,
    UNIQUE (order_id, tracking_number)
);

CREATE INDEX IF NOT EXISTS idx_order_tracking_numbers_order_id
    ON order_tracking_numbers(order_id)
    WHERE deleted IS NULL;

DO $$
DECLARE
  seed_count integer := current_setting('wareboxes.seed_load_count')::integer;
  clear_seed boolean := current_setting('wareboxes.seed_clear_seed')::boolean;
  v_wh_id bigint;
  v_acct_id bigint;
  v_acct_ids bigint[] := ARRAY[]::bigint[];
  v_item_id bigint;
  v_item_ids bigint[] := ARRAY[]::bigint[];
  v_dock_id bigint;
  v_recv_loc_id bigint;
  v_seed_user_id bigint;
  v_load_id bigint;
  v_address_id bigint;
  v_order_id bigint;
  created_at timestamptz;
  load_status text;
  load_type text;
  order_status text;
  expected bigint;
  received bigint;
  rejected bigint;
  missing bigint;
  line_status text;
  account_names text[] := ARRAY['Acme Grocery', 'Northstar Retail', 'Summit Wholesale', 'Redwood Foods'];
  item_names text[] := ARRAY['Seed Canned Beans', 'Seed Paper Towels', 'Seed Sparkling Water', 'Seed Dog Food', 'Seed Protein Bars', 'Seed Olive Oil', 'Seed Pasta Sauce', 'Seed Granola'];
  statuses text[] := ARRAY['planned', 'scheduled', 'arrived', 'receiving', 'received', 'rejected', 'closed', 'cancelled'];
  order_statuses text[] := ARRAY['open', 'processing', 'awaiting shipment', 'shipped', 'held'];
  carriers text[] := ARRAY['Roadrunner Freight', 'Estes Express', 'Old Dominion', 'FedEx Freight', 'XPO', 'R+L Carriers'];
  parcel_services text[] := ARRAY['Ground', '2Day', 'Next Day Air', 'Home Delivery'];
  i integer;
  j integer;
  k integer;
  seed_no text;
  order_no text;
BEGIN
  IF clear_seed THEN
    DELETE FROM load_orders lo WHERE lo.load_id IN (SELECT l.id FROM loads l WHERE l.reference_number LIKE 'WB-SEED-%');
    DELETE FROM load_orders lo WHERE lo.order_id IN (SELECT o.id FROM orders o WHERE o.order_key LIKE 'WB-ORD-%');
    DELETE FROM order_tracking_numbers otn WHERE otn.order_id IN (SELECT o.id FROM orders o WHERE o.order_key LIKE 'WB-ORD-%');
    DELETE FROM order_items oi WHERE oi.order_id IN (SELECT o.id FROM orders o WHERE o.order_key LIKE 'WB-ORD-%');
    DELETE FROM orders o WHERE o.order_key LIKE 'WB-ORD-%';
    DELETE FROM load_activity la WHERE la.load_id IN (SELECT l.id FROM loads l WHERE l.reference_number LIKE 'WB-SEED-%');
    DELETE FROM load_files lf WHERE lf.load_id IN (SELECT l.id FROM loads l WHERE l.reference_number LIKE 'WB-SEED-%');
    DELETE FROM load_notes ln WHERE ln.load_id IN (SELECT l.id FROM loads l WHERE l.reference_number LIKE 'WB-SEED-%');
    DELETE FROM load_lines ll WHERE ll.load_id IN (SELECT l.id FROM loads l WHERE l.reference_number LIKE 'WB-SEED-%');
    DELETE FROM loads WHERE reference_number LIKE 'WB-SEED-%';
  END IF;

  SELECT id INTO v_wh_id FROM warehouses WHERE name = 'Seed Warehouse' ORDER BY id LIMIT 1;
  IF v_wh_id IS NULL THEN
    INSERT INTO warehouses (created, name) VALUES (now(), 'Seed Warehouse') RETURNING id INTO v_wh_id;
  END IF;

  FOR i IN 1..array_length(account_names, 1) LOOP
    INSERT INTO accounts (created, name, email)
    VALUES (now(), account_names[i], lower(replace(account_names[i], ' ', '.')) || '@example.test')
    ON CONFLICT (name) DO UPDATE SET email = EXCLUDED.email
    RETURNING id INTO v_acct_id;
    v_acct_ids := array_append(v_acct_ids, v_acct_id);

    INSERT INTO account_warehouses (created, account_id, warehouse_id)
    VALUES (now(), v_acct_id, v_wh_id)
    ON CONFLICT (account_id, warehouse_id) DO NOTHING;
  END LOOP;

  INSERT INTO locations (created, warehouse_id, barcode, name, type, active, pickable, receivable)
  VALUES (now(), v_wh_id, 'SEED-DOCK-01', 'Seed Dock Door 1', 'dock', true, false, true)
  ON CONFLICT (barcode) DO UPDATE SET name = EXCLUDED.name, active = true, receivable = true
  RETURNING id INTO v_dock_id;

  INSERT INTO locations (created, warehouse_id, barcode, name, type, active, pickable, receivable)
  VALUES (now(), v_wh_id, 'SEED-RECV-01', 'Seed Receiving Staging', 'staging', true, false, true)
  ON CONFLICT (barcode) DO UPDATE SET name = EXCLUDED.name, active = true, receivable = true
  RETURNING id INTO v_recv_loc_id;

  FOR i IN 1..array_length(item_names, 1) LOOP
    SELECT id INTO v_item_id FROM items WHERE description = item_names[i] ORDER BY id LIMIT 1;
    IF v_item_id IS NULL THEN
      INSERT INTO items (created, description, notes, packaging_unit, pallet_hi, pallet_ti, inner_units)
      VALUES (now(), item_names[i], 'Seed load visualization item', 'case', 5, 8, 12)
      RETURNING id INTO v_item_id;
    END IF;
    v_item_ids := array_append(v_item_ids, v_item_id);
  END LOOP;

  INSERT INTO users (created, first_name, last_name, email)
  VALUES (now(), 'Seed', 'Receiver', 'seed.receiver@example.test')
  ON CONFLICT (email) DO UPDATE SET first_name = EXCLUDED.first_name
  RETURNING id INTO v_seed_user_id;

  FOR i IN 1..seed_count LOOP
    seed_no := lpad(i::text, greatest(length(i::text), 6), '0');
    load_status := statuses[((i - 1) % array_length(statuses, 1)) + 1];
    load_type := CASE WHEN i % 6 = 0 THEN 'outbound' ELSE 'inbound' END;
    created_at := now() - (i || ' days')::interval + ((i % 9) || ' hours')::interval;

    INSERT INTO loads (
      created, warehouse_id, account_id, status, type, reference_number, invoice_number,
      carrier, trailer_number, seal_number, dock_door_location_id, expected_time,
      appointment_time, actual_time, arrival, rejected, receive_completed, closed,
      checked_in_by, closed_by
    ) VALUES (
      created_at,
      v_wh_id,
      v_acct_ids[((i - 1) % array_length(v_acct_ids, 1)) + 1],
      load_status,
      load_type,
      'WB-SEED-' || seed_no,
      'INV-SEED-' || seed_no,
      carriers[((i - 1) % array_length(carriers, 1)) + 1],
      'TRL-' || (7000 + i),
      'SEAL-' || (9000 + i),
      v_dock_id,
      created_at + interval '18 hours',
      created_at + interval '22 hours',
      CASE WHEN load_status IN ('arrived', 'receiving', 'received', 'rejected', 'closed') THEN created_at + interval '23 hours' ELSE NULL END,
      CASE WHEN load_status IN ('arrived', 'receiving', 'received', 'rejected', 'closed') THEN created_at + interval '23 hours' ELSE NULL END,
      CASE WHEN load_status = 'rejected' THEN created_at + interval '25 hours' ELSE NULL END,
      load_status IN ('received', 'closed'),
      CASE WHEN load_status = 'closed' THEN created_at + interval '30 hours' ELSE NULL END,
      CASE WHEN load_status IN ('arrived', 'receiving', 'received', 'rejected', 'closed') THEN v_seed_user_id ELSE NULL END,
      CASE WHEN load_status = 'closed' THEN v_seed_user_id ELSE NULL END
    ) RETURNING id INTO v_load_id;

    INSERT INTO load_notes (created, load_id, note)
    VALUES (created_at + interval '10 minutes', v_load_id, 'Seeded load for UI visualization');

    IF i % 3 = 0 THEN
      INSERT INTO load_files (created, load_id, original_name, name, path, content_type, category)
      VALUES (
        created_at + interval '15 minutes',
        v_load_id,
        'invoice-' || i || '.jpg',
        'invoice-' || i || '.jpg',
        '/seed/invoices/invoice-' || i || '.jpg',
        'image/jpeg',
        'invoice'
      );
    END IF;

    IF load_type = 'outbound' THEN
      FOR j IN 1..(3 + (i % 4)) LOOP
        order_no := seed_no || '-' || lpad(j::text, 2, '0');
        order_status := order_statuses[((i + j - 2) % array_length(order_statuses, 1)) + 1];

        INSERT INTO addresses (
          created, name, company, line1, city, state, postal_code, country, phone, email
        ) VALUES (
          created_at + (j || ' minutes')::interval,
          'Seed Customer ' || i || '-' || j,
          'Seed Ship-To Co',
          (1000 + i + j) || ' Market St',
          CASE WHEN j % 3 = 0 THEN 'Portland' WHEN j % 3 = 1 THEN 'Seattle' ELSE 'Reno' END,
          CASE WHEN j % 3 = 0 THEN 'OR' WHEN j % 3 = 1 THEN 'WA' ELSE 'NV' END,
          lpad((97000 + i + j)::text, 5, '0'),
          'US',
          '555-010' || (j % 10),
          'seed.customer+' || i || '-' || j || '@example.test'
        ) RETURNING id INTO v_address_id;

        INSERT INTO orders (
          order_key, created, rush, status, address_id, confirmed, closed, ship_by, account_id
        ) VALUES (
          'WB-ORD-' || order_no,
          created_at + (j || ' minutes')::interval,
          (i + j) % 7 = 0,
          order_status,
          v_address_id,
          CASE WHEN order_status IN ('awaiting shipment', 'shipped') THEN created_at + interval '4 hours' ELSE NULL END,
          CASE WHEN order_status = 'shipped' THEN created_at + interval '8 hours' ELSE NULL END,
          created_at + interval '1 day',
          v_acct_ids[((i - 1) % array_length(v_acct_ids, 1)) + 1]
        ) RETURNING id INTO v_order_id;

        INSERT INTO load_orders (created, load_id, order_id)
        VALUES (created_at + (j || ' minutes')::interval, v_load_id, v_order_id);

        FOR k IN 1..(1 + (j % 3)) LOOP
          INSERT INTO order_items (created, qty, item_id, order_id)
          VALUES (
            created_at + ((j * 10 + k) || ' minutes')::interval,
            1 + ((i + j + k) % 6),
            v_item_ids[((i + j + k - 3) % array_length(v_item_ids, 1)) + 1],
            v_order_id
          );
        END LOOP;

        FOR k IN 1..(1 + (j % 2)) LOOP
          INSERT INTO order_tracking_numbers (
            created, order_id, tracking_number, carrier, service
          ) VALUES (
            created_at + ((j * 10 + k) || ' minutes')::interval,
            v_order_id,
            '1ZWB' || order_no || '-' || lpad(k::text, 2, '0') || '-' || lpad(((i * j * k) % 999999)::text, 6, '0'),
            CASE WHEN k % 2 = 0 THEN 'FedEx' ELSE 'UPS' END,
            parcel_services[((i + j + k - 3) % array_length(parcel_services, 1)) + 1]
          );
        END LOOP;
      END LOOP;
    ELSE
      FOR j IN 1..(2 + (i % 4)) LOOP
        expected := 20 + (((i + j) % 8) * 5);
        received := 0;
        rejected := 0;
        missing := 0;

        IF load_status IN ('planned', 'scheduled', 'arrived', 'cancelled') THEN
          NULL;
        ELSIF load_status = 'receiving' THEN
          IF j % 3 = 1 THEN
            received := expected;
          ELSIF j % 3 = 2 THEN
            received := greatest(1, expected / 2);
          ELSE
            rejected := 2;
            missing := 1;
          END IF;
        ELSIF load_status IN ('received', 'closed') THEN
          received := expected;
        ELSIF load_status = 'rejected' THEN
          IF j % 2 = 0 THEN
            rejected := expected;
          ELSE
            received := greatest(1, expected / 3);
            rejected := 2;
          END IF;
        END IF;

        IF received + rejected + missing >= expected THEN
          IF received > 0 THEN
            line_status := 'received';
          ELSIF rejected > 0 THEN
            line_status := 'rejected';
          ELSE
            line_status := 'missing';
          END IF;
        ELSIF received > 0 OR rejected > 0 OR missing > 0 THEN
          line_status := 'partial';
        ELSE
          line_status := 'pending';
        END IF;

        INSERT INTO load_lines (
          created, load_id, item_id, expected_qty, received_qty, rejected_qty, missing_qty,
          missing_confirmed_by, missing_confirmed_at, lot, status
        ) VALUES (
          created_at + (j || ' minutes')::interval,
          v_load_id,
          v_item_ids[((i + j - 2) % array_length(v_item_ids, 1)) + 1],
          expected,
          received,
          rejected,
          missing,
          CASE WHEN missing > 0 THEN v_seed_user_id ELSE NULL END,
          CASE WHEN missing > 0 THEN created_at + interval '1 hour' ELSE NULL END,
          'LOT-' || to_char(created_at, 'YYYYMMDD') || '-' || j,
          line_status
        );
      END LOOP;
    END IF;

    INSERT INTO load_activity (created, load_id, user_id, action, message)
    VALUES (created_at, v_load_id, v_seed_user_id, 'seeded', 'seeded visual test load');
  END LOOP;

  RAISE NOTICE 'Seeded % loads. Warehouse %, dock location %, receive location %.', seed_count, v_wh_id, v_dock_id, v_recv_loc_id;
END $$;
SQL
