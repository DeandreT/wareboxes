use serde_json::json;

use super::*;

fn load_id(value: i64) -> LoadId {
    LoadId::try_from(value).unwrap()
}

fn line_id(value: i64) -> LoadLineId {
    LoadLineId::try_from(value).unwrap()
}

fn owner_id(value: i64) -> InventoryOwnerId {
    InventoryOwnerId::try_from(value).unwrap()
}

fn facility_id(value: i64) -> FacilityId {
    FacilityId::try_from(value).unwrap()
}

fn location_id(value: i64) -> LocationId {
    LocationId::try_from(value).unwrap()
}

fn item_id(value: i64) -> ItemId {
    ItemId::try_from(value).unwrap()
}

fn positive(value: i64) -> PositiveQuantity {
    PositiveQuantity::try_from(value).unwrap()
}

fn quantity(value: i64) -> NonNegativeQuantity {
    NonNegativeQuantity::new(value).unwrap()
}

fn line(
    id: i64,
    barcode: &str,
    expected: i64,
    received: i64,
    rejected: i64,
    missing: i64,
) -> ExpectedReceiptLine {
    let remaining = expected - received - rejected - missing;
    ExpectedReceiptLine::try_new(ExpectedReceiptLineInput {
        load_line_id: line_id(id),
        item_id: item_id(id + 1_000),
        item_description: Some(format!("Item {id}")),
        uom: StockDimension::new("case").unwrap(),
        item_barcodes: vec![ItemBarcode::new(barcode).unwrap()],
        expected: positive(expected),
        received: quantity(received),
        rejected: quantity(rejected),
        missing: quantity(missing),
        remaining: quantity(remaining),
        lot: Some(StockDimension::new("LOT-1").unwrap()),
        serial: None,
        expiration: Some(Expiration::new("2027-07-26T00:00:00Z").unwrap()),
    })
    .unwrap()
}

fn session_with(
    load: i64,
    owner: i64,
    facility: i64,
    lines: Vec<ExpectedReceiptLine>,
) -> ReceivingSession {
    ReceivingSession::try_new(ReceivingSessionInput {
        load_id: load_id(load),
        inventory_owner_id: owner_id(owner),
        facility_id: facility_id(facility),
        reference_number: Some(format!("ASN-{load}")),
        status: ReceivingLoadStatus::Arrived,
        dock: ReceivingDock::new(
            location_id(90),
            DockBarcode::new("DOCK-1").unwrap(),
            Some("Receiving Dock 1".into()),
        ),
        lines,
    })
    .unwrap()
}

fn session(lines: Vec<ExpectedReceiptLine>) -> ReceivingSession {
    session_with(10, 20, 30, lines)
}

fn recovery_snapshot(selected_line: ExpectedReceiptLine) -> ConfirmationRecoverySnapshot {
    ConfirmationRecoverySnapshot::try_new(ConfirmationRecoverySnapshotInput {
        load_barcode: LoadBarcode::new("LOAD-10").unwrap(),
        load_id: load_id(10),
        inventory_owner_id: owner_id(20),
        facility_id: facility_id(30),
        reference_number: Some("ASN-10".into()),
        status: ReceivingLoadStatus::Arrived,
        dock: ReceivingDock::new(
            location_id(90),
            DockBarcode::new("DOCK-1").unwrap(),
            Some("Receiving Dock 1".into()),
        ),
        selected_line,
    })
    .unwrap()
}

fn resolve(reducer: &mut ExpectedReceivingReducer, session: ReceivingSession) -> LoadResolutionId {
    let effect = reducer.scan_load(" wb:load-10 ");
    let ReceivingTransition::Effect(ReceivingEffect::ResolveLoad {
        resolution_id,
        barcode,
    }) = effect
    else {
        panic!("load scan should request resolution");
    };
    assert_eq!(barcode.as_str(), "WB:LOAD-10");
    assert_eq!(
        reducer.load_resolved(resolution_id, session),
        ReceivingTransition::Applied
    );
    resolution_id
}

fn prepare_received(
    reducer: &mut ExpectedReceivingReducer,
    receive_quantity: i64,
) -> (ConfirmationId, ConfirmationIntent) {
    assert_eq!(reducer.scan_item("ITEM-1"), ReceivingTransition::Applied);
    assert_eq!(reducer.scan_dock("DOCK-1"), ReceivingTransition::Applied);
    assert_eq!(
        reducer.set_quantity(receive_quantity),
        ReceivingTransition::Applied
    );
    let effect = reducer.begin_confirmation(CommandAccess::Allowed);
    let ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation {
        confirmation_id,
        intent,
    }) = effect
    else {
        panic!("ready receipt should produce a durable intent");
    };
    (confirmation_id, intent)
}

