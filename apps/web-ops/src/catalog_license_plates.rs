use std::cmp::Ordering;

use leptos::prelude::*;
use wareboxes_core::dto::{AddLicensePlate, LicensePlateIdRequest, LicensePlateUpdate};
use wareboxes_core::models::{Facility, InventoryOwner, LicensePlate, Location};

use crate::api;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::{use_toast_bus, ToastBus};
use crate::view_model::format_quantity;

use super::{label_or_id, optional_text, CatalogStore};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PlateSort {
    Id,
    Barcode,
    Client,
    Facility,
    Location,
    Units,
    Status,
}

#[component]
pub(super) fn LicensePlateCatalog(store: CatalogStore) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let lookup = RwSignal::new(String::new());
    let show_inactive = RwSignal::new(false);
    let selected_id = RwSignal::new(None::<i64>);
    let creating = RwSignal::new(false);
    let lookup_pending = RwSignal::new(false);
    let lookup_error = RwSignal::new(None::<String>);
    let sort = RwSignal::new(SortSpec {
        key: PlateSort::Id,
        direction: SortDirection::Descending,
    });
    let toasts = use_toast_bus();

    let lookup_barcode = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let value = lookup.get_untracked().trim().to_owned();
        if value.is_empty() || lookup_pending.get_untracked() {
            return;
        }
        lookup_pending.set(true);
        lookup_error.set(None);
        let path = format!(
            "/api/license-plates/barcode/{}",
            encode_path_segment(&value)
        );
        leptos::task::spawn_local(async move {
            match api::internal_get::<Option<LicensePlate>>(&path).await {
                Ok(Some(plate)) => {
                    selected_id.set(Some(plate.id));
                    creating.set(false);
                    lookup_pending.set(false);
                    toasts.info(format!("License plate #{} located.", plate.id));
                }
                Ok(None) => {
                    lookup_error.set(Some(format!("No active license plate matches {value}.")));
                    lookup_pending.set(false);
                }
                Err(error) if error.unauthorized => store.on_unauthorized.run(()),
                Err(error) => {
                    toasts.error(error.message.clone());
                    lookup_error.set(Some(error.message));
                    lookup_pending.set(false);
                }
            }
        });
    };

    view! {
        <div class="catalog-layout">
            <section class="data-section catalog-browser">
                <div class="catalog-toolbar plate-toolbar">
                    <SearchField
                        label="Filter license plates".to_owned()
                        placeholder="Filter plates, clients, facilities, or locations"
                        value=filter
                    />
                    <form class="plate-lookup" on:submit=lookup_barcode>
                        <label>
                            <span class="sr-only">"Exact license plate scan code"</span>
                            <input
                                type="search"
                                placeholder="Exact scan lookup"
                                prop:value=move || lookup.get()
                                on:input=move |event| lookup.set(event_target_value(&event))
                            />
                        </label>
                        <button class="button secondary-action compact" type="submit" disabled=move || lookup_pending.get()>
                            {move || if lookup_pending.get() { "Looking..." } else { "Find" }}
                        </button>
                    </form>
                    <label class="catalog-check">
                        <input type="checkbox" prop:checked=move || show_inactive.get() on:change=move |event| show_inactive.set(event_target_checked(&event))/>
                        <span>"Inactive"</span>
                    </label>
                    <button
                        class="button primary-action compact"
                        type="button"
                        on:click=move |_| {
                            selected_id.set(None);
                            creating.set(true);
                        }
                    >
                        "New plate"
                    </button>
                </div>
                {move || lookup_error.get().map(|message| view! {
                    <div class="catalog-inline-error lookup-error" role="alert">{message}</div>
                })}
                <div class="table-scroll catalog-table-scroll">
                    <table class="data-table catalog-table plate-table">
                        <caption class="sr-only">"License plates in the current client and facility scope"</caption>
                        <thead>
                            <tr>
                                <SortableHeader label="ID" active=move || sort.get().key == PlateSort::Id direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlateSort::Id)) numeric=true/>
                                <SortableHeader label="Scan code" active=move || sort.get().key == PlateSort::Barcode direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlateSort::Barcode))/>
                                <SortableHeader label="Client" active=move || sort.get().key == PlateSort::Client direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlateSort::Client))/>
                                <SortableHeader label="Facility" active=move || sort.get().key == PlateSort::Facility direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlateSort::Facility))/>
                                <SortableHeader label="Location" active=move || sort.get().key == PlateSort::Location direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlateSort::Location))/>
                                <SortableHeader label="Units" active=move || sort.get().key == PlateSort::Units direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlateSort::Units)) numeric=true/>
                                <SortableHeader label="Status" active=move || sort.get().key == PlateSort::Status direction=move || sort.get().direction on_sort=Callback::new(move |_| SortSpec::select(sort, PlateSort::Status))/>
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let data = store.data.get();
                                let plates = visible_plates(
                                    &data.license_plates,
                                    &data.clients,
                                    &data.facilities,
                                    &data.locations,
                                    &filter.get(),
                                    show_inactive.get(),
                                    sort.get(),
                                );
                                if plates.is_empty() {
                                    view! {
                                        <tr><td class="table-empty-row" colspan="7">"No license plates match this view."</td></tr>
                                    }
                                    .into_any()
                                } else {
                                    plates
                                        .into_iter()
                                        .map(|plate| {
                                            let id = plate.id;
                                            let inactive = plate.deleted.is_some();
                                            view! {
                                                <tr class:selected=move || selected_id.get() == Some(id)>
                                                    <td class="numeric muted">{id}</td>
                                                    <td>
                                                        <button
                                                            class="catalog-row-link mono"
                                                            type="button"
                                                            on:click=move |_| {
                                                                creating.set(false);
                                                                selected_id.set(Some(id));
                                                            }
                                                        >
                                                            {plate.barcode.clone().unwrap_or_else(|| "Unlabeled".to_owned())}
                                                        </button>
                                                    </td>
                                                    <td>{client_label(&data.clients, plate.inventory_owner_id.get())}</td>
                                                    <td>{facility_label(&data.facilities, plate.facility_id)}</td>
                                                    <td>{plate.location_id.map_or_else(|| "Not placed".to_owned(), |location_id| location_label(&data.locations, location_id))}</td>
                                                    <td class="numeric">{format_quantity(plate_units(&plate))}</td>
                                                    <td><span class=if inactive { "status muted" } else { "status open" }>{if inactive { "Inactive" } else { "Active" }}</span></td>
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

            <aside class="data-section catalog-editor plate-editor" aria-label="License plate editor">
                {move || {
                    let data = store.data.get();
                    if creating.get() {
                        view! {
                            <PlateCreate
                                store
                                clients=data.clients
                                facilities=data.facilities
                                on_cancel=Callback::new(move |_| creating.set(false))
                                on_created=Callback::new(move |id| {
                                    creating.set(false);
                                    selected_id.set(Some(id));
                                })
                            />
                        }
                        .into_any()
                    } else if let Some(plate) = selected_plate(&data.license_plates, selected_id.get()) {
                        view! {
                            <PlateDetail
                                store
                                plate
                                clients=data.clients
                                facilities=data.facilities
                                locations=data.locations
                            />
                        }
                        .into_any()
                    } else {
                        view! {
                            <div class="catalog-editor-empty">
                                <strong>"Select a license plate"</strong>
                                <p>"Review its scan identity, client ownership, position, and inventory contents."</p>
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
fn PlateCreate(
    store: CatalogStore,
    clients: Vec<InventoryOwner>,
    facilities: Vec<Facility>,
    on_cancel: Callback<()>,
    on_created: Callback<i64>,
) -> impl IntoView {
    let facility_id = RwSignal::new(
        facilities
            .first()
            .map(|facility| facility.id.to_string())
            .unwrap_or_default(),
    );
    let client_id = RwSignal::new(String::new());
    let barcode = RwSignal::new(String::new());
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
        let Ok(client) = client_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose a client.".to_owned()));
            return;
        };
        let request = AddLicensePlate {
            inventory_owner_id: client,
            facility_id: facility,
            barcode: optional_text(&barcode.get_untracked()),
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/license-plates/add", &request).await {
                Ok(id) => {
                    toasts.success(format!("License plate #{id} created."));
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
                <div><p class="eyebrow">"Container control"</p><h2>"New license plate"</h2></div>
                <button class="button quiet-action compact" type="button" on:click=move |_| on_cancel.run(())>"Cancel"</button>
            </div>
            <label>
                <span>"Facility"</span>
                <select required prop:value=move || facility_id.get() on:change=move |event| {
                    facility_id.set(event_target_value(&event));
                    client_id.set(String::new());
                }>
                    <option value="" disabled>"Choose facility"</option>
                    {facilities.iter().map(|facility| view! {
                        <option value=facility.id.to_string()>{facility_label(&facilities, facility.id)}</option>
                    }).collect_view()}
                </select>
            </label>
            <label>
                <span>"Client"</span>
                <select required prop:value=move || client_id.get() on:change=move |event| client_id.set(event_target_value(&event))>
                    <option value="">"Choose client"</option>
                    {move || {
                        let facility = facility_id.get().parse::<i64>().ok();
                        clients
                            .iter()
                            .filter(|client| {
                                facility.is_some_and(|facility| {
                                    client
                                        .inventory_owner_facilities
                                        .iter()
                                        .any(|allowed| allowed.id == facility)
                                })
                            })
                            .map(|client| view! {
                                <option value=client.id.to_string()>{client.name.clone()}</option>
                            })
                            .collect_view()
                    }}
                </select>
            </label>
            <label>
                <span>"Scan code"</span>
                <input type="text" autofocus prop:value=move || barcode.get() on:input=move |event| barcode.set(event_target_value(&event))/>
            </label>
            <InlineError error/>
            <div class="catalog-form-actions">
                <button class="button primary-action" type="submit" disabled=move || pending.get()>
                    {move || if pending.get() { "Creating..." } else { "Create license plate" }}
                </button>
            </div>
        </form>
    }
}

#[component]
fn PlateDetail(
    store: CatalogStore,
    plate: LicensePlate,
    clients: Vec<InventoryOwner>,
    facilities: Vec<Facility>,
    locations: Vec<Location>,
) -> impl IntoView {
    let barcode = RwSignal::new(plate.barcode.clone().unwrap_or_default());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let id = plate.id;
    let inactive = plate.deleted.is_some();
    let toasts = use_toast_bus();

    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let Some(value) = optional_text(&barcode.get_untracked()) else {
            error.set(Some("Enter a scan code.".to_owned()));
            return;
        };
        let request = LicensePlateUpdate {
            license_plate_id: id,
            barcode: Some(value.clone()),
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            handle_bool_command(
                store,
                "/api/license-plates/update",
                &request,
                format!("License plate #{id} scan code updated to {value}."),
                pending,
                error,
                toasts,
            )
            .await;
        });
    };

    let change_active = move |_| {
        if pending.get_untracked() {
            return;
        }
        let (path, message) = if inactive {
            (
                "/api/license-plates/restore",
                format!("License plate #{id} reactivated."),
            )
        } else {
            (
                "/api/license-plates/delete",
                format!("License plate #{id} deactivated."),
            )
        };
        let request = LicensePlateIdRequest {
            license_plate_id: id,
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            handle_bool_command(store, path, &request, message, pending, error, toasts).await;
        });
    };

    let client = client_label(&clients, plate.inventory_owner_id.get());
    let facility = facility_label(&facilities, plate.facility_id);
    let location = plate.location_id.map_or_else(
        || "Not placed".to_owned(),
        |location_id| location_label(&locations, location_id),
    );

    view! {
        <div class="catalog-detail">
            <div class="catalog-editor-heading">
                <div>
                    <p class="eyebrow">{format!("License plate #{id}")}</p>
                    <h2 class="mono">{plate.barcode.clone().unwrap_or_else(|| "Unlabeled".to_owned())}</h2>
                </div>
                <span class=if inactive { "status muted" } else { "status open" }>{if inactive { "Inactive" } else { "Active" }}</span>
            </div>
            <dl class="catalog-facts">
                <div><dt>"Client"</dt><dd>{client}</dd></div>
                <div><dt>"Facility"</dt><dd>{facility}</dd></div>
                <div><dt>"Location"</dt><dd>{location}</dd></div>
                <div><dt>"Movement"</dt><dd><span class="catalog-badge">"RF execution"</span></dd></div>
            </dl>
            <form class="catalog-form compact-form" on:submit=save>
                <label>
                    <span>"Scan code"</span>
                    <input type="text" required prop:value=move || barcode.get() on:input=move |event| barcode.set(event_target_value(&event))/>
                </label>
                <InlineError error/>
                <div class="catalog-form-actions split">
                    <div>
                        <button class="button primary-action compact" type="submit" disabled=move || pending.get() || inactive>"Save code"</button>
                        <button class="button quiet-action compact print-hide" type="button" on:click=move |_| print_page()>"Print"</button>
                    </div>
                    <button
                        class=if inactive { "button secondary-action compact" } else { "button danger-action compact" }
                        type="button"
                        disabled=move || pending.get()
                        on:click=change_active
                    >
                        {if inactive { "Reactivate" } else { "Deactivate" }}
                    </button>
                </div>
            </form>

            <section class="catalog-subsection">
                <div class="catalog-subheading">
                    <h3>"Inventory contents"</h3>
                    <span>{plate.contents.len()}</span>
                </div>
                <div class="table-scroll contents-scroll">
                    <table class="data-table contents-table">
                        <caption class="sr-only">{format!("Inventory currently on license plate #{id}")}</caption>
                        <thead>
                            <tr>
                                <th scope="col">"Balance"</th>
                                <th scope="col">"Batch"</th>
                                <th scope="col">"Disposition"</th>
                                <th scope="col" class="numeric">"On hand"</th>
                                <th scope="col" class="numeric">"Reserved"</th>
                                <th scope="col" class="numeric">"Held"</th>
                            </tr>
                        </thead>
                        <tbody>
                            {if plate.contents.is_empty() {
                                view! { <tr><td class="table-empty-row" colspan="6">"No inventory on this license plate."</td></tr> }.into_any()
                            } else {
                                plate.contents
                                    .into_iter()
                                    .map(|content| view! {
                                        <tr>
                                            <td class="numeric">{content.inventory_balance_id}</td>
                                            <td class="numeric">{content.item_batch_id}</td>
                                            <td><span class=status_class(content.status.as_str())>{content.status.to_string()}</span></td>
                                            <td class="numeric">{format_quantity(content.qty_on_hand)}</td>
                                            <td class="numeric">{format_quantity(content.qty_reserved)}</td>
                                            <td class="numeric">{format_quantity(content.qty_held)}</td>
                                        </tr>
                                    })
                                    .collect_view()
                                    .into_any()
                            }}
                        </tbody>
                    </table>
                </div>
            </section>
        </div>
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
            let message = "The license plate changed or is no longer in your scope.".to_owned();
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

fn visible_plates(
    plates: &[LicensePlate],
    clients: &[InventoryOwner],
    facilities: &[Facility],
    locations: &[Location],
    filter: &str,
    show_inactive: bool,
    sort: SortSpec<PlateSort>,
) -> Vec<LicensePlate> {
    let query = filter.trim().to_ascii_lowercase();
    let mut visible = plates
        .iter()
        .filter(|plate| {
            (show_inactive || plate.deleted.is_none())
                && (query.is_empty()
                    || plate.id.to_string().contains(&query)
                    || normalized(plate.barcode.as_deref()).contains(&query)
                    || client_label(clients, plate.inventory_owner_id.get())
                        .to_ascii_lowercase()
                        .contains(&query)
                    || facility_label(facilities, plate.facility_id)
                        .to_ascii_lowercase()
                        .contains(&query)
                    || plate.location_id.is_some_and(|location_id| {
                        location_label(locations, location_id)
                            .to_ascii_lowercase()
                            .contains(&query)
                    }))
        })
        .cloned()
        .collect::<Vec<_>>();
    visible.sort_by(|left, right| {
        let ordering = compare_plates(left, right, clients, facilities, locations, sort.key)
            .then_with(|| left.id.cmp(&right.id));
        if sort.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    visible
}

fn compare_plates(
    left: &LicensePlate,
    right: &LicensePlate,
    clients: &[InventoryOwner],
    facilities: &[Facility],
    locations: &[Location],
    key: PlateSort,
) -> Ordering {
    match key {
        PlateSort::Id => left.id.cmp(&right.id),
        PlateSort::Barcode => {
            normalized(left.barcode.as_deref()).cmp(&normalized(right.barcode.as_deref()))
        }
        PlateSort::Client => client_label(clients, left.inventory_owner_id.get())
            .to_ascii_lowercase()
            .cmp(&client_label(clients, right.inventory_owner_id.get()).to_ascii_lowercase()),
        PlateSort::Facility => facility_label(facilities, left.facility_id)
            .to_ascii_lowercase()
            .cmp(&facility_label(facilities, right.facility_id).to_ascii_lowercase()),
        PlateSort::Location => left
            .location_id
            .map_or_else(String::new, |id| location_label(locations, id))
            .to_ascii_lowercase()
            .cmp(
                &right
                    .location_id
                    .map_or_else(String::new, |id| location_label(locations, id))
                    .to_ascii_lowercase(),
            ),
        PlateSort::Units => plate_units(left).cmp(&plate_units(right)),
        PlateSort::Status => left.deleted.is_some().cmp(&right.deleted.is_some()),
    }
}

fn selected_plate(plates: &[LicensePlate], id: Option<i64>) -> Option<LicensePlate> {
    plates.iter().find(|plate| Some(plate.id) == id).cloned()
}

fn plate_units(plate: &LicensePlate) -> i64 {
    plate
        .contents
        .iter()
        .map(|content| content.qty_on_hand)
        .sum()
}

fn client_label(clients: &[InventoryOwner], id: i64) -> String {
    clients
        .iter()
        .find(|client| client.id == id)
        .map(|client| client.name.clone())
        .unwrap_or_else(|| format!("Client #{id}"))
}

fn facility_label(facilities: &[Facility], id: i64) -> String {
    let name = facilities
        .iter()
        .find(|facility| facility.id == id)
        .and_then(|facility| facility.name.as_deref());
    label_or_id(name, "Facility", id)
}

fn location_label(locations: &[Location], id: i64) -> String {
    locations
        .iter()
        .find(|location| location.id == id)
        .map(|location| {
            location
                .name
                .as_deref()
                .or(location.barcode.as_deref())
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Location #{id}"))
        })
        .unwrap_or_else(|| format!("Location #{id}"))
}

fn status_class(status: &str) -> &'static str {
    match status {
        "available" => "status open",
        "hold" | "quarantine" => "status held",
        "damaged" => "status muted",
        _ => "status processing",
    }
}

fn normalized(value: Option<&str>) -> String {
    value.unwrap_or_default().trim().to_ascii_lowercase()
}

fn encode_path_segment(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(char::from(byte));
        } else {
            use std::fmt::Write as _;
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

#[cfg(target_arch = "wasm32")]
fn print_page() {
    if let Some(window) = web_sys::window() {
        let _ = window.print();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn print_page() {}

#[cfg(test)]
mod tests {
    use super::encode_path_segment;

    #[test]
    fn barcode_lookup_encodes_one_url_path_segment() {
        assert_eq!(encode_path_segment("LP/A 1"), "LP%2FA%201");
        assert_eq!(encode_path_segment("ABC-123_9"), "ABC-123_9");
    }
}
