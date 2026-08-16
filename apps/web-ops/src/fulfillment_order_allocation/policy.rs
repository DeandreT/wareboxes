use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    AllocationPolicyResponse, AllocationPolicySource, ConfigurationScope, OrderAllocationStrategy,
};

#[component]
pub(super) fn AllocationPolicyBadge(policy: AllocationPolicyResponse) -> impl IntoView {
    let title = policy_title(&policy);
    let summary = policy_summary(&policy);
    view! { <span class="allocation-policy-badge" title=title>{summary}</span> }
}

#[component]
pub(super) fn CommittedAllocationPolicy(
    run_id: i64,
    policy: AllocationPolicyResponse,
) -> impl IntoView {
    let hash = short_hash(&policy.policy_hash);
    let summary = policy_summary(&policy);
    view! {
        <div class="allocation-policy-evidence" role="status">
            <strong>{format!("Run #{run_id} policy")}</strong>
            <span>{summary}</span>
            <code>{hash}</code>
        </div>
    }
}

pub(super) fn allocation_action_title(policy: Option<&AllocationPolicyResponse>) -> String {
    policy.map_or_else(
        || "Allocate eligible stock using the resolved policy".to_owned(),
        |policy| format!("Allocate eligible stock using {}", policy_summary(policy)),
    )
}

fn policy_summary(policy: &AllocationPolicyResponse) -> String {
    let rotation = match policy.strategy {
        OrderAllocationStrategy::Fifo => "FIFO",
        OrderAllocationStrategy::Fefo => "FEFO",
    };
    let behavior = if policy.require_complete_line {
        "complete lines"
    } else if policy.allow_partial {
        "partial allowed"
    } else {
        "complete order"
    };
    match policy.source {
        AllocationPolicySource::ProductDefault => {
            format!("{rotation} · {behavior} · product default")
        }
        AllocationPolicySource::Configuration => format!(
            "{rotation} · {behavior} · {} rev {}",
            policy
                .configuration_scope
                .map(scope_label)
                .unwrap_or("configured"),
            policy
                .configuration_revision
                .map_or_else(|| "?".to_owned(), |revision| revision.to_string())
        ),
    }
}

fn policy_title(policy: &AllocationPolicyResponse) -> String {
    let identity = policy.configuration_id.map_or_else(
        || "Product allocation default".to_owned(),
        |configuration_id| format!("Configuration #{configuration_id}"),
    );
    format!(
        "{identity}; policy hash {}",
        short_hash(&policy.policy_hash)
    )
}

const fn scope_label(scope: ConfigurationScope) -> &'static str {
    match scope {
        ConfigurationScope::Tenant => "tenant",
        ConfigurationScope::InventoryOwner { .. } => "client",
        ConfigurationScope::Facility { .. } => "facility",
        ConfigurationScope::OwnerFacility { .. } => "client + facility",
    }
}

fn short_hash(hash: &str) -> String {
    hash.chars().take(12).collect()
}

#[cfg(test)]
mod tests {
    use wareboxes_api_contract::v1::Revision;

    use super::*;

    #[test]
    fn policy_summary_distinguishes_default_fifo_and_configured_scope() {
        let default = AllocationPolicyResponse {
            source: AllocationPolicySource::ProductDefault,
            configuration_id: None,
            configuration_revision: None,
            configuration_scope: None,
            strategy: OrderAllocationStrategy::Fefo,
            allow_partial: true,
            require_complete_line: false,
            policy_hash: "a".repeat(64),
        };
        assert_eq!(
            policy_summary(&default),
            "FEFO · partial allowed · product default"
        );
        let configured = AllocationPolicyResponse {
            source: AllocationPolicySource::Configuration,
            configuration_id: Some(7),
            configuration_revision: Some(Revision::new(4).unwrap()),
            configuration_scope: Some(ConfigurationScope::OwnerFacility {
                inventory_owner_id: 2,
                facility_id: 3,
            }),
            strategy: OrderAllocationStrategy::Fifo,
            allow_partial: false,
            require_complete_line: true,
            policy_hash: "b".repeat(64),
        };
        assert_eq!(
            policy_summary(&configured),
            "FIFO · complete lines · client + facility rev 4"
        );
    }
}
