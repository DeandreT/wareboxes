use wareboxes_api_contract::v1::{
    ErrorResponse, ExpectedReceiptLineStatus, ExpectedReceivingLoadStatus,
    ExpectedReceivingLocation,
};

use super::*;
use crate::api::expected_receiving::ExpectedReceivingTransportFailure;

fn line(id: i64, barcode: &str, remaining: i64) -> ExpectedReceiptLine {
    ExpectedReceiptLine {
        load_line_id: id,
        item_id: id + 100,
        item_description: Some(format!("Item {id}")),
        uom: "case".into(),
        item_barcodes: vec![barcode.into()],
        expected_quantity: 10,
        received_quantity: 10 - remaining,
        rejected_quantity: 0,
        missing_quantity: 0,
        remaining_quantity: remaining,
        lot: Some("LOT-1".into()),
        serial: None,
        expiration: Some("2027-07-26T00:00:00+00:00".into()),
    }
}

fn session(lines: Vec<ExpectedReceiptLine>) -> ExpectedReceivingSessionResponse {
    ExpectedReceivingSessionResponse {
        load_id: 11,
        inventory_owner_id: 22,
        facility_id: 33,
        reference_number: Some("ASN-11".into()),
        status: ExpectedReceivingLoadStatus::Receiving,
        receiving_location: ExpectedReceivingLocation {
            location_id: 44,
            barcode: "DOCK-04".into(),
            name: Some("Dock 4".into()),
        },
        lines,
    }
}

fn load(state: &mut ExpectedReceivingWorkflowState, response: ExpectedReceivingSessionResponse) {
    state.load_id_draft = "11".into();
    let request = state.begin_session("load-1".into()).unwrap();
    assert_eq!(
        state.apply(ExpectedReceivingTransportEvent {
            request,
            outcome: Ok(ExpectedReceivingTransportOutcome::Session(response)),
        }),
        ExpectedReceivingApplyResult::Applied
    );
}

fn confirmation(
    disposition: ExpectedReceiptDisposition,
    quantity: i64,
    remaining: i64,
    complete: bool,
) -> ExpectedReceiptConfirmationResponse {
    ExpectedReceiptConfirmationResponse {
        load_id: 11,
        load_line_id: 55,
        disposition,
        quantity,
        inventory_transaction_id: (disposition == ExpectedReceiptDisposition::Received)
            .then_some(71),
        inventory_balance_id: (disposition == ExpectedReceiptDisposition::Received).then_some(72),
        item_batch_id: (disposition == ExpectedReceiptDisposition::Received).then_some(73),
        license_plate_id: None,
        line_status: if remaining == 0 {
            ExpectedReceiptLineStatus::Received
        } else {
            ExpectedReceiptLineStatus::Partial
        },
        load_status: if complete {
            ExpectedReceivingLoadStatus::Received
        } else {
            ExpectedReceivingLoadStatus::Receiving
        },
        cumulative_received_quantity: if disposition == ExpectedReceiptDisposition::Received {
            quantity
        } else {
            0
        },
        cumulative_rejected_quantity: if disposition == ExpectedReceiptDisposition::Rejected {
            quantity
        } else {
            0
        },
        cumulative_missing_quantity: if disposition == ExpectedReceiptDisposition::Missing {
            quantity
        } else {
            0
        },
        remaining_quantity: remaining,
        receive_completed: complete,
    }
}

#[test]
fn received_flow_requires_exact_item_and_dock_scans() {
    let mut state = ExpectedReceivingWorkflowState::default();
    load(&mut state, session(vec![line(55, "ITEM-55", 4)]));
    assert_eq!(
        state.scan_stage(),
        Some(ExpectedReceivingScanStage::ItemBarcode)
    );

    state.scan_draft = "WRONG".into();
    assert!(state
        .submit_scan("unused".into(), "unused".into())
        .is_none());
    assert!(state.scan_error().is_some());
    state.scan_draft = "ITEM-55".into();
    state.submit_scan("unused".into(), "unused".into());
    assert_eq!(state.selected_line_id(), Some(55));
    assert_eq!(
        state.scan_stage(),
        Some(ExpectedReceivingScanStage::ReceivingLocation)
    );

    state.scan_draft = "DOCK-03".into();
    state.submit_scan("unused".into(), "unused".into());
    assert!(state.scan_error().is_some());
    state.scan_draft = "DOCK-04".into();
    state.submit_scan("unused".into(), "unused".into());
    assert_eq!(state.scan_stage(), None);
    assert!(state
        .begin_confirmation("confirm-1".into(), "key-1".into())
        .is_some());
}