fn partial_result(receive_quantity: i64) -> ConfirmationResult {
    ConfirmationResult {
        load_id: load_id(10),
        load_line_id: line_id(100),
        disposition: ConfirmationMode::Received,
        quantity: positive(receive_quantity),
        cumulative_received: quantity(receive_quantity),
        cumulative_rejected: quantity(0),
        cumulative_missing: quantity(0),
        remaining: quantity(10 - receive_quantity),
        receive_completed: false,
    }
}

#[test]
fn load_barcode_uses_server_alphabet_and_canonical_form() {
    for (input, expected) in [
        ("asn-100", "ASN-100"),
        ("  wb:load_10.2  ", "WB:LOAD_10.2"),
        ("9", "9"),
    ] {
        assert_eq!(LoadBarcode::new(input).unwrap().as_str(), expected);
    }
    let maximum_length = "A".repeat(200);
    assert_eq!(
        LoadBarcode::new(&maximum_length).unwrap().as_str(),
        maximum_length
    );

    for invalid in ["", " ", "-ASN-1", ".ASN-1", "ASN 1", "ASN/1", "ÅSN-1"] {
        assert_eq!(
            LoadBarcode::new(invalid),
            Err(ReceivingValidationError::InvalidLoadBarcode)
        );
    }
    assert_eq!(
        LoadBarcode::new("A".repeat(201)),
        Err(ReceivingValidationError::InvalidLoadBarcode)
    );

    let decoded: LoadBarcode = serde_json::from_str(r#""  load:7  ""#).unwrap();
    assert_eq!(decoded.as_str(), "LOAD:7");
    assert_eq!(serde_json::to_string(&decoded).unwrap(), r#""LOAD:7""#);
}

#[test]
fn other_scanner_codes_preserve_case_and_require_trimmed_input() {
    assert_eq!(ItemBarcode::new("item-a").unwrap().as_str(), "item-a");
    assert_eq!(
        DockBarcode::new(" DOCK-1 "),
        Err(ReceivingValidationError::InvalidBarcode)
    );
    assert_eq!(LicensePlateBarcode::new("LP/01").unwrap().as_str(), "LP/01");
}

#[test]
fn line_and_session_boundaries_reject_invalid_projections() {
    let invalid = ExpectedReceiptLine::try_new(ExpectedReceiptLineInput {
        load_line_id: line_id(1),
        item_id: item_id(2),
        item_description: None,
        uom: StockDimension::new("each").unwrap(),
        item_barcodes: vec![ItemBarcode::new("ITEM").unwrap()],
        expected: positive(10),
        received: quantity(4),
        rejected: quantity(1),
        missing: quantity(1),
        remaining: quantity(5),
        lot: None,
        serial: None,
        expiration: None,
    });
    assert_eq!(
        invalid,
        Err(ReceivingValidationError::InvalidLineQuantities)
    );

    let closed = line(1, "ITEM", 10, 10, 0, 0);
    assert_eq!(
        ReceivingSession::try_new(ReceivingSessionInput {
            load_id: load_id(1),
            inventory_owner_id: owner_id(2),
            facility_id: facility_id(3),
            reference_number: None,
            status: ReceivingLoadStatus::Receiving,
            dock: ReceivingDock::new(location_id(4), DockBarcode::new("DOCK").unwrap(), None,),
            lines: vec![closed],
        }),
        Err(ReceivingValidationError::ClosedLineInSession)
    );
}

#[test]
fn load_resolution_is_correlated_retryable_and_stale_safe() {
    let mut reducer = ExpectedReceivingReducer::default();
    assert_eq!(
        reducer.focus_target(),
        FocusTarget::Scanner(ScannerTarget::LoadBarcode)
    );
    let ReceivingTransition::Effect(ReceivingEffect::ResolveLoad { resolution_id, .. }) =
        reducer.scan_load("LOAD-10")
    else {
        panic!("valid load scan should resolve");
    };
    assert_eq!(
        reducer.focus_target(),
        FocusTarget::Blocked(InteractionBlock::ResolvingLoad)
    );
    assert_eq!(
        reducer.load_resolved(
            LoadResolutionId(999),
            session(vec![line(100, "ITEM-1", 10, 0, 0, 0)])
        ),
        ReceivingTransition::Ignored
    );
    assert_eq!(
        reducer.load_resolution_failed(resolution_id, LoadResolutionFailure::Retryable),
        ReceivingTransition::Applied
    );
    assert_eq!(
        reducer.operator_error(),
        Some(&ReceivingOperatorError::ConnectionUnavailable)
    );

    let ReceivingTransition::Effect(ReceivingEffect::ResolveLoad {
        resolution_id: retry_id,
        ..
    }) = reducer.retry_load_resolution()
    else {
        panic!("retry should resolve the same scan");
    };
    assert_ne!(retry_id, resolution_id);
    assert_eq!(
        reducer.load_resolved(
            resolution_id,
            session(vec![line(100, "ITEM-1", 10, 0, 0, 0)])
        ),
        ReceivingTransition::Ignored
    );
    assert_eq!(
        reducer.load_resolved(retry_id, session(vec![line(100, "ITEM-1", 10, 0, 0, 0)])),
        ReceivingTransition::Applied
    );
}

#[test]
fn invalid_load_response_requires_reconciliation() {
    let mut reducer = ExpectedReceivingReducer::default();
    let ReceivingTransition::Effect(ReceivingEffect::ResolveLoad { resolution_id, .. }) =
        reducer.scan_load("LOAD-10")
    else {
        panic!("valid load scan should resolve");
    };
    assert_eq!(
        reducer.load_resolution_failed(resolution_id, LoadResolutionFailure::InvalidResponse),
        ReceivingTransition::ReconciliationRequired(ReconciliationReason::InvalidServerState)
    );
    assert_eq!(
        reducer.focus_target(),
        FocusTarget::Blocked(InteractionBlock::ReconciliationRequired)
    );
}

#[test]
fn item_scan_selects_unique_line_and_reports_ambiguity() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![
            line(100, "SHARED", 10, 0, 0, 0),
            line(101, "SHARED", 4, 0, 0, 0),
        ]),
    );
    assert_eq!(reducer.scan_item("UNKNOWN"), ReceivingTransition::Applied);
    assert_eq!(
        reducer.operator_error(),
        Some(&ReceivingOperatorError::ItemNotExpected)
    );
    assert_eq!(reducer.scan_item("shared"), ReceivingTransition::Applied);
    assert_eq!(
        reducer.operator_error(),
        Some(&ReceivingOperatorError::ItemMatchesMultipleLines {
            line_ids: vec![line_id(100), line_id(101)]
        })
    );
    assert!(reducer.selected_line().is_none());
    assert_eq!(
        reducer.select_line(line_id(101)),
        ReceivingTransition::Applied
    );
    assert_eq!(
        reducer
            .selected_line()
            .map(ExpectedReceiptLine::load_line_id),
        Some(line_id(101))
    );
    assert_eq!(
        reducer.focus_target(),
        FocusTarget::Scanner(ScannerTarget::DockBarcode)
    );
}

