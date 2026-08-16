use wareboxes_domain::{CarrierServiceCode, ShipmentId, ShortShipDemandQuantities};

use crate::error::{AppError, AppResult};

use super::{AddressSnapshot, DocumentCarton, DocumentLine, DocumentManifest};

pub(super) fn render_packing_slip(
    shipment_id: ShipmentId,
    order_key: &str,
    addresses: &[AddressSnapshot],
    lines: &[DocumentLine],
    cartons: &[DocumentCarton],
    demand: ShortShipDemandQuantities,
    include_tracking_barcodes: bool,
) -> AppResult<String> {
    let origin = addresses.iter().find(|address| address.role == "origin");
    let destination = addresses
        .iter()
        .find(|address| address.role == "destination");
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Packing slip</title><style>body{font:14px system-ui,sans-serif;color:#111;margin:32px}h1{font-size:24px;margin:0 0 8px}h2{font-size:15px;margin:24px 0 8px}.meta,.addresses{display:grid;grid-template-columns:1fr 1fr;gap:24px}table{width:100%;border-collapse:collapse}th,td{text-align:left;border-bottom:1px solid #bbb;padding:7px 5px}.num{text-align:right}.summary{margin-left:auto;width:320px}.muted{color:#555}.tracking-barcode svg{display:block;width:180px;height:40px}@media print{body{margin:12mm}}</style></head><body>",
    );
    html.push_str("<h1>Packing slip</h1><div class=\"meta\"><div><strong>Order</strong><br>");
    escape_html_into(order_key, &mut html);
    html.push_str("</div><div><strong>Shipment</strong><br>");
    html.push_str(&shipment_id.get().to_string());
    html.push_str("</div></div><div class=\"addresses\">");
    render_address("Ship from", origin, &mut html);
    render_address("Ship to", destination, &mut html);
    html.push_str("</div><h2>Contents</h2><table><thead><tr><th>Line</th><th>Item</th><th>Description</th><th>UOM</th><th class=\"num\">Ordered</th><th class=\"num\">Packed</th><th class=\"num\">Short</th><th class=\"num\">Substituted</th></tr></thead><tbody>");
    for line in lines {
        html.push_str("<tr><td>");
        escape_html_into(&line.line_key, &mut html);
        html.push_str("</td><td>");
        html.push_str(&line.item_id.get().to_string());
        html.push_str("</td><td>");
        escape_html_into(&line.item_description, &mut html);
        html.push_str("</td><td>");
        escape_html_into(&line.uom, &mut html);
        html.push_str("</td><td class=\"num\">");
        html.push_str(&line.ordered_quantity.to_string());
        html.push_str("</td><td class=\"num\">");
        html.push_str(&line.packed_quantity.to_string());
        html.push_str("</td><td class=\"num\">");
        html.push_str(&line.accepted_short_quantity.to_string());
        html.push_str("</td><td class=\"num\">");
        html.push_str(&line.accepted_substitute_quantity.to_string());
        html.push_str("</td></tr>");
    }
    html.push_str("</tbody></table><h2>Cartons</h2><table><thead><tr><th>#</th><th>Carton</th><th class=\"num\">Quantity</th><th class=\"num\">Weight (g)</th>");
    if include_tracking_barcodes {
        html.push_str("<th>Tracking barcode</th>");
    }
    html.push_str("</tr></thead><tbody>");
    for carton in cartons {
        html.push_str("<tr><td>");
        html.push_str(&carton.sequence.to_string());
        html.push_str("</td><td>");
        escape_html_into(&carton.barcode, &mut html);
        html.push_str("</td><td class=\"num\">");
        html.push_str(&carton.packed_quantity.to_string());
        html.push_str("</td><td class=\"num\">");
        html.push_str(
            &carton
                .weight_grams
                .map_or_else(|| "-".to_owned(), |weight| weight.to_string()),
        );
        if include_tracking_barcodes {
            let tracking = carton.tracking_number.as_ref().ok_or_else(|| {
                AppError::internal("shipment carton tracking snapshot is missing")
            })?;
            let tracking_svg =
                wareboxes_barcodes::svg("code128", tracking.as_str()).map_err(|_| {
                    AppError::conflict("tracking number cannot be encoded as a Code 128 label")
                })?;
            html.push_str("</td><td><div class=\"tracking-barcode\">");
            html.push_str(&tracking_svg);
            html.push_str("</div><small>");
            escape_html_into(tracking.as_str(), &mut html);
            html.push_str("</small>");
        }
        html.push_str("</td></tr>");
    }
    html.push_str(
        "</tbody></table><table class=\"summary\"><tbody><tr><th>Ordered</th><td class=\"num\">",
    );
    html.push_str(&demand.ordered().get().to_string());
    html.push_str("</td></tr><tr><th>Packed</th><td class=\"num\">");
    html.push_str(&demand.effective().get().to_string());
    html.push_str("</td></tr><tr><th>Accepted short</th><td class=\"num\">");
    html.push_str(&demand.accepted_short().get().to_string());
    html.push_str("</td></tr><tr><th>Accepted substitution</th><td class=\"num\">");
    html.push_str(&demand.accepted_substitute().get().to_string());
    html.push_str("</td></tr></tbody></table></body></html>");
    Ok(html)
}

