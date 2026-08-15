//! Tenant-safe, replay-safe decision-table configuration lifecycle and resolution.

use sqlx::Row;
use wareboxes_application::configuration::{
    ActivateConfigurationCommand, ActivateConfigurationResult, ApproveConfigurationResult,
    ConfigurationCursor, ConfigurationLifecycleCommand, ConfigurationPage, ConfigurationPageQuery,
    ConfigurationReadModel, ConfigurationSimulationResult, CreateConfigurationCommand,
    CreateConfigurationResult, RetireConfigurationResult, RollbackConfigurationCommand,
    RollbackConfigurationResult, SimulateConfigurationQuery, SubmitConfigurationResult,
    ACTIVATE_CONFIGURATION_OPERATION, APPROVE_CONFIGURATION_OPERATION,
    CREATE_CONFIGURATION_OPERATION, RETIRE_CONFIGURATION_OPERATION,
    ROLLBACK_CONFIGURATION_OPERATION, SUBMIT_CONFIGURATION_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    resolve_effective_rule, ConfigurationEffectiveWindow, ConfigurationScope, ConfigurationStatus,
    ConfigurationVersionId, DecisionRuleDefinition, DecisionRuleKind, EffectiveDecisionRule,
    FacilityId, InventoryOwnerId, TenantId, Timestamp, UserId,
};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;
use wareboxes_persistence_postgres::outbox::{self, NewOutboxEvent};

use crate::db::{begin_tenant_transaction, now_iso, Db};
use crate::error::{AppError, AppResult};
use crate::repo::access::{lock_current_scope_tx, require_permission_tx, ScopeBindings};
use crate::repo::orders::next_outbox_sequence_tx;

const PERMISSION: &str = "admin";

fn internal(error: impl std::fmt::Display) -> AppError {
    AppError::internal(error.to_string())
}

const fn kind_name(kind: DecisionRuleKind) -> &'static str {
    match kind {
        DecisionRuleKind::Receipt => "receipt",
        DecisionRuleKind::Putaway => "putaway",
        DecisionRuleKind::Allocation => "allocation",
        DecisionRuleKind::Replenishment => "replenishment",
        DecisionRuleKind::Wave => "wave",
        DecisionRuleKind::Pick => "pick",
        DecisionRuleKind::Pack => "pack",
        DecisionRuleKind::Count => "count",
        DecisionRuleKind::Document => "document",
        DecisionRuleKind::Billing => "billing",
    }
}

fn parse_kind(value: &str) -> AppResult<DecisionRuleKind> {
    match value {
        "receipt" => Ok(DecisionRuleKind::Receipt),
        "putaway" => Ok(DecisionRuleKind::Putaway),
        "allocation" => Ok(DecisionRuleKind::Allocation),
        "replenishment" => Ok(DecisionRuleKind::Replenishment),
        "wave" => Ok(DecisionRuleKind::Wave),
        "pick" => Ok(DecisionRuleKind::Pick),
        "pack" => Ok(DecisionRuleKind::Pack),
        "count" => Ok(DecisionRuleKind::Count),
        "document" => Ok(DecisionRuleKind::Document),
        "billing" => Ok(DecisionRuleKind::Billing),
        _ => Err(AppError::internal("invalid configuration kind")),
    }
}

const fn status_name(status: ConfigurationStatus) -> &'static str {
    match status {
        ConfigurationStatus::Draft => "draft",
        ConfigurationStatus::PendingApproval => "pending_approval",
        ConfigurationStatus::Approved => "approved",
        ConfigurationStatus::Active => "active",
        ConfigurationStatus::Retired => "retired",
    }
}

fn parse_status(value: &str) -> AppResult<ConfigurationStatus> {
    match value {
        "draft" => Ok(ConfigurationStatus::Draft),
        "pending_approval" => Ok(ConfigurationStatus::PendingApproval),
        "approved" => Ok(ConfigurationStatus::Approved),
        "active" => Ok(ConfigurationStatus::Active),
        "retired" => Ok(ConfigurationStatus::Retired),
        _ => Err(AppError::internal("invalid configuration status")),
    }
}

