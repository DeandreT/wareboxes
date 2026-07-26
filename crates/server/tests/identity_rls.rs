mod common;

use common::*;
use sha2::{Digest, Sha256};
use wareboxes_core::dto::UpdateUserAccessScope;

const SCOPE_TABLES: [&str; 3] = [
    "tenant_memberships",
    "user_facilities",
    "user_inventory_owners",
];
const SCOPE_SEQUENCES: [&str; 3] = [
    "tenant_memberships_id_seq",
    "user_facilities_id_seq",
    "user_inventory_owners_id_seq",
];
const SESSION_FUNCTIONS: [&str; 3] = [
    "public.session_user_id(text)",
    "public.create_session_record(text,bigint)",
    "public.destroy_session_record(text)",
];

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

#[derive(Debug, PartialEq, Eq, sqlx::FromRow)]
struct FunctionSecurity {
    signature: String,
    app_can_execute: bool,
    public_can_execute: bool,
    security_definer: bool,
    settings: Vec<String>,
}

#[derive(Clone, Copy)]
struct ScopeRefs {
    tenant_id: TenantId,
    user_id: i64,
    facility_id: i64,
    spare_facility_id: i64,
    inventory_owner_id: i64,
    spare_inventory_owner_id: i64,
}

#[tokio::test]
async fn identity_scopes_require_tenant_or_authenticated_session_context() {
    let fixture = Fixture::new().await;
    assert_exact_runtime_privileges(&fixture.db).await;
    db::validate_runtime_role(&fixture.db).await.unwrap();

    let user_a = fixture.user("identity-rls-a@test.com").await;
    let user_b = fixture.user("identity-rls-b@test.com").await;
    let tenant_a = tenant_for_user(&fixture.db, user_a.id).await;
    let tenant_b = tenant_for_user(&fixture.db, user_b.id).await;
    let tenant_c = create_tenant(&fixture.db, "identity-rls-c", "Identity RLS C").await;

    let mut tenant_c_tx = tenant_tx(&fixture.db, tenant_c).await;
    sqlx::query("INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)")
        .bind(tenant_c.get())
        .bind(user_a.id)
        .execute(&mut *tenant_c_tx)
        .await
        .unwrap();
    tenant_c_tx.commit().await.unwrap();

    let refs_a = create_restricted_scope(&fixture, tenant_a, user_a.id, "A").await;
    let refs_b = create_restricted_scope(&fixture, tenant_b, user_b.id, "B").await;
    let refs_c = create_restricted_scope(&fixture, tenant_c, user_a.id, "C").await;

    let unbound_counts: (i64, i64, i64) = sqlx::query_as(
        r#"
        SELECT (SELECT COUNT(*) FROM tenant_memberships),
               (SELECT COUNT(*) FROM user_facilities),
               (SELECT COUNT(*) FROM user_inventory_owners)
        "#,
    )
    .fetch_one(&fixture.db)
    .await
    .unwrap();
    assert_eq!(unbound_counts, (0, 0, 0));
    let sessions_error = sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM sessions")
        .fetch_one(&fixture.db)
        .await
        .unwrap_err();
    assert_sqlstate(sessions_error, "42501");

    let token_a = auth::create_session(&fixture.db, user_a.id).await.unwrap();
    let token_b = auth::create_session(&fixture.db, user_b.id).await.unwrap();
    let admin_db = admin_db_for(&fixture.db).await;
    let database_owned_expiry: bool = sqlx::query_scalar(
        r#"
        SELECT created BETWEEN CURRENT_TIMESTAMP - INTERVAL '1 minute'
                           AND CURRENT_TIMESTAMP
           AND expires = created + INTERVAL '30 days'
        FROM sessions
        WHERE token = $1
        "#,
    )
    .bind(token_hash(&token_a))
    .fetch_one(&admin_db)
    .await
    .unwrap();
    assert!(database_owned_expiry);

    assert_session_access(&fixture.db, &token_a, &[(refs_a, true), (refs_c, false)]).await;
    assert_session_access(&fixture.db, &token_b, &[(refs_b, true)]).await;
    assert_session_rows(
        &fixture.db,
        &token_a,
        &[tenant_a, tenant_c],
        &[refs_a.facility_id, refs_c.facility_id],
        &[refs_a.inventory_owner_id, refs_c.inventory_owner_id],
    )
    .await;

    let mut session_tx = db::begin_session_transaction(&fixture.db, &token_hash(&token_a))
        .await
        .unwrap();
    let session_updates =
        sqlx::query("UPDATE tenant_memberships SET all_facilities = TRUE WHERE tenant_id = $1")
            .bind(tenant_a.get())
            .execute(&mut *session_tx)
            .await
            .unwrap()
            .rows_affected();
    assert_eq!(session_updates, 0);
    assert!(
        db::bind_session_context(&mut session_tx, &token_hash(&token_b))
            .await
            .is_err()
    );
    assert!(db::bind_tenant_context(&mut session_tx, tenant_a)
        .await
        .is_err());
    session_tx.rollback().await.unwrap();

    let mut tenant_context = tenant_tx(&fixture.db, tenant_a).await;
    assert!(
        db::bind_session_context(&mut tenant_context, &token_hash(&token_a))
            .await
            .is_err()
    );
    tenant_context.rollback().await.unwrap();

    assert_tenant_isolation(&fixture.db, refs_a, refs_b).await;
    assert_cross_tenant_inserts_fail(&fixture.db, tenant_b, refs_a, user_b.id).await;

    assert!(
        auth::tenant_accesses_for_session(&fixture.db, "guessed-session-token")
            .await
            .unwrap()
            .is_empty()
    );

    let destroyed_token = auth::create_session(&fixture.db, user_a.id).await.unwrap();
    auth::destroy_session(&fixture.db, &destroyed_token)
        .await
        .unwrap();
    assert!(
        auth::tenant_accesses_for_session(&fixture.db, &destroyed_token)
            .await
            .unwrap()
            .is_empty()
    );

    let expired_token = auth::create_session(&fixture.db, user_a.id).await.unwrap();
    expire_session(&fixture.db, &expired_token).await;
    assert!(
        auth::tenant_accesses_for_session(&fixture.db, &expired_token)
            .await
            .unwrap()
            .is_empty()
    );

    assert_session_access(&fixture.db, &token_a, &[(refs_a, true), (refs_c, false)]).await;
}

