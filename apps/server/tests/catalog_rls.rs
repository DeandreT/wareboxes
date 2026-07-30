mod common;

use common::*;

const TABLES: [&str; 7] = [
    "dims",
    "items",
    "skus",
    "barcodes",
    "item_pack_links",
    "inventory_owner_items",
    "item_batches",
];

const SEQUENCES: [&str; 7] = [
    "dims_id_seq",
    "items_id_seq",
    "skus_id_seq",
    "barcodes_id_seq",
    "item_pack_links_id_seq",
    "inventory_owner_items_id_seq",
    "item_batches_id_seq",
];

#[derive(Clone, Copy, Debug)]
struct CatalogRefs {
    tenant_id: TenantId,
    inventory_owner_id: i64,
    dims_ids: [i64; 2],
    item_ids: [i64; 2],
    sku_id: i64,
    barcode_id: i64,
    item_pack_link_id: i64,
    inventory_owner_item_id: i64,
    item_batch_id: i64,
}

#[derive(Clone, Copy, Debug)]
enum CatalogTable {
    Dims,
    Items,
    Skus,
    Barcodes,
    ItemPackLinks,
    InventoryOwnerItems,
    ItemBatches,
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

impl TablePrivileges {
    fn new(table_name: &str, can_update: bool) -> Self {
        Self {
            table_name: table_name.to_owned(),
            can_select: true,
            can_insert: true,
            can_update,
            can_delete: false,
            can_truncate: false,
            can_reference: false,
            can_trigger: false,
        }
    }
}

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct SequencePrivileges {
    sequence_name: String,
    can_use: bool,
    can_select: bool,
    can_update: bool,
}

impl CatalogTable {
    const ALL: [Self; 7] = [
        Self::Dims,
        Self::Items,
        Self::Skus,
        Self::Barcodes,
        Self::ItemPackLinks,
        Self::InventoryOwnerItems,
        Self::ItemBatches,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::Dims => "dims",
            Self::Items => "items",
            Self::Skus => "skus",
            Self::Barcodes => "barcodes",
            Self::ItemPackLinks => "item_pack_links",
            Self::InventoryOwnerItems => "inventory_owner_items",
            Self::ItemBatches => "item_batches",
        }
    }

    fn id(self, refs: CatalogRefs) -> i64 {
        match self {
            Self::Dims => refs.dims_ids[0],
            Self::Items => refs.item_ids[0],
            Self::Skus => refs.sku_id,
            Self::Barcodes => refs.barcode_id,
            Self::ItemPackLinks => refs.item_pack_link_id,
            Self::InventoryOwnerItems => refs.inventory_owner_item_id,
            Self::ItemBatches => refs.item_batch_id,
        }
    }
}

#[tokio::test]
async fn catalog_requires_a_transaction_local_tenant_context_and_exact_privileges() {
    let fixture = Fixture::new().await;
    assert_exact_runtime_privileges(&fixture.db).await;

    let user_a = fixture.user("catalog-rls-a@test.com").await;
    let user_b = fixture.user("catalog-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let refs_a = catalog_refs(&fixture, tenant_a, "CATALOG-RLS-A").await;
    let refs_b = catalog_refs(&fixture, tenant_b, "CATALOG-RLS-B").await;

    assert_repository_catalog(&fixture.db, refs_a).await;
    assert_repository_catalog(&fixture.db, refs_b).await;
    let source_a = snapshot(&fixture.db, tenant_a).await;
    let source_b = snapshot(&fixture.db, tenant_b).await;

    let unbound_counts: (i64, i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM dims),
               (SELECT COUNT(*) FROM items),
               (SELECT COUNT(*) FROM skus),
               (SELECT COUNT(*) FROM barcodes),
               (SELECT COUNT(*) FROM item_pack_links),
               (SELECT COUNT(*) FROM inventory_owner_items),
               (SELECT COUNT(*) FROM item_batches)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0, 0, 0, 0, 0, 0));

    let mut unbound_connection = fixture.db.acquire().await.unwrap();
    assert_eq!(
        permitted_update_counts(&mut unbound_connection, refs_a).await,
        [0; 5]
    );
    drop(unbound_connection);
    assert_restricted_updates_fail(&fixture.db, None, refs_a).await;
    assert_deletes_fail(&fixture.db, None, refs_a).await;
    assert_forged_inserts_fail(&fixture.db, None, refs_a, "UNBOUND").await;

    let mut tenant_b_tx = tenant_tx(&fixture.db, tenant_b).await;
    let visible_ids = visible_ids(&mut tenant_b_tx).await;
    assert_eq!(
        visible_ids,
        (
            refs_b.dims_ids.to_vec(),
            refs_b.item_ids.to_vec(),
            vec![refs_b.sku_id],
            vec![refs_b.barcode_id],
            vec![refs_b.item_pack_link_id],
            vec![refs_b.inventory_owner_item_id],
            vec![refs_b.item_batch_id],
        )
    );
    assert_eq!(
        permitted_update_counts(&mut tenant_b_tx, refs_a).await,
        [0; 5]
    );
    tenant_b_tx.rollback().await.unwrap();

    assert_restricted_updates_fail(&fixture.db, Some(tenant_b), refs_a).await;
    assert_deletes_fail(&fixture.db, Some(tenant_b), refs_a).await;
    assert_forged_inserts_fail(&fixture.db, Some(tenant_b), refs_a, "CROSS-TENANT").await;

    assert_repository_catalog(&fixture.db, refs_a).await;
    assert_repository_catalog(&fixture.db, refs_b).await;
    assert_eq!(snapshot(&fixture.db, tenant_a).await, source_a);
    assert_eq!(snapshot(&fixture.db, tenant_b).await, source_b);
}

