mod common;

use common::*;
use wareboxes_core::dto::AddAuditLocationCount;

const TABLES: [&str; 6] = [
    "audit_waves",
    "audit_location_counts",
    "audit_wave_items",
    "audit_wave_inventory_owners",
    "audit_wave_locations",
    "audit_wave_assignments",
];
const SEQUENCES: [&str; 6] = [
    "audit_waves_id_seq",
    "audit_location_counts_id_seq",
    "audit_wave_items_id_seq",
    "audit_wave_inventory_owners_id_seq",
    "audit_wave_locations_id_seq",
    "audit_wave_assignments_id_seq",
];

#[derive(Clone, Copy)]
struct AuditRefs {
    tenant_id: TenantId,
    user_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    location_id: i64,
    item_id: i64,
    wave_id: i64,
    count_id: i64,
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
async fn inventory_audits_require_tenant_context_and_exact_runtime_privileges() {
    let fixture = Fixture::new().await;
    assert_exact_runtime_privileges(&fixture.db).await;

    let refs_a = audit_refs(&fixture, "audit-rls-a@test.com", "AUDIT-RLS-A").await;
    let refs_b = audit_refs(&fixture, "audit-rls-b@test.com", "AUDIT-RLS-B").await;
    assert_repository_audit(&fixture.db, refs_a).await;
    assert_repository_audit(&fixture.db, refs_b).await;

    let snapshot_a = snapshot(&fixture.db, refs_a.tenant_id).await;
    let snapshot_b = snapshot(&fixture.db, refs_b.tenant_id).await;

    let unbound_counts: (i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM audit_waves),
               (SELECT COUNT(*) FROM audit_location_counts)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0));

    let mut tenant_b_tx = tenant_tx(&fixture.db, refs_b.tenant_id).await;
    let visible_b: (Vec<i64>, Vec<i64>) = sqlx::query_as(
        r#"
        SELECT ARRAY(SELECT id FROM audit_waves ORDER BY id),
               ARRAY(SELECT id FROM audit_location_counts ORDER BY id)
        "#,
    )
    .fetch_one(&mut *tenant_b_tx)
    .await
    .unwrap();
    assert_eq!(visible_b, (vec![refs_b.wave_id], vec![refs_b.count_id]));
    assert_eq!(
        cross_tenant_update_counts(&mut tenant_b_tx, refs_a).await,
        [0, 0]
    );
    tenant_b_tx.rollback().await.unwrap();

    let mut unbound = fixture.db.acquire().await.unwrap();
    assert_eq!(
        cross_tenant_update_counts(&mut unbound, refs_a).await,
        [0, 0]
    );
    drop(unbound);

    assert_dormant_tables_are_inaccessible(&fixture.db).await;
    assert_deletes_fail(&fixture.db, refs_a).await;
    assert_forged_inserts_fail(&fixture.db, None, refs_a, "UNBOUND").await;
    assert_forged_inserts_fail(&fixture.db, Some(refs_b.tenant_id), refs_a, "CROSS").await;

    assert_repository_audit(&fixture.db, refs_a).await;
    assert_repository_audit(&fixture.db, refs_b).await;
    assert_eq!(snapshot(&fixture.db, refs_a.tenant_id).await, snapshot_a);
    assert_eq!(snapshot(&fixture.db, refs_b.tenant_id).await, snapshot_b);
}