const fn scope_values(scope: ConfigurationScope) -> (&'static str, Option<i64>, Option<i64>) {
    match scope {
        ConfigurationScope::Tenant => ("tenant", None, None),
        ConfigurationScope::InventoryOwner { inventory_owner_id } => {
            ("inventory_owner", Some(inventory_owner_id.get()), None)
        }
        ConfigurationScope::Facility { facility_id } => ("facility", None, Some(facility_id.get())),
        ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => (
            "owner_facility",
            Some(inventory_owner_id.get()),
            Some(facility_id.get()),
        ),
    }
}

fn scope_from_row(row: &sqlx::postgres::PgRow) -> AppResult<ConfigurationScope> {
    let owner_id = row.try_get::<Option<i64>, _>("inventory_owner_id")?;
    let facility_id = row.try_get::<Option<i64>, _>("facility_id")?;
    match row.try_get::<String, _>("scope_level")?.as_str() {
        "tenant" if owner_id.is_none() && facility_id.is_none() => Ok(ConfigurationScope::Tenant),
        "inventory_owner" if owner_id.is_some() && facility_id.is_none() => {
            Ok(ConfigurationScope::InventoryOwner {
                inventory_owner_id: InventoryOwnerId::new(owner_id.unwrap_or_default())
                    .map_err(internal)?,
            })
        }
        "facility" if owner_id.is_none() && facility_id.is_some() => {
            Ok(ConfigurationScope::Facility {
                facility_id: FacilityId::new(facility_id.unwrap_or_default()).map_err(internal)?,
            })
        }
        "owner_facility" if owner_id.is_some() && facility_id.is_some() => {
            Ok(ConfigurationScope::OwnerFacility {
                inventory_owner_id: InventoryOwnerId::new(owner_id.unwrap_or_default())
                    .map_err(internal)?,
                facility_id: FacilityId::new(facility_id.unwrap_or_default()).map_err(internal)?,
            })
        }
        _ => Err(AppError::internal("invalid stored configuration scope")),
    }
}

fn model_from_row(row: &sqlx::postgres::PgRow) -> AppResult<ConfigurationReadModel> {
    let definition = serde_json::from_value::<DecisionRuleDefinition>(row.try_get("definition")?)
        .map_err(internal)?;
    definition.validate().map_err(internal)?;
    let stored_kind = parse_kind(&row.try_get::<String, _>("kind")?)?;
    if definition.kind() != stored_kind {
        return Err(AppError::internal("stored configuration kind mismatch"));
    }
    Ok(ConfigurationReadModel {
        configuration_id: ConfigurationVersionId::new(row.try_get("id")?).map_err(internal)?,
        revision: row.try_get("revision")?,
        scope: scope_from_row(row)?,
        status: parse_status(&row.try_get::<String, _>("status")?)?,
        effective_window: ConfigurationEffectiveWindow::new(
            row.try_get("effective_from")?,
            row.try_get("effective_until")?,
        )
        .map_err(internal)?,
        definition,
        created_by: UserId::new(row.try_get("created_by_user_id")?).map_err(internal)?,
        created_at: row.try_get("created_at")?,
        submitted_by: optional_user(row, "submitted_by_user_id")?,
        submitted_at: row.try_get("submitted_at")?,
        approved_by: optional_user(row, "approved_by_user_id")?,
        approved_at: row.try_get("approved_at")?,
        activated_by: optional_user(row, "activated_by_user_id")?,
        activated_at: row.try_get("activated_at")?,
        retired_by: optional_user(row, "retired_by_user_id")?,
        retired_at: row.try_get("retired_at")?,
        rollback_of_configuration_id: row
            .try_get::<Option<i64>, _>("rollback_of_configuration_id")?
            .map(ConfigurationVersionId::new)
            .transpose()
            .map_err(internal)?,
    })
}

fn optional_user(row: &sqlx::postgres::PgRow, column: &str) -> AppResult<Option<UserId>> {
    row.try_get::<Option<i64>, _>(column)?
        .map(UserId::new)
        .transpose()
        .map_err(internal)
}

fn require_scope(scope: &ScopeBindings, configured: ConfigurationScope) -> AppResult<()> {
    let visible = match configured {
        ConfigurationScope::Tenant => true,
        ConfigurationScope::InventoryOwner { inventory_owner_id } => {
            scope.includes_inventory_owner(inventory_owner_id.get())
        }
        ConfigurationScope::Facility { facility_id } => scope.includes_facility(facility_id.get()),
        ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => {
            scope.includes_inventory_owner(inventory_owner_id.get())
                && scope.includes_facility(facility_id.get())
        }
    };
    if visible {
        Ok(())
    } else {
        Err(AppError::not_found("configuration"))
    }
}

