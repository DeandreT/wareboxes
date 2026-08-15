mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CreateValueAddedWorkInputRequest, CreateValueAddedWorkOutputRequest,
    CreateValueAddedWorkRequest, Revision, ValueAddedInventoryStatus, ValueAddedWorkKind,
    ValueAddedWorkLifecycleRequest, ValueAddedWorkResponse, ValueAddedWorkStatus,
};
use wareboxes_application::billing::{
    BillingContractLifecycleCommand, ConfigureBillingRateCommand, CreateBillingContractCommand,
};
use wareboxes_core::dto::UpdateUserAccessScope;
use wareboxes_core::models::TenantAccess;
use wareboxes_domain::{
    BillableEventType, BillingContractNumber, BillingEffectiveWindow, BillingRateDefinition,
    BillingUnit, CurrencyCode, InventoryOwnerId,
};

struct Rig {
    fixture: Fixture,
    access: TenantAccess,
    token: String,
    owner_id: i64,
    facility_id: i64,
    first: ReceivedBalance,
    second: ReceivedBalance,
    output_location_id: i64,
    output_batch_id: i64,
    app: axum::Router,
}

async fn grant_admin(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let permission = wareboxes_persistence_postgres::permissions::add_permission(
        &fixture.db,
        tenant_id,
        "admin",
        Some("Warehouse administrator"),
    )
    .await
    .unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        "vas-billing-admin",
        None,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_permission(
        &fixture.db,
        tenant_id,
        role,
        permission,
    )
    .await
    .unwrap();
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user_id, role)
        .await
        .unwrap();
}

impl Rig {
    async fn new(email: &str) -> Self {
        let fixture = Fixture::new().await;
        let user = fixture.wms_user(email).await;
        let access = default_tenant_for_user(&fixture.db, user.id).await.unwrap();
        grant_admin(&fixture, access.tenant_id, user.id).await;
        let owner_id = fixture
            .inventory_owner(access.tenant_id, "VAS Test Client")
            .await;
        let facility_id = fixture
            .facility(access.tenant_id, "VAS Test Facility")
            .await;
        fixture
            .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
            .await;
        let first_item = fixture.item(access.tenant_id, "Component A", "each").await;
        let second_item = fixture.item(access.tenant_id, "Component B", "each").await;
        let output_item = fixture.item(access.tenant_id, "Finished kit", "each").await;
        let first = fixture
            .received_balance(
                &access,
                ReceivedBalanceSetup {
                    inventory_owner_id: owner_id,
                    facility_id,
                    item_id: first_item,
                    qty: 10,
                    key: "VAS-COMP-A",
                },
            )
            .await;
        let second = fixture
            .received_balance(
                &access,
                ReceivedBalanceSetup {
                    inventory_owner_id: owner_id,
                    facility_id,
                    item_id: second_item,
                    qty: 8,
                    key: "VAS-COMP-B",
                },
            )
            .await;
        let output_location_id = fixture
            .location(access.tenant_id, facility_id, "VAS-FINISHED")
            .await;
        let output_batch_id = wareboxes_api::repo::inventory::add_item_batch(
            &fixture.db,
            access.tenant_id,
            owner_id,
            output_item,
            None,
            Some("VAS-FINISHED-LOT"),
            None,
            None,
        )
        .await
        .unwrap();
        let token = wareboxes_api::auth::create_session(&fixture.db, user.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            access,
            token,
            owner_id,
            facility_id,
            first,
            second,
            output_location_id,
            output_batch_id,
            app,
        }
    }

    fn create_body(&self, number: &str) -> CreateValueAddedWorkRequest {
        CreateValueAddedWorkRequest {
            inventory_owner_id: self.owner_id,
            facility_id: self.facility_id,
            number: number.into(),
            kind: ValueAddedWorkKind::Kit,
            note: Some("Build the standard promotional kit".into()),
            inputs: vec![
                CreateValueAddedWorkInputRequest {
                    inventory_balance_id: self.first.balance_id,
                    quantity: 2,
                },
                CreateValueAddedWorkInputRequest {
                    inventory_balance_id: self.second.balance_id,
                    quantity: 3,
                },
            ],
            outputs: vec![CreateValueAddedWorkOutputRequest {
                location_id: self.output_location_id,
                license_plate_id: None,
                item_batch_id: self.output_batch_id,
                inventory_status: ValueAddedInventoryStatus::Available,
                quantity: 1,
            }],
        }
    }

