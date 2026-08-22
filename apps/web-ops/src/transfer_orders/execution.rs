use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    DispatchTransferOrderLineRequest, DispatchTransferOrderRequest, ReceiveTransferOrderRequest,
    TransferExecutionReadinessResponse, TransferOrderDetailResponse, TransferOrderStatus,
};

use crate::api;
use crate::toast::use_toast_bus;
use crate::view_model::format_quantity;

#[component]
pub(super) fn TransferExecutionDialog(
    detail: TransferOrderDetailResponse,
    on_close: Callback<()>,
    on_changed: Callback<i64>,
    on_unauthorized: Callback<()>,
) -> impl IntoView {
    let readiness = RwSignal::new(None::<TransferExecutionReadinessResponse>);
    let loading = RwSignal::new(true);
    let pending = RwSignal::new(false);
    let error = RwSignal::new(None::<String>);
    let location_id = RwSignal::new(String::new());
    let location_scan = RwSignal::new(String::new());
    let dispatch_retry = RwSignal::new(None::<(DispatchTransferOrderRequest, String)>);
    let receipt_retry = RwSignal::new(None::<(ReceiveTransferOrderRequest, String)>);
    let id = detail.summary.transfer_order_id;
    let revision = detail.summary.revision;
    let status = detail.summary.status;
    let lines = detail.lines.clone();
    let transfer_number = detail.summary.number.clone();
    let preview_detail = RwSignal::new(detail.clone());
    let executable_lines = lines.clone();
    let toasts = use_toast_bus();

    Effect::new(move |_| {
        leptos::task::spawn_local(async move {
            match api::transfer_execution_readiness(id).await {
                Ok(value) => {
                    let first = match status {
                        TransferOrderStatus::Released => value.transit_locations.first(),
                        TransferOrderStatus::InTransit => value.receiving_locations.first(),
                        _ => None,
                    };
                    if let Some(first) = first {
                        location_id.set(first.location_id.to_string());
                        location_scan.set(first.barcode.clone());
                    }
                    readiness.set(Some(value));
                    loading.set(false);
                }
                Err(value) if value.unauthorized => on_unauthorized.run(()),
                Err(value) => {
                    error.set(Some(value.message));
                    loading.set(false);
                }
            }
        });
    });

    let submit = move |event: leptos::ev::SubmitEvent| {
        event.prevent_default();
        if pending.get_untracked() {
            return;
        }
        let Some(current) = readiness.get_untracked() else {
            error.set(Some("Execution readiness is still loading.".into()));
            return;
        };
        let Ok(selected_location_id) = location_id.get_untracked().parse::<i64>() else {
            error.set(Some("Choose an execution location.".into()));
            return;
        };
        let scan = location_scan.get_untracked().trim().to_owned();
        if scan.is_empty() {
            error.set(Some("Scan the selected location.".into()));
            return;
        }
        pending.set(true);
        error.set(None);
        match status {
            TransferOrderStatus::Released => {
                let request = match build_dispatch_request(
                    revision,
                    selected_location_id,
                    scan,
                    &lines,
                    &current,
                ) {
                    Ok(request) => request,
                    Err(message) => {
                        pending.set(false);
                        error.set(Some(message));
                        return;
                    }
                };
                let key = dispatch_retry
                    .get_untracked()
                    .filter(|(saved, _)| saved == &request)
                    .map_or_else(api::new_idempotency_key, |(_, key)| key);
                dispatch_retry.set(Some((request.clone(), key.clone())));
                leptos::task::spawn_local(async move {
                    match api::dispatch_transfer_order(id, &request, &key).await {
                        Ok(_) => {
                            pending.set(false);
                            dispatch_retry.set(None);
                            toasts.success("Transfer dispatched to the in-transit lane.");
                            on_close.run(());
                            on_changed.run(id);
                        }
                        Err(value) if value.unauthorized => on_unauthorized.run(()),
                        Err(value) => {
                            pending.set(false);
                            if !value.ambiguous_outcome {
                                dispatch_retry.set(None);
                            }
                            error.set(Some(value.message.clone()));
                            toasts.error(value.message);
                        }
                    }
                });
            }
            TransferOrderStatus::InTransit => {
                let request = ReceiveTransferOrderRequest {
                    expected_revision: revision,
                    destination_location_id: selected_location_id,
                    destination_location_barcode: scan,
                };
                let key = receipt_retry
                    .get_untracked()
                    .filter(|(saved, _)| saved == &request)
                    .map_or_else(api::new_idempotency_key, |(_, key)| key);
                receipt_retry.set(Some((request.clone(), key.clone())));
                leptos::task::spawn_local(async move {
                    match api::receive_transfer_order(id, &request, &key).await {
                        Ok(_) => {
                            pending.set(false);
                            receipt_retry.set(None);
                            toasts.success("Transfer received at the destination facility.");
                            on_close.run(());
                            on_changed.run(id);
                        }
                        Err(value) if value.unauthorized => on_unauthorized.run(()),
                        Err(value) => {
                            pending.set(false);
                            if !value.ambiguous_outcome {
                                receipt_retry.set(None);
                            }
                            error.set(Some(value.message.clone()));
                            toasts.error(value.message);
                        }
                    }
                });
            }
            _ => {
                pending.set(false);
                error.set(Some(
                    "This transfer is not executable in its current state.".into(),
                ));
            }
        }
    };
    let can_submit = move || {
        readiness.with(|current| {
            current.as_ref().is_some_and(|current| match status {
                TransferOrderStatus::Released => {
                    !current.transit_locations.is_empty()
                        && build_dispatch_lines(&executable_lines, current).is_ok()
                }
                TransferOrderStatus::InTransit => !current.receiving_locations.is_empty(),
                _ => false,
            })
        })
    };

    view! {
        <div class="purchase-order-dialog-backdrop">
            <form class="purchase-order-dialog transfer-execution-dialog" role="dialog" aria-modal="true" aria-labelledby="transfer-execution-title" on:submit=submit>
                <header><div><span class="eyebrow">{transfer_number}</span><h2 id="transfer-execution-title">{match status {TransferOrderStatus::Released=>"Dispatch transfer",TransferOrderStatus::InTransit=>"Receive transfer",_=>"Transfer execution"}}</h2></div><button class="text-button" type="button" on:click=move |_|on_close.run(())>"Close"</button></header>
                <Show when=move ||loading.get()><p class="panel-loading" role="status">"Loading current stock and locations..."</p></Show>
                <Show when=move ||readiness.get().is_some()>{move ||readiness.get().map(|current|{
                    let locations=if status==TransferOrderStatus::Released {current.transit_locations.clone()} else {current.receiving_locations.clone()};
                    view!{<>
                        <div class="fulfillment-form-grid two-column"><label><span>{if status==TransferOrderStatus::Released{"In-transit lane"}else{"Receiving location"}}</span><select required prop:value=move ||location_id.get() on:change=move |event|{let value=event_target_value(&event);if let Some(location)=locations.iter().find(|item|item.location_id.to_string()==value){location_scan.set(location.barcode.clone());}location_id.set(value);}>{locations.clone().into_iter().map(|item|view!{<option value=item.location_id>{format!("{} · {}",item.name,item.barcode)}</option>}).collect_view()}</select></label><label><span>"Scanned location"</span><input required prop:value=move ||location_scan.get() on:input=move |event|location_scan.set(event_target_value(&event))/></label></div>
                        {if status==TransferOrderStatus::Released {view!{<DispatchPreview detail=preview_detail.get() readiness=current/>}.into_any()} else {view!{<ReceiptPreview detail=preview_detail.get()/>}.into_any()}}
                    </>}
                })}</Show>
                <Show when=move ||error.get().is_some()>{move ||error.get().map(|message|view!{<p class="inline-command-error" role="alert">{message}</p>})}</Show>
                <footer><button class="button quiet-action compact" type="button" on:click=move |_|on_close.run(())>"Go back"</button><button class="button primary-action compact" type="submit" disabled=move ||pending.get()||loading.get()||!can_submit()>{move ||if pending.get(){"Saving..."}else if status==TransferOrderStatus::Released{"Confirm dispatch"}else{"Confirm receipt"}}</button></footer>
            </form>
        </div>
    }
}

