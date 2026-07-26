//! Version 1 public API primitives.

mod cursor;
mod error;
mod idempotency;
mod inventory;
mod putaway;
mod revision;

pub use cursor::{
    CursorPage, CursorPageRequest, OpaqueCursor, OpaqueCursorError, PageLimit, PageLimitError,
    DEFAULT_PAGE_LIMIT, MAX_CURSOR_LENGTH, MAX_PAGE_LIMIT,
};
pub use error::{ErrorReason, ErrorResponse, FieldViolation};
pub use idempotency::{
    IdempotencyKey, IdempotencyKeyError, IDEMPOTENCY_KEY_HEADER, MAX_IDEMPOTENCY_KEY_LENGTH,
};
pub use inventory::{
    InventoryBalancePage, InventoryBalancePageRequest, InventoryBalanceResponse,
    InventoryBalanceStatus, InventoryQuantity,
};
pub use putaway::{
    ConfirmPutawayRequest, CreatePutawayTaskRequest, CreatePutawayTaskResponse,
    PutawayConfirmationResponse,
};
pub use revision::{Revision, RevisionError, RevisionPrecondition};

/// URL prefix for the version 1 public API.
pub const API_PREFIX: &str = "/api/v1";
