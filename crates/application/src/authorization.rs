use wareboxes_domain::Timestamp;

/// Permission data projected for application queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionReadModel {
    pub id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: String,
    pub description: Option<String>,
}

/// Role hierarchy and inherited permissions projected for application queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleReadModel {
    pub id: i64,
    pub created: Timestamp,
    pub deleted: Option<Timestamp>,
    pub name: String,
    pub description: Option<String>,
    pub parent_id: Option<i64>,
    pub self_user_id: Option<i64>,
    pub parent_roles: Vec<RoleReadModel>,
    pub child_roles: Vec<RoleReadModel>,
    pub role_permissions: Vec<PermissionReadModel>,
}

impl RoleReadModel {
    pub fn is_self_role(&self) -> bool {
        self.self_user_id.is_some()
    }
}
