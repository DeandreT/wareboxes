use super::*;

#[component]
pub(super) fn OrderDetailPanel(
    order: Order,
    tab: RwSignal<OrderDetailTab>,
    facilities: Vec<AccessScopeResource>,
    locations: Vec<Location>,
    pending: RwSignal<bool>,
    load_error: RwSignal<Option<String>>,
    on_refreshed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let order_id = order.id;
    let cancellation_order_key = StoredValue::new(order.order_key.clone());
    let rush = RwSignal::new(order.rush);
    let ship_by = RwSignal::new(timestamp_input(order.ship_by));
    let recipient_name = RwSignal::new(order.recipient_name.clone().unwrap_or_default());
    let company = RwSignal::new(order.destination_company.clone().unwrap_or_default());
    let phone = RwSignal::new(order.destination_phone.clone().unwrap_or_default());
    let email = RwSignal::new(order.destination_email.clone().unwrap_or_default());
    let line1 = RwSignal::new(order.line1.clone().unwrap_or_default());
    let line2 = RwSignal::new(order.line2.clone().unwrap_or_default());
    let city = RwSignal::new(order.city.clone().unwrap_or_default());
    let state = RwSignal::new(order.state.clone().unwrap_or_default());
    let postal_code = RwSignal::new(order.postal_code.clone().unwrap_or_default());
    let country = RwSignal::new(order.country.clone().unwrap_or_default());
    let command_pending = RwSignal::new(false);
    let command_error = RwSignal::new(None::<String>);
    let amendment_retry = RwSignal::new(None::<(AmendFulfillmentOrderRequest, String)>);
    let cancel_open = RwSignal::new(false);
    let hold_open = RwSignal::new(false);
    let hold_reason = RwSignal::new("customer_request".to_owned());
    let hold_note = RwSignal::new(String::new());
    let release_candidate = RwSignal::new(None::<i64>);
    let release_note = RwSignal::new(String::new());
    let reservation_item_names = StoredValue::new(
        order
            .order_items
            .iter()
            .filter_map(|line| {
                line.item_description
                    .clone()
                    .map(|description| (line.item_id, description))
            })
            .collect::<Vec<_>>(),
    );
    let facility_names = StoredValue::new(
        facilities
            .iter()
            .map(|facility| (facility.id, facility.name.clone()))
            .collect::<Vec<_>>(),
    );
    let facilities = StoredValue::new(facilities);
    let locations = StoredValue::new(locations);
    let toasts = use_toast_bus();

    let save = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if command_pending.get_untracked() {
            return;
        }
        let (request, idempotency_key) = match amendment_retry.get_untracked() {
            Some(attempt) => attempt,
            None => {
                let recipient_name_value = recipient_name.get_untracked().trim().to_owned();
                let line1_value = line1.get_untracked().trim().to_owned();
                let city_value = city.get_untracked().trim().to_owned();
                let state_value = state.get_untracked().trim().to_owned();
                let postal_code_value = postal_code.get_untracked().trim().to_owned();
                let country_value = country.get_untracked().trim().to_owned();
                if [
                    recipient_name_value.as_str(),
                    line1_value.as_str(),
                    city_value.as_str(),
                    state_value.as_str(),
                    postal_code_value.as_str(),
                    country_value.as_str(),
                ]
                .into_iter()
                .any(str::is_empty)
                {
                    command_error.set(Some(
                        "Complete the required shipping destination fields.".to_owned(),
                    ));
                    return;
                }
                let ship_by_value = match parse_optional_timestamp(&ship_by.get_untracked()) {
                    Ok(value) => value,
                    Err(message) => {
                        command_error.set(Some(message));
                        return;
                    }
                };
                let expected_revision = match Revision::new(order.revision) {
                    Ok(revision) => revision,
                    Err(_) => {
                        command_error.set(Some(
                            "The order revision is invalid. Refresh the order.".to_owned(),
                        ));
                        return;
                    }
                };
                (
                    AmendFulfillmentOrderRequest {
                        expected_revision,
                        rush: rush.get_untracked(),
                        ship_by: ship_by_value.map(|value| value.to_rfc3339()),
                        destination: FulfillmentOrderDestination {
                            recipient_name: recipient_name_value,
                            company: optional_text(&company.get_untracked()),
                            phone: optional_text(&phone.get_untracked()),
                            email: optional_text(&email.get_untracked()),
                            line1: line1_value,
                            line2: optional_text(&line2.get_untracked()),
                            city: city_value,
                            region: state_value,
                            postal_code: postal_code_value,
                            country: country_value,
                        },
                    },
                    api::new_idempotency_key(),
                )
            }
        };
        amendment_retry.set(Some((request.clone(), idempotency_key.clone())));
        command_pending.set(true);
        command_error.set(None);
        leptos::task::spawn_local(async move {
            match api::amend_fulfillment_order(order_id, &request, &idempotency_key).await {
                Ok(result) => {
                    amendment_retry.set(None);
                    command_pending.set(false);
                    toasts.success(format!(
                        "Order header amended at revision {}.",
                        result.revision.get()
                    ));
                    on_refreshed.run(order_id);
                }
                Err(api_error) if api_error.unauthorized => {
                    amendment_retry.set(None);
                    command_pending.set(false);
                    on_unauthorized.run(());
                }
                Err(api_error) if api_error.ambiguous_outcome => {
                    command_error.set(Some(format!(
                        "{} The result is unknown; retry sends the exact saved amendment.",
                        api_error.message
                    )));
                    command_pending.set(false);
                }
                Err(api_error) => {
                    amendment_retry.set(None);
                    toasts.error(api_error.message.clone());
                    command_error.set(Some(api_error.message));
                    command_pending.set(false);
                    on_refreshed.run(order_id);
                }
            }
        });
    };

    let place_hold = move |_| {
        if command_pending.get_untracked() || amendment_retry.get_untracked().is_some() {
            return;
        }
        let Some(reason) = parse_order_hold_reason(&hold_reason.get_untracked()) else {
            command_error.set(Some("Choose a valid hold reason.".to_owned()));
            return;
        };
        let note = optional_text(&hold_note.get_untracked());
        if reason == OrderHoldRequestReason::Other && note.is_none() {
            command_error.set(Some("Add a note for an Other hold.".to_owned()));
            return;
        }
        let request = PlaceOrderHoldRequest { reason, note };
        command_pending.set(true);
        command_error.set(None);
        let idempotency_key = api::new_idempotency_key();
        leptos::task::spawn_local(async move {
            match api::place_order_hold(order_id, &request, &idempotency_key).await {
                Ok(result) => {
                    hold_open.set(false);
                    hold_note.set(String::new());
                    command_pending.set(false);
                    toasts.success(format!(
                        "Order hold #{} placed. {} active hold(s).",
                        result.hold_id, result.active_hold_count
                    ));
                    on_refreshed.run(order_id);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    command_error.set(Some(api_error.message));
                    command_pending.set(false);
                }
            }
        });
    };

    let release_hold = move |_| {
        if command_pending.get_untracked() || amendment_retry.get_untracked().is_some() {
            return;
        }
        let Some(hold_id) = release_candidate.get_untracked() else {
            return;
        };
        let request = ReleaseOrderHoldRequest {
            note: optional_text(&release_note.get_untracked()),
        };
        command_pending.set(true);
        command_error.set(None);
        let idempotency_key = api::new_idempotency_key();
        leptos::task::spawn_local(async move {
            match api::release_order_hold(order_id, hold_id, &request, &idempotency_key).await {
                Ok(result) => {
                    release_candidate.set(None);
                    release_note.set(String::new());
                    command_pending.set(false);
                    toasts.success(if result.active_hold_count == 0 {
                        "The last order hold was released; the order is open.".to_owned()
                    } else {
                        format!(
                            "Hold #{hold_id} released. {} active hold(s) remain.",
                            result.active_hold_count
                        )
                    });
                    on_refreshed.run(order_id);
                }
                Err(api_error) if api_error.unauthorized => on_unauthorized.run(()),
                Err(api_error) => {
                    toasts.error(api_error.message.clone());
                    command_error.set(Some(api_error.message));
                    command_pending.set(false);
                }
            }
        });
    };

    view! {
        <div class="fulfillment-detail-content">
            <div class="detail-heading">
                <div>
                    <span class="eyebrow">{format!("Order #{}", order.id)}</span>
                    <h2>{order.order_key.clone()}</h2>
                </div>
                <span class=order_status_class(order.status)>{title_case(order.status.as_str())}</span>
            </div>
            <dl class="detail-facts four-column">
                <div>
                    <dt>"Client"</dt>
                    <dd>{order.inventory_owner_name.clone().unwrap_or_else(|| "Unassigned".to_owned())}</dd>
                </div>
                <div>
                    <dt>"Ordered"</dt>
                    <dd>{format_quantity(order.ordered_qty)}</dd>
                </div>
                <div>
                    <dt>"Reserved"</dt>
                    <dd>{format_quantity(order.reserved_qty)}</dd>
                </div>
                <div>
                    <dt>"Created"</dt>
                    <dd>{short_timestamp(order.created)}</dd>
                </div>
            </dl>
            <div class="detail-tabs" role="tablist" aria-label="Order detail sections">
                {[
                    (OrderDetailTab::Header, "Header"),
                    (OrderDetailTab::Lines, "Lines"),
                    (OrderDetailTab::Fulfillment, "Fulfillment"),
                    (OrderDetailTab::Holds, "Holds"),
                    (OrderDetailTab::Activity, "Activity"),
                ]
                    .into_iter()
                    .map(|(value, label)| {
                        view! {
                            <button
                                type="button"
                                role="tab"
                                aria-selected=move || (tab.get() == value).to_string()
                                class:active=move || tab.get() == value
                                on:click=move |_| tab.set(value)
                            >
                                {label}
                            </button>
                        }
                    })
                    .collect_view()}
            </div>

            <Show when=move || pending.get()>
                <div class="detail-loading" role="status">"Refreshing order..."</div>
            </Show>
            <Show when=move || load_error.get().is_some()>
                <p class="inline-command-error" role="alert">{move || load_error.get().unwrap_or_default()}</p>
            </Show>

            <Show when=move || tab.get() == OrderDetailTab::Header>
                <form class="fulfillment-form detail-form" on:submit=save>
                    <div class="form-grid two-column">
                        <label>
                            <span>"Order number"</span>
                            <input
                                readonly
                                value=order.order_key.clone()
                            />
                        </label>
                        <label>
                            <span>"Ship by (UTC)"</span>
                            <input
                                type="datetime-local"
                                disabled=move || amendment_retry.get().is_some()
                                prop:value=move || ship_by.get()
                                on:input=move |event| ship_by.set(event_target_value(&event))
                            />
                        </label>
                        <label class="checkbox-label">
                            <input
                                type="checkbox"
                                disabled=move || amendment_retry.get().is_some()
                                prop:checked=move || rush.get()
                                on:change=move |event| rush.set(event_target_checked(&event))
                            />
                            <span>"Rush order"</span>
                        </label>
                    </div>
                    <fieldset>
                        <legend>"Ship to"</legend>
                        <div class="form-grid two-column">
                            <label>
                                <span>"Recipient name"</span>
                                <input
                                    required
                                    autocomplete="name"
                                    disabled=move || amendment_retry.get().is_some()
                                    prop:value=move || recipient_name.get()
                                    on:input=move |event| recipient_name.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"Company"</span>
                                <input
                                    autocomplete="organization"
                                    disabled=move || amendment_retry.get().is_some()
                                    prop:value=move || company.get()
                                    on:input=move |event| company.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"Phone"</span>
                                <input
                                    autocomplete="tel"
                                    disabled=move || amendment_retry.get().is_some()
                                    prop:value=move || phone.get()
                                    on:input=move |event| phone.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"Email"</span>
                                <input
                                    type="email"
                                    autocomplete="email"
                                    disabled=move || amendment_retry.get().is_some()
                                    prop:value=move || email.get()
                                    on:input=move |event| email.set(event_target_value(&event))
                                />
                            </label>
                            <label class="wide">
                                <span>"Address line 1"</span>
                                <input
                                    disabled=move || amendment_retry.get().is_some()
                                    prop:value=move || line1.get()
                                    on:input=move |event| line1.set(event_target_value(&event))
                                />
                            </label>
                            <label class="wide">
                                <span>"Address line 2"</span>
                                <input
                                    disabled=move || amendment_retry.get().is_some()
                                    prop:value=move || line2.get()
                                    on:input=move |event| line2.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"City"</span>
                                <input
                                    disabled=move || amendment_retry.get().is_some()
                                    prop:value=move || city.get()
                                    on:input=move |event| city.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"State / region"</span>
                                <input
                                    disabled=move || amendment_retry.get().is_some()
                                    prop:value=move || state.get()
                                    on:input=move |event| state.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"Postal code"</span>
                                <input
                                    disabled=move || amendment_retry.get().is_some()
                                    prop:value=move || postal_code.get()
                                    on:input=move |event| postal_code.set(event_target_value(&event))
                                />
                            </label>
                            <label>
                                <span>"Country"</span>
                                <input
                                    disabled=move || amendment_retry.get().is_some()
                                    prop:value=move || country.get()
                                    on:input=move |event| country.set(event_target_value(&event))
                                />
                            </label>
                        </div>
                    </fieldset>
                    <Show when=move || amendment_retry.get().is_some()>
                        <p class="inline-command-note" role="status">
                            "The exact amendment and idempotency key are retained for retry."
                        </p>
                    </Show>
                    <Show when=move || command_error.get().is_some()>
                        <p class="inline-command-error" role="alert">
                            {move || command_error.get().unwrap_or_default()}
                        </p>
                    </Show>
                    <div class="form-actions">
                        <button
                            class="button primary-action"
                            type="submit"
                            disabled=move || command_pending.get() || !matches!(order.status, OrderStatus::Open | OrderStatus::Held)
                        >
                            {move || if command_pending.get() { "Saving" } else if amendment_retry.get().is_some() { "Retry exact amendment" } else { "Save header" }}
                        </button>
                        {can_place_order_hold(order.status).then(|| {
                            view! {
                                <button
                                    class="button secondary-action"
                                    type="button"
                                    disabled=move || amendment_retry.get().is_some()
                                    on:click=move |_| {
                                        cancel_open.set(false);
                                        hold_open.set(true);
                                    }
                                >
                                    <Icon icon=UiIcon::Holds/>
                                    "Place hold"
                                </button>
                            }
                        })}
                        {can_cancel_order(order.status).then(|| {
                            view! {
                                <button
                                    class="button danger-action"
                                    type="button"
                                    disabled=move || amendment_retry.get().is_some()
                                    on:click=move |_| {
                                        hold_open.set(false);
                                        cancel_open.set(true);
                                    }
                                >
                                    <Icon icon=UiIcon::Alert/>
                                    "Cancel order"
                                </button>
                            }
                        })}
                    </div>
                    <Show when=move || hold_open.get()>
                        <section class="confirmation-panel order-hold-panel" role="dialog" aria-labelledby="place-order-hold-title">
                            <h3 id="place-order-hold-title">"Place order hold"</h3>
                            <p>"Block release and execution until every active hold is cleared."</p>
                            <label>
                                <span>"Reason"</span>
                                <select
                                    prop:value=move || hold_reason.get()
                                    on:change=move |event| hold_reason.set(event_target_value(&event))
                                >
                                    <option value="address_review">"Address review"</option>
                                    <option value="compliance_review">"Compliance review"</option>
                                    <option value="customer_request">"Client request"</option>
                                    <option value="inventory_shortage">"Inventory shortage"</option>
                                    <option value="payment_review">"Payment review"</option>
                                    <option value="other">"Other"</option>
                                </select>
                            </label>
                            <label>
                                <span>"Note"</span>
                                <textarea
                                    maxlength="1000"
                                    rows="3"
                                    prop:value=move || hold_note.get()
                                    on:input=move |event| hold_note.set(event_target_value(&event))
                                ></textarea>
                            </label>
                            <div class="form-actions">
                                <button
                                    type="button"
                                    class="button primary-action"
                                    disabled=move || command_pending.get()
                                    on:click=place_hold
                                >
                                    <Icon icon=UiIcon::Holds/>
                                    {move || if command_pending.get() { "Placing" } else { "Place hold" }}
                                </button>
                                <button
                                    type="button"
                                    class="button secondary-action"
                                    on:click=move |_| hold_open.set(false)
                                >
                                    "Close"
                                </button>
                            </div>
                        </section>
                    </Show>
                    <Show when=move || cancel_open.get()>
                        <OrderCancellationPanel
                            order_id
                            order_key=cancellation_order_key.get_value()
                            revision=order.revision
                            processing=matches!(order.status, OrderStatus::Processing)
                            on_close=Callback::new(move |_| cancel_open.set(false))
                            on_refreshed
                            on_unauthorized
                        />
                    </Show>
                </form>
            </Show>

            <Show when=move || tab.get() == OrderDetailTab::Lines>
                <div class="detail-section">
                    <div class="detail-section-title">
                        <h3>"Demand lines"</h3>
                        <span>{format!("{} lines", order.order_items.len())}</span>
                    </div>
                    <div class="table-scroll">
                        <table class="data-table detail-table order-demand-lines-table">
                            <thead>
                                <tr>
                                    <th>"Line"</th><th>"Item"</th><th>"Description"</th>
                                    <th>"UOM"</th><th class="numeric">"Quantity"</th>
                                </tr>
                            </thead>
                            <tbody>
                                {order
                                    .order_items
                                    .clone()
                                    .into_iter()
                                    .map(|line| {
                                        view! {
                                            <tr>
                                                <td>
                                                    <strong>{line.line_key}</strong>
                                                    <small class="cell-detail">
                                                        {format!("Line {}", line.line_number)}
                                                    </small>
                                                </td>
                                                <td>{format!("#{}", line.item_id)}</td>
                                                <td>{line.item_description.unwrap_or_else(|| "-".to_owned())}</td>
                                                <td>{line.uom}</td>
                                                <td class="numeric strong">{format_quantity(line.qty)}</td>
                                            </tr>
                                        }
                                    })
                                    .collect_view()}
                            </tbody>
                        </table>
                        {order.order_items.is_empty().then(|| {
                            view! { <p class="empty-state">"No demand lines are attached to this order."</p> }
                        })}
                    </div>
                </div>
            </Show>

            <Show when=move || tab.get() == OrderDetailTab::Fulfillment>
                <div class="detail-section-stack">
                    <OrderAllocationPanel
                        order_id
                        facilities=facilities.get_value()
                        locations=locations.get_value()
                        on_refreshed
                        on_unauthorized
                    />
                    <PickReversalPanel
                        order_id
                        order_revision=order.revision
                        order_status=order.status
                        on_refreshed
                        on_unauthorized
                    />
                    <section class="detail-section">
                        <div class="detail-section-title">
                            <h3>"Reservations"</h3>
                            <span>{format!("{} records", order.reservations.len())}</span>
                        </div>
                        <div class="table-scroll">
                            <table class="data-table detail-table">
                                <thead>
                                    <tr>
                                        <th>"Item"</th><th>"Facility"</th><th>"UOM"</th><th>"State"</th>
                                        <th class="numeric">"Reserved"</th><th class="numeric">"Allocated"</th>
                                        <th>"Created"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {order
                                        .reservations
                                        .clone()
                                        .into_iter()
                                        .map(|reservation| {
                                            let item_label = reservation_item_names.with_value(|names| {
                                                names
                                                    .iter()
                                                    .find(|(id, _)| *id == reservation.item_id)
                                                    .map(|(_, name)| name.clone())
                                                    .unwrap_or_else(|| format!("Item #{}", reservation.item_id))
                                            });
                                            let facility_label = facility_names.with_value(|names| {
                                                names
                                                    .iter()
                                                    .find(|(id, _)| *id == reservation.facility_id)
                                                    .map(|(_, name)| name.clone())
                                                    .unwrap_or_else(|| {
                                                        format!("Facility #{}", reservation.facility_id)
                                                    })
                                            });
                                            view! {
                                                <tr>
                                                    <td>
                                                        <strong>{item_label}</strong>
                                                        <small class="cell-detail">{format!("#{}", reservation.item_id)}</small>
                                                    </td>
                                                    <td>{facility_label}</td>
                                                    <td>{reservation.uom}</td>
                                                    <td>{title_case(reservation.status.as_str())}</td>
                                                    <td class="numeric">{format_quantity(reservation.qty)}</td>
                                                    <td class="numeric">{format_quantity(reservation.allocated_qty)}</td>
                                                    <td>{short_timestamp(reservation.created)}</td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()}
                                </tbody>
                            </table>
                            {order.reservations.is_empty().then(|| {
                                view! { <p class="empty-state">"No stock is currently reserved."</p> }
                            })}
                        </div>
                    </section>
                    <section class="detail-section">
                        <div class="detail-section-title">
                            <h3>"Tracking"</h3>
                            <span>{format!("{} numbers", order.tracking_numbers.len())}</span>
                        </div>
                        <div class="tracking-list">
                            {order
                                .tracking_numbers
                                .clone()
                                .into_iter()
                                .map(|tracking| {
                                    view! {
                                        <div class="tracking-row">
                                            <strong>{tracking.tracking_number}</strong>
                                            <span>{tracking.carrier.unwrap_or_else(|| "Carrier not set".to_owned())}</span>
                                            <span>{tracking.service.unwrap_or_else(|| "Service not set".to_owned())}</span>
                                        </div>
                                    }
                                })
                                .collect_view()}
                            {order.tracking_numbers.is_empty().then(|| {
                                view! { <p class="empty-state">"No tracking numbers have been recorded."</p> }
                            })}
                        </div>
                    </section>
                </div>
            </Show>

            <Show when=move || tab.get() == OrderDetailTab::Holds>
                <div class="detail-section-stack">
                    <section class="detail-section">
                        <div class="detail-section-title">
                            <h3>"Order holds"</h3>
                            <span>{format!(
                                "{} active / {} total",
                                order.holds.iter().filter(|hold| hold.is_active()).count(),
                                order.holds.len()
                            )}</span>
                        </div>
                        <div class="table-scroll">
                            <table class="data-table detail-table order-holds-table">
                                <thead>
                                    <tr>
                                        <th>"Hold"</th><th>"Placed"</th><th>"State"</th>
                                    </tr>
                                </thead>
                                <tbody>
                                    {order
                                        .holds
                                        .clone()
                                        .into_iter()
                                        .map(|hold| {
                                            let active = hold.is_active();
                                            let hold_detail = hold
                                                .note
                                                .as_deref()
                                                .map_or_else(
                                                    || format!("#{}", hold.id),
                                                    |note| format!("#{} - {note}", hold.id),
                                                );
                                            let hold_detail_title = hold_detail.clone();
                                            let released_detail = hold.released_at.map(|released_at| {
                                                format!(
                                                    "{} - User #{}",
                                                    short_timestamp(released_at),
                                                    hold.released_by_user_id.unwrap_or_default()
                                                )
                                            });
                                            view! {
                                                <tr>
                                                    <td>
                                                        <strong>{order_hold_reason_label(hold.reason.as_str())}</strong>
                                                        <small class="cell-detail" title=hold_detail_title>{hold_detail}</small>
                                                    </td>
                                                    <td>
                                                        <strong>{short_timestamp(hold.created)}</strong>
                                                        <small class="cell-detail">{format!("User #{}", hold.created_by_user_id)}</small>
                                                    </td>
                                                    <td>
                                                        <div class="order-hold-state-line">
                                                            <span class=if active { "status held" } else { "status muted" }>
                                                                {if active { "Active" } else { "Released" }}
                                                            </span>
                                                            {active.then(|| {
                                                                let hold_id = hold.id;
                                                                view! {
                                                                    <button
                                                                        type="button"
                                                                        class="button table-action order-hold-release-action"
                                                                        title="Release order hold"
                                                                        aria-label="Release order hold"
                                                                        on:click=move |_| {
                                                                            release_note.set(String::new());
                                                                            release_candidate.set(Some(hold_id));
                                                                        }
                                                                    >
                                                                        <Icon icon=UiIcon::Unlock/>
                                                                    </button>
                                                                }
                                                            })}
                                                        </div>
                                                        {released_detail.map(|detail| {
                                                            let title = detail.clone();
                                                            view! { <small class="cell-detail" title=title>{detail}</small> }
                                                        })}
                                                        {hold.release_note.map(|note| {
                                                            let title = note.clone();
                                                            view! { <small class="cell-detail" title=title>{note}</small> }
                                                        })}
                                                    </td>
                                                </tr>
                                            }
                                        })
                                        .collect_view()}
                                </tbody>
                            </table>
                            {order.holds.is_empty().then(|| {
                                view! { <p class="empty-state">"No order holds have been recorded."</p> }
                            })}
                        </div>
                    </section>
                    <Show when=move || release_candidate.get().is_some()>
                        <section class="confirmation-panel release-hold-panel" role="alertdialog" aria-labelledby="release-order-hold-title">
                            <h3 id="release-order-hold-title">{move || format!(
                                "Release hold #{}?",
                                release_candidate.get().unwrap_or_default()
                            )}</h3>
                            <p>"The order stays blocked when another active hold remains."</p>
                            <label>
                                <span>"Release note"</span>
                                <textarea
                                    maxlength="1000"
                                    rows="3"
                                    prop:value=move || release_note.get()
                                    on:input=move |event| release_note.set(event_target_value(&event))
                                ></textarea>
                            </label>
                            <div class="form-actions">
                                <button
                                    type="button"
                                    class="button primary-action"
                                    disabled=move || command_pending.get()
                                    on:click=release_hold
                                >
                                    <Icon icon=UiIcon::Unlock/>
                                    {move || if command_pending.get() { "Releasing" } else { "Release hold" }}
                                </button>
                                <button
                                    type="button"
                                    class="button secondary-action"
                                    on:click=move |_| release_candidate.set(None)
                                >
                                    "Keep hold"
                                </button>
                            </div>
                        </section>
                    </Show>
                </div>
            </Show>

            <Show when=move || tab.get() == OrderDetailTab::Activity>
                <div class="detail-section">
                    <div class="detail-section-title">
                        <h3>"Order activity"</h3>
                        <span>{format!("{} events", order.activity.len())}</span>
                    </div>
                    <ol class="activity-list">
                        {order
                            .activity
                            .clone()
                            .into_iter()
                            .rev()
                            .map(|activity| {
                                view! {
                                    <li>
                                        <span>{short_timestamp(activity.created)}</span>
                                        <strong>{title_case(&activity.action)}</strong>
                                    </li>
                                }
                            })
                            .collect_view()}
                    </ol>
                    {order.activity.is_empty().then(|| {
                        view! { <p class="empty-state">"No activity has been recorded."</p> }
                    })}
                </div>
            </Show>
        </div>
    }
}

fn can_cancel_order(status: OrderStatus) -> bool {
    matches!(
        status,
        OrderStatus::Open | OrderStatus::Held | OrderStatus::Processing
    )
}

fn can_place_order_hold(status: OrderStatus) -> bool {
    matches!(status, OrderStatus::Open | OrderStatus::Held)
}

fn parse_order_hold_reason(value: &str) -> Option<OrderHoldRequestReason> {
    match value {
        "address_review" => Some(OrderHoldRequestReason::AddressReview),
        "compliance_review" => Some(OrderHoldRequestReason::ComplianceReview),
        "customer_request" => Some(OrderHoldRequestReason::CustomerRequest),
        "inventory_shortage" => Some(OrderHoldRequestReason::InventoryShortage),
        "payment_review" => Some(OrderHoldRequestReason::PaymentReview),
        "other" => Some(OrderHoldRequestReason::Other),
        _ => None,
    }
}

fn order_hold_reason_label(value: &str) -> &'static str {
    match value {
        "address_review" => "Address review",
        "compliance_review" => "Compliance review",
        "customer_request" => "Client request",
        "inventory_shortage" => "Inventory shortage",
        "payment_review" => "Payment review",
        "other" => "Other",
        _ => "Unknown",
    }
}

pub(super) fn title_case(value: &str) -> String {
    value
        .split(['_', ' '])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + chars.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_not_offered_for_terminal_orders() {
        assert!(can_cancel_order(OrderStatus::Open));
        assert!(can_cancel_order(OrderStatus::Held));
        assert!(can_cancel_order(OrderStatus::Processing));
        assert!(!can_cancel_order(OrderStatus::AwaitingShipment));
        assert!(!can_cancel_order(OrderStatus::AwaitingPacking));
        assert!(!can_cancel_order(OrderStatus::Packing));
        assert!(!can_cancel_order(OrderStatus::Shipped));
        assert!(!can_cancel_order(OrderStatus::Cancelled));
        assert!(!can_cancel_order(OrderStatus::Void));
    }

    #[test]
    fn labels_replace_wire_separators() {
        assert_eq!(title_case("awaiting shipment"), "Awaiting Shipment");
        assert_eq!(title_case("quality_hold"), "Quality Hold");
    }
}
