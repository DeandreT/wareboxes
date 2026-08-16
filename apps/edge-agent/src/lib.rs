#![doc = include_str!("../README.md")]

pub mod adapter;
pub mod cloud;
pub mod command;
pub mod engine;
pub mod store;
pub mod types;

pub use adapter::{
    AdapterCapabilities, AdapterFailure, AdapterFailureClass, AdapterRegistry, DeviceAdapter,
    HealthReport, RecoveryOutcome,
};
pub use cloud::{
    CloudClient, CloudClientConfig, CloudSync, CloudSyncConfig, CloudSyncError,
    CloudSyncLoopConfig, CloudSyncLoopControl, CloudSyncSummary, CloudTransport,
    CloudTransportError,
};
pub use command::{
    CommandEnvelope, CommandRecord, CommandRequest, CommandResult, CommandState, DeviceCommand,
    RecoveryPolicy, SubmissionOutcome,
};
pub use engine::{EdgeEngine, EngineConfig, RunSummary};
pub use store::{CloudDelivery, CloudDeliveryRecord, EdgeStore, PendingCloudCommand, StoreError};
pub use types::{
    ActorId, CommandId, ControlAction, ControlMode, CorrelationId, DeviceClass, DeviceDescriptor,
    DeviceId, FacilityId, HealthState, IdempotencyKey, SafetyConfirmation, TenantId,
};
