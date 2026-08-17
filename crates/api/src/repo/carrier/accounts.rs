use wareboxes_application::carrier::{
    CarrierAccountReadModel, ChangeCarrierAccountStatusCommand, CreateCarrierAccountCommand,
    ReconfigureCarrierAccountCommand, CHANGE_CARRIER_ACCOUNT_STATUS_OPERATION,
    CREATE_CARRIER_ACCOUNT_OPERATION, RECONFIGURE_CARRIER_ACCOUNT_OPERATION,
};
use wareboxes_application::idempotency::PreparedCommand;
use wareboxes_application::CommandContext;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{CarrierAccountId, FacilityId, InventoryOwnerId};
use wareboxes_persistence_postgres::db::{bind_tenant_context, now_iso, Db};
use wareboxes_persistence_postgres::idempotency::PostgresPreparedCommandExt;

use crate::error::{AppError, AppResult};
use crate::repo::access::{current_scope_tx, lock_current_scope_tx, require_permission_tx};

use super::mapping::{self, ACCOUNT_COLUMNS};
use super::{bind_actor_tx, insert_outbox_tx, CarrierEvent};

const MAX_PAGE: u16 = 100;

#[derive(Debug, Clone, Copy)]
pub struct CarrierAccountPageFilter {
    pub inventory_owner_id: InventoryOwnerId,
    pub facility_id: FacilityId,
    pub include_disabled: bool,
    pub after_account_id: Option<CarrierAccountId>,
    pub limit: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarrierAccountPage {
    pub items: Vec<CarrierAccountReadModel>,
    pub next_account_id: Option<CarrierAccountId>,
}

pub async fn create(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &CreateCarrierAccountCommand,
) -> AppResult<CarrierAccountReadModel> {
    context.require_actor(access.tenant_id, access.user_id)?;
    let prepared = PreparedCommand::new_v1(context, CREATE_CARRIER_ACCOUNT_OPERATION, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "admin").await?;
    require_dimensions(
        &mut tx,
        access,
        &scope,
        command.inventory_owner_id,
        command.facility_id,
    )
    .await?;
    if let Some(result) = prepared
        .replayed::<CarrierAccountReadModel>(&mut tx)
        .await?
    {
        require_read_model_visible(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    lock_natural_key(
        &mut tx,
        access.tenant_id.get(),
        command.inventory_owner_id.get(),
        command.facility_id.get(),
        command.carrier_code.as_str(),
    )
    .await?;
    let exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM carrier_accounts
           WHERE tenant_id=$1 AND inventory_owner_id=$2 AND facility_id=$3
             AND carrier_code=$4)"#,
    )
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(command.carrier_code.as_str())
    .fetch_one(&mut *tx)
    .await?;
    if exists {
        return Err(AppError::conflict(
            "carrier account already exists for this client, facility, and carrier",
        ));
    }
    let now = now_iso();
    let row = sqlx::query(&format!(
        r#"INSERT INTO carrier_accounts AS account
           (tenant_id,inventory_owner_id,facility_id,display_name,carrier_code,
            account_key,status,revision,configured_by_user_id,configured_at,
            updated_by_user_id,updated_at)
           VALUES($1,$2,$3,$4,$5,$6,'active',1,$7,$8,$7,$8)
           RETURNING {}"#,
        ACCOUNT_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(command.inventory_owner_id.get())
    .bind(command.facility_id.get())
    .bind(command.display_name.as_str())
    .bind(command.carrier_code.as_str())
    .bind(command.account_key.as_str())
    .bind(context.actor_id.get())
    .bind(now)
    .fetch_one(&mut *tx)
    .await?;
    let result = mapping::account(&row)?;
    insert_version(&mut tx, &result).await?;
    insert_account_event(&mut tx, &result, context.actor_id.get(), "created", now).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn reconfigure(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ReconfigureCarrierAccountCommand,
) -> AppResult<CarrierAccountReadModel> {
    let display_name = command.display_name.as_str().to_owned();
    let account_key = command.account_key.as_str().to_owned();
    mutate(
        db,
        access,
        context,
        RECONFIGURE_CARRIER_ACCOUNT_OPERATION,
        command,
        command.account_id,
        command.expected_revision,
        move |query| query.bind(display_name).bind(account_key),
        r#"UPDATE carrier_accounts AS account
           SET display_name=$4,account_key=$5,revision=revision+1,
               updated_by_user_id=$6,updated_at=$7
           WHERE tenant_id=$1 AND id=$2 AND revision=$3
           RETURNING "#,
        "reconfigured",
    )
    .await
}

pub async fn change_status(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    command: &ChangeCarrierAccountStatusCommand,
) -> AppResult<CarrierAccountReadModel> {
    mutate(
        db,
        access,
        context,
        CHANGE_CARRIER_ACCOUNT_STATUS_OPERATION,
        command,
        command.account_id,
        command.expected_revision,
        |query| query.bind(command.status.as_str()),
        r#"UPDATE carrier_accounts AS account
           SET status=$4,revision=revision+1,updated_by_user_id=$5,updated_at=$6
           WHERE tenant_id=$1 AND id=$2 AND revision=$3
           RETURNING "#,
        "status_changed",
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn mutate<C, F>(
    db: &Db,
    access: &TenantAccess,
    context: &CommandContext,
    operation: &'static str,
    command: &C,
    account_id: CarrierAccountId,
    expected_revision: u32,
    bind_changes: F,
    update_prefix: &str,
    event: &'static str,
) -> AppResult<CarrierAccountReadModel>
where
    C: serde::Serialize,
    F: for<'q> FnOnce(
        sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    ) -> sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
{
    context.require_actor(access.tenant_id, access.user_id)?;
    if expected_revision == 0 {
        return Err(AppError::bad_request("expected revision must be positive"));
    }
    let prepared = PreparedCommand::new_v1(context, operation, command)?;
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    bind_actor_tx(&mut tx, context.actor_id.get()).await?;
    let scope = lock_current_scope_tx(&mut tx, access.tenant_id, context.actor_id.get()).await?;
    require_permission_tx(&mut tx, access.tenant_id, context.actor_id.get(), "admin").await?;
    if let Some(result) = prepared
        .replayed::<CarrierAccountReadModel>(&mut tx)
        .await?
    {
        require_read_model_visible(&scope, &result)?;
        tx.commit().await?;
        return Ok(result);
    }
    let current = lock_account(&mut tx, access, account_id, &scope).await?;
    if current.revision != expected_revision {
        return Err(AppError::conflict("carrier account revision is stale"));
    }
    let now = now_iso();
    let sql = format!("{update_prefix}{ACCOUNT_COLUMNS}");
    let query = sqlx::query(&sql)
        .bind(access.tenant_id.get())
        .bind(account_id.get())
        .bind(i32::try_from(expected_revision).map_err(|_| {
            AppError::bad_request("carrier account revision exceeds the supported range")
        })?);
    let query = bind_changes(query);
    let row = query
        .bind(context.actor_id.get())
        .bind(now)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| AppError::conflict("carrier account changed concurrently"))?;
    let result = mapping::account(&row)?;
    insert_version(&mut tx, &result).await?;
    insert_account_event(&mut tx, &result, context.actor_id.get(), event, now).await?;
    Ok(prepared.commit(tx, result).await?)
}

pub async fn list(
    db: &Db,
    access: &TenantAccess,
    filter: CarrierAccountPageFilter,
) -> AppResult<CarrierAccountPage> {
    if filter.limit == 0 || filter.limit > MAX_PAGE {
        return Err(AppError::bad_request(
            "carrier account page size is outside the supported range",
        ));
    }
    let mut tx = db.begin().await?;
    bind_tenant_context(&mut tx, access.tenant_id).await?;
    require_permission_tx(&mut tx, access.tenant_id, access.user_id.get(), "wms").await?;
    let scope = current_scope_tx(&mut tx, access.tenant_id, access.user_id.get()).await?;
    require_dimensions(
        &mut tx,
        access,
        &scope,
        filter.inventory_owner_id,
        filter.facility_id,
    )
    .await?;
    let rows = sqlx::query(&format!(
        r#"SELECT {} FROM carrier_accounts account
           WHERE account.tenant_id=$1 AND account.inventory_owner_id=$2
             AND account.facility_id=$3 AND ($4 OR account.status='active')
             AND account.id>$5 ORDER BY account.id LIMIT $6"#,
        ACCOUNT_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(filter.inventory_owner_id.get())
    .bind(filter.facility_id.get())
    .bind(filter.include_disabled)
    .bind(filter.after_account_id.map_or(0, CarrierAccountId::get))
    .bind(i64::from(filter.limit) + 1)
    .fetch_all(&mut *tx)
    .await?;
    let mut items = rows
        .iter()
        .map(mapping::account)
        .collect::<AppResult<Vec<_>>>()?;
    let has_more = items.len() > usize::from(filter.limit);
    if has_more {
        items.pop();
    }
    let next_account_id = has_more
        .then(|| items.last().map(|item| item.account_id))
        .flatten();
    tx.commit().await?;
    Ok(CarrierAccountPage {
        items,
        next_account_id,
    })
}

async fn lock_account(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    account_id: CarrierAccountId,
    scope: &crate::repo::access::ScopeBindings,
) -> AppResult<CarrierAccountReadModel> {
    let row = sqlx::query(&format!(
        r#"SELECT {} FROM carrier_accounts account
           WHERE account.tenant_id=$1 AND account.id=$2
             AND ($3 OR account.facility_id=ANY($4))
             AND ($5 OR account.inventory_owner_id=ANY($6))
           FOR UPDATE OF account"#,
        ACCOUNT_COLUMNS
    ))
    .bind(access.tenant_id.get())
    .bind(account_id.get())
    .bind(scope.all_facilities)
    .bind(&scope.facility_ids)
    .bind(scope.all_inventory_owners)
    .bind(&scope.inventory_owner_ids)
    .fetch_optional(&mut **tx)
    .await?
    .ok_or_else(|| AppError::not_found("carrier account"))?;
    mapping::account(&row)
}

async fn insert_version(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account: &CarrierAccountReadModel,
) -> AppResult<()> {
    sqlx::query(
        r#"INSERT INTO carrier_account_versions
           (tenant_id,inventory_owner_id,facility_id,carrier_account_id,revision,
            display_name,carrier_code,account_key,status,changed_by_user_id,changed_at)
           VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)"#,
    )
    .bind(account.tenant_id.get())
    .bind(account.inventory_owner_id.get())
    .bind(account.facility_id.get())
    .bind(account.account_id.get())
    .bind(
        i32::try_from(account.revision)
            .map_err(|_| AppError::internal("carrier account revision exceeds database range"))?,
    )
    .bind(account.display_name.as_str())
    .bind(account.carrier_code.as_str())
    .bind(account.account_key.as_str())
    .bind(account.status.as_str())
    .bind(account.updated_by.get())
    .bind(account.updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn insert_account_event(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    account: &CarrierAccountReadModel,
    actor_user_id: i64,
    transition: &str,
    occurred_at: wareboxes_domain::Timestamp,
) -> AppResult<()> {
    let payload = serde_json::to_value(account)
        .map_err(|error| AppError::internal(format!("serializing carrier account: {error}")))?;
    insert_outbox_tx(
        tx,
        CarrierEvent {
            tenant_id: account.tenant_id,
            inventory_owner_id: account.inventory_owner_id,
            facility_id: account.facility_id,
            actor_user_id,
            aggregate_type: "carrier_account",
            aggregate_id: account.account_id.get().to_string(),
            event_type: match transition {
                "created" => "carrier.account.created",
                "reconfigured" => "carrier.account.reconfigured",
                _ => "carrier.account.status_changed",
            },
            event_key: format!(
                "carrier-account:{}:{}:{}",
                account.account_id.get(),
                account.revision,
                transition
            ),
            payload: &payload,
            occurred_at,
        },
    )
    .await
}

async fn require_dimensions(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    access: &TenantAccess,
    scope: &crate::repo::access::ScopeBindings,
    inventory_owner_id: InventoryOwnerId,
    facility_id: FacilityId,
) -> AppResult<()> {
    if !scope.includes_inventory_owner(inventory_owner_id.get())
        || !scope.includes_facility(facility_id.get())
    {
        return Err(AppError::not_found("carrier account scope"));
    }
    let exists: bool = sqlx::query_scalar(
        r#"SELECT EXISTS(SELECT 1 FROM inventory_owner_facilities assignment
           JOIN inventory_owners owner ON owner.tenant_id=assignment.tenant_id
             AND owner.id=assignment.inventory_owner_id AND owner.deleted IS NULL
           JOIN facilities facility ON facility.tenant_id=assignment.tenant_id
             AND facility.id=assignment.facility_id AND facility.deleted IS NULL
           WHERE assignment.tenant_id=$1 AND assignment.inventory_owner_id=$2
             AND assignment.facility_id=$3 AND assignment.deleted IS NULL)"#,
    )
    .bind(access.tenant_id.get())
    .bind(inventory_owner_id.get())
    .bind(facility_id.get())
    .fetch_one(&mut **tx)
    .await?;
    if exists {
        Ok(())
    } else {
        Err(AppError::not_found("carrier account scope"))
    }
}

fn require_read_model_visible(
    scope: &crate::repo::access::ScopeBindings,
    account: &CarrierAccountReadModel,
) -> AppResult<()> {
    if scope.includes_facility(account.facility_id.get())
        && scope.includes_inventory_owner(account.inventory_owner_id.get())
    {
        Ok(())
    } else {
        Err(AppError::not_found("carrier account"))
    }
}

async fn lock_natural_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: i64,
    owner_id: i64,
    facility_id: i64,
    carrier_code: &str,
) -> AppResult<()> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1,0))")
        .bind(format!(
            "carrier-account:{tenant_id}:{owner_id}:{facility_id}:{carrier_code}"
        ))
        .execute(&mut **tx)
        .await?;
    Ok(())
}
