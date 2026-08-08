mod common;

use common::*;
use wareboxes_core::models::LoadFileCategory;

const TABLES: [&str; 5] = [
    "loads",
    "load_lines",
    "load_notes",
    "load_files",
    "load_orders",
];

const SEQUENCES: [&str; 5] = [
    "loads_id_seq",
    "load_lines_id_seq",
    "load_notes_id_seq",
    "load_files_id_seq",
    "load_orders_id_seq",
];

#[derive(Clone, Copy)]
struct LoadRefs {
    tenant_id: TenantId,
    load_id: i64,
    line_id: i64,
    note_id: i64,
    file_id: i64,
    load_order_id: i64,
    facility_id: i64,
    inventory_owner_id: i64,
    item_id: i64,
    order_id: i64,
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
async fn load_aggregate_requires_tenant_context_and_exact_runtime_privileges() {
    let fixture = Fixture::new().await;
    assert_exact_runtime_privileges(&fixture.db).await;

    let refs_a = load_refs(&fixture, "load-rls-a@test.com", "LOAD-RLS-A").await;
    let refs_b = load_refs(&fixture, "load-rls-b@test.com", "LOAD-RLS-B").await;
    assert_repository_load(&fixture.db, refs_a).await;
    assert_repository_load(&fixture.db, refs_b).await;

    let snapshot_a = snapshot(&fixture.db, refs_a.tenant_id).await;
    let snapshot_b = snapshot(&fixture.db, refs_b.tenant_id).await;

    let unbound_counts: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM loads),
               (SELECT COUNT(*) FROM load_lines),
               (SELECT COUNT(*) FROM load_notes),
               (SELECT COUNT(*) FROM load_files),
               (SELECT COUNT(*) FROM load_orders)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0, 0, 0, 0));

    let mut unbound = fixture.db.acquire().await.unwrap();
    assert_eq!(allowed_update_counts(&mut unbound, refs_a).await, [0; 4]);
    drop(unbound);

    let mut tenant_b_tx = tenant_tx(&fixture.db, refs_b.tenant_id).await;
    assert_eq!(
        visible_ids(&mut tenant_b_tx).await,
        (
            vec![refs_b.load_id],
            vec![refs_b.line_id],
            vec![refs_b.note_id],
            vec![refs_b.file_id],
            vec![refs_b.load_order_id],
        )
    );
    assert_eq!(
        allowed_update_counts(&mut tenant_b_tx, refs_a).await,
        [0; 4]
    );
    tenant_b_tx.rollback().await.unwrap();

    assert_load_order_updates_fail(&fixture.db, refs_a).await;
    assert_deletes_fail(&fixture.db, refs_a).await;
    assert_forged_inserts_fail(&fixture.db, None, refs_a, "UNBOUND").await;
    assert_forged_inserts_fail(&fixture.db, Some(refs_b.tenant_id), refs_a, "CROSS").await;

    assert_repository_load(&fixture.db, refs_a).await;
    assert_repository_load(&fixture.db, refs_b).await;
    assert_eq!(snapshot(&fixture.db, refs_a.tenant_id).await, snapshot_a);
    assert_eq!(snapshot(&fixture.db, refs_b.tenant_id).await, snapshot_b);
}

