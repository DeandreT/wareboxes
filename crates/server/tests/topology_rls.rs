mod common;

use common::*;

const TABLES: [&str; 4] = [
    "facilities",
    "locations",
    "inventory_owners",
    "inventory_owner_facilities",
];

const SEQUENCES: [&str; 4] = [
    "facilities_id_seq",
    "locations_id_seq",
    "inventory_owners_id_seq",
    "inventory_owner_facilities_id_seq",
];

#[derive(Clone, Copy, Debug)]
struct TopologyRefs {
    tenant_id: TenantId,
    facility_ids: [i64; 2],
    location_ids: [i64; 2],
    inventory_owner_id: i64,
    assignment_id: i64,
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct TablePrivileges {
    table_name: String,
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
    sequence_name: String,
    can_use: bool,
    can_select: bool,
    can_update: bool,
}

#[tokio::test]
async fn topology_requires_tenant_context_and_exact_runtime_privileges() {
    let fixture = Fixture::new().await;
    assert_exact_runtime_privileges(&fixture.db).await;

    let user_a = fixture.user("topology-rls-a@test.com").await;
    let user_b = fixture.user("topology-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let refs_a = topology_refs(&fixture, tenant_a, "TOPOLOGY-RLS-A").await;
    let refs_b = topology_refs(&fixture, tenant_b, "TOPOLOGY-RLS-B").await;

    assert_repository_topology(&fixture.db, refs_a).await;
    assert_repository_topology(&fixture.db, refs_b).await;
    let source_a = snapshot(&fixture.db, tenant_a).await;
    let source_b = snapshot(&fixture.db, tenant_b).await;

    let unbound_counts: (i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM facilities),
               (SELECT COUNT(*) FROM locations),
               (SELECT COUNT(*) FROM inventory_owners),
               (SELECT COUNT(*) FROM inventory_owner_facilities)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0, 0, 0));

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    assert_eq!(
        visible_ids(&mut tenant_b_tx).await,
        (
            refs_b.facility_ids.to_vec(),
            refs_b.location_ids.to_vec(),
            vec![refs_b.inventory_owner_id],
            vec![refs_b.assignment_id],
        )
    );
    assert_eq!(
        allowed_update_counts(&mut tenant_b_tx, refs_a).await,
        [0, 0]
    );
    tenant_b_tx.rollback().await.unwrap();

    assert_acl_restricted_updates_fail(&fixture.db, refs_a).await;
    assert_deletes_fail(&fixture.db, refs_a).await;
    assert_forged_inserts_fail(&fixture.db, None, refs_a, "UNBOUND").await;
    assert_forged_inserts_fail(&fixture.db, Some(tenant_b), refs_a, "CROSS-TENANT").await;

    let cross_facility_parent = repo::locations::add_location(
        &fixture.db,
        tenant_a,
        refs_a.facility_ids[1],
        Some(refs_a.location_ids[0]),
        Some("TOPOLOGY-RLS-A-CROSS-PARENT"),
        Some("cross-parent"),
        "bin",
        true,
        true,
        false,
    )
    .await;
    assert!(
        cross_facility_parent.is_err(),
        "a location parent must belong to the same facility"
    );

    assert_repository_topology(&fixture.db, refs_a).await;
    assert_repository_topology(&fixture.db, refs_b).await;
    assert_eq!(snapshot(&fixture.db, tenant_a).await, source_a);
    assert_eq!(snapshot(&fixture.db, tenant_b).await, source_b);
}

