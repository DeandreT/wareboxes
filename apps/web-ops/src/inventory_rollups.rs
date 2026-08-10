use leptos::prelude::*;
#[cfg(target_arch = "wasm32")]
use wareboxes_api_contract::v1::{
    InventoryFacilityRollupPage, InventoryItemRollupPage, InventoryLocationRollupPage,
};
use wareboxes_api_contract::v1::{
    InventoryFacilityRollupResponse, InventoryItemRollupResponse, InventoryLocationRollupResponse,
    InventoryRollupQuantity, OpaqueCursor,
};

#[cfg(target_arch = "wasm32")]
use crate::api;
use crate::components::SearchField;
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::view_model::format_quantity;

#[cfg(target_arch = "wasm32")]
const PAGE_LIMIT: usize = 250;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum InventoryRollupKind {
    Location,
    Facility,
    Item,
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "hydration constructs inventory rollup row collections"
    )
)]
enum RollupRows {
    Location(Vec<InventoryLocationRollupResponse>),
    Facility(Vec<InventoryFacilityRollupResponse>),
    Item(Vec<InventoryItemRollupResponse>),
}

impl RollupRows {
    fn len(&self) -> usize {
        match self {
            Self::Location(rows) => rows.len(),
            Self::Facility(rows) => rows.len(),
            Self::Item(rows) => rows.len(),
        }
    }

    #[cfg(target_arch = "wasm32")]
    fn extend(&mut self, next: Self) {
        match (self, next) {
            (Self::Location(rows), Self::Location(next)) => rows.extend(next),
            (Self::Facility(rows), Self::Facility(next)) => rows.extend(next),
            (Self::Item(rows), Self::Item(next)) => rows.extend(next),
            _ => {}
        }
    }
}

#[derive(Clone)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "hydration constructs terminal inventory rollup states"
    )
)]
enum RollupState {
    Loading,
    Ready {
        rows: RollupRows,
        next_cursor: Option<OpaqueCursor>,
    },
    Failed(String),
}

#[derive(Clone, Copy)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "hydration consumes inventory rollup request state"
    )
)]
struct RollupRequestState {
    state: RwSignal<RollupState>,
    loading_more: RwSignal<bool>,
    page_error: RwSignal<Option<String>>,
    generation: RwSignal<u64>,
    on_unauthorized: Callback<()>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RollupSort {
    Client,
    Item,
    Scope,
    Balances,
    Batches,
    Locations,
}

#[component]
pub fn InventoryRollupsWorkbench(
    kind: InventoryRollupKind,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let state = RwSignal::new(RollupState::Loading);
    let filter = RwSignal::new(String::new());
    let applied_filter = RwSignal::new(String::new());
    let loading_more = RwSignal::new(false);
    let page_error = RwSignal::new(None::<String>);
    let generation = RwSignal::new(0_u64);
    let sort = RwSignal::new(SortSpec {
        key: RollupSort::Client,
        direction: SortDirection::Ascending,
    });
    let request_state = RollupRequestState {
        state,
        loading_more,
        page_error,
        generation,
        on_unauthorized,
    };

    #[cfg(target_arch = "wasm32")]
    request_rollups(
        kind,
        None,
        applied_filter.get_untracked(),
        sort.get_untracked(),
        request_state,
    );

    let reload = move || {
        request_rollups(
            kind,
            None,
            applied_filter.get_untracked(),
            sort.get_untracked(),
            request_state,
        );
    };
    let retry = move |_| reload();
    let apply_filter = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        applied_filter.set(filter.get_untracked().trim().to_owned());
        reload();
    };
    let change_sort = move |key: RollupSort| {
        SortSpec::select(sort, key);
        reload();
    };
    let load_more = move |_| {
        let RollupState::Ready { next_cursor, .. } = state.get_untracked() else {
            return;
        };
        let Some(cursor) = next_cursor else {
            return;
        };
        request_rollups(
            kind,
            Some(cursor),
            applied_filter.get_untracked(),
            sort.get_untracked(),
            request_state,
        );
    };

