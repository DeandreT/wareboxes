mod view;

use leptos::prelude::*;
use lucide_leptos::{Boxes, RefreshCw};
use wareboxes_api_contract::v1::PickZoneWorkspaceResponse;
use wareboxes_api_contract::web::access::{AccessScopeResource, AccessScopeWorkspace};

use crate::api;

use self::view::queue_table;

#[derive(Clone, Copy)]
struct Signals {
    workspace: RwSignal<Option<PickZoneWorkspaceResponse>>,
    facility_id: RwSignal<Option<i64>>,
    owner_id: RwSignal<Option<i64>>,
    loading: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
}

#[component]
pub(crate) fn PickZonesWorkspace(
    access: AccessScopeWorkspace,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let signals = Signals {
        workspace: RwSignal::new(None),
        facility_id: RwSignal::new(single_id(&access.facilities)),
        owner_id: RwSignal::new(single_id(&access.inventory_owners)),
        loading: RwSignal::new(true),
        error: RwSignal::new(None),
        generation: RwSignal::new(0),
    };
    let facilities = StoredValue::new(access.facilities);
    let owners = StoredValue::new(access.inventory_owners);
    Effect::new(move |_| request_workspace(signals, on_unauthorized));

    view! {
        <section class="pick-zone-panel data-section">
            <header class="pick-zone-heading">
                <div><span class="eyebrow">"Scanner-directed execution"</span><h2><Boxes size=18/>"Pick zones"</h2><p>"Monitor exact zone queues and use the immutable zone ID on RF devices for shift handoff."</p></div>
                <button type="button" class="icon-button" title="Refresh zone queues" aria-label="Refresh zone queues" disabled=move || signals.loading.get() on:click=move |_| request_workspace(signals,on_unauthorized)><RefreshCw size=15/></button>
            </header>
            <div class="pick-zone-toolbar">
                <label><span>"Facility"</span><select prop:value=move || optional_value(signals.facility_id.get()) on:change=move |event| { signals.facility_id.set(parse_id(&event_target_value(&event))); request_workspace(signals,on_unauthorized); }><option value="">"Select facility"</option>{resource_options(facilities)}</select></label>
                <label><span>"Client"</span><select prop:value=move || optional_value(signals.owner_id.get()) on:change=move |event| { signals.owner_id.set(parse_id(&event_target_value(&event))); request_workspace(signals,on_unauthorized); }><option value="">"Select client"</option>{resource_options(owners)}</select></label>
            </div>
            <Show when=move || signals.error.get().is_some()><p class="inline-command-error" role="alert">{move || signals.error.get().unwrap_or_default()}</p></Show>
            {move || {
                if signals.loading.get() {
                    view! { <div class="workspace-state compact"><h3>"Loading zone queues"</h3></div> }.into_any()
                } else if let Some(workspace)=signals.workspace.get() {
                    queue_table(workspace).into_any()
                } else if signals.error.get().is_some() {
                    view! { <div class="workspace-empty"><h3>"Zone queues unavailable"</h3><p>"Review the error above, then refresh this workspace."</p></div> }.into_any()
                } else {
                    view! { <div class="workspace-empty"><h3>"Select an exact scope"</h3><p>"Choose one facility and one client to read pick-zone demand."</p></div> }.into_any()
                }
            }}
        </section>
    }
}

fn request_workspace(signals: Signals, on_unauthorized: Callback<()>) {
    let generation = signals.generation.get_untracked().wrapping_add(1);
    signals.generation.set(generation);
    signals.error.set(None);
    let (Some(facility_id), Some(owner_id)) = (
        signals.facility_id.get_untracked(),
        signals.owner_id.get_untracked(),
    ) else {
        signals.workspace.set(None);
        signals.loading.set(false);
        return;
    };
    signals.loading.set(true);
    leptos::task::spawn_local(async move {
        match api::pick_zone_workspace(facility_id, owner_id).await {
            Ok(workspace) if signals.generation.get_untracked() == generation => {
                signals.workspace.set(Some(workspace));
                signals.loading.set(false);
            }
            Ok(_) => {}
            Err(_) if signals.generation.get_untracked() != generation => {}
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) => {
                signals.error.set(Some(error.message));
                signals.loading.set(false);
            }
        }
    });
}

fn resource_options(resources: StoredValue<Vec<AccessScopeResource>>) -> impl IntoView {
    resources.with_value(|items| {
        items
            .iter()
            .map(|item| view! { <option value=item.id>{item.name.clone()}</option> })
            .collect_view()
    })
}

fn single_id(resources: &[AccessScopeResource]) -> Option<i64> {
    (resources.len() == 1).then(|| resources[0].id)
}

fn optional_value(value: Option<i64>) -> String {
    value.map_or_else(String::new, |id| id.to_string())
}

fn parse_id(value: &str) -> Option<i64> {
    value.parse::<i64>().ok().filter(|id| *id > 0)
}