#[test]
fn received_flow_has_explicit_focus_and_exact_serializable_intent() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    assert_eq!(
        reducer.focus_target(),
        FocusTarget::Scanner(ScannerTarget::ItemBarcode)
    );
    reducer.scan_item("ITEM-1");
    assert_eq!(
        reducer.focus_target(),
        FocusTarget::Scanner(ScannerTarget::DockBarcode)
    );
    reducer.scan_dock("WRONG");
    assert_eq!(
        reducer.operator_error(),
        Some(&ReceivingOperatorError::WrongReceivingDock)
    );
    reducer.scan_dock("DOCK-1");
    assert_eq!(reducer.focus_target(), FocusTarget::Quantity);
    reducer.set_quantity(3);
    reducer.set_container_capture(ContainerCapture::LicensePlate);
    assert_eq!(
        reducer.focus_target(),
        FocusTarget::Scanner(ScannerTarget::LicensePlateBarcode)
    );
    reducer.scan_license_plate("LP-100");
    assert_eq!(reducer.focus_target(), FocusTarget::ConfirmAction);
    assert_eq!(
        reducer.confirmation_guard(CommandAccess::Blocked(CommandAccessBlock::Offline)),
        ActionGuard::Blocked(ActionBlockReason::Device(CommandAccessBlock::Offline))
    );

    let ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation {
        confirmation_id: _,
        intent,
    }) = reducer.begin_confirmation(CommandAccess::Allowed)
    else {
        panic!("ready receipt should persist");
    };
    assert!(intent.is_current_and_valid());
    assert_eq!(
        serde_json::to_value(&intent).unwrap(),
        json!({
            "schema_version": 2,
            "load_id": 10,
            "load_line_id": 100,
            "command": {
                "disposition": "received",
                "item_barcode": "ITEM-1",
                "receiving_location_barcode": "DOCK-1",
                "quantity": 3,
                "license_plate_barcode": "LP-100",
                "lot": "LOT-1",
                "serial": null,
                "expiration": "2027-07-26T00:00:00Z"
            },
            "recovery": {
                "load_barcode": "WB:LOAD-10",
                "load_id": 10,
                "inventory_owner_id": 20,
                "facility_id": 30,
                "reference_number": "ASN-10",
                "status": "arrived",
                "dock": {
                    "location_id": 90,
                    "barcode": "DOCK-1",
                    "name": "Receiving Dock 1"
                },
                "selected_line": {
                    "load_line_id": 100,
                    "item_id": 1100,
                    "item_description": "Item 100",
                    "uom": "case",
                    "item_barcodes": ["ITEM-1"],
                    "expected": 10,
                    "received": 0,
                    "rejected": 0,
                    "missing": 0,
                    "remaining": 10,
                    "lot": "LOT-1",
                    "serial": null,
                    "expiration": "2027-07-26T00:00:00Z"
                }
            }
        })
    );
    assert_eq!(
        serde_json::from_slice::<ConfirmationIntent>(&intent.canonical_payload().unwrap()).unwrap(),
        intent
    );
    assert_eq!(
        reducer.focus_target(),
        FocusTarget::Blocked(InteractionBlock::ConfirmationPending)
    );
}

