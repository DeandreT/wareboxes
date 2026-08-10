use leptos::prelude::*;
use wareboxes_core::models::{
    Facility, InventoryOwner, InventoryOwnerItem, Item, ItemPackLink, LicensePlate, Location,
};

#[cfg(target_arch = "wasm32")]
use crate::api;
#[cfg(target_arch = "wasm32")]
use crate::toast::use_toast_bus;
use crate::workspace_layout::{PaneControls, SplitPaneState};

#[path = "catalog_item_storage_policies.rs"]
mod item_storage_policies;
#[path = "catalog_item_traceability_policies.rs"]
mod item_traceability_policies;
#[path = "catalog_items.rs"]
mod items;
#[path = "catalog_license_plates.rs"]
mod license_plates;
#[path = "catalog_locations.rs"]
mod locations;
#[path = "catalog_storage_zones.rs"]
mod storage_zones;

use item_storage_policies::ItemStoragePolicyCatalog;
use item_traceability_policies::ItemTraceabilityPolicyCatalog;
use items::ItemCatalog;
use license_plates::LicensePlateCatalog;
use locations::LocationCatalog;
use storage_zones::StorageZoneCatalog;

#[derive(Clone, Default)]
pub(crate) struct CatalogData {
    pub(crate) items: Vec<Item>,
    pub(crate) item_pack_links: Vec<ItemPackLink>,
    pub(crate) item_owner_assignments: Vec<InventoryOwnerItem>,
    pub(crate) locations: Vec<Location>,
    pub(crate) license_plates: Vec<LicensePlate>,
    pub(crate) facilities: Vec<Facility>,
    pub(crate) clients: Vec<InventoryOwner>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(target_arch = "wasm32"),
    expect(
        dead_code,
        reason = "hydration constructs the terminal catalog loading states"
    )
)]
pub(crate) enum LoadState {
    Loading,
    Ready,
    Refreshing,
    Failed,
}

#[derive(Clone, Copy)]
pub(crate) struct CatalogStore {
    pub(crate) data: RwSignal<CatalogData>,
    pub(crate) load_state: RwSignal<LoadState>,
    pub(crate) error: RwSignal<Option<String>>,
    pub(crate) on_unauthorized: Callback<()>,
}

