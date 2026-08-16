use super::*;

fn claim(with_source_plate: bool) -> PickClaim {
    PickClaim {
        task_id: 41,
        order_id: 51,
        inventory_owner_id: 2,
        facility_id: 3,
        order_key: "SO-1051".into(),
        order_revision: 4,
        priority: 90,
        ship_by: Some("2026-08-09T20:00:00Z".into()),
        lease_expires_at: "2026-08-08T20:00:00Z".into(),
        destination_location_id: 9,
        destination_location_barcode: "STAGE-01".into(),
        destination_location_name: Some("Outbound stage 1".into()),
        execution: PickExecutionEvidence::discrete(),
        pick_policy: PickDecisionPolicy::product_default(),
        suggested_destination_license_plate_barcode: None,
        content: PickClaimContent {
            content_id: 61,
            order_line_id: 71,
            inventory_allocation_id: 81,
            source_inventory_balance_id: 91,
            item_batch_id: 101,
            source_location_id: 8,
            source_location_barcode: "A-01-02".into(),
            source_location_name: Some("Forward A-01-02".into()),
            source_license_plate_id: with_source_plate.then_some(12),
            source_license_plate_barcode: with_source_plate.then(|| "LP-SOURCE".into()),
            item_id: 111,
            item_description: Some("Case-picked filters".into()),
            item_barcodes: vec!["CASE-111".into(), "0012345678905".into()],
            uom: "case".into(),
            lot: Some("LOT-8".into()),
            serial: None,
            expiration: Some("2027-03-01T00:00:00Z".into()),
            planned_quantity: 4,
            state: PickContentState::Pending,
        },
    }
}

fn activate(workflow: &mut PickingWorkflow, claim: PickClaim) {
    let effect = workflow
        .begin_claim_next("claim-command".into(), "claim-key".into())
        .unwrap();
    assert!(matches!(effect, WorkflowEffect::PersistCommand(_)));
    assert!(matches!(
        workflow.command_persisted("claim-command", 7),
        Transition::Effect(WorkflowEffect::DispatchPersistedCommand { record_id: 7 })
    ));
    workflow.dispatch_started(7);
    workflow.durable_outcome_recorded(7, CommandOutcome::PickClaimed(Some(Box::new(claim))));
}

fn scan(workflow: &mut PickingWorkflow, value: &str) -> Option<WorkflowEffect> {
    *workflow.scan_draft_mut() = value.into();
    workflow.submit_scan("confirm-command".into(), "confirm-key".into())
}

#[test]
fn cluster_claim_requires_a_positive_scanned_route_id() {
    let mut workflow = PickingWorkflow::default();
    *workflow.cluster_id_draft_mut() = "bad-route".into();
    assert!(
        workflow
            .begin_cluster_claim("cluster-command".into(), "cluster-key".into())
            .is_none()
    );
    assert_eq!(
        workflow.error(),
        Some("Scan or enter a positive cluster route ID")
    );

    *workflow.cluster_id_draft_mut() = "44".into();
    let effect = workflow
        .begin_cluster_claim("cluster-command".into(), "cluster-key".into())
        .unwrap();
    let WorkflowEffect::PersistCommand(draft) = effect else {
        panic!("cluster claim should persist before dispatch");
    };
    assert_eq!(
        draft.command,
        RfCommand::Picking(PickingCommand::ClaimCluster { cluster_id: 44 })
    );
}

#[test]
fn loose_pick_enforces_source_item_and_destination_plate_sequence() {
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, claim(false));

    assert_eq!(
        workflow.expected_scan(),
        Some(PickScanStage::SourceLocation)
    );
    assert_eq!(scan(&mut workflow, "A-01-02"), None);
    assert_eq!(workflow.expected_scan(), Some(PickScanStage::Item));
    assert_eq!(scan(&mut workflow, "CASE-111"), None);
    assert_eq!(
        workflow.expected_scan(),
        Some(PickScanStage::DestinationLicensePlate)
    );

    let WorkflowEffect::PersistCommand(draft) = scan(&mut workflow, "LP-DEST").unwrap() else {
        panic!("destination plate must persist the pick confirmation");
    };
    assert!(matches!(
        draft.command,
        RfCommand::Picking(PickingCommand::Confirm {
            task_id: 41,
            content_id: 61,
            ref source_location_barcode,
            ref item_barcode,
            source_license_plate_barcode: None,
            ref destination_license_plate_barcode,
        }) if source_location_barcode.as_deref() == Some("A-01-02")
            && item_barcode.as_deref() == Some("CASE-111")
            && destination_license_plate_barcode.as_deref() == Some("LP-DEST")
    ));
}