async fn load_refs(fixture: &Fixture, email: &str, key: &str) -> LoadRefs {
    let user = fixture.user(email).await;
    let tenant_id = tenant_for_user(&fixture.db, user.id).await;
    let facility_id = fixture
        .facility(tenant_id, &format!("{key} facility"))
        .await;
    let inventory_owner_id = fixture.inventory_owner(tenant_id, key).await;
    fixture
        .assign_owner_to_facility(tenant_id, inventory_owner_id, facility_id)
        .await;
    let item_id = fixture
        .item(tenant_id, &format!("{key} item"), "each")
        .await;
    let order_id = fixture
        .order_header(tenant_id, &format!("{key}-ORDER"), inventory_owner_id)
        .await;

    let load_id = repo::loads::add_load(
        &fixture.db,
        tenant_id,
        user.id,
        facility_id,
        inventory_owner_id,
        LoadType::Inbound,
        Some(key),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let line_id = repo::loads::add_line(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        item_id,
        None,
        1,
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let note_id = repo::loads::add_note(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        &format!("{key} note"),
    )
    .await
    .unwrap();
    let file_id = repo::loads::add_file(
        &fixture.db,
        tenant_id,
        user.id,
        load_id,
        &format!("{key}.txt"),
        &format!("{key}-stored.txt"),
        &format!("/tmp/{key}.txt"),
        Some("text/plain"),
        LoadFileCategory::General,
    )
    .await
    .unwrap();

    let admin_db = admin_db_for(&fixture.db).await;
    let load_order_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO load_orders
            (tenant_id, inventory_owner_id, created, load_id, order_id)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING id
        "#,
    )
    .bind(tenant_id.get())
    .bind(inventory_owner_id)
    .bind(db::now_iso())
    .bind(load_id)
    .bind(order_id)
    .fetch_one(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;

    LoadRefs {
        tenant_id,
        load_id,
        line_id,
        note_id,
        file_id,
        load_order_id,
        facility_id,
        inventory_owner_id,
        item_id,
        order_id,
    }
}

async fn assert_repository_load(db: &db::Db, refs: LoadRefs) {
    let load = repo::loads::get_load(db, refs.tenant_id, refs.load_id, false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(load.id, refs.load_id);
    assert_eq!(load.lines.len(), 1);
    assert_eq!(load.lines[0].id, refs.line_id);
    assert_eq!(load.notes.len(), 1);
    assert_eq!(load.notes[0].id, refs.note_id);
    assert_eq!(load.files.len(), 1);
    assert_eq!(load.files[0].id, refs.file_id);
    assert_eq!(load.orders.len(), 1);
    assert_eq!(load.orders[0].id, refs.order_id);
}

async fn visible_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> (Vec<i64>, Vec<i64>, Vec<i64>, Vec<i64>, Vec<i64>) {
    sqlx::query_as(
        r#"
        SELECT ARRAY(SELECT id FROM loads ORDER BY id),
               ARRAY(SELECT id FROM load_lines ORDER BY id),
               ARRAY(SELECT id FROM load_notes ORDER BY id),
               ARRAY(SELECT id FROM load_files ORDER BY id),
               ARRAY(SELECT id FROM load_orders ORDER BY id)
        "#,
    )
    .fetch_one(&mut **tx)
    .await
    .unwrap()
}

async fn allowed_update_counts(connection: &mut sqlx::PgConnection, refs: LoadRefs) -> [u64; 4] {
    let load = sqlx::query("UPDATE loads SET carrier = carrier WHERE id = $1")
        .bind(refs.load_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let line = sqlx::query("UPDATE load_lines SET expected_qty = expected_qty WHERE id = $1")
        .bind(refs.line_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let note = sqlx::query("UPDATE load_notes SET note = note WHERE id = $1")
        .bind(refs.note_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let file = sqlx::query("UPDATE load_files SET name = name WHERE id = $1")
        .bind(refs.file_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    [load, line, note, file]
}

async fn assert_load_order_updates_fail(db: &db::Db, refs: LoadRefs) {
    let mut tx = tenant_tx(db, refs.tenant_id).await;
    assert!(
        sqlx::query("UPDATE load_orders SET deleted = deleted WHERE id = $1")
            .bind(refs.load_order_id)
            .execute(&mut *tx)
            .await
            .is_err(),
        "load_orders must not be updateable by the runtime role"
    );
    tx.rollback().await.unwrap();
}

async fn assert_deletes_fail(db: &db::Db, refs: LoadRefs) {
    for (table, id) in [
        ("loads", refs.load_id),
        ("load_lines", refs.line_id),
        ("load_notes", refs.note_id),
        ("load_files", refs.file_id),
        ("load_orders", refs.load_order_id),
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
    refs: LoadRefs,
    key: &str,
) {
    for table in TABLES {
        let mut tx = db.begin().await.unwrap();
        if let Some(tenant_id) = context {
            db::bind_tenant_context(&mut tx, tenant_id).await.unwrap();
        }
        let result = match table {
            "loads" => {
                sqlx::query(
                    r#"
                    INSERT INTO loads
                        (tenant_id, created, facility_id, inventory_owner_id,
                         execution_barcode, type)
                    VALUES ($1, $2, $3, $4, $5, 'inbound')
                    "#,
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(refs.facility_id)
                .bind(refs.inventory_owner_id)
                .bind(format!("FORGED-{key}"))
                .execute(&mut *tx)
                .await
            }
            "load_lines" => {
                sqlx::query(
                    r#"
                    INSERT INTO load_lines
                        (tenant_id, created, load_id, item_id, expected_qty)
                    VALUES ($1, $2, $3, $4, 1)
                    "#,
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(refs.load_id)
                .bind(refs.item_id)
                .execute(&mut *tx)
                .await
            }
            "load_notes" => {
                sqlx::query(
                    r#"
                    INSERT INTO load_notes (tenant_id, created, load_id, note)
                    VALUES ($1, $2, $3, $4)
                    "#,
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(refs.load_id)
                .bind(format!("{key} forged note"))
                .execute(&mut *tx)
                .await
            }
            "load_files" => {
                sqlx::query(
                    r#"
                    INSERT INTO load_files
                        (tenant_id, created, load_id, original_name, name, path)
                    VALUES ($1, $2, $3, $4, $4, $5)
                    "#,
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(refs.load_id)
                .bind(format!("{key}-forged.txt"))
                .bind(format!("/tmp/{key}-forged.txt"))
                .execute(&mut *tx)
                .await
            }
            "load_orders" => {
                sqlx::query(
                    r#"
                    INSERT INTO load_orders
                        (tenant_id, inventory_owner_id, created, load_id, order_id)
                    VALUES ($1, $2, $3, $4, $5)
                    "#,
                )
                .bind(refs.tenant_id.get())
                .bind(refs.inventory_owner_id)
                .bind(db::now_iso())
                .bind(refs.load_id)
                .bind(refs.order_id)
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
        TABLES
            .iter()
            .enumerate()
            .map(|(index, table)| TablePrivileges {
                table_name: (*table).to_owned(),
                can_select: true,
                can_insert: index < 4,
                can_update: index < 4,
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
                can_use: index < 4,
                can_select: false,
                can_update: false,
            })
            .collect::<Vec<_>>()
    );
}

async fn snapshot(db: &db::Db, tenant_id: TenantId) -> String {
    let mut tx = tenant_tx(db, tenant_id).await;
    let snapshot = sqlx::query_scalar(
        r#"
        SELECT jsonb_build_object(
            'loads',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(load) ORDER BY id) FROM loads load),
                    '[]'::jsonb
                ),
            'load_lines',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(line) ORDER BY id) FROM load_lines line),
                    '[]'::jsonb
                ),
            'load_notes',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(note) ORDER BY id) FROM load_notes note),
                    '[]'::jsonb
                ),
            'load_files',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(file) ORDER BY id) FROM load_files file),
                    '[]'::jsonb
                ),
            'load_orders',
                COALESCE(
                    (SELECT jsonb_agg(to_jsonb(load_order) ORDER BY id)
                     FROM load_orders load_order),
                    '[]'::jsonb
                )
        )::TEXT
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    snapshot
}
