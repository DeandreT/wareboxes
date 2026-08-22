mod common;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::{IDEMPOTENCY_KEY_HEADER, REQUEST_ID_HEADER};
use wareboxes_api::routes;
use wareboxes_api::state::AppState;
use wareboxes_api_contract::v1::{ErrorReason as V1ErrorReason, ErrorResponse as V1ErrorResponse};
use wareboxes_api_contract::web::{ErrorCode, ErrorResponse};

async fn error_body(response: axum::response::Response) -> ErrorResponse {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn response_text(response: axum::response::Response) -> String {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn tenant_cell_move_command_request(
    token: &str,
    tenant_id: i64,
    uri: &str,
    idempotency_key: &str,
    body: &serde_json::Value,
) -> Request<Body> {
    Request::builder()
        .method(Method::POST)
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string())
        .header(IDEMPOTENCY_KEY_HEADER, idempotency_key)
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(body).unwrap()))
        .unwrap()
}

fn metric_value(metrics: &str, sample: &str) -> f64 {
    metrics
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(' ')?;
            (name == sample).then(|| value.parse::<f64>().unwrap())
        })
        .unwrap_or_else(|| panic!("missing metric sample {sample}"))
}

fn assert_tenant_cell_move_snapshot_omitted(metrics: &str) {
    for metric in [
        "wareboxes_tenant_cell_moves_active{status=",
        "wareboxes_tenant_cell_move_oldest_active_age_seconds",
        "wareboxes_tenant_write_fences_active",
        "wareboxes_tenant_write_fence_max_age_seconds",
        "wareboxes_tenant_write_fence_state_mismatches",
        "wareboxes_tenant_cell_moves_awaiting_post_cutover_verification",
        "wareboxes_tenant_cell_moves_awaiting_validation",
        "wareboxes_tenant_cell_move_max_copy_replay_lag_bytes",
        "wareboxes_tenant_cell_move_capacity_reservations{direction=",
        "wareboxes_data_cells_exhausted_active",
        "wareboxes_tenant_cell_move_unpublished_outbox_events",
        "wareboxes_tenant_cell_move_oldest_unpublished_outbox_age_seconds",
        "wareboxes_tenant_cell_move_outcomes_total{outcome=",
    ] {
        assert!(
            !metrics.contains(metric),
            "failed collection exported authoritative metric {metric}"
        );
    }
}

async fn grant_platform_administrator(db: &wareboxes_api::db::Db, user_id: i64) {
    let admin_db = common::admin_db_for(db).await;
    sqlx::query(
        r#"INSERT INTO platform_administrators
        (user_id,revision,granted_at,granted_by_user_id)
        VALUES($1,1,CURRENT_TIMESTAMP,$1)"#,
    )
    .bind(user_id)
    .execute(&admin_db)
    .await
    .unwrap();
    admin_db.close().await;
}