#[test]
fn configured_optional_scans_skip_only_to_an_unambiguous_container() {
    let mut configured = claim(false);
    configured.pick_policy = PickDecisionPolicy {
        source: PickDecisionPolicySource::Configuration,
        configuration_id: Some(7),
        configuration_revision: Some(3),
        configuration_scope: Some(PickDecisionPolicyScope::OwnerFacility {
            inventory_owner_id: configured.inventory_owner_id,
            facility_id: configured.facility_id,
        }),
        require_source_location_scan: false,
        require_item_scan: false,
        require_destination_container_scan: false,
        policy_hash: "a".repeat(64),
    };
    configured.suggested_destination_license_plate_barcode = Some("TOTE-7".into());
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, configured);
    assert_eq!(workflow.expected_scan(), None);

    let WorkflowEffect::PersistCommand(draft) = workflow
        .begin_confirmation("pick-policy-confirm".into(), "pick-policy-key".into())
        .unwrap()
    else {
        panic!("policy-driven confirmation must be durable");
    };
    assert!(matches!(
        draft.command,
        RfCommand::Picking(PickingCommand::Confirm {
            source_location_barcode: None,
            item_barcode: None,
            destination_license_plate_barcode: None,
            ..
        })
    ));
}

#[test]
fn source_license_plate_is_required_and_cannot_be_reused_as_destination() {
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, claim(true));

    scan(&mut workflow, "A-01-02");
    scan(&mut workflow, "0012345678905");
    assert_eq!(
        workflow.expected_scan(),
        Some(PickScanStage::SourceLicensePlate)
    );
    scan(&mut workflow, "LP-SOURCE");
    assert_eq!(
        workflow.expected_scan(),
        Some(PickScanStage::DestinationLicensePlate)
    );
    assert_eq!(scan(&mut workflow, "LP-SOURCE"), None);
    assert_eq!(
        workflow.error(),
        Some("Destination license plate must differ from the source")
    );
    assert_eq!(
        workflow.expected_scan(),
        Some(PickScanStage::DestinationLicensePlate)
    );
}

#[test]
fn full_pallet_scan_confirms_the_same_physical_license_plate() {
    let mut pallet = claim(true);
    pallet.execution = PickExecutionEvidence::pallet();
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, pallet);

    scan(&mut workflow, "A-01-02");
    scan(&mut workflow, "0012345678905");
    let WorkflowEffect::PersistCommand(draft) = scan(&mut workflow, "LP-SOURCE").unwrap() else {
        panic!("pallet scan should persist one confirmation");
    };
    assert!(matches!(
        draft.command,
        RfCommand::Picking(PickingCommand::Confirm {
            source_license_plate_barcode: Some(ref source),
            destination_license_plate_barcode: Some(ref destination),
            ..
        }) if source == "LP-SOURCE" && destination == "LP-SOURCE"
    ));
    assert_eq!(workflow.expected_scan(), None);
}

#[test]
fn ambiguous_confirmation_retries_the_same_durable_record() {
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, claim(false));
    scan(&mut workflow, "A-01-02");
    scan(&mut workflow, "CASE-111");
    scan(&mut workflow, "LP-DEST");
    workflow.command_persisted("confirm-command", 17);
    workflow.dispatch_started(17);
    workflow.dispatch_ambiguous(17, "connection ended after send");

    assert_eq!(
        workflow.retry_ambiguous(),
        Some(WorkflowEffect::DispatchPersistedCommand { record_id: 17 })
    );
}

#[test]
fn mismatched_confirmation_result_requires_reconciliation() {
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, claim(false));
    scan(&mut workflow, "A-01-02");
    scan(&mut workflow, "CASE-111");
    scan(&mut workflow, "LP-DEST");
    workflow.command_persisted("confirm-command", 17);
    workflow.dispatch_started(17);
    workflow.durable_outcome_recorded(
        17,
        CommandOutcome::PickConfirmed {
            task_id: 999,
            content_id: 61,
            task_completed: true,
            order_ready_to_pack: false,
        },
    );

    assert_eq!(workflow.activity(), Activity::ReconcileRequired);
    assert!(workflow.claim().is_some());
}

#[test]
fn missing_inventory_reports_no_pick_after_exact_source_scans() {
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, claim(true));
    workflow.begin_shortage();

    assert_eq!(
        workflow.expected_scan(),
        Some(PickScanStage::SourceLocation)
    );
    assert_eq!(scan(&mut workflow, "A-01-02"), None);
    assert_eq!(
        workflow.expected_scan(),
        Some(PickScanStage::SourceLicensePlate)
    );
    assert_eq!(scan(&mut workflow, "LP-SOURCE"), None);
    assert_eq!(workflow.expected_scan(), None);
    assert_eq!(workflow.shortage_validation_message(), None);

    let WorkflowEffect::PersistCommand(draft) = workflow
        .begin_shortage_report("short-command".into(), "short-key".into())
        .unwrap()
    else {
        panic!("valid shortage must enter durable persistence");
    };
    let RfCommand::Picking(PickingCommand::ReportShortage(command)) = draft.command else {
        panic!("expected a shortage command");
    };
    assert_eq!(command.task_id, 41);
    assert_eq!(command.content_id, 61);
    assert_eq!(command.source_location_barcode, "A-01-02");
    assert_eq!(
        command.source_license_plate_barcode.as_deref(),
        Some("LP-SOURCE")
    );
    assert_eq!(command.observed_item_barcode, None);
    assert_eq!(command.reason, PickShortageReason::InventoryMissing);
    assert_eq!(command.outcome, PickShortageOutcome::NoPick);
}