async fn catalog_refs(fixture: &Fixture, tenant_id: TenantId, key: &str) -> CatalogRefs {
    let master_item_id = repo::items::add_item(
        &fixture.db,
        tenant_id,
        &format!("{key} master"),
        None,
        "case",
        Some(12),
        Some(8),
        Some(6),
        Some("in"),
        Some(10),
        Some("lb"),
    )
    .await
    .unwrap();
    let single_item_id = fixture
        .item(tenant_id, &format!("{key} single"), "each")
        .await;
    let sku_id = repo::items::add_sku(
        &fixture.db,
        tenant_id,
        master_item_id,
        &format!("{key}-SKU"),
        None,
    )
    .await
    .unwrap();
    let barcode_id = repo::items::add_barcode(
        &fixture.db,
        tenant_id,
        master_item_id,
        &format!("{key}-BARCODE"),
        "code128",
        None,
    )
    .await
    .unwrap();
    let item_pack_link_id = repo::items::add_item_pack_link(
        &fixture.db,
        tenant_id,
        master_item_id,
        single_item_id,
        12,
        None,
    )
    .await
    .unwrap();
    let inventory_owner_id = fixture.inventory_owner(tenant_id, key).await;
    let item_batch_id = repo::inventory::add_item_batch(
        &fixture.db,
        tenant_id,
        inventory_owner_id,
        master_item_id,
        None,
        Some(key),
        None,
        None,
    )
    .await
    .unwrap();

    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    let dims_ids: Vec<i64> = sqlx::query_scalar("SELECT dims_id FROM items ORDER BY id")
        .fetch_all(&mut *tx)
        .await
        .unwrap();
    let inventory_owner_item_id: i64 = sqlx::query_scalar(
        r#"
        SELECT id
        FROM inventory_owner_items
        WHERE inventory_owner_id = $1 AND item_id = $2
        "#,
    )
    .bind(inventory_owner_id)
    .bind(master_item_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();

    CatalogRefs {
        tenant_id,
        inventory_owner_id,
        dims_ids: dims_ids.try_into().unwrap(),
        item_ids: [master_item_id, single_item_id],
        sku_id,
        barcode_id,
        item_pack_link_id,
        inventory_owner_item_id,
        item_batch_id,
    }
}

async fn assert_repository_catalog(db: &db::Db, refs: CatalogRefs) {
    let items = repo::items::get_items(db, refs.tenant_id, false)
        .await
        .unwrap();
    assert_eq!(
        items.iter().map(|item| item.id).collect::<Vec<_>>(),
        refs.item_ids
    );
    let master = items
        .iter()
        .find(|item| item.id == refs.item_ids[0])
        .unwrap();
    assert_eq!(
        master.skus.iter().map(|sku| sku.id).collect::<Vec<_>>(),
        vec![refs.sku_id]
    );
    assert_eq!(
        master
            .barcodes
            .iter()
            .map(|barcode| barcode.id)
            .collect::<Vec<_>>(),
        vec![refs.barcode_id]
    );
    assert_eq!(
        repo::items::get_item_pack_links(db, refs.tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .map(|link| link.id)
            .collect::<Vec<_>>(),
        vec![refs.item_pack_link_id]
    );
    assert_eq!(
        repo::inventory::get_item_batches(db, refs.tenant_id, false)
            .await
            .unwrap()
            .into_iter()
            .map(|batch| batch.id)
            .collect::<Vec<_>>(),
        vec![refs.item_batch_id]
    );
}

async fn visible_ids(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> (
    Vec<i64>,
    Vec<i64>,
    Vec<i64>,
    Vec<i64>,
    Vec<i64>,
    Vec<i64>,
    Vec<i64>,
) {
    sqlx::query_as(
        r#"
        SELECT ARRAY(SELECT id FROM dims ORDER BY id),
               ARRAY(SELECT id FROM items ORDER BY id),
               ARRAY(SELECT id FROM skus ORDER BY id),
               ARRAY(SELECT id FROM barcodes ORDER BY id),
               ARRAY(SELECT id FROM item_pack_links ORDER BY id),
               ARRAY(SELECT id FROM inventory_owner_items ORDER BY id),
               ARRAY(SELECT id FROM item_batches ORDER BY id)
        "#,
    )
    .fetch_one(&mut **tx)
    .await
    .unwrap()
}

async fn permitted_update_counts(
    connection: &mut sqlx::PgConnection,
    refs: CatalogRefs,
) -> [u64; 5] {
    let item = sqlx::query("UPDATE items SET description = description WHERE id = $1")
        .bind(refs.item_ids[0])
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let barcode = sqlx::query("UPDATE barcodes SET notes = notes WHERE id = $1")
        .bind(refs.barcode_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let pack_link = sqlx::query("UPDATE item_pack_links SET notes = notes WHERE id = $1")
        .bind(refs.item_pack_link_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    let owner_item =
        sqlx::query("UPDATE inventory_owner_items SET deleted = deleted WHERE id = $1")
            .bind(refs.inventory_owner_item_id)
            .execute(&mut *connection)
            .await
            .unwrap()
            .rows_affected();
    let batch = sqlx::query("UPDATE item_batches SET deleted = deleted WHERE id = $1")
        .bind(refs.item_batch_id)
        .execute(&mut *connection)
        .await
        .unwrap()
        .rows_affected();
    [item, barcode, pack_link, owner_item, batch]
}

async fn assert_restricted_updates_fail(db: &db::Db, context: Option<TenantId>, refs: CatalogRefs) {
    for (table, id) in [("dims", refs.dims_ids[0]), ("skus", refs.sku_id)] {
        let mut tx = db.begin().await.unwrap();
        if let Some(tenant_id) = context {
            db::bind_tenant_context(&mut tx, tenant_id).await.unwrap();
        }
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

async fn assert_deletes_fail(db: &db::Db, context: Option<TenantId>, refs: CatalogRefs) {
    for table in CatalogTable::ALL {
        let mut tx = db.begin().await.unwrap();
        if let Some(tenant_id) = context {
            db::bind_tenant_context(&mut tx, tenant_id).await.unwrap();
        }
        assert!(
            sqlx::query(&format!("DELETE FROM {} WHERE id = $1", table.name()))
                .bind(table.id(refs))
                .execute(&mut *tx)
                .await
                .is_err(),
            "{} must not be deleteable by the runtime role",
            table.name()
        );
        tx.rollback().await.unwrap();
    }
}

async fn assert_forged_inserts_fail(
    db: &db::Db,
    context: Option<TenantId>,
    refs: CatalogRefs,
    key: &str,
) {
    for table in CatalogTable::ALL {
        let mut tx = db.begin().await.unwrap();
        if let Some(tenant_id) = context {
            db::bind_tenant_context(&mut tx, tenant_id).await.unwrap();
        }
        let result =
            match table {
                CatalogTable::Dims => {
                    sqlx::query("INSERT INTO dims (tenant_id, created) VALUES ($1, $2)")
                        .bind(refs.tenant_id.get())
                        .bind(db::now_iso())
                        .execute(&mut *tx)
                        .await
                }
                CatalogTable::Items => {
                    sqlx::query(
                        r#"
                    INSERT INTO items
                        (tenant_id, created, description, packaging_unit, dims_id)
                    VALUES ($1, $2, $3, 'each', $4)
                    "#,
                    )
                    .bind(refs.tenant_id.get())
                    .bind(db::now_iso())
                    .bind(format!("{key} forged item"))
                    .bind(refs.dims_ids[0])
                    .execute(&mut *tx)
                    .await
                }
                CatalogTable::Skus => sqlx::query(
                    "INSERT INTO skus (tenant_id, created, name, item_id) VALUES ($1, $2, $3, $4)",
                )
                .bind(refs.tenant_id.get())
                .bind(db::now_iso())
                .bind(format!("{key}-FORGED-SKU"))
                .bind(refs.item_ids[0])
                .execute(&mut *tx)
                .await,
                CatalogTable::Barcodes => {
                    sqlx::query(
                        r#"
                    INSERT INTO barcodes (tenant_id, created, name, type, item_id)
                    VALUES ($1, $2, $3, 'code128', $4)
                    "#,
                    )
                    .bind(refs.tenant_id.get())
                    .bind(db::now_iso())
                    .bind(format!("{key}-FORGED-BARCODE"))
                    .bind(refs.item_ids[0])
                    .execute(&mut *tx)
                    .await
                }
                CatalogTable::ItemPackLinks => {
                    sqlx::query(
                        r#"
                    INSERT INTO item_pack_links
                        (tenant_id, created, master_item_id, single_item_id, inner_qty)
                    VALUES ($1, $2, $3, $4, 99)
                    "#,
                    )
                    .bind(refs.tenant_id.get())
                    .bind(db::now_iso())
                    .bind(refs.item_ids[0])
                    .bind(refs.item_ids[1])
                    .execute(&mut *tx)
                    .await
                }
                CatalogTable::InventoryOwnerItems => {
                    sqlx::query(
                        r#"
                    INSERT INTO inventory_owner_items
                        (tenant_id, created, inventory_owner_id, item_id)
                    VALUES ($1, $2, $3, $4)
                    "#,
                    )
                    .bind(refs.tenant_id.get())
                    .bind(db::now_iso())
                    .bind(refs.inventory_owner_id)
                    .bind(refs.item_ids[1])
                    .execute(&mut *tx)
                    .await
                }
                CatalogTable::ItemBatches => {
                    sqlx::query(
                        r#"
                    INSERT INTO item_batches
                        (tenant_id, inventory_owner_id, created, item_id, uom, lot)
                    VALUES ($1, $2, $3, $4, 'case', $5)
                    "#,
                    )
                    .bind(refs.tenant_id.get())
                    .bind(refs.inventory_owner_id)
                    .bind(db::now_iso())
                    .bind(refs.item_ids[0])
                    .bind(format!("{key}-FORGED-LOT"))
                    .execute(&mut *tx)
                    .await
                }
            };
        assert!(
            result.is_err(),
            "{} accepted an insert outside its tenant context",
            table.name()
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
    let expected_table_privileges = [
        TablePrivileges::new("dims", false),
        TablePrivileges::new("items", true),
        TablePrivileges::new("skus", false),
        TablePrivileges::new("barcodes", true),
        TablePrivileges::new("item_pack_links", true),
        TablePrivileges::new("inventory_owner_items", true),
        TablePrivileges::new("item_batches", true),
    ];
    assert_eq!(table_privileges, expected_table_privileges);

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
            .map(|sequence| SequencePrivileges {
                sequence_name: (*sequence).to_owned(),
                can_use: true,
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
            'dims', (SELECT jsonb_agg(to_jsonb(row) ORDER BY id) FROM dims row),
            'items', (SELECT jsonb_agg(to_jsonb(row) ORDER BY id) FROM items row),
            'skus', (SELECT jsonb_agg(to_jsonb(row) ORDER BY id) FROM skus row),
            'barcodes', (SELECT jsonb_agg(to_jsonb(row) ORDER BY id) FROM barcodes row),
            'item_pack_links', (
                SELECT jsonb_agg(to_jsonb(row) ORDER BY id) FROM item_pack_links row
            ),
            'inventory_owner_items', (
                SELECT jsonb_agg(to_jsonb(row) ORDER BY id) FROM inventory_owner_items row
            ),
            'item_batches', (
                SELECT jsonb_agg(to_jsonb(row) ORDER BY id) FROM item_batches row
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
