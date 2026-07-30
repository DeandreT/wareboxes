use super::{
    ConfirmationMode, ContainerCapture, ReceiptExceptionReason, ReceivingDraftSnapshot,
    ReceivingUiState, work_mode_switch_allowed,
};
use crate::expected_receiving::ReceivingActivity;
use crate::workflow::Activity;

fn draft(reason: Option<ReceiptExceptionReason>, note: Option<&str>) -> ReceivingDraftSnapshot {
    ReceivingDraftSnapshot {
        mode: ConfirmationMode::Rejected,
        selected_line_id: None,
        item_barcode: Some("CASE-100".into()),
        dock_barcode: None,
        quantity: Some(2),
        container: ContainerCapture::Loose,
        license_plate_barcode: None,
        reason,
        note: note.map(str::to_owned),
    }
}

#[test]
fn work_mode_changes_only_without_owned_work() {
    assert!(work_mode_switch_allowed(
        Activity::Idle,
        ReceivingActivity::AwaitingLoad,
        Activity::Idle
    ));
    assert!(work_mode_switch_allowed(
        Activity::Idle,
        ReceivingActivity::LoadComplete,
        Activity::Idle
    ));
    assert!(!work_mode_switch_allowed(
        Activity::Active,
        ReceivingActivity::AwaitingLoad,
        Activity::Idle
    ));
    assert!(!work_mode_switch_allowed(
        Activity::Idle,
        ReceivingActivity::Active,
        Activity::Idle
    ));
    assert!(!work_mode_switch_allowed(
        Activity::Idle,
        ReceivingActivity::ConfirmationPending,
        Activity::Idle
    ));
    assert!(!work_mode_switch_allowed(
        Activity::Idle,
        ReceivingActivity::AwaitingLoad,
        Activity::Active
    ));
}

#[test]
fn invalid_displayed_note_or_quantity_never_matches_saved_draft() {
    let snapshot = draft(Some(ReceiptExceptionReason::Other), Some("Seal broken"));
    let mut ui = ReceivingUiState::default();
    ui.sync_from_snapshot(Some(snapshot.clone()));
    assert!(ui.displayed_confirmation_matches(&snapshot));

    ui.note_draft.push(' ');
    assert!(!ui.displayed_confirmation_matches(&snapshot));

    ui.note_draft = "Seal broken".into();
    ui.quantity_draft = "999".into();
    assert!(!ui.displayed_confirmation_matches(&snapshot));
}

#[test]
fn changing_away_from_other_clears_the_displayed_note() {
    let mut ui = ReceivingUiState::default();
    ui.sync_from_snapshot(Some(draft(
        Some(ReceiptExceptionReason::Other),
        Some("Seal broken"),
    )));
    assert_eq!(ui.note_draft, "Seal broken");

    ui.sync_from_snapshot(Some(draft(Some(ReceiptExceptionReason::Damaged), None)));
    assert!(ui.note_draft.is_empty());

    let other_without_note = draft(Some(ReceiptExceptionReason::Other), None);
    ui.sync_from_snapshot(Some(other_without_note.clone()));
    assert!(ui.note_draft.is_empty());
    assert!(!ui.displayed_confirmation_matches(&other_without_note));
}