async fn v1_error_body(response: axum::response::Response) -> V1ErrorResponse {
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn health_endpoints_distinguish_liveness_from_database_readiness() {
    let fixture = common::Fixture::new().await;
    let platform_admin = fixture
        .user("metrics-empty-platform-admin@test.local")
        .await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let db = fixture.db.clone();
    let app = routes::app(AppState::new(db.clone()));

    let live = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(live.status(), StatusCode::OK);
    assert!(response_text(live).await.contains("\"status\":\"ok\""));

    let ready = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(ready.status(), StatusCode::OK);
    assert!(response_text(ready).await.contains("\"status\":\"ready\""));

    let metrics = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metrics.status(), StatusCode::OK);
    assert!(metrics
        .headers()
        .get(header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("text/plain"));
    let metrics = response_text(metrics).await;
    assert!(metrics.contains("wareboxes_http_requests_total{status_class=\"2xx\"} 2"));
    assert!(metrics.contains("wareboxes_readiness_checks_total{result=\"ready\"} 1"));
    assert!(metrics.contains("wareboxes_database_pool_connections{state=\"open\"}"));
    assert!(metrics.contains("wareboxes_tenant_cell_move_metrics_collection_success 1"));
    for status in ["planned", "copying", "frozen", "validated", "cut_over"] {
        assert!(metrics.contains(&format!(
            "wareboxes_tenant_cell_moves_active{{status=\"{status}\"}} 0"
        )));
    }
    assert_eq!(
        metric_value(
            &metrics,
            "wareboxes_tenant_cell_move_oldest_active_age_seconds"
        ),
        0.0
    );
    assert_eq!(
        metric_value(&metrics, "wareboxes_tenant_write_fences_active"),
        0.0
    );
    assert_eq!(
        metric_value(&metrics, "wareboxes_tenant_write_fence_max_age_seconds"),
        0.0
    );
    assert_eq!(
        metric_value(&metrics, "wareboxes_tenant_write_fence_state_mismatches"),
        0.0
    );
    assert_eq!(
        metric_value(
            &metrics,
            "wareboxes_tenant_cell_moves_awaiting_post_cutover_verification"
        ),
        0.0
    );
    assert_eq!(
        metric_value(&metrics, "wareboxes_tenant_cell_moves_awaiting_validation"),
        0.0
    );
    assert_eq!(
        metric_value(
            &metrics,
            "wareboxes_tenant_cell_move_max_copy_replay_lag_bytes"
        ),
        0.0
    );
    for direction in ["target", "source_rollback"] {
        assert_eq!(
            metric_value(
                &metrics,
                &format!(
                    "wareboxes_tenant_cell_move_capacity_reservations{{direction=\"{direction}\"}}"
                )
            ),
            0.0
        );
    }
    for metric in [
        "wareboxes_data_cells_exhausted_active",
        "wareboxes_tenant_cell_move_unpublished_outbox_events",
        "wareboxes_tenant_cell_move_oldest_unpublished_outbox_age_seconds",
    ] {
        assert_eq!(metric_value(&metrics, metric), 0.0);
    }
    for outcome in ["cut_over", "completed", "rolled_back", "cancelled"] {
        assert_eq!(
            metric_value(
                &metrics,
                &format!("wareboxes_tenant_cell_move_outcomes_total{{outcome=\"{outcome}\"}}")
            ),
            0.0
        );
    }
    for command in ["validate", "cutover", "rollback"] {
        assert_eq!(
            metric_value(
                &metrics,
                &format!(
                    "wareboxes_tenant_cell_move_command_rejections_total{{command=\"{command}\"}}"
                )
            ),
            0.0
        );
    }
}

#[tokio::test]
async fn readiness_fails_closed_when_the_database_is_unavailable() {
    let db = common::setup().await;
    let app = routes::app(AppState::new(db.clone()));

    let metrics_without_platform_admin = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metrics_without_platform_admin.status(), StatusCode::OK);
    let metrics_without_platform_admin = response_text(metrics_without_platform_admin).await;
    assert!(metrics_without_platform_admin
        .contains("wareboxes_tenant_cell_move_metrics_collection_success 0"));
    assert_tenant_cell_move_snapshot_omitted(&metrics_without_platform_admin);

    db.close().await;

    let live = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/health/live")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(live.status(), StatusCode::OK);

    for path in ["/health", "/health/ready"] {
        let ready = app
            .clone()
            .oneshot(Request::builder().uri(path).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(ready.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert!(response_text(ready)
            .await
            .contains("\"status\":\"unready\""));
    }

    let metrics = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metrics.status(), StatusCode::OK);
    let metrics = response_text(metrics).await;
    assert!(metrics.contains("wareboxes_build_info"));
    assert!(metrics.contains("wareboxes_tenant_cell_move_metrics_collection_success 0"));
    assert_tenant_cell_move_snapshot_omitted(&metrics);
}

#[tokio::test]
async fn metrics_report_cell_move_lag_reservations_and_outcomes() {
    let fixture = common::Fixture::new().await;
    let platform_admin = fixture.user("metrics-platform-admin@test.local").await;
    let tenant_id = common::tenant_for_user(&fixture.db, platform_admin.id).await;
    let frozen_tenant_user = fixture.user("metrics-frozen-tenant@test.local").await;
    let frozen_tenant_id = common::tenant_for_user(&fixture.db, frozen_tenant_user.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let unauthorized_metrics = sqlx::query("SELECT * FROM tenant_cell_move_outbox_metrics()")
        .fetch_one(&fixture.db)
        .await
        .err()
        .expect("metrics function must reject a missing platform actor");
    assert_eq!(
        unauthorized_metrics
            .as_database_error()
            .and_then(|error| error.code())
            .as_deref(),
        Some("42501")
    );
    let admin_db = common::admin_db_for(&fixture.db).await;
    let mut tx = admin_db.begin().await.unwrap();

    // Build an age-controlled collector fixture without exercising the move API;
    // lifecycle behavior and its trigger evidence are covered by the dedicated
    // tenant-cell-move contract tests.
    sqlx::query("SET LOCAL session_replication_role = replica")
        .execute(&mut *tx)
        .await
        .unwrap();
    let source_data_cell_id: i64 =
        sqlx::query_scalar("SELECT data_cell_id FROM tenant_cell_placements WHERE tenant_id=$1")
            .bind(tenant_id.get())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let frozen_source_data_cell_id: i64 =
        sqlx::query_scalar("SELECT data_cell_id FROM tenant_cell_placements WHERE tenant_id=$1")
            .bind(frozen_tenant_id.get())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let target_data_cell_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO data_cells(
            cell_key,name,region,residency_code,mode,status,revision,max_tenants,
            created_at,created_by_user_id
        ) VALUES(
            'metrics-target','Metrics target','metrics-region','GLOBAL','shared',
            'active',1,1,CURRENT_TIMESTAMP,$1
        ) RETURNING id"#,
    )
    .bind(platform_admin.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let _frozen_tenant_cell_move_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO tenant_cell_moves(
            tenant_id,source_data_cell_id,target_data_cell_id,
            source_placement_revision,residency_requirement,status,revision,
            last_action,reason,requested_at,requested_by_user_id,
            changed_at,changed_by_user_id,change_reason,
            copy_reference,copy_started_at,copy_started_by_user_id,
            latest_source_wal_lsn,latest_target_replay_lsn,copied_row_count,
            copied_bytes,checkpointed_at,checkpointed_by_user_id,
            frozen_at,frozen_by_user_id
        ) VALUES(
            $1,$2,$3,1,'GLOBAL','frozen',4,'writes_frozen',
            'metrics frozen collector fixture',CURRENT_TIMESTAMP-INTERVAL '2 hours',$4,
            CURRENT_TIMESTAMP-INTERVAL '20 minutes',$4,'metrics fixture freeze',
            'metrics-frozen-copy',CURRENT_TIMESTAMP-INTERVAL '100 minutes',$4,
            '0/120'::PG_LSN,'0/20'::PG_LSN,84,8192,
            CURRENT_TIMESTAMP-INTERVAL '25 minutes',$4,
            CURRENT_TIMESTAMP-INTERVAL '20 minutes',$4
        ) RETURNING id"#,
    )
    .bind(frozen_tenant_id.get())
    .bind(frozen_source_data_cell_id)
    .bind(target_data_cell_id)
    .bind(platform_admin.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let tenant_cell_move_id: i64 = sqlx::query_scalar(
        r#"INSERT INTO tenant_cell_moves(
            tenant_id,source_data_cell_id,target_data_cell_id,
            source_placement_revision,cutover_placement_revision,
            residency_requirement,status,revision,last_action,reason,
            requested_at,requested_by_user_id,changed_at,changed_by_user_id,
            copy_reference,copy_started_at,copy_started_by_user_id,
            latest_source_wal_lsn,latest_target_replay_lsn,copied_row_count,
            copied_bytes,checkpointed_at,checkpointed_by_user_id,
            frozen_at,frozen_by_user_id,validated_at,validated_by_user_id,
            cutover_at,cutover_by_user_id
        ) VALUES(
            $1,$2,$3,1,2,'GLOBAL','cut_over',6,'cut_over',
            'metrics collector fixture',CURRENT_TIMESTAMP-INTERVAL '3 hours',$4,
            CURRENT_TIMESTAMP-INTERVAL '10 minutes',$4,
            'metrics-copy',CURRENT_TIMESTAMP-INTERVAL '170 minutes',$4,
            '0/20'::PG_LSN,'0/20'::PG_LSN,42,4096,
            CURRENT_TIMESTAMP-INTERVAL '30 minutes',$4,
            CURRENT_TIMESTAMP-INTERVAL '20 minutes',$4,
            CURRENT_TIMESTAMP-INTERVAL '15 minutes',$4,
            CURRENT_TIMESTAMP-INTERVAL '10 minutes',$4
        ) RETURNING id"#,
    )
    .bind(tenant_id.get())
    .bind(source_data_cell_id)
    .bind(target_data_cell_id)
    .bind(platform_admin.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO tenant_cell_move_events(
            tenant_id,tenant_cell_move_id,action,move_revision,previous_status,
            resulting_status,actor_user_id,occurred_at,request_id,evidence
        ) VALUES(
            $1,$2,'writes_frozen',4,'copying','frozen',$3,
            CURRENT_TIMESTAMP-INTERVAL '20 minutes','metrics-freeze-event','{}'::jsonb
        )"#,
    )
    .bind(tenant_id.get())
    .bind(tenant_cell_move_id)
    .bind(platform_admin.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO tenant_write_fences(
            tenant_id,tenant_cell_move_id,fence_epoch,frozen_at,frozen_by_user_id
        ) VALUES(
            $1,$2,3,CURRENT_TIMESTAMP-INTERVAL '20 minutes',$3
        )"#,
    )
    .bind(tenant_id.get())
    .bind(tenant_cell_move_id)
    .bind(platform_admin.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO outbox_events(
            tenant_id,actor_user_id,created,event_key,aggregate_type,aggregate_id,
            ordering_key,aggregate_sequence,event_type,schema_version,payload,
            occurred_at,available_at
        ) VALUES(
            $1,$3,CURRENT_TIMESTAMP-INTERVAL '12 minutes',
            'metrics-tenant-cell-move-outbox','tenant_cell_move',$2::text,
            'tenant-cell-move:'||$2::text,1,'tenant_cell_move.cut_over.v1',1,
            jsonb_build_object('tenant_cell_move_id',$2),
            CURRENT_TIMESTAMP-INTERVAL '12 minutes',
            CURRENT_TIMESTAMP-INTERVAL '12 minutes'
        )"#,
    )
    .bind(tenant_id.get())
    .bind(tenant_cell_move_id)
    .bind(platform_admin.id)
    .execute(&mut *tx)
    .await
    .unwrap();
    tx.commit().await.unwrap();
    admin_db.close().await;

    let metrics = routes::app(AppState::new(fixture.db.clone()))
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metrics.status(), StatusCode::OK);
    let metrics = response_text(metrics).await;
    assert!(metrics.contains("wareboxes_tenant_cell_move_metrics_collection_success 1"));
    assert!(metrics.contains("wareboxes_tenant_cell_moves_active{status=\"cut_over\"} 1"));
    assert!(metrics.contains("wareboxes_tenant_cell_moves_active{status=\"frozen\"} 1"));
    assert_eq!(
        metric_value(&metrics, "wareboxes_tenant_write_fences_active"),
        1.0
    );
    assert_eq!(
        metric_value(&metrics, "wareboxes_tenant_write_fence_state_mismatches"),
        2.0
    );
    assert_eq!(
        metric_value(
            &metrics,
            "wareboxes_tenant_cell_moves_awaiting_post_cutover_verification"
        ),
        1.0
    );
    assert_eq!(
        metric_value(&metrics, "wareboxes_data_cells_exhausted_active"),
        1.0
    );
    assert_eq!(
        metric_value(
            &metrics,
            "wareboxes_tenant_cell_move_unpublished_outbox_events"
        ),
        1.0
    );
    assert!(
        metric_value(
            &metrics,
            "wareboxes_tenant_cell_move_oldest_unpublished_outbox_age_seconds"
        ) > 700.0
    );
    assert_eq!(
        metric_value(&metrics, "wareboxes_tenant_cell_moves_awaiting_validation"),
        1.0
    );
    assert_eq!(
        metric_value(
            &metrics,
            "wareboxes_tenant_cell_move_max_copy_replay_lag_bytes"
        ),
        256.0
    );
    assert_eq!(
        metric_value(
            &metrics,
            "wareboxes_tenant_cell_move_capacity_reservations{direction=\"target\"}"
        ),
        1.0
    );
    assert_eq!(
        metric_value(
            &metrics,
            "wareboxes_tenant_cell_move_capacity_reservations{direction=\"source_rollback\"}"
        ),
        1.0
    );
    assert_eq!(
        metric_value(
            &metrics,
            "wareboxes_tenant_cell_move_outcomes_total{outcome=\"cut_over\"}"
        ),
        1.0
    );
    for outcome in ["completed", "rolled_back", "cancelled"] {
        assert_eq!(
            metric_value(
                &metrics,
                &format!("wareboxes_tenant_cell_move_outcomes_total{{outcome=\"{outcome}\"}}")
            ),
            0.0
        );
    }
    let oldest_active_revision_age = metric_value(
        &metrics,
        "wareboxes_tenant_cell_move_oldest_active_age_seconds",
    );
    assert!(oldest_active_revision_age > 1_100.0);
    assert!(oldest_active_revision_age < 1_800.0);
    assert!(metric_value(&metrics, "wareboxes_tenant_write_fence_max_age_seconds") > 1_100.0);
    assert!(!metrics.contains("tenant_id="));
    assert!(!metrics.contains("tenant_cell_move_id="));
}

