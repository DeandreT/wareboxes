mod common;

use common::*;

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct TablePrivileges {
    can_select: bool,
    can_insert: bool,
    can_update: bool,
    can_delete: bool,
    can_truncate: bool,
    can_reference: bool,
    can_trigger: bool,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct SequencePrivileges {
    can_use: bool,
    can_select: bool,
    can_update: bool,
}

#[tokio::test]
async fn replenishment_ledgers_are_forced_rls_and_minimally_granted() {
    let fixture = Fixture::new().await;
    let admin = admin_db_for(&fixture.db).await;

    for (table_name, policy_name, can_insert) in [
        (
            "loose_inventory_movement_claims",
            "loose_inventory_movement_claims_tenant_isolation",
            false,
        ),
        (
            "replenishment_policies",
            "replenishment_policies_tenant_isolation",
            true,
        ),
        (
            "replenishment_policy_sources",
            "replenishment_policy_sources_tenant_isolation",
            true,
        ),
        (
            "replenishment_plan_runs",
            "replenishment_plan_runs_tenant_isolation",
            true,
        ),
        (
            "replenishment_tasks",
            "replenishment_tasks_tenant_isolation",
            true,
        ),
        (
            "replenishment_cancellations",
            "replenishment_cancellations_tenant_isolation",
            true,
        ),
        (
            "replenishment_confirmations",
            "replenishment_confirmations_tenant_isolation",
            true,
        ),
    ] {
        let rls: (bool, bool) = sqlx::query_as(
            "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid = $1::regclass",
        )
        .bind(table_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(rls, (true, true), "RLS is not forced for {table_name}");

        let policy_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_policies WHERE tablename = $1 AND policyname = $2)",
        )
        .bind(table_name)
        .bind(policy_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(policy_exists, "missing policy {policy_name}");

        let privileges: TablePrivileges = sqlx::query_as(
            r#"
            SELECT has_table_privilege('wareboxes_app', $1, 'SELECT') AS can_select,
                   has_table_privilege('wareboxes_app', $1, 'INSERT') AS can_insert,
                   has_table_privilege('wareboxes_app', $1, 'UPDATE') AS can_update,
                   has_table_privilege('wareboxes_app', $1, 'DELETE') AS can_delete,
                   has_table_privilege('wareboxes_app', $1, 'TRUNCATE') AS can_truncate,
                   has_table_privilege('wareboxes_app', $1, 'REFERENCES') AS can_reference,
                   has_table_privilege('wareboxes_app', $1, 'TRIGGER') AS can_trigger
            "#,
        )
        .bind(table_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            privileges,
            TablePrivileges {
                can_select: true,
                can_insert,
                can_update: false,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            },
            "unexpected privileges for {table_name}",
        );
    }

    for column_name in ["effective_to", "retired_by_user_id"] {
        let can_update: bool = sqlx::query_scalar(
            "SELECT has_column_privilege('wareboxes_app', 'replenishment_policies', $1, 'UPDATE')",
        )
        .bind(column_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(
            can_update,
            "missing policy retirement grant on {column_name}"
        );
    }
    let can_update_revision: bool = sqlx::query_scalar(
        "SELECT has_column_privilege('wareboxes_app', 'replenishment_policies', 'revision', 'UPDATE')",
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(!can_update_revision);

    let movement_trigger_count: i64 = sqlx::query_scalar(
        r#"
        SELECT count(*)
        FROM pg_trigger trigger
        JOIN pg_class task_table ON task_table.oid = trigger.tgrelid
        WHERE NOT trigger.tgisinternal
          AND task_table.relname IN (
              'putaway_tasks', 'inventory_relocation_tasks', 'replenishment_tasks'
          )
          AND trigger.tgname IN (
              'putaway_tasks_claim_loose_source',
              'putaway_tasks_release_loose_source',
              'inventory_relocation_tasks_claim_loose_source',
              'inventory_relocation_tasks_release_loose_source',
              'replenishment_tasks_claim_loose_source',
              'replenishment_tasks_release_loose_source'
          )
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(movement_trigger_count, 6);

    let active_claim_index_is_unique: bool = sqlx::query_scalar(
        r#"
        SELECT index.indisunique AND index.indpred IS NOT NULL
        FROM pg_index index
        WHERE index.indexrelid =
            'loose_inventory_movement_claims_active_source_idx'::regclass
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(active_claim_index_is_unique);

    for (sequence_name, can_use) in [
        ("loose_inventory_movement_claims_id_seq", false),
        ("replenishment_policies_id_seq", true),
        ("replenishment_plan_runs_id_seq", true),
        ("replenishment_cancellations_id_seq", true),
        ("replenishment_confirmations_id_seq", true),
    ] {
        let privileges: SequencePrivileges = sqlx::query_as(
            r#"
            SELECT has_sequence_privilege('wareboxes_app', $1, 'USAGE') AS can_use,
                   has_sequence_privilege('wareboxes_app', $1, 'SELECT') AS can_select,
                   has_sequence_privilege('wareboxes_app', $1, 'UPDATE') AS can_update
            "#,
        )
        .bind(sequence_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            privileges,
            SequencePrivileges {
                can_use,
                can_select: false,
                can_update: false,
            },
            "unexpected privileges for {sequence_name}",
        );
    }

    admin.close().await;
}

#[tokio::test]
async fn policy_retirement_preserves_revision_and_rejects_active_inbound_work() {
    let fixture = Fixture::new().await;
    let user = fixture.user("replenishment-policy@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, "Replenishment Policy Owner")
        .await;
    let facility_id = fixture
        .facility(tenant_id, "Replenishment Policy Facility")
        .await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let item_id = fixture.item(tenant_id, "Replenishment Item", "each").await;
    let pick_face_location_id = fixture
        .location(tenant_id, facility_id, "REPLENISH-PICK-FACE")
        .await;
    let reserve_location_id = wareboxes_persistence_postgres::locations::add_location(
        &fixture.db,
        tenant_id,
        facility_id,
        None,
        Some("REPLENISH-RESERVE"),
        Some("Replenishment Reserve"),
        "reserve",
        true,
        false,
        false,
    )
    .await
    .unwrap();

    let configured_at = db::now_iso();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO inventory_owner_items
            (tenant_id, created, inventory_owner_id, item_id)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(tenant_id.get())
    .bind(configured_at)
    .bind(inventory_owner_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    let first_policy_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO replenishment_policies (
            tenant_id, inventory_owner_id, facility_id,
            pick_face_location_id, item_id, uom, minimum_qty, target_qty,
            revision, supersedes_policy_id, source_location_count,
            effective_from, configured_by_user_id, configured_at
        )
        VALUES ($1, $2, $3, $4, $5, 'each', 0, 10, 1, NULL, 1, $6, $7, $6)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .bind(pick_face_location_id)
    .bind(item_id)
    .bind(configured_at)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    insert_policy_source(
        &mut tx,
        tenant_id,
        inventory_owner_id,
        facility_id,
        first_policy_id,
        reserve_location_id,
    )
    .await;
    tx.commit().await.unwrap();

    let retired_at = db::now_iso();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let retired_revision: i64 = sqlx::query_scalar(
        r#"
        UPDATE replenishment_policies
        SET effective_to = $1, retired_by_user_id = $2
        WHERE tenant_id = $3 AND id = $4
        RETURNING revision
        "#,
    )
    .bind(retired_at)
    .bind(user.id)
    .bind(tenant_id.get())
    .bind(first_policy_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(retired_revision, 1);

    let successor_at = db::now_iso();
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let successor_policy_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO replenishment_policies (
            tenant_id, inventory_owner_id, facility_id,
            pick_face_location_id, item_id, uom, minimum_qty, target_qty,
            revision, supersedes_policy_id, source_location_count,
            effective_from, configured_by_user_id, configured_at
        )
        VALUES ($1, $2, $3, $4, $5, 'each', 0, 12, 2, $6, 1, $7, $8, $7)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .bind(pick_face_location_id)
    .bind(item_id)
    .bind(first_policy_id)
    .bind(successor_at)
    .bind(user.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    insert_policy_source(
        &mut tx,
        tenant_id,
        inventory_owner_id,
        facility_id,
        successor_policy_id,
        reserve_location_id,
    )
    .await;
    tx.commit().await.unwrap();

    insert_active_replenishment_fixture(
        &fixture,
        tenant_id,
        user.id,
        inventory_owner_id,
        facility_id,
        item_id,
        pick_face_location_id,
        reserve_location_id,
        successor_policy_id,
        successor_at,
    )
    .await;

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let error = sqlx::query(
        r#"
        UPDATE replenishment_policies
        SET effective_to = $1, retired_by_user_id = $2
        WHERE tenant_id = $3 AND id = $4
        "#,
    )
    .bind(db::now_iso())
    .bind(user.id)
    .bind(tenant_id.get())
    .bind(successor_policy_id)
    .execute(&mut *tx)
    .await
    .unwrap_err();
    assert_eq!(
        error.as_database_error().unwrap().code().as_deref(),
        Some("55000")
    );
    tx.rollback().await.unwrap();
}

async fn insert_policy_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    inventory_owner_id: i64,
    facility_id: i64,
    policy_id: i64,
    source_location_id: i64,
) {
    sqlx::query(
        r#"
        INSERT INTO replenishment_policy_sources (
            tenant_id, inventory_owner_id, facility_id,
            policy_id, source_location_id, source_sequence
        )
        VALUES ($1, $2, $3, $4, $5, 1)
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .bind(policy_id)
    .bind(source_location_id)
    .execute(&mut **tx)
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
async fn insert_active_replenishment_fixture(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    inventory_owner_id: i64,
    facility_id: i64,
    item_id: i64,
    pick_face_location_id: i64,
    reserve_location_id: i64,
    policy_id: i64,
    created_at: wareboxes_domain::Timestamp,
) {
    let admin = admin_db_for(&fixture.db).await;
    let mut tx = admin.begin().await.unwrap();
    sqlx::query("SET LOCAL session_replication_role = 'replica'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let task_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO work_tasks (
            tenant_id, created, task_type, status, title, created_by,
            facility_id, inventory_owner_id
        )
        VALUES ($1, $2, 'replenishment', 'open', 'Active inbound', $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(created_at)
    .bind(user_id)
    .bind(facility_id)
    .bind(inventory_owner_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let plan_run_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO replenishment_plan_runs (
            tenant_id, inventory_owner_id, facility_id, policy_id,
            policy_revision, pick_face_location_id, item_id, uom,
            minimum_qty, target_qty, source_location_count,
            pick_face_free_qty, active_inbound_qty, projected_free_qty,
            unallocated_demand_qty, required_level_qty, target_gap_qty,
            reserve_free_qty, planned_qty, work_count, outcome,
            planned_by_user_id, planned_at
        )
        VALUES (
            $1, $2, $3, $4, 2, $5, $6, 'each', 0, 12, 1,
            0, 0, 0, 1, 12, 12, 12, 12, 1, 'fully_planned', $7, $8
        )
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(facility_id)
    .bind(policy_id)
    .bind(pick_face_location_id)
    .bind(item_id)
    .bind(user_id)
    .bind(created_at)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO replenishment_tasks (
            tenant_id, task_id, plan_run_id, policy_id, policy_revision,
            inventory_owner_id, facility_id, source_inventory_balance_id,
            source_location_id, destination_location_id, item_batch_id,
            item_id, uom, inventory_status, source_free_qty, planned_qty,
            source_received_at, travel_sequence
        )
        VALUES ($1, $2, $3, $4, 2, $5, $6, 900001, $7, $8, 900002,
                $9, 'each', 'available', 12, 12, $10, 1)
        "#,
    )
    .bind(tenant_id.get())
    .bind(task_id)
    .bind(plan_run_id)
    .bind(policy_id)
    .bind(inventory_owner_id)
    .bind(facility_id)
    .bind(reserve_location_id)
    .bind(pick_face_location_id)
    .bind(item_id)
    .bind(created_at)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    admin.close().await;
}