#[test]
fn quantity_and_required_scans_guard_received_confirmation() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(&mut reducer, session(vec![line(100, "ITEM-1", 5, 0, 0, 0)]));
    assert_eq!(
        reducer.confirmation_guard(CommandAccess::Allowed),
        ActionGuard::Blocked(ActionBlockReason::NoSelectedLine)
    );
    reducer.scan_item("ITEM-1");
    assert_eq!(
        reducer.confirmation_guard(CommandAccess::Allowed),
        ActionGuard::Blocked(ActionBlockReason::QuantityRequired)
    );
    assert_eq!(reducer.set_quantity(6), ReceivingTransition::Applied);
    assert_eq!(
        reducer.operator_error(),
        Some(&ReceivingOperatorError::QuantityExceedsRemaining)
    );
    reducer.set_quantity(5);
    assert_eq!(
        reducer.confirmation_guard(CommandAccess::Allowed),
        ActionGuard::Blocked(ActionBlockReason::DockScanRequired)
    );
    reducer.scan_dock("DOCK-1");
    reducer.set_container_capture(ContainerCapture::LicensePlate);
    assert_eq!(
        reducer.confirmation_guard(CommandAccess::Allowed),
        ActionGuard::Blocked(ActionBlockReason::LicensePlateScanRequired)
    );
}

#[test]
fn rejected_intent_requires_reason_and_other_note() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    reducer.scan_item("ITEM-1");
    reducer.set_quantity(2);
    reducer.select_mode(ConfirmationMode::Rejected);
    assert_eq!(reducer.focus_target(), FocusTarget::ExceptionReason);
    reducer.set_exception_reason(ReceiptExceptionReason::Other);
    assert_eq!(reducer.focus_target(), FocusTarget::ExceptionNote);
    assert_eq!(
        reducer.confirmation_guard(CommandAccess::Allowed),
        ActionGuard::Blocked(ActionBlockReason::ExceptionNoteRequired)
    );
    reducer.set_exception_note(Some("Cartons failed inspection"));

    let ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation { intent, .. }) =
        reducer.begin_confirmation(CommandAccess::Allowed)
    else {
        panic!("complete rejection should persist");
    };
    assert_eq!(
        intent.command,
        ExpectedReceiptCommand::Rejected {
            item_barcode: ItemBarcode::new("ITEM-1").unwrap(),
            quantity: positive(2),
            reason: ReceiptExceptionReason::Other,
            note: Some(ExceptionNote::new("Cartons failed inspection").unwrap())
        }
    );
}

#[test]
fn missing_intent_can_select_a_line_without_claiming_an_item_scan() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(&mut reducer, session(vec![line(100, "ITEM-1", 4, 0, 0, 0)]));
    reducer.select_line(line_id(100));
    reducer.select_mode(ConfirmationMode::Missing);
    assert_eq!(reducer.focus_target(), FocusTarget::Quantity);
    reducer.set_quantity(4);
    reducer.set_exception_reason(ReceiptExceptionReason::ShortShipment);

    let ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation { intent, .. }) =
        reducer.begin_confirmation(CommandAccess::Allowed)
    else {
        panic!("missing confirmation should persist");
    };
    assert_eq!(
        intent.command,
        ExpectedReceiptCommand::Missing {
            quantity: positive(4),
            reason: ReceiptExceptionReason::ShortShipment,
            note: None
        }
    );
}

#[test]
fn definitive_rejection_restores_the_exact_operator_draft() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    let (confirmation_id, first_intent) = prepare_received(&mut reducer, 3);
    assert_eq!(
        reducer.confirmation_failed(confirmation_id, ConfirmationFailure::Rejected),
        ReceivingTransition::Applied
    );
    assert_eq!(
        reducer.operator_error(),
        Some(&ReceivingOperatorError::ConfirmationRejected)
    );
    assert_eq!(reducer.focus_target(), FocusTarget::ConfirmAction);

    let ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation {
        intent: retried_intent,
        ..
    }) = reducer.begin_confirmation(CommandAccess::Allowed)
    else {
        panic!("corrected retry should remain available");
    };
    assert_eq!(retried_intent, first_intent);
}