    view! {
        <section class="data-section inventory-rollups">
            {move || match state.get() {
                RollupState::Loading => view! {
                    <div class="rollup-state" aria-live="polite">
                        <span class="loading-line" aria-hidden="true"></span>
                        <strong>{format!("Loading {}", kind.label())}</strong>
                    </div>
                }
                .into_any(),
                RollupState::Failed(message) => view! {
                    <div class="rollup-state" role="alert">
                        <strong>"Inventory summary is unavailable"</strong>
                        <span>{message}</span>
                        <button class="button secondary-action" type="button" on:click=retry>
                            "Retry"
                        </button>
                    </div>
                }
                .into_any(),
                RollupState::Ready { rows, next_cursor } => view! {
                    <form class="table-toolbar" on:submit=apply_filter>
                        <div class="toolbar-summary">
                            <strong>{format_quantity(rows.len() as i64)}</strong>
                            <span>{format!("{} loaded", kind.label())}</span>
                        </div>
                        <SearchField
                            label=format!("Search {}", kind.label())
                            placeholder="Search all summaries"
                            value=filter
                        />
                        <button class="button secondary-action" type="submit">"Apply"</button>
                    </form>
                    <RollupTable rows sort on_sort=Callback::new(change_sort)/>
                    <div class="table-footer">
                        <span>
                            {if next_cursor.is_some() {
                                "More summaries available"
                            } else {
                                "All summaries loaded"
                            }}
                        </span>
                        {move || page_error.get().map(|message| {
                            view! { <span class="inline-error" role="alert">{message}</span> }
                        })}
                        <button
                            class="button secondary-action"
                            type="button"
                            disabled=move || next_cursor.is_none() || loading_more.get()
                            on:click=load_more
                        >
                            {move || if loading_more.get() { "Loading" } else { "Load more" }}
                        </button>
                    </div>
                }
                .into_any(),
            }}
        </section>
    }
}

impl InventoryRollupKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Location => "location summaries",
            Self::Facility => "facility summaries",
            Self::Item => "item summaries",
        }
    }

    #[cfg(target_arch = "wasm32")]
    const fn path(self) -> &'static str {
        match self {
            Self::Location => "/api/v1/inventory/rollups/by-location",
            Self::Facility => "/api/v1/inventory/rollups/by-facility",
            Self::Item => "/api/v1/inventory/rollups/by-item",
        }
    }
}

#[cfg(target_arch = "wasm32")]
fn request_rollups(
    kind: InventoryRollupKind,
    cursor: Option<OpaqueCursor>,
    query: String,
    sort: SortSpec<RollupSort>,
    request: RollupRequestState,
) {
    let RollupRequestState {
        state,
        loading_more,
        page_error,
        generation,
        on_unauthorized,
    } = request;
    let append = cursor.is_some();
    if append {
        loading_more.set(true);
    } else {
        state.set(RollupState::Loading);
    }
    page_error.set(None);
    let request_generation = generation.get_untracked().wrapping_add(1);
    generation.set(request_generation);
    leptos::task::spawn_local(async move {
        let mut path = format!(
            "{}?limit={PAGE_LIMIT}&sort={}&direction={}",
            kind.path(),
            api_sort(sort.key),
            api_direction(sort.direction),
        );
        if !query.is_empty() {
            path.push_str("&query=");
            path.push_str(&urlencoding::encode(&query));
        }
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(&urlencoding::encode(cursor.as_str()));
        }
        let result = fetch_rollups(kind, &path).await;
        if generation.get_untracked() != request_generation {
            return;
        }
        match result {
            Ok((next_rows, next_cursor)) => {
                if append {
                    state.update(|current| {
                        if let RollupState::Ready {
                            rows,
                            next_cursor: cursor,
                        } = current
                        {
                            rows.extend(next_rows);
                            *cursor = next_cursor;
                        }
                    });
                } else {
                    state.set(RollupState::Ready {
                        rows: next_rows,
                        next_cursor,
                    });
                }
            }
            Err(error) if error.unauthorized => on_unauthorized.run(()),
            Err(error) if append => page_error.set(Some(error.message)),
            Err(error) => state.set(RollupState::Failed(error.message)),
        }
        loading_more.set(false);
    });
}

