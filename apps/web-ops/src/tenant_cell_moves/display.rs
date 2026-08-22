use wareboxes_api_contract::v1::{
    Revision, TenantCellMoveAction, TenantCellMoveBlocker, TenantCellMoveEventAction,
    TenantCellMoveStatus,
};

pub(super) const fn current_placement_revision(
    status: TenantCellMoveStatus,
    source: Revision,
    cutover: Option<Revision>,
    rollback: Option<Revision>,
) -> Revision {
    match status {
        TenantCellMoveStatus::CutOver | TenantCellMoveStatus::Completed => match cutover {
            Some(revision) => revision,
            None => source,
        },
        TenantCellMoveStatus::RolledBack => match rollback {
            Some(revision) => revision,
            None => source,
        },
        TenantCellMoveStatus::Planned
        | TenantCellMoveStatus::Copying
        | TenantCellMoveStatus::Frozen
        | TenantCellMoveStatus::Validated
        | TenantCellMoveStatus::Cancelled => source,
    }
}

pub(super) const fn status_label(status: TenantCellMoveStatus) -> &'static str {
    match status {
        TenantCellMoveStatus::Planned => "Planned",
        TenantCellMoveStatus::Copying => "Copying",
        TenantCellMoveStatus::Frozen => "Writes frozen",
        TenantCellMoveStatus::Validated => "Validated",
        TenantCellMoveStatus::CutOver => "Cut over",
        TenantCellMoveStatus::Completed => "Completed",
        TenantCellMoveStatus::Cancelled => "Cancelled",
        TenantCellMoveStatus::RolledBack => "Rolled back",
    }
}

pub(super) const fn status_class(status: TenantCellMoveStatus) -> &'static str {
    match status {
        TenantCellMoveStatus::Planned => "status-badge info",
        TenantCellMoveStatus::Copying | TenantCellMoveStatus::Frozen => "status-badge warning",
        TenantCellMoveStatus::Validated | TenantCellMoveStatus::CutOver => "status-badge info",
        TenantCellMoveStatus::Completed => "status-badge success",
        TenantCellMoveStatus::Cancelled | TenantCellMoveStatus::RolledBack => {
            "status-badge neutral"
        }
    }
}

pub(super) const fn status_wire(status: Option<TenantCellMoveStatus>) -> &'static str {
    match status {
        None => "",
        Some(TenantCellMoveStatus::Planned) => "planned",
        Some(TenantCellMoveStatus::Copying) => "copying",
        Some(TenantCellMoveStatus::Frozen) => "frozen",
        Some(TenantCellMoveStatus::Validated) => "validated",
        Some(TenantCellMoveStatus::CutOver) => "cut_over",
        Some(TenantCellMoveStatus::Completed) => "completed",
        Some(TenantCellMoveStatus::Cancelled) => "cancelled",
        Some(TenantCellMoveStatus::RolledBack) => "rolled_back",
    }
}

pub(super) fn parse_status(value: &str) -> Option<TenantCellMoveStatus> {
    match value {
        "planned" => Some(TenantCellMoveStatus::Planned),
        "copying" => Some(TenantCellMoveStatus::Copying),
        "frozen" => Some(TenantCellMoveStatus::Frozen),
        "validated" => Some(TenantCellMoveStatus::Validated),
        "cut_over" => Some(TenantCellMoveStatus::CutOver),
        "completed" => Some(TenantCellMoveStatus::Completed),
        "cancelled" => Some(TenantCellMoveStatus::Cancelled),
        "rolled_back" => Some(TenantCellMoveStatus::RolledBack),
        _ => None,
    }
}

pub(super) const fn action_label(action: TenantCellMoveAction) -> &'static str {
    match action {
        TenantCellMoveAction::StartCopy => "Start copy",
        TenantCellMoveAction::Checkpoint => "Record checkpoint",
        TenantCellMoveAction::Freeze => "Freeze writes",
        TenantCellMoveAction::Validate => "Record validation",
        TenantCellMoveAction::Cutover => "Cut over",
        TenantCellMoveAction::VerifyCutover => "Verify cutover",
        TenantCellMoveAction::Complete => "Complete",
        TenantCellMoveAction::Rollback => "Roll back",
        TenantCellMoveAction::Cancel => "Cancel",
    }
}

