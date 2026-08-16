use sqlx::Row;
use wareboxes_application::packing::CartonWeightEvidence;
use wareboxes_domain::{
    AutomationCommandId, AutomationDeviceId, CartonId, CartonWeightEvidenceId, InventoryOwnerId,
    PackSessionId, TenantId, Timestamp, UserId, WeightGrams,
};

use crate::error::{AppError, AppResult};

pub(super) async fn bind_actor_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    actor_user_id: i64,
) -> AppResult<()> {
    sqlx::query("SELECT set_config('wareboxes.actor_user_id',$1,true)")
        .bind(actor_user_id.to_string())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(super) struct WeightEvidenceCapture {
    pub tenant_id: TenantId,
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: i64,
    pub session_id: PackSessionId,
    pub carton_id: CartonId,
    pub carton_reopen_count: i64,
    pub weight_grams: WeightGrams,
    pub automation_command_id: Option<AutomationCommandId>,
    pub captured_by: UserId,
    pub captured_at: Timestamp,
}

pub(super) async fn capture_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    capture: WeightEvidenceCapture,
) -> AppResult<CartonWeightEvidence> {
    let scale = if let Some(command_id) = capture.automation_command_id {
        let row = sqlx::query(
            r#"SELECT command.status,command.device_id,device.device_key,
                      command.requested_by_user_id,command.requested_at,
                      command.completed_at,
                      command.packing_inventory_owner_id,command.packing_session_id,
                      command.packing_carton_id,command.packing_carton_reopen_count,
                      (command.result_payload->'result'->>'mass_milligrams')::bigint
                        AS mass_milligrams,
                      (command.result_payload->'result'->>'stable')::boolean AS stable
               FROM automation_commands command
               INNER JOIN automation_devices device
                 ON device.tenant_id=command.tenant_id AND device.id=command.device_id
               WHERE command.tenant_id=$1 AND command.facility_id=$2 AND command.id=$3
                 AND command.device_class='scale'
                 AND command.command_payload->'command'->>'operation'='read_stable_weight'
               FOR SHARE OF command,device"#,
        )
        .bind(capture.tenant_id.get())
        .bind(capture.facility_id)
        .bind(command_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::bad_request("scale reading is not available for this facility"))?;
        let mass_milligrams: Option<i64> = row.try_get("mass_milligrams")?;
        let stable: Option<bool> = row.try_get("stable")?;
        let completed_at: Option<Timestamp> = row.try_get("completed_at")?;
        if row.try_get::<String, _>("status")? != "succeeded"
            || stable != Some(true)
            || completed_at.is_none()
            || mass_milligrams != capture.weight_grams.get().checked_mul(1000)
            || row.try_get::<Option<i64>, _>("packing_inventory_owner_id")?
                != Some(capture.inventory_owner_id.get())
            || row.try_get::<Option<i64>, _>("packing_session_id")?
                != Some(capture.session_id.get())
            || row.try_get::<Option<i64>, _>("packing_carton_id")? != Some(capture.carton_id.get())
            || row.try_get::<Option<i64>, _>("packing_carton_reopen_count")?
                != Some(capture.carton_reopen_count)
        {
            return Err(AppError::conflict(
                "scale reading is not a matching completed stable weight",
            ));
        }
        Some((
            positive(row.try_get("device_id")?, AutomationDeviceId::new)?,
            row.try_get::<String, _>("device_key")?,
            positive(row.try_get("requested_by_user_id")?, UserId::new)?,
            row.try_get::<Timestamp, _>("requested_at")?,
            completed_at
                .ok_or_else(|| AppError::internal("scale reading lacks completion time"))?,
        ))
    } else {
        None
    };

    let evidence_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO carton_weight_evidence
           (tenant_id,inventory_owner_id,facility_id,packing_session_id,carton_id,
            carton_reopen_count,source,weight_g,automation_command_id,
            captured_by_user_id,captured_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) RETURNING id"#,
    )
    .bind(capture.tenant_id.get())
    .bind(capture.inventory_owner_id.get())
    .bind(capture.facility_id)
    .bind(capture.session_id.get())
    .bind(capture.carton_id.get())
    .bind(capture.carton_reopen_count)
    .bind(if scale.is_some() {
        "automation_scale"
    } else {
        "manual"
    })
    .bind(capture.weight_grams.get())
    .bind(capture.automation_command_id.map(|id| id.get()))
    .bind(capture.captured_by.get())
    .bind(capture.captured_at)
    .fetch_one(&mut **tx)
    .await?;
    let evidence_id = positive(evidence_id, CartonWeightEvidenceId::new)?;
    Ok(match scale {
        Some((device_id, device_key, requested_by, requested_at, completed_at)) => {
            CartonWeightEvidence::AutomationScale {
                evidence_id,
                weight_grams: capture.weight_grams,
                automation_command_id: capture.automation_command_id.ok_or_else(|| {
                    AppError::internal("automation evidence lacks command identity")
                })?,
                device_id,
                device_key,
                requested_by,
                requested_at,
                completed_at,
                captured_by: capture.captured_by,
                captured_at: capture.captured_at,
            }
        }
        None => CartonWeightEvidence::Manual {
            evidence_id,
            weight_grams: capture.weight_grams,
            captured_by: capture.captured_by,
            captured_at: capture.captured_at,
        },
    })
}

