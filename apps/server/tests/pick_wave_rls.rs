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
async fn typed_pick_wave_ledgers_force_tenant_rls_and_expose_only_workflow_privileges() {
    let fixture = Fixture::new().await;
    for table_name in ["pick_waves", "pick_wave_orders"] {
        let privileges: TablePrivileges = sqlx::query_as(
            r#"SELECT has_table_privilege(current_user,$1,'SELECT') AS can_select,
                      has_table_privilege(current_user,$1,'INSERT') AS can_insert,
                      has_table_privilege(current_user,$1,'UPDATE') AS can_update,
                      has_table_privilege(current_user,$1,'DELETE') AS can_delete,
                      has_table_privilege(current_user,$1,'TRUNCATE') AS can_truncate,
                      has_table_privilege(current_user,$1,'REFERENCES') AS can_reference,
                      has_table_privilege(current_user,$1,'TRIGGER') AS can_trigger"#,
        )
        .bind(table_name)
        .fetch_one(&fixture.db)
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
    for sequence_name in ["pick_waves_id_seq", "pick_wave_orders_id_seq"] {
        let privileges: SequencePrivileges = sqlx::query_as(
            r#"SELECT has_sequence_privilege(current_user,$1,'USAGE') AS can_use,
                      has_sequence_privilege(current_user,$1,'SELECT') AS can_select,
                      has_sequence_privilege(current_user,$1,'UPDATE') AS can_update"#,
        )
        .bind(sequence_name)
        .fetch_one(&fixture.db)
        .await
        .unwrap();
        assert_eq!(
            privileges,
            SequencePrivileges {
                can_use: true,
                can_select: false,
                can_update: false,
            }
        );
    }
    let forced: Vec<(String, bool, bool, i64)> = sqlx::query_as(
        r#"SELECT class.relname,class.relrowsecurity,class.relforcerowsecurity,
                  COUNT(policy.policyname)::bigint
           FROM pg_class class JOIN pg_namespace namespace ON namespace.oid=class.relnamespace
           LEFT JOIN pg_policies policy ON policy.schemaname=namespace.nspname
             AND policy.tablename=class.relname
           WHERE namespace.nspname='public' AND class.relname=ANY($1)
           GROUP BY class.relname,class.relrowsecurity,class.relforcerowsecurity
           ORDER BY class.relname"#,
    )
    .bind(["pick_wave_orders", "pick_waves"])
    .fetch_all(&fixture.db)
    .await
    .unwrap();
    assert_eq!(
        forced,
        vec![
            ("pick_wave_orders".to_owned(), true, true, 1),
            ("pick_waves".to_owned(), true, true, 1),
        ]
    );
    let column_updates: (bool, bool, bool, bool) = sqlx::query_as(
        r#"SELECT has_column_privilege(current_user,'pick_waves','status','UPDATE'),
                  has_column_privilege(current_user,'pick_waves','name','UPDATE'),
                  has_column_privilege(current_user,'pick_wave_orders','active','UPDATE'),
                  has_column_privilege(current_user,'pick_wave_orders','order_id','UPDATE')"#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(column_updates, (true, false, true, false));
}

#[tokio::test]
async fn typed_outbound_tables_expose_only_required_runtime_privileges() {
    let fixture = Fixture::new().await;
    for (table_name, can_update) in [
        ("order_releases", false),
        ("order_release_allocations", false),
        ("pick_tasks", true),
        ("pick_task_contents", true),
        ("pick_confirmations", false),
        ("outbound_order_containers", false),
        ("packing_sessions", true),
        ("packing_session_allocations", false),
        ("cartons", true),
        ("carton_contents", false),
    ] {
        let privileges: TablePrivileges = sqlx::query_as(
            r#"
            SELECT has_table_privilege(current_user, $1, 'SELECT') AS can_select,
                   has_table_privilege(current_user, $1, 'INSERT') AS can_insert,
                   has_table_privilege(current_user, $1, 'UPDATE') AS can_update,
                   has_table_privilege(current_user, $1, 'DELETE') AS can_delete,
                   has_table_privilege(current_user, $1, 'TRUNCATE') AS can_truncate,
                   has_table_privilege(current_user, $1, 'REFERENCES') AS can_reference,
                   has_table_privilege(current_user, $1, 'TRIGGER') AS can_trigger
            "#,
        )
        .bind(table_name)
        .fetch_one(&fixture.db)
        .await
        .unwrap();
        assert_eq!(
            privileges,
            TablePrivileges {
                can_select: true,
                can_insert: true,
                can_update,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            },
            "unexpected privileges for {table_name}",
        );
    }

    for sequence_name in [
        "order_releases_id_seq",
        "pick_tasks_id_seq",
        "pick_task_contents_id_seq",
        "pick_confirmations_id_seq",
        "outbound_order_containers_id_seq",
        "packing_sessions_id_seq",
        "packing_session_allocations_id_seq",
        "cartons_id_seq",
        "carton_contents_id_seq",
    ] {
        let privileges: SequencePrivileges = sqlx::query_as(
            r#"
            SELECT has_sequence_privilege(current_user, $1, 'USAGE') AS can_use,
                   has_sequence_privilege(current_user, $1, 'SELECT') AS can_select,
                   has_sequence_privilege(current_user, $1, 'UPDATE') AS can_update
            "#,
        )
        .bind(sequence_name)
        .fetch_one(&fixture.db)
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

    let container_columns: (bool, bool, bool, bool) = sqlx::query_as(
        r#"
        SELECT has_column_privilege(current_user, 'outbound_order_containers', 'released_at', 'UPDATE'),
               has_column_privilege(current_user, 'outbound_order_containers', 'released_by_user_id', 'UPDATE'),
               has_column_privilege(current_user, 'outbound_order_containers', 'release_order_cancellation_id', 'UPDATE'),
               has_column_privilege(current_user, 'outbound_order_containers', 'order_id', 'UPDATE')
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(container_columns, (true, true, true, false));
}