pub(super) fn render_carton_label_set(
    shipment_id: ShipmentId,
    order_key: &str,
    addresses: &[AddressSnapshot],
    cartons: &[DocumentCarton],
    manifest: &DocumentManifest,
) -> AppResult<String> {
    let origin = addresses
        .iter()
        .find(|address| address.role == "origin")
        .ok_or_else(|| AppError::internal("shipment origin snapshot is missing"))?;
    let destination = addresses
        .iter()
        .find(|address| address.role == "destination")
        .ok_or_else(|| AppError::internal("shipment destination snapshot is missing"))?;
    let mut html = String::from(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"utf-8\"><title>Carton labels</title><style>@page{size:4in 6in;margin:0}*{box-sizing:border-box}body{margin:0;color:#000;background:#fff;font:12px Arial,sans-serif}.label{width:4in;height:6in;padding:.18in;display:grid;grid-template-rows:auto auto auto 1fr auto;gap:.08in;break-after:page;border:1px solid #000}.label:last-child{break-after:auto}.top{display:flex;justify-content:space-between;align-items:start;border-bottom:2px solid #000;padding-bottom:.06in}.carrier{font-size:22px;font-weight:800}.service,.carton{font-size:13px;font-weight:700}.addresses{display:grid;grid-template-columns:1fr 1fr;gap:.1in}.address{border:1px solid #000;padding:.06in;line-height:1.25}.address strong{display:block;font-size:9px;text-transform:uppercase;margin-bottom:2px}.barcode{display:grid;place-items:center;min-height:1.05in;overflow:hidden}.barcode svg{display:block;width:100%;height:.95in}.tracking{font-size:16px;font-weight:800;text-align:center}.meta{display:grid;grid-template-columns:1fr 1fr;gap:3px 12px;border-top:1px solid #000;padding-top:.05in}.meta span{font-size:9px;text-transform:uppercase}.meta strong{display:block;font-size:11px}@media screen{body{background:#ddd}.label{margin:12px auto;background:#fff}}@media print{.label{border:0}}</style></head><body>",
    );
    for carton in cartons {
        let tracking = carton
            .tracking_number
            .as_ref()
            .ok_or_else(|| AppError::internal("shipment carton tracking snapshot is missing"))?;
        let tracking_svg = wareboxes_barcodes::svg("code128", tracking.as_str()).map_err(|_| {
            AppError::conflict("tracking number cannot be encoded as a Code 128 label")
        })?;
        let carton_svg = wareboxes_barcodes::svg("code128", &carton.barcode).map_err(|_| {
            AppError::conflict("carton barcode cannot be encoded as a Code 128 label")
        })?;
        html.push_str(
            "<section class=\"label\"><header class=\"top\"><div><div class=\"carrier\">",
        );
        escape_html_into(manifest.carrier_code.as_str(), &mut html);
        html.push_str("</div><div class=\"service\">");
        escape_html_into(
            manifest
                .service_code
                .as_ref()
                .map_or("STANDARD", CarrierServiceCode::as_str),
            &mut html,
        );
        html.push_str("</div></div><div class=\"carton\">Carton ");
        html.push_str(&carton.sequence.to_string());
        html.push_str(" of ");
        html.push_str(&cartons.len().to_string());
        html.push_str("</div></header><div class=\"addresses\">");
        render_label_address("Ship from", origin, &mut html);
        render_label_address("Ship to", destination, &mut html);
        html.push_str("</div><div><div class=\"barcode\">");
        html.push_str(&tracking_svg);
        html.push_str("</div><div class=\"tracking\">");
        escape_html_into(tracking.as_str(), &mut html);
        html.push_str("</div></div><div class=\"barcode\">");
        html.push_str(&carton_svg);
        html.push_str("</div><footer class=\"meta\"><div><span>Order</span><strong>");
        escape_html_into(order_key, &mut html);
        html.push_str("</strong></div><div><span>Shipment</span><strong>");
        html.push_str(&shipment_id.get().to_string());
        html.push_str("</strong></div><div><span>Manifest</span><strong>");
        escape_html_into(manifest.manifest_reference.as_str(), &mut html);
        html.push_str("</strong></div><div><span>Weight / dimensions</span><strong>");
        html.push_str(&label_measurements(carton));
        html.push_str("</strong></div></footer></section>");
    }
    html.push_str("</body></html>");
    Ok(html)
}

