use wareboxes_api_contract::v1::{PackAllocationDispositionResponse, PackSessionResponse};

use super::{
    IdentityCandidate, IdentityScanError, IdentityScanStage, ItemIdentityResolution,
    PendingItemIdentity, ResolvedItemIdentity,
};

pub(super) fn matching_item_candidates(
    session: &PackSessionResponse,
    source_barcode: &str,
    item_barcode: &str,
) -> Vec<IdentityCandidate> {
    session
        .allocations
        .iter()
        .filter(|allocation| {
            matches!(
                allocation.disposition,
                PackAllocationDispositionResponse::Available
            ) && allocation.license_plate_barcode == source_barcode
                && allocation
                    .item_barcodes
                    .iter()
                    .any(|barcode| barcode == item_barcode)
        })
        .map(|allocation| IdentityCandidate {
            allocation_id: allocation.inventory_allocation_id,
            lot: allocation.lot.clone(),
            serial: allocation.serial.clone(),
        })
        .collect()
}

pub(super) fn start_item_identity(
    item_barcode: String,
    candidates: Vec<IdentityCandidate>,
) -> Result<ItemIdentityResolution, IdentityScanError> {
    if candidates.iter().any(|candidate| candidate.lot.is_some()) {
        return Ok(ItemIdentityResolution::Await(PendingItemIdentity {
            item_barcode,
            candidates,
            lot_scan: None,
            stage: IdentityScanStage::Lot,
        }));
    }
    resolve_or_request_serial(item_barcode, candidates, None)
}

pub(super) fn advance_item_identity(
    pending: &PendingItemIdentity,
    scan: &str,
) -> Result<ItemIdentityResolution, IdentityScanError> {
    match pending.stage {
        IdentityScanStage::Lot => {
            let candidates = pending
                .candidates
                .iter()
                .filter(|candidate| candidate.lot.as_deref() == Some(scan))
                .cloned()
                .collect::<Vec<_>>();
            if candidates.is_empty() {
                return Err(IdentityScanError {
                    message: "That lot does not match the scanned item.",
                    reset: false,
                });
            }
            resolve_or_request_serial(
                pending.item_barcode.clone(),
                candidates,
                Some(scan.to_owned()),
            )
        }
        IdentityScanStage::Serial => {
            let candidates = pending
                .candidates
                .iter()
                .filter(|candidate| candidate.serial.as_deref() == Some(scan))
                .collect::<Vec<_>>();
            match candidates.as_slice() {
                [] => Err(IdentityScanError {
                    message: "That serial does not match the scanned item.",
                    reset: false,
                }),
                [candidate] => Ok(ItemIdentityResolution::Resolved(ResolvedItemIdentity {
                    allocation_id: candidate.allocation_id,
                    item_barcode: pending.item_barcode.clone(),
                    lot_scan: pending.lot_scan.clone(),
                    serial_scan: Some(scan.to_owned()),
                })),
                _ => Err(IdentityScanError {
                    message: "That serial matches multiple allocations; packing cannot continue.",
                    reset: true,
                }),
            }
        }
    }
}

fn resolve_or_request_serial(
    item_barcode: String,
    candidates: Vec<IdentityCandidate>,
    lot_scan: Option<String>,
) -> Result<ItemIdentityResolution, IdentityScanError> {
    if candidates
        .iter()
        .any(|candidate| candidate.serial.is_some())
    {
        return Ok(ItemIdentityResolution::Await(PendingItemIdentity {
            item_barcode,
            candidates,
            lot_scan,
            stage: IdentityScanStage::Serial,
        }));
    }
    match candidates.as_slice() {
        [candidate] => Ok(ItemIdentityResolution::Resolved(ResolvedItemIdentity {
            allocation_id: candidate.allocation_id,
            item_barcode,
            lot_scan,
            serial_scan: None,
        })),
        _ => Err(IdentityScanError {
            message: "That item matches multiple allocations without a scannable identity.",
            reset: true,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::{advance_item_identity, start_item_identity};
    use crate::packing::{IdentityCandidate, IdentityScanStage, ItemIdentityResolution};

    #[test]
    fn uncontrolled_item_resolves_only_when_one_allocation_matches() {
        let resolution = start_item_identity(
            "ITEM-1".to_owned(),
            vec![IdentityCandidate {
                allocation_id: 41,
                lot: None,
                serial: None,
            }],
        )
        .unwrap();

        let ItemIdentityResolution::Resolved(identity) = resolution else {
            panic!("one uncontrolled allocation should resolve immediately");
        };
        assert_eq!(identity.allocation_id, 41);
        assert_eq!(identity.item_barcode, "ITEM-1");
        assert_eq!(identity.lot_scan, None);
        assert_eq!(identity.serial_scan, None);
    }

    #[test]
    fn lot_and_serial_are_required_as_separate_scans() {
        let resolution = start_item_identity(
            "ITEM-CONTROLLED".to_owned(),
            vec![IdentityCandidate {
                allocation_id: 52,
                lot: Some("LOT-52".to_owned()),
                serial: Some("SERIAL-52".to_owned()),
            }],
        )
        .unwrap();
        let ItemIdentityResolution::Await(lot_pending) = resolution else {
            panic!("lot-controlled stock should request a lot scan");
        };
        assert_eq!(lot_pending.stage, IdentityScanStage::Lot);
        assert_eq!(lot_pending.lot_scan, None);

        let resolution = advance_item_identity(&lot_pending, "LOT-52").unwrap();
        let ItemIdentityResolution::Await(serial_pending) = resolution else {
            panic!("serialized stock should request a serial scan after lot");
        };
        assert_eq!(serial_pending.stage, IdentityScanStage::Serial);
        assert_eq!(serial_pending.lot_scan.as_deref(), Some("LOT-52"));

        let resolution = advance_item_identity(&serial_pending, "SERIAL-52").unwrap();
        let ItemIdentityResolution::Resolved(identity) = resolution else {
            panic!("matching serial should resolve the allocation");
        };
        assert_eq!(identity.allocation_id, 52);
        assert_eq!(identity.lot_scan.as_deref(), Some("LOT-52"));
        assert_eq!(identity.serial_scan.as_deref(), Some("SERIAL-52"));
    }

    #[test]
    fn wrong_identity_scan_retains_stage_but_unsafe_ambiguity_resets_it() {
        let resolution = start_item_identity(
            "ITEM-LOT".to_owned(),
            vec![
                IdentityCandidate {
                    allocation_id: 61,
                    lot: Some("LOT-A".to_owned()),
                    serial: None,
                },
                IdentityCandidate {
                    allocation_id: 62,
                    lot: Some("LOT-A".to_owned()),
                    serial: None,
                },
            ],
        )
        .unwrap();
        let ItemIdentityResolution::Await(pending) = resolution else {
            panic!("lot-controlled stock should request identity");
        };

        let wrong_lot = advance_item_identity(&pending, "LOT-WRONG").unwrap_err();
        assert!(!wrong_lot.reset);

        let ambiguous = advance_item_identity(&pending, "LOT-A").unwrap_err();
        assert!(ambiguous.reset);
    }
}
