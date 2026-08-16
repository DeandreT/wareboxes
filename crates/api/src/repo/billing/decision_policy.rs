use sqlx::Row;
use wareboxes_application::billing_decision_policy::BillingDecisionPolicyReadModel;
use wareboxes_domain::{
    BillingRateDefinition, BillingRateId, ConfigurationScope, ConfigurationVersionId, CurrencyCode,
    DecisionRuleDefinition, FacilityId, InventoryOwnerId,
};

use super::{internal, parse_event, parse_unit};
use crate::error::{AppError, AppResult};

pub(super) fn from_charge_row(
    row: &sqlx::postgres::PgRow,
) -> AppResult<BillingDecisionPolicyReadModel> {
    let event_type = parse_event(&row.try_get::<String, _>("event_type")?)?;
    let unit = parse_unit(&row.try_get::<String, _>("unit")?)?;
    let currency: String = row.try_get("currency")?;
    let rate_minor = positive(row.try_get("rate_minor")?, "billing policy rate")?;
    let minimum_charge_minor = nonnegative(
        row.try_get("minimum_charge_minor")?,
        "billing policy minimum",
    )?;
    let source: String = row.try_get("decision_policy_source")?;
    let policy = match source.as_str() {
        "contract_rate" => BillingDecisionPolicyReadModel::contract_rate(
            BillingRateId::new(required(row, "rate_version_id")?).map_err(internal)?,
            required(row, "contract_rate_revision")?,
            &BillingRateDefinition::new(
                event_type,
                unit,
                CurrencyCode::new(currency).map_err(internal)?,
                rate_minor,
                minimum_charge_minor,
            )
            .map_err(internal)?,
        )
        .map_err(internal)?,
        "configuration" => BillingDecisionPolicyReadModel::from_configuration(
            ConfigurationVersionId::new(required(row, "decision_configuration_id")?)
                .map_err(internal)?,
            required(row, "decision_configuration_revision")?,
            configuration_scope(
                &required::<String>(row, "decision_scope_level")?,
                row.try_get("decision_inventory_owner_id")?,
                row.try_get("decision_facility_id")?,
            )?,
            &DecisionRuleDefinition::Billing {
                event_type,
                unit,
                currency,
                rate_minor,
                minimum_charge_minor,
            },
        )
        .map_err(internal)?,
        _ => return Err(AppError::internal("Billing policy source is invalid")),
    };
    if row.try_get::<String, _>("decision_policy_hash")? != policy.policy_hash {
        return Err(AppError::internal(
            "Billing decision policy evidence is inconsistent",
        ));
    }
    Ok(policy)
}

fn configuration_scope(
    level: &str,
    inventory_owner_id: Option<i64>,
    facility_id: Option<i64>,
) -> AppResult<ConfigurationScope> {
    match level {
        "tenant" if inventory_owner_id.is_none() && facility_id.is_none() => {
            Ok(ConfigurationScope::Tenant)
        }
        "inventory_owner" if facility_id.is_none() => Ok(ConfigurationScope::InventoryOwner {
            inventory_owner_id: owner(inventory_owner_id)?,
        }),
        "facility" if inventory_owner_id.is_none() => Ok(ConfigurationScope::Facility {
            facility_id: facility(facility_id)?,
        }),
        "owner_facility" => Ok(ConfigurationScope::OwnerFacility {
            inventory_owner_id: owner(inventory_owner_id)?,
            facility_id: facility(facility_id)?,
        }),
        _ => Err(AppError::internal("Billing configuration scope is invalid")),
    }
}

fn required<T>(row: &sqlx::postgres::PgRow, column: &str) -> AppResult<T>
where
    for<'r> T: sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get::<Option<T>, _>(column)?
        .ok_or_else(|| AppError::internal(format!("Billing policy {column} is missing")))
}

fn positive(value: i64, field: &str) -> AppResult<u64> {
    let value = u64::try_from(value)
        .map_err(|_| AppError::internal(format!("stored {field} is invalid")))?;
    if value == 0 {
        Err(AppError::internal(format!("stored {field} is invalid")))
    } else {
        Ok(value)
    }
}

fn nonnegative(value: i64, field: &str) -> AppResult<u64> {
    u64::try_from(value).map_err(|_| AppError::internal(format!("stored {field} is invalid")))
}

fn owner(value: Option<i64>) -> AppResult<InventoryOwnerId> {
    InventoryOwnerId::new(
        value.ok_or_else(|| AppError::internal("Billing policy owner is missing"))?,
    )
    .map_err(internal)
}

fn facility(value: Option<i64>) -> AppResult<FacilityId> {
    FacilityId::new(value.ok_or_else(|| AppError::internal("Billing policy facility is missing"))?)
        .map_err(internal)
}