pub(super) fn from_row(row: &sqlx::postgres::PgRow) -> AppResult<Option<CartonWeightEvidence>> {
    let Some(evidence_id) = row.try_get::<Option<i64>, _>("weight_evidence_id")? else {
        return Ok(None);
    };
    let evidence_id = positive(evidence_id, CartonWeightEvidenceId::new)?;
    let weight_grams = positive(required::<i64>(row, "evidence_weight_g")?, WeightGrams::new)?;
    let captured_by = positive(
        required::<i64>(row, "weight_captured_by_user_id")?,
        UserId::new,
    )?;
    let captured_at = required(row, "weight_captured_at")?;
    match required::<String>(row, "weight_source")?.as_str() {
        "manual" => Ok(Some(CartonWeightEvidence::Manual {
            evidence_id,
            weight_grams,
            captured_by,
            captured_at,
        })),
        "automation_scale" => Ok(Some(CartonWeightEvidence::AutomationScale {
            evidence_id,
            weight_grams,
            automation_command_id: positive(
                required::<i64>(row, "weight_automation_command_id")?,
                AutomationCommandId::new,
            )?,
            device_id: positive(
                required::<i64>(row, "weight_device_id")?,
                AutomationDeviceId::new,
            )?,
            device_key: required(row, "weight_device_key")?,
            requested_by: positive(
                required::<i64>(row, "weight_requested_by_user_id")?,
                UserId::new,
            )?,
            requested_at: required(row, "weight_requested_at")?,
            completed_at: required(row, "weight_completed_at")?,
            captured_by,
            captured_at,
        })),
        _ => Err(AppError::internal(
            "carton weight evidence has invalid source",
        )),
    }
}

fn required<T>(row: &sqlx::postgres::PgRow, column: &str) -> AppResult<T>
where
    T: for<'r> sqlx::Decode<'r, sqlx::Postgres> + sqlx::Type<sqlx::Postgres>,
{
    row.try_get::<Option<T>, _>(column)?
        .ok_or_else(|| AppError::internal(format!("carton weight evidence lacks {column}")))
}

fn positive<T, E>(value: i64, constructor: impl FnOnce(i64) -> Result<T, E>) -> AppResult<T>
where
    E: std::fmt::Display,
{
    constructor(value).map_err(|error| AppError::internal(error.to_string()))
}

pub(super) const SELECT_COLUMNS: &str = r#"
evidence.id AS weight_evidence_id,evidence.source AS weight_source,
evidence.weight_g AS evidence_weight_g,
evidence.automation_command_id AS weight_automation_command_id,
evidence.captured_by_user_id AS weight_captured_by_user_id,
evidence.captured_at AS weight_captured_at,
weight_command.device_id AS weight_device_id,weight_device.device_key AS weight_device_key,
weight_command.requested_by_user_id AS weight_requested_by_user_id,
weight_command.requested_at AS weight_requested_at,
weight_command.completed_at AS weight_completed_at"#;
