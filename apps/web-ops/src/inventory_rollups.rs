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
    let loading_more = RwSignal::new(false);
    let page_error = RwSignal::new(None::<String>);
    let sort = RwSignal::new(SortSpec {
        key: RollupSort::Client,
        direction: SortDirection::Ascending,
    });

    #[cfg(target_arch = "wasm32")]
    request_rollups(kind, None, state, loading_more, page_error, on_unauthorized);

    let retry =
        move |_| request_rollups(kind, None, state, loading_more, page_error, on_unauthorized);
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
            state,
            loading_more,
            page_error,
            on_unauthorized,
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
                    <div class="table-toolbar">
                        <div class="toolbar-summary">
                            <strong>{format_quantity(rows.len() as i64)}</strong>
                            <span>{format!("{} loaded", kind.label())}</span>
                        </div>
                        <SearchField
                            label=format!("Filter {}", kind.label())
                            placeholder="Filter summaries"
                            value=filter
                        />
                    </div>
                    <RollupTable rows filter sort/>
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
    state: RwSignal<RollupState>,
    loading_more: RwSignal<bool>,
    page_error: RwSignal<Option<String>>,
    on_unauthorized: Callback<()>,
) {
    let append = cursor.is_some();
    if append {
        loading_more.set(true);
    } else {
        state.set(RollupState::Loading);
    }
    page_error.set(None);
    leptos::task::spawn_local(async move {
        let mut path = format!("{}?limit={PAGE_LIMIT}", kind.path());
        if let Some(cursor) = cursor {
            path.push_str("&cursor=");
            path.push_str(cursor.as_str());
        }
        let result = fetch_rollups(kind, &path).await;
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
    _state: RwSignal<RollupState>,
    _loading_more: RwSignal<bool>,
    _page_error: RwSignal<Option<String>>,
    _on_unauthorized: Callback<()>,
) {
}

#[component]
fn RollupTable(
    rows: RollupRows,
    filter: RwSignal<String>,
    sort: RwSignal<SortSpec<RollupSort>>,
) -> impl IntoView {
    match rows {
        RollupRows::Location(rows) => view! { <LocationRollupTable rows filter sort/> }.into_any(),
        RollupRows::Facility(rows) => view! { <FacilityRollupTable rows filter sort/> }.into_any(),
        RollupRows::Item(rows) => view! { <ItemRollupTable rows filter sort/> }.into_any(),
    }
}

#[component]
fn LocationRollupTable(
    rows: Vec<InventoryLocationRollupResponse>,
    filter: RwSignal<String>,
    sort: RwSignal<SortSpec<RollupSort>>,
) -> impl IntoView {
    view! {
        <div class="table-scroll">
            <table class="data-table rollup-table location-rollup-table">
                <thead><tr>
                    <RollupHeader label="Client" key=RollupSort::Client sort/>
                    <RollupHeader label="Item" key=RollupSort::Item sort/>
                    <RollupHeader label="Location" key=RollupSort::Scope sort/>
                    <RollupHeader label="Balances" key=RollupSort::Balances sort numeric=true/>
                    <RollupHeader label="Batches" key=RollupSort::Batches sort numeric=true/>
                    <th scope="col">"Quantity by UOM"</th>
                </tr></thead>
                <tbody>
                    {move || {
                        let query = normalized_filter(filter);
                        let mut matching = rows
                            .clone()
                            .into_iter()
                            .filter(|row| location_matches(row, &query))
                            .collect::<Vec<_>>();
                        matching.sort_by(|left, right| {
                            let left_item = item_label(
                                left.item_id,
                                left.primary_sku.as_deref(),
                                left.item_description.as_deref(),
                            );
                            let right_item = item_label(
                                right.item_id,
                                right.primary_sku.as_deref(),
                                right.item_description.as_deref(),
                            );
                            let left_scope = location_label(left);
                            let right_scope = location_label(right);
                            apply_sort(
                                sort.get(),
                                RollupSortValues {
                                    client: &left.inventory_owner_name,
                                    item: &left_item,
                                    scope: &left_scope,
                                    balances: left.balance_count,
                                    batches: left.batch_count,
                                    locations: 1,
                                },
                                RollupSortValues {
                                    client: &right.inventory_owner_name,
                                    item: &right_item,
                                    scope: &right_scope,
                                    balances: right.balance_count,
                                    batches: right.batch_count,
                                    locations: 1,
                                },
                            )
                        });
                        if matching.is_empty() {
                            empty_row(6)
                        } else {
                            matching
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
    filter: RwSignal<String>,
    sort: RwSignal<SortSpec<RollupSort>>,
) -> impl IntoView {
    view! {
        <div class="table-scroll">
            <table class="data-table rollup-table facility-rollup-table">
                <thead><tr>
                    <RollupHeader label="Client" key=RollupSort::Client sort/>
                    <RollupHeader label="Item" key=RollupSort::Item sort/>
                    <RollupHeader label="Facility" key=RollupSort::Scope sort/>
                    <RollupHeader label="Locations" key=RollupSort::Locations sort numeric=true/>
                    <RollupHeader label="Balances" key=RollupSort::Balances sort numeric=true/>
                    <RollupHeader label="Batches" key=RollupSort::Batches sort numeric=true/>
                    <th scope="col">"Quantity by UOM"</th>
                </tr></thead>
                <tbody>
                    {move || {
                        let query = normalized_filter(filter);
                        let mut matching = rows
                            .clone()
                            .into_iter()
                            .filter(|row| facility_matches(row, &query))
                            .collect::<Vec<_>>();
                        matching.sort_by(|left, right| {
                            let left_item = item_label(
                                left.item_id,
                                left.primary_sku.as_deref(),
                                left.item_description.as_deref(),
                            );
                            let right_item = item_label(
                                right.item_id,
                                right.primary_sku.as_deref(),
                                right.item_description.as_deref(),
                            );
                            let left_scope =
                                facility_label(left.facility_id, left.facility_name.as_deref());
                            let right_scope =
                                facility_label(right.facility_id, right.facility_name.as_deref());
                            apply_sort(
                                sort.get(),
                                RollupSortValues {
                                    client: &left.inventory_owner_name,
                                    item: &left_item,
                                    scope: &left_scope,
                                    balances: left.balance_count,
                                    batches: left.batch_count,
                                    locations: left.location_count,
                                },
                                RollupSortValues {
                                    client: &right.inventory_owner_name,
                                    item: &right_item,
                                    scope: &right_scope,
                                    balances: right.balance_count,
                                    batches: right.batch_count,
                                    locations: right.location_count,
                                },
                            )
                        });
                        if matching.is_empty() {
                            empty_row(7)
                        } else {
                            matching.into_iter().map(|row| view! {
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
    filter: RwSignal<String>,
    sort: RwSignal<SortSpec<RollupSort>>,
) -> impl IntoView {
    view! {
        <div class="table-scroll">
            <table class="data-table rollup-table item-rollup-table">
                <thead><tr>
                    <RollupHeader label="Client" key=RollupSort::Client sort/>
                    <RollupHeader label="Item" key=RollupSort::Item sort/>
                    <RollupHeader label="Facilities" key=RollupSort::Scope sort numeric=true/>
                    <RollupHeader label="Locations" key=RollupSort::Locations sort numeric=true/>
                    <RollupHeader label="Balances" key=RollupSort::Balances sort numeric=true/>
                    <RollupHeader label="Batches" key=RollupSort::Batches sort numeric=true/>
                    <th scope="col">"Quantity by UOM"</th>
                </tr></thead>
                <tbody>
                    {move || {
                        let query = normalized_filter(filter);
                        let mut matching = rows
                            .clone()
                            .into_iter()
                            .filter(|row| item_matches(row, &query))
                            .collect::<Vec<_>>();
                        matching.sort_by(|left, right| {
                            let left_item = item_label(left.item_id, left.primary_sku.as_deref(), left.item_description.as_deref());
                            let right_item = item_label(right.item_id, right.primary_sku.as_deref(), right.item_description.as_deref());
                            let spec = sort.get();
                            let ordering = match spec.key {
                                RollupSort::Client => left.inventory_owner_name.to_ascii_lowercase().cmp(&right.inventory_owner_name.to_ascii_lowercase()),
                                RollupSort::Item => left_item.to_ascii_lowercase().cmp(&right_item.to_ascii_lowercase()),
                                RollupSort::Scope => left.facility_count.cmp(&right.facility_count),
                                RollupSort::Locations => left.location_count.cmp(&right.location_count),
                                RollupSort::Balances => left.balance_count.cmp(&right.balance_count),
                                RollupSort::Batches => left.batch_count.cmp(&right.batch_count),
                            };
                            directed(ordering.then_with(|| left.item_id.cmp(&right.item_id)), spec.direction)
                        });
                        if matching.is_empty() {
                            empty_row(7)
                        } else {
                            matching.into_iter().map(|row| view! {
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
    #[prop(default = false)] numeric: bool,
) -> impl IntoView {
    view! {
        <SortableHeader
            label
            active=move || sort.get().key == key
            direction=move || sort.get().direction
            on_sort=Callback::new(move |_| SortSpec::select(sort, key))
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

struct RollupSortValues<'a> {
    client: &'a str,
    item: &'a str,
    scope: &'a str,
    balances: i64,
    batches: i64,
    locations: i64,
}

fn apply_sort(
    spec: SortSpec<RollupSort>,
    left: RollupSortValues<'_>,
    right: RollupSortValues<'_>,
) -> std::cmp::Ordering {
    let ordering = match spec.key {
        RollupSort::Client => left
            .client
            .to_ascii_lowercase()
            .cmp(&right.client.to_ascii_lowercase()),
        RollupSort::Item => left
            .item
            .to_ascii_lowercase()
            .cmp(&right.item.to_ascii_lowercase()),
        RollupSort::Scope => left
            .scope
            .to_ascii_lowercase()
            .cmp(&right.scope.to_ascii_lowercase()),
        RollupSort::Balances => left.balances.cmp(&right.balances),
        RollupSort::Batches => left.batches.cmp(&right.batches),
        RollupSort::Locations => left.locations.cmp(&right.locations),
    };
    directed(ordering, spec.direction)
}

fn directed(ordering: std::cmp::Ordering, direction: SortDirection) -> std::cmp::Ordering {
    if direction == SortDirection::Ascending {
        ordering
    } else {
        ordering.reverse()
    }
}

fn normalized_filter(filter: RwSignal<String>) -> String {
    filter.get().trim().to_ascii_lowercase()
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

fn location_matches(row: &InventoryLocationRollupResponse, query: &str) -> bool {
    query.is_empty()
        || [
            row.inventory_owner_name.as_str(),
            row.primary_sku.as_deref().unwrap_or_default(),
            row.item_description.as_deref().unwrap_or_default(),
            row.facility_name.as_deref().unwrap_or_default(),
            row.location_name.as_deref().unwrap_or_default(),
            row.location_barcode.as_deref().unwrap_or_default(),
        ]
        .iter()
        .any(|value| value.to_ascii_lowercase().contains(query))
}

fn facility_matches(row: &InventoryFacilityRollupResponse, query: &str) -> bool {
    query.is_empty()
        || [
            row.inventory_owner_name.as_str(),
            row.primary_sku.as_deref().unwrap_or_default(),
            row.item_description.as_deref().unwrap_or_default(),
            row.facility_name.as_deref().unwrap_or_default(),
        ]
        .iter()
        .any(|value| value.to_ascii_lowercase().contains(query))
}

fn item_matches(row: &InventoryItemRollupResponse, query: &str) -> bool {
    query.is_empty()
        || [
            row.inventory_owner_name.as_str(),
            row.primary_sku.as_deref().unwrap_or_default(),
            row.item_description.as_deref().unwrap_or_default(),
        ]
        .iter()
        .any(|value| value.to_ascii_lowercase().contains(query))
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
