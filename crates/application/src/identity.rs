use wareboxes_domain::{Timestamp, UserId};

use crate::authorization::{PermissionReadModel, RoleReadModel};

/// A platform identity projected independently of any tenant membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserIdentityReadModel {
    pub id: UserId,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub email: String,
    pub nick_name: Option<String>,
    pub phone: Option<String>,
}

/// A user as visible inside one tenant, including tenant-local authorization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TenantUserReadModel {
    pub identity: UserIdentityReadModel,
    pub membership_deleted: Option<Timestamp>,
    pub direct_roles: Vec<RoleReadModel>,
    pub permissions: Vec<PermissionReadModel>,
}

/// User-owned presentation preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct UserSettingsReadModel {
    pub light_mode: bool,
}
