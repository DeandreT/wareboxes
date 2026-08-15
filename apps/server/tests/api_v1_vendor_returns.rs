mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    CreateVendorReturnLineRequest, CreateVendorReturnRequest, VendorReturnLifecycleRequest,
    VendorReturnReason, VendorReturnResponse, VendorReturnStatus,
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
        "vendor-return-billing-admin",
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
            .inventory_owner(access.tenant_id, "Vendor Return Client")
            .await;
        let facility_id = fixture
            .facility(access.tenant_id, "Vendor Return Facility")
            .await;
        fixture
            .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
            .await;
        let first_item = fixture
            .item(access.tenant_id, "Defective component", "each")
            .await;
        let second_item = fixture
            .item(access.tenant_id, "Recalled component", "each")
            .await;
        let first = fixture
            .received_balance(
                &access,
                ReceivedBalanceSetup {
                    inventory_owner_id: owner_id,
                    facility_id,
                    item_id: first_item,
                    qty: 10,
                    key: "VENDOR-RETURN-A",
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
                    key: "VENDOR-RETURN-B",
                },
            )
            .await;
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
            app,
        }
    }

    fn body(&self, number: &str) -> CreateVendorReturnRequest {
        CreateVendorReturnRequest {
            inventory_owner_id: self.owner_id,
            facility_id: self.facility_id,
            number: number.into(),
            vendor_name: "Acme Components".into(),
            vendor_reference: Some("RGA-2048".into()),
            note: Some("Return approved by vendor quality".into()),
            lines: vec![
                CreateVendorReturnLineRequest {
                    inventory_balance_id: self.first.balance_id,
                    quantity: 2,
                    reason: VendorReturnReason::Defective,
                    note: Some("Failed bench test".into()),
                },
                CreateVendorReturnLineRequest {
                    inventory_balance_id: self.second.balance_id,
                    quantity: 3,
                    reason: VendorReturnReason::Recall,
                    note: Some("Supplier recall 2026-17".into()),
                },
            ],
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

async fn configure_return_billing(rig: &Rig) {
    let context = wareboxes_application::CommandContext {
        tenant_id: rig.access.tenant_id,
        actor_id: rig.access.user_id,
        request_id: "vendor-return-billing".into(),
        idempotency_key: Some("vendor-return-billing".into()),
    };
    let contract = wareboxes_api::repo::billing::create_contract(
        &rig.fixture.db,
        &rig.access,
        &context,
        &CreateBillingContractCommand {
            inventory_owner_id: InventoryOwnerId::new(rig.owner_id).unwrap(),
            contract_number: BillingContractNumber::new("VENDOR-RETURN-CONTRACT".into()).unwrap(),
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
    wareboxes_api::repo::billing::configure_rate(
        &rig.fixture.db,
        &rig.access,
        &wareboxes_application::CommandContext {
            idempotency_key: Some("vendor-return-rate".into()),
            request_id: "vendor-return-rate".into(),
            ..context.clone()
        },
        &ConfigureBillingRateCommand {
            contract_id: contract.contract_id,
            definition: BillingRateDefinition::new(
                BillableEventType::ReturnUnit,
                BillingUnit::Each,
                CurrencyCode::new("USD".into()).unwrap(),
                75,
                0,
            )
            .unwrap(),
            effective_window: BillingEffectiveWindow::new(
                db::now_iso() - std::time::Duration::from_secs(3_600),
                None,
            )
            .unwrap(),
            expected_revision: None,
        },
    )
    .await
    .unwrap();
    wareboxes_api::repo::billing::activate_contract(
        &rig.fixture.db,
        &rig.access,
        &wareboxes_application::CommandContext {
            idempotency_key: Some("vendor-return-activate".into()),
            request_id: "vendor-return-activate".into(),
            ..context
        },
        &BillingContractLifecycleCommand {
            contract_id: contract.contract_id,
            expected_revision: contract.revision,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn released_vendor_return_reserves_and_shipment_posts_billed_outbound_journal() {
    let rig = Rig::new("vendor-return-ship@test.local").await;
    configure_return_billing(&rig).await;
    let body = rig.body("VENDOR-RETURN-100");
    let created: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            "vendor-returns",
            Some("vendor-return-create"),
            Some(&body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(created.status, VendorReturnStatus::Draft);
    let replay: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            "vendor-returns",
            Some("vendor-return-create"),
            Some(&body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, created);
    let released: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            &format!("vendor-returns/{}/releases", created.vendor_return_id),
            Some("vendor-return-release"),
            Some(&VendorReturnLifecycleRequest {
                expected_revision: created.revision,
                note: "Stock staged against the vendor RGA".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(released.status, VendorReturnStatus::Released);
    assert!(released.lines.iter().all(|line| line.hold_id.is_some()));
    let shipment = VendorReturnLifecycleRequest {
        expected_revision: released.revision,
        note: "Carrier receipt and trailer departure verified".into(),
    };
    let shipped: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            &format!("vendor-returns/{}/shipments", created.vendor_return_id),
            Some("vendor-return-ship"),
            Some(&shipment),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(shipped.status, VendorReturnStatus::Shipped);
    assert!(shipped.shipment_inventory_transaction_id.is_some());
    assert!(shipped.billable_event_id.is_some());
    let replay: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            &format!("vendor-returns/{}/shipments", created.vendor_return_id),
            Some("vendor-return-ship"),
            Some(&shipment),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, shipped);

    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let balances: Vec<(i64, i64, i64)> = sqlx::query_as(
        r#"SELECT id,qty_on_hand,qty_held FROM inventory_balances
           WHERE tenant_id=$1 AND id=ANY($2) ORDER BY id"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(vec![rig.first.balance_id, rig.second.balance_id])
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        balances,
        vec![(rig.first.balance_id, 8, 0), (rig.second.balance_id, 5, 0)]
    );
    let entries: Vec<i64> = sqlx::query_scalar(
        r#"SELECT quantity_delta FROM inventory_entries WHERE tenant_id=$1
           AND transaction_id=$2 ORDER BY id"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(shipped.shipment_inventory_transaction_id.unwrap())
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(entries, vec![-2, -3]);
    let billable: (String, i64, String) = sqlx::query_as(
        r#"SELECT event_type,quantity,source_type FROM billable_events
           WHERE tenant_id=$1 AND id=$2"#,
    )
    .bind(rig.access.tenant_id.get())
    .bind(shipped.billable_event_id.unwrap())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(billable, ("return_unit".into(), 5, "vendor_return".into()));
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
async fn competing_releases_cannot_overcommit_vendor_return_stock() {
    let rig = Rig::new("vendor-return-concurrency@test.local").await;
    let mut first_body = rig.body("VENDOR-RETURN-COMPETE-1");
    first_body.lines.truncate(1);
    first_body.lines[0].quantity = 8;
    let mut second_body = first_body.clone();
    second_body.number = "VENDOR-RETURN-COMPETE-2".into();
    let first: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            "vendor-returns",
            Some("vendor-compete-create-1"),
            Some(&first_body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let second: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            "vendor-returns",
            Some("vendor-compete-create-2"),
            Some(&second_body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let request = |revision| VendorReturnLifecycleRequest {
        expected_revision: revision,
        note: "Reserve competing vendor-return stock".into(),
    };
    let first_path = format!("vendor-returns/{}/releases", first.vendor_return_id);
    let second_path = format!("vendor-returns/{}/releases", second.vendor_return_id);
    let first_request = request(first.revision);
    let second_request = request(second.revision);
    let first_release = rig.send(
        Method::POST,
        &first_path,
        Some("vendor-compete-release-1"),
        Some(&first_request),
    );
    let second_release = rig.send(
        Method::POST,
        &second_path,
        Some("vendor-compete-release-2"),
        Some(&second_request),
    );
    let (first_response, second_response) = tokio::join!(first_release, second_release);
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
}

#[tokio::test]
async fn cancellation_releases_vendor_return_holds_and_scope_loss_hides_replay() {
    let rig = Rig::new("vendor-return-cancel@test.local").await;
    let mut body = rig.body("VENDOR-RETURN-CANCEL");
    body.lines.truncate(1);
    let created: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            "vendor-returns",
            Some("vendor-cancel-create"),
            Some(&body),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let released: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            &format!("vendor-returns/{}/releases", created.vendor_return_id),
            Some("vendor-cancel-release"),
            Some(&VendorReturnLifecycleRequest {
                expected_revision: created.revision,
                note: "Stage return stock".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let cancelled: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            &format!("vendor-returns/{}/cancellations", created.vendor_return_id),
            Some("vendor-cancel-command"),
            Some(&VendorReturnLifecycleRequest {
                expected_revision: released.revision,
                note: "Vendor withdrew the RGA".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(cancelled.status, VendorReturnStatus::Cancelled);
    assert!(cancelled.shipment_inventory_transaction_id.is_none());
    let mut tx = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    let state: (i64, i64) = sqlx::query_as(
        "SELECT qty_on_hand,qty_held FROM inventory_balances WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.access.tenant_id.get())
    .bind(rig.first.balance_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(state, (10, 0));
    tx.rollback().await.unwrap();

    wareboxes_api::repo::tenants::update_user_access_scope(
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
    .unwrap();
    assert_eq!(
        rig.send::<serde_json::Value>(
            Method::GET,
            &format!("vendor-returns/{}", created.vendor_return_id),
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
            "vendor-returns",
            Some("vendor-cancel-create"),
            Some(&body),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );
}

#[tokio::test]
async fn database_rejects_inventory_hold_attachment_without_a_vendor_return_transition() {
    let rig = Rig::new("vendor-return-integrity@test.local").await;
    let hold_id = wareboxes_api::repo::inventory::place_inventory_hold(
        &rig.fixture.db,
        &rig.access,
        &wareboxes_application::CommandContext {
            tenant_id: rig.access.tenant_id,
            actor_id: rig.access.user_id,
            request_id: "vendor-return-integrity-hold".into(),
            idempotency_key: Some("vendor-return-integrity-hold".into()),
        },
        &wareboxes_api::repo::inventory::PlaceInventoryHoldCommand {
            inventory_balance_id: rig.second.balance_id,
            qty: 1,
            reason: wareboxes_core::models::InventoryHoldReason::QualityInspection,
            note: Some("Independent hold used to exercise vendor-return database integrity"),
            reference_type: Some("quality_inspection"),
            reference_id: Some(1),
        },
    )
    .await
    .unwrap()
    .hold_id;

    let mut draft_body = rig.body("VENDOR-RETURN-INTEGRITY-DRAFT");
    draft_body.lines.truncate(1);
    let draft: VendorReturnResponse = json(
        rig.send(
            Method::POST,
            "vendor-returns",
            Some("vendor-return-integrity-create-draft"),
            Some(&draft_body),
        )
        .await,
        StatusCode::OK,
    )
    .await;

    let mut tamper = tenant_tx(&rig.fixture.db, rig.access.tenant_id).await;
    sqlx::query(
        "UPDATE vendor_return_lines SET inventory_hold_id=$1 WHERE tenant_id=$2 AND vendor_return_id=$3",
    )
    .bind(hold_id)
    .bind(rig.access.tenant_id.get())
    .bind(draft.vendor_return_id)
    .execute(&mut *tamper)
    .await
    .unwrap();
    let error = tamper.commit().await.unwrap_err();
    assert!(
        error
            .to_string()
            .contains("draft vendor return cannot reserve inventory"),
        "unexpected deferred integrity error: {error}"
    );
}
