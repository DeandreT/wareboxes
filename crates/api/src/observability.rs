use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

use crate::db::Db;

const TENANT_CELL_MOVE_METRICS_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(sqlx::FromRow)]
struct TenantCellMoveMetricsSnapshot {
    planned: i64,
    copying: i64,
    frozen: i64,
    validated: i64,
    cut_over: i64,
    oldest_active_age_seconds: f64,
    active_write_fences: i64,
    write_fence_max_age_seconds: f64,
    write_fence_state_mismatches: i64,
    awaiting_post_cutover_verification: i64,
    awaiting_validation: i64,
    max_copy_replay_lag_bytes: f64,
    inbound_capacity_reservations: i64,
    rollback_capacity_reservations: i64,
    exhausted_active_data_cells: i64,
    unpublished_outbox_events: i64,
    oldest_unpublished_outbox_age_seconds: f64,
    accepted_cutovers: i64,
    completed: i64,
    rolled_back: i64,
    cancelled: i64,
}

impl TenantCellMoveMetricsSnapshot {
    async fn collect(db: &Db) -> Result<Self, sqlx::Error> {
        let mut tx = db.begin().await?;
        sqlx::query("SET TRANSACTION READ ONLY")
            .execute(&mut *tx)
            .await?;
        // Bound execution inside PostgreSQL as well as in Tokio. If a scrape future
        // is cancelled, the server must not retain a slow collector query and its
        // pool connection beyond the outer two-second budget.
        sqlx::query("SET LOCAL statement_timeout = '1500ms'")
            .execute(&mut *tx)
            .await?;

        // The governed move tables are protected by platform RLS. Select an active,
        // bootstrap-managed administrator only for this transaction so the private
        // collector can aggregate every move without weakening the policies or
        // exporting tenant identifiers.
        let platform_actor_id: i64 = sqlx::query_scalar(
            r#"SELECT administrator.user_id
            FROM platform_administrators administrator
            WHERE platform_actor_is_administrator(administrator.user_id)
            ORDER BY administrator.user_id
            LIMIT 1"#,
        )
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query_scalar::<_, String>(
            "SELECT set_config('wareboxes.platform_actor_user_id',$1,true)",
        )
        .bind(platform_actor_id.to_string())
        .fetch_one(&mut *tx)
        .await?;

        let snapshot = sqlx::query_as(
            r#"WITH move_metrics AS (
                SELECT
                    COUNT(*) FILTER (WHERE status='planned') AS planned,
                    COUNT(*) FILTER (WHERE status='copying') AS copying,
                    COUNT(*) FILTER (WHERE status='frozen') AS frozen,
                    COUNT(*) FILTER (WHERE status='validated') AS validated,
                    COUNT(*) FILTER (WHERE status='cut_over') AS cut_over,
                    COALESCE(GREATEST(
                        EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP-MIN(
                            COALESCE(changed_at,requested_at))
                            FILTER (WHERE status IN (
                                'planned','copying','frozen','validated','cut_over'
                            ))))
                            ::DOUBLE PRECISION,
                        0::DOUBLE PRECISION
                    ), 0::DOUBLE PRECISION) AS oldest_active_age_seconds,
                    COUNT(*) FILTER (
                        WHERE status='cut_over' AND post_cutover_verified_at IS NULL
                    ) AS awaiting_post_cutover_verification,
                    COUNT(*) FILTER (WHERE status='frozen') AS awaiting_validation,
                    COALESCE(MAX(
                        (latest_source_wal_lsn-latest_target_replay_lsn)::DOUBLE PRECISION
                    ) FILTER (WHERE status IN ('copying','frozen')
                        AND latest_source_wal_lsn IS NOT NULL
                        AND latest_target_replay_lsn IS NOT NULL),
                        0::DOUBLE PRECISION) AS max_copy_replay_lag_bytes,
                    COUNT(*) FILTER (WHERE status IN (
                        'planned','copying','frozen','validated'
                    )) AS inbound_capacity_reservations,
                    COUNT(*) FILTER (WHERE status='cut_over')
                        AS rollback_capacity_reservations,
                    COUNT(*) FILTER (WHERE status IN (
                        'cut_over','completed','rolled_back'
                    )) AS accepted_cutovers,
                    COUNT(*) FILTER (WHERE status='completed') AS completed,
                    COUNT(*) FILTER (WHERE status='rolled_back') AS rolled_back,
                    COUNT(*) FILTER (WHERE status='cancelled') AS cancelled
                FROM tenant_cell_moves
            ), fence_metrics AS (
                SELECT
                    COUNT(*) AS active_write_fences,
                    COALESCE(GREATEST(
                        EXTRACT(EPOCH FROM (CURRENT_TIMESTAMP-MIN(frozen_at)))
                            ::DOUBLE PRECISION,
                        0::DOUBLE PRECISION
                    ), 0::DOUBLE PRECISION) AS write_fence_max_age_seconds
                FROM tenant_write_fences
            ), fence_state_metrics AS (
                SELECT COUNT(*) AS write_fence_state_mismatches
                FROM tenant_cell_moves move
                FULL OUTER JOIN tenant_write_fences fence
                  ON fence.tenant_id=move.tenant_id
                 AND fence.tenant_cell_move_id=move.id
                LEFT JOIN LATERAL (
                    SELECT event.move_revision,event.actor_user_id,event.occurred_at
                    FROM tenant_cell_move_events event
                    WHERE event.tenant_id=move.tenant_id
                      AND event.tenant_cell_move_id=move.id
                      AND event.action='writes_frozen'
                    ORDER BY event.move_revision DESC
                    LIMIT 1
                ) freeze_event ON true
                WHERE (move.status IN ('frozen','validated','cut_over'))
                    IS DISTINCT FROM (fence.tenant_id IS NOT NULL)
                   OR (fence.tenant_id IS NOT NULL AND move.id IS NOT NULL AND (
                       fence.frozen_at IS DISTINCT FROM move.frozen_at
                       OR fence.frozen_by_user_id IS DISTINCT FROM move.frozen_by_user_id
                       OR ROW(fence.fence_epoch,fence.frozen_at,fence.frozen_by_user_id)
                          IS DISTINCT FROM ROW(freeze_event.move_revision,
                            freeze_event.occurred_at,freeze_event.actor_user_id)
                   ))
            ), placement_counts AS (
                SELECT data_cell_id,COUNT(*) AS placement_count
                FROM tenant_cell_placements
                GROUP BY data_cell_id
            ), inbound_reservations AS (
                SELECT target_data_cell_id AS data_cell_id,COUNT(*) AS reservation_count
                FROM tenant_cell_moves
                WHERE status IN ('planned','copying','frozen','validated')
                GROUP BY target_data_cell_id
            ), rollback_reservations AS (
                SELECT source_data_cell_id AS data_cell_id,COUNT(*) AS reservation_count
                FROM tenant_cell_moves
                WHERE status='cut_over'
                GROUP BY source_data_cell_id
            ), data_cell_metrics AS (
                SELECT COUNT(*) FILTER (
                    WHERE cell.status='active' AND cell.mode='shared'
                      AND COALESCE(placements.placement_count,0)
                        +COALESCE(inbound.reservation_count,0)
                        +COALESCE(rollback.reservation_count,0)>=cell.max_tenants
                ) AS exhausted_active_data_cells
                FROM data_cells cell
                LEFT JOIN placement_counts placements ON placements.data_cell_id=cell.id
                LEFT JOIN inbound_reservations inbound ON inbound.data_cell_id=cell.id
                LEFT JOIN rollback_reservations rollback ON rollback.data_cell_id=cell.id
            ), outbox_metrics AS (
                SELECT * FROM public.tenant_cell_move_outbox_metrics()
            )
            SELECT move_metrics.planned,move_metrics.copying,move_metrics.frozen,
                move_metrics.validated,move_metrics.cut_over,
                move_metrics.oldest_active_age_seconds,
                fence_metrics.active_write_fences,
                fence_metrics.write_fence_max_age_seconds,
                fence_state_metrics.write_fence_state_mismatches,
                move_metrics.awaiting_post_cutover_verification,
                move_metrics.awaiting_validation,
                move_metrics.max_copy_replay_lag_bytes,
                move_metrics.inbound_capacity_reservations,
                move_metrics.rollback_capacity_reservations,
                data_cell_metrics.exhausted_active_data_cells,
                outbox_metrics.unpublished_outbox_events,
                outbox_metrics.oldest_unpublished_outbox_age_seconds,
                move_metrics.accepted_cutovers,move_metrics.completed,
                move_metrics.rolled_back,move_metrics.cancelled
            FROM move_metrics
            CROSS JOIN fence_metrics
            CROSS JOIN fence_state_metrics
            CROSS JOIN data_cell_metrics
            CROSS JOIN outbox_metrics"#,
        )
        .fetch_one(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(snapshot)
    }

    fn render(&self, output: &mut String) {
        output.push_str(
            "# HELP wareboxes_tenant_cell_moves_active Current governed tenant-cell moves by lifecycle status.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_cell_moves_active gauge\n");
        for (status, value) in [
            ("planned", self.planned),
            ("copying", self.copying),
            ("frozen", self.frozen),
            ("validated", self.validated),
            ("cut_over", self.cut_over),
        ] {
            let _ = writeln!(
                output,
                "wareboxes_tenant_cell_moves_active{{status=\"{status}\"}} {value}"
            );
        }
        output.push_str(
            "# HELP wareboxes_tenant_cell_move_oldest_active_age_seconds Time since the least recently revised active governed tenant-cell move changed.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_cell_move_oldest_active_age_seconds gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_tenant_cell_move_oldest_active_age_seconds {:.3}",
            self.oldest_active_age_seconds
        );
        output.push_str(
            "# HELP wareboxes_tenant_write_fences_active Current tenant write fences held by governed cell moves.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_write_fences_active gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_tenant_write_fences_active {}",
            self.active_write_fences
        );
        output.push_str(
            "# HELP wareboxes_tenant_write_fence_max_age_seconds Maximum age of an active tenant write fence.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_write_fence_max_age_seconds gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_tenant_write_fence_max_age_seconds {:.3}",
            self.write_fence_max_age_seconds
        );
        output.push_str(
            "# HELP wareboxes_tenant_write_fence_state_mismatches Governed moves and write fences whose expected active state does not agree.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_write_fence_state_mismatches gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_tenant_write_fence_state_mismatches {}",
            self.write_fence_state_mismatches
        );
        output.push_str(
            "# HELP wareboxes_tenant_cell_moves_awaiting_post_cutover_verification Cut-over tenant-cell moves awaiting post-cutover verification.\n",
        );
        output.push_str(
            "# TYPE wareboxes_tenant_cell_moves_awaiting_post_cutover_verification gauge\n",
        );
        let _ = writeln!(
            output,
            "wareboxes_tenant_cell_moves_awaiting_post_cutover_verification {}",
            self.awaiting_post_cutover_verification
        );
        output.push_str(
            "# HELP wareboxes_tenant_cell_moves_awaiting_validation Frozen tenant-cell moves awaiting final validation.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_cell_moves_awaiting_validation gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_tenant_cell_moves_awaiting_validation {}",
            self.awaiting_validation
        );
        output.push_str(
            "# HELP wareboxes_tenant_cell_move_max_copy_replay_lag_bytes Maximum source-to-target WAL replay lag for copying or frozen moves.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_cell_move_max_copy_replay_lag_bytes gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_tenant_cell_move_max_copy_replay_lag_bytes {:.0}",
            self.max_copy_replay_lag_bytes
        );
        output.push_str(
            "# HELP wareboxes_tenant_cell_move_capacity_reservations Current tenant-cell move capacity reservations by direction.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_cell_move_capacity_reservations gauge\n");
        for (direction, value) in [
            ("target", self.inbound_capacity_reservations),
            ("source_rollback", self.rollback_capacity_reservations),
        ] {
            let _ = writeln!(
                output,
                "wareboxes_tenant_cell_move_capacity_reservations{{direction=\"{direction}\"}} {value}"
            );
        }
        output.push_str(
            "# HELP wareboxes_data_cells_exhausted_active Active shared data cells with no placement capacity after inbound and rollback reservations.\n",
        );
        output.push_str("# TYPE wareboxes_data_cells_exhausted_active gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_data_cells_exhausted_active {}",
            self.exhausted_active_data_cells
        );
        output.push_str(
            "# HELP wareboxes_tenant_cell_move_unpublished_outbox_events Unpublished and undiscarded tenant-cell-move outbox events.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_cell_move_unpublished_outbox_events gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_tenant_cell_move_unpublished_outbox_events {}",
            self.unpublished_outbox_events
        );
        output.push_str(
            "# HELP wareboxes_tenant_cell_move_oldest_unpublished_outbox_age_seconds Age of the oldest unpublished and undiscarded tenant-cell-move outbox event.\n",
        );
        output.push_str(
            "# TYPE wareboxes_tenant_cell_move_oldest_unpublished_outbox_age_seconds gauge\n",
        );
        let _ = writeln!(
            output,
            "wareboxes_tenant_cell_move_oldest_unpublished_outbox_age_seconds {:.3}",
            self.oldest_unpublished_outbox_age_seconds
        );
        output.push_str(
            "# HELP wareboxes_tenant_cell_move_outcomes_total Accepted terminal and placement outcomes recorded by governed tenant-cell moves.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_cell_move_outcomes_total counter\n");
        for (outcome, value) in [
            ("cut_over", self.accepted_cutovers),
            ("completed", self.completed),
            ("rolled_back", self.rolled_back),
            ("cancelled", self.cancelled),
        ] {
            let _ = writeln!(
                output,
                "wareboxes_tenant_cell_move_outcomes_total{{outcome=\"{outcome}\"}} {value}"
            );
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum TenantCellMoveCommandMetric {
    Validate,
    Cutover,
    Rollback,
}

impl TenantCellMoveCommandMetric {
    const fn label(self) -> &'static str {
        match self {
            Self::Validate => "validate",
            Self::Cutover => "cutover",
            Self::Rollback => "rollback",
        }
    }
}

fn render_tenant_cell_move_metrics(
    output: &mut String,
    snapshot: Option<&TenantCellMoveMetricsSnapshot>,
) {
    output.push_str(
        "# HELP wareboxes_tenant_cell_move_metrics_collection_success Whether governed tenant-cell move metrics were collected successfully.\n",
    );
    output.push_str("# TYPE wareboxes_tenant_cell_move_metrics_collection_success gauge\n");
    let _ = writeln!(
        output,
        "wareboxes_tenant_cell_move_metrics_collection_success {}",
        u8::from(snapshot.is_some())
    );
    if let Some(snapshot) = snapshot {
        snapshot.render(output);
    }
}

pub struct HttpMetrics {
    started_at: Instant,
    requests_total: AtomicU64,
    requests_in_flight: AtomicU64,
    request_duration_micros: AtomicU64,
    status_1xx: AtomicU64,
    status_2xx: AtomicU64,
    status_3xx: AtomicU64,
    status_4xx: AtomicU64,
    status_5xx: AtomicU64,
    readiness_ready: AtomicU64,
    readiness_unready: AtomicU64,
    rejected_tenant_cell_move_validations: AtomicU64,
    rejected_tenant_cell_move_cutovers: AtomicU64,
    rejected_tenant_cell_move_rollbacks: AtomicU64,
}

impl Default for HttpMetrics {
    fn default() -> Self {
        Self {
            started_at: Instant::now(),
            requests_total: AtomicU64::new(0),
            requests_in_flight: AtomicU64::new(0),
            request_duration_micros: AtomicU64::new(0),
            status_1xx: AtomicU64::new(0),
            status_2xx: AtomicU64::new(0),
            status_3xx: AtomicU64::new(0),
            status_4xx: AtomicU64::new(0),
            status_5xx: AtomicU64::new(0),
            readiness_ready: AtomicU64::new(0),
            readiness_unready: AtomicU64::new(0),
            rejected_tenant_cell_move_validations: AtomicU64::new(0),
            rejected_tenant_cell_move_cutovers: AtomicU64::new(0),
            rejected_tenant_cell_move_rollbacks: AtomicU64::new(0),
        }
    }
}

impl HttpMetrics {
    pub(crate) fn record_tenant_cell_move_command_rejection(
        &self,
        command: TenantCellMoveCommandMetric,
    ) {
        let counter = match command {
            TenantCellMoveCommandMetric::Validate => &self.rejected_tenant_cell_move_validations,
            TenantCellMoveCommandMetric::Cutover => &self.rejected_tenant_cell_move_cutovers,
            TenantCellMoveCommandMetric::Rollback => &self.rejected_tenant_cell_move_rollbacks,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    pub fn record_readiness(&self, ready: bool) {
        let counter = if ready {
            &self.readiness_ready
        } else {
            &self.readiness_unready
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn record_response(&self, status: StatusCode, elapsed_micros: u64) {
        self.requests_total.fetch_add(1, Ordering::Relaxed);
        self.request_duration_micros
            .fetch_add(elapsed_micros, Ordering::Relaxed);
        let counter = match status.as_u16() / 100 {
            1 => &self.status_1xx,
            2 => &self.status_2xx,
            3 => &self.status_3xx,
            4 => &self.status_4xx,
            _ => &self.status_5xx,
        };
        counter.fetch_add(1, Ordering::Relaxed);
    }

    fn render(&self, db: &Db) -> String {
        let mut output = String::with_capacity(2_048);
        let request_count = self.requests_total.load(Ordering::Relaxed);
        let duration_seconds =
            self.request_duration_micros.load(Ordering::Relaxed) as f64 / 1_000_000.0;

        output.push_str("# HELP wareboxes_build_info Static build information.\n");
        output.push_str("# TYPE wareboxes_build_info gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_build_info{{version=\"{}\"}} 1",
            env!("CARGO_PKG_VERSION")
        );
        output.push_str("# HELP wareboxes_process_uptime_seconds Process uptime in seconds.\n");
        output.push_str("# TYPE wareboxes_process_uptime_seconds gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_process_uptime_seconds {:.3}",
            self.started_at.elapsed().as_secs_f64()
        );
        output.push_str("# HELP wareboxes_http_requests_total Completed HTTP requests.\n");
        output.push_str("# TYPE wareboxes_http_requests_total counter\n");
        for (class, value) in [
            ("1xx", self.status_1xx.load(Ordering::Relaxed)),
            ("2xx", self.status_2xx.load(Ordering::Relaxed)),
            ("3xx", self.status_3xx.load(Ordering::Relaxed)),
            ("4xx", self.status_4xx.load(Ordering::Relaxed)),
            ("5xx", self.status_5xx.load(Ordering::Relaxed)),
        ] {
            let _ = writeln!(
                output,
                "wareboxes_http_requests_total{{status_class=\"{class}\"}} {value}"
            );
        }
        output.push_str(
            "# HELP wareboxes_http_requests_in_flight HTTP requests currently executing.\n",
        );
        output.push_str("# TYPE wareboxes_http_requests_in_flight gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_http_requests_in_flight {}",
            self.requests_in_flight.load(Ordering::Relaxed)
        );
        output.push_str(
            "# HELP wareboxes_http_request_duration_seconds Total HTTP request duration.\n",
        );
        output.push_str("# TYPE wareboxes_http_request_duration_seconds summary\n");
        let _ = writeln!(
            output,
            "wareboxes_http_request_duration_seconds_sum {duration_seconds:.6}"
        );
        let _ = writeln!(
            output,
            "wareboxes_http_request_duration_seconds_count {request_count}"
        );
        output.push_str("# HELP wareboxes_readiness_checks_total Readiness checks by result.\n");
        output.push_str("# TYPE wareboxes_readiness_checks_total counter\n");
        let _ = writeln!(
            output,
            "wareboxes_readiness_checks_total{{result=\"ready\"}} {}",
            self.readiness_ready.load(Ordering::Relaxed)
        );
        let _ = writeln!(
            output,
            "wareboxes_readiness_checks_total{{result=\"unready\"}} {}",
            self.readiness_unready.load(Ordering::Relaxed)
        );
        output.push_str(
            "# HELP wareboxes_tenant_cell_move_command_rejections_total Authorized tenant-cell-move command attempts that returned an error.\n",
        );
        output.push_str("# TYPE wareboxes_tenant_cell_move_command_rejections_total counter\n");
        for (command, value) in [
            (
                TenantCellMoveCommandMetric::Validate,
                self.rejected_tenant_cell_move_validations
                    .load(Ordering::Relaxed),
            ),
            (
                TenantCellMoveCommandMetric::Cutover,
                self.rejected_tenant_cell_move_cutovers
                    .load(Ordering::Relaxed),
            ),
            (
                TenantCellMoveCommandMetric::Rollback,
                self.rejected_tenant_cell_move_rollbacks
                    .load(Ordering::Relaxed),
            ),
        ] {
            let _ = writeln!(
                output,
                "wareboxes_tenant_cell_move_command_rejections_total{{command=\"{}\"}} {value}",
                command.label()
            );
        }
        output
            .push_str("# HELP wareboxes_database_pool_connections PostgreSQL pool connections.\n");
        output.push_str("# TYPE wareboxes_database_pool_connections gauge\n");
        let _ = writeln!(
            output,
            "wareboxes_database_pool_connections{{state=\"open\"}} {}",
            db.size()
        );
        let _ = writeln!(
            output,
            "wareboxes_database_pool_connections{{state=\"idle\"}} {}",
            db.num_idle()
        );
        output
    }
}

struct InFlightGuard(Arc<HttpMetrics>);

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        self.0.requests_in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}

pub async fn observe_request(
    State(metrics): State<Arc<HttpMetrics>>,
    request: Request,
    next: Next,
) -> Response {
    metrics.requests_in_flight.fetch_add(1, Ordering::Relaxed);
    let in_flight = InFlightGuard(metrics.clone());
    let started_at = Instant::now();
    let response = next.run(request).await;
    let elapsed_micros = u64::try_from(started_at.elapsed().as_micros()).unwrap_or(u64::MAX);
    metrics.record_response(response.status(), elapsed_micros);
    drop(in_flight);
    response
}

pub async fn metrics(State(state): State<crate::state::AppState>) -> Response {
    let tenant_cell_move_metrics = match tokio::time::timeout(
        TENANT_CELL_MOVE_METRICS_TIMEOUT,
        TenantCellMoveMetricsSnapshot::collect(&state.db),
    )
    .await
    {
        Ok(Ok(snapshot)) => Some(snapshot),
        Ok(Err(error)) => {
            tracing::warn!(%error, "tenant-cell move metrics collection failed");
            None
        }
        Err(_) => {
            tracing::warn!(
                timeout_seconds = TENANT_CELL_MOVE_METRICS_TIMEOUT.as_secs(),
                "tenant-cell move metrics collection timed out"
            );
            None
        }
    };
    let mut body = state.metrics.render(&state.db);
    render_tenant_cell_move_metrics(&mut body, tenant_cell_move_metrics.as_ref());
    let mut response = Response::new(Body::from(body));
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; version=0.0.4; charset=utf-8"),
    );
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}
