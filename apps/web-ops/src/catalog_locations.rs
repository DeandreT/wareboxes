use std::cmp::Ordering;

use leptos::prelude::*;
use wareboxes_core::dto::{AddLocation, LocationIdRequest, LocationUpdate};
use wareboxes_core::models::{Facility, Location};

use crate::api;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::{use_toast_bus, ToastBus};

use super::{label_or_id, optional_text, CatalogStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LocationSort {
    Id,
    Facility,
    Name,
    Barcode,
    Type,
    Status,
}

#[component]
pub(super) fn LocationCatalog(store: CatalogStore) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let show_inactive = RwSignal::new(false);
    let selected_id = RwSignal::new(None::<i64>);
    let creating = RwSignal::new(false);
    let sort = RwSignal::new(SortSpec {
        key: LocationSort::Facility,
        direction: SortDirection::Ascending,
    });

    view! {
        <div class="catalog-layout">
            <section class="data-section catalog-browser">
                <div class="catalog-toolbar">
                    <SearchField
                        label="Filter locations by name, scan code, type, or facility".to_owned()
                        placeholder="Name, scan code, type, or facility"
                        value=filter
                    />
                    <label class="catalog-check">
                        <input
                            type="checkbox"
                            prop:checked=move || show_inactive.get()
                            on:change=move |event| show_inactive.set(event_target_checked(&event))
                        />
                        <span>"Inactive"</span>
                    </label>
                    <span class="catalog-count">
                        {move || {
                            let data = store.data.get();
                            visible_locations(
                                &data.locations,
                                &data.facilities,
                                &filter.get(),
                                show_inactive.get(),
                                sort.get(),
                            )
                            .len()
                        }}
                        " shown"
                    </span>
                    <button
                        class="button primary-action compact"
                        type="button"
                        on:click=move |_| {
                            selected_id.set(None);
                            creating.set(true);
                        }
                    >
                        "New location"
                    </button>
                </div>
                <div class="table-scroll catalog-table-scroll">
                    <table class="data-table catalog-table location-table">
                        <caption class="sr-only">"Warehouse locations in the current facility scope"</caption>
                        <thead>
                            <tr>
                                <SortableHeader label="ID" active=move || sort.get().key == LocationSort::Id direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, LocationSort::Id)) numeric=true/>
                                <SortableHeader label="Facility" active=move || sort.get().key == LocationSort::Facility direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, LocationSort::Facility))/>
                                <SortableHeader label="Location" active=move || sort.get().key == LocationSort::Name direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, LocationSort::Name))/>
                                <SortableHeader label="Scan code" active=move || sort.get().key == LocationSort::Barcode direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, LocationSort::Barcode))/>
                                <SortableHeader label="Type" active=move || sort.get().key == LocationSort::Type direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, LocationSort::Type))/>
                                <th scope="col">"Capabilities"</th>
                                <SortableHeader label="Status" active=move || sort.get().key == LocationSort::Status direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, LocationSort::Status))/>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let data = store.data.get();
                                let locations = visible_locations(
                                    &data.locations,
                                    &data.facilities,
                                    &filter.get(),
                                    show_inactive.get(),
                                    sort.get(),
                                );
                                if locations.is_empty() {
                                    view! {
                                        <tr><td class="table-empty-row" colspan="7">"No locations match this view."</td></tr>
                                    }
                                    .into_any()
                                } else {
                                    locations
                                        .into_iter()
                                        .map(|location| {
                                            let id = location.id;
                                            let status = location_status(&location);
                                            view! {
                                                <tr class:selected=move || selected_id.get() == Some(id)>
                                                    <td class="numeric muted">{id}</td>
                                                    <td>{facility_label(&data.facilities, location.facility_id)}</td>
                                                    <td>
                                                        <button
                                                            class="catalog-row-link"
                                                            type="button"
                                                            on:click=move |_| {
                                                                creating.set(false);
                                                                selected_id.set(Some(id));
                                                            }
                                                        >
                                                            {location_label(&location)}
                                                        </button>
                                                    </td>
                                                    <td><code>{location.barcode.unwrap_or_else(|| "-".to_owned())}</code></td>
                                                    <td>{location.r#type}</td>
                                                    <td>
                                                        <div class="capability-list">
                                                            {location.pickable.then(|| view! { <span>"Pick"</span> })}
                                                            {location.receivable.then(|| view! { <span>"Receive"</span> })}
                                                        </div>
                                                    </td>
                                                    <td><span class=status.1>{status.0}</span></td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()
                                        .into_any()
                                }
                            }}
                        </tbody>
                    </table>
                </div>
            </section>

            <aside class="data-section catalog-editor" aria-label="Location editor">
                {move || {
                    let data = store.data.get();
                    if creating.get() {
                        view! {
                            <LocationCreate
                                store
                                facilities=data.facilities
                                locations=data.locations
                                on_cancel=Callback::new(move |_| creating.set(false))
                                on_created=Callback::new(move |id| {
                                    creating.set(false);
                                    selected_id.set(Some(id));
                                })
                            />
                        }
                        .into_any()
                    } else if let Some(location) = selected_location(&data.locations, selected_id.get()) {
                        view! {
                            <LocationDetail
                                store
                                location
                                facilities=data.facilities
                                locations=data.locations
                            />
                        }
                        .into_any()
                    } else {
                        view! {
                            <div class="catalog-editor-empty">
                                <strong>"Select a location"</strong>
                                <p>"Review scan identity, hierarchy, and work capabilities."</p>
                            </div>
                        }
                        .into_any()
                    }
                }}
            </aside>
        </div>
    }
}