async fn create_tenant(db: &db::Db, slug: &str, name: &str) -> TenantId {
    let admin_db = admin_db_for(db).await;
    let id: i64 =
        sqlx::query_scalar("INSERT INTO tenants (slug, name) VALUES ($1, $2) RETURNING id")
            .bind(slug)
            .bind(name)
            .fetch_one(&admin_db)
            .await
            .unwrap();
    admin_db.close().await;
    TenantId::new(id).unwrap()
}

async fn create_restricted_scope(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    key: &str,
) -> ScopeRefs {
    let facility_id = fixture
        .facility(tenant_id, &format!("Identity RLS {key} facility"))
        .await;
    let spare_facility_id = fixture
        .facility(tenant_id, &format!("Identity RLS {key} spare facility"))
        .await;
    let inventory_owner_id = fixture
        .inventory_owner(tenant_id, &format!("Identity RLS {key} owner"))
        .await;
    let spare_inventory_owner_id = fixture
        .inventory_owner(tenant_id, &format!("Identity RLS {key} spare owner"))
        .await;
    let updated = repo::tenants::update_user_access_scope(
        &fixture.db,
        tenant_id,
        &UpdateUserAccessScope {
            user_id,
            all_facilities: false,
            facility_ids: vec![facility_id],
            all_inventory_owners: false,
            inventory_owner_ids: vec![inventory_owner_id],
        },
    )
    .await
    .unwrap();
    assert!(updated);

    ScopeRefs {
        tenant_id,
        user_id,
        facility_id,
        spare_facility_id,
        inventory_owner_id,
        spare_inventory_owner_id,
    }
}

