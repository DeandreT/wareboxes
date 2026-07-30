use leptos::prelude::*;

#[path = "admin_clients.rs"]
mod clients;
#[path = "admin_count_plans.rs"]
mod count_plans;
#[path = "admin_security.rs"]
mod security;
#[path = "admin_workforce.rs"]
mod workforce;

pub use clients::ClientsWorkbench;
pub use count_plans::CountPlansWorkbench;
pub use security::{PermissionsWorkbench, RolesWorkbench, UsersWorkbench};
pub use workforce::EmployeesWorkbench;

use wareboxes_core::models::{Facility, InventoryOwner};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdministrationArea {
    Clients,
    Users,
    Roles,
    Permissions,
    Employees,
    CountPlans,
}

impl AdministrationArea {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Clients => "Clients",
            Self::Users => "Users",
            Self::Roles => "Roles",
            Self::Permissions => "Permissions",
            Self::Employees => "Employees",
            Self::CountPlans => "Count plans",
        }
    }

    pub const fn eyebrow(self) -> &'static str {
        match self {
            Self::Clients => "Warehouse network",
            Self::Users | Self::Roles | Self::Permissions => "Organization security",
            Self::Employees => "Warehouse workforce",
            Self::CountPlans => "Inventory accuracy",
        }
    }
}

#[component]
pub fn AdministrationWorkspace(
    area: AdministrationArea,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    view! {
        <section class="page-heading">
            <div>
                <p class="eyebrow">{area.eyebrow()}</p>
                <h1>{area.title()}</h1>
                <p>{area_description(area)}</p>
            </div>
        </section>
        {match area {
            AdministrationArea::Clients => {
                view! { <ClientsWorkbench on_unauthorized/> }.into_any()
            }
            AdministrationArea::Users => {
                view! { <UsersWorkbench on_unauthorized/> }.into_any()
            }
            AdministrationArea::Roles => {
                view! { <RolesWorkbench on_unauthorized/> }.into_any()
            }
            AdministrationArea::Permissions => {
                view! { <PermissionsWorkbench on_unauthorized/> }.into_any()
            }
            AdministrationArea::Employees => {
                view! { <EmployeesWorkbench on_unauthorized/> }.into_any()
            }
            AdministrationArea::CountPlans => {
                view! { <CountPlansWorkbench on_unauthorized/> }.into_any()
            }
        }}
    }
}

const fn area_description(area: AdministrationArea) -> &'static str {
    match area {
        AdministrationArea::Clients => {
            "Maintain client contacts and the facilities where their stock may operate."
        }
        AdministrationArea::Users => "Maintain organization members and assign operational roles.",
        AdministrationArea::Roles => {
            "Compose warehouse responsibilities from permissions and role hierarchy."
        }
        AdministrationArea::Permissions => {
            "Maintain the permission catalog used by organization roles."
        }
        AdministrationArea::Employees => {
            "Maintain warehouse employees and their facility assignments."
        }
        AdministrationArea::CountPlans => {
            "Plan facility and client inventory counts and review recorded count lines."
        }
    }
}

#[component]
pub(super) fn WorkbenchLoading(label: &'static str) -> impl IntoView {
    view! {
        <section class="admin-state" aria-live="polite" aria-busy="true">
            <span class="loading-line" aria-hidden="true"></span>
            <strong>{format!("Loading {label}")}</strong>
        </section>
    }
}

#[component]
pub(super) fn WorkbenchError(message: String, retry: Callback<()>) -> impl IntoView {
    view! {
        <section class="admin-state admin-error" role="alert">
            <strong>"Could not load this workspace"</strong>
            <span>{message}</span>
            <button
                type="button"
                class="button secondary-action compact"
                on:click=move |_| retry.run(())
            >
                "Retry"
            </button>
        </section>
    }
}

#[component]
pub(super) fn InlineCommandError(message: ReadSignal<Option<String>>) -> impl IntoView {
    view! {
        {move || {
            message.get().map(|message| {
                view! {
                    <p class="inline-command-error" role="alert">{message}</p>
                }
            })
        }}
    }
}

#[component]
pub(super) fn DeletedToggle(show_deleted: RwSignal<bool>) -> impl IntoView {
    view! {
        <label class="admin-toggle">
            <input
                type="checkbox"
                prop:checked=move || show_deleted.get()
                on:change=move |event| show_deleted.set(event_target_checked(&event))
            />
            <span>"Include inactive"</span>
        </label>
    }
}