async fn validate_scope_references_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    scope: ConfigurationScope,
) -> AppResult<()> {
    let valid = match scope {
        ConfigurationScope::Tenant => true,
        ConfigurationScope::InventoryOwner {
            inventory_owner_id,
        } => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM inventory_owners WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL)",
            )
            .bind(tenant_id.get())
            .bind(inventory_owner_id.get())
            .fetch_one(&mut **tx)
            .await?
        }
        ConfigurationScope::Facility { facility_id } => {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM facilities WHERE tenant_id=$1 AND id=$2 AND deleted IS NULL)",
            )
            .bind(tenant_id.get())
            .bind(facility_id.get())
            .fetch_one(&mut **tx)
            .await?
        }
        ConfigurationScope::OwnerFacility {
            inventory_owner_id,
            facility_id,
        } => {
            sqlx::query_scalar::<_, bool>(
                r#"
                SELECT EXISTS(
                    SELECT 1 FROM inventory_owner_facilities link
                    JOIN inventory_owners owner ON owner.tenant_id=link.tenant_id
                      AND owner.id=link.inventory_owner_id AND owner.deleted IS NULL
                    JOIN facilities facility ON facility.tenant_id=link.tenant_id
                      AND facility.id=link.facility_id AND facility.deleted IS NULL
                    WHERE link.tenant_id=$1 AND link.inventory_owner_id=$2
                      AND link.facility_id=$3 AND link.deleted IS NULL)
                "#,
            )
            .bind(tenant_id.get())
            .bind(inventory_owner_id.get())
            .bind(facility_id.get())
            .fetch_one(&mut **tx)
            .await?
        }
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::not_found("configuration scope"))
    }
}

async fn lock_natural_key_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    kind: DecisionRuleKind,
    scope: ConfigurationScope,
) -> AppResult<()> {
    let (scope_level, owner_id, facility_id) = scope_values(scope);
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "configuration:{}:{}:{}:{}:{}",
            tenant_id.get(),
            kind_name(kind),
            scope_level,
            owner_id.unwrap_or_default(),
            facility_id.unwrap_or_default()
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn read_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    configuration_id: ConfigurationVersionId,
) -> AppResult<ConfigurationReadModel> {
    let row = sqlx::query("SELECT * FROM configuration_versions WHERE tenant_id=$1 AND id=$2")
        .bind(tenant_id.get())
        .bind(configuration_id.get())
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| AppError::not_found("configuration"))?;
    model_from_row(&row)
}

async fn replay_scope_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    prepared: &PreparedCommand,
    scope: &ScopeBindings,
) -> AppResult<()> {
    let row = sqlx::query(
        r#"
        SELECT configuration.scope_level,configuration.inventory_owner_id,
               configuration.facility_id
        FROM command_idempotency_records command
        JOIN configuration_versions configuration
          ON configuration.tenant_id=command.tenant_id
         AND configuration.id=(command.result_json->>'configuration_id')::BIGINT
        WHERE command.tenant_id=$1 AND command.operation=$2 AND command.idempotency_key=$3
        "#,
    )
    .bind(prepared.tenant_id().get())
    .bind(prepared.operation().as_str())
    .bind(prepared.idempotency_key())
    .fetch_optional(&mut **tx)
    .await?;
    if let Some(row) = row {
        require_scope(scope, scope_from_row(&row)?)?;
    }
    Ok(())
}