#[tokio::test]
async fn tenant_cell_move_outbox_metrics_use_selective_index_with_representative_backlog() {
    const UNRELATED_PENDING_EVENTS: i32 = 4_096;
    const TENANT_CELL_MOVE_PENDING_EVENTS: i32 = 64;
    const METRICS_INDEX: &str = "outbox_events_tenant_cell_move_pending_metrics_idx";

    let fixture = common::Fixture::new().await;
    let platform_admin = fixture
        .user("metrics-outbox-plan-platform-admin@test.local")
        .await;
    let tenant_id = common::tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let admin_db = common::admin_db_for(&fixture.db).await;
    let baseline_tenant_cell_move_events: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*) FROM outbox_events
        WHERE aggregate_type='tenant_cell_move'
          AND published_at IS NULL AND discarded_at IS NULL"#,
    )
    .fetch_one(&admin_db)
    .await
    .unwrap();

    sqlx::query(
        r#"INSERT INTO outbox_events(
            tenant_id,actor_user_id,created,event_key,aggregate_type,aggregate_id,
            ordering_key,aggregate_sequence,event_type,schema_version,payload,
            occurred_at,available_at
        )
        SELECT $1,$2,CURRENT_TIMESTAMP,
            'metrics-unrelated-pending-'||backlog.entry_number::text,
            'inventory_transaction',backlog.entry_number::text,
            'metrics-unrelated-pending:'||backlog.entry_number::text,
            1,'inventory_transaction.recorded.v1',1,
            jsonb_build_object('entry_number',backlog.entry_number),
            CURRENT_TIMESTAMP,CURRENT_TIMESTAMP
        FROM generate_series(1,$3::integer) AS backlog(entry_number)"#,
    )
    .bind(tenant_id.get())
    .bind(platform_admin.id)
    .bind(UNRELATED_PENDING_EVENTS)
    .execute(&admin_db)
    .await
    .unwrap();
    sqlx::query(
        r#"INSERT INTO outbox_events(
            tenant_id,actor_user_id,created,event_key,aggregate_type,aggregate_id,
            ordering_key,aggregate_sequence,event_type,schema_version,payload,
            occurred_at,available_at
        )
        SELECT $1,$2,
            CURRENT_TIMESTAMP-make_interval(secs=>backlog.entry_number::double precision),
            'metrics-tenant-cell-move-pending-'||backlog.entry_number::text,
            'tenant_cell_move','move-'||backlog.entry_number::text,
            'metrics-tenant-cell-move-pending:'||backlog.entry_number::text,
            1,'tenant_cell_move.planned.v1',1,
            jsonb_build_object('tenant_cell_move_id',backlog.entry_number),
            CURRENT_TIMESTAMP,CURRENT_TIMESTAMP
        FROM generate_series(1,$3::integer) AS backlog(entry_number)"#,
    )
    .bind(tenant_id.get())
    .bind(platform_admin.id)
    .bind(TENANT_CELL_MOVE_PENDING_EVENTS)
    .execute(&admin_db)
    .await
    .unwrap();
    sqlx::query("ANALYZE outbox_events")
        .execute(&admin_db)
        .await
        .unwrap();

    let plan = sqlx::query_scalar::<_, String>(
        r#"EXPLAIN (COSTS OFF)
        SELECT COUNT(*),COALESCE(GREATEST(
            EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP-MIN(outbox.created)))::double precision,
            0::double precision),0::double precision)
        FROM outbox_events outbox
        WHERE outbox.aggregate_type='tenant_cell_move'
          AND outbox.published_at IS NULL AND outbox.discarded_at IS NULL"#,
    )
    .fetch_all(&admin_db)
    .await
    .unwrap()
    .join("\n");
    assert!(
        plan.contains(METRICS_INDEX),
        "tenant-cell-move outbox metrics must use {METRICS_INDEX} at representative backlog scale:\n{plan}"
    );
    assert!(
        !plan.contains("Seq Scan on outbox_events"),
        "tenant-cell-move outbox metrics regressed to a full outbox scan:\n{plan}"
    );

    let mut metrics_tx = fixture.db.begin().await.unwrap();
    sqlx::query_scalar::<_, String>(
        "SELECT set_config('wareboxes.platform_actor_user_id',$1,true)",
    )
    .bind(platform_admin.id.to_string())
    .fetch_one(&mut *metrics_tx)
    .await
    .unwrap();
    let (unpublished_events, oldest_age_seconds): (i64, f64) =
        sqlx::query_as("SELECT * FROM tenant_cell_move_outbox_metrics()")
            .fetch_one(&mut *metrics_tx)
            .await
            .unwrap();
    metrics_tx.commit().await.unwrap();
    assert_eq!(
        unpublished_events,
        baseline_tenant_cell_move_events + i64::from(TENANT_CELL_MOVE_PENDING_EVENTS)
    );
    assert!(oldest_age_seconds >= f64::from(TENANT_CELL_MOVE_PENDING_EVENTS));
    admin_db.close().await;
}

