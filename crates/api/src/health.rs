use std::time::Duration;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde::Serialize;

use crate::state::AppState;

const READINESS_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    status: &'static str,
    service: &'static str,
    version: &'static str,
    checks: HealthChecks,
}

#[derive(Debug, Serialize)]
struct HealthChecks {
    database: &'static str,
    schema: &'static str,
}

pub async fn live() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        service: "wareboxes-api",
        version: env!("CARGO_PKG_VERSION"),
        checks: HealthChecks {
            database: "not_checked",
            schema: "not_checked",
        },
    })
}

pub async fn ready(State(state): State<AppState>) -> impl IntoResponse {
    let check = tokio::time::timeout(
        READINESS_TIMEOUT,
        sqlx::query_scalar::<_, bool>(
            "SELECT to_regclass('public.tenants') IS NOT NULL
                 AND to_regclass('public.inventory_transactions') IS NOT NULL
                 AND to_regclass('public.outbox_events') IS NOT NULL",
        )
        .fetch_one(&state.db),
    )
    .await;

    let ready = matches!(check, Ok(Ok(true)));
    state.metrics.record_readiness(ready);
    if ready {
        return (
            StatusCode::OK,
            Json(HealthResponse {
                status: "ready",
                service: "wareboxes-api",
                version: env!("CARGO_PKG_VERSION"),
                checks: HealthChecks {
                    database: "ok",
                    schema: "ok",
                },
            }),
        );
    }

    match check {
        Ok(Err(error)) => tracing::error!(%error, "readiness database check failed"),
        Ok(Ok(false)) => tracing::error!("readiness schema check failed"),
        Err(_) => tracing::error!(
            timeout_seconds = READINESS_TIMEOUT.as_secs(),
            "readiness database check timed out"
        ),
        Ok(Ok(true)) => {}
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(HealthResponse {
            status: "unready",
            service: "wareboxes-api",
            version: env!("CARGO_PKG_VERSION"),
            checks: HealthChecks {
                database: "unavailable",
                schema: "unknown",
            },
        }),
    )
}