#[cfg(target_arch = "wasm32")]
async fn fetch_rollups(
    kind: InventoryRollupKind,
    path: &str,
) -> Result<(RollupRows, Option<OpaqueCursor>), api::ApiError> {
    match kind {
        InventoryRollupKind::Location => {
            let page = api::internal_get::<InventoryLocationRollupPage>(path).await?;
            Ok((RollupRows::Location(page.items), page.next_cursor))
        }
        InventoryRollupKind::Facility => {
            let page = api::internal_get::<InventoryFacilityRollupPage>(path).await?;
            Ok((RollupRows::Facility(page.items), page.next_cursor))
        }
        InventoryRollupKind::Item => {
            let page = api::internal_get::<InventoryItemRollupPage>(path).await?;
            Ok((RollupRows::Item(page.items), page.next_cursor))
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn request_rollups(
    _kind: InventoryRollupKind,
    _cursor: Option<OpaqueCursor>,
    _query: String,
    _sort: SortSpec<RollupSort>,
    _request: RollupRequestState,
) {
}

#[component]
fn RollupTable(
    rows: RollupRows,
    sort: RwSignal<SortSpec<RollupSort>>,
    on_sort: Callback<RollupSort>,
) -> impl IntoView {
    match rows {
        RollupRows::Location(rows) => view! { <LocationRollupTable rows sort on_sort/> }.into_any(),
        RollupRows::Facility(rows) => view! { <FacilityRollupTable rows sort on_sort/> }.into_any(),
        RollupRows::Item(rows) => view! { <ItemRollupTable rows sort on_sort/> }.into_any(),
    }
}

#[component]
fn LocationRollupTable(
    rows: Vec<InventoryLocationRollupResponse>,
    sort: RwSignal<SortSpec<RollupSort>>,
    on_sort: Callback<RollupSort>,
) -> impl IntoView {
    view! {
        <div class="table-scroll">
            <table class="data-table rollup-table location-rollup-table">
                <thead><tr>
                    <RollupHeader label="Client" key=RollupSort::Client sort on_sort/>
                    <RollupHeader label="Item" key=RollupSort::Item sort on_sort/>
                    <RollupHeader label="Location" key=RollupSort::Scope sort on_sort/>
                    <RollupHeader label="Balances" key=RollupSort::Balances sort on_sort numeric=true/>
                    <RollupHeader label="Batches" key=RollupSort::Batches sort on_sort numeric=true/>
                    <th scope="col">"Quantity by UOM"</th>
                </tr></thead>
                <tbody>
                    {move || {
                        if rows.is_empty() {
                            empty_row(6)
                        } else {
                            rows.clone()
                                .into_iter()
                                .map(|row| {
                                    let item = item_label(
                                        row.item_id,
                                        row.primary_sku.as_deref(),
                                        row.item_description.as_deref(),
                                    );
                                    let location = location_label(&row);
                                    let facility = facility_label(
                                        row.facility_id,
                                        row.facility_name.as_deref(),
                                    );
                                    view! {
                                        <tr>
                                            <td>{row.inventory_owner_name}</td>
                                            <td>{item}</td>
                                            <td>
                                                <strong>{location}</strong>
                                                <small class="cell-detail">{facility}</small>
                                            </td>
                                            <td class="numeric">{format_quantity(row.balance_count)}</td>
                                            <td class="numeric">{format_quantity(row.batch_count)}</td>
                                            <td><QuantityBreakdown quantities=row.quantities/></td>
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
    }
}

#[component]
fn FacilityRollupTable(
    rows: Vec<InventoryFacilityRollupResponse>,
    sort: RwSignal<SortSpec<RollupSort>>,
    on_sort: Callback<RollupSort>,
) -> impl IntoView {
    view! {
        <div class="table-scroll">
            <table class="data-table rollup-table facility-rollup-table">
                <thead><tr>
                    <RollupHeader label="Client" key=RollupSort::Client sort on_sort/>
                    <RollupHeader label="Item" key=RollupSort::Item sort on_sort/>
                    <RollupHeader label="Facility" key=RollupSort::Scope sort on_sort/>
                    <RollupHeader label="Locations" key=RollupSort::Locations sort on_sort numeric=true/>
                    <RollupHeader label="Balances" key=RollupSort::Balances sort on_sort numeric=true/>
                    <RollupHeader label="Batches" key=RollupSort::Batches sort on_sort numeric=true/>
                    <th scope="col">"Quantity by UOM"</th>
                </tr></thead>
                <tbody>
                    {move || {
                        if rows.is_empty() {
                            empty_row(7)
                        } else {
                            rows.clone().into_iter().map(|row| view! {
                                <tr>
                                    <td>{row.inventory_owner_name}</td>
                                    <td>{item_label(row.item_id, row.primary_sku.as_deref(), row.item_description.as_deref())}</td>
                                    <td><strong>{facility_label(row.facility_id, row.facility_name.as_deref())}</strong></td>
                                    <td class="numeric">{format_quantity(row.location_count)}</td>
                                    <td class="numeric">{format_quantity(row.balance_count)}</td>
                                    <td class="numeric">{format_quantity(row.batch_count)}</td>
                                    <td><QuantityBreakdown quantities=row.quantities/></td>
                                </tr>
                            }).collect_view().into_any()
                        }
                    }}
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn ItemRollupTable(
    rows: Vec<InventoryItemRollupResponse>,
    sort: RwSignal<SortSpec<RollupSort>>,
    on_sort: Callback<RollupSort>,
) -> impl IntoView {
    view! {
        <div class="table-scroll">
            <table class="data-table rollup-table item-rollup-table">
                <thead><tr>
                    <RollupHeader label="Client" key=RollupSort::Client sort on_sort/>
                    <RollupHeader label="Item" key=RollupSort::Item sort on_sort/>
                    <RollupHeader label="Facilities" key=RollupSort::Scope sort on_sort numeric=true/>
                    <RollupHeader label="Locations" key=RollupSort::Locations sort on_sort numeric=true/>
                    <RollupHeader label="Balances" key=RollupSort::Balances sort on_sort numeric=true/>
                    <RollupHeader label="Batches" key=RollupSort::Batches sort on_sort numeric=true/>
                    <th scope="col">"Quantity by UOM"</th>
                </tr></thead>
                <tbody>
                    {move || {
                        if rows.is_empty() {
                            empty_row(7)
                        } else {
                            rows.clone().into_iter().map(|row| view! {
                                <tr>
                                    <td>{row.inventory_owner_name}</td>
                                    <td><strong>{item_label(row.item_id, row.primary_sku.as_deref(), row.item_description.as_deref())}</strong></td>
                                    <td class="numeric">{format_quantity(row.facility_count)}</td>
                                    <td class="numeric">{format_quantity(row.location_count)}</td>
                                    <td class="numeric">{format_quantity(row.balance_count)}</td>
                                    <td class="numeric">{format_quantity(row.batch_count)}</td>
                                    <td><QuantityBreakdown quantities=row.quantities/></td>
                                </tr>
                            }).collect_view().into_any()
                        }
                    }}
                </tbody>
            </table>
        </div>
    }
}

#[component]
fn RollupHeader(
    label: &'static str,
    key: RollupSort,
    sort: RwSignal<SortSpec<RollupSort>>,
    on_sort: Callback<RollupSort>,
    #[prop(default = false)] numeric: bool,
) -> impl IntoView {
    view! {
        <SortableHeader
            label
            active=move || sort.get().key == key
            direction=move || sort.get().direction
            on_sort=Callback::new(move |_| on_sort.run(key))
            numeric
        />
    }
}

#[component]
fn QuantityBreakdown(quantities: Vec<InventoryRollupQuantity>) -> impl IntoView {
    view! {
        <div class="rollup-quantities">
            {quantities.into_iter().map(|quantity| view! {
                <div>
                    <strong>{format_quantity(quantity.quantity.on_hand)}</strong>
                    <span>{quantity.uom}</span>
                    <small>{format!(
                        "Available {} | Reserved {} | Held {}",
                        format_quantity(quantity.quantity.available),
                        format_quantity(quantity.quantity.reserved),
                        format_quantity(quantity.quantity.held),
                    )}</small>
                </div>
            }).collect_view()}
        </div>
    }
}

fn item_label(id: i64, sku: Option<&str>, description: Option<&str>) -> String {
    sku.or(description)
        .map(str::to_owned)
        .unwrap_or_else(|| format!("Item #{id}"))
}

fn facility_label(id: i64, name: Option<&str>) -> String {
    name.map(str::to_owned)
        .unwrap_or_else(|| format!("Facility #{id}"))
}

fn location_label(row: &InventoryLocationRollupResponse) -> String {
    row.location_barcode
        .clone()
        .or_else(|| row.location_name.clone())
        .unwrap_or_else(|| format!("Location #{}", row.location_id))
}

#[cfg(target_arch = "wasm32")]
const fn api_sort(sort: RollupSort) -> &'static str {
    match sort {
        RollupSort::Client => "client",
        RollupSort::Item => "item",
        RollupSort::Scope => "scope",
        RollupSort::Balances => "balances",
        RollupSort::Batches => "batches",
        RollupSort::Locations => "locations",
    }
}

#[cfg(target_arch = "wasm32")]
const fn api_direction(direction: SortDirection) -> &'static str {
    match direction {
        SortDirection::Ascending => "ascending",
        SortDirection::Descending => "descending",
    }
}

fn empty_row(columns: usize) -> AnyView {
    view! {
        <tr><td class="table-empty-row" colspan=columns>"No summaries match this filter."</td></tr>
    }
    .into_any()
}

#[cfg(test)]
mod tests {
    use super::{facility_label, item_label};

    #[test]
    fn rollup_labels_use_operational_fallbacks() {
        assert_eq!(item_label(7, Some("SKU-7"), Some("Widget")), "SKU-7");
        assert_eq!(item_label(7, None, None), "Item #7");
        assert_eq!(facility_label(3, None), "Facility #3");
    }
}
