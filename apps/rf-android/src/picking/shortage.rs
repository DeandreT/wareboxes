use serde::{Deserialize, Serialize};

use super::{PickClaim, PickClaimContent, PickScanStage};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PickShortageReason {
    InventoryMissing,
    InsufficientQuantity,
    DamagedInventory,
    WrongInventory,
    LotOrSerialMismatch,
    Other,
}

impl PickShortageReason {
    pub const ALL: [Self; 6] = [
        Self::InventoryMissing,
        Self::InsufficientQuantity,
        Self::DamagedInventory,
        Self::WrongInventory,
        Self::LotOrSerialMismatch,
        Self::Other,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::InventoryMissing => "Inventory missing",
            Self::InsufficientQuantity => "Insufficient quantity",
            Self::DamagedInventory => "Damaged inventory",
            Self::WrongInventory => "Wrong inventory",
            Self::LotOrSerialMismatch => "Lot or serial mismatch",
            Self::Other => "Other",
        }
    }

    pub const fn supports_partial(self) -> bool {
        matches!(
            self,
            Self::InsufficientQuantity | Self::DamagedInventory | Self::Other
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickShortageDisposition {
    NoPick,
    Partial,
}

impl PickShortageDisposition {
    pub const fn label(self) -> &'static str {
        match self {
            Self::NoPick => "None picked",
            Self::Partial => "Partial pick",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickControlledEvidence {
    Lot,
    Serial,
}

impl PickControlledEvidence {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Lot => "Lot",
            Self::Serial => "Serial",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PickShortageOutcome {
    NoPick,
    Partial {
        picked_quantity: i64,
        destination_license_plate_barcode: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PickShortageCommand {
    pub task_id: i64,
    pub content_id: i64,
    pub source_location_barcode: String,
    pub source_license_plate_barcode: Option<String>,
    pub observed_item_barcode: Option<String>,
    pub observed_lot: Option<String>,
    pub observed_serial: Option<String>,
    pub reason: PickShortageReason,
    pub note: Option<String>,
    pub outcome: PickShortageOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickShortageDraft {
    pub(super) reason: PickShortageReason,
    pub(super) disposition: PickShortageDisposition,
    pub(super) controlled_evidence: Option<PickControlledEvidence>,
    pub(super) picked_quantity: String,
    pub(super) note: String,
    pub(super) observed_item_barcode: Option<String>,
    pub(super) observed_lot: Option<String>,
    pub(super) observed_serial: Option<String>,
    pub(super) destination_license_plate_barcode: Option<String>,
}

impl PickShortageDraft {
    pub const fn reason(&self) -> PickShortageReason {
        self.reason
    }

    pub const fn disposition(&self) -> PickShortageDisposition {
        self.disposition
    }

    pub const fn controlled_evidence(&self) -> Option<PickControlledEvidence> {
        self.controlled_evidence
    }

    pub fn picked_quantity_mut(&mut self) -> &mut String {
        &mut self.picked_quantity
    }

    pub fn note_mut(&mut self) -> &mut String {
        &mut self.note
    }

    pub fn observed_item_barcode(&self) -> Option<&str> {
        self.observed_item_barcode.as_deref()
    }

    pub fn observed_lot(&self) -> Option<&str> {
        self.observed_lot.as_deref()
    }

    pub fn observed_serial(&self) -> Option<&str> {
        self.observed_serial.as_deref()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickShortageStatus {
    AwaitingInventory,
    RecoveryInProgress,
    Resolved,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickShortageReportResult {
    pub shortage_id: i64,
    pub shortage_revision: i64,
    pub task_id: i64,
    pub content_id: i64,
    pub order_id: i64,
    pub order_revision: i64,
    pub planned_quantity: i64,
    pub picked_quantity: i64,
    pub short_quantity: i64,
    pub reason: PickShortageReason,
    pub note: Option<String>,
    pub observed_item_barcode: Option<String>,
    pub observed_lot: Option<String>,
    pub observed_serial: Option<String>,
    pub status: PickShortageStatus,
}

pub(super) fn expected_shortage_scan(
    content: &PickClaimContent,
    shortage: &PickShortageDraft,
    source_location_verified: bool,
    source_license_plate_verified: bool,
) -> Option<PickScanStage> {
    if !source_location_verified {
        return Some(PickScanStage::SourceLocation);
    }
    if content.source_license_plate_barcode.is_some() && !source_license_plate_verified {
        return Some(PickScanStage::SourceLicensePlate);
    }

    let item_required = shortage.disposition == PickShortageDisposition::Partial
        || matches!(
            shortage.reason,
            PickShortageReason::InsufficientQuantity
                | PickShortageReason::DamagedInventory
                | PickShortageReason::WrongInventory
                | PickShortageReason::LotOrSerialMismatch
        );
    if item_required && shortage.observed_item_barcode.is_none() {
        return Some(PickScanStage::ObservedItem);
    }

    if shortage.reason == PickShortageReason::LotOrSerialMismatch {
        return match shortage.controlled_evidence {
            Some(PickControlledEvidence::Lot) if shortage.observed_lot.is_none() => {
                Some(PickScanStage::ObservedLot)
            }
            Some(PickControlledEvidence::Serial) if shortage.observed_serial.is_none() => {
                Some(PickScanStage::ObservedSerial)
            }
            _ => None,
        };
    }

    if shortage.disposition == PickShortageDisposition::Partial {
        if content.lot.is_some() && shortage.observed_lot.is_none() {
            return Some(PickScanStage::ObservedLot);
        }
        if content.serial.is_some() && shortage.observed_serial.is_none() {
            return Some(PickScanStage::ObservedSerial);
        }
        if shortage.destination_license_plate_barcode.is_none() {
            return Some(PickScanStage::ShortageDestinationLicensePlate);
        }
    }
    None
}

pub(super) fn validate_shortage(
    claim: &PickClaim,
    shortage: &PickShortageDraft,
    source_location_scan: Option<&str>,
    source_license_plate_scan: Option<&str>,
) -> Result<(), &'static str> {
    let content = &claim.content;
    if source_location_scan != Some(content.source_location_barcode.as_str()) {
        return Err("Scan the directed source location");
    }
    if content.source_license_plate_barcode.as_deref() != source_license_plate_scan {
        return Err("Scan the directed source license plate");
    }
    for scan in [
        shortage.observed_item_barcode.as_deref(),
        shortage.observed_lot.as_deref(),
        shortage.observed_serial.as_deref(),
        shortage.destination_license_plate_barcode.as_deref(),
    ]
    .into_iter()
    .flatten()
    {
        if !valid_scan(scan) {
            return Err("Scan evidence is invalid");
        }
    }

    let note = shortage.note.trim();
    if note.chars().count() > 500 {
        return Err("Short-pick note cannot exceed 500 characters");
    }
    if shortage.reason == PickShortageReason::Other && note.is_empty() {
        return Err("Add a note for Other");
    }

    let observed_item_matches = shortage
        .observed_item_barcode
        .as_deref()
        .is_some_and(|observed| item_matches(content, observed));
    match shortage.reason {
        PickShortageReason::InventoryMissing => {}
        PickShortageReason::InsufficientQuantity | PickShortageReason::DamagedInventory => {
            if !observed_item_matches {
                return Err("Scan the directed item as evidence");
            }
        }
        PickShortageReason::WrongInventory => {
            let Some(observed) = shortage.observed_item_barcode.as_deref() else {
                return Err("Scan the item that was found");
            };
            if item_matches(content, observed) {
                return Err("Observed item must differ from the directed item");
            }
        }
        PickShortageReason::LotOrSerialMismatch => {
            if !observed_item_matches {
                return Err("Scan the directed item before its lot or serial");
            }
            let mismatch = match shortage.controlled_evidence {
                Some(PickControlledEvidence::Lot) => {
                    content.lot.as_deref().is_some_and(|directed| {
                        shortage
                            .observed_lot
                            .as_deref()
                            .is_some_and(|value| value != directed)
                    })
                }
                Some(PickControlledEvidence::Serial) => {
                    content.serial.as_deref().is_some_and(|directed| {
                        shortage
                            .observed_serial
                            .as_deref()
                            .is_some_and(|value| value != directed)
                    })
                }
                None => false,
            };
            if !mismatch {
                return Err("Scan a lot or serial that differs from the directed stock");
            }
        }
        PickShortageReason::Other => {}
    }

    if shortage.disposition == PickShortageDisposition::Partial {
        if !shortage.reason.supports_partial() {
            return Err("This reason cannot record a partial pick");
        }
        if !observed_item_matches {
            return Err("Scan the directed item for a partial pick");
        }
        if content.lot.as_deref() != shortage.observed_lot.as_deref() {
            return Err("Scan the directed lot for a partial pick");
        }
        if content.serial.as_deref() != shortage.observed_serial.as_deref() {
            return Err("Scan the directed serial for a partial pick");
        }
        let picked_quantity = shortage
            .picked_quantity
            .trim()
            .parse::<i64>()
            .map_err(|_| "Enter the quantity physically picked")?;
        if picked_quantity <= 0 || picked_quantity >= content.planned_quantity {
            return Err("Partial quantity must be between zero and planned quantity");
        }
        let Some(destination) = shortage.destination_license_plate_barcode.as_deref() else {
            return Err("Scan the partial-pick destination license plate");
        };
        if content.source_license_plate_barcode.as_deref() == Some(destination) {
            return Err("Destination license plate must differ from the source");
        }
    }
    Ok(())
}

fn item_matches(content: &PickClaimContent, observed: &str) -> bool {
    content
        .item_barcodes
        .iter()
        .any(|barcode| barcode == observed)
}

fn valid_scan(value: &str) -> bool {
    !value.is_empty()
        && value.trim() == value
        && value.chars().count() <= 200
        && !value.chars().any(char::is_control)
}