#[test]
fn durable_intent_round_trip_restores_pending_work_and_exact_draft() {
    let mut before_restart = ExpectedReceivingReducer::default();
    resolve(
        &mut before_restart,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    before_restart.scan_item("ITEM-1");
    before_restart.scan_dock("DOCK-1");
    before_restart.set_quantity(3);
    before_restart.set_container_capture(ContainerCapture::LicensePlate);
    before_restart.scan_license_plate("LP-RESTART-1");
    let ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation { intent, .. }) =
        before_restart.begin_confirmation(CommandAccess::Allowed)
    else {
        panic!("complete license plate receipt should persist");
    };
    let durable_payload = intent.canonical_payload().unwrap();
    let recovered: ConfirmationIntent = serde_json::from_slice(&durable_payload).unwrap();
    assert_eq!(recovered, intent);

    let mut restarted = ExpectedReceivingReducer::default();
    let confirmation_id = restarted
        .restore_pending_confirmation(recovered.clone())
        .unwrap();
    assert_eq!(restarted.activity(), ReceivingActivity::ConfirmationPending);
    assert_eq!(
        restarted.focus_target(),
        FocusTarget::Blocked(InteractionBlock::ConfirmationPending)
    );
    let restored_session = restarted.session().unwrap();
    assert_eq!(restored_session.load_id(), load_id(10));
    assert_eq!(restored_session.inventory_owner_id(), owner_id(20));
    assert_eq!(restored_session.facility_id(), facility_id(30));
    assert_eq!(restored_session.reference_number(), Some("ASN-10"));
    assert_eq!(restored_session.lines().len(), 1);

    assert_eq!(
        restarted.confirmation_failed(confirmation_id, ConfirmationFailure::Rejected),
        ReceivingTransition::Applied
    );
    assert_eq!(restarted.focus_target(), FocusTarget::ConfirmAction);
    let ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation {
        intent: retried, ..
    }) = restarted.begin_confirmation(CommandAccess::Allowed)
    else {
        panic!("restored rejected command should retain its exact draft");
    };
    assert_eq!(retried, recovered);
}

#[test]
fn draft_view_exposes_the_exact_scans_and_controls_for_pending_work() {
    let mut reducer = ExpectedReceivingReducer::default();
    assert!(reducer.confirmation_draft_view().is_none());
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    assert_eq!(
        reducer.confirmation_draft_view(),
        Some(ConfirmationDraftView {
            mode: ConfirmationMode::Received,
            selected_line_id: None,
            item_barcode: None,
            dock_barcode: None,
            quantity: None,
            container_capture: ContainerCapture::Loose,
            license_plate_barcode: None,
            exception_reason: None,
            exception_note: None,
        })
    );

    reducer.scan_item("ITEM-1");
    reducer.scan_dock("DOCK-1");
    reducer.set_quantity(3);
    reducer.set_container_capture(ContainerCapture::LicensePlate);
    reducer.scan_license_plate("LP-100");
    let expected_item = ItemBarcode::new("ITEM-1").unwrap();
    let expected_dock = DockBarcode::new("DOCK-1").unwrap();
    let expected_license_plate = LicensePlateBarcode::new("LP-100").unwrap();
    assert_eq!(
        reducer.confirmation_draft_view(),
        Some(ConfirmationDraftView {
            mode: ConfirmationMode::Received,
            selected_line_id: Some(line_id(100)),
            item_barcode: Some(&expected_item),
            dock_barcode: Some(&expected_dock),
            quantity: Some(positive(3)),
            container_capture: ContainerCapture::LicensePlate,
            license_plate_barcode: Some(&expected_license_plate),
            exception_reason: None,
            exception_note: None,
        })
    );
    assert!(matches!(
        reducer.begin_confirmation(CommandAccess::Allowed),
        ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation { .. })
    ));
    assert_eq!(
        reducer
            .confirmation_draft_view()
            .and_then(|draft| draft.license_plate_barcode)
            .map(LicensePlateBarcode::as_str),
        Some("LP-100")
    );
}

#[test]
fn recovered_rejected_draft_view_preserves_operator_visible_values() {
    let intent = ConfirmationIntent::try_new(
        recovery_snapshot(line(100, "ITEM-1", 10, 0, 0, 0)),
        ExpectedReceiptCommand::Rejected {
            item_barcode: ItemBarcode::new("ITEM-1").unwrap(),
            quantity: positive(2),
            reason: ReceiptExceptionReason::Other,
            note: Some(ExceptionNote::new("Seal did not match").unwrap()),
        },
    )
    .unwrap();
    let intent: ConfirmationIntent =
        serde_json::from_slice(&intent.canonical_payload().unwrap()).unwrap();
    let mut restarted = ExpectedReceivingReducer::default();
    let confirmation_id = restarted.restore_pending_confirmation(intent).unwrap();

    let view = restarted.confirmation_draft_view().unwrap();
    assert_eq!(view.mode, ConfirmationMode::Rejected);
    assert_eq!(view.selected_line_id, Some(line_id(100)));
    assert_eq!(view.item_barcode.map(ItemBarcode::as_str), Some("ITEM-1"));
    assert_eq!(view.dock_barcode, None);
    assert_eq!(view.quantity, Some(positive(2)));
    assert_eq!(view.container_capture, ContainerCapture::Loose);
    assert_eq!(view.license_plate_barcode, None);
    assert_eq!(view.exception_reason, Some(ReceiptExceptionReason::Other));
    assert_eq!(view.exception_note, Some("Seal did not match"));

    restarted.confirmation_failed(confirmation_id, ConfirmationFailure::Rejected);
    assert_eq!(
        restarted
            .confirmation_draft_view()
            .and_then(|draft| draft.exception_note),
        Some("Seal did not match")
    );
}

