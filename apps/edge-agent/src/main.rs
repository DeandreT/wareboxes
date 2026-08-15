use std::path::PathBuf;

use anyhow::{bail, Context};
use chrono::Utc;
use tracing_subscriber::EnvFilter;
use wareboxes_edge_agent::{
    ActorId, ControlAction, DeviceClass, DeviceDescriptor, DeviceId, EdgeStore, FacilityId,
    SafetyConfirmation, TenantId,
};

fn main() -> anyhow::Result<()> {
    init_tracing()?;
    let mut arguments = std::env::args().skip(1);
    let store_path = std::env::var_os("EDGE_STORE_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("wareboxes-edge.sqlite3"));
    let mut store = EdgeStore::open(&store_path)
        .with_context(|| format!("opening edge store at {}", store_path.display()))?;
    let Some(command) = arguments.next() else {
        print_usage();
        return Ok(());
    };
    match command.as_str() {
        "status" => {
            let devices = store.list_devices()?;
            println!("{}", serde_json::to_string_pretty(&devices)?);
        }
        "command" => {
            let command_id = required(&mut arguments, "command ID")?;
            reject_extra(arguments)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&store.command(&command_id)?)?
            );
        }
        "attempts" => {
            let command_id = required(&mut arguments, "command ID")?;
            reject_extra(arguments)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&store.attempts(&command_id)?)?
            );
        }
        "command-events" => {
            let command_id = required(&mut arguments, "command ID")?;
            reject_extra(arguments)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&store.command_events(&command_id)?)?
            );
        }
        "control-events" => {
            let device_id = DeviceId::new(required(&mut arguments, "device ID")?)?;
            reject_extra(arguments)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&store.control_events(&device_id)?)?
            );
        }
        "heartbeats" => {
            let device_id = DeviceId::new(required(&mut arguments, "device ID")?)?;
            reject_extra(arguments)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&store.heartbeat_events(&device_id)?)?
            );
        }
        "register" => {
            let device_id = DeviceId::new(required(&mut arguments, "device ID")?)?;
            let tenant_id = TenantId::new(required(&mut arguments, "tenant ID")?)?;
            let facility_id = FacilityId::new(required(&mut arguments, "facility ID")?)?;
            let class = required(&mut arguments, "device class")?.parse::<DeviceClass>()?;
            let display_name = required(&mut arguments, "display name")?;
            let actor = ActorId::new(required(&mut arguments, "actor ID")?)?;
            let reason = required(&mut arguments, "reason")?;
            reject_extra(arguments)?;
            let status = store.register_device(
                DeviceDescriptor {
                    tenant_id,
                    facility_id,
                    device_id,
                    class,
                    display_name,
                },
                &actor,
                &reason,
                Utc::now(),
            )?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        "disable" | "manual-fallback" => {
            let device_id = DeviceId::new(required(&mut arguments, "device ID")?)?;
            let actor = ActorId::new(required(&mut arguments, "actor ID")?)?;
            let reason = required(&mut arguments, "reason")?;
            reject_extra(arguments)?;
            let action = if command == "disable" {
                ControlAction::Disable
            } else {
                ControlAction::EnterManualFallback
            };
            let status =
                store.change_control_mode(&device_id, action, &actor, &reason, Utc::now())?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        "resume" => {
            let device_id = DeviceId::new(required(&mut arguments, "device ID")?)?;
            let actor = ActorId::new(required(&mut arguments, "actor ID")?)?;
            let reason = required(&mut arguments, "reason")?;
            let confirmation = required(&mut arguments, "confirmation")?;
            reject_extra(arguments)?;
            if confirmation != "CONFIRM-SAFE-TO-RESUME" {
                bail!("resume requires the exact confirmation token CONFIRM-SAFE-TO-RESUME");
            }
            let status = store.change_control_mode(
                &device_id,
                ControlAction::ResumeAutomation(
                    SafetyConfirmation::after_physical_safety_checklist(),
                ),
                &actor,
                &reason,
                Utc::now(),
            )?;
            println!("{}", serde_json::to_string_pretty(&status)?);
        }
        "resolve" => {
            let command_id = required(&mut arguments, "command ID")?;
            let actor = ActorId::new(required(&mut arguments, "actor ID")?)?;
            let note = required(&mut arguments, "resolution note")?;
            reject_extra(arguments)?;
            let record = store.resolve_manually(&command_id, &actor, &note, Utc::now())?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        "retry" => {
            let command_id = required(&mut arguments, "command ID")?;
            let actor = ActorId::new(required(&mut arguments, "actor ID")?)?;
            let reason = required(&mut arguments, "reason")?;
            reject_extra(arguments)?;
            let record = store.retry_after_review(&command_id, &actor, &reason, Utc::now())?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        "cancel" => {
            let command_id = required(&mut arguments, "command ID")?;
            let actor = ActorId::new(required(&mut arguments, "actor ID")?)?;
            let reason = required(&mut arguments, "reason")?;
            reject_extra(arguments)?;
            let record = store.cancel_command(&command_id, &actor, &reason, Utc::now())?;
            println!("{}", serde_json::to_string_pretty(&record)?);
        }
        _ => bail!("unknown edge-agent command: {command}"),
    }
    Ok(())
}

fn required(arguments: &mut impl Iterator<Item = String>, field: &str) -> anyhow::Result<String> {
    arguments.next().with_context(|| format!("missing {field}"))
}

fn reject_extra(mut arguments: impl Iterator<Item = String>) -> anyhow::Result<()> {
    if let Some(value) = arguments.next() {
        bail!("unexpected argument: {value}");
    }
    Ok(())
}

fn print_usage() {
    eprintln!(
        "usage: wareboxes-edge-agent <status|command|attempts|command-events|control-events|heartbeats|register|disable|manual-fallback|resume|resolve|retry|cancel> ..."
    );
}

fn init_tracing() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info,wareboxes_edge_agent=debug")),
        )
        .try_init()
        .map_err(|error| anyhow::anyhow!("initializing edge-agent tracing: {error}"))
}