#[tokio::test]
async fn metrics_count_authorized_tenant_cell_move_command_rejections() {
    let fixture = common::Fixture::new().await;
    let platform_admin = fixture
        .user("metrics-command-rejection-admin@test.local")
        .await;
    let tenant_id = common::tenant_for_user(&fixture.db, platform_admin.id).await;
    grant_platform_administrator(&fixture.db, platform_admin.id).await;
    let token = common::auth::create_session(&fixture.db, platform_admin.id)
        .await
        .unwrap();
    let app = routes::app(AppState::new(fixture.db.clone()));
    let missing_move_id = i64::MAX - 17;
    let checksum = "a".repeat(64);

    let commands = [
        (
            "validations",
            "metrics-reject-validation",
            serde_json::json!({
                "expected_revision": 1,
                "validation": {
                    "tool_version": "metrics-validator/1.0.0",
                    "source_lsn": "0/20",
                    "target_replay_lsn": "0/20",
                    "source_row_count": 0,
                    "target_row_count": 0,
                    "source_data_checksum": checksum,
                    "target_data_checksum": checksum,
                    "source_schema_checksum": checksum,
                    "target_schema_checksum": checksum,
                    "source_object_manifest_checksum": checksum,
                    "target_object_manifest_checksum": checksum,
                    "inventory_reconciled": true,
                    "idempotency_verified": true,
                    "outbox_verified": true
                }
            }),
        ),
        (
            "cutovers",
            "metrics-reject-cutover",
            serde_json::json!({
                "expected_revision": 1,
                "expected_placement_revision": 1
            }),
        ),
        (
            "rollbacks",
            "metrics-reject-rollback",
            serde_json::json!({
                "expected_revision": 1,
                "verification": {
                    "tool_version": "metrics-rollback-validator/1.0.0",
                    "routing_reference": "deployment/metrics-rollback",
                    "observed_data_cell_id": 1,
                    "expected_rollback_placement_revision": 3,
                    "routing_verified": true,
                    "source_read_verified": true,
                    "write_fence_verified": true,
                    "inventory_reconciled": true,
                    "idempotency_verified": true,
                    "outbox_verified": true
                },
                "reason": "exercise bounded command rejection metrics"
            }),
        ),
    ];

    for (path, key, body) in commands {
        let response = app
            .clone()
            .oneshot(tenant_cell_move_command_request(
                &token,
                tenant_id.get(),
                &format!("/api/v1/platform/tenant-cell-moves/{missing_move_id}/{path}"),
                key,
                &body,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    let metrics = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(metrics.status(), StatusCode::OK);
    let metrics = response_text(metrics).await;
    for command in ["validate", "cutover", "rollback"] {
        assert_eq!(
            metric_value(
                &metrics,
                &format!(
                    "wareboxes_tenant_cell_move_command_rejections_total{{command=\"{command}\"}}"
                )
            ),
            1.0
        );
    }
    assert!(!metrics.contains("missing_move_id"));
    assert!(!metrics.contains(&missing_move_id.to_string()));
}

#[tokio::test]
async fn traffic_limits_fail_with_the_versioned_error_contract_and_spare_health() {
    let db = common::setup().await;
    let security = wareboxes_api::config::SecurityConfig {
        request_rate_limit_per_second: 2,
        ..wareboxes_api::config::SecurityConfig::default()
    };
    let app = routes::app(AppState::with_security(db, security));

    for request_number in 0..2 {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/v1/not-a-route")
                    .header(REQUEST_ID_HEADER, format!("rate-{request_number}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        assert_eq!(
            v1_error_body(response).await.reason,
            V1ErrorReason::NotFound
        );
    }

    let limited = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/v1/not-a-route")
                .header(REQUEST_ID_HEADER, "rate-limited")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(limited.status(), StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(limited.headers().get(header::RETRY_AFTER).unwrap(), "1");
    let limited = v1_error_body(limited).await;
    assert_eq!(limited.reason, V1ErrorReason::RateLimited);
    assert_eq!(limited.request_id, "rate-limited");

    let health = app
        .oneshot(
            Request::builder()
                .uri("/health/ready")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(health.status(), StatusCode::OK);
}

#[tokio::test]
async fn responses_expose_correlated_request_ids_and_stable_errors() {
    let db = common::setup().await;
    let app = routes::app(AppState::new(db));

    let success = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/health")
                .header(REQUEST_ID_HEADER, "client-42.trace_1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(success.status(), StatusCode::OK);
    assert_eq!(
        success.headers().get(REQUEST_ID_HEADER).unwrap(),
        "client-42.trace_1"
    );

    let validation = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .header(REQUEST_ID_HEADER, "validation-1")
                .body(Body::from(r#"{"email":"bad","password":""}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(validation.status(), StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        validation.headers().get(REQUEST_ID_HEADER).unwrap(),
        "validation-1"
    );
    let validation_body = error_body(validation).await;
    assert_eq!(validation_body.code, ErrorCode::ValidationFailed);
    assert_eq!(validation_body.message, "validation failed");
    assert_eq!(validation_body.request_id, "validation-1");
    assert!(validation_body
        .details
        .iter()
        .any(|detail| detail.field == "email"));
    assert!(validation_body
        .details
        .iter()
        .any(|detail| detail.field == "password"));

    let malformed = app
        .clone()
        .oneshot(
            Request::builder()
                .method(Method::POST)
                .uri("/api/auth/login")
                .header(header::CONTENT_TYPE, "application/json")
                .header(REQUEST_ID_HEADER, "malformed-1")
                .body(Body::from("{"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(malformed.status().is_client_error());
    assert_eq!(
        malformed.headers().get(REQUEST_ID_HEADER).unwrap(),
        "malformed-1"
    );
    let malformed_body = error_body(malformed).await;
    assert_eq!(malformed_body.code, ErrorCode::InvalidRequest);
    assert_eq!(malformed_body.request_id, "malformed-1");

    let missing = app
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/api/not-a-route")
                .header(REQUEST_ID_HEADER, "not valid")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);
    let generated = missing
        .headers()
        .get(REQUEST_ID_HEADER)
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    assert!(generated.starts_with("req_"));
    let missing_body = error_body(missing).await;
    assert_eq!(missing_body.code, ErrorCode::NotFound);
    assert_eq!(missing_body.request_id, generated);
}
