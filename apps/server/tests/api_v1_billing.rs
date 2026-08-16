mod common;
#[path = "api_v1_billing/decision_policy.rs"]
mod decision_policy;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{repo, routes, state::AppState};
use wareboxes_api_contract::v1::{
    BillableEventResponse, BillableEventType, BillingContractResponse, BillingContractStatus,
    BillingDecisionPolicySource, BillingFinancialExportResponse, BillingLifecycleRequest,
    BillingPageRequest, BillingRateResponse, BillingReviewDecision, BillingRunResponse,
    BillingRunStatus, BillingStorageSnapshotResponse, BillingUnit, BillingWorkspaceResponse,
    CaptureBillableEventRequest, CaptureBillingStorageSnapshotRequest, ConfigureBillingRateRequest,
    CreateBillingContractRequest, ExportBillingRunRequest, GenerateBillingRunRequest, PageLimit,
    ReviewBillingRunRequest, Revision,
};
use wareboxes_core::dto::UpdateUserAccessScope;

async fn add_membership(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

async fn grant_admin(fixture: &Fixture, tenant_id: TenantId, user_id: i64, suffix: &str) {
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
        &format!("billing-admin-{suffix}"),
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

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    uri: &str,
    idempotency_key: Option<&str>,
    body: Option<&T>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = idempotency_key {
        builder = builder.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(body).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn response_json<T: serde::de::DeserializeOwned>(
    response: axum::response::Response,
    expected: StatusCode,
) -> T {
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(
        status,
        expected,
        "unexpected response: {}",
        String::from_utf8_lossy(&bytes)
    );
    serde_json::from_slice(&bytes).unwrap()
}

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    creator_id: i64,
    creator_token: String,
    approver_id: i64,
    approver_token: String,
    owner_id: i64,
    facility_id: i64,
    app: axum::Router,
}

impl Rig {
    async fn new() -> Self {
        let fixture = Fixture::new().await;
        let creator = fixture.user("billing-creator@test.local").await;
        let tenant_id = tenant_for_user(&fixture.db, creator.id).await;
        grant_admin(&fixture, tenant_id, creator.id, "creator").await;
        let approver = fixture.user("billing-approver@test.local").await;
        add_membership(&fixture, tenant_id, approver.id).await;
        grant_admin(&fixture, tenant_id, approver.id, "approver").await;
        let owner_id = fixture.inventory_owner(tenant_id, "Billing Client").await;
        let facility_id = fixture.facility(tenant_id, "Billing DC").await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;
        let creator_token = wareboxes_api::auth::create_session(&fixture.db, creator.id)
            .await
            .unwrap();
        let approver_token = wareboxes_api::auth::create_session(&fixture.db, approver.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            creator_id: creator.id,
            creator_token,
            approver_id: approver.id,
            approver_token,
            owner_id,
            facility_id,
            app,
        }
    }

    async fn send<T: Serialize>(
        &self,
        token: &str,
        method: Method,
        uri: &str,
        key: Option<&str>,
        body: Option<&T>,
    ) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(request(token, self.tenant_id, method, uri, key, body))
            .await
            .unwrap()
    }

    async fn create_contract(&self, key: &str, number: &str) -> BillingContractResponse {
        response_json(
            self.send(
                &self.creator_token,
                Method::POST,
                "/api/v1/billing/contracts",
                Some(key),
                Some(&CreateBillingContractRequest {
                    inventory_owner_id: self.owner_id,
                    contract_number: number.into(),
                    currency: "usd".into(),
                    effective_from: "2026-01-01T00:00:00Z".into(),
                    effective_until: None,
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }

    async fn rate(
        &self,
        contract_id: i64,
        key: &str,
        event_type: BillableEventType,
        unit: BillingUnit,
        rate_minor: u64,
    ) -> BillingRateResponse {
        response_json(
            self.send(
                &self.creator_token,
                Method::POST,
                &format!("/api/v1/billing/contracts/{contract_id}/rates"),
                Some(key),
                Some(&ConfigureBillingRateRequest {
                    event_type,
                    unit,
                    currency: "USD".into(),
                    rate_minor,
                    minimum_charge_minor: 1_500,
                    effective_from: "2026-01-01T00:00:00Z".into(),
                    effective_until: None,
                    expected_revision: None,
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }

    async fn activate(&self, contract: &BillingContractResponse) -> BillingContractResponse {
        response_json(
            self.send(
                &self.creator_token,
                Method::POST,
                &format!(
                    "/api/v1/billing/contracts/{}/activations",
                    contract.contract_id
                ),
                Some("billing-contract-activate"),
                Some(&BillingLifecycleRequest {
                    expected_revision: contract.revision,
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }

    async fn event(
        &self,
        contract_id: i64,
        key: &str,
        event_type: BillableEventType,
        unit: BillingUnit,
        quantity: i64,
        reference: &str,
    ) -> axum::response::Response {
        self.send(
            &self.creator_token,
            Method::POST,
            &format!("/api/v1/billing/contracts/{contract_id}/billable-events"),
            Some(key),
            Some(&CaptureBillableEventRequest {
                facility_id: self.facility_id,
                event_type,
                unit,
                quantity,
                source_reference: reference.into(),
                description: "Authorized client service".into(),
                occurred_at: "2026-08-02T12:00:00Z".into(),
            }),
        )
        .await
    }

    fn period() -> GenerateBillingRunRequest {
        GenerateBillingRunRequest {
            facility_id: Some(1),
            period_from: "2026-08-01T00:00:00Z".into(),
            period_until: "2026-08-10T00:00:00Z".into(),
        }
    }

    async fn generate(&self, contract_id: i64, key: &str) -> axum::response::Response {
        let mut period = Self::period();
        period.facility_id = Some(self.facility_id);
        self.send(
            &self.creator_token,
            Method::POST,
            &format!("/api/v1/billing/contracts/{contract_id}/reconciliation-runs"),
            Some(key),
            Some(&period),
        )
        .await
    }
}

#[tokio::test]
async fn billing_lifecycle_storage_reconciliation_review_export_and_replay_are_complete() {
    let rig = Rig::new().await;
    let contract = rig.create_contract("contract-create", "CLIENT-2026").await;
    assert_eq!(contract.status, BillingContractStatus::Draft);
    assert_eq!(contract.currency, "USD");
    let rate = rig
        .rate(
            contract.contract_id,
            "accessorial-rate",
            BillableEventType::Accessorial,
            BillingUnit::Event,
            1_000,
        )
        .await;
    assert_eq!(rate.revision.get(), 1);
    let active = rig.activate(&contract).await;
    assert_eq!(active.status, BillingContractStatus::Active);
    assert_eq!(active.revision.get(), 2);

    let snapshot: BillingStorageSnapshotResponse = response_json(
        rig.send(
            &rig.creator_token,
            Method::POST,
            &format!(
                "/api/v1/billing/contracts/{}/storage-snapshots",
                contract.contract_id
            ),
            Some("storage-snapshot"),
            Some(&CaptureBillingStorageSnapshotRequest {
                facility_id: rig.facility_id,
                snapshot_date: "2026-08-03".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(snapshot.pallet_count, 0);
    assert_eq!(snapshot.unit_count, 0);

    let event: BillableEventResponse = response_json(
        rig.event(
            contract.contract_id,
            "accessorial-event",
            BillableEventType::Accessorial,
            BillingUnit::Event,
            2,
            "SPECIAL-HANDLING-1",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let replay: BillableEventResponse = response_json(
        rig.event(
            contract.contract_id,
            "accessorial-event",
            BillableEventType::Accessorial,
            BillingUnit::Event,
            2,
            "SPECIAL-HANDLING-1",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(event, replay);

    let run: BillingRunResponse = response_json(
        rig.generate(contract.contract_id, "run-generate").await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(run.attempt, 1);
    assert_eq!(run.event_count, 1);
    assert_eq!(run.charge_count, 1);
    assert_eq!(run.unmatched_event_count, 0);
    assert_eq!(run.total_minor, 2_000);
    assert_eq!(run.charges[0].gross_minor, 2_000);
    assert_eq!(
        run.charges[0].decision_policy.source,
        BillingDecisionPolicySource::ContractRate
    );
    assert_eq!(run.charges[0].rate_id, Some(rate.rate_id));

    let approve = ReviewBillingRunRequest {
        expected_revision: run.revision,
        decision: BillingReviewDecision::Approve,
        note: Some("Reconciled to service log".into()),
    };
    assert_eq!(
        rig.send(
            &rig.creator_token,
            Method::POST,
            &format!("/api/v1/billing/reconciliation-runs/{}/reviews", run.run_id),
            Some("self-review"),
            Some(&approve),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let approved: BillingRunResponse = response_json(
        rig.send(
            &rig.approver_token,
            Method::POST,
            &format!("/api/v1/billing/reconciliation-runs/{}/reviews", run.run_id),
            Some("approve-run"),
            Some(&approve),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(approved.status, BillingRunStatus::Approved);
    assert_eq!(approved.reviewed_by, Some(rig.approver_id));

    let export: BillingFinancialExportResponse = response_json(
        rig.send(
            &rig.creator_token,
            Method::POST,
            &format!("/api/v1/billing/reconciliation-runs/{}/exports", run.run_id),
            Some("export-run"),
            Some(&ExportBillingRunRequest {
                expected_revision: approved.revision,
                external_batch_key: "ERP-2026-08-CLIENT".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(export.line_count, 1);
    assert_eq!(export.total_minor, 2_000);
    assert_eq!(
        export.content_sha256,
        hex::encode(Sha256::digest(export.csv_content.as_bytes()))
    );
    assert!(export.csv_content.contains("SPECIAL-HANDLING-1"));
    assert!(export.csv_content.contains("decision_policy_source"));
    assert!(export
        .csv_content
        .contains(&run.charges[0].decision_policy.policy_hash));

    let workspace: BillingWorkspaceResponse = response_json(
        rig.send::<BillingPageRequest>(
            &rig.creator_token,
            Method::GET,
            &format!(
                "/api/v1/billing/workspace?inventory_owner_id={}&limit=10",
                rig.owner_id
            ),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(workspace.contracts.len(), 1);
    assert_eq!(workspace.rates.len(), 1);
    assert_eq!(workspace.events.len(), 1);
    assert_eq!(workspace.runs[0].status, BillingRunStatus::Exported);

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let event_count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM outbox_events WHERE tenant_id=$1 AND aggregate_type LIKE '%billing%' OR (tenant_id=$1 AND event_type LIKE 'billing.%')",
    )
    .bind(rig.tenant_id.get())
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(event_count >= 7);
}

#[tokio::test]
async fn unmatched_rejection_correction_permissions_scope_and_database_guards_fail_closed() {
    let rig = Rig::new().await;
    let contract = rig
        .create_contract("correction-contract", "CORRECT-2026")
        .await;
    rig.rate(
        contract.contract_id,
        "accessorial-only",
        BillableEventType::Accessorial,
        BillingUnit::Event,
        500,
    )
    .await;
    let active = rig.activate(&contract).await;
    let _: BillableEventResponse = response_json(
        rig.event(
            contract.contract_id,
            "detention-event",
            BillableEventType::DetentionHour,
            BillingUnit::Hour,
            3,
            "TRAILER-DETENTION-7",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let first: BillingRunResponse = response_json(
        rig.generate(contract.contract_id, "correction-run-1").await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(first.unmatched_event_count, 1);
    let approve = ReviewBillingRunRequest {
        expected_revision: first.revision,
        decision: BillingReviewDecision::Approve,
        note: None,
    };
    assert_eq!(
        rig.send(
            &rig.approver_token,
            Method::POST,
            &format!(
                "/api/v1/billing/reconciliation-runs/{}/reviews",
                first.run_id
            ),
            Some("invalid-unmatched-approval"),
            Some(&approve),
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );
    let rejected: BillingRunResponse = response_json(
        rig.send(
            &rig.approver_token,
            Method::POST,
            &format!(
                "/api/v1/billing/reconciliation-runs/{}/reviews",
                first.run_id
            ),
            Some("reject-unmatched"),
            Some(&ReviewBillingRunRequest {
                expected_revision: first.revision,
                decision: BillingReviewDecision::Reject,
                note: Some("Missing detention rate".into()),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(rejected.status, BillingRunStatus::Rejected);
    rig.rate(
        contract.contract_id,
        "detention-rate",
        BillableEventType::DetentionHour,
        BillingUnit::Hour,
        700,
    )
    .await;
    let corrected: BillingRunResponse = response_json(
        rig.generate(contract.contract_id, "correction-run-2").await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(corrected.attempt, 2);
    assert_eq!(corrected.supersedes_run_id, Some(first.run_id));
    assert_eq!(corrected.unmatched_event_count, 0);
    assert_eq!(corrected.total_minor, 2_100);

    let viewer = rig.fixture.user("billing-viewer@test.local").await;
    add_membership(&rig.fixture, rig.tenant_id, viewer.id).await;
    let viewer_token = wareboxes_api::auth::create_session(&rig.fixture.db, viewer.id)
        .await
        .unwrap();
    assert_eq!(
        rig.send::<serde_json::Value>(
            &viewer_token,
            Method::GET,
            "/api/v1/billing/workspace",
            None,
            None,
        )
        .await
        .status(),
        StatusCode::FORBIDDEN
    );
    assert!(repo::tenants::update_user_access_scope(
        &rig.fixture.db,
        rig.tenant_id,
        &UpdateUserAccessScope {
            user_id: rig.creator_id,
            all_inventory_owners: false,
            all_facilities: true,
            inventory_owner_ids: vec![],
            facility_ids: vec![],
        },
    )
    .await
    .unwrap());
    let refreshed = wareboxes_api::auth::create_session(&rig.fixture.db, rig.creator_id)
        .await
        .unwrap();
    assert_eq!(
        rig.send(
            &refreshed,
            Method::POST,
            &format!(
                "/api/v1/billing/contracts/{}/activations",
                active.contract_id
            ),
            Some("billing-contract-activate"),
            Some(&BillingLifecycleRequest {
                expected_revision: Revision::new(1).unwrap(),
            }),
        )
        .await
        .status(),
        StatusCode::NOT_FOUND
    );

    let mut immutable = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let event_mutation = sqlx::query(
        "UPDATE billable_events SET quantity=quantity+1 WHERE tenant_id=$1 AND contract_id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(contract.contract_id)
    .execute(&mut *immutable)
    .await;
    assert!(event_mutation.is_err());
    immutable.rollback().await.unwrap();

    let grants: (bool, bool, bool) = sqlx::query_as(
        r#"SELECT has_table_privilege('wareboxes_app','billable_events','SELECT'),
                  has_table_privilege('wareboxes_app','billable_events','INSERT'),
                  has_table_privilege('wareboxes_app','billable_events','DELETE')"#,
    )
    .fetch_one(&rig.fixture.db)
    .await
    .unwrap();
    assert_eq!(grants, (true, true, false));

    let page_request = BillingPageRequest {
        inventory_owner_id: Some(rig.owner_id),
        contract_id: Some(contract.contract_id),
        cursor: None,
        limit: PageLimit::new(10).unwrap(),
    };
    assert_eq!(page_request.limit.get(), 10);
}

#[tokio::test]
async fn operational_receipts_are_derived_once_and_concurrent_generation_has_one_winner() {
    let rig = Rig::new().await;
    let contract = rig.create_contract("operations-contract", "OPS-2026").await;
    rig.rate(
        contract.contract_id,
        "received-unit-rate",
        BillableEventType::ReceivedUnit,
        BillingUnit::Each,
        100,
    )
    .await;
    rig.rate(
        contract.contract_id,
        "receipt-line-rate",
        BillableEventType::ReceiptLine,
        BillingUnit::Event,
        500,
    )
    .await;
    rig.activate(&contract).await;

    let access = repo::tenants::access_for_user(&rig.fixture.db, rig.creator_id, rig.tenant_id)
        .await
        .unwrap()
        .unwrap();
    let item_id = rig
        .fixture
        .item(rig.tenant_id, "Billable Receipt Item", "each")
        .await;
    let period_from = (db::now_iso() - std::time::Duration::from_secs(3_600)).to_rfc3339();
    rig.fixture
        .received_balance(
            &access,
            ReceivedBalanceSetup {
                inventory_owner_id: rig.owner_id,
                facility_id: rig.facility_id,
                item_id,
                qty: 7,
                key: "BILLING-RECEIPT",
            },
        )
        .await;
    let period_until = db::now_iso().to_rfc3339();
    let request = GenerateBillingRunRequest {
        facility_id: Some(rig.facility_id),
        period_from,
        period_until,
    };
    let uri = format!(
        "/api/v1/billing/contracts/{}/reconciliation-runs",
        contract.contract_id
    );
    let (left, right) = tokio::join!(
        rig.send(
            &rig.creator_token,
            Method::POST,
            &uri,
            Some("operations-run-left"),
            Some(&request),
        ),
        rig.send(
            &rig.creator_token,
            Method::POST,
            &uri,
            Some("operations-run-right"),
            Some(&request),
        )
    );
    assert_eq!(
        [left.status(), right.status()]
            .into_iter()
            .filter(|status| *status == StatusCode::OK)
            .count(),
        1
    );
    assert_eq!(
        [left.status(), right.status()]
            .into_iter()
            .filter(|status| *status == StatusCode::CONFLICT)
            .count(),
        1
    );
    let winner = if left.status() == StatusCode::OK {
        left
    } else {
        right
    };
    let run: BillingRunResponse = response_json(winner, StatusCode::OK).await;
    assert_eq!(run.event_count, 2);
    assert_eq!(run.charge_count, 2);
    assert_eq!(run.unmatched_event_count, 0);
    assert_eq!(run.total_minor, 3_000);
    assert!(run.charges.iter().any(
        |charge| charge.event_type == BillableEventType::ReceivedUnit && charge.quantity == 7
    ));
    assert!(run
        .charges
        .iter()
        .any(|charge| charge.event_type == BillableEventType::ReceiptLine && charge.quantity == 1));

    let mut tx = tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let derived: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM billable_events WHERE tenant_id=$1 AND contract_id=$2 AND source_type='inventory_transaction'",
    )
    .bind(rig.tenant_id.get())
    .bind(contract.contract_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(derived, 2);
}