impl CatalogStore {
    pub(crate) fn refresh(self) {
        refresh_catalog(self);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CatalogSection {
    Items,
    Locations,
    LicensePlates,
    StorageZones,
    ItemStoragePolicies,
    ItemTraceabilityPolicies,
}

#[component]
pub fn CatalogWorkbench(on_unauthorized: Callback<()>, can_supervise: bool) -> impl IntoView {
    let store = CatalogStore {
        data: RwSignal::new(CatalogData::default()),
        load_state: RwSignal::new(LoadState::Loading),
        error: RwSignal::new(None),
        on_unauthorized,
    };
    let section = RwSignal::new(CatalogSection::Items);
    let layout = SplitPaneState::new("catalog", 760);

    store.refresh();

    view! {
        <section class="catalog-workbench">
            <header class="catalog-heading">
                <div>
                    <p class="eyebrow">"Warehouse setup"</p>
                    <h1>"Master data"</h1>
                    <p>"Maintain sellable items, storage topology, facility locations, and scannable license plates."</p>
                </div>
                <div class="catalog-heading-actions">
                    <PaneControls layout master_label="catalog table" detail_label="catalog detail"/>
                    <button
                        class="button secondary-action compact"
                        type="button"
                        disabled=move || store.load_state.get() == LoadState::Refreshing
                        on:click=move |_| store.refresh()
                    >
                        {move || {
                            if store.load_state.get() == LoadState::Refreshing {
                                "Refreshing"
                            } else {
                                "Refresh"
                            }
                        }}
                    </button>
                </div>
            </header>

            <nav class="catalog-tabs" aria-label="Master data">
                <button
                    type="button"
                    class:active=move || section.get() == CatalogSection::Items
                    aria-current=move || {
                        (section.get() == CatalogSection::Items).then_some("page")
                    }
                    on:click=move |_| section.set(CatalogSection::Items)
                >
                    "Items"
                    <span>{move || store.data.get().items.len()}</span>
                </button>
                <button
                    type="button"
                    class:active=move || section.get() == CatalogSection::Locations
                    aria-current=move || {
                        (section.get() == CatalogSection::Locations).then_some("page")
                    }
                    on:click=move |_| section.set(CatalogSection::Locations)
                >
                    "Locations"
                    <span>{move || store.data.get().locations.len()}</span>
                </button>
                <button
                    type="button"
                    class:active=move || section.get() == CatalogSection::LicensePlates
                    aria-current=move || {
                        (section.get() == CatalogSection::LicensePlates).then_some("page")
                    }
                    on:click=move |_| section.set(CatalogSection::LicensePlates)
                >
                    "License plates"
                    <span>{move || store.data.get().license_plates.len()}</span>
                </button>
                <button
                    type="button"
                    class:active=move || section.get() == CatalogSection::StorageZones
                    aria-current=move || (section.get() == CatalogSection::StorageZones).then_some("page")
                    on:click=move |_| section.set(CatalogSection::StorageZones)
                >
                    "Storage zones"
                </button>
                <button
                    type="button"
                    class:active=move || section.get() == CatalogSection::ItemStoragePolicies
                    aria-current=move || (section.get() == CatalogSection::ItemStoragePolicies).then_some("page")
                    on:click=move |_| section.set(CatalogSection::ItemStoragePolicies)
                >
                    "Storage policies"
                </button>
                <button
                    type="button"
                    class:active=move || section.get() == CatalogSection::ItemTraceabilityPolicies
                    aria-current=move || (section.get() == CatalogSection::ItemTraceabilityPolicies).then_some("page")
                    on:click=move |_| section.set(CatalogSection::ItemTraceabilityPolicies)
                >
                    "Traceability"
                </button>
            </nav>

            {move || match store.load_state.get() {
                LoadState::Loading => view! {
                    <div class="catalog-state" role="status">
                        <span class="loading-line" aria-hidden="true"></span>
                        <strong>"Loading master data"</strong>
                    </div>
                }
                .into_any(),
                LoadState::Failed => {
                    let message = store
                        .error
                        .get()
                        .unwrap_or_else(|| "Master data is unavailable.".to_owned());
                    view! {
                        <div class="catalog-state catalog-error" role="alert">
                            <strong>"Master data is unavailable"</strong>
                            <span>{message}</span>
                            <button
                                class="button primary-action compact"
                                type="button"
                                on:click=move |_| store.refresh()
                            >
                                "Retry"
                            </button>
                        </div>
                    }
                    .into_any()
                }
                LoadState::Ready | LoadState::Refreshing => match section.get() {
                    CatalogSection::Items => view! { <ItemCatalog store layout can_supervise/> }.into_any(),
                    CatalogSection::Locations => view! { <LocationCatalog store layout/> }.into_any(),
                    CatalogSection::LicensePlates => {
                        view! { <LicensePlateCatalog store layout/> }.into_any()
                    }
                    CatalogSection::StorageZones => {
                        view! { <StorageZoneCatalog store can_supervise layout/> }.into_any()
                    }
                    CatalogSection::ItemStoragePolicies => {
                        view! { <ItemStoragePolicyCatalog store can_supervise layout/> }.into_any()
                    }
                    CatalogSection::ItemTraceabilityPolicies => {
                        view! { <ItemTraceabilityPolicyCatalog store can_supervise layout/> }.into_any()
                    }
                },
            }}
        </section>
    }
}

pub(crate) fn optional_text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

pub(crate) fn label_or_id(value: Option<&str>, entity: &str, id: i64) -> String {
    value
        .filter(|value| !value.trim().is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{entity} #{id}"))
}

fn refresh_catalog(store: CatalogStore) {
    #[cfg(not(target_arch = "wasm32"))]
    let _ = store;

    #[cfg(target_arch = "wasm32")]
    {
        let next = if store.load_state.get_untracked() == LoadState::Ready {
            LoadState::Refreshing
        } else {
            LoadState::Loading
        };
        store.load_state.set(next);
        store.error.set(None);
        leptos::task::spawn_local(async move {
            match load_catalog().await {
                Ok(data) => {
                    store.data.set(data);
                    store.load_state.set(LoadState::Ready);
                }
                Err(error) if error.unauthorized => store.on_unauthorized.run(()),
                Err(error) => {
                    let toasts = use_toast_bus();
                    toasts.error(error.message.clone());
                    store.error.set(Some(error.message));
                    store.load_state.set(LoadState::Failed);
                }
            }
        });
    }
}

#[cfg(target_arch = "wasm32")]
async fn load_catalog() -> Result<CatalogData, api::ApiError> {
    Ok(CatalogData {
        items: api::internal_get("/api/items?show_deleted=true").await?,
        item_pack_links: api::internal_get("/api/items/pack-links?show_deleted=false").await?,
        item_owner_assignments: api::internal_get("/api/items/inventory-owners?show_deleted=false")
            .await?,
        locations: api::internal_get("/api/locations?show_deleted=true").await?,
        license_plates: api::internal_get("/api/license-plates?show_deleted=true").await?,
        facilities: api::internal_get("/api/facilities?show_deleted=false").await?,
        clients: api::internal_get("/api/inventory-owners?show_deleted=false").await?,
    })
}

#[cfg(test)]
mod tests {
    use super::{label_or_id, optional_text};

    #[test]
    fn optional_text_trims_and_rejects_empty_input() {
        assert_eq!(optional_text("  Dock A  "), Some("Dock A".to_owned()));
        assert_eq!(optional_text(" \n "), None);
    }

    #[test]
    fn labels_fall_back_to_warehouse_entity_and_id() {
        assert_eq!(label_or_id(Some("A-01-02"), "Location", 7), "A-01-02");
        assert_eq!(label_or_id(None, "Location", 7), "Location #7");
    }
}
