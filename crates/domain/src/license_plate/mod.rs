//! License-plate/container hierarchy policies.

pub const MAX_LICENSE_PLATE_HIERARCHY_DEPTH: u8 = 8;
pub const MAX_LICENSE_PLATE_HIERARCHY_NODES: u32 = 1_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LicensePlateAttachmentSnapshot {
    pub child_id: i64,
    pub parent_id: i64,
    pub child_has_parent: bool,
    pub child_deleted: bool,
    pub parent_deleted: bool,
    pub same_inventory_owner: bool,
    pub same_facility: bool,
    pub same_location: bool,
    pub parent_chain_contains_child: bool,
    pub parent_depth: u8,
    pub child_subtree_height: u8,
    pub parent_tree_size: u32,
    pub child_tree_size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum LicensePlateAttachmentError {
    #[error("a license plate cannot contain itself")]
    SelfAttachment,
    #[error("the child license plate is already nested; detach it first")]
    ChildAlreadyNested,
    #[error("inactive license plates cannot be nested")]
    InactiveLicensePlate,
    #[error("nested license plates must belong to the same inventory owner")]
    InventoryOwnerMismatch,
    #[error("nested license plates must belong to the same facility")]
    FacilityMismatch,
    #[error("nested license plates must occupy the same location")]
    LocationMismatch,
    #[error("the requested relationship would create a hierarchy cycle")]
    Cycle,
    #[error("license plate hierarchy cannot exceed {MAX_LICENSE_PLATE_HIERARCHY_DEPTH} levels")]
    MaximumDepthExceeded,
    #[error(
        "license plate hierarchy cannot exceed {MAX_LICENSE_PLATE_HIERARCHY_NODES} containers"
    )]
    MaximumNodesExceeded,
}

pub fn validate_license_plate_attachment(
    snapshot: LicensePlateAttachmentSnapshot,
) -> Result<(), LicensePlateAttachmentError> {
    if snapshot.child_id == snapshot.parent_id {
        return Err(LicensePlateAttachmentError::SelfAttachment);
    }
    if snapshot.child_has_parent {
        return Err(LicensePlateAttachmentError::ChildAlreadyNested);
    }
    if snapshot.child_deleted || snapshot.parent_deleted {
        return Err(LicensePlateAttachmentError::InactiveLicensePlate);
    }
    if !snapshot.same_inventory_owner {
        return Err(LicensePlateAttachmentError::InventoryOwnerMismatch);
    }
    if !snapshot.same_facility {
        return Err(LicensePlateAttachmentError::FacilityMismatch);
    }
    if !snapshot.same_location {
        return Err(LicensePlateAttachmentError::LocationMismatch);
    }
    if snapshot.parent_chain_contains_child {
        return Err(LicensePlateAttachmentError::Cycle);
    }
    let resulting_depth = snapshot
        .parent_depth
        .checked_add(1)
        .and_then(|depth| depth.checked_add(snapshot.child_subtree_height))
        .ok_or(LicensePlateAttachmentError::MaximumDepthExceeded)?;
    if resulting_depth > MAX_LICENSE_PLATE_HIERARCHY_DEPTH {
        return Err(LicensePlateAttachmentError::MaximumDepthExceeded);
    }
    let resulting_nodes = snapshot
        .parent_tree_size
        .checked_add(snapshot.child_tree_size)
        .ok_or(LicensePlateAttachmentError::MaximumNodesExceeded)?;
    if resulting_nodes > MAX_LICENSE_PLATE_HIERARCHY_NODES {
        return Err(LicensePlateAttachmentError::MaximumNodesExceeded);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid() -> LicensePlateAttachmentSnapshot {
        LicensePlateAttachmentSnapshot {
            child_id: 10,
            parent_id: 20,
            child_has_parent: false,
            child_deleted: false,
            parent_deleted: false,
            same_inventory_owner: true,
            same_facility: true,
            same_location: true,
            parent_chain_contains_child: false,
            parent_depth: 2,
            child_subtree_height: 3,
            parent_tree_size: 400,
            child_tree_size: 300,
        }
    }

    #[test]
    fn accepts_an_in_scope_acyclic_attachment_within_depth_limit() {
        assert_eq!(validate_license_plate_attachment(valid()), Ok(()));
    }

    #[test]
    fn rejects_cycles_scope_mismatches_and_excess_depth() {
        let mut cycle = valid();
        cycle.parent_chain_contains_child = true;
        assert_eq!(
            validate_license_plate_attachment(cycle),
            Err(LicensePlateAttachmentError::Cycle)
        );

        let mut owner = valid();
        owner.same_inventory_owner = false;
        assert_eq!(
            validate_license_plate_attachment(owner),
            Err(LicensePlateAttachmentError::InventoryOwnerMismatch)
        );

        let mut deep = valid();
        deep.parent_depth = 5;
        deep.child_subtree_height = 3;
        assert_eq!(
            validate_license_plate_attachment(deep),
            Err(LicensePlateAttachmentError::MaximumDepthExceeded)
        );

        let mut broad = valid();
        broad.parent_tree_size = 800;
        broad.child_tree_size = 201;
        assert_eq!(
            validate_license_plate_attachment(broad),
            Err(LicensePlateAttachmentError::MaximumNodesExceeded)
        );
    }
}