#[test]
fn exception_intents_restore_their_exact_rejected_and_missing_drafts() {
    let commands = [
        ExpectedReceiptCommand::Rejected {
            item_barcode: ItemBarcode::new("ITEM-1").unwrap(),
            quantity: positive(2),
            reason: ReceiptExceptionReason::Other,
            note: Some(ExceptionNote::new("Seal did not match").unwrap()),
        },
        ExpectedReceiptCommand::Missing {
            quantity: positive(4),
            reason: ReceiptExceptionReason::ShortShipment,
            note: Some(ExceptionNote::new("Carrier count was short").unwrap()),
        },
    ];

    for command in commands {
        let intent = ConfirmationIntent::try_new(
            recovery_snapshot(line(100, "ITEM-1", 10, 0, 0, 0)),
            command,
        )
        .unwrap();
        let intent: ConfirmationIntent =
            serde_json::from_slice(&intent.canonical_payload().unwrap()).unwrap();
        let mut restarted = ExpectedReceivingReducer::default();
        let confirmation_id = restarted
            .restore_pending_confirmation(intent.clone())
            .unwrap();
        assert_eq!(
            restarted.confirmation_failed(confirmation_id, ConfirmationFailure::Rejected),
            ReceivingTransition::Applied
        );
        assert_eq!(restarted.focus_target(), FocusTarget::ConfirmAction);
        let ReceivingTransition::Effect(ReceivingEffect::PersistConfirmation {
            intent: retried,
            ..
        }) = restarted.begin_confirmation(CommandAccess::Allowed)
        else {
            panic!("restored exception command should retain its exact draft");
        };
        assert_eq!(retried, intent);
    }
}

#[test]
fn restored_pending_work_validates_success_before_refresh_or_completion() {
    let mut before_restart = ExpectedReceivingReducer::default();
    resolve(
        &mut before_restart,
        session(vec![line(100, "ITEM-1", 10, 2, 0, 0)]),
    );
    let (_, partial_intent) = prepare_received(&mut before_restart, 3);

    let mut restarted = ExpectedReceivingReducer::default();
    let confirmation_id = restarted
        .restore_pending_confirmation(partial_intent)
        .unwrap();
    let result = ConfirmationResult {
        load_id: load_id(10),
        load_line_id: line_id(100),
        disposition: ConfirmationMode::Received,
        quantity: positive(3),
        cumulative_received: quantity(5),
        cumulative_rejected: quantity(0),
        cumulative_missing: quantity(0),
        remaining: quantity(5),
        receive_completed: false,
    };
    assert!(matches!(
        restarted.confirmation_succeeded(confirmation_id, result),
        ReceivingTransition::Effect(ReceivingEffect::RefreshSession { .. })
    ));

    let mut before_restart = ExpectedReceivingReducer::default();
    resolve(
        &mut before_restart,
        session(vec![line(100, "ITEM-1", 3, 0, 0, 0)]),
    );
    let (_, completing_intent) = prepare_received(&mut before_restart, 3);
    let mut restarted = ExpectedReceivingReducer::default();
    let confirmation_id = restarted
        .restore_pending_confirmation(completing_intent)
        .unwrap();
    let result = ConfirmationResult {
        load_id: load_id(10),
        load_line_id: line_id(100),
        disposition: ConfirmationMode::Received,
        quantity: positive(3),
        cumulative_received: quantity(3),
        cumulative_rejected: quantity(0),
        cumulative_missing: quantity(0),
        remaining: quantity(0),
        receive_completed: true,
    };
    assert_eq!(
        restarted.confirmation_succeeded(confirmation_id, result),
        ReceivingTransition::Applied
    );
    assert_eq!(restarted.activity(), ReceivingActivity::LoadComplete);
}

