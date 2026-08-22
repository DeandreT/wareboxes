use leptos::{html, prelude::*};
use wareboxes_api_contract::v1::{CancelShipmentRequest, ShipmentResponse, ShipmentStatus};

use crate::components::{Icon, UiIcon};
use crate::workspace_layout::{PaneControls, SplitPaneHandle, SplitPaneState};

use super::cancellation::ShipmentCancellationAction;
use super::carrier::CarrierManifestPanel;
use super::display::{dimensions_label, shipment_status_label};
use super::documents::ShipmentDocumentsPanel;
use super::{DeparturePanel, ManifestPanel, ShippingSignals};

#[component]
pub(super) fn ShipmentExecution(
    shipment: ShipmentResponse,
    signals: ShippingSignals,
    scan_input: NodeRef<html::Input>,
    layout: SplitPaneState,
    on_manifest: Callback<()>,
    on_cancel: Callback<(i64, CancelShipmentRequest)>,
    on_scan: Callback<()>,
    on_depart: Callback<()>,
    can_cancel: bool,
    can_manage_carriers: bool,
    can_retry_carriers: bool,
    on_carrier_manifested: Callback<(i64, i64)>,
) -> impl IntoView {
    let carton_count = shipment.cartons.len();
    let packed_quantity = shipment
        .cartons
        .iter()
        .map(|carton| carton.packed_quantity)
        .sum::<i64>();
    let shipment_id = shipment.shipment_id;
    let shipment_revision = shipment.revision;
    let order_revision = shipment.order_revision;
    let inventory_owner_id = shipment.inventory_owner_id;
    let facility_id = shipment.facility_id;
    let order_id = shipment.order_id;
    view! {
        <div class="shipping-execution split-workspace" style=move || layout.style() data-pane-mode=move || layout.mode_attribute()>
            <section class="shipping-cartons split-master">
                <header>
                    <div><h3>"Cartons"</h3><span>{format!("{carton_count} cartons · {packed_quantity} units")}</span></div>
                    <div class="shipping-carton-header-actions">
                        <span class="status success">{shipment_status_label(shipment.status)}</span>
                        <PaneControls layout master_label="carton workspace" detail_label="shipping controls"/>
                    </div>
                </header>
                <div class="table-scroll shipping-carton-scroll">
                    <table class="data-table shipping-carton-table">
                        <caption class="sr-only">"Cartons assigned to the selected shipment"</caption>
                        <thead><tr><th>"#"</th><th>"Carton"</th><th>"Lines/qty"</th><th>"Weight"</th><th>"Dimensions"</th><th>"Tracking"</th><th>"Departure"</th></tr></thead>
                        <tbody>
                            {shipment.cartons.clone().into_iter().map(|carton| {
                                let carton_barcode = carton.carton_barcode;
                                let carton_title = carton_barcode.clone();
                                let packed = format!("{} / {}", carton.content_count, carton.packed_quantity);
                                let dimensions = dimensions_label(carton.length_mm, carton.width_mm, carton.height_mm);
                                let dimensions_title = dimensions.clone();
                                let tracking = carton.tracking_number.unwrap_or_else(|| "Unassigned".into());
                                let tracking_title = tracking.clone();
                                let departure = carton.departed_at.as_ref().map_or("Remaining", |_| "Departed");
                                view! {
                                <tr>
                                    <td>{carton.sequence}</td>
                                    <td class="mono" title=carton_title>{carton_barcode}</td>
                                    <td>{packed}</td>
                                    <td>{carton.weight_grams.map_or_else(|| "—".into(), |value| format!("{value} g"))}</td>
                                    <td title=dimensions_title>{dimensions}</td>
                                    <td class="mono" title=tracking_title>{tracking}</td>
                                    <td><span class=if carton.departed_at.is_some() { "status success" } else { "status" }>{departure}</span></td>
                                </tr>
                            }}).collect_view()}
                        </tbody>
                    </table>
                </div>
                <ShipmentDocumentsPanel
                    shipment_id
                    shipment_revision
                    shipment_status=shipment.status
                    on_unauthorized=signals.on_unauthorized
                />
            </section>
            <SplitPaneHandle layout/>
            <aside class="shipping-command-panel split-detail">
                {match shipment.status {
                    ShipmentStatus::AwaitingManifest => view! {
                        <CarrierManifestPanel
                            shipment_id
                            order_id
                            inventory_owner_id
                            facility_id
                            shipment_revision
                            can_manage=can_manage_carriers
                            can_retry=can_retry_carriers
                            on_manifested=on_carrier_manifested
                            on_unauthorized=signals.on_unauthorized
                        />
                        <details class="shipping-manual-fallback">
                            <summary>"Manual carrier fallback"</summary>
                            <p>"Use only when the carrier gateway is unavailable and tracking was obtained outside Wareboxes."</p>
                            <ManifestPanel signals on_manifest/>
                        </details>
                        {can_cancel.then(|| view! {
                            <ShipmentCancellationAction
                                shipment_id
                                shipment_revision
                                order_revision
                                blocked=Signal::derive(move || signals.pending.get() || signals.retry.get().is_some())
                                on_cancel
                            />
                        })}
                    }.into_any(),
                    ShipmentStatus::Manifested => view! {
                        <DeparturePanel shipment signals scan_input on_scan on_depart/>
                        {can_cancel.then(|| view! {
                            <ShipmentCancellationAction
                                shipment_id
                                shipment_revision
                                order_revision
                                blocked=Signal::derive(move || signals.pending.get() || signals.retry.get().is_some())
                                on_cancel
                            />
                        })}
                    }.into_any(),
                    ShipmentStatus::PartiallyDeparted => view! {
                        <DeparturePanel shipment signals scan_input on_scan on_depart/>
                    }.into_any(),
                    ShipmentStatus::Departed => view! {
                        <div class="shipping-complete"><Icon icon=UiIcon::Shipping/><h3>"Shipment departed"</h3><p>"Inventory and the order are posted as shipped."</p></div>
                    }.into_any(),
                    ShipmentStatus::Cancelled => view! {
                        <div class="shipping-complete"><Icon icon=UiIcon::Reverse/><h3>"Shipment cancelled"</h3><p>"The immutable attempt remains available in shipment history."</p></div>
                    }.into_any(),
                }}
            </aside>
        </div>
    }
}