#[component]
fn DispatchPreview(
    detail: TransferOrderDetailResponse,
    readiness: TransferExecutionReadinessResponse,
) -> impl IntoView {
    let (planned, unavailable) = match build_dispatch_lines(&detail.lines, &readiness) {
        Ok(lines) => (lines, None),
        Err(message) => (Vec::new(), Some(message)),
    };
    let planned_count = planned.len();
    view! {<section><div class="detail-section-heading"><h3>"Stock to move"</h3><span>{if planned_count==0{"Not ready".into()}else{format!("{planned_count} source balances")}}</span></div>{unavailable.map(|message|view!{<p class="transfer-execution-unavailable" role="status">{message}</p>})}<div class="table-scroll"><table class="dense-table"><thead><tr><th>"Item / source"</th><th>"Lot / serial"</th><th class="numeric">"Qty"</th></tr></thead><tbody>{planned.into_iter().map(|line|{let candidate=readiness.dispatch_candidates.iter().find(|value|value.source_inventory_balance_id==line.source_inventory_balance_id).cloned();candidate.map(|value|view!{<tr><td><strong>{value.item_description}</strong><small>{format!("{} · {}",value.source_location_name,value.source_location_barcode)}</small></td><td>{value.lot.or(value.serial).unwrap_or_else(||"Uncontrolled".into())}</td><td class="numeric"><strong>{format_quantity(line.quantity)}</strong></td></tr>})}).collect_view()}</tbody></table></div></section>}
}

