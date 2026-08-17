use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use wareboxes_api_contract::v1::{
    CarrierAccountPage, CarrierAccountResponse, CarrierManifestJobPage, CarrierManifestJobResponse,
    CarrierManifestJobStatus,
};
use wareboxes_application::carrier::{
    CarrierManifestAdapterRequest, CarrierManifestAdapterResponse, CarrierPackageManifestResult,
};
use wareboxes_domain::{ManifestReference, TrackingNumber};
use wareboxes_persistence_postgres::carrier_manifest::PostgresCarrierManifestStore;
use wareboxes_worker::{
    CarrierGateway, CarrierGatewayError, CarrierManifestStore, CarrierManifestWorker,
    CarrierManifestWorkerConfig,
};

use super::*;

#[derive(Clone, Copy)]
enum GatewayStep {
    Retryable,
    Permanent,
    Success,
}

struct ScriptedGateway {
    steps: Mutex<VecDeque<GatewayStep>>,
    requests: Mutex<Vec<CarrierManifestAdapterRequest>>,
}

impl ScriptedGateway {
    fn new(steps: impl IntoIterator<Item = GatewayStep>) -> Self {
        Self {
            steps: Mutex::new(steps.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    fn requests(&self) -> Vec<CarrierManifestAdapterRequest> {
        self.requests.lock().unwrap().clone()
    }
}

#[async_trait]
impl CarrierGateway for ScriptedGateway {
    fn name(&self) -> &'static str {
        "acceptance-carrier"
    }

    async fn manifest(
        &self,
        request: &CarrierManifestAdapterRequest,
    ) -> Result<CarrierManifestAdapterResponse, CarrierGatewayError> {
        self.requests.lock().unwrap().push(request.clone());
        match self.steps.lock().unwrap().pop_front().unwrap() {
            GatewayStep::Retryable => Err(CarrierGatewayError::retryable(
                "carrier_busy",
                "carrier asked us to retry",
            )
            .with_retry_after(Duration::ZERO)),
            GatewayStep::Permanent => Err(CarrierGatewayError::permanent(
                "address_rejected",
                "carrier rejected the destination address",
            )),
            GatewayStep::Success => Ok(CarrierManifestAdapterResponse {
                schema_version: request.schema_version,
                request_key: request.request_key.clone(),
                manifest_reference: ManifestReference::new(format!(
                    "GATEWAY-MANIFEST-{}",
                    request.shipment_id
                ))
                .unwrap(),
                packages: request
                    .packages
                    .iter()
                    .map(|package| CarrierPackageManifestResult {
                        carton_id: package.carton_id,
                        tracking_number: TrackingNumber::new(format!(
                            "GW-{}-{}",
                            request.shipment_id, package.carton_id
                        ))
                        .unwrap(),
                    })
                    .collect(),
            }),
        }
    }
}

async fn grant_carrier_manager(fixture: &Fixture, tenant_id: TenantId, user_id: i64) {
    let mut tx = tenant_tx(&fixture.db, tenant_id).await;
    sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES($1,$2)")
        .bind(tenant_id.get())
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        &format!("carrier-manager-{user_id}"),
        Some("Carrier account and recovery manager"),
    )
    .await
    .unwrap();
    for permission_name in ["admin", "wms", "wms_supervisor"] {
        let permission = match wareboxes_persistence_postgres::permissions::find_by_name(
            &fixture.db,
            tenant_id,
            permission_name,
        )
        .await
        .unwrap()
        {
            Some(permission) => permission.id,
            None => wareboxes_persistence_postgres::permissions::add_permission(
                &fixture.db,
                tenant_id,
                permission_name,
                Some("Carrier acceptance permission"),
            )
            .await
            .unwrap(),
        };
        assert!(wareboxes_persistence_postgres::roles::add_role_permission(
            &fixture.db,
            tenant_id,
            role,
            permission,
        )
        .await
        .unwrap());
    }
    assert!(wareboxes_persistence_postgres::roles::add_role_to_user(
        &fixture.db,
        tenant_id,
        user_id,
        role,
    )
    .await
    .unwrap());
}

fn worker_config() -> CarrierManifestWorkerConfig {
    CarrierManifestWorkerConfig {
        batch_size: 10,
        tenant_page_size: 100,
        lease: Duration::from_secs(30),
        request_timeout: Duration::from_secs(10),
        retry_delay: Duration::ZERO,
        retry_delay_cap: Duration::ZERO,
        max_attempts: 3,
    }
}

#[tokio::test]
async fn carrier_manifest_jobs_are_versioned_replay_safe_recoverable_and_atomic() {
    let fixture = Fixture::new().await;
    let operator = fixture.wms_user("carrier-operator@test.local").await;
    let access = default_tenant_for_user(&fixture.db, operator.id)
        .await
        .unwrap();
    grant_orders(&fixture.db, access.tenant_id, operator.id, "carrier-orders").await;
    let manager = fixture.user("carrier-manager@test.local").await;
    grant_carrier_manager(&fixture, access.tenant_id, manager.id).await;

    let owner_id = fixture
        .inventory_owner(access.tenant_id, "Carrier Owner")
        .await;
    let facility_id = fixture.facility(access.tenant_id, "Carrier Facility").await;
    fixture
        .assign_owner_to_facility(access.tenant_id, owner_id, facility_id)
        .await;
    let station_id =
        execution_location(&fixture, access.tenant_id, facility_id, "CARRIER-PACK").await;
    plate_at(
        &fixture,
        access.tenant_id,
        owner_id,
        facility_id,
        station_id,
        "CARRIER-TOTE",
    )
    .await;
    set_facility_address(
        &fixture,
        access.tenant_id,
        facility_id,
        "carrier-origin",
        true,
    )
    .await;
    let operator_token = auth::create_session(&fixture.db, operator.id)
        .await
        .unwrap();
    let manager_token = auth::create_session(&fixture.db, manager.id).await.unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let ready = prepare_ready_shipment(
        &fixture,
        &app,
        &operator_token,
        &access,
        owner_id,
        facility_id,
        station_id,
        "CARRIER",
    )
    .await;
    let shipment: CreateShipmentResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/orders/{}/shipments", ready.order_id),
                Some("carrier-shipment"),
                Some(create_shipment_body(&ready)),
            )
            .await,
            StatusCode::OK,
            "create carrier shipment",
        )
        .await,
    )
    .await;
    let shipment_id = shipment.shipment.shipment_id;

    let account_body = json!({
        "inventory_owner_id": owner_id,
        "facility_id": facility_id,
        "display_name": "Parcel gateway",
        "carrier_code": "UPS",
        "account_key": "ups-west-primary"
    });
    let account: CarrierAccountResponse = response_json(
        expect_status(
            send(
                &app,
                &manager_token,
                access.tenant_id,
                Method::POST,
                "/api/v1/carrier-accounts",
                Some("carrier-account"),
                Some(account_body.clone()),
            )
            .await,
            StatusCode::OK,
            "create carrier account",
        )
        .await,
    )
    .await;
    let replayed: CarrierAccountResponse = response_json(
        expect_status(
            send(
                &app,
                &manager_token,
                access.tenant_id,
                Method::POST,
                "/api/v1/carrier-accounts",
                Some("carrier-account"),
                Some(account_body),
            )
            .await,
            StatusCode::OK,
            "replay carrier account",
        )
        .await,
    )
    .await;
    assert_eq!(replayed, account);
    let accounts: CarrierAccountPage = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/carrier-accounts?inventory_owner_id={owner_id}&facility_id={facility_id}&limit=1"
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "list carrier accounts",
        )
        .await,
    )
    .await;
    assert_eq!(accounts.items, vec![account.clone()]);

    let queue_body = json!({
        "account_id": account.account_id,
        "service_code": "GROUND",
        "expected_shipment_revision": 1
    });
    let queued: CarrierManifestJobResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/shipments/{shipment_id}/carrier-manifest-jobs"),
                Some("carrier-queue"),
                Some(queue_body.clone()),
            )
            .await,
            StatusCode::OK,
            "queue carrier manifest",
        )
        .await,
    )
    .await;
    assert_eq!(queued.status, CarrierManifestJobStatus::Queued);
    let queue_replay: CarrierManifestJobResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/shipments/{shipment_id}/carrier-manifest-jobs"),
                Some("carrier-queue"),
                Some(queue_body),
            )
            .await,
            StatusCode::OK,
            "replay carrier queue",
        )
        .await,
    )
    .await;
    assert_eq!(queue_replay, queued);

    let cancelled: CarrierManifestJobResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/shipments/{shipment_id}/carrier-manifest-jobs/{}/cancellations",
                    queued.job_id
                ),
                Some("carrier-cancel"),
                Some(json!({"expected_revision": queued.revision})),
            )
            .await,
            StatusCode::OK,
            "cancel queued carrier manifest",
        )
        .await,
    )
    .await;
    assert_eq!(cancelled.status, CarrierManifestJobStatus::Cancelled);
    let cancellation_replay: CarrierManifestJobResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/shipments/{shipment_id}/carrier-manifest-jobs/{}/cancellations",
                    queued.job_id
                ),
                Some("carrier-cancel"),
                Some(json!({"expected_revision": queued.revision})),
            )
            .await,
            StatusCode::OK,
            "replay carrier cancellation",
        )
        .await,
    )
    .await;
    assert_eq!(cancellation_replay, cancelled);
    let queued: CarrierManifestJobResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::POST,
                &format!("/api/v1/shipments/{shipment_id}/carrier-manifest-jobs"),
                Some("carrier-queue-replacement"),
                Some(json!({
                    "account_id": account.account_id,
                    "service_code": "GROUND",
                    "expected_shipment_revision": 1
                })),
            )
            .await,
            StatusCode::OK,
            "queue replacement carrier manifest",
        )
        .await,
    )
    .await;

    let manual = send(
        &app,
        &operator_token,
        access.tenant_id,
        Method::POST,
        &format!("/api/v1/shipments/{shipment_id}/manifests"),
        Some("carrier-manual-conflict"),
        Some(manifest_body(&ready, "CARRIER-MANUAL", 1)),
    )
    .await;
    assert_eq!(manual.status(), StatusCode::CONFLICT);
    let reconfigure_active = send(
        &app,
        &manager_token,
        access.tenant_id,
        Method::POST,
        &format!(
            "/api/v1/carrier-accounts/{}/reconfigurations",
            account.account_id
        ),
        Some("carrier-reconfigure-active"),
        Some(json!({
            "display_name": "Parcel gateway updated",
            "account_key": "ups-west-secondary",
            "expected_revision": 1
        })),
    )
    .await;
    assert_eq!(reconfigure_active.status(), StatusCode::CONFLICT);

    let store = Arc::new(PostgresCarrierManifestStore::new(fixture.db.clone()));
    let abandoned = store
        .claim(
            access.tenant_id,
            "carrier-worker-abandoned",
            10,
            Duration::from_secs(1),
        )
        .await
        .unwrap();
    assert_eq!(abandoned.len(), 1);
    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let gateway = Arc::new(ScriptedGateway::new([
        GatewayStep::Retryable,
        GatewayStep::Permanent,
        GatewayStep::Success,
    ]));
    let first = CarrierManifestWorker::new(
        Arc::clone(&store),
        Arc::clone(&gateway),
        "carrier-worker-a",
        worker_config(),
    )
    .unwrap()
    .run_discovered_cycle()
    .await
    .unwrap();
    assert_eq!(first.retry_scheduled, 1);
    let second = CarrierManifestWorker::new(
        Arc::clone(&store),
        Arc::clone(&gateway),
        "carrier-worker-b",
        worker_config(),
    )
    .unwrap()
    .run_discovered_cycle()
    .await
    .unwrap();
    assert_eq!(second.failed, 1);

    let failed: CarrierManifestJobResponse = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::GET,
                &format!(
                    "/api/v1/shipments/{shipment_id}/carrier-manifest-jobs/{}",
                    queued.job_id
                ),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "read failed carrier manifest",
        )
        .await,
    )
    .await;
    assert_eq!(failed.status, CarrierManifestJobStatus::Failed);
    assert_eq!(failed.attempt_count, 3);
    let retried: CarrierManifestJobResponse = response_json(
        expect_status(
            send(
                &app,
                &manager_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/shipments/{shipment_id}/carrier-manifest-jobs/{}/retries",
                    queued.job_id
                ),
                Some("carrier-supervisor-retry"),
                Some(json!({"expected_revision": failed.revision})),
            )
            .await,
            StatusCode::OK,
            "retry failed carrier manifest",
        )
        .await,
    )
    .await;
    assert_eq!(retried.status, CarrierManifestJobStatus::RetryScheduled);
    assert_eq!(retried.request_key, queued.request_key);
    assert_eq!(retried.request_sha256, queued.request_sha256);

    let worker_c = CarrierManifestWorker::new(
        Arc::clone(&store),
        Arc::clone(&gateway),
        "carrier-worker-c",
        worker_config(),
    )
    .unwrap();
    let worker_d = CarrierManifestWorker::new(
        Arc::clone(&store),
        Arc::clone(&gateway),
        "carrier-worker-d",
        worker_config(),
    )
    .unwrap();
    let (third, fourth) = tokio::join!(
        worker_c.run_discovered_cycle(),
        worker_d.run_discovered_cycle()
    );
    let third = third.unwrap();
    let fourth = fourth.unwrap();
    assert_eq!(third.claimed + fourth.claimed, 1);
    assert_eq!(third.succeeded + fourth.succeeded, 1);

    let jobs: CarrierManifestJobPage = response_json(
        expect_status(
            send(
                &app,
                &operator_token,
                access.tenant_id,
                Method::GET,
                &format!("/api/v1/shipments/{shipment_id}/carrier-manifest-jobs?limit=10"),
                None,
                None,
            )
            .await,
            StatusCode::OK,
            "list carrier manifest history",
        )
        .await,
    )
    .await;
    assert_eq!(jobs.items.len(), 2);
    let succeeded = &jobs.items[0];
    assert_eq!(succeeded.status, CarrierManifestJobStatus::Succeeded);
    assert_eq!(succeeded.attempt_count, 4);
    assert_eq!(succeeded.account_revision.get(), 1);
    assert_eq!(succeeded.account_key, "ups-west-primary");
    assert!(succeeded.manifest_id.is_some());
    assert_eq!(jobs.items[1].status, CarrierManifestJobStatus::Cancelled);

    let captured = gateway.requests();
    assert_eq!(captured.len(), 3);
    assert!(captured.windows(2).all(|pair| pair[0] == pair[1]));
    assert_eq!(captured[0].request_key, queued.request_key);

    let mut tx = tenant_tx(&fixture.db, access.tenant_id).await;
    let evidence: (i64, i64, i64, i64, i64) = sqlx::query_as(
        r#"SELECT
          (SELECT COUNT(*) FROM carrier_manifest_attempts WHERE carrier_manifest_job_id=$1),
          (SELECT COUNT(*) FROM carrier_manifest_attempt_results WHERE carrier_manifest_job_id=$1),
          (SELECT COUNT(*) FROM shipment_manifests WHERE shipment_id=$2),
          (SELECT COUNT(*) FROM shipment_manifest_packages WHERE shipment_id=$2),
          (SELECT COUNT(*) FROM outbox_events WHERE event_type IN
             ('carrier.manifest.claim_lost','carrier.manifest.retry_scheduled','carrier.manifest.failed',
              'carrier.manifest.succeeded','shipping.shipment_manifested')
             AND (aggregate_id=$1::text OR payload->>'shipment_id'=$2::text))"#,
    )
    .bind(queued.job_id)
    .bind(shipment_id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(evidence, (4, 4, 1, 2, 6));
    let attempt_history: Vec<(String, String)> = sqlx::query_as(
        r#"SELECT outcome,recorded_by_worker_id
           FROM carrier_manifest_attempt_results
           WHERE carrier_manifest_job_id=$1 ORDER BY attempt_number"#,
    )
    .bind(queued.job_id)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(attempt_history[0].0, "claim_lost");
    assert_eq!(attempt_history[0].1, "carrier-worker-a");
    tx.rollback().await.unwrap();

    let updated: CarrierAccountResponse = response_json(
        expect_status(
            send(
                &app,
                &manager_token,
                access.tenant_id,
                Method::POST,
                &format!(
                    "/api/v1/carrier-accounts/{}/reconfigurations",
                    account.account_id
                ),
                Some("carrier-reconfigure-complete"),
                Some(json!({
                    "display_name": "Parcel gateway updated",
                    "account_key": "ups-west-secondary",
                    "expected_revision": 1
                })),
            )
            .await,
            StatusCode::OK,
            "reconfigure completed carrier account",
        )
        .await,
    )
    .await;
    assert_eq!(updated.revision.get(), 2);

    let foreign = fixture.wms_user("carrier-foreign@test.local").await;
    let foreign_access = default_tenant_for_user(&fixture.db, foreign.id)
        .await
        .unwrap();
    let foreign_token = auth::create_session(&fixture.db, foreign.id).await.unwrap();
    let concealed = send(
        &app,
        &foreign_token,
        foreign_access.tenant_id,
        Method::GET,
        &format!(
            "/api/v1/shipments/{shipment_id}/carrier-manifest-jobs/{}",
            queued.job_id
        ),
        None,
        None,
    )
    .await;
    assert_eq!(concealed.status(), StatusCode::NOT_FOUND);
    let mut foreign_tx = tenant_tx(&fixture.db, foreign_access.tenant_id).await;
    for table in [
        "carrier_accounts",
        "carrier_account_versions",
        "carrier_manifest_jobs",
        "carrier_manifest_attempts",
        "carrier_manifest_attempt_results",
    ] {
        let count: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&mut *foreign_tx)
            .await
            .unwrap();
        assert_eq!(count, 0, "{table} leaked across tenants");
    }
    foreign_tx.rollback().await.unwrap();
}
