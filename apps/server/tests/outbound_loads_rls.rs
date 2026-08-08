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
async fn outbound_load_tables_are_forced_rls_and_minimally_granted() {
    let fixture = Fixture::new().await;
    let admin = admin_db_for(&fixture.db).await;

    for (table_name, can_insert) in [
        ("packed_inventory_positions", false),
        ("packed_carton_move_confirmations", true),
        ("packed_carton_move_details", true),
        ("outbound_loads", true),
        ("outbound_load_shipments", true),
        ("outbound_load_cartons", true),
        ("outbound_load_cancellations", true),
    ] {
        let rls: (bool, bool) = sqlx::query_as(
            "SELECT relrowsecurity, relforcerowsecurity FROM pg_class WHERE oid = $1::regclass",
        )
        .bind(table_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(rls, (true, true), "RLS is not forced for {table_name}");

        let policy_name = format!("{table_name}_tenant_isolation");
        let policy_exists: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM pg_policies WHERE tablename = $1 AND policyname = $2)",
        )
        .bind(table_name)
        .bind(&policy_name)
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

    for (table_name, allowed, denied) in [
        (
            "packed_inventory_positions",
            &[
                "state",
                "outbound_load_id",
                "outbound_load_carton_id",
                "load_sequence",
                "current_inventory_allocation_id",
                "current_inventory_balance_id",
                "current_location_id",
                "current_license_plate_id",
                "revision",
                "positioned_at",
                "departure_inventory_transaction_id",
                "departed_at",
            ][..],
            &["tenant_id", "carton_content_id", "packed_qty"][..],
        ),
        (
            "outbound_loads",
            &[
                "state",
                "revision",
                "dock_door_location_id",
                "trailer_number",
                "seal_number",
                "released_by_user_id",
                "released_at",
                "loading_started_by_user_id",
                "loading_started_at",
                "ready_to_depart_by_user_id",
                "ready_to_depart_at",
                "departed_by_user_id",
                "departed_at",
                "cancelled_by_user_id",
                "cancelled_at",
            ][..],
            &[
                "tenant_id",
                "facility_id",
                "staging_lane_location_id",
                "virtual_trailer_location_id",
            ][..],
        ),
        (
            "outbound_load_shipments",
            &["closed_at"][..],
            &["tenant_id", "inventory_owner_id", "shipment_id"][..],
        ),
        (
            "outbound_load_cartons",
            &[
                "state",
                "revision",
                "last_move_confirmation_id",
                "staged_at",
                "loaded_at",
                "departed_at",
                "closed_at",
            ][..],
            &["tenant_id", "inventory_owner_id", "shipment_carton_id"][..],
        ),
    ] {
        for column_name in allowed {
            let can_update: bool = sqlx::query_scalar(
                "SELECT has_column_privilege('wareboxes_app', $1, $2, 'UPDATE')",
            )
            .bind(table_name)
            .bind(column_name)
            .fetch_one(&admin)
            .await
            .unwrap();
            assert!(can_update, "missing UPDATE on {table_name}.{column_name}");
        }
        for column_name in denied {
            let can_update: bool = sqlx::query_scalar(
                "SELECT has_column_privilege('wareboxes_app', $1, $2, 'UPDATE')",
            )
            .bind(table_name)
            .bind(column_name)
            .fetch_one(&admin)
            .await
            .unwrap();
            assert!(
                !can_update,
                "unexpected UPDATE on {table_name}.{column_name}"
            );
        }
    }

    for sequence_name in [
        "packed_carton_move_confirmations_id_seq",
        "packed_carton_move_details_id_seq",
        "outbound_loads_id_seq",
        "outbound_load_shipments_id_seq",
        "outbound_load_cartons_id_seq",
        "outbound_load_cancellations_id_seq",
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

#[tokio::test]
async fn outbound_load_schema_canaries_preserve_scopes_and_active_assignments() {
    let fixture = Fixture::new().await;
    let admin = admin_db_for(&fixture.db).await;

    for index_name in [
        "outbound_loads_active_staging_lane_idx",
        "outbound_loads_active_dock_door_idx",
        "outbound_loads_active_trailer_number_idx",
        "outbound_loads_active_virtual_trailer_idx",
        "outbound_load_shipments_active_shipment_idx",
        "outbound_load_cartons_active_shipment_carton_idx",
        "outbound_load_cartons_active_license_plate_idx",
    ] {
        let canary: (bool, bool) = sqlx::query_as(
            r#"
            SELECT index.indisunique, index.indpred IS NOT NULL
            FROM pg_index index
            WHERE index.indexrelid = $1::regclass
            "#,
        )
        .bind(index_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert_eq!(canary, (true, true), "invalid active index {index_name}");
    }

    for constraint_name in [
        "packed_inventory_positions_content_fkey",
        "packed_inventory_positions_current_allocation_fkey",
        "packed_carton_move_confirmations_load_carton_fkey",
        "packed_carton_move_details_position_fkey",
        "outbound_load_shipments_shipment_fkey",
        "outbound_load_cartons_load_shipment_fkey",
        "outbound_load_cartons_shipment_carton_fkey",
    ] {
        let definition: String = sqlx::query_scalar(
            r#"
            SELECT pg_get_constraintdef(oid)
            FROM pg_constraint
            WHERE conname = $1
            "#,
        )
        .bind(constraint_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(
            definition.contains("tenant_id"),
            "{constraint_name} is not tenant scoped: {definition}"
        );
    }

    for trigger_name in [
        "carton_contents_initialize_packed_position",
        "packed_inventory_positions_guard_mutation",
        "packed_inventory_positions_require_evidence",
        "packed_carton_move_confirmations_validate",
        "packed_carton_move_confirmations_are_immutable",
        "packed_carton_move_details_validate",
        "packed_carton_move_details_are_immutable",
        "outbound_loads_validate",
        "outbound_loads_guard_mutation",
        "outbound_loads_require_consistency",
        "outbound_load_shipments_validate",
        "outbound_load_shipments_guard_mutation",
        "outbound_load_shipments_require_consistency",
        "outbound_load_cartons_validate",
        "outbound_load_cartons_guard_mutation",
        "outbound_load_cartons_require_consistency",
        "outbound_load_cancellations_validate",
        "outbound_load_cancellations_are_immutable",
        "shipments_reject_assigned_direct_departure",
    ] {
        let exists: bool = sqlx::query_scalar(
            r#"
            SELECT EXISTS (
                SELECT 1
                FROM pg_trigger
                WHERE tgname = $1 AND NOT tgisinternal
            )
            "#,
        )
        .bind(trigger_name)
        .fetch_one(&admin)
        .await
        .unwrap();
        assert!(exists, "missing persistence canary trigger {trigger_name}");
    }

    let initialization_is_definer: bool = sqlx::query_scalar(
        r#"
        SELECT prosecdef
        FROM pg_proc
        WHERE oid = 'public.initialize_packed_inventory_position()'::regprocedure
        "#,
    )
    .fetch_one(&admin)
    .await
    .unwrap();
    assert!(initialization_is_definer);

    admin.close().await;
}
