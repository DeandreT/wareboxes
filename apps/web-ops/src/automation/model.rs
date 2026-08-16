use wareboxes_api_contract::v1::{
    AutomationConveyorCommand, AutomationDeviceClass, AutomationDeviceCommand,
    AutomationPlcCommand, AutomationPrintFormat, AutomationPrinterCommand,
    AutomationRobotMissionKind, AutomationRoboticsCommand, AutomationScaleCommand,
    AutomationSortationCommand, AutomationWeightUnit,
};

pub(super) fn class_label(value: AutomationDeviceClass) -> &'static str {
    match value {
        AutomationDeviceClass::Plc => "PLC",
        AutomationDeviceClass::Conveyor => "Conveyor",
        AutomationDeviceClass::Robotics => "Robotics",
        AutomationDeviceClass::Sortation => "Sortation",
        AutomationDeviceClass::Printer => "Printer",
        AutomationDeviceClass::Scale => "Scale",
    }
}

pub(super) fn operations(value: AutomationDeviceClass) -> &'static [(&'static str, &'static str)] {
    match value {
        AutomationDeviceClass::Plc => &[
            ("set_output", "Set output"),
            ("pulse_output", "Pulse output"),
            ("reset_fault", "Reset fault"),
        ],
        AutomationDeviceClass::Conveyor => &[
            ("route_carrier", "Route carrier"),
            ("start_zone", "Start zone"),
            ("stop_zone", "Stop zone"),
        ],
        AutomationDeviceClass::Robotics => &[
            ("dispatch_mission", "Dispatch mission"),
            ("cancel_mission", "Cancel mission"),
        ],
        AutomationDeviceClass::Sortation => &[("divert", "Divert"), ("reject", "Reject")],
        AutomationDeviceClass::Printer => &[
            ("print_document", "Print document"),
            ("cancel_print_job", "Cancel print job"),
        ],
        AutomationDeviceClass::Scale => &[("read_weight", "Read stable weight"), ("tare", "Tare")],
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct CommandDraft {
    pub operation: String,
    pub first: String,
    pub second: String,
    pub third: String,
    pub fourth: String,
    pub choice: String,
    pub number: String,
    pub flag: bool,
}

impl CommandDraft {
    pub fn reset_for(&mut self, class: AutomationDeviceClass) {
        *self = Self::default();
        self.operation = operations(class)[0].0.to_owned();
        self.choice = match class {
            AutomationDeviceClass::Robotics => "transport",
            AutomationDeviceClass::Printer => "zpl",
            AutomationDeviceClass::Scale => "gram",
            _ => "",
        }
        .to_owned();
        self.number = match class {
            AutomationDeviceClass::Printer => "1",
            AutomationDeviceClass::Scale => "5000",
            _ => "",
        }
        .to_owned();
    }

    pub fn build(&self, class: AutomationDeviceClass) -> Result<AutomationDeviceCommand, String> {
        let required = |value: &str, label: &str| {
            let value = value.trim();
            if value.is_empty() {
                Err(format!("{label} is required."))
            } else {
                Ok(value.to_owned())
            }
        };
        let number = |label: &str| {
            self.number
                .parse::<u32>()
                .map_err(|_| format!("{label} must be a positive whole number."))
                .and_then(|value| {
                    if value == 0 {
                        Err(format!("{label} must be positive."))
                    } else {
                        Ok(value)
                    }
                })
        };
        match (class, self.operation.as_str()) {
            (AutomationDeviceClass::Plc, "set_output") => Ok(AutomationDeviceCommand::Plc(
                AutomationPlcCommand::SetDiscreteOutput {
                    point: required(&self.first, "PLC point")?,
                    value: self.flag,
                },
            )),
            (AutomationDeviceClass::Plc, "pulse_output") => Ok(AutomationDeviceCommand::Plc(
                AutomationPlcCommand::PulseDiscreteOutput {
                    point: required(&self.first, "PLC point")?,
                    duration_ms: number("Pulse duration")?,
                },
            )),
            (AutomationDeviceClass::Plc, "reset_fault") => Ok(AutomationDeviceCommand::Plc(
                AutomationPlcCommand::ResetFault {
                    fault_code: required(&self.first, "Fault code")?,
                },
            )),
            (AutomationDeviceClass::Conveyor, "route_carrier") => Ok(
                AutomationDeviceCommand::Conveyor(AutomationConveyorCommand::RouteCarrier {
                    carrier_id: required(&self.first, "Carrier ID")?,
                    destination: required(&self.second, "Destination")?,
                }),
            ),
            (AutomationDeviceClass::Conveyor, "start_zone") => Ok(
                AutomationDeviceCommand::Conveyor(AutomationConveyorCommand::StartZone {
                    zone: required(&self.first, "Zone")?,
                }),
            ),
            (AutomationDeviceClass::Conveyor, "stop_zone") => Ok(
                AutomationDeviceCommand::Conveyor(AutomationConveyorCommand::StopZone {
                    zone: required(&self.first, "Zone")?,
                }),
            ),
            (AutomationDeviceClass::Robotics, "dispatch_mission") => Ok(
                AutomationDeviceCommand::Robotics(AutomationRoboticsCommand::DispatchMission {
                    mission_id: required(&self.first, "Mission ID")?,
                    mission_kind: mission_kind(&self.choice)?,
                    source: required(&self.second, "Source")?,
                    destination: required(&self.third, "Destination")?,
                    payload_id: optional(&self.fourth),
                }),
            ),
            (AutomationDeviceClass::Robotics, "cancel_mission") => Ok(
                AutomationDeviceCommand::Robotics(AutomationRoboticsCommand::CancelMission {
                    mission_id: required(&self.first, "Mission ID")?,
                }),
            ),
            (AutomationDeviceClass::Sortation, "divert") => Ok(AutomationDeviceCommand::Sortation(
                AutomationSortationCommand::Divert {
                    tracking_id: required(&self.first, "Tracking ID")?,
                    chute: required(&self.second, "Chute")?,
                },
            )),
            (AutomationDeviceClass::Sortation, "reject") => Ok(AutomationDeviceCommand::Sortation(
                AutomationSortationCommand::Reject {
                    tracking_id: required(&self.first, "Tracking ID")?,
                    lane: required(&self.second, "Lane")?,
                    reason_code: required(&self.third, "Reason code")?,
                },
            )),
            (AutomationDeviceClass::Printer, "print_document") => Ok(
                AutomationDeviceCommand::Printer(AutomationPrinterCommand::PrintDocument {
                    document_id: required(&self.first, "Document ID")?,
                    format: print_format(&self.choice)?,
                    content: required(&self.second, "Print content")?,
                    copies: u16::try_from(number("Copies")?)
                        .map_err(|_| "Copies are too large.".to_owned())?,
                }),
            ),
            (AutomationDeviceClass::Printer, "cancel_print_job") => Ok(
                AutomationDeviceCommand::Printer(AutomationPrinterCommand::CancelPrintJob {
                    spool_job_id: required(&self.first, "Spool job ID")?,
                }),
            ),
            (AutomationDeviceClass::Scale, "read_weight") => Ok(AutomationDeviceCommand::Scale(
                AutomationScaleCommand::ReadStableWeight {
                    requested_unit: weight_unit(&self.choice)?,
                    timeout_ms: number("Scale timeout")?,
                },
            )),
            (AutomationDeviceClass::Scale, "tare") => {
                Ok(AutomationDeviceCommand::Scale(AutomationScaleCommand::Tare))
            }
            _ => Err("Select an operation supported by this device class.".into()),
        }
    }
}

fn optional(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn mission_kind(value: &str) -> Result<AutomationRobotMissionKind, String> {
    match value {
        "pick" => Ok(AutomationRobotMissionKind::Pick),
        "place" => Ok(AutomationRobotMissionKind::Place),
        "transport" => Ok(AutomationRobotMissionKind::Transport),
        "charge" => Ok(AutomationRobotMissionKind::Charge),
        _ => Err("Select a mission kind.".into()),
    }
}

fn print_format(value: &str) -> Result<AutomationPrintFormat, String> {
    match value {
        "zpl" => Ok(AutomationPrintFormat::Zpl),
        "pdf" => Ok(AutomationPrintFormat::Pdf),
        "png" => Ok(AutomationPrintFormat::Png),
        "html" => Ok(AutomationPrintFormat::Html),
        _ => Err("Select a print format.".into()),
    }
}

fn weight_unit(value: &str) -> Result<AutomationWeightUnit, String> {
    match value {
        "gram" => Ok(AutomationWeightUnit::Gram),
        "kilogram" => Ok(AutomationWeightUnit::Kilogram),
        "pound" => Ok(AutomationWeightUnit::Pound),
        _ => Err("Select a weight unit.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_device_class_builds_a_typed_command() {
        let cases = [
            (AutomationDeviceClass::Plc, "point-1", "", "", ""),
            (
                AutomationDeviceClass::Conveyor,
                "carrier-1",
                "chute-2",
                "",
                "",
            ),
            (
                AutomationDeviceClass::Robotics,
                "mission-1",
                "source-1",
                "destination-1",
                "payload-1",
            ),
            (
                AutomationDeviceClass::Sortation,
                "tracking-1",
                "chute-1",
                "",
                "",
            ),
            (
                AutomationDeviceClass::Printer,
                "document-1",
                "^XA^XZ",
                "",
                "",
            ),
            (AutomationDeviceClass::Scale, "", "", "", ""),
        ];
        for (class, first, second, third, fourth) in cases {
            let mut draft = CommandDraft::default();
            draft.reset_for(class);
            draft.first = first.into();
            draft.second = second.into();
            draft.third = third.into();
            draft.fourth = fourth.into();
            assert!(draft.build(class).is_ok(), "failed for {class:?}");
        }
    }
}
