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
