#[path = "admin_permissions.rs"]
mod permissions;
#[path = "admin_roles.rs"]
mod roles;
#[path = "admin_users.rs"]
mod users;

pub use permissions::PermissionsWorkbench;
pub use roles::RolesWorkbench;
pub use users::UsersWorkbench;