#[component]
fn LocationCreate(
    store: CatalogStore,
    facilities: Vec<Facility>,
    locations: Vec<Location>,
    on_cancel: Callback<()>,
    on_created: Callback<i64>,
) -> impl IntoView {
    let facility_id = RwSignal::new(
        facilities
            .first()
            .map(|facility| facility.id.to_string())
            .unwrap_or_default(),
    );
    let parent_id = RwSignal::new(String::new());
    let name = RwSignal::new(String::new());
    let barcode = RwSignal::new(String::new());
    let location_type = RwSignal::new("bin".to_owned());
    let active = RwSignal::new(true);
    let pickable = RwSignal::new(true);
    let receivable = RwSignal::new(false);
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let Ok(facility) = facility_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a facility.".to_owned()));
            return;
        };
        let Some(kind) = optional_text(&location_type.get_untracked()) else {
            error.set(Some("Enter a location type.".to_owned()));
            return;
        };
        let parent = parse_optional_id(&parent_id.get_untracked());
        if parent.is_err() {
            error.set(Some("Parent location is invalid.".to_owned()));
            return;
        }
        let request = AddLocation {
            facility_id: facility,
            parent_location_id: parent.ok().flatten(),
            barcode: optional_text(&barcode.get_untracked()),
            name: optional_text(&name.get_untracked()),
            r#type: kind,
            active: Some(active.get_untracked()),
            pickable: Some(pickable.get_untracked()),
            receivable: Some(receivable.get_untracked()),
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/locations/add", &request).await {
                Ok(id) => {
                    toasts.success(format!("Location #{id} created."));
                    store.refresh();
                    on_created.run(id);
                }
                Err(api_error) if api_error.unauthorized => store.on_unauthorized.run(()),
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    error.set(Some(api_error.message));
                    pending.set(false);
                }
            }
        });
    };

    view! {
        <form class="catalog-form" on:submit=submit>
            <div class="catalog-editor-heading">
                <div><p class="eyebrow">"Facility layout"</p><h2>"New location"</h2></div>
                <button class="button quiet-action compact" type="button" on:click=move |_| on_cancel.run(())>"Cancel"</button>
            </div>
            <div class="form-grid two">
                <label>
                    <span>"Facility"</span>
                    <select required prop:value=move || facility_id.get() on:change=move |event| {
                        facility_id.set(event_target_value(&event));
                        parent_id.set(String::new());
                    }>
                        <option value="" disabled>"Choose facility"</option>
                        {facilities
                            .iter()
                            .map(|facility| view! {
                                <option value=facility.id.to_string()>{facility_label(&facilities, facility.id)}</option>
                            })
                            .collect_view()}
                    </select>
                </label>
                <label>
                    <span>"Parent location"</span>
                    <select prop:value=move || parent_id.get() on:change=move |event| parent_id.set(event_target_value(&event))>
                        <option value="">"None"</option>
                        {move || {
                            let facility = facility_id.get().parse::<i64>().ok();
                            locations
                                .iter()
                                .filter(|location| Some(location.facility_id) == facility && location.deleted.is_none())
                                .map(|location| view! {
                                    <option value=location.id.to_string()>{location_label(location)}</option>
                                })
                                .collect_view()
                        }}
                    </select>
                </label>
                <label>
                    <span>"Name"</span>
                    <input type="text" autofocus prop:value=move || name.get() on:input=move |event| name.set(event_target_value(&event))/>
                </label>
                <label>
                    <span>"Scan code"</span>
                    <input type="text" prop:value=move || barcode.get() on:input=move |event| barcode.set(event_target_value(&event))/>
                </label>
                <label>
                    <span>"Location type"</span>
                    <input type="text" required prop:value=move || location_type.get() on:input=move |event| location_type.set(event_target_value(&event))/>
                </label>
            </div>
            <LocationFlags active pickable receivable/>
            <InlineError error/>
            <div class="catalog-form-actions">
                <button class="button primary-action" type="submit" disabled=move || pending.get()>
                    {move || if pending.get() { "Creating..." } else { "Create location" }}
                </button>
            </div>
        </form>
    }
}