    async fn send<T: serde::Serialize>(
        &self,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<&T>,
    ) -> axum::response::Response {
        let mut builder = Request::builder()
            .method(method)
            .uri(format!("/api/v1/{path}"))
            .header(header::AUTHORIZATION, format!("Bearer {}", self.token))
            .header(TENANT_ID_HEADER, self.access.tenant_id.to_string());
        if let Some(key) = key {
            builder = builder.header(IDEMPOTENCY_KEY_HEADER, key);
        }
        let body = if let Some(body) = body {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(body).unwrap())
        } else {
            Body::empty()
        };
        self.app
            .clone()
            .oneshot(builder.body(body).unwrap())
            .await
            .unwrap()
    }
}

async fn json<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
    expected: StatusCode,
) -> T {
    let status = response.status();
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        expected,
        "unexpected response: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).unwrap()
}

#[tokio::test]
async fn released_kit_reserves_inputs_and_completion_is_journal_balanced_and_replay_safe() {
    let rig = Rig::new("vas-kit@test.local").await;
    let body = rig.create_body("VAS-KIT-100");
    let created: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            "value-added-work",
            Some("vas-kit-create"),
            Some(&body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(created.status, ValueAddedWorkStatus::Draft);
    assert_eq!(created.revision, Revision::new(1).unwrap());
    assert!(created.inputs.iter().all(|input| input.hold_id.is_none()));

    let replay: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            "value-added-work",
            Some("vas-kit-create"),
            Some(&body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, created);

    let released: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            &format!("value-added-work/{}/releases", created.work_id),
            Some("vas-kit-release"),
            Some(&ValueAddedWorkLifecycleRequest {
                expected_revision: created.revision,
                note: "Components staged for assembly".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(released.status, ValueAddedWorkStatus::Released);
    assert_eq!(released.revision, Revision::new(2).unwrap());
    assert!(released.inputs.iter().all(|input| input.hold_id.is_some()));

    let mut held_tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let held: Vec<i64> = sqlx::query_scalar(
        r#"SELECT qty_held FROM inventory_balances WHERE tenant_id=$1
           AND id=ANY($2) ORDER BY id"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(vec![rig.first.balance_id, rig.second.balance_id])
    .fetch_all(&mut *held_tx)
    .await
    .unwrap();
    assert_eq!(held, vec![2, 3]);
    held_tx.rollback().await.unwrap();

    let billing_context = wareboxes_application::CommandContext {
        tenant_id: rig.access.tenant_id,
        actor_id: rig.access.user_id,
        request_id: "vas-billing-contract".into(),
        idempotency_key: Some("vas-billing-contract".into()),
    };
    let contract = wareboxes_api::repo::billing::create_contract(
        &rig.fixture.db,
        &rig.access,
        &billing_context,
        &CreateBillingContractCommand {
            inventory_owner_id: InventoryOwnerId::new(rig.owner_id).unwrap(),
            contract_number: BillingContractNumber::new("VAS-CONTRACT-1".into()).unwrap(),
            currency: CurrencyCode::new("USD".into()).unwrap(),
            effective_window: BillingEffectiveWindow::new(
                db::now_iso() - std::time::Duration::from_secs(3_600),
                None,
            )
            .unwrap(),
        },
    )
    .await
    .unwrap();
    let rate_window =
        BillingEffectiveWindow::new(db::now_iso() - std::time::Duration::from_secs(3_600), None)
            .unwrap();
    wareboxes_api::repo::billing::configure_rate(
        &rig.fixture.db,
        &rig.access,
        &wareboxes_application::CommandContext {
            tenant_id: rig.access.tenant_id,
            actor_id: rig.access.user_id,
            request_id: "vas-billing-rate".into(),
            idempotency_key: Some("vas-billing-rate".into()),
        },
        &ConfigureBillingRateCommand {
            contract_id: contract.contract_id,
            definition: BillingRateDefinition::new(
                BillableEventType::KitUnit,
                BillingUnit::Each,
                CurrencyCode::new("USD".into()).unwrap(),
                125,
                0,
            )
            .unwrap(),
            effective_window: rate_window,
            expected_revision: None,
        },
    )
    .await
    .unwrap();
    wareboxes_api::repo::billing::activate_contract(
        &rig.fixture.db,
        &rig.access,
        &wareboxes_application::CommandContext {
            tenant_id: rig.access.tenant_id,
            actor_id: rig.access.user_id,
            request_id: "vas-billing-activate".into(),
            idempotency_key: Some("vas-billing-activate".into()),
        },
        &BillingContractLifecycleCommand {
            contract_id: contract.contract_id,
            expected_revision: contract.revision,
        },
    )
    .await
    .unwrap();

    let complete_body = ValueAddedWorkLifecycleRequest {
        expected_revision: released.revision,
        note: "Kit recipe scan and output count verified".into(),
    };
    let completed: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            &format!("value-added-work/{}/completions", created.work_id),
            Some("vas-kit-complete"),
            Some(&complete_body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(completed.status, ValueAddedWorkStatus::Completed);
    assert_eq!(completed.revision, Revision::new(3).unwrap());
    assert!(completed.completion_inventory_transaction_id.is_some());
    assert!(completed.billable_event_id.is_some());

    let complete_replay: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            &format!("value-added-work/{}/completions", created.work_id),
            Some("vas-kit-complete"),
            Some(&complete_body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(complete_replay, completed);

    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let input_state: Vec<(i64, i64, i64)> = sqlx::query_as(
        r#"SELECT id,qty_on_hand,qty_held FROM inventory_balances
           WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(vec![rig.first.balance_id, rig.second.balance_id])
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        input_state,
        vec![(rig.first.balance_id, 8, 0), (rig.second.balance_id, 5, 0)]
    );
    let output_quantity: i64 = sqlx::query_scalar(
        r#"SELECT qty_on_hand FROM inventory_balances WHERE tenant_id=$1
           AND inventory_owner_id=$2 AND location_id=$3 AND item_batch_id=$4
           AND license_plate_id IS NULL AND status='available' AND deleted IS NULL"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(rig.owner_id)
    .bind(rig.output_location_id)
    .bind(rig.output_batch_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(output_quantity, 1);
    let journal: Vec<i64> = sqlx::query_scalar(
        r#"SELECT quantity_delta FROM inventory_entries WHERE tenant_id=$1
           AND transaction_id=$2 ORDER BY id"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(completed.completion_inventory_transaction_id.unwrap())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(journal, vec![-2, -3, 1]);
    let billable: (String, String, i64, String, String) = sqlx::query_as(
        r#"SELECT event_type,unit,quantity,source_type,source_reference
           FROM billable_events WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(completed.billable_event_id.unwrap())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        billable,
        (
            "kit_unit".into(),
            "each".into(),
            1,
            "value_added_work_order".into(),
            created.work_id.to_string()
        )
    );
    let reconciliation_issues: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM inventory_reconciliation WHERE tenant_id=$1
           AND inventory_owner_id=$2 AND facility_id=$3"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(rig.owner_id)
    .bind(rig.facility_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(reconciliation_issues, 0);
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn competing_releases_cannot_overcommit_the_same_inventory() {
    let rig = Rig::new("vas-concurrency@test.local").await;
    let mut first_body = rig.create_body("VAS-COMPETE-1");
    first_body.kind = ValueAddedWorkKind::ValueAddedService;
    first_body.inputs.truncate(1);
    first_body.inputs[0].quantity = 8;
    let mut second_body = first_body.clone();
    second_body.number = "VAS-COMPETE-2".into();

    let first: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            "value-added-work",
            Some("vas-compete-create-1"),
            Some(&first_body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let second: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            "value-added-work",
            Some("vas-compete-create-2"),
            Some(&second_body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let first_path = format!("value-added-work/{}/releases", first.work_id);
    let second_path = format!("value-added-work/{}/releases", second.work_id);
    let first_release = ValueAddedWorkLifecycleRequest {
        expected_revision: first.revision,
        note: "Release first competing work".into(),
    };
    let second_release = ValueAddedWorkLifecycleRequest {
        expected_revision: second.revision,
        note: "Release second competing work".into(),
    };
    let first_request = rig.send(
        Method::POST,
        &first_path,
        Some("vas-compete-release-1"),
        Some(&first_release),
    );
    let second_request = rig.send(
        Method::POST,
        &second_path,
        Some("vas-compete-release-2"),
        Some(&second_release),
    );
    let (first_response, second_response) = tokio::join!(first_request, second_request);
    let statuses = [first_response.status(), second_response.status()];
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let (held, on_hand): (i64, i64) = sqlx::query_as(
        "SELECT qty_held,qty_on_hand FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.access.tenant_id.get())
    .bind(rig.first.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!((held, on_hand), (8, 10));
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn cancellation_releases_work_holds_without_inventory_effects_and_scope_loss_hides_replays() {
    let rig = Rig::new("vas-cancel-scope@test.local").await;
    let mut body = rig.create_body("VAS-CANCEL-1");
    body.kind = ValueAddedWorkKind::ValueAddedService;
    body.inputs.truncate(1);
    body.inputs[0].quantity = 4;
    let created: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            "value-added-work",
            Some("vas-cancel-create"),
            Some(&body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let released: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            &format!("value-added-work/{}/releases", created.work_id),
            Some("vas-cancel-release"),
            Some(&ValueAddedWorkLifecycleRequest {
                expected_revision: created.revision,
                note: "Reserve stock for cancellable work".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let cancelled: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            &format!("value-added-work/{}/cancellations", created.work_id),
            Some("vas-cancel-command"),
            Some(&ValueAddedWorkLifecycleRequest {
                expected_revision: released.revision,
                note: "Customer withdrew the service request".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(cancelled.status, ValueAddedWorkStatus::Cancelled);
    assert_eq!(cancelled.revision, Revision::new(3).unwrap());
    assert_eq!(cancelled.events.len(), 3);
    assert!(cancelled.completion_inventory_transaction_id.is_none());

    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let balance: (i64, i64) = sqlx::query_as(
        "SELECT qty_on_hand,qty_held FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.access.tenant_id.get())
    .bind(rig.first.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(balance, (10, 0));
    let released_holds: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM inventory_holds WHERE tenant_id=$1
           AND reference_type='value_added_work_order' AND reference_id=$2
           AND status='released' AND deleted IS NOT NULL"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(created.work_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(released_holds, 1);
    let inventory_transactions: i64 = sqlx::query_scalar(
        r#"SELECT count(*) FROM inventory_transactions WHERE tenant_id=$1
           AND reference_type='value_added_work_order' AND reference_id=$2"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(created.work_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(inventory_transactions, 0);
    tx.rollback().await.unwrap();

    assert!(wareboxes_api::repo::tenants::update_user_access_scope(
        &rig.fixture.db,
        rig.access.tenant_id,
        &UpdateUserAccessScope {
            user_id: rig.access.user_id.get(),
            all_facilities: true,
            facility_ids: vec![],
            all_inventory_owners: false,
            inventory_owner_ids: vec![],
        },
    )
    .await
    .unwrap());
    assert_eq!(
        rig.send::<serde_json::Value>(
            Method::GET,
            &format!("value-added-work/{}", created.work_id),
            None,
            None,
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        rig.send(
            Method::POST,
            "value-added-work",
            Some("vas-cancel-create"),
            Some(&body),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn database_rejects_inventory_hold_attachment_without_a_work_transition() {
    let rig = Rig::new("vas-integrity@test.local").await;
    let hold_id = wareboxes_api::repo::inventory::place_inventory_hold(
        &rig.fixture.db,
        &rig.access,
        &wareboxes_application::CommandContext {
            tenant_id: rig.access.tenant_id,
            actor_id: rig.access.user_id,
            request_id: "vas-integrity-hold".into(),
            idempotency_key: Some("vas-integrity-hold".into()),
        },
        &wareboxes_api::repo::inventory::PlaceInventoryHoldCommand {
            inventory_balance_id: rig.second.balance_id,
            qty: 1,
            reason: wareboxes_core::models::InventoryHoldReason::QualityInspection,
            note: Some("Independent hold used to exercise VAS database integrity"),
            reference_type: Some("quality_inspection"),
            reference_id: Some(1),
        },
    )
    .await
    .unwrap()
    .hold_id;

    let mut draft_body = rig.create_body("VAS-INTEGRITY-DRAFT");
    draft_body.kind = ValueAddedWorkKind::ValueAddedService;
    draft_body.inputs.truncate(1);
    let draft: ValueAddedWorkResponse = json(
        rig.send(
            Method::POST,
            "value-added-work",
            Some("vas-integrity-create-draft"),
            Some(&draft_body),
        )
        .await,
        StatusCode::OK,
    )
    .await;

    let mut tamper = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    sqlx::query(
        "UPDATE value_added_work_inputs SET inventory_hold_id=$1 WHERE tenant_id=$2 AND work_id=$3",
    )
    .bind(hold_id)
    .bind(rig.access.tenant_id.get())
    .bind(draft.work_id)
    .execute(&mut *tamper)
    .await
    .unwrap();
    let error = tamper.commit().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("draft value-added work cannot reserve inventory"),
        "unexpected deferred integrity error: {error}"
    );
}
