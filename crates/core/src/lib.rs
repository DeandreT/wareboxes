//! `wareboxes-core` — domain models, API DTOs, validation rules and the
//! error taxonomy shared between the Axum server and the egui client.
pub mod dto;
pub mod error;
pub mod models;

pub use error::{CoreError, CoreResult, FieldError};

/// Convert `validator::ValidationErrors` into our flat `FieldError` list so
/// the server and client speak the same validation language.
pub fn field_errors(errors: &validator::ValidationErrors) -> Vec<FieldError> {
    let mut out = Vec::new();
    for (field, kind) in errors.field_errors() {
        for e in kind {
            let message = e
                .message
                .as_ref()
                .map(|m| m.to_string())
                .unwrap_or_else(|| format!("Invalid {field}"));
            out.push(FieldError {
                field: field.to_string(),
                message,
            });
        }
    }
    out
}