#[component]
fn LocationDetail(
    store: CatalogStore,
    location: Location,
    facilities: Vec<Facility>,
    locations: Vec<Location>,
) -> impl IntoView {
    let parent_id = RwSignal::new(
        location
            .parent_location_id
            .map(|id| id.to_string())
            .unwrap_or_default(),
    );
    let name = RwSignal::new(location.name.clone().unwrap_or_default());
    let barcode = RwSignal::new(location.barcode.clone().unwrap_or_default());
    let location_type = RwSignal::new(location.r#type.clone());
    let active = RwSignal::new(location.active);
    let pickable = RwSignal::new(location.pickable);
    let receivable = RwSignal::new(location.receivable);
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let id = location.id;
    let archived = location.deleted.is_some();
    let facility_id = location.facility_id;
    let toasts = use_toast_bus();

    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let Some(kind) = optional_text(&location_type.get_untracked()) else {
            error.set(Some("Enter a location type.".to_owned()));
            return;
        };
        let parent = parse_optional_id(&parent_id.get_untracked());
        if parent.is_err() || parent.ok().flatten() == Some(id) {
            error.set(Some("Choose a valid parent location.".to_owned()));
            return;
        }
        let request = LocationUpdate {
            location_id: id,
            parent_location_id: parse_optional_id(&parent_id.get_untracked()).ok().flatten(),
            barcode: optional_text(&barcode.get_untracked()),
            name: optional_text(&name.get_untracked()),
            r#type: Some(kind),
            active: Some(active.get_untracked()),
            pickable: Some(pickable.get_untracked()),
            receivable: Some(receivable.get_untracked()),
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            handle_bool_command(
                store,
                "/api/locations/update",
                &request,
                format!("Location #{} updated.", request.location_id),
                pending,
                error,
                toasts,
            )
            .await;
        });
    };

    let change_archive = move |_| {
        if pending.get_untracked() {
            return;
        }
        let (path, message) = if archived {
            (
                "/api/locations/restore",
                format!("Location #{id} reactivated."),
            )
        } else {
            (
                "/api/locations/delete",
                format!("Location #{id} deactivated."),
            )
        };
        let request = LocationIdRequest { location_id: id };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            handle_bool_command(store, path, &request, message, pending, error, toasts).await;
        });
    };

    view! {
        <form class="catalog-form compact-form" on:submit=save>
            <div class="catalog-editor-heading">
                <div>
                    <p class="eyebrow">{format!("Location #{id}")}</p>
                    <h2>{location_label(&location)}</h2>
                    <small>{facility_label(&facilities, facility_id)}</small>
                </div>
                <span class=location_status(&location).1>{location_status(&location).0}</span>
            </div>
            <div class="form-grid two">
                <label>
                    <span>"Name"</span>
                    <input type="text" prop:value=move || name.get() on:input=move |event| name.set(event_target_value(&event))/>
                </label>
                <label>
                    <span>"Scan code"</span>
                    <input type="text" prop:value=move || barcode.get() on:input=move |event| barcode.set(event_target_value(&event))/>
                </label>
                <label>
                    <span>"Type"</span>
                    <input type="text" required prop:value=move || location_type.get() on:input=move |event| location_type.set(event_target_value(&event))/>
                </label>
                <label>
                    <span>"Parent location"</span>
                    <select prop:value=move || parent_id.get() on:change=move |event| parent_id.set(event_target_value(&event))>
                        <option value="">"No change"</option>
                        {locations
                            .iter()
                            .filter(|candidate| {
                                candidate.facility_id == facility_id
                                    && candidate.id != id
                                    && candidate.deleted.is_none()
                            })
                            .map(|candidate| view! {
                                <option value=candidate.id.to_string()>{location_label(candidate)}</option>
                            })
                            .collect_view()}
                    </select>
                </label>
            </div>
            <LocationFlags active pickable receivable/>
            <InlineError error/>
            <div class="catalog-form-actions split">
                <button class="button primary-action compact" type="submit" disabled=move || pending.get() || archived>"Save location"</button>
                <button
                    class=if archived { "button secondary-action compact" } else { "button danger-action compact" }
                    type="button"
                    disabled=move || pending.get()
                    on:click=change_archive
                >
                    {if archived { "Reactivate" } else { "Deactivate" }}
                </button>
            </div>
        </form>
    }
}

