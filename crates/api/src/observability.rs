use std::fmt::Write;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use axum::body::Body;
use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::Response;

use crate::db::Db;

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
        }
    }
}

impl HttpMetrics {
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
    let body = state.metrics.render(&state.db);
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