#[test]
fn item_scan_matches_ascii_case_insensitively_and_preserves_scanned_text() {
    let mut state = ExpectedReceivingWorkflowState::default();
    load(&mut state, session(vec![line(55, "Case-55", 4)]));
    state.scan_draft = "case-55".into();
    state.submit_scan("unused".into(), "unused".into());
    state.scan_draft = "DOCK-04".into();
    state.submit_scan("unused".into(), "unused".into());

    let request = state
        .begin_confirmation("confirm-1".into(), "receive-key".into())
        .unwrap();
    let ExpectedReceivingCommand::Confirm { body, .. } = request.command else {
        panic!("expected confirmation command");
    };
    let ConfirmExpectedReceiptRequest::Received { item_barcode, .. } = body else {
        panic!("expected received body");
    };
    assert_eq!(item_barcode, "case-55");
}

#[test]
fn duplicate_item_scan_requires_explicit_line_selection() {
    let mut state = ExpectedReceivingWorkflowState::default();
    load(
        &mut state,
        session(vec![line(55, "DUPLICATE", 2), line(56, "DUPLICATE", 3)]),
    );
    state.scan_draft = "DUPLICATE".into();
    state.submit_scan("unused".into(), "unused".into());
    assert_eq!(state.selected_line_id(), None);
    assert!(state.scan_error().unwrap().contains("multiple"));

    assert!(state.select_line(56));
    state.scan_draft = "DUPLICATE".into();
    state.submit_scan("unused".into(), "unused".into());
    assert_eq!(state.selected_line_id(), Some(56));
    assert_eq!(
        state.scan_stage(),
        Some(ExpectedReceivingScanStage::ReceivingLocation)
    );
}

#[test]
fn case_variant_duplicate_item_scan_is_ambiguous() {
    let mut state = ExpectedReceivingWorkflowState::default();
    load(
        &mut state,
        session(vec![line(55, "Case-55", 2), line(56, "CASE-55", 3)]),
    );
    state.scan_draft = "case-55".into();
    state.submit_scan("unused".into(), "unused".into());

    assert_eq!(state.selected_line_id(), None);
    assert!(state.scan_error().unwrap().contains("multiple"));
}

#[test]
fn missing_flow_never_fabricates_an_item_scan() {
    let mut state = ExpectedReceivingWorkflowState::default();
    load(&mut state, session(vec![line(55, "ITEM-55", 3)]));
    assert!(state.select_line(55));
    state.select_disposition(ExpectedReceiptDisposition::Missing);
    state.select_reason(ExpectedReceiptExceptionReason::ShortShipment);
    state.quantity_draft = "3".into();

    let request = state
        .begin_confirmation("missing-1".into(), "missing-key".into())
        .unwrap();
    let ExpectedReceivingCommand::Confirm { body, .. } = request.command else {
        panic!("expected confirmation command");
    };
    assert_eq!(
        body,
        ConfirmExpectedReceiptRequest::Missing {
            quantity: 3,
            reason: ExpectedReceiptExceptionReason::ShortShipment,
            note: None,
        }
    );
}

#[test]
fn retry_keeps_the_exact_command_and_key() {
    let mut state = ExpectedReceivingWorkflowState::default();
    load(&mut state, session(vec![line(55, "ITEM-55", 4)]));
    state.select_line(55);
    state.select_disposition(ExpectedReceiptDisposition::Missing);
    state.select_reason(ExpectedReceiptExceptionReason::Other);
    state.note_draft = "Not on trailer".into();
    let request = state
        .begin_confirmation("confirm-1".into(), "stable-key".into())
        .unwrap();
    let failed = ExpectedReceivingTransportEvent {
        request: request.clone(),
        outcome: Err(ExpectedReceivingTransportFailure {
            status: None,
            error: None,
            message: "network unavailable".into(),
        }),
    };
    state.apply(failed);

    let retry = state.retry("confirm-2".into()).unwrap();
    assert_ne!(retry.request_id, request.request_id);
    assert_eq!(retry.command, request.command);
}

