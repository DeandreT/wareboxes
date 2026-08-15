use std::sync::Arc;

use crate::config::SecurityConfig;
use crate::db::Db;
use crate::observability::HttpMetrics;
use crate::traffic::TrafficGate;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub security: SecurityConfig,
    pub metrics: Arc<HttpMetrics>,
    pub traffic: Arc<TrafficGate>,
}

impl AppState {
    pub fn new(db: Db) -> Self {
        let security = SecurityConfig::default();
        Self {
            db,
            metrics: Arc::new(HttpMetrics::default()),
            traffic: Arc::new(TrafficGate::new(&security)),
            security,
        }
    }

    pub fn with_security(db: Db, security: SecurityConfig) -> Self {
        let traffic = Arc::new(TrafficGate::new(&security));
        Self {
            db,
            security,
            metrics: Arc::new(HttpMetrics::default()),
            traffic,
        }
    }
}
