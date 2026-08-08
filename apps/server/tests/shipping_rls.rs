mod common;

use common::{admin_db_for, Fixture};

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
async fn shipping_ledgers_are_forced_rls_and_minimally_granted() {
    let fixture = Fixture::new().await;
    let admin = admin_db_for(&fixture.db).await;

    for (table_name, policy_name, can_update) in [
        ("shipments", "shipments_tenant_isolation", true),
        (
            "shipment_address_snapshots",
            "shipment_address_snapshots_tenant_isolation",
            false,
        ),
        (
            "shipment_cartons",
            "shipment_cartons_tenant_isolation",
            false,
        ),
        (
            "shipment_manifests",
            "shipment_manifests_tenant_isolation",
            false,
        ),
        (
            "shipment_manifest_packages",
            "shipment_manifest_packages_tenant_isolation",
            false,
        ),
        (
            "shipment_confirmations",
            "shipment_confirmations_tenant_isolation",
            false,
        ),
        (
            "shipment_confirmation_cartons",
            "shipment_confirmation_cartons_tenant_isolation",
            false,
        ),
        (
            "pick_short_ship_dispositions",
            "pick_short_ship_dispositions_tenant_isolation",
            false,
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
        "shipments_id_seq",
        "shipment_address_snapshots_id_seq",
        "shipment_cartons_id_seq",
        "shipment_manifests_id_seq",
        "shipment_manifest_packages_id_seq",
        "shipment_confirmations_id_seq",
        "shipment_confirmation_cartons_id_seq",
        "pick_short_ship_dispositions_id_seq",
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
                can_use: true,
                can_select: false,
                can_update: false,
            },
            "unexpected privileges for {sequence_name}",
        );
    }

    admin.close().await;
}
