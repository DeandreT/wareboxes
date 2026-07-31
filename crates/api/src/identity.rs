use wareboxes_application::authorization::{PermissionReadModel, RoleReadModel};
use wareboxes_application::identity::{
    TenantUserReadModel, UserIdentityReadModel, UserSettingsReadModel,
};
use wareboxes_core::dto::UserSettings;
use wareboxes_core::models::{Permission, Role, User};
use wareboxes_domain::Timestamp;

fn permission_response(permission: PermissionReadModel) -> Permission {
    Permission {
        id: permission.id,
        created: permission.created,
        deleted: permission.deleted,
        name: permission.name,
        description: permission.description,
    }
}

fn role_response(role: RoleReadModel) -> Role {
    Role {
        id: role.id,
        created: role.created,
        deleted: role.deleted,
        name: role.name,
        description: role.description,
        parent_id: role.parent_id,
        self_user_id: role.self_user_id,
        parent_roles: role.parent_roles.into_iter().map(role_response).collect(),
        child_roles: role.child_roles.into_iter().map(role_response).collect(),
        role_permissions: role
            .role_permissions
            .into_iter()
            .map(permission_response)
            .collect(),
    }
}

fn user_response_with_deleted(identity: UserIdentityReadModel, deleted: Option<Timestamp>) -> User {
    User {
        id: identity.id.get(),
        created: identity.created,
        deleted,
        first_name: identity.first_name,
        last_name: identity.last_name,
        email: identity.email,
        nick_name: identity.nick_name,
        phone: identity.phone,
        user_roles: Vec::new(),
        user_permissions: Vec::new(),
    }
}

pub(crate) fn identity_response(identity: UserIdentityReadModel) -> User {
    let deleted = identity.deleted;
    user_response_with_deleted(identity, deleted)
}

pub(crate) fn enriched_user_response(user: TenantUserReadModel) -> User {
    let deleted = user.identity.deleted;
    tenant_user_response_with_deleted(user, deleted)
}

pub(crate) fn tenant_user_response(user: TenantUserReadModel) -> User {
    let deleted = user.membership_deleted;
    tenant_user_response_with_deleted(user, deleted)
}

fn tenant_user_response_with_deleted(
    user: TenantUserReadModel,
    deleted: Option<Timestamp>,
) -> User {
    let mut response = user_response_with_deleted(user.identity, deleted);
    response.user_roles = user.direct_roles.into_iter().map(role_response).collect();
    response.user_permissions = user
        .permissions
        .into_iter()
        .map(permission_response)
        .collect();
    response
}

pub(crate) fn settings_response(settings: UserSettingsReadModel) -> UserSettings {
    UserSettings {
        light_mode: settings.light_mode,
    }
}