#[component]
pub(super) fn FacilityChecks(
    facilities: Signal<Vec<Facility>>,
    selected: RwSignal<Vec<i64>>,
    legend: &'static str,
) -> impl IntoView {
    view! {
        <fieldset class="admin-check-grid">
            <legend>{legend}</legend>
            {move || {
                facilities
                    .get()
                    .into_iter()
                    .filter(|facility| facility.deleted.is_none())
                    .map(|facility| {
                        let id = facility.id;
                        let label = facility_label(&facility);
                        view! {
                            <label>
                                <input
                                    type="checkbox"
                                    value=id
                                    prop:checked=move || selected.get().contains(&id)
                                    on:change=move |event| {
                                        let checked = event_target_checked(&event);
                                        selected.update(|ids| set_membership(ids, id, checked));
                                    }
                                />
                                <span>{label}</span>
                            </label>
                        }
                    })
                    .collect_view()
            }}
        </fieldset>
    }
}

#[component]
pub(super) fn ClientPicker(
    clients: Signal<Vec<InventoryOwner>>,
    selected: RwSignal<String>,
    id: &'static str,
    label: &'static str,
) -> impl IntoView {
    view! {
        <label>
            <span>{label}</span>
            <select
                id=id
                required
                prop:value=move || selected.get()
                on:change=move |event| selected.set(event_target_value(&event))
            >
                <option value="">"Select client"</option>
                {move || {
                    clients
                        .get()
                        .into_iter()
                        .filter(|client| client.deleted.is_none())
                        .map(|client| {
                            view! {
                                <option value=client.id.to_string()>{client.name}</option>
                            }
                        })
                        .collect_view()
                }}
            </select>
        </label>
    }
}

#[component]
pub(super) fn FacilityPicker(
    facilities: Signal<Vec<Facility>>,
    selected: RwSignal<String>,
    id: &'static str,
    label: &'static str,
) -> impl IntoView {
    view! {
        <label>
            <span>{label}</span>
            <select
                id=id
                required
                prop:value=move || selected.get()
                on:change=move |event| selected.set(event_target_value(&event))
            >
                <option value="">"Select facility"</option>
                {move || {
                    facilities
                        .get()
                        .into_iter()
                        .filter(|facility| facility.deleted.is_none())
                        .map(|facility| {
                            view! {
                                <option value=facility.id.to_string()>
                                    {facility_label(&facility)}
                                </option>
                            }
                        })
                        .collect_view()
                }}
            </select>
        </label>
    }
}

pub(super) fn facility_label(facility: &Facility) -> String {
    facility
        .name
        .clone()
        .unwrap_or_else(|| format!("Facility {}", facility.id))
}

pub(super) fn facility_names(ids: &[i64], facilities: &[Facility]) -> String {
    let names = ids
        .iter()
        .filter_map(|id| facilities.iter().find(|facility| facility.id == *id))
        .map(facility_label)
        .collect::<Vec<_>>();
    if names.is_empty() {
        "No facilities".to_owned()
    } else {
        names.join(", ")
    }
}

pub(super) fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

pub(super) fn selected_id(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|id| *id > 0)
}

pub(super) fn set_membership(ids: &mut Vec<i64>, id: i64, selected: bool) {
    if selected {
        if !ids.contains(&id) {
            ids.push(id);
            ids.sort_unstable();
        }
    } else {
        ids.retain(|current| *current != id);
    }
}

pub(super) fn status_class(inactive: bool) -> &'static str {
    if inactive {
        "status muted"
    } else {
        "status open"
    }
}

pub(super) fn command_result(ok: bool, not_found: &'static str) -> Result<(), String> {
    ok.then_some(()).ok_or_else(|| not_found.to_owned())
}

#[cfg(test)]
mod tests {
    use super::{optional_text, selected_id, set_membership};

    #[test]
    fn option_helpers_reject_empty_or_invalid_values() {
        assert_eq!(optional_text("  "), None);
        assert_eq!(optional_text(" Dock lead "), Some("Dock lead".to_owned()));
        assert_eq!(selected_id("0"), None);
        assert_eq!(selected_id("abc"), None);
        assert_eq!(selected_id("19"), Some(19));
    }

    #[test]
    fn membership_is_sorted_and_unique() {
        let mut ids = vec![5, 9];
        set_membership(&mut ids, 3, true);
        set_membership(&mut ids, 5, true);
        assert_eq!(ids, vec![3, 5, 9]);
        set_membership(&mut ids, 5, false);
        assert_eq!(ids, vec![3, 9]);
    }
}