#[test]
fn recovery_context_tampering_fails_closed() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    let (_, intent) = prepare_received(&mut reducer, 3);

    let mut mismatched_load = serde_json::to_value(&intent).unwrap();
    mismatched_load["recovery"]["load_id"] = json!(11);
    let mismatched_load: ConfirmationIntent = serde_json::from_value(mismatched_load).unwrap();
    assert!(!mismatched_load.is_current_and_valid());

    let mut wrong_item = serde_json::to_value(&intent).unwrap();
    wrong_item["command"]["item_barcode"] = json!("OTHER-ITEM");
    let wrong_item: ConfirmationIntent = serde_json::from_value(wrong_item).unwrap();
    assert!(!wrong_item.is_current_and_valid());

    let mut wrong_dock = serde_json::to_value(&intent).unwrap();
    wrong_dock["recovery"]["dock"]["barcode"] = json!("DOCK-2");
    let wrong_dock: ConfirmationIntent = serde_json::from_value(wrong_dock).unwrap();
    assert!(!wrong_dock.is_current_and_valid());

    let mut excessive_quantity = serde_json::to_value(&intent).unwrap();
    excessive_quantity["recovery"]["selected_line"]["expected"] = json!(2);
    excessive_quantity["recovery"]["selected_line"]["remaining"] = json!(2);
    let excessive_quantity: ConfirmationIntent =
        serde_json::from_value(excessive_quantity).unwrap();
    assert!(!excessive_quantity.is_current_and_valid());

    let mut restarted = ExpectedReceivingReducer::default();
    assert_eq!(
        restarted.restore_pending_confirmation(wrong_item),
        Err(ReconciliationReason::CommandIntegrityFailure)
    );
    assert_eq!(restarted.activity(), ReceivingActivity::ReconcileRequired);
}

#[test]
fn recovery_snapshot_deserialization_revalidates_the_selected_line() {
    let snapshot = recovery_snapshot(line(100, "ITEM-1", 10, 0, 0, 0));
    let mut invalid = serde_json::to_value(snapshot).unwrap();
    invalid["selected_line"]["remaining"] = json!(0);
    invalid["selected_line"]["received"] = json!(10);
    assert!(
        serde_json::from_value::<ConfirmationRecoverySnapshot>(invalid).is_err(),
        "a closed selected line cannot be restored as pending work"
    );
}

#[test]
fn partial_success_updates_cumulative_state_then_requires_refresh() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    let (confirmation_id, _) = prepare_received(&mut reducer, 3);
    let ReceivingTransition::Effect(ReceivingEffect::RefreshSession {
        refresh_id,
        load_id: refresh_load,
    }) = reducer.confirmation_succeeded(confirmation_id, partial_result(3))
    else {
        panic!("partial success should refresh");
    };
    assert_eq!(refresh_load, load_id(10));
    assert_eq!(
        reducer.last_confirmation(),
        Some(ConfirmationSummary {
            load_id: load_id(10),
            load_line_id: line_id(100),
            disposition: ConfirmationMode::Received,
            quantity: positive(3),
            cumulative_received: quantity(3),
            cumulative_rejected: quantity(0),
            cumulative_missing: quantity(0),
            remaining: quantity(7),
            receive_completed: false,
        })
    );
    assert_eq!(
        reducer.focus_target(),
        FocusTarget::Blocked(InteractionBlock::Refreshing)
    );

    let refreshed = session(vec![line(100, "ITEM-1", 10, 3, 0, 0)]);
    assert_eq!(
        reducer.refresh_succeeded(refresh_id, refreshed),
        ReceivingTransition::Applied
    );
    assert_eq!(reducer.activity(), ReceivingActivity::Active);
    assert_eq!(
        reducer.focus_target(),
        FocusTarget::Scanner(ScannerTarget::ItemBarcode)
    );
}

#[test]
fn completed_load_accepts_no_more_actions_but_can_scan_new_work() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(&mut reducer, session(vec![line(100, "ITEM-1", 3, 0, 0, 0)]));
    let (confirmation_id, _) = prepare_received(&mut reducer, 3);
    let mut result = partial_result(3);
    result.remaining = quantity(0);
    result.receive_completed = true;
    assert_eq!(
        reducer.confirmation_succeeded(confirmation_id, result),
        ReceivingTransition::Applied
    );
    assert_eq!(reducer.activity(), ReceivingActivity::LoadComplete);
    assert_eq!(
        reducer.confirmation_guard(CommandAccess::Allowed),
        ActionGuard::Blocked(ActionBlockReason::LoadComplete)
    );
    assert!(matches!(
        reducer.scan_load("LOAD-11"),
        ReceivingTransition::Effect(ReceivingEffect::ResolveLoad { .. })
    ));
}

#[test]
fn mismatched_confirmation_requires_reconciliation() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    let (confirmation_id, _) = prepare_received(&mut reducer, 3);
    let mut result = partial_result(3);
    result.load_id = load_id(11);
    assert_eq!(
        reducer.confirmation_succeeded(confirmation_id, result),
        ReceivingTransition::ReconciliationRequired(
            ReconciliationReason::ConfirmationIdentityMismatch
        )
    );
    assert_eq!(reducer.activity(), ReceivingActivity::ReconcileRequired);
    assert_eq!(
        reducer.confirmation_guard(CommandAccess::Allowed),
        ActionGuard::Blocked(ActionBlockReason::ReconciliationRequired)
    );
}

#[test]
fn invalid_cumulative_response_requires_reconciliation() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    let (confirmation_id, _) = prepare_received(&mut reducer, 3);
    let mut result = partial_result(3);
    result.cumulative_received = quantity(4);
    result.remaining = quantity(6);
    assert_eq!(
        reducer.confirmation_succeeded(confirmation_id, result),
        ReceivingTransition::ReconciliationRequired(
            ReconciliationReason::CumulativeQuantityInvalid
        )
    );
}

