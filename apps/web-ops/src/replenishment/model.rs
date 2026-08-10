use leptos::prelude::*;
use wareboxes_api_contract::v1::{
    ConfigureReplenishmentPolicyRequest, ConfigureReplenishmentPolicyResponse, OpaqueCursor,
    PlanReplenishmentRequest, PlanReplenishmentResponse, ReplenishmentPlanningOutcome,
    ReplenishmentPolicyPage, ReplenishmentPolicyReadinessEntryResponse, ReplenishmentQueuePage,
    ReplenishmentReserveSourceLocationIds, ReplenishmentWorkStatus,
    RetireReplenishmentPolicyRequest, RetireReplenishmentPolicyResponse,
};
use wareboxes_api_contract::web::access::AccessScopeWorkspace;
use wareboxes_core::models::{Item, Location};

use crate::sorting::{SortDirection, SortSpec};
use crate::toast::ToastBus;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum ReplenishmentTab {
    Policies,
    Work,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkSort {
    Created,
    Priority,
    Client,
    Facility,
    Item,
    Source,
    Destination,
    Quantity,
    Status,
    Lease,
}

#[derive(Clone, Copy)]
pub(super) struct ScopeFilters {
    pub facility_id: RwSignal<String>,
    pub inventory_owner_id: RwSignal<String>,
    pub item_id: RwSignal<String>,
    pub pick_face_location_id: RwSignal<String>,
    pub work_status: RwSignal<String>,
}

impl ScopeFilters {
    pub fn new() -> Self {
        Self {
            facility_id: RwSignal::new(String::new()),
            inventory_owner_id: RwSignal::new(String::new()),
            item_id: RwSignal::new(String::new()),
            pick_face_location_id: RwSignal::new(String::new()),
            work_status: RwSignal::new(String::new()),
        }
    }

    pub fn validate(self) -> Result<(), String> {
        for (label, value) in [
            ("Facility", self.facility_id.get_untracked()),
            ("Client", self.inventory_owner_id.get_untracked()),
            ("Item", self.item_id.get_untracked()),
            ("Pick face", self.pick_face_location_id.get_untracked()),
        ] {
            validate_optional_id(&value, label)?;
        }
        Ok(())
    }

    pub fn facility(self) -> Option<i64> {
        parse_optional_id(&self.facility_id.get_untracked())
    }

    pub fn owner(self) -> Option<i64> {
        parse_optional_id(&self.inventory_owner_id.get_untracked())
    }

    pub fn item(self) -> Option<i64> {
        parse_optional_id(&self.item_id.get_untracked())
    }

    pub fn pick_face(self) -> Option<i64> {
        parse_optional_id(&self.pick_face_location_id.get_untracked())
    }

    pub fn status(self) -> Option<ReplenishmentWorkStatus> {
        match self.work_status.get_untracked().as_str() {
            "pending" => Some(ReplenishmentWorkStatus::Pending),
            "claimed" => Some(ReplenishmentWorkStatus::Claimed),
            "completed" => Some(ReplenishmentWorkStatus::Completed),
            "cancelled" => Some(ReplenishmentWorkStatus::Cancelled),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct PolicyPageSignals {
    pub page: RwSignal<Option<ReplenishmentPolicyPage>>,
    pub current_cursor: RwSignal<Option<OpaqueCursor>>,
    pub cursor_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    pub loading: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub generation: RwSignal<u64>,
}

impl PolicyPageSignals {
    pub fn new(initial: Option<ReplenishmentPolicyPage>) -> Self {
        Self {
            page: RwSignal::new(initial),
            current_cursor: RwSignal::new(None),
            cursor_history: RwSignal::new(Vec::new()),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            generation: RwSignal::new(0),
        }
    }
}

#[derive(Clone, Copy)]
pub(super) struct WorkPageSignals {
    pub page: RwSignal<Option<ReplenishmentQueuePage>>,
    pub current_cursor: RwSignal<Option<OpaqueCursor>>,
    pub cursor_history: RwSignal<Vec<Option<OpaqueCursor>>>,
    pub loading: RwSignal<bool>,
    pub error: RwSignal<Option<String>>,
    pub generation: RwSignal<u64>,
    pub sort: RwSignal<SortSpec<WorkSort>>,
}

impl WorkPageSignals {
    pub fn new(initial: Option<ReplenishmentQueuePage>) -> Self {
        Self {
            page: RwSignal::new(initial),
            current_cursor: RwSignal::new(None),
            cursor_history: RwSignal::new(Vec::new()),
            loading: RwSignal::new(false),
            error: RwSignal::new(None),
            generation: RwSignal::new(0),
            sort: RwSignal::new(SortSpec {
                key: WorkSort::Priority,
                direction: SortDirection::Descending,
            }),
        }
    }
}

#[derive(Clone, Default)]
pub(super) struct ReplenishmentReferenceData {
    pub access: AccessScopeWorkspace,
    pub items: Vec<Item>,
    pub locations: Vec<Location>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PolicyDialogMode {
    Configure(Option<ReplenishmentPolicyReadinessEntryResponse>),
    Plan(ReplenishmentPolicyReadinessEntryResponse),
    Retire(ReplenishmentPolicyReadinessEntryResponse),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PolicyCommandAttempt {
    Configure {
        request: ConfigureReplenishmentPolicyRequest,
        idempotency_key: String,
    },
    Plan {
        policy_id: i64,
        request: PlanReplenishmentRequest,
        idempotency_key: String,
    },
    Retire {
        policy_id: i64,
        request: RetireReplenishmentPolicyRequest,
        idempotency_key: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum PolicyCommandResult {
    Configured(ConfigureReplenishmentPolicyResponse),
    Planned(PlanReplenishmentResponse),
    Retired(RetireReplenishmentPolicyResponse),
}

#[derive(Clone, Copy)]
pub(super) struct CommandSignals {
    pub dialog: RwSignal<Option<PolicyDialogMode>>,
    pub pending: RwSignal<bool>,
    pub retry: RwSignal<Option<PolicyCommandAttempt>>,
    pub error: RwSignal<Option<String>>,
    pub invalidated: RwSignal<bool>,
    pub toasts: ToastBus,
    pub on_unauthorized: Callback<()>,
    pub on_authoritative_refresh: Callback<()>,
}

pub(super) fn parse_optional_id(value: &str) -> Option<i64> {
    value.trim().parse::<i64>().ok().filter(|value| *value > 0)
}

pub(super) fn validate_optional_id(value: &str, label: &str) -> Result<(), String> {
    if value.trim().is_empty() || parse_optional_id(value).is_some() {
        Ok(())
    } else {
        Err(format!("{label} ID must be a positive whole number."))
    }
}

pub(super) const fn planning_outcome_label(outcome: ReplenishmentPlanningOutcome) -> &'static str {
    match outcome {
        ReplenishmentPlanningOutcome::NotNeeded => "No move",
        ReplenishmentPlanningOutcome::InsufficientReserve => "No reserve",
        ReplenishmentPlanningOutcome::PartiallyPlanned => "Partial",
        ReplenishmentPlanningOutcome::FullyPlanned => "Full",
    }
}

pub(super) const fn planning_outcome_class(outcome: ReplenishmentPlanningOutcome) -> &'static str {
    match outcome {
        ReplenishmentPlanningOutcome::NotNeeded => "status shipped",
        ReplenishmentPlanningOutcome::InsufficientReserve => "status held",
        ReplenishmentPlanningOutcome::PartiallyPlanned => "status processing",
        ReplenishmentPlanningOutcome::FullyPlanned => "status open",
    }
}

pub(super) const fn work_status_label(status: ReplenishmentWorkStatus) -> &'static str {
    match status {
        ReplenishmentWorkStatus::Pending => "Pending",
        ReplenishmentWorkStatus::Claimed => "Claimed",
        ReplenishmentWorkStatus::Completed => "Completed",
        ReplenishmentWorkStatus::Cancelled => "Cancelled",
    }
}

pub(super) const fn work_status_class(status: ReplenishmentWorkStatus) -> &'static str {
    match status {
        ReplenishmentWorkStatus::Pending => "status open",
        ReplenishmentWorkStatus::Claimed => "status processing",
        ReplenishmentWorkStatus::Completed => "status shipped",
        ReplenishmentWorkStatus::Cancelled => "status muted",
    }
}

pub(super) fn compact_timestamp(value: &str) -> String {
    value.get(..16).unwrap_or(value).replace('T', " ")
}

pub(super) fn location_label(location: &Location) -> String {
    let identity = location
        .name
        .as_deref()
        .or(location.barcode.as_deref())
        .map_or_else(|| format!("Location #{}", location.id), str::to_owned);
    match (location.name.as_deref(), location.barcode.as_deref()) {
        (Some(_), Some(barcode)) => format!("{identity} / {barcode}"),
        _ => identity,
    }
}

pub(super) fn item_label(item: &Item) -> String {
    let sku = item.skus.first().map(|sku| sku.name.as_str());
    match (item.description.as_deref(), sku) {
        (Some(description), Some(sku)) => format!("{description} / {sku}"),
        (Some(description), None) => description.to_owned(),
        (None, Some(sku)) => sku.to_owned(),
        (None, None) => format!("Item #{}", item.id),
    }
}

pub(super) struct PolicyRequestInput<'a> {
    pub owner: &'a str,
    pub facility: &'a str,
    pub item: &'a str,
    pub uom: &'a str,
    pub pick_face: &'a str,
    pub minimum: &'a str,
    pub target: &'a str,
    pub reserve_sources: Vec<i64>,
    pub expected_revision: Option<wareboxes_api_contract::v1::Revision>,
}

pub(super) fn build_policy_request(
    input: PolicyRequestInput<'_>,
) -> Result<ConfigureReplenishmentPolicyRequest, String> {
    let inventory_owner_id = required_id(input.owner, "Client")?;
    let facility_id = required_id(input.facility, "Facility")?;
    let item_id = required_id(input.item, "Item")?;
    let pick_face_location_id = required_id(input.pick_face, "Pick face")?;
    let uom = input.uom.trim();
    if uom.is_empty() || uom.chars().count() > 32 || uom.chars().any(char::is_control) {
        return Err("UOM is required and cannot exceed 32 characters.".to_owned());
    }
    let minimum_quantity = nonnegative_quantity(input.minimum, "Minimum")?;
    let target_quantity = nonnegative_quantity(input.target, "Target")?;
    if target_quantity <= minimum_quantity {
        return Err("Target must be greater than minimum.".to_owned());
    }
    let reserve_source_location_ids =
        ReplenishmentReserveSourceLocationIds::new(input.reserve_sources)
            .map_err(|_| "Select at least one reserve source location.".to_owned())?;
    if reserve_source_location_ids
        .as_slice()
        .contains(&pick_face_location_id)
    {
        return Err("The pick face cannot also be a reserve source.".to_owned());
    }
    Ok(ConfigureReplenishmentPolicyRequest {
        inventory_owner_id,
        facility_id,
        item_id,
        uom: uom.to_owned(),
        pick_face_location_id,
        minimum_quantity,
        target_quantity,
        reserve_source_location_ids,
        expected_revision: input.expected_revision,
    })
}

fn required_id(value: &str, label: &str) -> Result<i64, String> {
    parse_optional_id(value).ok_or_else(|| format!("Select a valid {label}."))
}

fn nonnegative_quantity(value: &str, label: &str) -> Result<i64, String> {
    value
        .trim()
        .parse::<i64>()
        .ok()
        .filter(|value| *value >= 0)
        .ok_or_else(|| format!("{label} must be a nonnegative whole number."))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::Revision;

    #[test]
    fn policy_request_normalizes_sources_and_keeps_revision() {
        let request = build_policy_request(PolicyRequestInput {
            owner: " 2 ",
            facility: "3",
            item: "4",
            uom: " each ",
            pick_face: "10",
            minimum: "5",
            target: "20",
            reserve_sources: vec![12, 11, 12],
            expected_revision: Some(Revision::new(7).unwrap()),
        })
        .unwrap();

        assert_eq!(request.uom, "each");
        assert_eq!(request.reserve_source_location_ids.as_slice(), &[11, 12]);
        assert_eq!(request.expected_revision.unwrap().get(), 7);
    }

    #[test]
    fn policy_request_rejects_invalid_thresholds_and_source_overlap() {
        let invalid_thresholds = PolicyRequestInput {
            owner: "2",
            facility: "3",
            item: "4",
            uom: "each",
            pick_face: "10",
            minimum: "5",
            target: "5",
            reserve_sources: vec![11],
            expected_revision: None,
        };
        assert!(build_policy_request(invalid_thresholds).is_err());
        let overlapping_source = PolicyRequestInput {
            owner: "2",
            facility: "3",
            item: "4",
            uom: "each",
            pick_face: "10",
            minimum: "5",
            target: "20",
            reserve_sources: vec![10],
            expected_revision: None,
        };
        assert!(build_policy_request(overlapping_source).is_err());
    }

    #[test]
    fn status_labels_cover_all_server_outcomes() {
        assert_eq!(
            planning_outcome_label(ReplenishmentPlanningOutcome::NotNeeded),
            "No move"
        );
        assert_eq!(
            planning_outcome_label(ReplenishmentPlanningOutcome::InsufficientReserve),
            "No reserve"
        );
        assert_eq!(
            work_status_label(ReplenishmentWorkStatus::Claimed),
            "Claimed"
        );
    }
}
