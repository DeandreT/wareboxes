use axum::http::Method;
use serde_json::json;
use wareboxes_api_contract::v1::{
    YardAppointmentResponse, YardAssetResponse, YardLocationResponse, YardVisitResponse,
    YardVisitStatus,
};

use super::support::SeedContext;

pub async fn seed(context: &SeedContext) -> anyhow::Result<()> {
    let gate = location(
        context,
        "demo-yard-gate",
        "GATE-1",
        "Main security gate",
        "gate",
    )
    .await?;
    let parking = location(
        context,
        "demo-yard-parking",
        "YARD-A",
        "North trailer parking",
        "parking",
    )
    .await?;
    let door_one = location(
        context,
        "demo-yard-door-one",
        "DOOR-1",
        "Receiving door 1",
        "dock_door",
    )
    .await?;
    let door_two = location(
        context,
        "demo-yard-door-two",
        "DOOR-2",
        "Receiving door 2",
        "dock_door",
    )
    .await?;

    let active_asset = asset(context, "demo-yard-asset-active", "TRL-5341").await?;
    let completed_asset = asset(context, "demo-yard-asset-complete", "TRL-7812").await?;

    let active_appointment = appointment(
        context,
        "demo-yard-appointment-active",
        "APT-RNO-5341",
        "TRL-5341",
        "2026-08-12T14:00:00Z",
        "2026-08-12T15:00:00Z",
    )
    .await?;
    let completed_appointment = appointment(
        context,
        "demo-yard-appointment-complete",
        "APT-RNO-7812",
        "TRL-7812",
        "2026-08-12T10:00:00Z",
        "2026-08-12T11:00:00Z",
    )
    .await?;
    let _scheduled = appointment(
        context,
        "demo-yard-appointment-scheduled",
        "APT-RNO-9920",
        "TRL-9920",
        "2026-08-13T08:00:00Z",
        "2026-08-13T09:00:00Z",
    )
    .await?;

    let active = gate_in(
        context,
        "demo-yard-gate-in-active",
        active_appointment.appointment_id,
        active_asset.asset_id,
        gate.location_id,
        "Avery Morgan",
    )
    .await?;
    let spotted: YardVisitResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/yard/visits/{}/spot-moves", active.visit_id),
            "demo-yard-spot-active",
            json!({
                "expected_revision":active.revision,
                "destination_location_id":parking.location_id,
                "note":"Staged while receiving door cleared"
            }),
        )
        .await?;
    let at_door: YardVisitResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/yard/visits/{}/door-assignments", active.visit_id),
            "demo-yard-door-active",
            json!({
                "expected_revision":spotted.revision,
                "door_location_id":door_one.location_id,
                "note":"Door ready for inbound unload"
            }),
        )
        .await?;
    let unloading: YardVisitResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/yard/visits/{}/operation-starts", active.visit_id),
            "demo-yard-start-active",
            json!({
                "expected_revision":at_door.revision,
                "operation":"unloading",
                "note":"Inbound unload in progress"
            }),
        )
        .await?;
    if unloading.status != YardVisitStatus::Unloading {
        anyhow::bail!("demo active yard visit did not reach unloading");
    }

    let completed = gate_in(
        context,
        "demo-yard-gate-in-complete",
        completed_appointment.appointment_id,
        completed_asset.asset_id,
        gate.location_id,
        "Jordan Lee",
    )
    .await?;
    let at_door: YardVisitResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/yard/visits/{}/door-assignments",
                completed.visit_id
            ),
            "demo-yard-door-complete",
            json!({
                "expected_revision":completed.revision,
                "door_location_id":door_two.location_id,
                "note":"Directed to open receiving door"
            }),
        )
        .await?;
    let started: YardVisitResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/yard/visits/{}/operation-starts",
                completed.visit_id
            ),
            "demo-yard-start-complete",
            json!({
                "expected_revision":at_door.revision,
                "operation":"unloading",
                "note":"Receiving team began unload"
            }),
        )
        .await?;
    let ready: YardVisitResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/yard/visits/{}/operation-completions",
                completed.visit_id
            ),
            "demo-yard-complete-operation",
            json!({
                "expected_revision":started.revision,
                "operation":"unloading",
                "note":"Trailer empty and paperwork returned"
            }),
        )
        .await?;
    let departed: YardVisitResponse = context
        .command(
            Method::POST,
            &format!("/api/v1/yard/visits/{}/gate-outs", completed.visit_id),
            "demo-yard-gate-out-complete",
            json!({
                "expected_revision":ready.revision,
                "note":"Seal and exit confirmed by gate operator"
            }),
        )
        .await?;
    if departed.status != YardVisitStatus::GatedOut || departed.detention.is_none() {
        anyhow::bail!("demo completed yard visit is missing detention evidence");
    }

    println!(
        "seeded yard appointment board with active visit {} and completed visit {}",
        active.visit_id, completed.visit_id
    );
    Ok(())
}

async fn location(
    context: &SeedContext,
    key: &str,
    code: &str,
    name: &str,
    kind: &str,
) -> anyhow::Result<YardLocationResponse> {
    context
        .command(
            Method::POST,
            "/api/v1/yard/locations",
            key,
            json!({
                "facility_id":context.facility_id,
                "code":code,
                "name":name,
                "kind":kind
            }),
        )
        .await
}

async fn asset(
    context: &SeedContext,
    key: &str,
    asset_number: &str,
) -> anyhow::Result<YardAssetResponse> {
    context
        .command(
            Method::POST,
            "/api/v1/yard/assets",
            key,
            json!({
                "kind":"trailer",
                "asset_number":asset_number,
                "carrier":"Sierra Freight"
            }),
        )
        .await
}

async fn appointment(
    context: &SeedContext,
    key: &str,
    appointment_number: &str,
    expected_asset_number: &str,
    scheduled_from: &str,
    scheduled_until: &str,
) -> anyhow::Result<YardAppointmentResponse> {
    context
        .command(
            Method::POST,
            "/api/v1/yard/appointments",
            key,
            json!({
                "inventory_owner_id":context.inventory_owner_id,
                "facility_id":context.facility_id,
                "direction":"inbound",
                "appointment_number":appointment_number,
                "scheduled_from":scheduled_from,
                "scheduled_until":scheduled_until,
                "carrier":"Sierra Freight",
                "expected_asset_kind":"trailer",
                "expected_asset_number":expected_asset_number,
                "free_minutes":120,
                "note":"Demo inbound yard appointment"
            }),
        )
        .await
}

async fn gate_in(
    context: &SeedContext,
    key: &str,
    appointment_id: i64,
    asset_id: i64,
    gate_location_id: i64,
    driver_name: &str,
) -> anyhow::Result<YardVisitResponse> {
    context
        .command(
            Method::POST,
            "/api/v1/yard/visits",
            key,
            json!({
                "appointment_id":appointment_id,
                "inventory_owner_id":context.inventory_owner_id,
                "facility_id":context.facility_id,
                "direction":"inbound",
                "asset_id":asset_id,
                "driver_name":driver_name,
                "gate_location_id":gate_location_id,
                "note":"Driver identity and asset seal verified"
            }),
        )
        .await
}