#[test]
fn refresh_retry_blocks_scans_and_reuses_local_cumulative_result() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    let (confirmation_id, _) = prepare_received(&mut reducer, 3);
    let ReceivingTransition::Effect(ReceivingEffect::RefreshSession { refresh_id, .. }) =
        reducer.confirmation_succeeded(confirmation_id, partial_result(3))
    else {
        panic!("partial success should refresh");
    };
    assert_eq!(
        reducer.refresh_failed(refresh_id, RefreshFailure::Retryable),
        ReceivingTransition::Applied
    );
    assert_eq!(reducer.activity(), ReceivingActivity::RefreshFailed);
    assert_eq!(
        reducer.submit_scan("ITEM-1"),
        ReceivingTransition::Blocked(ActionBlockReason::WorkflowBusy)
    );
    let ReceivingTransition::Effect(ReceivingEffect::RefreshSession {
        refresh_id: retry_id,
        ..
    }) = reducer.retry_refresh()
    else {
        panic!("refresh retry should be emitted");
    };
    assert_ne!(retry_id, refresh_id);
    assert_eq!(
        reducer.refresh_succeeded(refresh_id, session(vec![line(100, "ITEM-1", 10, 3, 0, 0)])),
        ReceivingTransition::Ignored
    );
}

#[test]
fn refresh_aggregate_or_quantity_regression_requires_reconciliation() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    let (confirmation_id, _) = prepare_received(&mut reducer, 3);
    let ReceivingTransition::Effect(ReceivingEffect::RefreshSession { refresh_id, .. }) =
        reducer.confirmation_succeeded(confirmation_id, partial_result(3))
    else {
        panic!("partial success should refresh");
    };
    assert_eq!(
        reducer.refresh_succeeded(
            refresh_id,
            session_with(10, 21, 30, vec![line(100, "ITEM-1", 10, 3, 0, 0)])
        ),
        ReceivingTransition::ReconciliationRequired(ReconciliationReason::RefreshAggregateMismatch)
    );

    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 2, 0, 0)]),
    );
    let (confirmation_id, _) = prepare_received(&mut reducer, 3);
    let result = ConfirmationResult {
        cumulative_received: quantity(5),
        remaining: quantity(5),
        ..partial_result(3)
    };
    let ReceivingTransition::Effect(ReceivingEffect::RefreshSession { refresh_id, .. }) =
        reducer.confirmation_succeeded(confirmation_id, result)
    else {
        panic!("partial success should refresh");
    };
    assert_eq!(
        reducer.refresh_succeeded(refresh_id, session(vec![line(100, "ITEM-1", 10, 4, 0, 0)])),
        ReceivingTransition::ReconciliationRequired(ReconciliationReason::RefreshQuantityRegressed)
    );
}

#[test]
fn stale_confirmation_callbacks_are_ignored() {
    let mut reducer = ExpectedReceivingReducer::default();
    resolve(
        &mut reducer,
        session(vec![line(100, "ITEM-1", 10, 0, 0, 0)]),
    );
    let (confirmation_id, _) = prepare_received(&mut reducer, 3);
    assert_eq!(
        reducer.confirmation_succeeded(ConfirmationId(999), partial_result(3)),
        ReceivingTransition::Ignored
    );
    assert_eq!(reducer.activity(), ReceivingActivity::ConfirmationPending);
    assert_eq!(
        reducer.confirmation_failed(confirmation_id, ConfirmationFailure::CommandStillPending),
        ReceivingTransition::Applied
    );
    assert_eq!(reducer.activity(), ReceivingActivity::ConfirmationPending);
}

#[test]
fn durable_intent_validation_rejects_unknown_schema_and_incomplete_other_reason() {
    let valid = ConfirmationIntent::try_new(
        recovery_snapshot(line(100, "ITEM-1", 10, 0, 0, 0)),
        ExpectedReceiptCommand::Missing {
            quantity: positive(1),
            reason: ReceiptExceptionReason::Other,
            note: Some(ExceptionNote::new("Not listed").unwrap()),
        },
    )
    .unwrap();
    assert!(valid.is_current_and_valid());

    let mut wrong_schema = valid.clone();
    wrong_schema.schema_version += 1;
    assert!(!wrong_schema.is_current_and_valid());

    let missing_note = ConfirmationIntent::try_new(
        valid.recovery.as_ref().clone(),
        ExpectedReceiptCommand::Missing {
            quantity: positive(1),
            reason: ReceiptExceptionReason::Other,
            note: None,
        },
    );
    assert_eq!(
        missing_note,
        Err(ReceivingValidationError::InvalidConfirmationIntent)
    );

    let mut unknown_field = serde_json::to_value(valid).unwrap();
    unknown_field["tenant_id"] = json!(99);
    assert!(serde_json::from_value::<ConfirmationIntent>(unknown_field).is_err());
}