async fn topology_refs(fixture: &Fixture, tenant_id: TenantId, key: &str) -> TopologyRefs {
    let first_facility = fixture.facility(tenant_id, &format!("{key} first")).await;
    let second_facility = fixture.facility(tenant_id, &format!("{key} second")).await;
    let parent_location = fixture
        .location(tenant_id, first_facility, &format!("{key}-PARENT"))
        .await;
    let child_location = repo::locations::add_location(
        &fixture.db,
        tenant_id,
        first_facility,
        Some(parent_location),
        Some(&format!("{key}-CHILD")),
        Some("child"),
        "bin",
        true,
        true,
        false,
    )
    .await
    .unwrap();
    let inventory_owner_id = fixture.inventory_owner(tenant_id, key).await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, first_facility)
        .await;

    let admin_db = admin_db_for(&fixture.db).await;
    let assignment_id: i64 = sqlx::query_scalar(
        r#"
        SELECT id
        FROM inventory_owner_facilities
        WHERE tenant_id = $1
          AND inventory_owner_id = $2
          AND facility_id = $3
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(first_facility)
    .fetch_one(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;

    TopologyRefs {
        tenant_id,
        facility_ids: [first_facility, second_facility],
        location_ids: [parent_location, child_location],
        inventory_owner_id,
        assignment_id,
    }
}

async fn assert_repository_topology(db: &db::Db, refs: TopologyRefs) {
    assert_eq!(
        repo::facilities::get_facilities(db, refs.tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .map(|facility| facility.id)
            .collect::<Vec<_>>(),
        refs.facility_ids
    );
    assert_eq!(
        repo::locations::get_locations(db, refs.tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .map(|location| location.id)
            .collect::<Vec<_>>(),
        refs.location_ids
    );

    let owners = repo::inventory_owners::get_inventory_owners(db, refs.tenant_id, false)
        .await
        .unwrap();
    assert_eq!(owners.len(), 1);
    assert_eq!(owners[0].id, refs.inventory_owner_id);
    assert_eq!(
        owners[0]
            .inventory_owner_facilities
            .iter()
            .map(|facility| facility.id)
            .collect::<Vec<_>>(),
        vec![refs.facility_ids[0]]
    );
}

async fn visible_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> (Vec<i64>, Vec<i64>, Vec<i64>, Vec<i64>) {
    sqlx::query_as(
        r#"
        SELECT ARRAY(SELECT id FROM facilities ORDER BY id),
               ARRAY(SELECT id FROM locations ORDER BY id),
               ARRAY(SELECT id FROM inventory_owners ORDER BY id),
               ARRAY(SELECT id FROM inventory_owner_facilities ORDER BY id)
        "#,
    )
    .fetch_one(&mut **tx)
    .await
    .unwrap()
}

async fn allowed_update_counts(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    refs: TopologyRefs,
) -> [u64; 2] {
    let location = sqlx::query("UPDATE locations SET name = name WHERE id = $1")
        .bind(refs.location_ids[0])
        .execute(&mut **tx)
        .await
        .unwrap()
        .rows_affected();
    let owner = sqlx::query("UPDATE inventory_owners SET name = name WHERE id = $1")
        .bind(refs.inventory_owner_id)
        .execute(&mut **tx)
        .await
        .unwrap()
        .rows_affected();
    [location, owner]
}

async fn assert_acl_restricted_updates_fail(db: &db::Db, refs: TopologyRefs) {
    for (table, id) in [
        ("facilities", refs.facility_ids[0]),
        ("inventory_owner_facilities", refs.assignment_id),
    ] {
        let mut tx = tenant_tx(db, refs.tenant_id).await;
        assert!(
            sqlx::query(&format!(
                "UPDATE {table} SET deleted = deleted WHERE id = $1"
            ))
            .bind(id)
            .execute(&mut *tx)
            .await
            .is_err(),
            "{table} must not be updateable by the runtime role"
        );
        tx.rollback().await.unwrap();
    }
}

async fn assert_deletes_fail(db: &db::Db, refs: TopologyRefs) {
    for (table, id) in [
        ("facilities", refs.facility_ids[0]),
        ("locations", refs.location_ids[0]),
        ("inventory_owners", refs.inventory_owner_id),
        ("inventory_owner_facilities", refs.assignment_id),
    ] {
        let mut tx = tenant_tx(db, refs.tenant_id).await;
        assert!(
            sqlx::query(&format!("DELETE FROM {table} WHERE id = $1"))
                .bind(id)
                .execute(&mut *tx)
                .await
                .is_err(),
            "{table} must not be deleteable by the runtime role"
        );
        tx.rollback().await.unwrap();
    }
}

async fn assert_forged_inserts_fail(
    db: &db::Db,
    context: Option<TenantId>,
    refs: TopologyRefs,
    key: &str,
) {
    for table in TABLES {
        let mut tx = db.begin().await.unwrap();
        if let Some(tenant_id) = context {
            db::bind_tenant_context(&mut tx, tenant_id).await.unwrap();
        }
        let result = match table {
            "facilities" => {
                sqlx::query("INSERT INTO facilities (tenant_id, created, name) VALUES ($1, $2, $3)")
                    .bind(refs.tenant_id.get())
                    .bind(db::now_iso())
                    .bind(format!("{key} forged facility"))
                    .execute(&mut *tx)
                    .await
            }
            "locations" => {
                sqlx::query(
                    r#"
                INSERT INTO locations
                    (tenant_id, created, facility_id, barcode, type)
                VALUES ($1, $2, $3, $4, 'bin')
                "#,
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(refs.facility_ids[0])
                .bind(format!("{key}-FORGED-LOCATION"))
                .execute(&mut *tx)
                .await
            }
            "inventory_owners" => {
                sqlx::query(
                    r#"
                INSERT INTO inventory_owners (tenant_id, created, name, email)
                VALUES ($1, $2, $3, $4)
                "#,
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(format!("{key} forged owner"))
                .bind(format!("{key}@test.local"))
                .execute(&mut *tx)
                .await
            }
            "inventory_owner_facilities" => {
                sqlx::query(
                    r#"
                INSERT INTO inventory_owner_facilities
                    (tenant_id, created, inventory_owner_id, facility_id)
                VALUES ($1, $2, $3, $4)
                "#,
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(refs.inventory_owner_id)
                .bind(refs.facility_ids[1])
                .execute(&mut *tx)
                .await
            }
            _ => unreachable!(),
        };
        assert!(
            result.is_err(),
            "{table} accepted an insert outside its tenant context"
        );
        tx.rollback().await.unwrap();
    }
}

async fn assert_exact_runtime_privileges(db: &db::Db) {
    let table_privileges: Vec<TablePrivileges> = sqlx::query_as(
        r#"
        SELECT table_name,
               has_table_privilege(current_user, 'public.' || table_name, 'SELECT')
                   AS can_select,
               has_table_privilege(current_user, 'public.' || table_name, 'INSERT')
                   AS can_insert,
               has_table_privilege(current_user, 'public.' || table_name, 'UPDATE')
                   AS can_update,
               has_table_privilege(current_user, 'public.' || table_name, 'DELETE')
                   AS can_delete,
               has_table_privilege(current_user, 'public.' || table_name, 'TRUNCATE')
                   AS can_truncate,
               has_table_privilege(current_user, 'public.' || table_name, 'REFERENCES')
                   AS can_reference,
               has_table_privilege(current_user, 'public.' || table_name, 'TRIGGER')
                   AS can_trigger
        FROM unnest($1::TEXT[]) WITH ORDINALITY AS tables(table_name, ordinal)
        ORDER BY ordinal
        "#,
    )
    .bind(TABLES.as_slice())
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(
        table_privileges,
        [
            TablePrivileges {
                table_name: "facilities".into(),
                can_select: true,
                can_insert: true,
                can_update: false,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            },
            TablePrivileges {
                table_name: "locations".into(),
                can_select: true,
                can_insert: true,
                can_update: true,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            },
            TablePrivileges {
                table_name: "inventory_owners".into(),
                can_select: true,
                can_insert: true,
                can_update: true,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            },
            TablePrivileges {
                table_name: "inventory_owner_facilities".into(),
                can_select: true,
                can_insert: false,
                can_update: false,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            },
        ]
    );

    let sequence_privileges: Vec<SequencePrivileges> = sqlx::query_as(
        r#"
        SELECT sequence_name,
               has_sequence_privilege(current_user, 'public.' || sequence_name, 'USAGE')
                   AS can_use,
               has_sequence_privilege(current_user, 'public.' || sequence_name, 'SELECT')
                   AS can_select,
               has_sequence_privilege(current_user, 'public.' || sequence_name, 'UPDATE')
                   AS can_update
        FROM unnest($1::TEXT[]) WITH ORDINALITY AS sequences(sequence_name, ordinal)
        ORDER BY ordinal
        "#,
    )
    .bind(SEQUENCES.as_slice())
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(
        sequence_privileges,
        SEQUENCES
            .iter()
            .enumerate()
            .map(|(index, sequence)| SequencePrivileges {
                sequence_name: (*sequence).to_owned(),
                can_use: index < 3,
                can_select: false,
                can_update: false,
            })
            .collect::<Vec<_>>()
    );
}

async fn snapshot(db: &db::Db, tenant_id: TenantId) -> String {
    let mut tx = tenant_tx(db, tenant_id).await;
    let snapshot: String = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'facilities',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(facility) ORDER BY id) FROM facilities facility),
                    '[]'::jsonb
                ),
            'locations',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(location) ORDER BY id) FROM locations location),
                    '[]'::jsonb
                ),
            'inventory_owners',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(owner_row) ORDER BY id)
                     FROM inventory_owners owner_row),
                    '[]'::jsonb
                ),
            'inventory_owner_facilities',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(owner_facility) ORDER BY id)
                     FROM inventory_owner_facilities owner_facility),
                    '[]'::jsonb
                )
        )::text
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    snapshot
}