async fn assert_session_access(db: &db::Db, token: &str, expected: &[(ScopeRefs, bool)]) {
    let access = auth::tenant_accesses_for_session(db, token).await.unwrap();
    assert_eq!(access.len(), expected.len());
    for (refs, is_default) in expected {
        let tenant_access = access
            .iter()
            .find(|access| access.tenant_id == refs.tenant_id)
            .unwrap();
        assert_eq!(tenant_access.user_id.get(), refs.user_id);
        assert_eq!(tenant_access.is_default, *is_default);
        assert!(!tenant_access.site_scope.all_facilities);
        assert_eq!(
            tenant_access
                .site_scope
                .facility_ids
                .iter()
                .map(|id| id.get())
                .collect::<Vec<_>>(),
            vec![refs.facility_id]
        );
        assert!(!tenant_access.owner_scope.all_inventory_owners);
        assert_eq!(
            tenant_access
                .owner_scope
                .inventory_owner_ids
                .iter()
                .map(|id| id.get())
                .collect::<Vec<_>>(),
            vec![refs.inventory_owner_id]
        );
    }
}

async fn assert_session_rows(
    db: &db::Db,
    token: &str,
    expected_tenants: &[TenantId],
    expected_facilities: &[i64],
    expected_inventory_owners: &[i64],
) {
    let mut tx = db::begin_session_transaction(db, &token_hash(token))
        .await
        .unwrap();
    let rows: (Vec<i64>, Vec<i64>, Vec<i64>) = sqlx::query_as(
        r#"
        SELECT ARRAY(SELECT tenant_id FROM tenant_memberships ORDER BY tenant_id),
               ARRAY(SELECT facility_id FROM user_facilities ORDER BY facility_id),
               ARRAY(
                   SELECT inventory_owner_id
                   FROM user_inventory_owners
                   ORDER BY inventory_owner_id
               )
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.rollback().await.unwrap();

    let mut expected_tenants = expected_tenants
        .iter()
        .map(|tenant_id| tenant_id.get())
        .collect::<Vec<_>>();
    expected_tenants.sort_unstable();
    let mut expected_facilities = expected_facilities.to_vec();
    expected_facilities.sort_unstable();
    let mut expected_inventory_owners = expected_inventory_owners.to_vec();
    expected_inventory_owners.sort_unstable();
    assert_eq!(
        rows,
        (
            expected_tenants,
            expected_facilities,
            expected_inventory_owners
        )
    );
}

async fn assert_tenant_isolation(db: &db::Db, foreign: ScopeRefs, local: ScopeRefs) {
    let mut tx = tenant_tx(db, local.tenant_id).await;
    let visible: (Vec<i64>, Vec<i64>, Vec<i64>) = sqlx::query_as(
        r#"
        SELECT ARRAY(SELECT tenant_id FROM tenant_memberships ORDER BY tenant_id),
               ARRAY(SELECT facility_id FROM user_facilities ORDER BY facility_id),
               ARRAY(
                   SELECT inventory_owner_id
                   FROM user_inventory_owners
                   ORDER BY inventory_owner_id
               )
        "#,
    )
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        visible,
        (
            vec![local.tenant_id.get()],
            vec![local.facility_id],
            vec![local.inventory_owner_id]
        )
    );

    let membership_updates =
        sqlx::query("UPDATE tenant_memberships SET all_facilities = TRUE WHERE tenant_id = $1")
            .bind(foreign.tenant_id.get())
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
    let facility_updates =
        sqlx::query("UPDATE user_facilities SET deleted = CURRENT_TIMESTAMP WHERE tenant_id = $1")
            .bind(foreign.tenant_id.get())
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected();
    let inventory_owner_updates = sqlx::query(
        "UPDATE user_inventory_owners SET deleted = CURRENT_TIMESTAMP WHERE tenant_id = $1",
    )
    .bind(foreign.tenant_id.get())
    .execute(&mut *tx)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(
        (
            membership_updates,
            facility_updates,
            inventory_owner_updates
        ),
        (0, 0, 0)
    );
    tx.rollback().await.unwrap();
}

