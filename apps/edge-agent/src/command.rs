use std::num::{NonZeroU16, NonZeroU32};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::types::{
    CommandId, CorrelationId, DeviceClass, DeviceId, FacilityId, IdempotencyKey, TenantId,
};

pub const COMMAND_SCHEMA_VERSION: u16 = 1;
const MAX_REFERENCE_LENGTH: usize = 200;
const MAX_PRINT_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum CommandError {
    #[error("edge command schema version {0} is unsupported")]
    UnsupportedSchema(u16),
    #[error("{field} must contain between 1 and {max} characters")]
    InvalidText { field: &'static str, max: usize },
    #[error("printer content cannot exceed {MAX_PRINT_PAYLOAD_BYTES} bytes")]
    PrintPayloadTooLarge,
    #[error("scale timeout must be between 1 and 120,000 milliseconds")]
    InvalidScaleTimeout,
    #[error("PLC pulse duration must not exceed 60,000 milliseconds")]
    InvalidPulseDuration,
    #[error("command result kind does not match command kind")]
    ResultKindMismatch,
    #[error("command serialization failed: {0}")]
    Serialization(String),
}

fn validate_text(value: &str, field: &'static str, max: usize) -> Result<(), CommandError> {
    if value.trim().is_empty() || value.len() > max {
        Err(CommandError::InvalidText { field, max })
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryPolicy {
    /// Re-delivery is allowed only when the adapter reports durable device-side
    /// duplicate protection for the stable command ID and correlation ID.
    DeviceDeduplicatedReplay,
    /// The adapter must query the device/vendor controller before a replay.
    ProbeThenRetry,
    /// Any ambiguous outcome is quarantined for explicit operator resolution.
    ManualReview,
}

impl RecoveryPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeviceDeduplicatedReplay => "device_deduplicated_replay",
            Self::ProbeThenRetry => "probe_then_retry",
            Self::ManualReview => "manual_review",
        }
    }

    pub(crate) fn parse_storage(value: &str) -> Result<Self, CommandError> {
        match value {
            "device_deduplicated_replay" => Ok(Self::DeviceDeduplicatedReplay),
            "probe_then_retry" => Ok(Self::ProbeThenRetry),
            "manual_review" => Ok(Self::ManualReview),
            _ => Err(CommandError::InvalidText {
                field: "recovery policy",
                max: 64,
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum PlcCommand {
    SetDiscreteOutput {
        point: String,
        value: bool,
    },
    PulseDiscreteOutput {
        point: String,
        duration_ms: NonZeroU32,
    },
    ResetFault {
        fault_code: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ConveyorCommand {
    RouteCarrier {
        carrier_id: String,
        destination: String,
    },
    StartZone {
        zone: String,
    },
    StopZone {
        zone: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RobotMissionKind {
    Pick,
    Place,
    Transport,
    Charge,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum RoboticsCommand {
    DispatchMission {
        mission_id: String,
        mission_kind: RobotMissionKind,
        source: String,
        destination: String,
        payload_id: Option<String>,
    },
    CancelMission {
        mission_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum SortationCommand {
    Divert {
        tracking_id: String,
        chute: String,
    },
    Reject {
        tracking_id: String,
        lane: String,
        reason_code: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PrintFormat {
    Zpl,
    Pdf,
    Png,
    Html,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum PrinterCommand {
    PrintDocument {
        document_id: String,
        format: PrintFormat,
        /// ZPL/HTML text or base64-encoded binary content for PDF/PNG documents.
        content: String,
        copies: NonZeroU16,
    },
    CancelPrintJob {
        spool_job_id: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WeightUnit {
    Gram,
    Kilogram,
    Pound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum ScaleCommand {
    ReadStableWeight {
        requested_unit: WeightUnit,
        timeout_ms: NonZeroU32,
    },
    Tare,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "device_class", content = "command", rename_all = "snake_case")]
pub enum DeviceCommand {
    Plc(PlcCommand),
    Conveyor(ConveyorCommand),
    Robotics(RoboticsCommand),
    Sortation(SortationCommand),
    Printer(PrinterCommand),
    Scale(ScaleCommand),
}

impl DeviceCommand {
    pub const fn device_class(&self) -> DeviceClass {
        match self {
            Self::Plc(_) => DeviceClass::Plc,
            Self::Conveyor(_) => DeviceClass::Conveyor,
            Self::Robotics(_) => DeviceClass::Robotics,
            Self::Sortation(_) => DeviceClass::Sortation,
            Self::Printer(_) => DeviceClass::Printer,
            Self::Scale(_) => DeviceClass::Scale,
        }
    }

    pub fn validate(&self) -> Result<(), CommandError> {
        match self {
            Self::Plc(command) => match command {
                PlcCommand::SetDiscreteOutput { point, .. } => {
                    validate_text(point, "PLC point", MAX_REFERENCE_LENGTH)
                }
                PlcCommand::PulseDiscreteOutput { point, duration_ms } => {
                    validate_text(point, "PLC point", MAX_REFERENCE_LENGTH)?;
                    if duration_ms.get() > 60_000 {
                        return Err(CommandError::InvalidPulseDuration);
                    }
                    Ok(())
                }
                PlcCommand::ResetFault { fault_code } => {
                    validate_text(fault_code, "PLC fault code", MAX_REFERENCE_LENGTH)
                }
            },
            Self::Conveyor(command) => match command {
                ConveyorCommand::RouteCarrier {
                    carrier_id,
                    destination,
                } => {
                    validate_text(carrier_id, "carrier ID", MAX_REFERENCE_LENGTH)?;
                    validate_text(destination, "conveyor destination", MAX_REFERENCE_LENGTH)
                }
                ConveyorCommand::StartZone { zone } | ConveyorCommand::StopZone { zone } => {
                    validate_text(zone, "conveyor zone", MAX_REFERENCE_LENGTH)
                }
            },
            Self::Robotics(command) => match command {
                RoboticsCommand::DispatchMission {
                    mission_id,
                    source,
                    destination,
                    payload_id,
                    ..
                } => {
                    validate_text(mission_id, "robot mission ID", MAX_REFERENCE_LENGTH)?;
                    validate_text(source, "robot mission source", MAX_REFERENCE_LENGTH)?;
                    validate_text(
                        destination,
                        "robot mission destination",
                        MAX_REFERENCE_LENGTH,
                    )?;
                    if let Some(payload_id) = payload_id {
                        validate_text(payload_id, "robot payload ID", MAX_REFERENCE_LENGTH)?;
                    }
                    Ok(())
                }
                RoboticsCommand::CancelMission { mission_id } => {
                    validate_text(mission_id, "robot mission ID", MAX_REFERENCE_LENGTH)
                }
            },
            Self::Sortation(command) => match command {
                SortationCommand::Divert { tracking_id, chute } => {
                    validate_text(tracking_id, "sortation tracking ID", MAX_REFERENCE_LENGTH)?;
                    validate_text(chute, "sortation chute", MAX_REFERENCE_LENGTH)
                }
                SortationCommand::Reject {
                    tracking_id,
                    lane,
                    reason_code,
                } => {
                    validate_text(tracking_id, "sortation tracking ID", MAX_REFERENCE_LENGTH)?;
                    validate_text(lane, "sortation lane", MAX_REFERENCE_LENGTH)?;
                    validate_text(reason_code, "sortation reason code", MAX_REFERENCE_LENGTH)
                }
            },
            Self::Printer(command) => match command {
                PrinterCommand::PrintDocument {
                    document_id,
                    content,
                    ..
                } => {
                    validate_text(document_id, "print document ID", MAX_REFERENCE_LENGTH)?;
                    if content.is_empty() {
                        return Err(CommandError::InvalidText {
                            field: "print content",
                            max: MAX_PRINT_PAYLOAD_BYTES,
                        });
                    }
                    if content.len() > MAX_PRINT_PAYLOAD_BYTES {
                        return Err(CommandError::PrintPayloadTooLarge);
                    }
                    Ok(())
                }
                PrinterCommand::CancelPrintJob { spool_job_id } => {
                    validate_text(spool_job_id, "print spool job ID", MAX_REFERENCE_LENGTH)
                }
            },
            Self::Scale(command) => match command {
                ScaleCommand::ReadStableWeight { timeout_ms, .. } => {
                    if timeout_ms.get() > 120_000 {
                        Err(CommandError::InvalidScaleTimeout)
                    } else {
                        Ok(())
                    }
                }
                ScaleCommand::Tare => Ok(()),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRequest {
    pub schema_version: u16,
    pub command_id: CommandId,
    pub tenant_id: TenantId,
    pub facility_id: FacilityId,
    pub device_id: DeviceId,
    pub correlation_id: CorrelationId,
    pub idempotency_key: IdempotencyKey,
    pub recovery_policy: RecoveryPolicy,
    pub command: DeviceCommand,
}

impl CommandRequest {
    pub fn validate(&self) -> Result<(), CommandError> {
        if self.schema_version != COMMAND_SCHEMA_VERSION {
            return Err(CommandError::UnsupportedSchema(self.schema_version));
        }
        self.command.validate()
    }

    pub fn request_hash(&self) -> Result<[u8; 32], CommandError> {
        self.validate()?;
        let encoded = serde_json::to_vec(self)
            .map_err(|error| CommandError::Serialization(error.to_string()))?;
        Ok(Sha256::digest(encoded).into())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandEnvelope {
    pub request: CommandRequest,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlcResult {
    pub controller_reference: Option<String>,
    pub output_state: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConveyorResult {
    pub controller_reference: Option<String>,
    pub observed_zone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoboticsResult {
    pub controller_reference: String,
    pub mission_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SortationResult {
    pub controller_reference: Option<String>,
    pub observed_lane: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrinterResult {
    pub spool_job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScaleResult {
    /// Canonical mass used for audit and reconciliation, independent of display
    /// unit. Negative readings are allowed for calibration and tare diagnostics.
    pub mass_milligrams: i64,
    pub stable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "device_class", content = "result", rename_all = "snake_case")]
pub enum CommandResult {
    Plc(PlcResult),
    Conveyor(ConveyorResult),
    Robotics(RoboticsResult),
    Sortation(SortationResult),
    Printer(PrinterResult),
    Scale(ScaleResult),
}

impl CommandResult {
    pub const fn device_class(&self) -> DeviceClass {
        match self {
            Self::Plc(_) => DeviceClass::Plc,
            Self::Conveyor(_) => DeviceClass::Conveyor,
            Self::Robotics(_) => DeviceClass::Robotics,
            Self::Sortation(_) => DeviceClass::Sortation,
            Self::Printer(_) => DeviceClass::Printer,
            Self::Scale(_) => DeviceClass::Scale,
        }
    }

    pub fn validate_for(&self, command: &DeviceCommand) -> Result<(), CommandError> {
        if self.device_class() != command.device_class() {
            return Err(CommandError::ResultKindMismatch);
        }
        match self {
            Self::Plc(result) => {
                if let Some(reference) = &result.controller_reference {
                    validate_text(reference, "PLC controller reference", MAX_REFERENCE_LENGTH)?;
                }
            }
            Self::Conveyor(result) => {
                if let Some(reference) = &result.controller_reference {
                    validate_text(
                        reference,
                        "conveyor controller reference",
                        MAX_REFERENCE_LENGTH,
                    )?;
                }
                if let Some(zone) = &result.observed_zone {
                    validate_text(zone, "observed conveyor zone", MAX_REFERENCE_LENGTH)?;
                }
            }
            Self::Robotics(result) => {
                validate_text(
                    &result.controller_reference,
                    "robot controller reference",
                    MAX_REFERENCE_LENGTH,
                )?;
                validate_text(
                    &result.mission_state,
                    "robot mission state",
                    MAX_REFERENCE_LENGTH,
                )?;
            }
            Self::Sortation(result) => {
                if let Some(reference) = &result.controller_reference {
                    validate_text(
                        reference,
                        "sortation controller reference",
                        MAX_REFERENCE_LENGTH,
                    )?;
                }
                validate_text(
                    &result.observed_lane,
                    "observed sortation lane",
                    MAX_REFERENCE_LENGTH,
                )?;
            }
            Self::Printer(result) => validate_text(
                &result.spool_job_id,
                "print spool job ID",
                MAX_REFERENCE_LENGTH,
            )?,
            Self::Scale(_) => {}
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandState {
    Queued,
    Executing,
    RetryWait,
    RecoveryWait,
    Succeeded,
    Failed,
    ManualReview,
    ResolvedManually,
    Cancelled,
}

impl CommandState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Executing => "executing",
            Self::RetryWait => "retry_wait",
            Self::RecoveryWait => "recovery_wait",
            Self::Succeeded => "succeeded",
            Self::Failed => "failed",
            Self::ManualReview => "manual_review",
            Self::ResolvedManually => "resolved_manually",
            Self::Cancelled => "cancelled",
        }
    }

    pub(crate) fn parse_storage(value: &str) -> Result<Self, CommandError> {
        match value {
            "queued" => Ok(Self::Queued),
            "executing" => Ok(Self::Executing),
            "retry_wait" => Ok(Self::RetryWait),
            "recovery_wait" => Ok(Self::RecoveryWait),
            "succeeded" => Ok(Self::Succeeded),
            "failed" => Ok(Self::Failed),
            "manual_review" => Ok(Self::ManualReview),
            "resolved_manually" => Ok(Self::ResolvedManually),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(CommandError::InvalidText {
                field: "command state",
                max: 64,
            }),
        }
    }

    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Succeeded | Self::Failed | Self::ResolvedManually | Self::Cancelled
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRecord {
    pub request: CommandRequest,
    pub state: CommandState,
    pub attempt_count: u32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub next_attempt_at: DateTime<Utc>,
    pub result: Option<CommandResult>,
    pub last_error: Option<String>,
    pub resolution_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubmissionOutcome {
    Accepted(CommandRecord),
    Replayed(CommandRecord),
}

impl SubmissionOutcome {
    pub fn record(&self) -> &CommandRecord {
        match self {
            Self::Accepted(record) | Self::Replayed(record) => record,
        }
    }

    pub const fn is_replay(&self) -> bool {
        matches!(self, Self::Replayed(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_hash_is_stable_and_covers_payload() {
        let request = CommandRequest {
            schema_version: COMMAND_SCHEMA_VERSION,
            command_id: CommandId::new("command-1").unwrap(),
            tenant_id: TenantId::new("tenant-1").unwrap(),
            facility_id: FacilityId::new("facility-1").unwrap(),
            device_id: DeviceId::new("scale-1").unwrap(),
            correlation_id: CorrelationId::new("correlation-1").unwrap(),
            idempotency_key: IdempotencyKey::new("read-1").unwrap(),
            recovery_policy: RecoveryPolicy::ManualReview,
            command: DeviceCommand::Scale(ScaleCommand::Tare),
        };
        assert_eq!(
            request.request_hash().unwrap(),
            request.request_hash().unwrap()
        );

        let mut changed = request.clone();
        changed.command = DeviceCommand::Scale(ScaleCommand::ReadStableWeight {
            requested_unit: WeightUnit::Gram,
            timeout_ms: NonZeroU32::new(5_000).unwrap(),
        });
        assert_ne!(
            request.request_hash().unwrap(),
            changed.request_hash().unwrap()
        );
    }

    #[test]
    fn result_class_must_match_command_class() {
        let command = DeviceCommand::Scale(ScaleCommand::Tare);
        let result = CommandResult::Printer(PrinterResult {
            spool_job_id: "job-1".into(),
        });
        assert_eq!(
            result.validate_for(&command),
            Err(CommandError::ResultKindMismatch)
        );
    }

    #[test]
    fn each_vendor_neutral_command_has_an_explicit_device_class() {
        let commands = [
            DeviceCommand::Plc(PlcCommand::SetDiscreteOutput {
                point: "ready".into(),
                value: true,
            }),
            DeviceCommand::Conveyor(ConveyorCommand::StopZone {
                zone: "zone-1".into(),
            }),
            DeviceCommand::Robotics(RoboticsCommand::CancelMission {
                mission_id: "mission-1".into(),
            }),
            DeviceCommand::Sortation(SortationCommand::Divert {
                tracking_id: "carton-1".into(),
                chute: "chute-1".into(),
            }),
            DeviceCommand::Printer(PrinterCommand::CancelPrintJob {
                spool_job_id: "job-1".into(),
            }),
            DeviceCommand::Scale(ScaleCommand::Tare),
        ];
        assert_eq!(
            commands
                .iter()
                .map(DeviceCommand::device_class)
                .collect::<Vec<_>>(),
            vec![
                DeviceClass::Plc,
                DeviceClass::Conveyor,
                DeviceClass::Robotics,
                DeviceClass::Sortation,
                DeviceClass::Printer,
                DeviceClass::Scale,
            ]
        );
        assert!(commands.iter().all(|command| command.validate().is_ok()));
    }
}
