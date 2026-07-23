ALTER TABLE inventory_transactions ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_transactions FORCE ROW LEVEL SECURITY;

CREATE POLICY inventory_transactions_tenant_isolation
    ON inventory_transactions
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

ALTER TABLE inventory_entries ENABLE ROW LEVEL SECURITY;
ALTER TABLE inventory_entries FORCE ROW LEVEL SECURITY;

CREATE POLICY inventory_entries_tenant_isolation
    ON inventory_entries
    USING (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    )
    WITH CHECK (
        tenant_id = NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    );

CREATE OR REPLACE VIEW inventory_reconciliation
WITH (security_invoker = true)
AS
WITH journal AS (
    SELECT tenant_id, inventory_owner_id, facility_id, location_id,
           license_plate_id, item_batch_id, item_id, uom, status,
           SUM(quantity_delta)::BIGINT AS journal_qty
    FROM inventory_entries
    WHERE tenant_id =
        NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    GROUP BY tenant_id, inventory_owner_id, facility_id, location_id,
             license_plate_id, item_batch_id, item_id, uom, status
), projection AS (
    SELECT tenant_id, inventory_owner_id, facility_id, location_id,
           license_plate_id, item_batch_id, item_id, uom, status,
           SUM(qty_on_hand)::BIGINT AS projected_qty
    FROM inventory_balances
    WHERE deleted IS NULL
      AND tenant_id =
          NULLIF(current_setting('wareboxes.tenant_id', true), '')::BIGINT
    GROUP BY tenant_id, inventory_owner_id, facility_id, location_id,
             license_plate_id, item_batch_id, item_id, uom, status
)
SELECT COALESCE(journal.tenant_id, projection.tenant_id) AS tenant_id,
       COALESCE(journal.inventory_owner_id, projection.inventory_owner_id)
           AS inventory_owner_id,
       COALESCE(journal.facility_id, projection.facility_id) AS facility_id,
       COALESCE(journal.location_id, projection.location_id) AS location_id,
       COALESCE(journal.license_plate_id, projection.license_plate_id)
           AS license_plate_id,
       COALESCE(journal.item_batch_id, projection.item_batch_id) AS item_batch_id,
       COALESCE(journal.item_id, projection.item_id) AS item_id,
       COALESCE(journal.uom, projection.uom) AS uom,
       COALESCE(journal.status, projection.status) AS status,
       COALESCE(journal.journal_qty, 0)::BIGINT AS journal_qty,
       COALESCE(projection.projected_qty, 0)::BIGINT AS projected_qty,
       (COALESCE(projection.projected_qty, 0) -
        COALESCE(journal.journal_qty, 0))::BIGINT AS variance
FROM journal
FULL OUTER JOIN projection
    ON projection.tenant_id = journal.tenant_id
   AND projection.inventory_owner_id = journal.inventory_owner_id
   AND projection.facility_id = journal.facility_id
   AND projection.location_id = journal.location_id
   AND projection.license_plate_id IS NOT DISTINCT FROM journal.license_plate_id
   AND projection.item_batch_id = journal.item_batch_id
   AND projection.item_id = journal.item_id
   AND projection.uom = journal.uom
   AND projection.status = journal.status
WHERE COALESCE(projection.projected_qty, 0) <>
      COALESCE(journal.journal_qty, 0);

DO $$
BEGIN
    EXECUTE format(
        'COMMENT ON VIEW public.inventory_reconciliation IS %L',
        'wareboxes.tenant_contract.md5=' || md5(
            pg_get_viewdef('public.inventory_reconciliation'::REGCLASS, true)
        )
    );
END
$$;
