//! HTTP contracts used by the desktop operations application.

pub mod access;
mod error;

pub use access::{AccessScopeResource, AccessScopeWorkspace};
pub use error::{ErrorCode, ErrorResponse, FieldError};
