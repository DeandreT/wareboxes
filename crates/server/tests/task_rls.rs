mod common;

use common::*;

#[tokio::test]
async fn work_tasks_require_a_transaction_local_tenant_context() {
    let fixture = Fixture::new().await;
    let worker_a = fixture.wms_user("task-rls-a@test.com").await;
    let worker_b = fixture.wms_user("task-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, worker_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, worker_b.id).await;
    let facility_a = fixture.facility(tenant_a, "Task RLS Facility A").await;
    let facility_b = fixture.facility(tenant_b, "Task RLS Facility B").await;
    let location_a = fixture.location(tenant_a, facility_a, "TASK-RLS-A").await;
    let location_b = fixture.location(tenant_b, facility_b, "TASK-RLS-B").await;
    let task_a = create_task(&fixture, tenant_a, worker_a.id, location_a, "Task RLS A").await;
    let task_b = create_task(&fixture, tenant_b, worker_b.id, location_b, "Task RLS B").await;
    assert_ne!(task_a, task_b);
    assert!(
        repo::tasks::start_task(&fixture.db, tenant_a, task_a, worker_a.id)
            .await
            .unwrap()
    );
    assert!(
        repo::tasks::start_task(&fixture.db, tenant_b, task_b, worker_b.id)
            .await
            .unwrap()
    );

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let source_task: (i64, String, String, bool) =
        sqlx::query_as("SELECT id, status, title, deleted IS NULL FROM work_tasks WHERE id = $1")
            .bind(task_a)
            .fetch_one(&mut *tenant_a_tx)
            .await
            .unwrap();
    let source_progress: (i64, i64, String, Option<String>) = sqlx::query_as(
        r#"
        SELECT id, task_id, action, note
        FROM work_task_progress
        WHERE task_id = $1 AND action = 'started'
        "#,
    )
    .bind(task_a)
    .fetch_one(&mut *tenant_a_tx)
    .await
    .unwrap();
    tenant_a_tx.rollback().await.unwrap();

    let unbound_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM work_tasks),
               (SELECT COUNT(*) FROM work_task_progress)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0));
    let unbound_task_updates = sqlx::query("UPDATE work_tasks SET title = 'unbound' WHERE id = $1")
        .bind(task_a)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(unbound_task_updates, 0);
    let unbound_progress_updates =
        sqlx::query("UPDATE work_task_progress SET note = 'unbound' WHERE id = $1")
            .bind(source_progress.0)
            .execute(&fixture.db)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(unbound_progress_updates, 0);
    let unbound_task_deletes = sqlx::query("DELETE FROM work_tasks WHERE id = $1")
        .bind(task_a)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(unbound_task_deletes, 0);
    let unbound_progress_deletes = sqlx::query("DELETE FROM work_task_progress WHERE id = $1")
        .bind(source_progress.0)
        .execute(&fixture.db)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(unbound_progress_deletes, 0);

    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_task(
        &mut unbound_tx,
        tenant_a,
        worker_a.id,
        facility_a,
        "Task RLS unbound",
    )
    .await
    .is_err());
    unbound_tx.rollback().await.unwrap();
    let mut unbound_tx = fixture.db.begin().await.unwrap();
    assert!(insert_progress(
        &mut unbound_tx,
        tenant_a,
        worker_a.id,
        task_a,
        facility_a,
        "unbound",
    )
    .await
    .is_err());
    unbound_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let guessed_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM work_tasks WHERE id = $1),
               (SELECT COUNT(*) FROM work_task_progress WHERE id = $2)
        "#,
    )
    .bind(task_a)
    .bind(source_progress.0)
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(guessed_counts, (0, 0));
    let cross_tenant_task_updates =
        sqlx::query("UPDATE work_tasks SET title = 'cross-tenant' WHERE id = $1")
            .bind(task_a)
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(cross_tenant_task_updates, 0);
    let cross_tenant_progress_updates =
        sqlx::query("UPDATE work_task_progress SET note = 'cross-tenant' WHERE id = $1")
            .bind(source_progress.0)
            .execute(&mut *tenant_b_tx)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(cross_tenant_progress_updates, 0);
    let cross_tenant_task_deletes = sqlx::query("DELETE FROM work_tasks WHERE id = $1")
        .bind(task_a)
        .execute(&mut *tenant_b_tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(cross_tenant_task_deletes, 0);
    let cross_tenant_progress_deletes = sqlx::query("DELETE FROM work_task_progress WHERE id = $1")
        .bind(source_progress.0)
        .execute(&mut *tenant_b_tx)
        .await
        .unwrap()
        .rows_affected();
    assert_eq!(cross_tenant_progress_deletes, 0);
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_task(
        &mut tenant_b_tx,
        tenant_a,
        worker_a.id,
        facility_a,
        "Task RLS cross-tenant",
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_progress(
        &mut tenant_b_tx,
        tenant_b,
        worker_b.id,
        task_a,
        facility_b,
        "guessed_parent",
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();
    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert!(insert_progress(
        &mut tenant_b_tx,
        tenant_a,
        worker_a.id,
        task_a,
        facility_a,
        "cross_tenant",
    )
    .await
    .is_err());
    tenant_b_tx.rollback().await.unwrap();

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let unchanged_task: (i64, String, String, bool) =
        sqlx::query_as("SELECT id, status, title, deleted IS NULL FROM work_tasks WHERE id = $1")
            .bind(task_a)
            .fetch_one(&mut *tenant_a_tx)
            .await
            .unwrap();
    let unchanged_progress: (i64, i64, String, Option<String>) = sqlx::query_as(
        r#"
        SELECT id, task_id, action, note
        FROM work_task_progress
        WHERE task_id = $1 AND action = 'started'
        "#,
    )
    .bind(task_a)
    .fetch_one(&mut *tenant_a_tx)
    .await
    .unwrap();
    tenant_a_tx.rollback().await.unwrap();
    assert_eq!(unchanged_task, source_task);
    assert_eq!(unchanged_progress, source_progress);
}

async fn create_task(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    location_id: i64,
    instructions: &str,
) -> i64 {
    repo::tasks::create_location_cycle_count_task(
        &fixture.db,
        tenant_id,
        user_id,
        location_id,
        None,
        None,
        None,
        None,
        Some(instructions.to_owned()),
    )
    .await
    .unwrap()
}

async fn insert_task(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
    facility_id: i64,
    title: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO work_tasks (
            tenant_id, facility_id, created, task_type, title, created_by
        )
        VALUES ($1, $2, $3, 'cycle_count_location', $4, $5)
        "#,
    )
    .bind(tenant_id.get())
    .bind(facility_id)
    .bind(db::now_iso())
    .bind(title)
    .bind(user_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}

async fn insert_progress(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    tenant_id: TenantId,
    user_id: i64,
    task_id: i64,
    facility_id: i64,
    note: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        INSERT INTO work_task_progress (
            tenant_id, created, task_id, facility_id, user_id, action, note
        )
        VALUES ($1, $2, $3, $4, $5, 'started', $6)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(task_id)
    .bind(facility_id)
    .bind(user_id)
    .bind(note)
    .execute(&mut **tx)
    .await
    .map(|_| ())
}