fn render_label_address(label: &str, address: &AddressSnapshot, html: &mut String) {
    html.push_str("<div class=\"address\"><strong>");
    html.push_str(label);
    html.push_str("</strong>");
    if let Some(name) = address.name.as_deref() {
        escape_html_into(name, html);
        html.push_str("<br>");
    }
    if let Some(company) = address.company.as_deref() {
        escape_html_into(company, html);
        html.push_str("<br>");
    }
    escape_html_into(&address.line1, html);
    if let Some(line2) = address.line2.as_deref() {
        html.push_str("<br>");
        escape_html_into(line2, html);
    }
    html.push_str("<br>");
    escape_html_into(&address.city, html);
    if let Some(state) = address.state.as_deref() {
        html.push_str(", ");
        escape_html_into(state, html);
    }
    html.push(' ');
    escape_html_into(&address.postal_code, html);
    html.push_str("<br>");
    escape_html_into(&address.country, html);
    html.push_str("</div>");
}

fn label_measurements(carton: &DocumentCarton) -> String {
    let weight = carton
        .weight_grams
        .map_or_else(|| "-".to_owned(), |value| format!("{value} g"));
    match (carton.length_mm, carton.width_mm, carton.height_mm) {
        (Some(length), Some(width), Some(height)) => {
            format!("{weight} / {length}x{width}x{height} mm")
        }
        _ => weight,
    }
}

fn render_address(label: &str, address: Option<&AddressSnapshot>, html: &mut String) {
    html.push_str("<section><h2>");
    html.push_str(label);
    html.push_str("</h2>");
    if let Some(address) = address {
        for value in [
            address.name.as_deref(),
            address.company.as_deref(),
            Some(address.line1.as_str()),
            address.line2.as_deref(),
        ]
        .into_iter()
        .flatten()
        {
            escape_html_into(value, html);
            html.push_str("<br>");
        }
        escape_html_into(&address.city, html);
        if let Some(state) = address.state.as_deref() {
            html.push_str(", ");
            escape_html_into(state, html);
        }
        html.push(' ');
        escape_html_into(&address.postal_code, html);
        html.push_str("<br>");
        escape_html_into(&address.country, html);
        if let Some(phone) = address.phone.as_deref() {
            html.push_str("<br>");
            escape_html_into(phone, html);
        }
        if let Some(email) = address.email.as_deref() {
            html.push_str("<br>");
            escape_html_into(email, html);
        }
    }
    html.push_str("</section>");
}

fn escape_html_into(value: &str, output: &mut String) {
    for character in value.chars() {
        match character {
            '&' => output.push_str("&amp;"),
            '<' => output.push_str("&lt;"),
            '>' => output.push_str("&gt;"),
            '"' => output.push_str("&quot;"),
            '\'' => output.push_str("&#39;"),
            _ => output.push(character),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn html_escaping_prevents_snapshot_content_from_becoming_markup() {
        let mut output = String::new();
        escape_html_into("<script>\"x\" & 'y'</script>", &mut output);
        assert_eq!(
            output,
            "&lt;script&gt;&quot;x&quot; &amp; &#39;y&#39;&lt;/script&gt;"
        );
    }
}
