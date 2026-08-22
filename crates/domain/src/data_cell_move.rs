use std::fmt;
use std::str::FromStr;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{DataCellId, DataCellPlacementRevision};

pub const MAX_TENANT_CELL_MOVE_REASON_LENGTH: usize = 500;
pub const MAX_TENANT_CELL_MOVE_COPY_REFERENCE_LENGTH: usize = 200;
pub const MAX_TENANT_CELL_MOVE_ROUTING_REFERENCE_LENGTH: usize = 200;
pub const MAX_TENANT_CELL_MOVE_TOOL_VERSION_LENGTH: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TenantCellMoveError {
    #[error("{field} must not be blank or padded")]
    InvalidText { field: &'static str },
    #[error("{field} must not contain control characters")]
    ControlCharacter { field: &'static str },
    #[error("{field} exceeds {max} characters")]
    TooLong { field: &'static str, max: usize },
    #[error("tenant cell move revision must be a positive integer")]
    InvalidRevision,
    #[error("SHA-256 checksum must be exactly 64 lowercase hexadecimal characters")]
    InvalidSha256Checksum,
    #[error("PostgreSQL LSN must contain two hexadecimal 32-bit segments separated by '/'")]
    InvalidPostgresLsn,
    #[error("{field} must not be negative")]
    NegativeMeasurement { field: &'static str },
    #[error("copy checkpoint target replay LSN cannot be ahead of its source LSN")]
    CheckpointTargetAhead,
    #[error("final validation target replay LSN is behind its source LSN")]
    ValidationTargetBehind,
    #[error("final validation source and target row counts do not match")]
    ValidationRowCountMismatch,
    #[error("final validation {kind} checksums do not match")]
    ValidationChecksumMismatch { kind: &'static str },
    #[error("final validation did not verify {control}")]
    ValidationControlMissing { control: &'static str },
    #[error("tenant cell move cannot transition from {from} to {to}")]
    InvalidTransition {
        from: TenantCellMoveStatus,
        to: TenantCellMoveStatus,
    },
}

fn bounded_text(
    value: String,
    field: &'static str,
    max: usize,
) -> Result<String, TenantCellMoveError> {
    if value.trim().is_empty() || value.trim() != value {
        return Err(TenantCellMoveError::InvalidText { field });
    }
    if value.chars().any(char::is_control) {
        return Err(TenantCellMoveError::ControlCharacter { field });
    }
    if value.chars().count() > max {
        return Err(TenantCellMoveError::TooLong { field, max });
    }
    Ok(value)
}

macro_rules! bounded_text_value {
    ($name:ident, $field:literal, $max:expr) => {
        #[derive(Debug, Clone, PartialEq, Eq, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, TenantCellMoveError> {
                bounded_text(value.into(), $field, $max).map(Self)
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
            }
        }
    };
}

bounded_text_value!(
    TenantCellMoveReason,
    "tenant cell move reason",
    MAX_TENANT_CELL_MOVE_REASON_LENGTH
);
bounded_text_value!(
    TenantCellMoveCopyReference,
    "tenant cell move copy reference",
    MAX_TENANT_CELL_MOVE_COPY_REFERENCE_LENGTH
);
bounded_text_value!(
    TenantCellMoveRoutingReference,
    "tenant cell move routing reference",
    MAX_TENANT_CELL_MOVE_ROUTING_REFERENCE_LENGTH
);
bounded_text_value!(
    TenantCellMoveToolVersion,
    "tenant cell move validation tool version",
    MAX_TENANT_CELL_MOVE_TOOL_VERSION_LENGTH
);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha256Checksum(String);

impl Sha256Checksum {
    pub fn new(value: impl Into<String>) -> Result<Self, TenantCellMoveError> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(TenantCellMoveError::InvalidSha256Checksum);
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for Sha256Checksum {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(String::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PostgresLsn(u64);

impl PostgresLsn {
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl FromStr for PostgresLsn {
    type Err = TenantCellMoveError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((upper, lower)) = value.split_once('/') else {
            return Err(TenantCellMoveError::InvalidPostgresLsn);
        };
        if upper.is_empty()
            || lower.is_empty()
            || upper.len() > 8
            || lower.len() > 8
            || lower.contains('/')
            || !upper.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !lower.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(TenantCellMoveError::InvalidPostgresLsn);
        }
        let upper =
            u32::from_str_radix(upper, 16).map_err(|_| TenantCellMoveError::InvalidPostgresLsn)?;
        let lower =
            u32::from_str_radix(lower, 16).map_err(|_| TenantCellMoveError::InvalidPostgresLsn)?;
        Ok(Self((u64::from(upper) << 32) | u64::from(lower)))
    }
}

impl fmt::Display for PostgresLsn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:X}/{:X}",
            self.0 >> 32,
            self.0 & u64::from(u32::MAX)
        )
    }
}

impl Serialize for PostgresLsn {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for PostgresLsn {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct TenantCellMoveRevision(i64);

impl TenantCellMoveRevision {
    pub const fn new(value: i64) -> Result<Self, TenantCellMoveError> {
        if value > 0 {
            Ok(Self(value))
        } else {
            Err(TenantCellMoveError::InvalidRevision)
        }
    }

    pub const fn get(self) -> i64 {
        self.0
    }

    pub const fn checked_next(self) -> Option<Self> {
        match self.0.checked_add(1) {
            Some(value) => Some(Self(value)),
            None => None,
        }
    }
}

impl<'de> Deserialize<'de> for TenantCellMoveRevision {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::new(i64::deserialize(deserializer)?).map_err(D::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TenantCellMoveStatus {
    Planned,
    Copying,
    Frozen,
    Validated,
    CutOver,
    Completed,
    Cancelled,
    RolledBack,
}

impl TenantCellMoveStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Copying => "copying",
            Self::Frozen => "frozen",
            Self::Validated => "validated",
            Self::CutOver => "cut_over",
            Self::Completed => "completed",
            Self::Cancelled => "cancelled",
            Self::RolledBack => "rolled_back",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "planned" => Some(Self::Planned),
            "copying" => Some(Self::Copying),
            "frozen" => Some(Self::Frozen),
            "validated" => Some(Self::Validated),
            "cut_over" => Some(Self::CutOver),
            "completed" => Some(Self::Completed),
            "cancelled" => Some(Self::Cancelled),
            "rolled_back" => Some(Self::RolledBack),
            _ => None,
        }
    }

    pub const fn is_write_fenced(self) -> bool {
        matches!(self, Self::Frozen | Self::Validated | Self::CutOver)
    }

    pub fn require_transition(self, next: Self) -> Result<(), TenantCellMoveError> {
        if matches!(
            (self, next),
            (Self::Planned, Self::Copying)
                | (Self::Copying, Self::Frozen)
                | (Self::Frozen, Self::Validated)
                | (Self::Validated, Self::CutOver)
                | (Self::CutOver, Self::Completed)
                | (Self::CutOver, Self::RolledBack)
                | (
                    Self::Planned | Self::Copying | Self::Frozen | Self::Validated,
                    Self::Cancelled
                )
        ) {
            Ok(())
        } else {
            Err(TenantCellMoveError::InvalidTransition {
                from: self,
                to: next,
            })
        }
    }
}

impl fmt::Display for TenantCellMoveStatus {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantCellMoveCheckpointInput {
    pub source_lsn: PostgresLsn,
    pub target_replay_lsn: PostgresLsn,
    pub copied_row_count: i64,
    pub copied_bytes: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TenantCellMoveCheckpoint {
    source_lsn: PostgresLsn,
    target_replay_lsn: PostgresLsn,
    copied_row_count: i64,
    copied_bytes: i64,
}

impl<'de> Deserialize<'de> for TenantCellMoveCheckpoint {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            source_lsn: PostgresLsn,
            target_replay_lsn: PostgresLsn,
            copied_row_count: i64,
            copied_bytes: i64,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(TenantCellMoveCheckpointInput {
            source_lsn: raw.source_lsn,
            target_replay_lsn: raw.target_replay_lsn,
            copied_row_count: raw.copied_row_count,
            copied_bytes: raw.copied_bytes,
        })
        .map_err(D::Error::custom)
    }
}

impl TenantCellMoveCheckpoint {
    pub fn new(input: TenantCellMoveCheckpointInput) -> Result<Self, TenantCellMoveError> {
        require_nonnegative(input.copied_row_count, "copied row count")?;
        require_nonnegative(input.copied_bytes, "copied bytes")?;
        if input.target_replay_lsn > input.source_lsn {
            return Err(TenantCellMoveError::CheckpointTargetAhead);
        }
        Ok(Self {
            source_lsn: input.source_lsn,
            target_replay_lsn: input.target_replay_lsn,
            copied_row_count: input.copied_row_count,
            copied_bytes: input.copied_bytes,
        })
    }

    pub const fn source_lsn(&self) -> PostgresLsn {
        self.source_lsn
    }

    pub const fn target_replay_lsn(&self) -> PostgresLsn {
        self.target_replay_lsn
    }

    pub const fn copied_row_count(&self) -> i64 {
        self.copied_row_count
    }

    pub const fn copied_bytes(&self) -> i64 {
        self.copied_bytes
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantCellMoveValidationInput {
    pub tool_version: TenantCellMoveToolVersion,
    pub source_lsn: PostgresLsn,
    pub target_replay_lsn: PostgresLsn,
    pub source_row_count: i64,
    pub target_row_count: i64,
    pub source_data_checksum: Sha256Checksum,
    pub target_data_checksum: Sha256Checksum,
    pub source_schema_checksum: Sha256Checksum,
    pub target_schema_checksum: Sha256Checksum,
    pub source_object_manifest_checksum: Sha256Checksum,
    pub target_object_manifest_checksum: Sha256Checksum,
    pub inventory_reconciled: bool,
    pub idempotency_verified: bool,
    pub outbox_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TenantCellMoveValidation {
    tool_version: TenantCellMoveToolVersion,
    source_lsn: PostgresLsn,
    target_replay_lsn: PostgresLsn,
    source_row_count: i64,
    target_row_count: i64,
    source_data_checksum: Sha256Checksum,
    target_data_checksum: Sha256Checksum,
    source_schema_checksum: Sha256Checksum,
    target_schema_checksum: Sha256Checksum,
    source_object_manifest_checksum: Sha256Checksum,
    target_object_manifest_checksum: Sha256Checksum,
    inventory_reconciled: bool,
    idempotency_verified: bool,
    outbox_verified: bool,
}

impl<'de> Deserialize<'de> for TenantCellMoveValidation {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            tool_version: TenantCellMoveToolVersion,
            source_lsn: PostgresLsn,
            target_replay_lsn: PostgresLsn,
            source_row_count: i64,
            target_row_count: i64,
            source_data_checksum: Sha256Checksum,
            target_data_checksum: Sha256Checksum,
            source_schema_checksum: Sha256Checksum,
            target_schema_checksum: Sha256Checksum,
            source_object_manifest_checksum: Sha256Checksum,
            target_object_manifest_checksum: Sha256Checksum,
            inventory_reconciled: bool,
            idempotency_verified: bool,
            outbox_verified: bool,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(TenantCellMoveValidationInput {
            tool_version: raw.tool_version,
            source_lsn: raw.source_lsn,
            target_replay_lsn: raw.target_replay_lsn,
            source_row_count: raw.source_row_count,
            target_row_count: raw.target_row_count,
            source_data_checksum: raw.source_data_checksum,
            target_data_checksum: raw.target_data_checksum,
            source_schema_checksum: raw.source_schema_checksum,
            target_schema_checksum: raw.target_schema_checksum,
            source_object_manifest_checksum: raw.source_object_manifest_checksum,
            target_object_manifest_checksum: raw.target_object_manifest_checksum,
            inventory_reconciled: raw.inventory_reconciled,
            idempotency_verified: raw.idempotency_verified,
            outbox_verified: raw.outbox_verified,
        })
        .map_err(D::Error::custom)
    }
}

impl TenantCellMoveValidation {
    pub fn new(input: TenantCellMoveValidationInput) -> Result<Self, TenantCellMoveError> {
        require_nonnegative(input.source_row_count, "source row count")?;
        require_nonnegative(input.target_row_count, "target row count")?;
        if input.target_replay_lsn < input.source_lsn {
            return Err(TenantCellMoveError::ValidationTargetBehind);
        }
        if input.source_row_count != input.target_row_count {
            return Err(TenantCellMoveError::ValidationRowCountMismatch);
        }
        require_equal_checksum(
            &input.source_data_checksum,
            &input.target_data_checksum,
            "data",
        )?;
        require_equal_checksum(
            &input.source_schema_checksum,
            &input.target_schema_checksum,
            "schema",
        )?;
        require_equal_checksum(
            &input.source_object_manifest_checksum,
            &input.target_object_manifest_checksum,
            "object-manifest",
        )?;
        require_control(input.inventory_reconciled, "inventory reconciliation")?;
        require_control(input.idempotency_verified, "idempotency")?;
        require_control(input.outbox_verified, "outbox")?;
        Ok(Self {
            tool_version: input.tool_version,
            source_lsn: input.source_lsn,
            target_replay_lsn: input.target_replay_lsn,
            source_row_count: input.source_row_count,
            target_row_count: input.target_row_count,
            source_data_checksum: input.source_data_checksum,
            target_data_checksum: input.target_data_checksum,
            source_schema_checksum: input.source_schema_checksum,
            target_schema_checksum: input.target_schema_checksum,
            source_object_manifest_checksum: input.source_object_manifest_checksum,
            target_object_manifest_checksum: input.target_object_manifest_checksum,
            inventory_reconciled: input.inventory_reconciled,
            idempotency_verified: input.idempotency_verified,
            outbox_verified: input.outbox_verified,
        })
    }

    pub fn tool_version(&self) -> &TenantCellMoveToolVersion {
        &self.tool_version
    }

    pub const fn source_lsn(&self) -> PostgresLsn {
        self.source_lsn
    }

    pub const fn target_replay_lsn(&self) -> PostgresLsn {
        self.target_replay_lsn
    }

    pub const fn source_row_count(&self) -> i64 {
        self.source_row_count
    }

    pub const fn target_row_count(&self) -> i64 {
        self.target_row_count
    }

    pub fn source_data_checksum(&self) -> &Sha256Checksum {
        &self.source_data_checksum
    }

    pub fn target_data_checksum(&self) -> &Sha256Checksum {
        &self.target_data_checksum
    }

    pub fn source_schema_checksum(&self) -> &Sha256Checksum {
        &self.source_schema_checksum
    }

    pub fn target_schema_checksum(&self) -> &Sha256Checksum {
        &self.target_schema_checksum
    }

    pub fn source_object_manifest_checksum(&self) -> &Sha256Checksum {
        &self.source_object_manifest_checksum
    }

    pub fn target_object_manifest_checksum(&self) -> &Sha256Checksum {
        &self.target_object_manifest_checksum
    }

    pub const fn inventory_reconciled(&self) -> bool {
        self.inventory_reconciled
    }

    pub const fn idempotency_verified(&self) -> bool {
        self.idempotency_verified
    }

    pub const fn outbox_verified(&self) -> bool {
        self.outbox_verified
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantCellMoveCutoverVerificationInput {
    pub tool_version: TenantCellMoveToolVersion,
    pub routing_reference: TenantCellMoveRoutingReference,
    pub observed_data_cell_id: DataCellId,
    pub observed_placement_revision: DataCellPlacementRevision,
    pub routing_verified: bool,
    pub target_read_verified: bool,
    pub write_fence_verified: bool,
    pub inventory_reconciled: bool,
    pub idempotency_verified: bool,
    pub outbox_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TenantCellMoveCutoverVerification {
    tool_version: TenantCellMoveToolVersion,
    routing_reference: TenantCellMoveRoutingReference,
    observed_data_cell_id: DataCellId,
    observed_placement_revision: DataCellPlacementRevision,
    routing_verified: bool,
    target_read_verified: bool,
    write_fence_verified: bool,
    inventory_reconciled: bool,
    idempotency_verified: bool,
    outbox_verified: bool,
}

impl<'de> Deserialize<'de> for TenantCellMoveCutoverVerification {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            tool_version: TenantCellMoveToolVersion,
            routing_reference: TenantCellMoveRoutingReference,
            observed_data_cell_id: DataCellId,
            observed_placement_revision: DataCellPlacementRevision,
            routing_verified: bool,
            target_read_verified: bool,
            write_fence_verified: bool,
            inventory_reconciled: bool,
            idempotency_verified: bool,
            outbox_verified: bool,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(TenantCellMoveCutoverVerificationInput {
            tool_version: raw.tool_version,
            routing_reference: raw.routing_reference,
            observed_data_cell_id: raw.observed_data_cell_id,
            observed_placement_revision: raw.observed_placement_revision,
            routing_verified: raw.routing_verified,
            target_read_verified: raw.target_read_verified,
            write_fence_verified: raw.write_fence_verified,
            inventory_reconciled: raw.inventory_reconciled,
            idempotency_verified: raw.idempotency_verified,
            outbox_verified: raw.outbox_verified,
        })
        .map_err(D::Error::custom)
    }
}

impl TenantCellMoveCutoverVerification {
    pub fn new(input: TenantCellMoveCutoverVerificationInput) -> Result<Self, TenantCellMoveError> {
        require_control(input.routing_verified, "tenant routing")?;
        require_control(input.target_read_verified, "target-cell reads")?;
        require_control(input.write_fence_verified, "tenant write fence")?;
        require_control(input.inventory_reconciled, "inventory reconciliation")?;
        require_control(input.idempotency_verified, "idempotency")?;
        require_control(input.outbox_verified, "outbox")?;
        Ok(Self {
            tool_version: input.tool_version,
            routing_reference: input.routing_reference,
            observed_data_cell_id: input.observed_data_cell_id,
            observed_placement_revision: input.observed_placement_revision,
            routing_verified: input.routing_verified,
            target_read_verified: input.target_read_verified,
            write_fence_verified: input.write_fence_verified,
            inventory_reconciled: input.inventory_reconciled,
            idempotency_verified: input.idempotency_verified,
            outbox_verified: input.outbox_verified,
        })
    }

    pub fn tool_version(&self) -> &TenantCellMoveToolVersion {
        &self.tool_version
    }

    pub fn routing_reference(&self) -> &TenantCellMoveRoutingReference {
        &self.routing_reference
    }

    pub const fn observed_data_cell_id(&self) -> DataCellId {
        self.observed_data_cell_id
    }

    pub const fn observed_placement_revision(&self) -> DataCellPlacementRevision {
        self.observed_placement_revision
    }

    pub const fn routing_verified(&self) -> bool {
        self.routing_verified
    }

    pub const fn target_read_verified(&self) -> bool {
        self.target_read_verified
    }

    pub const fn write_fence_verified(&self) -> bool {
        self.write_fence_verified
    }

    pub const fn inventory_reconciled(&self) -> bool {
        self.inventory_reconciled
    }

    pub const fn idempotency_verified(&self) -> bool {
        self.idempotency_verified
    }

    pub const fn outbox_verified(&self) -> bool {
        self.outbox_verified
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantCellMoveRollbackVerificationInput {
    pub tool_version: TenantCellMoveToolVersion,
    pub routing_reference: TenantCellMoveRoutingReference,
    pub observed_data_cell_id: DataCellId,
    pub expected_rollback_placement_revision: DataCellPlacementRevision,
    pub routing_verified: bool,
    pub source_read_verified: bool,
    pub write_fence_verified: bool,
    pub inventory_reconciled: bool,
    pub idempotency_verified: bool,
    pub outbox_verified: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TenantCellMoveRollbackVerification {
    tool_version: TenantCellMoveToolVersion,
    routing_reference: TenantCellMoveRoutingReference,
    observed_data_cell_id: DataCellId,
    expected_rollback_placement_revision: DataCellPlacementRevision,
    routing_verified: bool,
    source_read_verified: bool,
    write_fence_verified: bool,
    inventory_reconciled: bool,
    idempotency_verified: bool,
    outbox_verified: bool,
}

impl<'de> Deserialize<'de> for TenantCellMoveRollbackVerification {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Raw {
            tool_version: TenantCellMoveToolVersion,
            routing_reference: TenantCellMoveRoutingReference,
            observed_data_cell_id: DataCellId,
            expected_rollback_placement_revision: DataCellPlacementRevision,
            routing_verified: bool,
            source_read_verified: bool,
            write_fence_verified: bool,
            inventory_reconciled: bool,
            idempotency_verified: bool,
            outbox_verified: bool,
        }

        let raw = Raw::deserialize(deserializer)?;
        Self::new(TenantCellMoveRollbackVerificationInput {
            tool_version: raw.tool_version,
            routing_reference: raw.routing_reference,
            observed_data_cell_id: raw.observed_data_cell_id,
            expected_rollback_placement_revision: raw.expected_rollback_placement_revision,
            routing_verified: raw.routing_verified,
            source_read_verified: raw.source_read_verified,
            write_fence_verified: raw.write_fence_verified,
            inventory_reconciled: raw.inventory_reconciled,
            idempotency_verified: raw.idempotency_verified,
            outbox_verified: raw.outbox_verified,
        })
        .map_err(D::Error::custom)
    }
}

impl TenantCellMoveRollbackVerification {
    pub fn new(
        input: TenantCellMoveRollbackVerificationInput,
    ) -> Result<Self, TenantCellMoveError> {
        require_control(input.routing_verified, "tenant routing")?;
        require_control(input.source_read_verified, "source-cell reads")?;
        require_control(input.write_fence_verified, "tenant write fence")?;
        require_control(input.inventory_reconciled, "inventory reconciliation")?;
        require_control(input.idempotency_verified, "idempotency")?;
        require_control(input.outbox_verified, "outbox")?;
        Ok(Self {
            tool_version: input.tool_version,
            routing_reference: input.routing_reference,
            observed_data_cell_id: input.observed_data_cell_id,
            expected_rollback_placement_revision: input.expected_rollback_placement_revision,
            routing_verified: input.routing_verified,
            source_read_verified: input.source_read_verified,
            write_fence_verified: input.write_fence_verified,
            inventory_reconciled: input.inventory_reconciled,
            idempotency_verified: input.idempotency_verified,
            outbox_verified: input.outbox_verified,
        })
    }

    pub fn tool_version(&self) -> &TenantCellMoveToolVersion {
        &self.tool_version
    }

    pub fn routing_reference(&self) -> &TenantCellMoveRoutingReference {
        &self.routing_reference
    }

    pub const fn observed_data_cell_id(&self) -> DataCellId {
        self.observed_data_cell_id
    }

    pub const fn expected_rollback_placement_revision(&self) -> DataCellPlacementRevision {
        self.expected_rollback_placement_revision
    }

    pub const fn routing_verified(&self) -> bool {
        self.routing_verified
    }

    pub const fn source_read_verified(&self) -> bool {
        self.source_read_verified
    }

    pub const fn write_fence_verified(&self) -> bool {
        self.write_fence_verified
    }

    pub const fn inventory_reconciled(&self) -> bool {
        self.inventory_reconciled
    }

    pub const fn idempotency_verified(&self) -> bool {
        self.idempotency_verified
    }

    pub const fn outbox_verified(&self) -> bool {
        self.outbox_verified
    }
}

fn require_nonnegative(value: i64, field: &'static str) -> Result<(), TenantCellMoveError> {
    if value < 0 {
        Err(TenantCellMoveError::NegativeMeasurement { field })
    } else {
        Ok(())
    }
}

fn require_equal_checksum(
    source: &Sha256Checksum,
    target: &Sha256Checksum,
    kind: &'static str,
) -> Result<(), TenantCellMoveError> {
    if source == target {
        Ok(())
    } else {
        Err(TenantCellMoveError::ValidationChecksumMismatch { kind })
    }
}

fn require_control(value: bool, control: &'static str) -> Result<(), TenantCellMoveError> {
    if value {
        Ok(())
    } else {
        Err(TenantCellMoveError::ValidationControlMissing { control })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn checksum(value: char) -> Sha256Checksum {
        Sha256Checksum::new(value.to_string().repeat(64)).unwrap()
    }

    fn valid_validation() -> TenantCellMoveValidationInput {
        TenantCellMoveValidationInput {
            tool_version: TenantCellMoveToolVersion::new("cell-validator/1.2.3").unwrap(),
            source_lsn: "16/B374D848".parse().unwrap(),
            target_replay_lsn: "16/B374D848".parse().unwrap(),
            source_row_count: 42,
            target_row_count: 42,
            source_data_checksum: checksum('a'),
            target_data_checksum: checksum('a'),
            source_schema_checksum: checksum('b'),
            target_schema_checksum: checksum('b'),
            source_object_manifest_checksum: checksum('c'),
            target_object_manifest_checksum: checksum('c'),
            inventory_reconciled: true,
            idempotency_verified: true,
            outbox_verified: true,
        }
    }

    #[test]
    fn move_lifecycle_has_one_safe_cutover_path() {
        let path = [
            TenantCellMoveStatus::Planned,
            TenantCellMoveStatus::Copying,
            TenantCellMoveStatus::Frozen,
            TenantCellMoveStatus::Validated,
            TenantCellMoveStatus::CutOver,
            TenantCellMoveStatus::Completed,
        ];
        for pair in path.windows(2) {
            assert!(pair[0].require_transition(pair[1]).is_ok());
        }
        for status in [
            TenantCellMoveStatus::Planned,
            TenantCellMoveStatus::Copying,
            TenantCellMoveStatus::Frozen,
            TenantCellMoveStatus::Validated,
        ] {
            assert!(status
                .require_transition(TenantCellMoveStatus::Cancelled)
                .is_ok());
        }
        assert!(TenantCellMoveStatus::CutOver
            .require_transition(TenantCellMoveStatus::RolledBack)
            .is_ok());
        assert!(TenantCellMoveStatus::Completed
            .require_transition(TenantCellMoveStatus::RolledBack)
            .is_err());
        assert!(TenantCellMoveStatus::Planned
            .require_transition(TenantCellMoveStatus::CutOver)
            .is_err());
        assert!(!TenantCellMoveStatus::Copying.is_write_fenced());
        assert!(TenantCellMoveStatus::Frozen.is_write_fenced());
        assert!(TenantCellMoveStatus::Validated.is_write_fenced());
        assert!(TenantCellMoveStatus::CutOver.is_write_fenced());
        assert!(!TenantCellMoveStatus::Completed.is_write_fenced());
    }

    #[test]
    fn evidence_value_objects_are_exact_and_bounded() {
        assert!(TenantCellMoveReason::new("regional evacuation INC-42").is_ok());
        assert!(TenantCellMoveReason::new(" padded ").is_err());
        assert!(TenantCellMoveCopyReference::new("x".repeat(201)).is_err());
        assert!(TenantCellMoveToolVersion::new("validator\n1").is_err());
        assert!(Sha256Checksum::new("a".repeat(64)).is_ok());
        assert!(Sha256Checksum::new("A".repeat(64)).is_err());
        assert!(Sha256Checksum::new("a".repeat(63)).is_err());
        assert!(
            serde_json::from_str::<Sha256Checksum>(&format!("\"{}\"", "A".repeat(64))).is_err()
        );
    }

    #[test]
    fn postgres_lsn_is_parsed_ordered_and_canonicalized() {
        let first: PostgresLsn = "0/16b6c50".parse().unwrap();
        let second: PostgresLsn = "1/0".parse().unwrap();
        assert!(first < second);
        assert_eq!(first.to_string(), "0/16B6C50");
        assert_eq!(serde_json::to_string(&first).unwrap(), "\"0/16B6C50\"");
        assert_eq!(
            serde_json::from_str::<PostgresLsn>("\"0/16b6c50\"").unwrap(),
            first
        );
        for invalid in ["", "0", "/1", "1/", "1/2/3", "100000000/0", "G/0"] {
            assert!(invalid.parse::<PostgresLsn>().is_err(), "{invalid}");
        }
    }

    #[test]
    fn progress_checkpoint_must_not_claim_future_replay() {
        let base = TenantCellMoveCheckpointInput {
            source_lsn: "2/0".parse().unwrap(),
            target_replay_lsn: "1/FFFFFFFF".parse().unwrap(),
            copied_row_count: 90,
            copied_bytes: 4096,
        };
        assert!(TenantCellMoveCheckpoint::new(base.clone()).is_ok());
        let equal = TenantCellMoveCheckpointInput {
            target_replay_lsn: base.source_lsn,
            ..base.clone()
        };
        assert!(TenantCellMoveCheckpoint::new(equal).is_ok());
        let ahead = TenantCellMoveCheckpointInput {
            target_replay_lsn: "2/1".parse().unwrap(),
            ..base.clone()
        };
        assert_eq!(
            TenantCellMoveCheckpoint::new(ahead),
            Err(TenantCellMoveError::CheckpointTargetAhead)
        );
        let negative = TenantCellMoveCheckpointInput {
            copied_bytes: -1,
            ..base
        };
        assert!(matches!(
            TenantCellMoveCheckpoint::new(negative),
            Err(TenantCellMoveError::NegativeMeasurement { .. })
        ));
        assert!(serde_json::from_str::<TenantCellMoveCheckpoint>(
            r#"{"source_lsn":"2/0","target_replay_lsn":"1/0","copied_row_count":0,"copied_bytes":-1}"#
        )
        .is_err());
    }

    #[test]
    fn final_validation_requires_complete_parity_and_controls() {
        assert!(TenantCellMoveValidation::new(valid_validation()).is_ok());
        let mut ahead = valid_validation();
        ahead.target_replay_lsn = "16/B374D849".parse().unwrap();
        assert!(TenantCellMoveValidation::new(ahead).is_ok());

        let mut behind = valid_validation();
        behind.target_replay_lsn = "16/B374D847".parse().unwrap();
        assert_eq!(
            TenantCellMoveValidation::new(behind),
            Err(TenantCellMoveError::ValidationTargetBehind)
        );

        let mut row_mismatch = valid_validation();
        row_mismatch.target_row_count = 41;
        assert_eq!(
            TenantCellMoveValidation::new(row_mismatch),
            Err(TenantCellMoveError::ValidationRowCountMismatch)
        );

        let mut checksum_mismatch = valid_validation();
        checksum_mismatch.target_object_manifest_checksum = checksum('d');
        assert_eq!(
            TenantCellMoveValidation::new(checksum_mismatch),
            Err(TenantCellMoveError::ValidationChecksumMismatch {
                kind: "object-manifest"
            })
        );

        let mut missing_control = valid_validation();
        missing_control.outbox_verified = false;
        assert_eq!(
            TenantCellMoveValidation::new(missing_control),
            Err(TenantCellMoveError::ValidationControlMissing { control: "outbox" })
        );
    }

    #[test]
    fn cutover_verification_requires_exact_routing_and_safety_proofs() {
        let input = TenantCellMoveCutoverVerificationInput {
            tool_version: TenantCellMoveToolVersion::new("cell-validator/1.2.3").unwrap(),
            routing_reference: TenantCellMoveRoutingReference::new("route-change/INC-42").unwrap(),
            observed_data_cell_id: DataCellId::new(8).unwrap(),
            observed_placement_revision: DataCellPlacementRevision::new(4).unwrap(),
            routing_verified: true,
            target_read_verified: true,
            write_fence_verified: true,
            inventory_reconciled: true,
            idempotency_verified: true,
            outbox_verified: true,
        };
        assert!(TenantCellMoveCutoverVerification::new(input.clone()).is_ok());

        let mut missing_routing = input.clone();
        missing_routing.routing_verified = false;
        assert_eq!(
            TenantCellMoveCutoverVerification::new(missing_routing),
            Err(TenantCellMoveError::ValidationControlMissing {
                control: "tenant routing"
            })
        );

        let mut missing_fence = input;
        missing_fence.write_fence_verified = false;
        assert!(TenantCellMoveCutoverVerification::new(missing_fence).is_err());
    }

    #[test]
    fn rollback_verification_requires_source_routing_and_safety_proofs() {
        let input = TenantCellMoveRollbackVerificationInput {
            tool_version: TenantCellMoveToolVersion::new("cell-validator/1.2.3").unwrap(),
            routing_reference: TenantCellMoveRoutingReference::new("route-change/INC-43").unwrap(),
            observed_data_cell_id: DataCellId::new(7).unwrap(),
            expected_rollback_placement_revision: DataCellPlacementRevision::new(5).unwrap(),
            routing_verified: true,
            source_read_verified: true,
            write_fence_verified: true,
            inventory_reconciled: true,
            idempotency_verified: true,
            outbox_verified: true,
        };
        assert!(TenantCellMoveRollbackVerification::new(input.clone()).is_ok());

        let mut missing_source_read = input.clone();
        missing_source_read.source_read_verified = false;
        assert_eq!(
            TenantCellMoveRollbackVerification::new(missing_source_read),
            Err(TenantCellMoveError::ValidationControlMissing {
                control: "source-cell reads"
            })
        );

        let mut missing_outbox = input;
        missing_outbox.outbox_verified = false;
        assert!(TenantCellMoveRollbackVerification::new(missing_outbox).is_err());
    }
}
