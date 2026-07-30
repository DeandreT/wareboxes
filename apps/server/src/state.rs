use crate::config::SecurityConfig;
use crate::db::Db;

#[derive(Clone)]
pub struct AppState {
    pub db: Db,
    pub security: SecurityConfig,
}

impl AppState {
    pub fn new(db: Db) -> Self {
        Self {
            db,
            security: SecurityConfig::default(),
        }
    }

    pub fn with_security(db: Db, security: SecurityConfig) -> Self {
        Self { db, security }
    }
}