#[component]
fn LocationFlags(
    active: RwSignal<bool>,
    pickable: RwSignal<bool>,
    receivable: RwSignal<bool>,
) -> impl IntoView {
    view! {
        <fieldset class="catalog-fieldset flag-fieldset">
            <legend>"Work capabilities"</legend>
            <label class="catalog-check"><input type="checkbox" prop:checked=move || active.get() on:change=move |event| active.set(event_target_checked(&event))/><span>"Operational"</span></label>
            <label class="catalog-check"><input type="checkbox" prop:checked=move || pickable.get() on:change=move |event| pickable.set(event_target_checked(&event))/><span>"Pickable"</span></label>
            <label class="catalog-check"><input type="checkbox" prop:checked=move || receivable.get() on:change=move |event| receivable.set(event_target_checked(&event))/><span>"Receivable"</span></label>
        </fieldset>
    }
}

#[component]
fn InlineError(error: RwSignal<Option<String>>) -> impl IntoView {
    move || {
        error
            .get()
            .map(|message| view! { <div class="catalog-inline-error" role="alert">{message}</div> })
    }
}

async fn handle_bool_command<T>(
    store: CatalogStore,
    path: &'static str,
    request: &T,
    success: String,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    toasts: ToastBus,
) where
    T: serde::Serialize,
{
    match api::internal_post::<_, bool>(path, request).await {
        Ok(true) => {
            toasts.success(success);
            pending.set(false);
            store.refresh();
        }
        Ok(false) => {
            let message = "The location changed or is no longer in your scope.".to_owned();
            toasts.error(message.clone());
            error.set(Some(message));
            pending.set(false);
        }
        Err(api_error) if api_error.unauthorized => store.on_unauthorized.run(()),
        Err(api_error) => {
            toasts.error(api_error.message.clone());
            error.set(Some(api_error.message));
            pending.set(false);
        }
    }
}

