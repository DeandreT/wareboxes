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

pub fn has_admin_override(permissions: &[PermissionReadModel]) -> bool {
    permissions
        .iter()
        .any(|permission| permission.name.eq_ignore_ascii_case("admin"))
}

pub fn has_named_permission(permissions: &[PermissionReadModel], name: &str) -> bool {
    has_admin_override(permissions)
        || permissions
            .iter()
            .any(|permission| permission.name.eq_ignore_ascii_case(name))
}

pub fn has_any_named_permission(permissions: &[PermissionReadModel], names: &[&str]) -> bool {
    has_admin_override(permissions)
        || names.iter().any(|name| {
            permissions
                .iter()
                .any(|permission| permission.name.eq_ignore_ascii_case(name))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn permission(name: &str) -> PermissionReadModel {
        PermissionReadModel {
            id: 1,
            created: Timestamp::default(),
            deleted: None,
            name: name.to_owned(),
            description: None,
        }
    }

    #[test]
    fn named_permissions_are_case_insensitive() {
        let permissions = [permission("WMS")];

        assert!(has_named_permission(&permissions, "wms"));
        assert!(has_any_named_permission(&permissions, &["orders", "WmS"]));
        assert!(!has_named_permission(&permissions, "orders"));
    }

    #[test]
    fn admin_overrides_named_permission_checks() {
        let permissions = [permission("ADMIN")];

        assert!(has_admin_override(&permissions));
        assert!(has_named_permission(&permissions, "unassigned"));
        assert!(has_any_named_permission(&permissions, &[]));
    }
}
