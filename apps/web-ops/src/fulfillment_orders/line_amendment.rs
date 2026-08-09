use std::collections::HashSet;

use super::*;
use wareboxes_api_contract::v1::{
    ReplaceFulfillmentOrderLineRequest, ReplaceFulfillmentOrderLinesRequest,
};

#[component]
pub(super) fn OrderLineAmendmentEditor(
    order: Order,
    on_close: Callback<()>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let order_id = order.id;
    let inventory_owner_id = order.inventory_owner_id;
    let revision = order.revision;
    let lines = RwSignal::new(
        order
            .order_items
            .iter()
            .map(|line| DraftOrderLine {
                line_key: line.line_key.clone(),
                item_id: line.item_id,
                description: line
                    .item_description
                    .clone()
                    .unwrap_or_else(|| format!("Item #{}", line.item_id)),
                requested_uom: line.uom.clone(),
                quantity: line.qty,
            })
            .collect::<Vec<_>>(),
    );
    let next_line_number = RwSignal::new(
        order
            .order_items
            .iter()
            .map(|line| line.line_number)
            .max()
            .unwrap_or_default()
            .saturating_add(1),
    );
    let item_search = RwSignal::new(String::new());
    let catalog_query = RwSignal::new(String::new());
    let entry_items = RwSignal::new(Vec::<OrderEntryItemResponse>::new());
    let items_pending = RwSignal::new(false);
    let selected_item_id = RwSignal::new(String::new());
    let add_quantity = RwSignal::new("1".to_owned());
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let retry = RwSignal::new(None::<(ReplaceFulfillmentOrderLinesRequest, String)>);
    let toasts = use_toast_bus();

    Effect::new(move |_| {
        let search = item_search.get();
        #[cfg(target_arch = "wasm32")]
        set_timeout(
            move || {
                if item_search.try_get_untracked().as_ref() == Some(&search)
                    && catalog_query.try_get_untracked().as_ref() != Some(&search)
                {
                    let _ = catalog_query.try_set(search);
                }
            },
            Duration::from_millis(250),
        );
        #[cfg(not(target_arch = "wasm32"))]
        catalog_query.set(search);
    });

    Effect::new(move |_| {
        let search = catalog_query.get();
        entry_items.set(Vec::new());
        selected_item_id.set(String::new());
        items_pending.set(true);
        leptos::task::spawn_local(async move {
            match api::order_entry_items(inventory_owner_id, &search).await {
                Ok(items) if catalog_query.try_get_untracked().as_ref() == Some(&search) => {
                    let _ = entry_items.try_set(items);
                    let _ = items_pending.try_set(false);
                }
                Ok(_) => {}
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) if catalog_query.try_get_untracked().as_ref() == Some(&search) => {
                    let _ = error.try_set(Some(api_error.message));
                    let _ = items_pending.try_set(false);
                }
                Err(_) => {}
            }
        });
    });

    let add_line = move |_| {
        if pending.get_untracked() || retry.get_untracked().is_some() {
            return;
        }
        let Ok(item_id) = selected_item_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose an item to add.".to_owned()));
            return;
        };
        let Ok(quantity) = add_quantity.get_untracked().parse::<i64>() else {
            error.set(Some("Quantity must be a positive whole number.".to_owned()));
            return;
        };
        if quantity <= 0 {
            error.set(Some("Quantity must be a positive whole number.".to_owned()));
            return;
        }
        let Some(item) = entry_items
            .get_untracked()
            .into_iter()
            .find(|item| item.item_id == item_id)
        else {
            error.set(Some("The selected item is no longer available.".to_owned()));
            return;
        };
        let mut current = lines.get_untracked();
        let mut line_number = next_line_number.get_untracked();
        let line_key = loop {
            let candidate = line_number.to_string();
            line_number = line_number.saturating_add(1);
            if current.iter().all(|line| line.line_key != candidate) {
                break candidate;
            }
        };
        current.push(DraftOrderLine {
            line_key,
            item_id,
            description: item
                .description
                .unwrap_or_else(|| format!("Item #{item_id}")),
            requested_uom: item.requested_uom,
            quantity,
        });
        next_line_number.set(line_number);
        lines.set(current);
        selected_item_id.set(String::new());
        add_quantity.set("1".to_owned());
        error.set(None);
    };

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let (request, idempotency_key) = match retry.get_untracked() {
            Some(saved) => saved,
            None => {
                let request = match replacement_request(revision, &lines.get_untracked()) {
                    Ok(request) => request,
                    Err(message) => {
                        error.set(Some(message));
                        return;
                    }
                };
                (request, api::new_idempotency_key())
            }
        };
        retry.set(Some((request.clone(), idempotency_key.clone())));
        pending.set(true);
        error.set(None);
        leptos::task::spawn_local(async move {
            match api::replace_fulfillment_order_lines(order_id, &request, &idempotency_key).await {
                Ok(result) => {
                    retry.set(None);
                    pending.set(false);
                    toasts.success(format!(
                        "Demand replaced at revision {}. {} reservation(s) and {} allocation(s) released.",
                        result.revision.get(),
                        result.released_reservation_count,
                        result.released_allocation_count,
                    ));
                    on_close.run(());
                    on_refreshed.run(order_id);
                }
                Err(api_error) if api_error.unauthorized => {
                    retry.set(None);
                    pending.set(false);
                    on_unauthorized.run(());
                }
                Err(api_error) if api_error.ambiguous_outcome => {
                    pending.set(false);
                    error.set(Some(format!(
                        "{} The exact replacement is retained for retry.",
                        api_error.message
                    )));
                }
                Err(api_error) => {
                    retry.set(None);
                    pending.set(false);
                    error.set(Some(api_error.message.clone()));
                    toasts.error(api_error.message);
                    on_refreshed.run(order_id);
                }
            }
        });
    };

    view! {
        <form class="order-line-amendment" on:submit=submit>
            <div class="order-line-amendment-heading">
                <div>
                    <strong>"Replace demand lines"</strong>
                    <span>"Exact replacement retires current lines and releases active commitments."</span>
                </div>
                <button
                    type="button"
                    class="icon-button"
                    title="Close line editor"
                    aria-label="Close line editor"
                    disabled=move || pending.get() || retry.get().is_some()
                    on:click=move |_| on_close.run(())
                >
                    <Icon icon=UiIcon::Close/>
                </button>
            </div>
            <div class="order-line-entry order-line-amendment-add">
                <label>
                    <span>"Find item"</span>
                    <input
                        type="search"
                        autocomplete="off"
                        placeholder="SKU, barcode, or description"
                        disabled=move || pending.get() || retry.get().is_some()
                        prop:value=move || item_search.get()
                        on:input=move |event| item_search.set(event_target_value(&event))
                    />
                </label>
                <label>
                    <span>"Item"</span>
                    <select
                        disabled=move || pending.get() || retry.get().is_some() || items_pending.get()
                        prop:value=move || selected_item_id.get()
                        on:change=move |event| selected_item_id.set(event_target_value(&event))
                    >
                        <option value="">{move || if items_pending.get() { "Loading items" } else { "Select item" }}</option>
                        {move || entry_items
                            .get()
                            .into_iter()
                            .map(|item| {
                                let label = item.description.unwrap_or_else(|| format!("Item #{}", item.item_id));
                                view! { <option value=item.item_id>{format!("{label} - {}", item.requested_uom)}</option> }
                            })
                            .collect_view()}
                    </select>
                </label>
                <label>
                    <span>"Quantity"</span>
                    <input
                        type="number"
                        min="1"
                        step="1"
                        disabled=move || pending.get() || retry.get().is_some()
                        prop:value=move || add_quantity.get()
                        on:input=move |event| add_quantity.set(event_target_value(&event))
                    />
                </label>
                <button
                    type="button"
                    class="button secondary-action order-line-add"
                    disabled=move || pending.get() || retry.get().is_some() || items_pending.get()
                    on:click=add_line
                >
                    <Icon icon=UiIcon::Add/>
                    "Add line"
                </button>
            </div>
            <div class="table-scroll order-lines-draft-scroll">
                <table class="data-table order-lines-draft-table order-line-amendment-table">
                    <thead>
                        <tr><th>"Line"</th><th>"Item"</th><th>"UOM"</th><th>"Qty"</th><th></th></tr>
                    </thead>
                    <tbody>
                        {move || lines
                            .get()
                            .into_iter()
                            .enumerate()
                            .map(|(index, line)| {
                                view! {
                                    <tr>
                                        <td>
                                            <input
                                                aria-label=format!("Line {} key", index + 1)
                                                maxlength="200"
                                                disabled=move || pending.get() || retry.get().is_some()
                                                value=line.line_key
                                                on:input=move |event| {
                                                    let value = event_target_value(&event);
                                                    lines.update(|current| {
                                                        if let Some(line) = current.get_mut(index) {
                                                            line.line_key = value;
                                                        }
                                                    });
                                                }
                                            />
                                        </td>
                                        <td>
                                            <strong>{line.description}</strong>
                                            <small class="cell-detail">{format!("Item #{}", line.item_id)}</small>
                                        </td>
                                        <td>{line.requested_uom}</td>
                                        <td>
                                            <input
                                                class="line-quantity-input"
                                                aria-label=format!("Line {} quantity", index + 1)
                                                type="number"
                                                min="1"
                                                step="1"
                                                disabled=move || pending.get() || retry.get().is_some()
                                                value=line.quantity
                                                on:input=move |event| {
                                                    if let Ok(quantity) = event_target_value(&event).parse::<i64>() {
                                                        lines.update(|current| {
                                                            if let Some(line) = current.get_mut(index) {
                                                                line.quantity = quantity;
                                                            }
                                                        });
                                                    }
                                                }
                                            />
                                        </td>
                                        <td>
                                            <button
                                                type="button"
                                                class="icon-button danger-icon"
                                                title="Remove demand line"
                                                aria-label=format!("Remove line {}", index + 1)
                                                disabled=move || pending.get() || retry.get().is_some()
                                                on:click=move |_| lines.update(|current| { current.remove(index); })
                                            >
                                                <Icon icon=UiIcon::Remove/>
                                            </button>
                                        </td>
                                    </tr>
                                }
                            })
                            .collect_view()}
                    </tbody>
                </table>
            </div>
            <Show when=move || retry.get().is_some()>
                <p class="inline-command-note" role="status">
                    "The exact line set and idempotency key are retained for retry."
                </p>
            </Show>
            <Show when=move || error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || error.get().unwrap_or_default()}</p>
            </Show>
            <div class="form-actions">
                <span class="order-line-amendment-total">{move || format!("{} lines / {} units", lines.get().len(), lines.get().iter().map(|line| line.quantity).sum::<i64>())}</span>
                <button
                    type="submit"
                    class="button primary-action"
                    disabled=move || pending.get()
                >
                    {move || if pending.get() { "Replacing" } else if retry.get().is_some() { "Retry exact replacement" } else { "Replace lines" }}
                </button>
            </div>
        </form>
    }
}