#[test]
fn hung_request_becomes_retryable_without_changing_the_command() {
    let mut state = ExpectedReceivingWorkflowState {
        load_id_draft: "11".into(),
        ..ExpectedReceivingWorkflowState::default()
    };
    assert!(!state.tick(100.0));
    let request = state.begin_session("load-1".into()).unwrap();
    assert!(!state.tick(109.999));
    assert_eq!(state.activity(), ExpectedReceivingActivity::Pending);
    assert!(state.tick(110.0));
    assert_eq!(state.activity(), ExpectedReceivingActivity::Retryable);

    let retry = state.retry("load-2".into()).unwrap();
    assert_ne!(retry.request_id, request.request_id);
    assert_eq!(retry.command, request.command);
}

#[test]
fn initial_load_not_found_returns_to_editable_ready_state() {
    let mut state = ExpectedReceivingWorkflowState {
        load_id_draft: "11".into(),
        ..ExpectedReceivingWorkflowState::default()
    };
    let request = state.begin_session("load-1".into()).unwrap();
    state.apply(ExpectedReceivingTransportEvent {
        request,
        outcome: Err(ExpectedReceivingTransportFailure {
            status: Some(404),
            error: Some(ErrorResponse::new(
                ErrorReason::NotFound,
                "load not found",
                "request-1",
            )),
            message: "load not found".into(),
        }),
    });

    assert_eq!(state.activity(), ExpectedReceivingActivity::Ready);
    assert_eq!(state.load_id_draft, "11");
    assert_eq!(state.scan_stage(), Some(ExpectedReceivingScanStage::LoadId));
    assert!(state.retry("load-2".into()).is_none());
    state.load_id_draft = "12".into();
    assert!(state.begin_session("load-3".into()).is_some());
}

#[test]
fn deterministic_validation_failure_is_correctable_without_exact_retry() {
    let mut state = ExpectedReceivingWorkflowState::default();
    load(&mut state, session(vec![line(55, "ITEM-55", 4)]));
    state.select_line(55);
    state.select_disposition(ExpectedReceiptDisposition::Missing);
    state.select_reason(ExpectedReceiptExceptionReason::ShortShipment);
    let request = state
        .begin_confirmation("missing-1".into(), "missing-key".into())
        .unwrap();
    state.apply(ExpectedReceivingTransportEvent {
        request,
        outcome: Err(ExpectedReceivingTransportFailure {
            status: Some(422),
            error: Some(ErrorResponse::new(
                ErrorReason::ValidationFailed,
                "validation failed",
                "request-1",
            )),
            message: "quantity is invalid".into(),
        }),
    });

    assert_eq!(state.activity(), ExpectedReceivingActivity::Ready);
    assert_eq!(state.request_error(), Some("quantity is invalid"));
    assert!(state.retry("missing-2".into()).is_none());
    assert!(state.session().is_some());
}

#[test]
fn request_timeout_response_retains_exact_retry() {
    let mut state = ExpectedReceivingWorkflowState {
        load_id_draft: "11".into(),
        ..ExpectedReceivingWorkflowState::default()
    };
    let request = state.begin_session("load-1".into()).unwrap();
    state.apply(ExpectedReceivingTransportEvent {
        request: request.clone(),
        outcome: Err(ExpectedReceivingTransportFailure {
            status: Some(408),
            error: None,
            message: "request timed out".into(),
        }),
    });

    assert_eq!(state.activity(), ExpectedReceivingActivity::Retryable);
    let retry = state.retry("load-2".into()).unwrap();
    assert_eq!(retry.command, request.command);
}

#[test]
fn malformed_confirmation_success_requires_reconciliation() {
    let mut state = ExpectedReceivingWorkflowState::default();
    load(&mut state, session(vec![line(55, "ITEM-55", 4)]));
    state.select_line(55);
    state.select_disposition(ExpectedReceiptDisposition::Missing);
    let request = state
        .begin_confirmation("missing-1".into(), "missing-key".into())
        .unwrap();
    state.apply(ExpectedReceivingTransportEvent {
        request,
        outcome: Err(ExpectedReceivingTransportFailure {
            status: Some(200),
            error: None,
            message: "confirmation response was invalid".into(),
        }),
    });

    assert_eq!(
        state.activity(),
        ExpectedReceivingActivity::ReconcileRequired
    );
    assert!(state.reconcile("reload-1".into()).is_some());
}

