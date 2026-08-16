use sqlx::Row;
use wareboxes_domain::{
    BillingContractId, FacilityId, InventoryOwnerId, TenantId, Timestamp, UserId,
};

use crate::error::{AppError, AppResult};

pub(super) struct ReconciliationRunInput<'a> {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub contract_id: BillingContractId,
    pub facility_id: Option<FacilityId>,
    pub period_from: Timestamp,
    pub period_until: Timestamp,
    pub attempt: i64,
    pub supersedes_run_id: Option<i64>,
    pub currency: &'a str,
    pub generated_by: UserId,
    pub generated_at: Timestamp,
}

/// Resolves every uncharged event against the exact decision state visible at
/// the event time, writes the immutable run, and materializes all charges in
/// the same transaction. The database trigger independently resolves the same
/// winner before accepting each charge.
pub(super) async fn create_run_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    input: ReconciliationRunInput<'_>,
) -> AppResult<i64> {
    let ReconciliationRunInput {
        tenant_id,
        inventory_owner_id,
        contract_id,
        facility_id,
        period_from,
        period_until,
        attempt,
        supersedes_run_id,
        currency,
        generated_by,
        generated_at,
    } = input;

    let stats = sqlx::query(
        r#"WITH eligible AS (
             SELECT event.id,event.quantity,
                    configuration.id AS configuration_id,
                    rate.id AS rate_id,
                    COALESCE((configuration.definition->>'rate_minor')::BIGINT,
                             rate.rate_minor) AS rate_minor,
                    COALESCE((configuration.definition->>'minimum_charge_minor')::BIGINT,
                             rate.minimum_charge_minor) AS minimum_charge_minor
             FROM billable_events event
             LEFT JOIN LATERAL (
               SELECT candidate.id,candidate.definition
               FROM configuration_versions candidate
               WHERE candidate.tenant_id=event.tenant_id AND candidate.kind='billing'
                 AND candidate.status IN ('active','retired')
                 AND candidate.activated_at<=event.occurred_at
                 AND (candidate.retired_at IS NULL OR candidate.retired_at>event.occurred_at)
                 AND candidate.effective_from<=event.occurred_at
                 AND (candidate.effective_until IS NULL
                      OR candidate.effective_until>event.occurred_at)
                 AND (candidate.inventory_owner_id IS NULL
                      OR candidate.inventory_owner_id=event.inventory_owner_id)
                 AND (candidate.facility_id IS NULL
                      OR candidate.facility_id=event.facility_id)
                 AND candidate.definition->>'event_type'=event.event_type
                 AND candidate.definition->>'unit'=event.unit
                 AND candidate.definition->>'currency'=$7
               ORDER BY CASE candidate.scope_level WHEN 'owner_facility' THEN 2
                          WHEN 'inventory_owner' THEN 1 WHEN 'facility' THEN 1 ELSE 0 END DESC,
                        candidate.effective_from DESC,candidate.revision DESC,candidate.id DESC
               LIMIT 1
             ) configuration ON true
             LEFT JOIN LATERAL (
               SELECT candidate.id,candidate.rate_minor,candidate.minimum_charge_minor
               FROM billing_rate_versions candidate
               WHERE configuration.id IS NULL
                 AND candidate.tenant_id=event.tenant_id
                 AND candidate.contract_id=event.contract_id
                 AND candidate.event_type=event.event_type AND candidate.unit=event.unit
                 AND candidate.currency=$7
                 AND candidate.effective_from<=event.occurred_at
                 AND (candidate.effective_until IS NULL
                      OR candidate.effective_until>event.occurred_at)
               ORDER BY candidate.revision DESC,candidate.id DESC LIMIT 1
             ) rate ON true
             WHERE event.tenant_id=$1 AND event.contract_id=$2
               AND event.occurred_at>=$3 AND event.occurred_at<$4
               AND event.captured_at<=$6
               AND ($5::BIGINT IS NULL OR event.facility_id=$5)
               AND NOT EXISTS(
                 SELECT 1 FROM billing_charges prior_charge
                 JOIN billing_reconciliation_runs prior_run
                   ON prior_run.tenant_id=prior_charge.tenant_id
                  AND prior_run.id=prior_charge.reconciliation_run_id
                 WHERE prior_charge.tenant_id=event.tenant_id
                   AND prior_charge.billable_event_id=event.id
                   AND prior_run.status<>'rejected'))
           SELECT count(*)::BIGINT AS event_count,
                  count(*) FILTER (WHERE configuration_id IS NOT NULL OR rate_id IS NOT NULL)::BIGINT
                    AS charge_count,
                  count(*) FILTER (WHERE configuration_id IS NULL AND rate_id IS NULL)::BIGINT
                    AS unmatched_event_count,
                  COALESCE(sum(GREATEST(rate_minor::NUMERIC*quantity,
                                        minimum_charge_minor::NUMERIC)) FILTER
                           (WHERE configuration_id IS NOT NULL OR rate_id IS NOT NULL),0)::TEXT
                    AS total_minor
           FROM eligible"#,
    )
    .bind(tenant_id.get())
    .bind(contract_id.get())
    .bind(period_from)
    .bind(period_until)
    .bind(facility_id.map(FacilityId::get))
    .bind(generated_at)
    .bind(currency)
    .fetch_one(&mut **tx)
    .await?;
    let event_count: i64 = stats.try_get("event_count")?;
    let charge_count: i64 = stats.try_get("charge_count")?;
    let unmatched_event_count: i64 = stats.try_get("unmatched_event_count")?;
    let total_text: String = stats.try_get("total_minor")?;
    let total_u128 = total_text
        .parse::<u128>()
        .map_err(|error| AppError::internal(format!("invalid billing total: {error}")))?;
    let total_minor = i64::try_from(total_u128)
        .map_err(|_| AppError::conflict("billing total exceeds supported financial range"))?;

    let run_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO billing_reconciliation_runs
             (tenant_id,inventory_owner_id,contract_id,facility_id,attempt,supersedes_run_id,
              period_from,period_until,event_count,charge_count,unmatched_event_count,total_minor,
              currency,generated_by_user_id,generated_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(contract_id.get())
    .bind(facility_id.map(FacilityId::get))
    .bind(attempt)
    .bind(supersedes_run_id)
    .bind(period_from)
    .bind(period_until)
    .bind(event_count)
    .bind(charge_count)
    .bind(unmatched_event_count)
    .bind(total_minor)
    .bind(currency)
    .bind(generated_by.get())
    .bind(generated_at)
    .fetch_one(&mut **tx)
    .await?;

    sqlx::query(
        r#"WITH eligible AS (
             SELECT event.*,
                    configuration.id AS configuration_id,
                    CASE configuration.status WHEN 'retired' THEN configuration.revision-1
                      ELSE configuration.revision END AS configuration_revision,
                    configuration.scope_level,
                    configuration.inventory_owner_id AS configuration_owner_id,
                    configuration.facility_id AS configuration_facility_id,
                    rate.id AS rate_id,rate.revision AS rate_revision,
                    COALESCE((configuration.definition->>'rate_minor')::BIGINT,
                             rate.rate_minor) AS resolved_rate_minor,
                    COALESCE((configuration.definition->>'minimum_charge_minor')::BIGINT,
                             rate.minimum_charge_minor) AS resolved_minimum_minor
             FROM billable_events event
             LEFT JOIN LATERAL (
               SELECT candidate.* FROM configuration_versions candidate
               WHERE candidate.tenant_id=event.tenant_id AND candidate.kind='billing'
                 AND candidate.status IN ('active','retired')
                 AND candidate.activated_at<=event.occurred_at
                 AND (candidate.retired_at IS NULL OR candidate.retired_at>event.occurred_at)
                 AND candidate.effective_from<=event.occurred_at
                 AND (candidate.effective_until IS NULL
                      OR candidate.effective_until>event.occurred_at)
                 AND (candidate.inventory_owner_id IS NULL
                      OR candidate.inventory_owner_id=event.inventory_owner_id)
                 AND (candidate.facility_id IS NULL
                      OR candidate.facility_id=event.facility_id)
                 AND candidate.definition->>'event_type'=event.event_type
                 AND candidate.definition->>'unit'=event.unit
                 AND candidate.definition->>'currency'=$8
               ORDER BY CASE candidate.scope_level WHEN 'owner_facility' THEN 2
                          WHEN 'inventory_owner' THEN 1 WHEN 'facility' THEN 1 ELSE 0 END DESC,
                        candidate.effective_from DESC,candidate.revision DESC,candidate.id DESC
               LIMIT 1
             ) configuration ON true
             LEFT JOIN LATERAL (
               SELECT candidate.id,candidate.revision,candidate.rate_minor,
                      candidate.minimum_charge_minor
               FROM billing_rate_versions candidate
               WHERE configuration.id IS NULL
                 AND candidate.tenant_id=event.tenant_id
                 AND candidate.contract_id=event.contract_id
                 AND candidate.event_type=event.event_type AND candidate.unit=event.unit
                 AND candidate.currency=$8
                 AND candidate.effective_from<=event.occurred_at
                 AND (candidate.effective_until IS NULL
                      OR candidate.effective_until>event.occurred_at)
               ORDER BY candidate.revision DESC,candidate.id DESC LIMIT 1
             ) rate ON true
             WHERE event.tenant_id=$1 AND event.contract_id=$2
               AND event.occurred_at>=$3 AND event.occurred_at<$4
               AND event.captured_at<=$6
               AND ($5::BIGINT IS NULL OR event.facility_id=$5)
               AND NOT EXISTS(
                 SELECT 1 FROM billing_charges prior_charge
                 JOIN billing_reconciliation_runs prior_run
                   ON prior_run.tenant_id=prior_charge.tenant_id
                  AND prior_run.id=prior_charge.reconciliation_run_id
                 WHERE prior_charge.tenant_id=event.tenant_id
                   AND prior_charge.billable_event_id=event.id
                   AND prior_run.status<>'rejected'))
           INSERT INTO billing_charges
             (tenant_id,inventory_owner_id,facility_id,contract_id,reconciliation_run_id,
              billable_event_id,rate_version_id,contract_rate_revision,
              decision_policy_source,decision_configuration_id,
              decision_configuration_revision,decision_scope_level,
              decision_inventory_owner_id,decision_facility_id,decision_policy_hash,
              event_type,unit,quantity,rate_minor,minimum_charge_minor,gross_minor,
              amount_minor,currency,source_type,source_reference,occurred_at,created_at)
           SELECT tenant_id,inventory_owner_id,facility_id,contract_id,$7,id,rate_id,rate_revision,
                  CASE WHEN configuration_id IS NOT NULL THEN 'configuration'
                       ELSE 'contract_rate' END,
                  configuration_id,configuration_revision,scope_level,
                  configuration_owner_id,configuration_facility_id,
                  public.billing_decision_policy_hash(
                    CASE WHEN configuration_id IS NOT NULL THEN 'configuration'
                         ELSE 'contract_rate' END,
                    rate_id,rate_revision,configuration_id,configuration_revision,scope_level,
                    configuration_owner_id,configuration_facility_id,event_type,unit,$8,
                    resolved_rate_minor,resolved_minimum_minor),
                  event_type,unit,quantity,resolved_rate_minor,resolved_minimum_minor,
                  (resolved_rate_minor::NUMERIC*quantity)::BIGINT,
                  GREATEST(resolved_rate_minor::NUMERIC*quantity,
                           resolved_minimum_minor::NUMERIC)::BIGINT,
                  $8,source_type,source_reference,occurred_at,$6
           FROM eligible WHERE configuration_id IS NOT NULL OR rate_id IS NOT NULL ORDER BY id"#,
    )
    .bind(tenant_id.get())
    .bind(contract_id.get())
    .bind(period_from)
    .bind(period_until)
    .bind(facility_id.map(FacilityId::get))
    .bind(generated_at)
    .bind(run_id)
    .bind(currency)
    .execute(&mut **tx)
    .await?;

    Ok(run_id)
}
