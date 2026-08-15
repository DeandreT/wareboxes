mod common;

use common::*;

async fn add_membership(db: &db::Db, tenant_id: TenantId, user_id: i64) {
    let mut tx = tenant_tx(db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn employee(
    fixture: &Fixture,
    tenant_id: TenantId,
    actor_id: i64,
    facility_id: i64,
    key: &str,
) -> i64 {
    let access = repo::tenants::access_for_user(&fixture.db, actor_id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    repo::employees::add_employee(
        &fixture.db,
        tenant_id,
        &access.site_scope,
        &repo::employees::NewEmployee {
            first_name: key,
            last_name: "Operator",
            title: "Warehouse operator",
            employee_type: "employee",
            email: None,
            phone: None,
            hired: db::now_iso(),
            facility_ids: &[facility_id],
        },
    )
    .await
    .unwrap()
}

async fn direct_link(
    db: &db::Db,
    tenant_id: TenantId,
    employee_id: i64,
    user_id: i64,
    actor_id: i64,
) -> Result<(), sqlx::Error> {
    let mut tx = db.begin().await?;
    db::bind_tenant_context(&mut tx, tenant_id).await.unwrap();
    let changed_at = db::now_iso();
    sqlx::query(
        r#"
        UPDATE employees SET user_id=$1,identity_revision=1,
          identity_changed_by_user_id=$2,identity_changed_at=$3
        WHERE tenant_id=$4 AND id=$5 AND identity_revision=0
        "#,
    )
    .bind(user_id)
    .bind(actor_id)
    .bind(changed_at)
    .bind(tenant_id.get())
    .bind(employee_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO employee_identity_changes
          (tenant_id,employee_id,previous_user_id,user_id,change_kind,reason,
           resulting_revision,changed_by_user_id,changed_at)
        VALUES($1,$2,NULL,$3,'linked','direct concurrency proof',1,$4,$5)
        "#,
    )
    .bind(tenant_id.get())
    .bind(employee_id)
    .bind(user_id)
    .bind(actor_id)
    .bind(changed_at)
    .execute(&mut *tx)
    .await?;
    tx.commit().await
}

#[tokio::test]
async fn direct_identity_writes_are_unique_concurrent_audited_and_scope_guarded() {
    let fixture = Fixture::new().await;
    let actor = fixture.user("identity-db-actor@test.local").await;
    let target = fixture.user("identity-db-target@test.local").await;
    let tenant_id = tenant_for_user(&fixture.db, actor.id).await;
    add_membership(&fixture.db, tenant_id, target.id).await;
    let facility_id = fixture.facility(tenant_id, "Identity Race DC").await;
    let employee_a = employee(&fixture, tenant_id, actor.id, facility_id, "Race-A").await;
    let employee_b = employee(&fixture, tenant_id, actor.id, facility_id, "Race-B").await;

    let first = direct_link(&fixture.db, tenant_id, employee_a, target.id, actor.id);
    let second = direct_link(&fixture.db, tenant_id, employee_b, target.id, actor.id);
    let (first, second) = tokio::join!(first, second);
    assert_eq!(
        [first.is_ok(), second.is_ok()]
            .into_iter()
            .filter(|ok| *ok)
            .count(),
        1
    );

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let state: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT
          (SELECT COUNT(*) FROM employees WHERE user_id=$1),
          (SELECT COUNT(*) FROM employee_identity_changes WHERE user_id=$1),
          (SELECT COUNT(*) FROM employees WHERE identity_revision=1)
        "#,
    )
    .bind(target.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    assert_eq!(state, (1, 1, 1));

    let mut membership_tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        "UPDATE tenant_memberships SET deleted=clock_timestamp() WHERE tenant_id=$1 AND user_id=$2",
    )
    .bind(tenant_id.get())
    .bind(target.id)
    .execute(&mut *membership_tx)
    .await
    .unwrap();
    assert!(membership_tx.commit().await.is_err());

    let mut scope_tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        "UPDATE tenant_memberships SET all_facilities=false WHERE tenant_id=$1 AND user_id=$2",
    )
    .bind(tenant_id.get())
    .bind(target.id)
    .execute(&mut *scope_tx)
    .await
    .unwrap();
    assert!(scope_tx.commit().await.is_err());
}

#[tokio::test]
async fn identity_audit_is_tenant_isolated_append_only_and_least_privileged() {
    let fixture = Fixture::new().await;
    let tenant_a_user = fixture.user("identity-rls-a@test.local").await;
    let tenant_b_user = fixture.user("identity-rls-b@test.local").await;
    let tenant_a = tenant_for_user(&fixture.db, tenant_a_user.id).await;
    let tenant_b = tenant_for_user(&fixture.db, tenant_b_user.id).await;
    let facility_b = fixture.facility(tenant_b, "Identity RLS B").await;
    let employee_b = employee(&fixture, tenant_b, tenant_b_user.id, facility_b, "RLS-B").await;
    direct_link(
        &fixture.db,
        tenant_b,
        employee_b,
        tenant_b_user.id,
        tenant_b_user.id,
    )
    .await
    .unwrap();

    let privileges: (bool, bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT
          has_table_privilege(current_user,'public.employee_identity_changes','SELECT'),
          has_table_privilege(current_user,'public.employee_identity_changes','INSERT'),
          has_table_privilege(current_user,'public.employee_identity_changes','UPDATE'),
          has_table_privilege(current_user,'public.employee_identity_changes','DELETE'),
          has_sequence_privilege(current_user,'public.employee_identity_changes_id_seq','USAGE')
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(privileges, (true, true, false, false, true));

    let unbound_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM employee_identity_changes")
        .fetch_one(&fixture.db)
        .await
        .unwrap();
    assert_eq!(unbound_count, 0);

    let mut tenant_a_tx = tenant_tx(&fixture.db, tenant_a).await;
    let tenant_a_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM employee_identity_changes")
        .fetch_one(&mut *tenant_a_tx)
        .await
        .unwrap();
    tenant_a_tx.rollback().await.unwrap();
    assert_eq!(tenant_a_count, 0);

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let tenant_b_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM employee_identity_changes")
        .fetch_one(&mut *tenant_b_tx)
        .await
        .unwrap();
    assert_eq!(tenant_b_count, 1);
    assert!(
        sqlx::query("UPDATE employee_identity_changes SET reason=reason")
            .execute(&mut *tenant_b_tx)
            .await
            .is_err()
    );
    tenant_b_tx.rollback().await.unwrap();

    let mut cross_tenant_tx = tenant_tx(&fixture.db, tenant_a).await;
    let forged = sqlx::query(
        r#"
        INSERT INTO employee_identity_changes
          (tenant_id,employee_id,previous_user_id,user_id,change_kind,reason,
           resulting_revision,changed_by_user_id,changed_at)
        VALUES($1,$2,NULL,$3,'linked','forged cross tenant',1,$3,$4)
        "#,
    )
    .bind(tenant_b.get())
    .bind(employee_b)
    .bind(tenant_b_user.id)
    .bind(db::now_iso())
    .execute(&mut *cross_tenant_tx)
    .await;
    assert!(forged.is_err());
    cross_tenant_tx.rollback().await.unwrap();
}