async fn assert_cross_tenant_inserts_fail(
    db: &db::Db,
    context: TenantId,
    foreign: ScopeRefs,
    foreign_user_id: i64,
) {
    let statements = [
        (
            "INSERT INTO tenant_memberships (tenant_id, user_id) VALUES ($1, $2)",
            foreign_user_id,
            0,
        ),
        (
            "INSERT INTO user_facilities (tenant_id, user_id, facility_id) VALUES ($1, $2, $3)",
            foreign.user_id,
            foreign.spare_facility_id,
        ),
        (
            "INSERT INTO user_inventory_owners \
             (tenant_id, user_id, inventory_owner_id) VALUES ($1, $2, $3)",
            foreign.user_id,
            foreign.spare_inventory_owner_id,
        ),
    ];

    for (statement, user_id, resource_id) in statements {
        let mut tx = tenant_tx(db, context).await;
        let mut query = sqlx::query(statement)
            .bind(foreign.tenant_id.get())
            .bind(user_id);
        if resource_id != 0 {
            query = query.bind(resource_id);
        }
        let error = query.execute(&mut *tx).await.unwrap_err();
        assert_sqlstate(error, "42501");
        tx.rollback().await.unwrap();
    }
}

async fn expire_session(db: &db::Db, token: &str) {
    let admin_db = admin_db_for(db).await;
    let updated = sqlx::query(
        "UPDATE sessions SET expires = CURRENT_TIMESTAMP - INTERVAL '1 day' WHERE token = $1",
    )
    .bind(token_hash(token))
    .execute(&admin_db)
    .await
    .unwrap()
    .rows_affected();
    assert_eq!(updated, 1);
    admin_db.close().await;
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
    .bind([SCOPE_TABLES.as_slice(), &["sessions"]].concat())
    .fetch_all(db)
    .await
    .unwrap();
    let mut expected = SCOPE_TABLES
        .iter()
        .map(|table| TablePrivileges {
            table_name: (*table).to_owned(),
            can_select: true,
            can_insert: true,
            can_update: true,
            can_delete: false,
            can_truncate: false,
            can_reference: false,
            can_trigger: false,
        })
        .collect::<Vec<_>>();
    expected.push(TablePrivileges {
        table_name: "sessions".to_owned(),
        can_select: false,
        can_insert: false,
        can_update: false,
        can_delete: false,
        can_truncate: false,
        can_reference: false,
        can_trigger: false,
    });
    assert_eq!(table_privileges, expected);

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
    .bind(SCOPE_SEQUENCES.as_slice())
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(
        sequence_privileges,
        SCOPE_SEQUENCES
            .iter()
            .map(|sequence| SequencePrivileges {
                sequence_name: (*sequence).to_owned(),
                can_use: true,
                can_select: false,
                can_update: false,
            })
            .collect::<Vec<_>>()
    );

    let function_security: Vec<FunctionSecurity> = sqlx::query_as(
        r#"
        SELECT signature,
               has_function_privilege(current_user, procedure.oid, 'EXECUTE')
                   AS app_can_execute,
               EXISTS (
                   SELECT 1
                   FROM aclexplode(
                       COALESCE(
                           procedure.proacl,
                           acldefault('f', procedure.proowner)
                       )
                   ) acl
                   WHERE acl.grantee = 0
                     AND acl.privilege_type = 'EXECUTE'
               ) AS public_can_execute,
               procedure.prosecdef AS security_definer,
               COALESCE(procedure.proconfig, ARRAY[]::TEXT[]) AS settings
        FROM unnest($1::TEXT[]) WITH ORDINALITY AS functions(signature, ordinal)
        JOIN pg_proc procedure ON procedure.oid = to_regprocedure(signature)
        ORDER BY ordinal
        "#,
    )
    .bind(SESSION_FUNCTIONS.as_slice())
    .fetch_all(db)
    .await
    .unwrap();
    assert_eq!(
        function_security,
        SESSION_FUNCTIONS
            .iter()
            .map(|signature| FunctionSecurity {
                signature: (*signature).to_owned(),
                app_can_execute: true,
                public_can_execute: false,
                security_definer: true,
                settings: vec!["search_path=pg_catalog, public".to_owned()],
            })
            .collect::<Vec<_>>()
    );
}

fn token_hash(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

fn assert_sqlstate(error: sqlx::Error, expected: &str) {
    let code = error.as_database_error().and_then(|error| error.code());
    assert_eq!(code.as_deref(), Some(expected));
}