fn selected_location(locations: &[Location], id: Option<i64>) -> Option<Location> {
    locations
        .iter()
        .find(|location| Some(location.id) == id)
        .cloned()
}

fn visible_locations(
    locations: &[Location],
    facilities: &[Facility],
    filter: &str,
    show_inactive: bool,
    sort: SortSpec<LocationSort>,
) -> Vec<Location> {
    let query = filter.trim().to_ascii_lowercase();
    let mut visible = locations
        .iter()
        .filter(|location| {
            (show_inactive || (location.deleted.is_none() && location.active))
                && (query.is_empty()
                    || location_label(location)
                        .to_ascii_lowercase()
                        .contains(&query)
                    || location
                        .barcode
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains(&query)
                    || location.r#type.to_ascii_lowercase().contains(&query)
                    || facility_label(facilities, location.facility_id)
                        .to_ascii_lowercase()
                        .contains(&query))
        })
        .cloned()
        .collect::<Vec<_>>();
    visible.sort_by(|left, right| {
        let ordering = compare_locations(left, right, facilities, sort.key)
            .then_with(|| left.id.cmp(&right.id));
        if sort.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    visible
}

fn compare_locations(
    left: &Location,
    right: &Location,
    facilities: &[Facility],
    key: LocationSort,
) -> Ordering {
    match key {
        LocationSort::Id => left.id.cmp(&right.id),
        LocationSort::Facility => facility_label(facilities, left.facility_id)
            .to_ascii_lowercase()
            .cmp(&facility_label(facilities, right.facility_id).to_ascii_lowercase()),
        LocationSort::Name => location_label(left)
            .to_ascii_lowercase()
            .cmp(&location_label(right).to_ascii_lowercase()),
        LocationSort::Barcode => {
            normalized(left.barcode.as_deref()).cmp(&normalized(right.barcode.as_deref()))
        }
        LocationSort::Type => left.r#type.cmp(&right.r#type),
        LocationSort::Status => location_status_rank(left).cmp(&location_status_rank(right)),
    }
}

fn facility_label(facilities: &[Facility], facility_id: i64) -> String {
    let name = facilities
        .iter()
        .find(|facility| facility.id == facility_id)
        .and_then(|facility| facility.name.as_deref());
    label_or_id(name, "Facility", facility_id)
}

fn location_label(location: &Location) -> String {
    location
        .name
        .as_deref()
        .or(location.barcode.as_deref())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Location #{}", location.id))
}

fn location_status(location: &Location) -> (&'static str, &'static str) {
    if location.deleted.is_some() {
        ("Inactive", "status muted")
    } else if !location.active {
        ("Closed", "status held")
    } else {
        ("Active", "status open")
    }
}

fn location_status_rank(location: &Location) -> u8 {
    if location.deleted.is_some() {
        2
    } else if !location.active {
        1
    } else {
        0
    }
}

fn parse_optional_id(value: &str) -> Result<Option<i64>, ()> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|id| *id > 0)
        .map(Some)
        .ok_or(())
}

fn normalized(value: Option<&str>) -> String {
    value.unwrap_or_default().trim().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::parse_optional_id;

    #[test]
    fn optional_parent_ids_are_positive() {
        assert_eq!(parse_optional_id(""), Ok(None));
        assert_eq!(parse_optional_id(" 42 "), Ok(Some(42)));
        assert_eq!(parse_optional_id("0"), Err(()));
        assert_eq!(parse_optional_id("-1"), Err(()));
    }
}
