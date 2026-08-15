use anyhow::Context;
use axum::http::Method;
use serde_json::json;
use wareboxes_api_contract::v1::ConfigurationResponse;

use super::support::SeedContext;

pub async fn seed(context: &SeedContext) -> anyhow::Result<()> {
    let approver_token = context.configuration_approver_token().await?;

    activate(
        context,
        &approver_token,
        "demo-configuration-allocation-tenant",
        json!({"level":"tenant"}),
        json!({
            "kind":"allocation",
            "rotation":"fifo",
            "allow_partial":false,
            "require_complete_line":true
        }),
    )
    .await?;
    activate(
        context,
        &approver_token,
        "demo-configuration-allocation-owner-facility",
        json!({
            "level":"owner_facility",
            "inventory_owner_id":context.inventory_owner_id,
            "facility_id":context.facility_id
        }),
        json!({
            "kind":"allocation",
            "rotation":"fefo",
            "allow_partial":true,
            "require_complete_line":false
        }),
    )
    .await?;
    activate(
        context,
        &approver_token,
        "demo-configuration-billing-owner",
        json!({
            "level":"inventory_owner",
            "inventory_owner_id":context.inventory_owner_id
        }),
        json!({
            "kind":"billing",
            "event_type":"received_unit",
            "unit":"each",
            "currency":"USD",
            "rate_minor":15,
            "minimum_charge_minor":100
        }),
    )
    .await?;

    let draft: ConfigurationResponse = context
        .command(
            Method::POST,
            "/api/v1/configurations",
            "demo-configuration-pick-draft",
            json!({
                "scope":{"level":"facility","facility_id":context.facility_id},
                "effective_from":"2025-01-01T00:00:00Z",
                "rule":{
                    "kind":"pick",
                    "require_source_location_scan":true,
                    "require_item_scan":true,
                    "require_destination_container_scan":true
                }
            }),
        )
        .await?;
    let _: ConfigurationResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/configurations/{}/submissions",
                draft.configuration_id
            ),
            "demo-configuration-pick-submit",
            json!({"expected_revision":draft.revision}),
        )
        .await?;

    wareboxes_api::auth::destroy_session(&context.db, &approver_token)
        .await
        .context("destroying demo configuration approver session")?;
    Ok(())
}

async fn activate(
    context: &SeedContext,
    approver_token: &str,
    prefix: &str,
    scope: serde_json::Value,
    rule: serde_json::Value,
) -> anyhow::Result<()> {
    let created: ConfigurationResponse = context
        .command(
            Method::POST,
            "/api/v1/configurations",
            &format!("{prefix}-create"),
            json!({
                "scope":scope,
                "effective_from":"2025-01-01T00:00:00Z",
                "rule":rule
            }),
        )
        .await?;
    let submitted: ConfigurationResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/configurations/{}/submissions",
                created.configuration_id
            ),
            &format!("{prefix}-submit"),
            json!({"expected_revision":created.revision}),
        )
        .await?;
    let approved: ConfigurationResponse = context
        .command_as(
            approver_token,
            Method::POST,
            &format!(
                "/api/v1/configurations/{}/approvals",
                created.configuration_id
            ),
            &format!("{prefix}-approve"),
            json!({"expected_revision":submitted.revision}),
        )
        .await?;
    let _: ConfigurationResponse = context
        .command(
            Method::POST,
            &format!(
                "/api/v1/configurations/{}/activations",
                created.configuration_id
            ),
            &format!("{prefix}-activate"),
            json!({"expected_revision":approved.revision}),
        )
        .await?;
    Ok(())
}
