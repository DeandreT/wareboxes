use serde::{Deserialize, Serialize};
use thiserror::Error;

pub const MAX_AUTOMATION_IDENTIFIER_LENGTH: usize = 128;
pub const MAX_AUTOMATION_DISPLAY_NAME_LENGTH: usize = 200;
pub const MAX_AUTOMATION_MESSAGE_LENGTH: usize = 1_000;
pub const MAX_AUTOMATION_PRINT_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AutomationError {
    #[error("{field} must contain between 1 and {max} characters")]
    InvalidText { field: &'static str, max: usize },
    #[error("automation print content cannot exceed {MAX_AUTOMATION_PRINT_BYTES} bytes")]
    PrintPayloadTooLarge,
    #[error("scale timeout must be between 1 and 120,000 milliseconds")]
    InvalidScaleTimeout,
    #[error("PLC pulse duration must be between 1 and 60,000 milliseconds")]
    InvalidPulseDuration,
    #[error("automation command class does not match its device")]
    DeviceClassMismatch,
    #[error("automation result class does not match its command")]
    ResultClassMismatch,
}

fn validate_text(value: &str, field: &'static str, max: usize) -> Result<(), AutomationError> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.chars().count() > max
        || value.chars().any(char::is_control)
    {
        Err(AutomationError::InvalidText { field, max })
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationDeviceClass {
    Plc,
    Conveyor,
    Robotics,
    Sortation,
    Printer,
    Scale,
}

impl AutomationDeviceClass {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Plc => "plc",
            Self::Conveyor => "conveyor",
            Self::Robotics => "robotics",
            Self::Sortation => "sortation",
            Self::Printer => "printer",
            Self::Scale => "scale",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationControlMode {
    Disabled,
    Automatic,
    ManualFallback,
}

impl AutomationControlMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Automatic => "automatic",
            Self::ManualFallback => "manual_fallback",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationHealthState {
    Unknown,
    Healthy,
    Degraded,
    Offline,
    Faulted,
}

impl AutomationHealthState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Healthy => "healthy",
            Self::Degraded => "degraded",
            Self::Offline => "offline",
            Self::Faulted => "faulted",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRecoveryPolicy {
    DeviceDeduplicatedReplay,
    ProbeThenRetry,
    ManualReview,
}

impl AutomationRecoveryPolicy {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::DeviceDeduplicatedReplay => "device_deduplicated_replay",
            Self::ProbeThenRetry => "probe_then_retry",
            Self::ManualReview => "manual_review",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationRobotMissionKind {
    Pick,
    Place,
    Transport,
    Charge,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationPrintFormat {
    Zpl,
    Pdf,
    Png,
    Html,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AutomationWeightUnit {
    Gram,
    Kilogram,
    Pound,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum AutomationPlcCommand {
    SetDiscreteOutput { point: String, value: bool },
    PulseDiscreteOutput { point: String, duration_ms: u32 },
    ResetFault { fault_code: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum AutomationConveyorCommand {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum AutomationRoboticsCommand {
    DispatchMission {
        mission_id: String,
        mission_kind: AutomationRobotMissionKind,
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
pub enum AutomationSortationCommand {
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum AutomationPrinterCommand {
    PrintDocument {
        document_id: String,
        format: AutomationPrintFormat,
        content: String,
        copies: u16,
    },
    CancelPrintJob {
        spool_job_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum AutomationScaleCommand {
    ReadStableWeight {
        requested_unit: AutomationWeightUnit,
        timeout_ms: u32,
    },
    Tare,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "device_class", content = "command", rename_all = "snake_case")]
pub enum AutomationDeviceCommand {
    Plc(AutomationPlcCommand),
    Conveyor(AutomationConveyorCommand),
    Robotics(AutomationRoboticsCommand),
    Sortation(AutomationSortationCommand),
    Printer(AutomationPrinterCommand),
    Scale(AutomationScaleCommand),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationPlcResult {
    pub controller_reference: Option<String>,
    pub output_state: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationConveyorResult {
    pub controller_reference: Option<String>,
    pub observed_zone: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationRoboticsResult {
    pub controller_reference: String,
    pub mission_state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationSortationResult {
    pub controller_reference: Option<String>,
    pub observed_lane: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationPrinterResult {
    pub spool_job_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AutomationScaleResult {
    pub mass_milligrams: i64,
    pub stable: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "device_class", content = "result", rename_all = "snake_case")]
pub enum AutomationCommandResult {
    Plc(AutomationPlcResult),
    Conveyor(AutomationConveyorResult),
    Robotics(AutomationRoboticsResult),
    Sortation(AutomationSortationResult),
    Printer(AutomationPrinterResult),
    Scale(AutomationScaleResult),
}

impl AutomationCommandResult {
    pub const fn device_class(&self) -> AutomationDeviceClass {
        match self {
            Self::Plc(_) => AutomationDeviceClass::Plc,
            Self::Conveyor(_) => AutomationDeviceClass::Conveyor,
            Self::Robotics(_) => AutomationDeviceClass::Robotics,
            Self::Sortation(_) => AutomationDeviceClass::Sortation,
            Self::Printer(_) => AutomationDeviceClass::Printer,
            Self::Scale(_) => AutomationDeviceClass::Scale,
        }
    }

    pub fn validate_for(&self, command: &AutomationDeviceCommand) -> Result<(), AutomationError> {
        if self.device_class() != command.device_class() {
            return Err(AutomationError::ResultClassMismatch);
        }
        match self {
            Self::Plc(result) => {
                if let Some(reference) = &result.controller_reference {
                    validate_text(
                        reference,
                        "PLC controller reference",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                }
            }
            Self::Conveyor(result) => {
                if let Some(reference) = &result.controller_reference {
                    validate_text(
                        reference,
                        "conveyor controller reference",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                }
                if let Some(zone) = &result.observed_zone {
                    validate_text(
                        zone,
                        "observed conveyor zone",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                }
            }
            Self::Robotics(result) => {
                validate_text(
                    &result.controller_reference,
                    "robot controller reference",
                    MAX_AUTOMATION_IDENTIFIER_LENGTH,
                )?;
                validate_text(
                    &result.mission_state,
                    "robot mission state",
                    MAX_AUTOMATION_IDENTIFIER_LENGTH,
                )?;
            }
            Self::Sortation(result) => {
                if let Some(reference) = &result.controller_reference {
                    validate_text(
                        reference,
                        "sortation controller reference",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                }
                validate_text(
                    &result.observed_lane,
                    "observed sortation lane",
                    MAX_AUTOMATION_IDENTIFIER_LENGTH,
                )?;
            }
            Self::Printer(result) => validate_text(
                &result.spool_job_id,
                "print spool job ID",
                MAX_AUTOMATION_IDENTIFIER_LENGTH,
            )?,
            Self::Scale(_) => {}
        }
        Ok(())
    }
}

impl AutomationDeviceCommand {
    pub const fn device_class(&self) -> AutomationDeviceClass {
        match self {
            Self::Plc(_) => AutomationDeviceClass::Plc,
            Self::Conveyor(_) => AutomationDeviceClass::Conveyor,
            Self::Robotics(_) => AutomationDeviceClass::Robotics,
            Self::Sortation(_) => AutomationDeviceClass::Sortation,
            Self::Printer(_) => AutomationDeviceClass::Printer,
            Self::Scale(_) => AutomationDeviceClass::Scale,
        }
    }

    pub fn validate(&self) -> Result<(), AutomationError> {
        match self {
            Self::Plc(command) => match command {
                AutomationPlcCommand::SetDiscreteOutput { point, .. } => {
                    validate_text(point, "PLC point", MAX_AUTOMATION_IDENTIFIER_LENGTH)
                }
                AutomationPlcCommand::PulseDiscreteOutput { point, duration_ms } => {
                    validate_text(point, "PLC point", MAX_AUTOMATION_IDENTIFIER_LENGTH)?;
                    if !(1..=60_000).contains(duration_ms) {
                        return Err(AutomationError::InvalidPulseDuration);
                    }
                    Ok(())
                }
                AutomationPlcCommand::ResetFault { fault_code } => validate_text(
                    fault_code,
                    "PLC fault code",
                    MAX_AUTOMATION_IDENTIFIER_LENGTH,
                ),
            },
            Self::Conveyor(command) => match command {
                AutomationConveyorCommand::RouteCarrier {
                    carrier_id,
                    destination,
                } => {
                    validate_text(
                        carrier_id,
                        "conveyor carrier ID",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                    validate_text(
                        destination,
                        "conveyor destination",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )
                }
                AutomationConveyorCommand::StartZone { zone }
                | AutomationConveyorCommand::StopZone { zone } => {
                    validate_text(zone, "conveyor zone", MAX_AUTOMATION_IDENTIFIER_LENGTH)
                }
            },
            Self::Robotics(command) => match command {
                AutomationRoboticsCommand::DispatchMission {
                    mission_id,
                    source,
                    destination,
                    payload_id,
                    ..
                } => {
                    validate_text(
                        mission_id,
                        "robot mission ID",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                    validate_text(
                        source,
                        "robot mission source",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                    validate_text(
                        destination,
                        "robot mission destination",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                    if let Some(payload_id) = payload_id {
                        validate_text(
                            payload_id,
                            "robot payload ID",
                            MAX_AUTOMATION_IDENTIFIER_LENGTH,
                        )?;
                    }
                    Ok(())
                }
                AutomationRoboticsCommand::CancelMission { mission_id } => validate_text(
                    mission_id,
                    "robot mission ID",
                    MAX_AUTOMATION_IDENTIFIER_LENGTH,
                ),
            },
            Self::Sortation(command) => match command {
                AutomationSortationCommand::Divert { tracking_id, chute } => {
                    validate_text(
                        tracking_id,
                        "sortation tracking ID",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                    validate_text(chute, "sortation chute", MAX_AUTOMATION_IDENTIFIER_LENGTH)
                }
                AutomationSortationCommand::Reject {
                    tracking_id,
                    lane,
                    reason_code,
                } => {
                    validate_text(
                        tracking_id,
                        "sortation tracking ID",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                    validate_text(lane, "sortation lane", MAX_AUTOMATION_IDENTIFIER_LENGTH)?;
                    validate_text(
                        reason_code,
                        "sortation reason code",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )
                }
            },
            Self::Printer(command) => match command {
                AutomationPrinterCommand::PrintDocument {
                    document_id,
                    content,
                    copies,
                    ..
                } => {
                    validate_text(
                        document_id,
                        "print document ID",
                        MAX_AUTOMATION_IDENTIFIER_LENGTH,
                    )?;
                    if content.is_empty() {
                        return Err(AutomationError::InvalidText {
                            field: "print content",
                            max: MAX_AUTOMATION_PRINT_BYTES,
                        });
                    }
                    if content.len() > MAX_AUTOMATION_PRINT_BYTES {
                        return Err(AutomationError::PrintPayloadTooLarge);
                    }
                    if *copies == 0 {
                        return Err(AutomationError::InvalidText {
                            field: "print copies",
                            max: usize::from(u16::MAX),
                        });
                    }
                    Ok(())
                }
                AutomationPrinterCommand::CancelPrintJob { spool_job_id } => validate_text(
                    spool_job_id,
                    "print spool job ID",
                    MAX_AUTOMATION_IDENTIFIER_LENGTH,
                ),
            },
            Self::Scale(command) => match command {
                AutomationScaleCommand::ReadStableWeight { timeout_ms, .. } => {
                    if !(1..=120_000).contains(timeout_ms) {
                        Err(AutomationError::InvalidScaleTimeout)
                    } else {
                        Ok(())
                    }
                }
                AutomationScaleCommand::Tare => Ok(()),
            },
        }
    }
}

pub fn validate_automation_device(
    device_key: &str,
    display_name: &str,
) -> Result<(), AutomationError> {
    validate_text(
        device_key,
        "automation device key",
        MAX_AUTOMATION_IDENTIFIER_LENGTH,
    )?;
    if !device_key.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
    }) {
        return Err(AutomationError::InvalidText {
            field: "automation device key",
            max: MAX_AUTOMATION_IDENTIFIER_LENGTH,
        });
    }
    validate_text(
        display_name,
        "automation device display name",
        MAX_AUTOMATION_DISPLAY_NAME_LENGTH,
    )
}

pub fn validate_automation_message(message: &str) -> Result<(), AutomationError> {
    validate_text(message, "automation message", MAX_AUTOMATION_MESSAGE_LENGTH)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn automation_commands_are_typed_and_bounded() {
        let valid = AutomationDeviceCommand::Scale(AutomationScaleCommand::ReadStableWeight {
            requested_unit: AutomationWeightUnit::Gram,
            timeout_ms: 5_000,
        });
        assert_eq!(valid.device_class(), AutomationDeviceClass::Scale);
        assert!(valid.validate().is_ok());
        assert_eq!(
            AutomationDeviceCommand::Plc(AutomationPlcCommand::PulseDiscreteOutput {
                point: "lane.release".into(),
                duration_ms: 0,
            })
            .validate(),
            Err(AutomationError::InvalidPulseDuration)
        );
    }

    #[test]
    fn automation_device_keys_are_compatible_with_edge_identifiers() {
        assert!(validate_automation_device("scale-01/pack:a", "Pack scale").is_ok());
        assert!(validate_automation_device("scale 01", "Pack scale").is_err());
        assert!(validate_automation_device("scale#01", "Pack scale").is_err());
    }
}
