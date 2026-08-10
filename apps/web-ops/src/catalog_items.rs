use std::cmp::Ordering;

use leptos::prelude::*;
use wareboxes_core::dto::{
    AddBarcode, AddItem, AddItemPackLink, AddSku, BarcodeIdRequest, ItemIdRequest,
    ItemPackLinkIdRequest, ItemUpdate,
};
use wareboxes_core::models::Item;

#[cfg(target_arch = "wasm32")]
use wasm_bindgen::{closure::Closure, JsCast};

use crate::api;
use crate::components::{Icon, SearchField, UiIcon};
use crate::sorting::{SortDirection, SortSpec, SortableHeader};
use crate::toast::use_toast_bus;
use crate::workspace_layout::{SplitPaneHandle, SplitPaneState};

use super::{optional_text, CatalogStore};

#[path = "catalog_items/pack.rs"]
mod pack;

use pack::{active_pack_links_for_item, pack_conversion_label};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ItemSort {
    Id,
    Description,
    Packaging,
    Skus,
    Barcodes,
    Status,
}

#[component]
pub(super) fn ItemCatalog(store: CatalogStore, layout: SplitPaneState) -> impl IntoView {
    let filter = RwSignal::new(String::new());
    let show_inactive = RwSignal::new(false);
    let selected_id = RwSignal::new(None::<i64>);
    let creating = RwSignal::new(false);
    let sort = RwSignal::new(SortSpec {
        key: ItemSort::Description,
        direction: SortDirection::Ascending,
    });

    view! {
        <div class="catalog-layout split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
            <section class="data-section catalog-browser split-master">
                <div class="catalog-toolbar">
                    <SearchField
                        label="Filter items by description, SKU, or barcode".to_owned()
                        placeholder="Item, SKU, barcode"
                        value=filter
                    />
                    <label class="catalog-check">
                        <input
                            type="checkbox"
                            prop:checked=move || show_inactive.get()
                            on:change=move |event| {
                                show_inactive.set(event_target_checked(&event))
                            }
                        />
                        <span>"Inactive"</span>
                    </label>
                    <span class="catalog-count">
                        {move || {
                            visible_items(
                                &store.data.get().items,
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
                            layout.show_detail();
                        }
                    >
                        "New item"
                    </button>
                </div>
                <div class="table-scroll catalog-table-scroll">
                    <table class="data-table catalog-table item-table">
                        <caption class="sr-only">"Item master records in the current organization"</caption>
                        <thead>
                            <tr>
                                <SortableHeader
                                    label="ID"
                                    active=move || sort.get().key == ItemSort::Id
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, ItemSort::Id)
                                    })
                                    numeric=true
                                />
                                <SortableHeader
                                    label="Description"
                                    active=move || sort.get().key == ItemSort::Description
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, ItemSort::Description)
                                    })
                                />
                                <SortableHeader
                                    label="Pack"
                                    active=move || sort.get().key == ItemSort::Packaging
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, ItemSort::Packaging)
                                    })
                                />
                                <SortableHeader
                                    label="SKUs"
                                    active=move || sort.get().key == ItemSort::Skus
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, ItemSort::Skus)
                                    })
                                    numeric=true
                                />
                                <SortableHeader
                                    label="Codes"
                                    active=move || sort.get().key == ItemSort::Barcodes
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, ItemSort::Barcodes)
                                    })
                                    numeric=true
                                />
                                <SortableHeader
                                    label="Status"
                                    active=move || sort.get().key == ItemSort::Status
                                    direction=move || sort.get().direction
                                    on_sort=Callback::new(move |_| {
                                        SortSpec::select(sort, ItemSort::Status)
                                    })
                                />
                            </tr>
                        </thead>
                        <tbody>
                            {move || {
                                let items = visible_items(
                                    &store.data.get().items,
                                    &filter.get(),
                                    show_inactive.get(),
                                    sort.get(),
                                );
                                if items.is_empty() {
                                    view! {
                                        <tr>
                                            <td class="table-empty-row" colspan="6">
                                                "No items match this view."
                                            </td>
                                        </tr>
                                    }
                                    .into_any()
                                } else {
                                    items
                                        .into_iter()
                                        .map(|item| {
                                            let id = item.id;
                                            let inactive = item.deleted.is_some();
                                            view! {
                                                <tr class:selected=move || {
                                                    selected_id.get() == Some(id)
                                                }>
                                                    <td class="numeric muted">{item.id}</td>
                                                    <td>
                                                        <button
                                                            type="button"
                                                            class="catalog-row-link"
                                                            on:click=move |_| {
                                                                creating.set(false);
                                                                selected_id.set(Some(id));
                                                                layout.show_detail();
                                                            }
                                                        >
                                                            {item
                                                                .description
                                                                .unwrap_or_else(|| "Unnamed item".to_owned())}
                                                        </button>
                                                    </td>
                                                    <td>
                                                        <span class="catalog-badge">
                                                            {packaging_label(&item.packaging_unit)}
                                                        </span>
                                                    </td>
                                                    <td class="numeric">{item.skus.len()}</td>
                                                    <td class="numeric">{item.barcodes.len()}</td>
                                                    <td>
                                                        <span class=if inactive {
                                                            "status muted"
                                                        } else {
                                                            "status open"
                                                        }>
                                                            {if inactive { "Inactive" } else { "Active" }}
                                                        </span>
                                                    </td>
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
            <SplitPaneHandle layout/>
            <aside class="data-section catalog-editor split-detail" aria-label="Item editor">
                {move || {
                    if creating.get() {
                        view! {
                            <ItemCreate
                                store
                                on_cancel=Callback::new(move |_| creating.set(false))
                                on_created=Callback::new(move |id| {
                                    creating.set(false);
                                    selected_id.set(Some(id));
                                })
                            />
                        }
                        .into_any()
                    } else if let Some(item) = selected_item(&store.data.get().items, selected_id.get()) {
                        view! { <ItemDetail store item/> }.into_any()
                    } else {
                        view! {
                            <div class="catalog-editor-empty">
                                <strong>"Select an item"</strong>
                                <p>"Review its identifiers, packaging, status, and scan codes."</p>
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
fn ItemCreate(
    store: CatalogStore,
    on_cancel: Callback<()>,
    on_created: Callback<i64>,
) -> impl IntoView {
    let description = RwSignal::new(String::new());
    let packaging = RwSignal::new("each".to_owned());
    let notes = RwSignal::new(String::new());
    let length = RwSignal::new(String::new());
    let width = RwSignal::new(String::new());
    let height = RwSignal::new(String::new());
    let length_uom = RwSignal::new("in".to_owned());
    let weight = RwSignal::new(String::new());
    let weight_uom = RwSignal::new("lb".to_owned());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let toasts = use_toast_bus();

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let description_value = description.get_untracked().trim().to_owned();
        if description_value.is_empty() {
            error.set(Some("Enter an item description.".to_owned()));
            return;
        }
        let dimensions = [
            ("Length", length.get_untracked()),
            ("Width", width.get_untracked()),
            ("Height", height.get_untracked()),
            ("Weight", weight.get_untracked()),
        ];
        let mut parsed = Vec::with_capacity(dimensions.len());
        for (label, value) in dimensions {
            match optional_nonnegative(&value) {
                Ok(value) => parsed.push(value),
                Err(()) => {
                    error.set(Some(format!(
                        "{label} must be a non-negative whole number."
                    )));
                    return;
                }
            }
        }
        let request = AddItem {
            description: description_value.clone(),
            packaging_unit: packaging.get_untracked(),
            notes: optional_text(&notes.get_untracked()),
            length: parsed[0],
            width: parsed[1],
            height: parsed[2],
            length_uom: (parsed[..3].iter().any(Option::is_some))
                .then(|| length_uom.get_untracked()),
            weight: parsed[3],
            weight_uom: parsed[3].map(|_| weight_uom.get_untracked()),
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/items/add", &request).await {
                Ok(id) => {
                    toasts.success(format!("Item #{id} created: {description_value}."));
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
                <div>
                    <p class="eyebrow">"Item master"</p>
                    <h2>"New item"</h2>
                </div>
                <button
                    class="button quiet-action compact"
                    type="button"
                    on:click=move |_| on_cancel.run(())
                >
                    "Cancel"
                </button>
            </div>
            <label>
                <span>"Description"</span>
                <input
                    type="text"
                    required
                    autofocus
                    prop:value=move || description.get()
                    on:input=move |event| description.set(event_target_value(&event))
                />
            </label>
            <div class="form-grid two">
                <label>
                    <span>"Packaging unit"</span>
                    <select
                        prop:value=move || packaging.get()
                        on:change=move |event| packaging.set(event_target_value(&event))
                    >
                        <option value="each">"Each"</option>
                        <option value="case">"Case"</option>
                    </select>
                </label>
                <label>
                    <span>"Notes"</span>
                    <input
                        type="text"
                        prop:value=move || notes.get()
                        on:input=move |event| notes.set(event_target_value(&event))
                    />
                </label>
            </div>
            <fieldset class="catalog-fieldset">
                <legend>"Physical dimensions"</legend>
                <div class="form-grid dimension-grid">
                    <label><span>"Length"</span><input type="number" min="0" step="1" prop:value=move || length.get() on:input=move |event| length.set(event_target_value(&event))/></label>
                    <label><span>"Width"</span><input type="number" min="0" step="1" prop:value=move || width.get() on:input=move |event| width.set(event_target_value(&event))/></label>
                    <label><span>"Height"</span><input type="number" min="0" step="1" prop:value=move || height.get() on:input=move |event| height.set(event_target_value(&event))/></label>
                    <label>
                        <span>"Length UOM"</span>
                        <select prop:value=move || length_uom.get() on:change=move |event| length_uom.set(event_target_value(&event))>
                            <option value="in">"in"</option>
                            <option value="cm">"cm"</option>
                            <option value="mm">"mm"</option>
                        </select>
                    </label>
                    <label><span>"Weight"</span><input type="number" min="0" step="1" prop:value=move || weight.get() on:input=move |event| weight.set(event_target_value(&event))/></label>
                    <label>
                        <span>"Weight UOM"</span>
                        <select prop:value=move || weight_uom.get() on:change=move |event| weight_uom.set(event_target_value(&event))>
                            <option value="lb">"lb"</option>
                            <option value="kg">"kg"</option>
                        </select>
                    </label>
                </div>
            </fieldset>
            <InlineError error/>
            <div class="catalog-form-actions">
                <button class="button primary-action" type="submit" disabled=move || pending.get()>
                    {move || if pending.get() { "Creating..." } else { "Create item" }}
                </button>
            </div>
        </form>
    }
}

#[component]
fn ItemDetail(store: CatalogStore, item: Item) -> impl IntoView {
    let description = RwSignal::new(item.description.clone().unwrap_or_default());
    let packaging = RwSignal::new(item.packaging_unit.clone());
    let notes = RwSignal::new(item.notes.clone().unwrap_or_default());
    let sku = RwSignal::new(String::new());
    let sku_notes = RwSignal::new(String::new());
    let barcode = RwSignal::new(String::new());
    let barcode_type = RwSignal::new("code128".to_owned());
    let barcode_notes = RwSignal::new(String::new());
    let contained_item_id = RwSignal::new(String::new());
    let contained_quantity = RwSignal::new("2".to_owned());
    let pack_notes = RwSignal::new(String::new());
    let print_barcode_id = RwSignal::new(None::<i64>);
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let item_id = item.id;
    let inactive = item.deleted.is_some();
    let toasts = use_toast_bus();
    let pack_options = StoredValue::new(
        store
            .data
            .get_untracked()
            .items
            .into_iter()
            .filter(|candidate| candidate.id != item_id && candidate.deleted.is_none())
            .map(|candidate| {
                (
                    candidate.id,
                    candidate
                        .description
                        .unwrap_or_else(|| format!("Item #{}", candidate.id)),
                )
            })
            .collect::<Vec<_>>(),
    );

    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let description_value = description.get_untracked().trim().to_owned();
        if description_value.is_empty() {
            error.set(Some("Description cannot be empty.".to_owned()));
            return;
        }
        let request = ItemUpdate {
            item_id,
            description: Some(description_value),
            notes: optional_text(&notes.get_untracked()),
            packaging_unit: Some(packaging.get_untracked()),
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            handle_bool_command(
                store,
                "/api/items/update",
                &request,
                format!("Item #{item_id} updated."),
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
                "/api/items/restore",
                format!("Item #{item_id} reactivated."),
            )
        } else {
            ("/api/items/delete", format!("Item #{item_id} deactivated."))
        };
        pending.set(true);
        error.set(None);
        let request = ItemIdRequest { item_id };
        leptos::task::spawn_local(async move {
            handle_bool_command(store, path, &request, message, pending, error, toasts).await;
        });
    };

    let add_sku = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        let name = sku.get_untracked().trim().to_owned();
        if name.is_empty() || pending.get_untracked() {
            return;
        }
        let request = AddSku {
            item_id,
            name: name.clone(),
            notes: optional_text(&sku_notes.get_untracked()),
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/items/skus/add", &request).await {
                Ok(id) => {
                    sku.set(String::new());
                    sku_notes.set(String::new());
                    toasts.success(format!("SKU {name} added as identifier #{id}."));
                    pending.set(false);
                    store.refresh();
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

    let add_barcode = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let barcode_type_value = barcode_type.get_untracked();
        let name = match barcode_label(&barcode_type_value, &barcode.get_untracked()) {
            Ok(label) => label.normalized_value,
            Err(message) => {
                error.set(Some(message));
                return;
            }
        };
        let request = AddBarcode {
            item_id,
            name: name.clone(),
            r#type: barcode_type_value,
            notes: optional_text(&barcode_notes.get_untracked()),
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/items/barcodes/add", &request).await {
                Ok(id) => {
                    barcode.set(String::new());
                    barcode_notes.set(String::new());
                    toasts.success(format!("Barcode {name} added as scan code #{id}."));
                    pending.set(false);
                    store.refresh();
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

    let add_pack_conversion = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let Some(single_item_id) = contained_item_id
            .get_untracked()
            .parse::<i64>()
            .ok()
            .filter(|id| pack_options.with_value(|items| items.iter().any(|item| item.0 == *id)))
        else {
            error.set(Some("Select the contained item.".to_owned()));
            return;
        };
        let Some(inner_qty) = contained_quantity
            .get_untracked()
            .parse::<i64>()
            .ok()
            .filter(|quantity| *quantity >= 2)
        else {
            error.set(Some(
                "Contained quantity must be a whole number of at least 2.".to_owned(),
            ));
            return;
        };
        let request = AddItemPackLink {
            master_item_id: item_id,
            single_item_id,
            inner_qty,
            notes: optional_text(&pack_notes.get_untracked()),
        };
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::internal_post::<_, i64>("/api/items/pack-links/add", &request).await {
                Ok(id) => {
                    contained_item_id.set(String::new());
                    contained_quantity.set("2".to_owned());
                    pack_notes.set(String::new());
                    toasts.success(format!("Pack conversion #{id} added."));
                    pending.set(false);
                    store.refresh();
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
        <div class="catalog-detail">
            <div class="catalog-editor-heading">
                <div>
                    <p class="eyebrow">{format!("Item #{item_id}")}</p>
                    <h2>{item.description.clone().unwrap_or_else(|| "Unnamed item".to_owned())}</h2>
                </div>
                <span class=if inactive { "status muted" } else { "status open" }>
                    {if inactive { "Inactive" } else { "Active" }}
                </span>
            </div>

            <form class="catalog-form compact-form" on:submit=save>
                <div class="form-grid two">
                    <label>
                        <span>"Description"</span>
                        <input type="text" required prop:value=move || description.get() on:input=move |event| description.set(event_target_value(&event))/>
                    </label>
                    <label>
                        <span>"Packaging"</span>
                        <select prop:value=move || packaging.get() on:change=move |event| packaging.set(event_target_value(&event))>
                            <option value="each">"Each"</option>
                            <option value="case">"Case"</option>
                        </select>
                    </label>
                </div>
                <label>
                    <span>"Notes"</span>
                    <input type="text" prop:value=move || notes.get() on:input=move |event| notes.set(event_target_value(&event))/>
                </label>
                <div class="catalog-form-actions split">
                    <button class="button primary-action compact" type="submit" disabled=move || pending.get() || inactive>
                        "Save item"
                    </button>
                    <button
                        class=if inactive {
                            "button secondary-action compact"
                        } else {
                            "button danger-action compact"
                        }
                        type="button"
                        disabled=move || pending.get()
                        on:click=change_active
                    >
                        {if inactive { "Reactivate" } else { "Deactivate" }}
                    </button>
                </div>
            </form>

            <InlineError error/>

            <section class="catalog-subsection">
                <div class="catalog-subheading">
                    <h3>"Pack conversions"</h3>
                    <span>{move || active_pack_links_for_item(&store.data.get().item_pack_links, item_id).len()}</span>
                </div>
                <form class="inline-command pack-command" on:submit=add_pack_conversion>
                    <label>
                        <span class="sr-only">"Contained item"</span>
                        <select required prop:value=move || contained_item_id.get() on:change=move |event| contained_item_id.set(event_target_value(&event))>
                            <option value="">"Select item"</option>
                            {pack_options.with_value(|items| items.iter().map(|(id, label)| view! { <option value=id.to_string()>{format!("{label} / #{id}")}</option> }).collect_view())}
                        </select>
                    </label>
                    <label>
                        <span class="sr-only">"Contained quantity"</span>
                        <input type="number" min="2" step="1" aria-label="Contained quantity" prop:value=move || contained_quantity.get() on:input=move |event| contained_quantity.set(event_target_value(&event))/>
                    </label>
                    <label>
                        <span class="sr-only">"Pack conversion notes"</span>
                        <input type="text" placeholder="Notes (optional)" prop:value=move || pack_notes.get() on:input=move |event| pack_notes.set(event_target_value(&event))/>
                    </label>
                    <button class="button secondary-action compact" type="submit" disabled=move || pending.get() || inactive>"Add conversion"</button>
                </form>
                <div class="identifier-list pack-conversion-list">
                    {move || {
                        let data = store.data.get();
                        let links = active_pack_links_for_item(&data.item_pack_links, item_id);
                        if links.is_empty() {
                            view! { <p class="catalog-empty">"No pack relationships defined."</p> }.into_any()
                        } else {
                            links.into_iter().map(|link| {
                                let link_id = link.id;
                                let (label, conversion) = pack_conversion_label(&link, &data.items, item_id);
                                let remove = move |_| {
                                    if pending.get_untracked() {
                                        return;
                                    }
                                    pending.set(true);
                                    error.set(None);
                                    let request = ItemPackLinkIdRequest { item_pack_link_id: link_id };
                                    leptos::task::spawn_local(async move {
                                        handle_bool_command(
                                            store,
                                            "/api/items/pack-links/delete",
                                            &request,
                                            format!("Pack conversion #{link_id} removed."),
                                            pending,
                                            error,
                                            toasts,
                                        )
                                        .await;
                                    });
                                };
                                view! {
                                    <div class="identifier-row pack-conversion-row">
                                        <strong>{label}</strong>
                                        <span>{conversion}</span>
                                        <button class="button barcode-action danger-action" type="button" aria-label=format!("Remove pack conversion {link_id}") title="Remove pack conversion" disabled=move || pending.get() || inactive on:click=remove><Icon icon=UiIcon::Remove/></button>
                                    </div>
                                }
                            }).collect_view().into_any()
                        }
                    }}
                </div>
            </section>

            <section class="catalog-subsection">
                <div class="catalog-subheading">
                    <h3>"SKUs"</h3>
                    <span>{item.skus.len()}</span>
                </div>
                <form class="inline-command" on:submit=add_sku>
                    <label>
                        <span class="sr-only">"SKU"</span>
                        <input type="text" placeholder="New SKU" prop:value=move || sku.get() on:input=move |event| sku.set(event_target_value(&event))/>
                    </label>
                    <label class="wide">
                        <span class="sr-only">"SKU notes"</span>
                        <input type="text" placeholder="Notes (optional)" prop:value=move || sku_notes.get() on:input=move |event| sku_notes.set(event_target_value(&event))/>
                    </label>
                    <button class="button secondary-action compact" type="submit" disabled=move || pending.get() || inactive>"Add SKU"</button>
                </form>
                <div class="identifier-list">
                    {if item.skus.is_empty() {
                        view! { <p class="catalog-empty">"No SKUs assigned."</p> }.into_any()
                    } else {
                        item.skus
                            .into_iter()
                            .map(|sku| {
                                view! {
                                    <div class="identifier-row">
                                        <code>{sku.name}</code>
                                        <span>{sku.notes.unwrap_or_default()}</span>
                                        <small>{format!("#{}", sku.id)}</small>
                                    </div>
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </div>
            </section>

            <section class="catalog-subsection">
                <div class="catalog-subheading">
                    <h3>"Barcodes"</h3>
                    <span>{item.barcodes.len()}</span>
                </div>
                <form class="inline-command barcode-command" on:submit=add_barcode>
                    <label>
                        <span class="sr-only">"Barcode value"</span>
                        <input type="text" placeholder="Barcode value" prop:value=move || barcode.get() on:input=move |event| barcode.set(event_target_value(&event))/>
                    </label>
                    <label>
                        <span class="sr-only">"Barcode type"</span>
                        <select prop:value=move || barcode_type.get() on:change=move |event| barcode_type.set(event_target_value(&event))>
                            <option value="code128">"Code 128"</option>
                            <option value="gs1-128">"GS1-128"</option>
                            <option value="upc-a">"UPC-A"</option>
                            <option value="qr">"QR Code"</option>
                        </select>
                    </label>
                    <label class="wide">
                        <span class="sr-only">"Barcode notes"</span>
                        <input type="text" placeholder="Notes (optional)" prop:value=move || barcode_notes.get() on:input=move |event| barcode_notes.set(event_target_value(&event))/>
                    </label>
                    <button class="button secondary-action compact" type="submit" disabled=move || pending.get() || inactive>"Add"</button>
                </form>
                {move || {
                    let value = barcode.get();
                    if value.trim().is_empty() {
                        None
                    } else {
                        Some(match barcode_label(&barcode_type.get(), &value) {
                            Ok(label) => view! {
                                <div class="barcode-draft-preview" aria-label="Barcode preview">
                                    <div class="barcode-symbol" inner_html=label.svg></div>
                                </div>
                            }
                            .into_any(),
                            Err(message) => view! {
                                <div class="barcode-validation" role="status">{message}</div>
                            }
                            .into_any(),
                        })
                    }
                }}
                <div class="barcode-list">
                    {if item.barcodes.is_empty() {
                        view! { <p class="catalog-empty">"No barcodes assigned."</p> }.into_any()
                    } else {
                        item.barcodes
                            .into_iter()
                            .map(|existing| {
                                let barcode_id = existing.id;
                                let value = existing.name.clone();
                                let label = barcode_label(&existing.r#type, &existing.name);
                                let remove = move |_| {
                                    if pending.get_untracked() {
                                        return;
                                    }
                                    pending.set(true);
                                    error.set(None);
                                    let request = BarcodeIdRequest { barcode_id };
                                    let value = value.clone();
                                    leptos::task::spawn_local(async move {
                                        handle_bool_command(
                                            store,
                                            "/api/items/barcodes/delete",
                                            &request,
                                            format!("Barcode {value} removed."),
                                            pending,
                                            error,
                                            toasts,
                                        )
                                        .await;
                                    });
                                };
                                match label {
                                    Ok(label) => {
                                        let normalized_value = label.normalized_value.clone();
                                        let barcode_kind = barcode_type_label(&existing.r#type);
                                        let accessible_label = format!(
                                            "{barcode_kind} barcode {normalized_value}"
                                        );
                                        let displayed_value = normalized_value.clone();
                                        let displayed_kind = barcode_kind.clone();
                                        view! {
                                            <div
                                                class="barcode-row"
                                                class:print-target=move || {
                                                    print_barcode_id.get() == Some(barcode_id)
                                                }
                                            >
                                                <div
                                                    class="barcode-label"
                                                    aria-label=accessible_label
                                                >
                                                    <div class="barcode-symbol" inner_html=label.svg></div>
                                                    <div class="barcode-data print-hide">
                                                        <code>{displayed_value}</code>
                                                        <span>{displayed_kind}</span>
                                                    </div>
                                                </div>
                                                <button
                                                    class="button barcode-action quiet-action print-hide"
                                                    type="button"
                                                    aria-label="Print barcode label"
                                                    title="Print barcode label"
                                                    on:click=move |_| {
                                                        print_barcode_label(
                                                            barcode_id,
                                                            print_barcode_id,
                                                        )
                                                    }
                                                >
                                                    <Icon icon=UiIcon::Print/>
                                                </button>
                                                <button
                                                    class="button barcode-action danger-action print-hide"
                                                    type="button"
                                                    aria-label="Remove barcode"
                                                    title="Remove barcode"
                                                    disabled=move || pending.get() || inactive
                                                    on:click=remove
                                                >
                                                    <Icon icon=UiIcon::Remove/>
                                                </button>
                                            </div>
                                        }
                                        .into_any()
                                    }
                                    Err(message) => view! {
                                        <div class="barcode-row barcode-invalid">
                                            <div>
                                                <code>{existing.name}</code>
                                                <span>{message}</span>
                                            </div>
                                            <button
                                                class="button barcode-action danger-action print-hide"
                                                type="button"
                                                aria-label="Remove invalid barcode"
                                                title="Remove invalid barcode"
                                                disabled=move || pending.get() || inactive
                                                on:click=remove
                                            >
                                                <Icon icon=UiIcon::Remove/>
                                            </button>
                                        </div>
                                    }
                                    .into_any(),
                                }
                            })
                            .collect_view()
                            .into_any()
                    }}
                </div>
            </section>
        </div>
    }
}

#[component]
fn InlineError(error: RwSignal<Option<String>>) -> impl IntoView {
    move || {
        error.get().map(|message| {
            view! {
                <div class="catalog-inline-error" role="alert">{message}</div>
            }
        })
    }
}

async fn handle_bool_command<T>(
    store: CatalogStore,
    path: &'static str,
    request: &T,
    success: String,
    pending: RwSignal<bool>,
    error: RwSignal<Option<String>>,
    toasts: crate::toast::ToastBus,
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
            let message = "The item changed or is no longer in your scope.".to_owned();
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

fn selected_item(items: &[Item], selected_id: Option<i64>) -> Option<Item> {
    items
        .iter()
        .find(|item| Some(item.id) == selected_id)
        .cloned()
}

fn visible_items(
    items: &[Item],
    filter: &str,
    show_inactive: bool,
    sort: SortSpec<ItemSort>,
) -> Vec<Item> {
    let query = filter.trim().to_ascii_lowercase();
    let mut visible = items
        .iter()
        .filter(|item| {
            (show_inactive || item.deleted.is_none())
                && (query.is_empty()
                    || item
                        .description
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase()
                        .contains(&query)
                    || item
                        .skus
                        .iter()
                        .any(|sku| sku.name.to_ascii_lowercase().contains(&query))
                    || item
                        .barcodes
                        .iter()
                        .any(|barcode| barcode.name.to_ascii_lowercase().contains(&query)))
        })
        .cloned()
        .collect::<Vec<_>>();
    visible.sort_by(|left, right| {
        let ordering = compare_items(left, right, sort.key).then_with(|| left.id.cmp(&right.id));
        if sort.direction == SortDirection::Ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    visible
}

fn compare_items(left: &Item, right: &Item, key: ItemSort) -> Ordering {
    match key {
        ItemSort::Id => left.id.cmp(&right.id),
        ItemSort::Description => {
            normalized(left.description.as_deref()).cmp(&normalized(right.description.as_deref()))
        }
        ItemSort::Packaging => left.packaging_unit.cmp(&right.packaging_unit),
        ItemSort::Skus => left.skus.len().cmp(&right.skus.len()),
        ItemSort::Barcodes => left.barcodes.len().cmp(&right.barcodes.len()),
        ItemSort::Status => left.deleted.is_some().cmp(&right.deleted.is_some()),
    }
}

fn normalized(value: Option<&str>) -> String {
    value.unwrap_or_default().trim().to_ascii_lowercase()
}

fn optional_nonnegative(value: &str) -> Result<Option<i64>, ()> {
    let value = value.trim();
    if value.is_empty() {
        return Ok(None);
    }
    value
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .map(Some)
        .ok_or(())
}

fn packaging_label(value: &str) -> String {
    match value {
        "each" => "Each".to_owned(),
        "case" => "Case".to_owned(),
        other => other.to_owned(),
    }
}

fn barcode_type_label(value: &str) -> String {
    match value {
        "code128" => "Code 128".to_owned(),
        "gs1-128" => "GS1-128".to_owned(),
        "upc-a" => "UPC-A".to_owned(),
        "qr" => "QR Code".to_owned(),
        other => other.to_owned(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct BarcodeLabel {
    normalized_value: String,
    svg: String,
}

fn barcode_label(kind: &str, value: &str) -> Result<BarcodeLabel, String> {
    let normalized_value = wareboxes_barcodes::normalized_value(kind, value)
        .map_err(|error| format!("Barcode cannot be scanned: {error}."))?;
    let svg = wareboxes_barcodes::svg(kind, &normalized_value)
        .map_err(|error| format!("Barcode cannot be rendered: {error}."))?;
    Ok(BarcodeLabel {
        normalized_value,
        svg,
    })
}

#[cfg(target_arch = "wasm32")]
fn print_barcode_label(barcode_id: i64, selected_id: RwSignal<Option<i64>>) {
    selected_id.set(Some(barcode_id));
    if let Some(window) = web_sys::window() {
        let print_window = window.clone();
        let callback = Closure::<dyn FnMut()>::new(move || {
            let _ = print_window.print();
            selected_id.set(None);
        });
        let _ = window.request_animation_frame(callback.as_ref().unchecked_ref());
        callback.forget();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn print_barcode_label(_barcode_id: i64, _selected_id: RwSignal<Option<i64>>) {}

#[cfg(test)]
mod tests {
    use wareboxes_core::models::Item;

    use super::{barcode_label, optional_nonnegative, visible_items, ItemSort};
    use crate::sorting::{SortDirection, SortSpec};

    fn item(id: i64, description: &str, deleted: bool) -> Item {
        serde_json::from_value(serde_json::json!({
            "id": id,
            "tenant_id": 1,
            "created": "2026-01-01T00:00:00Z",
            "deleted": deleted.then_some("2026-01-02T00:00:00Z"),
            "description": description,
            "notes": null,
            "packaging_unit": "each",
            "dims_id": null,
            "pallet_hi": null,
            "pallet_ti": null,
            "inner_units": null,
            "skus": [],
            "barcodes": []
        }))
        .expect("test item should deserialize")
    }

    #[test]
    fn active_filter_and_sort_are_deterministic() {
        let items = vec![
            item(2, "Widget B", false),
            item(1, "Widget A", false),
            item(3, "Old", true),
        ];
        let visible = visible_items(
            &items,
            "widget",
            false,
            SortSpec {
                key: ItemSort::Description,
                direction: SortDirection::Ascending,
            },
        );
        assert_eq!(
            visible.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![1, 2]
        );
    }

    #[test]
    fn optional_dimensions_reject_negative_and_non_numeric_values() {
        assert_eq!(optional_nonnegative(""), Ok(None));
        assert_eq!(optional_nonnegative("12"), Ok(Some(12)));
        assert_eq!(optional_nonnegative("-1"), Err(()));
        assert_eq!(optional_nonnegative("1.5"), Err(()));
    }

    #[test]
    fn barcode_labels_normalize_validate_and_render_scannable_symbols() {
        let upc = barcode_label("upc-a", "03600029145").unwrap();
        assert_eq!(upc.normalized_value, "036000291452");
        assert!(upc.svg.contains("<rect"));
        assert!(upc.svg.contains("036000291452"));

        let qr = barcode_label("qr", "WAREBOXES-42").unwrap();
        assert!(qr.svg.contains("<svg"));
        assert!(qr.svg.matches("<rect").count() > 20);

        assert!(barcode_label("upc-a", "123").is_err());
        assert!(barcode_label("code128", "\n").is_err());
    }
}