async fn enqueue_event_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    actor_id: UserId,
    result: &ConfigurationReadModel,
    transition: &str,
    occurred_at: Timestamp,
) -> AppResult<()> {
    let owner_id = match result.scope {
        ConfigurationScope::InventoryOwner { inventory_owner_id }
        | ConfigurationScope::OwnerFacility {
            inventory_owner_id, ..
        } => Some(inventory_owner_id),
        _ => None,
    };
    let facility_id = match result.scope {
        ConfigurationScope::Facility { facility_id }
        | ConfigurationScope::OwnerFacility { facility_id, .. } => Some(facility_id),
        _ => None,
    };
    let event_key = format!(
        "configuration:{}:{}:{transition}",
        result.configuration_id.get(),
        result.revision
    );
    let aggregate_id = result.configuration_id.get().to_string();
    let ordering_key = format!("configuration:{}", result.configuration_id.get());
    let event_type = format!("configuration.version.{transition}");
    let sequence = next_outbox_sequence_tx(tx, tenant_id, &ordering_key).await?;
    let payload = serde_json::to_value(result).map_err(internal)?;
    outbox::enqueue(
        tx,
        &NewOutboxEvent {
            tenant_id,
            inventory_owner_id: owner_id,
            facility_id,
            actor_user_id: Some(actor_id.get()),
            event_key: &event_key,
            aggregate_type: "configuration_version",
            aggregate_id: &aggregate_id,
            ordering_key: &ordering_key,
            aggregate_sequence: sequence,
            event_type: &event_type,
            schema_version: 1,
            payload: &payload,
            occurred_at,
        },
    )
    .await?;
    Ok(())
}

