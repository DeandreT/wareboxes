use axum::extract::{Path, State};
use axum::Json;
use wareboxes_api_contract::v1::{
    ExpectedReceiptLine, ExpectedReceivingLoadStatus, ExpectedReceivingLocation,
    ExpectedReceivingSessionResponse,
};

use super::error::{V1Error, V1Result};
use crate::auth::CurrentTenant;
use crate::error::AppError;
use crate::repo;
use crate::state::AppState;

const PERMISSION: &str = "wms";

pub async fn get_session(
    State(state): State<AppState>,
    user: CurrentTenant,
    Path(load_id): Path<i64>,
) -> V1Result<Json<ExpectedReceivingSessionResponse>> {
    user.require_permission(&state.db, PERMISSION).await?;
    require_positive(load_id, "load ID")?;
    let session =
        repo::expected_receiving::get_expected_receiving_session(&state.db, &user.tenant, load_id)
            .await?;

    Ok(Json(map_session(session)))
}

fn map_session(
    session: repo::expected_receiving::ExpectedReceivingSession,
) -> ExpectedReceivingSessionResponse {
    ExpectedReceivingSessionResponse {
        load_id: session.load_id,
        inventory_owner_id: session.inventory_owner_id,
        facility_id: session.facility_id,
        reference_number: session.reference_number,
        status: match session.status {
            repo::expected_receiving::ExpectedReceivingLoadStatus::Arrived => {
                ExpectedReceivingLoadStatus::Arrived
            }
            repo::expected_receiving::ExpectedReceivingLoadStatus::Receiving => {
                ExpectedReceivingLoadStatus::Receiving
            }
        },
        receiving_location: ExpectedReceivingLocation {
            location_id: session.receiving_location.location_id,
            barcode: session.receiving_location.barcode,
            name: session.receiving_location.name,
        },
        lines: session.lines.into_iter().map(map_line).collect(),
    }
}

fn map_line(line: repo::expected_receiving::ExpectedReceiptLine) -> ExpectedReceiptLine {
    ExpectedReceiptLine {
        load_line_id: line.load_line_id,
        item_id: line.item_id,
        item_description: line.item_description,
        uom: line.uom,
        item_barcodes: line.item_barcodes,
        expected_quantity: line.expected_quantity,
        received_quantity: line.received_quantity,
        rejected_quantity: line.rejected_quantity,
        missing_quantity: line.missing_quantity,
        remaining_quantity: line.remaining_quantity,
        lot: line.lot,
        serial: line.serial,
        expiration: line.expiration.map(|timestamp| timestamp.to_rfc3339()),
    }
}

fn require_positive(value: i64, label: &str) -> V1Result<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(invalid(format!("{label} must be positive")))
    }
}

fn invalid(message: impl Into<String>) -> V1Error {
    AppError::bad_request(message).into()
}
