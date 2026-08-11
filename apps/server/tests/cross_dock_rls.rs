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
async fn cross_dock_ledgers_are_forced_rls_and_minimally_granted() {
    let fixture = Fixture::new().await;
    let admin = admin_db_for(&fixture.db).await;

    for (table_name, policy_name) in [
        (
            "cross_dock_plan_runs",
            "cross_dock_plan_runs_tenant_isolation",
        ),
        ("cross_dock_tasks", "cross_dock_tasks_tenant_isolation"),
        (
            "cross_dock_confirmations",
            "cross_dock_confirmations_tenant_isolation",
        ),
        (
            "cross_dock_cancellations",
            "cross_dock_cancellations_tenant_isolation",
        ),
    ] {
        let rls: (bool, bool) = sqlx::query_as(
            "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid=$1::regclass",
        )
        .bind(table_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(rls, (true, true), "RLS is not forced for {table_name}");

        let policy_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pg_policies WHERE tablename=$1 AND policyname=$2)",
        )
        .bind(table_name)
        .bind(policy_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(policy_exists, "missing policy {policy_name}");

        let privileges: TablePrivileges = sqlx::query_as(
            r#"
            SELECT has_table_privilege('wareboxes_app',$1,'SELECT') AS can_select,
                   has_table_privilege('wareboxes_app',$1,'INSERT') AS can_insert,
                   has_table_privilege('wareboxes_app',$1,'UPDATE') AS can_update,
                   has_table_privilege('wareboxes_app',$1,'DELETE') AS can_delete,
                   has_table_privilege('wareboxes_app',$1,'TRUNCATE') AS can_truncate,
                   has_table_privilege('wareboxes_app',$1,'REFERENCES') AS can_reference,
                   has_table_privilege('wareboxes_app',$1,'TRIGGER') AS can_trigger
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
                can_insert: true,
                can_update: false,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            },
            "unexpected privileges for {table_name}",
        );
    }

    for sequence_name in [
        "cross_dock_plan_runs_id_seq",
        "cross_dock_confirmations_id_seq",
        "cross_dock_cancellations_id_seq",
    ] {
        let privileges: SequencePrivileges = sqlx::query_as(
            r#"
            SELECT has_sequence_privilege('wareboxes_app',$1,'USAGE') AS can_use,
                   has_sequence_privilege('wareboxes_app',$1,'SELECT') AS can_select,
                   has_sequence_privilege('wareboxes_app',$1,'UPDATE') AS can_update
            "#,
        )
        .bind(sequence_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(
            privileges,
            SequencePrivileges {
                can_use: true,
                can_select: false,
                can_update: false,
            },
            "unexpected privileges for {sequence_name}",
        );
    }

    let task_updates: (bool, bool) = sqlx::query_as(
        r#"
        SELECT has_column_privilege('wareboxes_app','cross_dock_tasks','closed_at','UPDATE'),
               has_column_privilege('wareboxes_app','cross_dock_tasks','planned_quantity','UPDATE')
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert_eq!(task_updates, (true, false));
    admin.close().await;
}
