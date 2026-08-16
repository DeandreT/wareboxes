use super::{PickClaim, PickExecutionMethod, PickingWorkflow};

#[derive(Debug, Clone)]
pub(super) struct BatchScanEvidence {
    pub(super) cluster_id: i64,
    pub(super) source_inventory_balance_id: i64,
    pub(super) item_batch_id: i64,
    pub(super) source_location_barcode: Option<String>,
    pub(super) item_barcode: Option<String>,
    pub(super) source_license_plate_barcode: Option<String>,
}

impl PickingWorkflow {
    pub(super) fn retain_batch_scan_evidence(&mut self) {
        let Some(claim) = self.claim.as_ref() else {
            return;
        };
        let Some(cluster_id) = claim
            .execution
            .cluster_id
            .filter(|_| claim.execution.method == PickExecutionMethod::BatchCart)
        else {
            self.batch_scan_evidence = None;
            return;
        };
        self.batch_scan_evidence = Some(BatchScanEvidence {
            cluster_id,
            source_inventory_balance_id: claim.content.source_inventory_balance_id,
            item_batch_id: claim.content.item_batch_id,
            source_location_barcode: self
                .source_location_was_scanned
                .then(|| self.source_location_scan.clone())
                .flatten(),
            item_barcode: self
                .item_was_scanned
                .then(|| self.item_scan.clone())
                .flatten(),
            source_license_plate_barcode: self.source_license_plate_scan.clone(),
        });
    }

    pub(super) fn batch_evidence_matches(&self, claim: &PickClaim) -> bool {
        self.batch_scan_evidence
            .as_ref()
            .is_some_and(|evidence| batch_evidence_matches_claim(evidence, claim))
    }
}

pub(super) fn batch_evidence_matches_claim(
    evidence: &BatchScanEvidence,
    claim: &PickClaim,
) -> bool {
    claim.execution.method == PickExecutionMethod::BatchCart
        && claim.execution.cluster_id == Some(evidence.cluster_id)
        && claim.content.source_inventory_balance_id == evidence.source_inventory_balance_id
        && claim.content.item_batch_id == evidence.item_batch_id
}