async fn audit_refs(fixture: &Fixture, email: &str, key: &str) -> AuditRefs {
    let user = fixture.user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let facility_id = fixture
        .facility(tenant_id, &format!("{key} facility"))
        .await;
    let inventory_owner_id = fixture.inventory_owner(tenant_id, key).await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let location_id = fixture.location(tenant_id, facility_id, key).await;
    let item_id = fixture
        .item(tenant_id, &format!("{key} item"), "each")
        .await;
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query(
        r#"
        INSERT INTO inventory_owner_items
            (tenant_id, created, inventory_owner_id, item_id)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(inventory_owner_id)
    .bind(item_id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let access = repo::tenants::access_for_user(&fixture.db, user.id, tenant_id)
        .await
        .unwrap()
        .unwrap();
    let wave_id = repo::audits::add_audit_wave(
        &fixture.db,
        &access,
        user.id,
        facility_id,
        inventory_owner_id,
        key,
        Some("RLS audit wave"),
    )
    .await
    .unwrap()
    .unwrap();
    let count_id = repo::audits::add_location_count(
        &fixture.db,
        &access,
        &AddAuditLocationCount {
            audit_wave_id: wave_id,
            location_id,
            item_id,
            uom: "each".to_owned(),
            lot: None,
            expiration: None,
            serial: None,
            count: 1,
        },
    )
    .await
    .unwrap()
    .unwrap();

    let admin_db = admin_db_for(&fixture.db).await;
    sqlx::query(
        r#"
        INSERT INTO audit_wave_items
            (tenant_id, created, item_id, audit_wave_id, facility_id, inventory_owner_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(item_id)
    .bind(wave_id)
    .bind(facility_id)
    .bind(inventory_owner_id)
    .execute(&admin_db)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO audit_wave_inventory_owners
            (tenant_id, created, inventory_owner_id, audit_wave_id, facility_id)
        VALUES ($1, $2, $3, $4, $5)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(inventory_owner_id)
    .bind(wave_id)
    .bind(facility_id)
    .execute(&admin_db)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO audit_wave_locations
            (tenant_id, created, location_id, audit_wave_id, auditor_id, facility_id,
             inventory_owner_id)
        VALUES ($1, $2, $3, $4, $5, $6, $7)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(location_id)
    .bind(wave_id)
    .bind(user.id)
    .bind(facility_id)
    .bind(inventory_owner_id)
    .execute(&admin_db)
    .await
    .unwrap();
    sqlx::query(
        r#"
        INSERT INTO audit_wave_assignments
            (tenant_id, created, audit_wave_id, auditor_id, facility_id, inventory_owner_id)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(tenant_id.get())
    .bind(db::now_iso())
    .bind(wave_id)
    .bind(user.id)
    .bind(facility_id)
    .bind(inventory_owner_id)
    .execute(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;

    AuditRefs {
        tenant_id,
        user_id: user.id,
        facility_id,
        inventory_owner_id,
        location_id,
        item_id,
        wave_id,
        count_id,
    }
}

async fn assert_repository_audit(db: &db::Db, refs: AuditRefs) {
    let access = repo::tenants::access_for_user(db, refs.user_id, refs.tenant_id)
        .await
        .unwrap()
        .unwrap();
    let waves = repo::audits::get_audit_waves(db, &access, false)
        .await
        .unwrap();
    assert_eq!(
        waves.iter().filter(|wave| wave.id == refs.wave_id).count(),
        1
    );
    let counts = repo::audits::get_location_counts(db, &access, refs.wave_id)
        .await
        .unwrap();
    assert_eq!(counts.len(), 1);
    assert_eq!(counts[0].id, refs.count_id);
}

async fn cross_tenant_update_counts(
    connection: &mut sqlx::PgConnection,
    refs: AuditRefs,
) -> [u64; 2] {
    let wave = sqlx::query("UPDATE audit_waves SET name = name WHERE id = $1")
        .bind(refs.wave_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let count = sqlx::query("UPDATE audit_location_counts SET count = count WHERE id = $1")
        .bind(refs.count_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    [wave, count]
}

async fn assert_dormant_tables_are_inaccessible(db: &db::Db) {
    for table in &TABLES[2..] {
        let error = sqlx::query_scalar::<_, i64>(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(db)
            .await
            .unwrap_err();
        assert_sqlstate(error, "42501");
    }
}

async fn assert_deletes_fail(db: &db::Db, refs: AuditRefs) {
    for table in TABLES {
        let mut tx = tenant_tx(db, refs.tenant_id).await;
        let error = sqlx::query(&format!("DELETE FROM {table} WHERE tenant_id = $1"))
            .bind(refs.tenant_id.get())
            .execute(&mut *tx)
            .await
            .unwrap_err();
        assert_sqlstate(error, "42501");
        tx.rollback().await.unwrap();
    }
}

async fn assert_forged_inserts_fail(
    db: &db::Db,
    context: Option<TenantId>,
    refs: AuditRefs,
    key: &str,
) {
    let mut wave_tx = db.begin().await.unwrap();
    if let Some(tenant_id) = context {
        db::bind_tenant_context(&mut wave_tx, tenant_id)
            .await
            .unwrap();
    }
    let error = sqlx::query(
        r#"
        INSERT INTO audit_waves
            (tenant_id, created, facility_id, inventory_owner_id, name, created_by)
        VALUES ($1, $2, $3, $4, $5, $6)
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(db::now_iso())
    .bind(refs.facility_id)
    .bind(refs.inventory_owner_id)
    .bind(format!("{key}-WAVE"))
    .bind(refs.user_id)
    .execute(&mut *wave_tx)
    .await
    .unwrap_err();
    assert_sqlstate(error, "42501");
    wave_tx.rollback().await.unwrap();

    let mut count_tx = db.begin().await.unwrap();
    if let Some(tenant_id) = context {
        db::bind_tenant_context(&mut count_tx, tenant_id)
            .await
            .unwrap();
    }
    let error = sqlx::query(
        r#"
        INSERT INTO audit_location_counts
            (tenant_id, created, audit_id, inventory_owner_id, facility_id, location_id,
             item_id, uom, serial, on_hand, count, approval_status)
        VALUES ($1, $2, $3, $4, $5, $6, $7, 'each', $8, 0, 0, 'pending')
        "#,
    )
    .bind(refs.tenant_id.get())
    .bind(db::now_iso())
    .bind(refs.wave_id)
    .bind(refs.inventory_owner_id)
    .bind(refs.facility_id)
    .bind(refs.location_id)
    .bind(refs.item_id)
    .bind(format!("{key}-SERIAL"))
    .execute(&mut *count_tx)
    .await
    .unwrap_err();
    assert_sqlstate(error, "42501");
    count_tx.rollback().await.unwrap();
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
        TABLES
            .iter()
            .enumerate()
            .map(|(index, table)| TablePrivileges {
                table_name: (*table).to_owned(),
                can_select: index < 2,
                can_insert: index < 2,
                can_update: index < 2,
                can_delete: false,
                can_truncate: false,
                can_reference: false,
                can_trigger: false,
            })
            .collect::<Vec<_>>()
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
                can_use: index < 2,
                can_select: false,
                can_update: false,
            })
            .collect::<Vec<_>>()
    );
}

async fn snapshot(db: &db::Db, tenant_id: TenantId) -> String {
    let admin_db = admin_db_for(db).await;
    let snapshot = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'audit_waves',
                (SELECT jsonb_agg(to_jsonb(row) ORDER BY id)
                 FROM audit_waves row WHERE tenant_id = $1),
            'audit_location_counts',
                (SELECT jsonb_agg(to_jsonb(row) ORDER BY id)
                 FROM audit_location_counts row WHERE tenant_id = $1),
            'audit_wave_items',
                (SELECT jsonb_agg(to_jsonb(row) ORDER BY id)
                 FROM audit_wave_items row WHERE tenant_id = $1),
            'audit_wave_inventory_owners',
                (SELECT jsonb_agg(to_jsonb(row) ORDER BY id)
                 FROM audit_wave_inventory_owners row WHERE tenant_id = $1),
            'audit_wave_locations',
                (SELECT jsonb_agg(to_jsonb(row) ORDER BY id)
                 FROM audit_wave_locations row WHERE tenant_id = $1),
            'audit_wave_assignments',
                (SELECT jsonb_agg(to_jsonb(row) ORDER BY id)
                 FROM audit_wave_assignments row WHERE tenant_id = $1)
        )::TEXT
        "#,
    )
    .bind(tenant_id.get())
    .fetch_one(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;
    snapshot
}

fn assert_sqlstate(error: sqlx::Error, expected: &str) {
    let code = error.as_database_error().and_then(|error| error.code());
    assert_eq!(code.as_deref(), Some(expected));
}