#[component]
fn ReceiptPreview(detail: TransferOrderDetailResponse) -> impl IntoView {
    view! {<section><div class="detail-section-heading"><h3>"In-transit stock"</h3><span>{format!("{} lines",detail.lines.len())}</span></div><table class="dense-table"><thead><tr><th>"Item"</th><th class="numeric">"Dispatched"</th></tr></thead><tbody>{detail.lines.into_iter().map(|line|view!{<tr><td>{line.item_description}</td><td class="numeric"><strong>{format_quantity(line.dispatched_quantity)}</strong></td></tr>}).collect_view()}</tbody></table></section>}
}

fn build_dispatch_request(
    revision: wareboxes_api_contract::v1::Revision,
    transit_location_id: i64,
    transit_location_barcode: String,
    lines: &[wareboxes_api_contract::v1::TransferOrderLineResponse],
    readiness: &TransferExecutionReadinessResponse,
) -> Result<DispatchTransferOrderRequest, String> {
    Ok(DispatchTransferOrderRequest {
        expected_revision: revision,
        transit_location_id,
        transit_location_barcode,
        lines: build_dispatch_lines(lines, readiness)?,
    })
}

fn build_dispatch_lines(
    lines: &[wareboxes_api_contract::v1::TransferOrderLineResponse],
    readiness: &TransferExecutionReadinessResponse,
) -> Result<Vec<DispatchTransferOrderLineRequest>, String> {
    let mut result = Vec::new();
    for line in lines {
        let line_candidates = readiness
            .dispatch_candidates
            .iter()
            .filter(|candidate| candidate.transfer_order_line_id == line.line_id)
            .collect::<Vec<_>>();
        let batch_id = line_candidates
            .iter()
            .map(|candidate| candidate.item_batch_id)
            .find(|batch_id| {
                line_candidates
                    .iter()
                    .filter(|candidate| candidate.item_batch_id == *batch_id)
                    .map(|candidate| candidate.free_quantity)
                    .sum::<i64>()
                    >= line.requested_quantity
            })
            .ok_or_else(|| {
                format!(
                    "{} has no single lot/serial identity with {} {} available.",
                    line.item_description, line.requested_quantity, line.uom
                )
            })?;
        let mut remaining = line.requested_quantity;
        for candidate in line_candidates
            .into_iter()
            .filter(|candidate| candidate.item_batch_id == batch_id)
        {
            if remaining == 0 {
                break;
            }
            let quantity = remaining.min(candidate.free_quantity);
            result.push(DispatchTransferOrderLineRequest {
                transfer_order_line_id: line.line_id,
                source_inventory_balance_id: candidate.source_inventory_balance_id,
                quantity,
                source_location_barcode: candidate.source_location_barcode.clone(),
            });
            remaining -= quantity;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_builder_never_mixes_batch_identity() {
        let lines = vec![wareboxes_api_contract::v1::TransferOrderLineResponse {
            line_id: 1,
            sequence: 1,
            item_id: 2,
            item_description: "Beans".into(),
            uom: "case".into(),
            requested_quantity: 5,
            dispatched_quantity: 0,
            received_quantity: 0,
        }];
        let candidate = |balance, item_batch, qty| {
            wareboxes_api_contract::v1::TransferDispatchCandidateResponse {
                transfer_order_line_id: 1,
                source_inventory_balance_id: balance,
                source_location_id: balance,
                source_location_barcode: format!("L-{balance}"),
                source_location_name: "Pick".into(),
                item_batch_id: item_batch,
                item_id: 2,
                item_description: "Beans".into(),
                uom: "case".into(),
                lot: Some(format!("LOT-{item_batch}")),
                expiration: None,
                serial: None,
                free_quantity: qty,
            }
        };
        let readiness = TransferExecutionReadinessResponse {
            transfer_order_id: 3,
            revision: wareboxes_api_contract::v1::Revision::new(2).unwrap(),
            status: TransferOrderStatus::Released,
            dispatch_candidates: vec![candidate(10, 20, 3), candidate(11, 21, 5)],
            transit_locations: vec![],
            receiving_locations: vec![],
        };
        let result = build_dispatch_lines(&lines, &readiness).unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].source_inventory_balance_id, 11);
        assert_eq!(result[0].quantity, 5);
    }
}