#[test]
fn wrong_inventory_requires_a_nonmatching_observed_item() {
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, claim(false));
    workflow.begin_shortage();
    workflow.set_shortage_reason(PickShortageReason::WrongInventory);
    scan(&mut workflow, "A-01-02");

    assert_eq!(workflow.expected_scan(), Some(PickScanStage::ObservedItem));
    assert_eq!(scan(&mut workflow, "CASE-111"), None);
    assert_eq!(
        workflow.error(),
        Some("Observed item matches the directed item")
    );
    assert_eq!(scan(&mut workflow, "CASE-999"), None);
    assert_eq!(workflow.shortage_validation_message(), None);
}

#[test]
fn changing_controlled_evidence_discards_prior_lot_and_serial_scans() {
    let mut controlled_claim = claim(false);
    controlled_claim.content.serial = Some("SERIAL-8".into());
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, controlled_claim);
    workflow.begin_shortage();
    workflow.set_shortage_reason(PickShortageReason::LotOrSerialMismatch);
    scan(&mut workflow, "A-01-02");
    scan(&mut workflow, "CASE-111");
    scan(&mut workflow, "WRONG-LOT");

    assert_eq!(
        workflow
            .shortage()
            .and_then(PickShortageDraft::observed_lot),
        Some("WRONG-LOT")
    );
    workflow.set_controlled_evidence(PickControlledEvidence::Serial);

    let shortage = workflow.shortage().unwrap();
    assert_eq!(shortage.observed_lot(), None);
    assert_eq!(shortage.observed_serial(), None);
    assert_eq!(
        workflow.expected_scan(),
        Some(PickScanStage::ObservedSerial)
    );
}

#[test]
fn partial_shortage_scans_matching_controlled_stock_and_destination() {
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, claim(false));
    workflow.begin_shortage();
    workflow.set_shortage_reason(PickShortageReason::InsufficientQuantity);
    workflow.set_shortage_disposition(PickShortageDisposition::Partial);
    scan(&mut workflow, "A-01-02");
    scan(&mut workflow, "CASE-111");
    assert_eq!(workflow.expected_scan(), Some(PickScanStage::ObservedLot));
    scan(&mut workflow, "LOT-8");
    assert_eq!(
        workflow.expected_scan(),
        Some(PickScanStage::ShortageDestinationLicensePlate)
    );
    scan(&mut workflow, "TOTE-2");
    workflow
        .shortage_mut()
        .unwrap()
        .picked_quantity_mut()
        .push('2');

    assert_eq!(workflow.shortage_validation_message(), None);
    let WorkflowEffect::PersistCommand(draft) = workflow
        .begin_shortage_report("short-command".into(), "short-key".into())
        .unwrap()
    else {
        panic!("partial shortage must persist");
    };
    let RfCommand::Picking(PickingCommand::ReportShortage(command)) = draft.command else {
        panic!("expected a shortage command");
    };
    assert_eq!(command.observed_item_barcode.as_deref(), Some("CASE-111"));
    assert_eq!(command.observed_lot.as_deref(), Some("LOT-8"));
    assert_eq!(
        command.outcome,
        PickShortageOutcome::Partial {
            picked_quantity: 2,
            destination_license_plate_barcode: "TOTE-2".into(),
        }
    );
}

#[test]
fn other_shortage_requires_a_bounded_note() {
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, claim(false));
    workflow.begin_shortage();
    workflow.set_shortage_reason(PickShortageReason::Other);
    scan(&mut workflow, "A-01-02");

    assert_eq!(
        workflow.shortage_validation_message(),
        Some("Add a note for Other")
    );
    workflow
        .shortage_mut()
        .unwrap()
        .note_mut()
        .push_str("Picker found mixed stock");
    assert_eq!(workflow.shortage_validation_message(), None);
}

#[test]
fn shortage_result_must_match_saved_evidence_before_claim_clears() {
    let mut workflow = PickingWorkflow::default();
    activate(&mut workflow, claim(false));
    workflow.begin_shortage();
    workflow.set_shortage_reason(PickShortageReason::WrongInventory);
    scan(&mut workflow, "A-01-02");
    scan(&mut workflow, "CASE-999");
    workflow.begin_shortage_report("short-command".into(), "short-key".into());
    workflow.command_persisted("short-command", 19);
    workflow.dispatch_started(19);

    workflow.durable_outcome_recorded(
        19,
        CommandOutcome::PickShortageReported(Box::new(PickShortageReportResult {
            shortage_id: 29,
            shortage_revision: 1,
            task_id: 41,
            content_id: 61,
            order_id: 51,
            order_revision: 5,
            planned_quantity: 4,
            picked_quantity: 0,
            short_quantity: 4,
            reason: PickShortageReason::WrongInventory,
            note: None,
            observed_item_barcode: Some("DIFFERENT-RESULT".into()),
            observed_lot: None,
            observed_serial: None,
            status: PickShortageStatus::AwaitingInventory,
        })),
    );

    assert_eq!(workflow.activity(), Activity::ReconcileRequired);
    assert!(workflow.claim().is_some());
}
