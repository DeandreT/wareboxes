use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::command::{
    CommandEnvelope, CommandResult, ConveyorCommand, ConveyorResult, PlcCommand, PlcResult,
    PrinterCommand, PrinterResult, RoboticsCommand, RoboticsResult, ScaleCommand, ScaleResult,
    SortationCommand, SortationResult,
};
use crate::types::{DeviceClass, DeviceDescriptor, DeviceId, HealthState, TypeError};

const MAX_ADAPTER_MESSAGE_LENGTH: usize = 1_000;
const MAX_ALARM_CODES: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdapterCapabilities {
    /// The downstream controller durably deduplicates the stable command or
    /// correlation identity across process and connection restarts.
    pub device_side_duplicate_protection: bool,
    /// The downstream controller can query a prior command by stable identity.
    pub recovery_probe: bool,
}

impl AdapterCapabilities {
    pub const fn manual_only() -> Self {
        Self {
            device_side_duplicate_protection: false,
            recovery_probe: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterFailureClass {
    /// The adapter knows the downstream controller did not accept the command;
    /// retrying later is safe.
    Retryable,
    /// The adapter knows the downstream controller rejected the command and no
    /// physical effect occurred.
    Permanent,
    /// The adapter cannot prove whether the downstream controller accepted the
    /// command. The engine applies the command's explicit recovery policy.
    Ambiguous,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("{class:?} adapter failure: {message}")]
pub struct AdapterFailure {
    pub class: AdapterFailureClass,
    pub message: String,
}

impl AdapterFailure {
    pub fn new(class: AdapterFailureClass, message: impl Into<String>) -> Self {
        let mut message = message.into();
        message.truncate(MAX_ADAPTER_MESSAGE_LENGTH);
        if message.trim().is_empty() {
            message = "adapter did not provide an error message".into();
        }
        Self { class, message }
    }

    pub fn retryable(message: impl Into<String>) -> Self {
        Self::new(AdapterFailureClass::Retryable, message)
    }

    pub fn permanent(message: impl Into<String>) -> Self {
        Self::new(AdapterFailureClass::Permanent, message)
    }

    pub fn ambiguous(message: impl Into<String>) -> Self {
        Self::new(AdapterFailureClass::Ambiguous, message)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthReport {
    pub state: HealthState,
    pub message: Option<String>,
    pub alarm_codes: Vec<String>,
}

impl HealthReport {
    pub fn healthy() -> Self {
        Self {
            state: HealthState::Healthy,
            message: None,
            alarm_codes: Vec::new(),
        }
    }

    pub fn validate(&self) -> Result<(), AdapterFailure> {
        if self
            .message
            .as_ref()
            .is_some_and(|message| message.len() > MAX_ADAPTER_MESSAGE_LENGTH)
        {
            return Err(AdapterFailure::permanent(
                "health message exceeds 1,000 characters",
            ));
        }
        if self.alarm_codes.len() > MAX_ALARM_CODES
            || self.alarm_codes.iter().any(|code| {
                code.trim().is_empty() || code.len() > 128 || code.chars().any(char::is_whitespace)
            })
        {
            return Err(AdapterFailure::permanent(
                "health report contains invalid alarm codes",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecoveryOutcome<T> {
    Completed(T),
    StillProcessing,
    NotFound,
    ManualReview { reason: String },
}

impl<T> RecoveryOutcome<T> {
    fn map<U>(self, mapper: impl FnOnce(T) -> U) -> RecoveryOutcome<U> {
        match self {
            Self::Completed(value) => RecoveryOutcome::Completed(mapper(value)),
            Self::StillProcessing => RecoveryOutcome::StillProcessing,
            Self::NotFound => RecoveryOutcome::NotFound,
            Self::ManualReview { reason } => RecoveryOutcome::ManualReview { reason },
        }
    }
}

/// Type-erased boundary used by the engine.
///
/// Vendor implementations normally implement one of the typed driver traits and
/// use its matching bridge. Implementations must propagate the stable command and
/// correlation IDs downstream and must classify uncertain outcomes as ambiguous.
pub trait DeviceAdapter: Send {
    fn descriptor(&self) -> &DeviceDescriptor;
    fn capabilities(&self) -> AdapterCapabilities;
    fn heartbeat(&mut self) -> Result<HealthReport, AdapterFailure>;
    fn execute(&mut self, envelope: &CommandEnvelope) -> Result<CommandResult, AdapterFailure>;
    fn recover(
        &mut self,
        envelope: &CommandEnvelope,
    ) -> Result<RecoveryOutcome<CommandResult>, AdapterFailure>;
}

macro_rules! define_driver {
    ($name:ident, $command:ty, $result:ty) => {
        pub trait $name: Send {
            fn capabilities(&self) -> AdapterCapabilities;
            fn heartbeat(&mut self) -> Result<HealthReport, AdapterFailure>;
            fn execute(
                &mut self,
                envelope: &CommandEnvelope,
                command: &$command,
            ) -> Result<$result, AdapterFailure>;
            fn recover(
                &mut self,
                envelope: &CommandEnvelope,
                command: &$command,
            ) -> Result<RecoveryOutcome<$result>, AdapterFailure>;
        }
    };
}

define_driver!(PlcDriver, PlcCommand, PlcResult);
define_driver!(ConveyorDriver, ConveyorCommand, ConveyorResult);
define_driver!(RoboticsDriver, RoboticsCommand, RoboticsResult);
define_driver!(SortationDriver, SortationCommand, SortationResult);
define_driver!(PrinterDriver, PrinterCommand, PrinterResult);
define_driver!(ScaleDriver, ScaleCommand, ScaleResult);

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error(transparent)]
    InvalidDescriptor(#[from] TypeError),
    #[error("adapter class {actual} does not match registered class {expected}")]
    ClassMismatch {
        expected: DeviceClass,
        actual: DeviceClass,
    },
    #[error("device {0} already has a registered adapter")]
    DuplicateDevice(DeviceId),
}

macro_rules! define_bridge {
    (
        $bridge:ident,
        $driver:ident,
        $class:expr,
        $variant:ident,
        $result_variant:ident
    ) => {
        pub struct $bridge<D> {
            descriptor: DeviceDescriptor,
            driver: D,
        }

        impl<D> $bridge<D>
        where
            D: $driver,
        {
            pub fn new(descriptor: DeviceDescriptor, driver: D) -> Result<Self, RegistryError> {
                descriptor.validate()?;
                if descriptor.class != $class {
                    return Err(RegistryError::ClassMismatch {
                        expected: $class,
                        actual: descriptor.class,
                    });
                }
                Ok(Self { descriptor, driver })
            }

            pub fn driver_mut(&mut self) -> &mut D {
                &mut self.driver
            }
        }

        impl<D> DeviceAdapter for $bridge<D>
        where
            D: $driver,
        {
            fn descriptor(&self) -> &DeviceDescriptor {
                &self.descriptor
            }

            fn capabilities(&self) -> AdapterCapabilities {
                self.driver.capabilities()
            }

            fn heartbeat(&mut self) -> Result<HealthReport, AdapterFailure> {
                self.driver.heartbeat()
            }

            fn execute(
                &mut self,
                envelope: &CommandEnvelope,
            ) -> Result<CommandResult, AdapterFailure> {
                match &envelope.request.command {
                    crate::command::DeviceCommand::$variant(command) => self
                        .driver
                        .execute(envelope, command)
                        .map(CommandResult::$result_variant),
                    _ => Err(AdapterFailure::permanent(
                        "command class does not match adapter class",
                    )),
                }
            }

            fn recover(
                &mut self,
                envelope: &CommandEnvelope,
            ) -> Result<RecoveryOutcome<CommandResult>, AdapterFailure> {
                match &envelope.request.command {
                    crate::command::DeviceCommand::$variant(command) => self
                        .driver
                        .recover(envelope, command)
                        .map(|outcome| outcome.map(CommandResult::$result_variant)),
                    _ => Err(AdapterFailure::permanent(
                        "command class does not match adapter class",
                    )),
                }
            }
        }
    };
}

define_bridge!(PlcAdapterBridge, PlcDriver, DeviceClass::Plc, Plc, Plc);
define_bridge!(
    ConveyorAdapterBridge,
    ConveyorDriver,
    DeviceClass::Conveyor,
    Conveyor,
    Conveyor
);
define_bridge!(
    RoboticsAdapterBridge,
    RoboticsDriver,
    DeviceClass::Robotics,
    Robotics,
    Robotics
);
define_bridge!(
    SortationAdapterBridge,
    SortationDriver,
    DeviceClass::Sortation,
    Sortation,
    Sortation
);
define_bridge!(
    PrinterAdapterBridge,
    PrinterDriver,
    DeviceClass::Printer,
    Printer,
    Printer
);
define_bridge!(
    ScaleAdapterBridge,
    ScaleDriver,
    DeviceClass::Scale,
    Scale,
    Scale
);

#[derive(Default)]
pub struct AdapterRegistry {
    adapters: BTreeMap<DeviceId, Box<dyn DeviceAdapter>>,
}

impl AdapterRegistry {
    pub fn register(&mut self, adapter: impl DeviceAdapter + 'static) -> Result<(), RegistryError> {
        adapter.descriptor().validate()?;
        let device_id = adapter.descriptor().device_id.clone();
        if self.adapters.contains_key(&device_id) {
            return Err(RegistryError::DuplicateDevice(device_id));
        }
        self.adapters.insert(device_id, Box::new(adapter));
        Ok(())
    }

    pub fn get(&self, device_id: &DeviceId) -> Option<&dyn DeviceAdapter> {
        self.adapters.get(device_id).map(Box::as_ref)
    }

    pub fn get_mut(&mut self, device_id: &DeviceId) -> Option<&mut (dyn DeviceAdapter + '_)> {
        match self.adapters.get_mut(device_id) {
            Some(adapter) => Some(adapter.as_mut()),
            None => None,
        }
    }

    pub fn len(&self) -> usize {
        self.adapters.len()
    }

    pub fn is_empty(&self) -> bool {
        self.adapters.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::command::{ScaleCommand, ScaleResult};
    use crate::types::{FacilityId, TenantId};

    struct TestScale;

    impl ScaleDriver for TestScale {
        fn capabilities(&self) -> AdapterCapabilities {
            AdapterCapabilities::manual_only()
        }

        fn heartbeat(&mut self) -> Result<HealthReport, AdapterFailure> {
            Ok(HealthReport::healthy())
        }

        fn execute(
            &mut self,
            _envelope: &CommandEnvelope,
            _command: &ScaleCommand,
        ) -> Result<ScaleResult, AdapterFailure> {
            Ok(ScaleResult {
                mass_milligrams: 42_000,
                stable: true,
            })
        }

        fn recover(
            &mut self,
            _envelope: &CommandEnvelope,
            _command: &ScaleCommand,
        ) -> Result<RecoveryOutcome<ScaleResult>, AdapterFailure> {
            Ok(RecoveryOutcome::NotFound)
        }
    }

    fn descriptor(class: DeviceClass) -> DeviceDescriptor {
        DeviceDescriptor {
            tenant_id: TenantId::new("tenant-1").unwrap(),
            facility_id: FacilityId::new("facility-1").unwrap(),
            device_id: DeviceId::new("scale-1").unwrap(),
            class,
            display_name: "Pack scale 1".into(),
        }
    }

    #[test]
    fn typed_bridge_rejects_wrong_device_class() {
        assert!(matches!(
            ScaleAdapterBridge::new(descriptor(DeviceClass::Printer), TestScale),
            Err(RegistryError::ClassMismatch { .. })
        ));
    }

    #[test]
    fn registry_rejects_duplicate_device_bindings() {
        let mut registry = AdapterRegistry::default();
        registry
            .register(ScaleAdapterBridge::new(descriptor(DeviceClass::Scale), TestScale).unwrap())
            .unwrap();
        let duplicate = ScaleAdapterBridge::new(descriptor(DeviceClass::Scale), TestScale).unwrap();
        assert!(matches!(
            registry.register(duplicate),
            Err(RegistryError::DuplicateDevice(_))
        ));
    }
}