fn replacement_request(
    revision: i64,
    lines: &[DraftOrderLine],
) -> Result<ReplaceFulfillmentOrderLinesRequest, String> {
    let expected_revision = Revision::new(revision)
        .map_err(|_| "The order revision is invalid. Refresh the order.".to_owned())?;
    if lines.is_empty() {
        return Err("At least one demand line is required.".to_owned());
    }
    let mut keys = HashSet::with_capacity(lines.len());
    let mut result = Vec::with_capacity(lines.len());
    for line in lines {
        let line_key = line.line_key.trim();
        if line_key.is_empty() {
            return Err("Every demand line requires a line key.".to_owned());
        }
        if !keys.insert(line_key.to_owned()) {
            return Err(format!("Line key {line_key} is duplicated."));
        }
        if line.quantity <= 0 {
            return Err(format!(
                "Line {line_key} quantity must be a positive whole number."
            ));
        }
        result.push(ReplaceFulfillmentOrderLineRequest {
            line_key: line_key.to_owned(),
            item_id: line.item_id,
            quantity: line.quantity,
            requested_uom: line.requested_uom.clone(),
        });
    }
    Ok(ReplaceFulfillmentOrderLinesRequest {
        expected_revision,
        lines: result,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(key: &str, quantity: i64) -> DraftOrderLine {
        DraftOrderLine {
            line_key: key.to_owned(),
            item_id: 41,
            description: "Test item".to_owned(),
            requested_uom: "case".to_owned(),
            quantity,
        }
    }

    #[test]
    fn exact_replacement_request_preserves_order_and_rejects_invalid_sets() {
        let request = replacement_request(3, &[line("B", 2), line("A", 4)]).unwrap();
        assert_eq!(request.expected_revision.get(), 3);
        assert_eq!(request.lines[0].line_key, "B");
        assert_eq!(request.lines[1].line_key, "A");
        assert!(replacement_request(3, &[]).is_err());
        assert!(replacement_request(3, &[line("A", 1), line("A", 2)]).is_err());
        assert!(replacement_request(3, &[line("A", 0)]).is_err());
    }
}
