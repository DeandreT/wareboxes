use leptos::prelude::*;
use wareboxes_api_contract::v1::PickZoneWorkspaceResponse;

pub(super) fn queue_table(workspace: PickZoneWorkspaceResponse) -> impl IntoView {
    let total_open = workspace
        .queues
        .iter()
        .map(|queue| queue.open_task_count)
        .sum::<i64>();
    let total_active = workspace
        .queues
        .iter()
        .map(|queue| queue.active_task_count)
        .sum::<i64>();
    view! {
        <div class="pick-zone-summary"><span><strong>{workspace.queues.len()}</strong>" active zones"</span><span><strong>{total_open}</strong>" ready tasks"</span><span><strong>{total_active}</strong>" active picks"</span></div>
        <div class="table-scroll"><table class="data-table pick-zone-table"><caption class="sr-only">"Active pick zones and scoped task demand"</caption><thead><tr><th>"Route"</th><th>"RF identity"</th><th class="numeric">"Ready"</th><th class="numeric">"Active"</th><th>"Oldest ready"</th></tr></thead><tbody>{if workspace.queues.is_empty() { view! { <tr><td colspan="5" class="table-empty-row">"No active pick zones are configured for this facility."</td></tr> }.into_any() } else { workspace.queues.into_iter().map(|queue| view! { <tr><td><strong>{queue.code}</strong><small class="cell-detail">{queue.name}</small></td><td><code>{format!("Zone ID {}",queue.storage_zone_id)}</code><small class="cell-detail">{format!("Revision {} · route {}",queue.revision,queue.travel_sequence)}</small></td><td class="numeric"><strong>{queue.open_task_count}</strong></td><td class="numeric">{queue.active_task_count}</td><td>{queue.oldest_open_task_at.as_deref().map(compact_time).unwrap_or_else(|| "—".into())}</td></tr> }).collect_view().into_any() }}</tbody></table></div>
    }
}

fn compact_time(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compact_time_keeps_operator_relevant_precision() {
        assert_eq!(compact_time("2026-08-16T11:42:33Z"), "2026-08-16 11:42");
    }
}
