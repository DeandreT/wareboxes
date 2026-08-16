use wareboxes_api_contract::v1::{
    CancelShipmentDocumentPrintRequest, OpaqueCursor, PrintShipmentDocumentRequest,
    PrintShipmentDocumentResponse, ShipmentDocumentPrintJobPage, ShipmentDocumentPrintJobResponse,
    ShipmentPrinterDevicePage,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn shipment_document_printers(
    document_id: i64,
) -> Result<ShipmentPrinterDevicePage, ApiError> {
    super::browser::get(&format!(
        "/api/v1/shipment-documents/{document_id}/printers"
    ))
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn shipment_document_printers(
    _document_id: i64,
) -> Result<ShipmentPrinterDevicePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn print_shipment_document(
    document_id: i64,
    request: &PrintShipmentDocumentRequest,
    idempotency_key: &str,
) -> Result<PrintShipmentDocumentResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/shipment-documents/{document_id}/print-jobs"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn print_shipment_document(
    _document_id: i64,
    _request: &PrintShipmentDocumentRequest,
    _idempotency_key: &str,
) -> Result<PrintShipmentDocumentResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn shipment_document_print_jobs(
    document_id: i64,
    cursor: Option<&OpaqueCursor>,
    limit: u16,
) -> Result<ShipmentDocumentPrintJobPage, ApiError> {
    super::browser::get(&print_job_page_path(document_id, cursor, limit)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn shipment_document_print_jobs(
    _document_id: i64,
    _cursor: Option<&OpaqueCursor>,
    _limit: u16,
) -> Result<ShipmentDocumentPrintJobPage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn shipment_document_print_job(
    document_id: i64,
    command_id: i64,
) -> Result<ShipmentDocumentPrintJobResponse, ApiError> {
    super::browser::get(&format!(
        "/api/v1/shipment-documents/{document_id}/print-jobs/{command_id}"
    ))
    .await
}

#[cfg(target_arch = "wasm32")]
pub async fn cancel_shipment_document_print(
    document_id: i64,
    command_id: i64,
    request: &CancelShipmentDocumentPrintRequest,
    idempotency_key: &str,
) -> Result<PrintShipmentDocumentResponse, ApiError> {
    super::browser::post(
        &format!("/api/v1/shipment-documents/{document_id}/print-jobs/{command_id}/cancellations"),
        request,
        idempotency_key,
    )
    .await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn cancel_shipment_document_print(
    _document_id: i64,
    _command_id: i64,
    _request: &CancelShipmentDocumentPrintRequest,
    _idempotency_key: &str,
) -> Result<PrintShipmentDocumentResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn shipment_document_print_job(
    _document_id: i64,
    _command_id: i64,
) -> Result<ShipmentDocumentPrintJobResponse, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(any(target_arch = "wasm32", test))]
fn print_job_page_path(document_id: i64, cursor: Option<&OpaqueCursor>, limit: u16) -> String {
    let mut path = format!("/api/v1/shipment-documents/{document_id}/print-jobs?limit={limit}");
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(cursor.as_str());
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn print_history_path_binds_document_cursor_and_limit() {
        let cursor = OpaqueCursor::new("sdp1.0000000000000001.0000000000000002").unwrap();
        assert_eq!(
            print_job_page_path(7, Some(&cursor), 25),
            "/api/v1/shipment-documents/7/print-jobs?limit=25&cursor=sdp1.0000000000000001.0000000000000002"
        );
    }
}
