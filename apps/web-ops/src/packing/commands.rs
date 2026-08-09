use wareboxes_api_contract::v1::{
    AbandonPackSessionRequest, CloseCartonRequest, CreateCartonRequest, OpenPackSessionRequest,
    PackPickedAllocationRequest, PackSessionResponse, RemovePackedContentRequest,
    ReopenCartonRequest, VoidCartonRequest,
};

use crate::api;

#[derive(Clone, Debug)]
pub(super) enum PendingPackingCommand {
    Open {
        order_id: i64,
        request: OpenPackSessionRequest,
        idempotency_key: String,
    },
    CreateCarton {
        session_id: i64,
        request: CreateCartonRequest,
        idempotency_key: String,
    },
    PackAllocation {
        session_id: i64,
        carton_id: i64,
        request: PackPickedAllocationRequest,
        idempotency_key: String,
    },
    RemoveContent {
        session_id: i64,
        carton_id: i64,
        content_id: i64,
        request: RemovePackedContentRequest,
        idempotency_key: String,
    },
    CloseCarton {
        session_id: i64,
        carton_id: i64,
        request: CloseCartonRequest,
        idempotency_key: String,
    },
    VoidCarton {
        session_id: i64,
        carton_id: i64,
        request: VoidCartonRequest,
        idempotency_key: String,
    },
    AbandonSession {
        session_id: i64,
        request: AbandonPackSessionRequest,
        idempotency_key: String,
    },
    ReopenCarton {
        session_id: i64,
        carton_id: i64,
        request: ReopenCartonRequest,
        idempotency_key: String,
    },
}

pub(super) enum PackingCommandResult {
    Opened(Box<PackSessionResponse>),
    Created {
        order_id: i64,
        carton_barcode: String,
    },
    Packed {
        order_id: i64,
        quantity: i64,
        uom: String,
    },
    Removed {
        order_id: i64,
        quantity: i64,
        uom: String,
        destination_tote_barcode: String,
    },
    Closed {
        order_id: i64,
        carton_barcode: String,
        ready: bool,
    },
    Voided {
        order_id: i64,
        carton_barcode: String,
    },
    Abandoned {
        order_id: i64,
    },
    Reopened {
        order_id: i64,
        carton_barcode: String,
    },
}

impl PendingPackingCommand {
    pub(super) const fn pending_message(&self) -> &'static str {
        match self {
            Self::Open { .. } => "Opening pack session...",
            Self::CreateCarton { .. } => "Opening carton...",
            Self::PackAllocation { .. } => "Confirming packed item...",
            Self::RemoveContent { .. } => "Returning packed content to the picked tote...",
            Self::CloseCarton { .. } => "Closing carton...",
            Self::VoidCarton { .. } => "Voiding empty carton...",
            Self::AbandonSession { .. } => "Abandoning empty packing session...",
            Self::ReopenCarton { .. } => "Reopening carton...",
        }
    }
}

pub(super) async fn execute_command(
    command: &PendingPackingCommand,
) -> Result<PackingCommandResult, api::ApiError> {
    match command {
        PendingPackingCommand::Open {
            order_id,
            request,
            idempotency_key,
        } => api::open_pack_session(*order_id, request, idempotency_key)
            .await
            .map(|response| PackingCommandResult::Opened(Box::new(response.session))),
        PendingPackingCommand::CreateCarton {
            session_id,
            request,
            idempotency_key,
        } => api::create_pack_carton(*session_id, request, idempotency_key)
            .await
            .map(|response| PackingCommandResult::Created {
                order_id: response.order_id,
                carton_barcode: response.carton.carton_barcode,
            }),
        PendingPackingCommand::PackAllocation {
            session_id,
            carton_id,
            request,
            idempotency_key,
        } => api::pack_allocation(*session_id, *carton_id, request, idempotency_key)
            .await
            .map(|response| PackingCommandResult::Packed {
                order_id: response.order_id,
                quantity: response.quantity,
                uom: response.uom,
            }),
        PendingPackingCommand::RemoveContent {
            session_id,
            carton_id,
            content_id,
            request,
            idempotency_key,
        } => api::remove_pack_content(
            *session_id,
            *carton_id,
            *content_id,
            request,
            idempotency_key,
        )
        .await
        .map(|response| PackingCommandResult::Removed {
            order_id: response.order_id,
            quantity: response.quantity,
            uom: response.uom,
            destination_tote_barcode: request.destination_license_plate_barcode.clone(),
        }),
        PendingPackingCommand::CloseCarton {
            session_id,
            carton_id,
            request,
            idempotency_key,
        } => api::close_pack_carton(*session_id, *carton_id, request, idempotency_key)
            .await
            .map(|response| PackingCommandResult::Closed {
                order_id: response.order_id,
                carton_barcode: request.carton_barcode.clone(),
                ready: response.ready_to_manifest,
            }),
        PendingPackingCommand::VoidCarton {
            session_id,
            carton_id,
            request,
            idempotency_key,
        } => api::void_pack_carton(*session_id, *carton_id, request, idempotency_key)
            .await
            .map(|response| PackingCommandResult::Voided {
                order_id: response.order_id,
                carton_barcode: request.carton_barcode.clone(),
            }),
        PendingPackingCommand::AbandonSession {
            session_id,
            request,
            idempotency_key,
        } => api::abandon_pack_session(*session_id, request, idempotency_key)
            .await
            .map(|response| PackingCommandResult::Abandoned {
                order_id: response.order_id,
            }),
        PendingPackingCommand::ReopenCarton {
            session_id,
            carton_id,
            request,
            idempotency_key,
        } => api::reopen_pack_carton(*session_id, *carton_id, request, idempotency_key)
            .await
            .map(|response| PackingCommandResult::Reopened {
                order_id: response.order_id,
                carton_barcode: request.carton_barcode.clone(),
            }),
    }
}