pub async fn create_configuration(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateConfigurationCommand,
) -> AppResult<CreateConfigurationResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    command.definition.validate().map_err(internal)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_CONFIGURATION_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    replay_scope_tx(&mut tx, &prepared, &scope).await?;
    require_scope(&scope, command.scope)?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    validate_scope_references_tx(&mut tx, access.tenant_id, command.scope).await?;
    let kind = command.definition.kind();
    lock_natural_key_tx(&mut tx, access.tenant_id, kind, command.scope).await?;
    let (scope_level, owner_id, facility_id) = scope_values(command.scope);
    let latest_revision: Option<i64> = sqlx::query_scalar(
        r#"
        SELECT max(revision) FROM configuration_versions
        WHERE tenant_id=$1 AND kind=$2 AND scope_level=$3
          AND inventory_owner_id IS NOT DISTINCT FROM $4
          AND facility_id IS NOT DISTINCT FROM $5
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(kind_name(kind))
    .bind(scope_level)
    .bind(owner_id)
    .bind(facility_id)
    .fetch_one(&mut *tx)
    .await?;
    let revision = match (latest_revision, command.expected_revision) {
        (None, None) => 1,
        (Some(latest), Some(expected)) if latest == expected => latest
            .checked_add(1)
            .ok_or_else(|| AppError::internal("configuration revision overflow"))?,
        (None, Some(_)) => {
            return Err(AppError::conflict(
                "configuration does not have a prior revision",
            ));
        }
        (Some(_), None) => {
            return Err(AppError::conflict(
                "expected_revision is required for an existing configuration scope",
            ));
        }
        (Some(_), Some(_)) => {
            return Err(AppError::conflict("configuration revision does not match"));
        }
    };
    let created_at = now_iso();
    let definition = serde_json::to_value(&command.definition).map_err(internal)?;
    let configuration_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO configuration_versions
          (tenant_id,kind,scope_level,inventory_owner_id,facility_id,revision,status,
           effective_from,effective_until,definition,created_by_user_id,created_at)
        VALUES ($1,$2,$3,$4,$5,$6,'draft',$7,$8,$9,$10,$11)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(kind_name(kind))
    .bind(scope_level)
    .bind(owner_id)
    .bind(facility_id)
    .bind(revision)
    .bind(command.effective_window.effective_from)
    .bind(command.effective_window.effective_until)
    .bind(definition)
    .bind(context.actor_id.get())
    .bind(created_at)
    .fetch_one(&mut *tx)
    .await?;
    let result = read_tx(
        &mut tx,
        access.tenant_id,
        ConfigurationVersionId::new(configuration_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "created",
        created_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

#[derive(Debug, Clone, Copy)]
enum LifecycleTransition {
    Submit,
    Approve,
    Retire,
}

impl LifecycleTransition {
    const fn operation(self) -> &'static str {
        match self {
            Self::Submit => SUBMIT_CONFIGURATION_OPERATION,
            Self::Approve => APPROVE_CONFIGURATION_OPERATION,
            Self::Retire => RETIRE_CONFIGURATION_OPERATION,
        }
    }

    const fn event_name(self) -> &'static str {
        match self {
            Self::Submit => "submitted",
            Self::Approve => "approved",
            Self::Retire => "retired",
        }
    }
}

async fn transition_configuration(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigurationLifecycleCommand,
    transition: LifecycleTransition,
) -> AppResult<ConfigurationReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, transition.operation(), command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    replay_scope_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let row =
        sqlx::query("SELECT * FROM configuration_versions WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
            .bind(access.tenant_id.get())
            .bind(command.configuration_id.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::not_found("configuration"))?;
    let current = model_from_row(&row)?;
    require_scope(&scope, current.scope)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict("configuration revision does not match"));
    }
    let now = now_iso();
    match transition {
        LifecycleTransition::Submit => {
            current
                .status
                .submit()
                .map_err(|error| AppError::conflict(error.to_string()))?;
            sqlx::query(
                r#"UPDATE configuration_versions
                   SET status='pending_approval',revision=revision+1,
                       submitted_by_user_id=$3,submitted_at=$4
                   WHERE tenant_id=$1 AND id=$2"#,
            )
            .bind(access.tenant_id.get())
            .bind(command.configuration_id.get())
            .bind(context.actor_id.get())
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        LifecycleTransition::Approve => {
            current
                .status
                .approve()
                .map_err(|error| AppError::conflict(error.to_string()))?;
            if current.created_by == context.actor_id {
                return Err(AppError::conflict(
                    "configuration approval requires a different administrator",
                ));
            }
            sqlx::query(
                r#"UPDATE configuration_versions
                   SET status='approved',revision=revision+1,
                       approved_by_user_id=$3,approved_at=$4
                   WHERE tenant_id=$1 AND id=$2"#,
            )
            .bind(access.tenant_id.get())
            .bind(command.configuration_id.get())
            .bind(context.actor_id.get())
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        LifecycleTransition::Retire => {
            current
                .status
                .retire()
                .map_err(|error| AppError::conflict(error.to_string()))?;
            sqlx::query(
                r#"UPDATE configuration_versions
                   SET status='retired',revision=revision+1,
                       retired_by_user_id=$3,retired_at=$4
                   WHERE tenant_id=$1 AND id=$2"#,
            )
            .bind(access.tenant_id.get())
            .bind(command.configuration_id.get())
            .bind(context.actor_id.get())
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
    }
    let result = read_tx(&mut tx, access.tenant_id, command.configuration_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        transition.event_name(),
        now,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn submit_configuration(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigurationLifecycleCommand,
) -> AppResult<SubmitConfigurationResult> {
    transition_configuration(db, access, context, command, LifecycleTransition::Submit).await
}

pub async fn approve_configuration(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigurationLifecycleCommand,
) -> AppResult<ApproveConfigurationResult> {
    transition_configuration(db, access, context, command, LifecycleTransition::Approve).await
}

pub async fn retire_configuration(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ConfigurationLifecycleCommand,
) -> AppResult<RetireConfigurationResult> {
    transition_configuration(db, access, context, command, LifecycleTransition::Retire).await
}

pub async fn activate_configuration(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ActivateConfigurationCommand,
) -> AppResult<ActivateConfigurationResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, ACTIVATE_CONFIGURATION_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    replay_scope_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let row =
        sqlx::query("SELECT * FROM configuration_versions WHERE tenant_id=$1 AND id=$2 FOR UPDATE")
            .bind(access.tenant_id.get())
            .bind(command.configuration_id.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::not_found("configuration"))?;
    let current = model_from_row(&row)?;
    require_scope(&scope, current.scope)?;
    if current.revision != command.expected_revision {
        return Err(AppError::conflict("configuration revision does not match"));
    }
    let activated_at = now_iso();
    current
        .status
        .activate(current.effective_window, activated_at)
        .map_err(|error| AppError::conflict(error.to_string()))?;
    let (scope_level, owner_id, facility_id) = scope_values(current.scope);
    let overlapping = sqlx::query_scalar::<_, i64>(
        r#"
        SELECT id FROM configuration_versions
        WHERE tenant_id=$1 AND id<>$2 AND kind=$3 AND scope_level=$4
          AND inventory_owner_id IS NOT DISTINCT FROM $5
          AND facility_id IS NOT DISTINCT FROM $6 AND status='active'
          AND tstzrange(effective_from,effective_until,'[)') &&
              tstzrange($7,$8,'[)')
        ORDER BY id FOR UPDATE
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(command.configuration_id.get())
    .bind(kind_name(current.definition.kind()))
    .bind(scope_level)
    .bind(owner_id)
    .bind(facility_id)
    .bind(current.effective_window.effective_from)
    .bind(current.effective_window.effective_until)
    .fetch_all(&mut *tx)
    .await?;
    for configuration_id in overlapping {
        sqlx::query(
            r#"UPDATE configuration_versions
               SET status='retired',revision=revision+1,
                   retired_by_user_id=$3,retired_at=$4
               WHERE tenant_id=$1 AND id=$2"#,
        )
        .bind(access.tenant_id.get())
        .bind(configuration_id)
        .bind(context.actor_id.get())
        .bind(activated_at)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        r#"UPDATE configuration_versions
           SET status='active',revision=revision+1,
               activated_by_user_id=$3,activated_at=$4
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.configuration_id.get())
    .bind(context.actor_id.get())
    .bind(activated_at)
    .execute(&mut *tx)
    .await?;
    let result = read_tx(&mut tx, access.tenant_id, command.configuration_id).await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "activated",
        activated_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn rollback_configuration(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &RollbackConfigurationCommand,
) -> AppResult<RollbackConfigurationResult> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, ROLLBACK_CONFIGURATION_OPERATION, command)?;
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id.get(),
        PERMISSION,
    )
    .await?;
    replay_scope_tx(&mut tx, &prepared, &scope).await?;
    if let Some(result) = prepared.replayed(&mut tx).await? {
        tx.commit().await?;
        return Ok(result);
    }
    let source_row =
        sqlx::query("SELECT * FROM configuration_versions WHERE tenant_id=$1 AND id=$2 FOR SHARE")
            .bind(access.tenant_id.get())
            .bind(command.source_configuration_id.get())
            .fetch_optional(&mut *tx)
            .await?
            .ok_or_else(|| AppError::not_found("configuration"))?;
    let source = model_from_row(&source_row)?;
    require_scope(&scope, source.scope)?;
    if source.revision != command.expected_source_revision {
        return Err(AppError::conflict("configuration revision does not match"));
    }
    if !matches!(
        source.status,
        ConfigurationStatus::Approved | ConfigurationStatus::Active | ConfigurationStatus::Retired
    ) {
        return Err(AppError::conflict(
            "only approved configuration history can be rolled back",
        ));
    }
    let kind = source.definition.kind();
    lock_natural_key_tx(&mut tx, access.tenant_id, kind, source.scope).await?;
    let (scope_level, owner_id, facility_id) = scope_values(source.scope);
    let latest_revision: i64 = sqlx::query_scalar(
        r#"
        SELECT max(revision) FROM configuration_versions
        WHERE tenant_id=$1 AND kind=$2 AND scope_level=$3
          AND inventory_owner_id IS NOT DISTINCT FROM $4
          AND facility_id IS NOT DISTINCT FROM $5
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(kind_name(kind))
    .bind(scope_level)
    .bind(owner_id)
    .bind(facility_id)
    .fetch_one(&mut *tx)
    .await?;
    let revision = latest_revision
        .checked_add(1)
        .ok_or_else(|| AppError::internal("configuration revision overflow"))?;
    let created_at = now_iso();
    let definition = serde_json::to_value(&source.definition).map_err(internal)?;
    let configuration_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO configuration_versions
          (tenant_id,kind,scope_level,inventory_owner_id,facility_id,revision,status,
           effective_from,effective_until,definition,created_by_user_id,created_at,
           rollback_of_configuration_id)
        VALUES ($1,$2,$3,$4,$5,$6,'draft',$7,$8,$9,$10,$11,$12)
        RETURNING id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(kind_name(kind))
    .bind(scope_level)
    .bind(owner_id)
    .bind(facility_id)
    .bind(revision)
    .bind(command.effective_window.effective_from)
    .bind(command.effective_window.effective_until)
    .bind(definition)
    .bind(context.actor_id.get())
    .bind(created_at)
    .bind(source.configuration_id.get())
    .fetch_one(&mut *tx)
    .await?;
    let result = read_tx(
        &mut tx,
        access.tenant_id,
        ConfigurationVersionId::new(configuration_id).map_err(internal)?,
    )
    .await?;
    enqueue_event_tx(
        &mut tx,
        access.tenant_id,
        context.actor_id,
        &result,
        "rollback_created",
        created_at,
    )
    .await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn configuration_page(
    db: &Db,
    access: &TenantAccess,
    query: ConfigurationPageQuery,
) -> AppResult<ConfigurationPage> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    if query
        .inventory_owner_id
        .is_some_and(|id| !scope.includes_inventory_owner(id.get()))
        || query
            .facility_id
            .is_some_and(|id| !scope.includes_facility(id.get()))
    {
        return Err(AppError::not_found("configuration"));
    }
    let rows = sqlx::query(
        r#"
        SELECT * FROM configuration_versions configuration
        WHERE tenant_id=$1
          AND ($2 OR inventory_owner_id IS NULL OR inventory_owner_id=ANY($3))
          AND ($4 OR facility_id IS NULL OR facility_id=ANY($5))
          AND ($6::TEXT IS NULL OR kind=$6)
          AND ($7::TEXT IS NULL OR status=$7)
          AND ($8::BIGINT IS NULL OR inventory_owner_id=$8)
          AND ($9::BIGINT IS NULL OR facility_id=$9)
          AND ($10::BIGINT IS NULL OR id<$10)
        ORDER BY id DESC LIMIT $11
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(query.kind.map(kind_name))
    .bind(query.status.map(status_name))
    .bind(query.inventory_owner_id.map(InventoryOwnerId::get))
    .bind(query.facility_id.map(FacilityId::get))
    .bind(
        query
            .cursor
            .map(|cursor| cursor.after_configuration_id.get()),
    )
    .bind(i64::from(query.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows
        .iter()
        .map(model_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    let next_cursor = if items.len() > usize::from(query.limit) {
        items.truncate(usize::from(query.limit));
        items.last().map(|item| ConfigurationCursor {
            after_configuration_id: item.configuration_id,
        })
    } else {
        None
    };
    tx.commit().await?;
    Ok(ConfigurationPage { items, next_cursor })
}

pub async fn simulate_configuration(
    db: &Db,
    access: &TenantAccess,
    query: SimulateConfigurationQuery,
) -> AppResult<ConfigurationSimulationResult> {
    let mut tx = begin_tenant_transaction(db, access.tenant_id).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), PERMISSION).await?;
    if !scope.includes_inventory_owner(query.inventory_owner_id.get())
        || !scope.includes_facility(query.facility_id.get())
    {
        return Err(AppError::not_found("configuration"));
    }
    validate_scope_references_tx(
        &mut tx,
        access.tenant_id,
        ConfigurationScope::OwnerFacility {
            inventory_owner_id: query.inventory_owner_id,
            facility_id: query.facility_id,
        },
    )
    .await?;
    let rows = sqlx::query(
        r#"
        SELECT * FROM configuration_versions
        WHERE tenant_id=$1 AND kind=$2 AND status='active'
          AND effective_from<=$3 AND (effective_until IS NULL OR effective_until>$3)
          AND (inventory_owner_id IS NULL OR inventory_owner_id=$4)
          AND (facility_id IS NULL OR facility_id=$5)
        ORDER BY id
        "#,
    )
    .bind(access.tenant_id.get())
    .bind(kind_name(query.kind))
    .bind(query.effective_at)
    .bind(query.inventory_owner_id.get())
    .bind(query.facility_id.get())
    .fetch_all(&mut *tx)
    .await?;
    let models = rows
        .iter()
        .map(model_from_row)
        .collect::<AppResult<Vec<_>>>()?;
    let candidates = models
        .iter()
        .map(|model| EffectiveDecisionRule {
            configuration_id: model.configuration_id.get(),
            revision: model.revision,
            scope: model.scope,
            status: model.status,
            effective_window: model.effective_window,
            definition: model.definition.clone(),
        })
        .collect::<Vec<_>>();
    let matched_id = resolve_effective_rule(
        &candidates,
        query.kind,
        query.inventory_owner_id,
        query.facility_id,
        query.effective_at,
    )
    .map_err(internal)?
    .map(|rule| rule.configuration_id);
    let matched_configuration = matched_id.and_then(|id| {
        models
            .iter()
            .find(|model| model.configuration_id.get() == id)
            .cloned()
    });
    let evaluated_candidate_count =
        u32::try_from(models.len()).map_err(|_| AppError::internal("too many configurations"))?;
    tx.commit().await?;
    Ok(ConfigurationSimulationResult {
        kind: query.kind,
        inventory_owner_id: query.inventory_owner_id,
        facility_id: query.facility_id,
        effective_at: query.effective_at,
        matched_configuration,
        evaluated_candidate_count,
    })
}