pub(super) const fn event_label(action: TenantCellMoveEventAction) -> &'static str {
    match action {
        TenantCellMoveEventAction::Planned => "Move planned",
        TenantCellMoveEventAction::CopyStarted => "Copy started",
        TenantCellMoveEventAction::CheckpointRecorded => "Checkpoint recorded",
        TenantCellMoveEventAction::WritesFrozen => "Writes frozen",
        TenantCellMoveEventAction::Validated => "Validation recorded",
        TenantCellMoveEventAction::CutOver => "Tenant cut over",
        TenantCellMoveEventAction::PostCutoverVerified => "Cutover verified",
        TenantCellMoveEventAction::Completed => "Move completed",
        TenantCellMoveEventAction::RolledBack => "Move rolled back",
        TenantCellMoveEventAction::Cancelled => "Move cancelled",
    }
}

pub(super) const fn blocker_label(blocker: TenantCellMoveBlocker) -> &'static str {
    match blocker {
        TenantCellMoveBlocker::ActionNotAvailableInStatus => {
            "Action is unavailable in the current status"
        }
        TenantCellMoveBlocker::ActorTenantMustBeSwitched => {
            "Switch your active tenant away from the tenant being moved"
        }
        TenantCellMoveBlocker::SourcePlacementChanged => {
            "Tenant placement no longer matches this move"
        }
        TenantCellMoveBlocker::TargetNotActive => "Target cell is not active",
        TenantCellMoveBlocker::TargetCapacityUnavailable => "Target cell has no reserved capacity",
        TenantCellMoveBlocker::ResidencyMismatch => {
            "Target cell does not satisfy the tenant residency requirement"
        }
        TenantCellMoveBlocker::CopyReferenceMissing => "A copy reference has not been recorded",
        TenantCellMoveBlocker::CheckpointMissing => "No replication checkpoint is available",
        TenantCellMoveBlocker::WriteFenceMissing => "Tenant writes are not frozen",
        TenantCellMoveBlocker::ValidationMissing => "Validation evidence is missing",
        TenantCellMoveBlocker::ValidationStale => {
            "Validation is stale relative to the latest move evidence"
        }
        TenantCellMoveBlocker::PostCutoverVerificationMissing => {
            "Post-cutover verification evidence is missing"
        }
    }
}

pub(super) fn short_timestamp(value: &str) -> String {
    value
        .replace('T', " ")
        .trim_end_matches('Z')
        .chars()
        .take(19)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision(value: i64) -> Revision {
        Revision::new(value).unwrap()
    }

    #[test]
    fn current_placement_revision_tracks_cutover_and_rollback() {
        let source = revision(3);
        let cutover = revision(4);
        let rollback = revision(5);

        assert_eq!(
            current_placement_revision(
                TenantCellMoveStatus::Validated,
                source,
                Some(cutover),
                Some(rollback),
            ),
            source
        );
        assert_eq!(
            current_placement_revision(TenantCellMoveStatus::CutOver, source, Some(cutover), None,),
            cutover
        );
        assert_eq!(
            current_placement_revision(
                TenantCellMoveStatus::Completed,
                source,
                Some(cutover),
                None,
            ),
            cutover
        );
        assert_eq!(
            current_placement_revision(
                TenantCellMoveStatus::RolledBack,
                source,
                Some(cutover),
                Some(rollback),
            ),
            rollback
        );
        assert_eq!(
            current_placement_revision(TenantCellMoveStatus::CutOver, source, None, None),
            source
        );
        assert_eq!(
            current_placement_revision(TenantCellMoveStatus::RolledBack, source, None, None),
            source
        );
        assert_eq!(
            current_placement_revision(
                TenantCellMoveStatus::Cancelled,
                source,
                Some(cutover),
                Some(rollback),
            ),
            source
        );
    }

    #[test]
    fn placement_drift_blocker_label_applies_before_and_after_cutover() {
        assert_eq!(
            blocker_label(TenantCellMoveBlocker::SourcePlacementChanged),
            "Tenant placement no longer matches this move"
        );
    }
}