#[test]
fn stale_response_is_ignored_and_conflict_requires_reconciliation() {
    let mut state = ExpectedReceivingWorkflowState {
        load_id_draft: "11".into(),
        ..ExpectedReceivingWorkflowState::default()
    };
    let request = state.begin_session("load-current".into()).unwrap();
    let stale = ExpectedReceivingTransportEvent {
        request: ExpectedReceivingRequest {
            request_id: "load-stale".into(),
            command: request.command.clone(),
        },
        outcome: Ok(ExpectedReceivingTransportOutcome::Session(session(vec![
            line(55, "ITEM-55", 4),
        ]))),
    };
    assert_eq!(state.apply(stale), ExpectedReceivingApplyResult::Ignored);

    state.apply(ExpectedReceivingTransportEvent {
        request,
        outcome: Ok(ExpectedReceivingTransportOutcome::Session(session(vec![
            line(55, "ITEM-55", 4),
        ]))),
    });
    let reload = state.reconcile("reload-current".into()).unwrap();
    state.apply(ExpectedReceivingTransportEvent {
        request: reload,
        outcome: Err(ExpectedReceivingTransportFailure {
            status: Some(409),
            error: Some(ErrorResponse::new(
                ErrorReason::Conflict,
                "load changed",
                "request-1",
            )),
            message: "load changed".into(),
        }),
    });
    assert_eq!(
        state.activity(),
        ExpectedReceivingActivity::ReconcileRequired
    );
    assert!(state.reconcile("reload-1".into()).is_some());
}

#[test]
fn confirmation_requests_reload_and_final_confirmation_completes() {
    let mut state = ExpectedReceivingWorkflowState::default();
    load(&mut state, session(vec![line(55, "ITEM-55", 4)]));
    state.select_line(55);
    state.select_disposition(ExpectedReceiptDisposition::Missing);
    state.select_reason(ExpectedReceiptExceptionReason::ShortShipment);
    state.quantity_draft = "2".into();
    let request = state
        .begin_confirmation("missing-1".into(), "missing-key-1".into())
        .unwrap();
    assert_eq!(
        state.apply(ExpectedReceivingTransportEvent {
            request,
            outcome: Ok(ExpectedReceivingTransportOutcome::Confirmation(
                confirmation(ExpectedReceiptDisposition::Missing, 2, 2, false),
            )),
        }),
        ExpectedReceivingApplyResult::ReloadRequired(11)
    );
    assert_eq!(state.session().unwrap().lines[0].remaining_quantity, 2);

    let reload = state.reconcile("reload-1".into()).unwrap();
    state.apply(ExpectedReceivingTransportEvent {
        request: reload,
        outcome: Ok(ExpectedReceivingTransportOutcome::Session(session(vec![
            line(55, "ITEM-55", 2),
        ]))),
    });
    state.select_line(55);
    state.select_disposition(ExpectedReceiptDisposition::Missing);
    state.select_reason(ExpectedReceiptExceptionReason::ShortShipment);
    state.quantity_draft = "2".into();
    let request = state
        .begin_confirmation("missing-2".into(), "missing-key-2".into())
        .unwrap();
    let result = state.apply(ExpectedReceivingTransportEvent {
        request,
        outcome: Ok(ExpectedReceivingTransportOutcome::Confirmation(
            confirmation(ExpectedReceiptDisposition::Missing, 2, 0, true),
        )),
    });
    assert!(matches!(result, ExpectedReceivingApplyResult::Completed(_)));
    assert!(state.session().is_none());
    assert_eq!(state.activity(), ExpectedReceivingActivity::Ready);
}

#[test]
fn confirmation_from_a_different_load_requires_reconciliation() {
    let mut state = ExpectedReceivingWorkflowState::default();
    load(&mut state, session(vec![line(55, "ITEM-55", 2)]));
    state.select_line(55);
    state.select_disposition(ExpectedReceiptDisposition::Missing);
    state.select_reason(ExpectedReceiptExceptionReason::ShortShipment);
    let request = state
        .begin_confirmation("missing-1".into(), "missing-key".into())
        .unwrap();
    let mut wrong_load = confirmation(ExpectedReceiptDisposition::Missing, 1, 1, false);
    wrong_load.load_id = 99;

    assert_eq!(
        state.apply(ExpectedReceivingTransportEvent {
            request,
            outcome: Ok(ExpectedReceivingTransportOutcome::Confirmation(wrong_load)),
        }),
        ExpectedReceivingApplyResult::Applied
    );
    assert_eq!(
        state.activity(),
        ExpectedReceivingActivity::ReconcileRequired
    );
    assert_eq!(state.session().unwrap().load_id, 11);
}
