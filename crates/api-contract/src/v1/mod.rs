//! Version 1 public API primitives.

mod cursor;
mod error;
mod expected_receiving;
mod idempotency;
mod inventory;
mod license_plate_putaway;
mod putaway;
mod putaway_claim;
mod revision;

pub use cursor::{
    CursorPage, CursorPageRequest, OpaqueCursor, OpaqueCursorError, PageLimit, PageLimitError,
    DEFAULT_PAGE_LIMIT, MAX_CURSOR_LENGTH, MAX_PAGE_LIMIT,
};
pub use error::{ErrorReason, ErrorResponse, FieldViolation};
pub use expected_receiving::{
    ExpectedReceiptLine, ExpectedReceivingLoadStatus, ExpectedReceivingLocation,
    ExpectedReceivingSessionResponse,
};
pub use idempotency::{
    IdempotencyKey, IdempotencyKeyError, IDEMPOTENCY_KEY_HEADER, MAX_IDEMPOTENCY_KEY_LENGTH,
};
pub use inventory::{
    InventoryBalancePage, InventoryBalancePageRequest, InventoryBalanceResponse,
    InventoryBalanceStatus, InventoryQuantity,
};
pub use license_plate_putaway::{
    ConfirmLicensePlatePutawayRequest, CreateLicensePlatePutawayTaskRequest,
    CreateLicensePlatePutawayTaskResponse, LicensePlatePutawayConfirmationResponse,
};
pub use putaway::{
    ConfirmPutawayRequest, CreatePutawayTaskRequest, CreatePutawayTaskResponse,
    PutawayConfirmationResponse,
};
pub use putaway_claim::{
    ClaimNextPutawayRequest, ClaimPutawayByIdRequest, HeartbeatPutawayClaimRequest,
    PutawayClaimDestinationLocation, PutawayClaimHeartbeatResponse, PutawayClaimReleaseReason,
    PutawayClaimReleaseResponse, PutawayClaimResponse, PutawayClaimSourceLocation,
    PutawayClaimWork, PutawayWorkflow, ReleasePutawayClaimRequest,
};
pub use revision::{Revision, RevisionError, RevisionPrecondition};

/// URL prefix for the version 1 public API.
pub const API_PREFIX: &str = "/api/v1";
