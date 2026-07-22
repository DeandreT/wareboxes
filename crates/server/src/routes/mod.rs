use axum::extract::DefaultBodyLimit;
use axum::http::{header, HeaderName, Method};
use axum::routing::{get, post};
use axum::Router;
use tower_http::cors::CorsLayer;
use tower_http::trace::TraceLayer;
use validator::Validate;
use wareboxes_core::field_errors;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

pub mod audits;
pub mod auth;
pub mod employees;
pub mod facilities;
pub mod inventory;
pub mod inventory_owners;
pub mod items;
pub mod license_plates;
pub mod loads;
pub mod locations;
pub mod orders;
pub mod permissions;
pub mod roles;
pub mod tasks;
pub mod users;

/// Validate a request body the same way the Zod `safeParse` did, surfacing
/// per-field messages.
pub fn validate<T: Validate>(value: &T) -> AppResult<()> {
    value
        .validate()
        .map_err(|e| AppError::Core(wareboxes_core::CoreError::Validation(field_errors(&e))))
}

pub fn app(state: AppState) -> Router {
    let api = Router::new()
        // auth
        .route("/auth/login", post(auth::login))
        .route("/auth/register", post(auth::register))
        .route("/auth/logout", post(auth::logout))
        .route("/auth/me", get(auth::me))
        .route("/auth/tenants", get(auth::tenants))
        .route("/auth/context", get(auth::context))
        .route("/auth/settings", post(auth::update_settings))
        // users
        .route("/users", get(users::list))
        .route("/users/update", post(users::update))
        .route("/users/delete", post(users::delete))
        .route("/users/restore", post(users::restore))
        .route("/users/roles/add", post(users::add_role))
        .route("/users/roles/delete", post(users::remove_role))
        // roles
        .route("/roles", get(roles::list))
        .route("/roles/add", post(roles::add))
        .route("/roles/update", post(roles::update))
        .route("/roles/delete", post(roles::delete))
        .route("/roles/restore", post(roles::restore))
        .route("/roles/children/add", post(roles::add_child))
        .route("/roles/children/delete", post(roles::remove_child))
        .route("/roles/permissions/add", post(roles::add_permission))
        .route("/roles/permissions/delete", post(roles::remove_permission))
        // permissions
        .route("/permissions", get(permissions::list))
        .route("/permissions/add", post(permissions::add))
        .route("/permissions/update", post(permissions::update))
        .route("/permissions/delete", post(permissions::delete))
        .route("/permissions/restore", post(permissions::restore))
        // inventory owners
        .route("/inventory-owners", get(inventory_owners::list))
        .route("/inventory-owners/add", post(inventory_owners::add))
        .route("/inventory-owners/update", post(inventory_owners::update))
        .route("/inventory-owners/delete", post(inventory_owners::delete))
        .route("/inventory-owners/restore", post(inventory_owners::restore))
        // facilities (read-only, mirrors the original getFacilities)
        .route("/facilities", get(facilities::list))
        // orders
        .route("/orders", get(orders::list))
        .route("/orders/:order_id", get(orders::get))
        .route("/orders/add", post(orders::add))
        .route("/orders/update", post(orders::update))
        .route("/orders/delete", post(orders::delete))
        .route("/orders/restore", post(orders::restore))
        // WMS catalog / locations
        .route("/items", get(items::list))
        .route("/items/add", post(items::add))
        .route("/items/update", post(items::update))
        .route("/items/delete", post(items::delete))
        .route("/items/restore", post(items::restore))
        .route("/items/pack-links", get(items::list_pack_links))
        .route("/items/pack-links/add", post(items::add_pack_link))
        .route("/items/pack-links/delete", post(items::delete_pack_link))
        .route("/items/skus/add", post(items::add_sku))
        .route("/items/barcodes/add", post(items::add_barcode))
        .route("/items/barcodes/delete", post(items::delete_barcode))
        .route("/locations", get(locations::list))
        .route("/locations/add", post(locations::add))
        .route("/locations/update", post(locations::update))
        .route("/locations/delete", post(locations::delete))
        .route("/locations/restore", post(locations::restore))
        .route("/license-plates", get(license_plates::list))
        .route(
            "/license-plates/barcode/:barcode",
            get(license_plates::get_by_barcode),
        )
        .route("/license-plates/add", post(license_plates::add))
        .route("/license-plates/update", post(license_plates::update))
        .route("/license-plates/move", post(license_plates::move_plate))
        .route("/license-plates/delete", post(license_plates::delete))
        .route("/license-plates/restore", post(license_plates::restore))
        // Loads / inventory
        .route("/loads", get(loads::list))
        .route("/loads/:load_id", get(loads::get))
        .route("/loads/add", post(loads::add))
        .route("/loads/update", post(loads::update))
        .route("/loads/delete", post(loads::delete))
        .route("/loads/restore", post(loads::restore))
        .route("/loads/notes/add", post(loads::add_note))
        .route("/loads/notes/delete", post(loads::delete_note))
        .route("/loads/files/add", post(loads::add_file))
        .route("/loads/files/delete", post(loads::delete_file))
        .route("/loads/lines/add", post(loads::add_line))
        .route("/loads/lines/receive", post(loads::receive_line))
        .route("/mobile/inbound/loads", get(loads::mobile_inbound_list))
        .route(
            "/mobile/inbound/loads/:load_id",
            get(loads::mobile_inbound_get),
        )
        .route(
            "/mobile/inbound/loads/:load_id/arrive",
            post(loads::mobile_arrive),
        )
        .route(
            "/mobile/inbound/lines/:load_line_id/receive",
            post(loads::mobile_receive_line),
        )
        .route("/inventory/batches", get(inventory::list_batches))
        .route("/inventory/batches/add", post(inventory::add_batch))
        .route("/inventory/batches/delete", post(inventory::delete_batch))
        .route("/inventory/balances", get(inventory::list_balances))
        .route(
            "/inventory/reconciliation",
            get(inventory::list_reconciliation_issues),
        )
        .route("/inventory/receive", post(inventory::receive))
        .route("/inventory/transactions", get(inventory::list_transactions))
        .route("/inventory/moves", post(inventory::move_stock))
        .route("/inventory/moves/split", post(inventory::split_move_stock))
        .route("/inventory/reservations", get(inventory::list_reservations))
        .route("/inventory/reservations/add", post(inventory::reserve))
        .route(
            "/inventory/reservations/cancel",
            post(inventory::cancel_reservation),
        )
        // Work tasks
        .route("/tasks", get(tasks::list))
        .route(
            "/tasks/cycle-counts/item-location/add",
            post(tasks::create_item_location_cycle_count),
        )
        .route(
            "/tasks/cycle-counts/location/add",
            post(tasks::create_location_cycle_count),
        )
        .route(
            "/tasks/break-master-packs/add",
            post(tasks::create_break_master_pack),
        )
        .route(
            "/tasks/unpack-cancelled-orders/add",
            post(tasks::create_unpack_cancelled_order),
        )
        .route(
            "/tasks/unpack-cancelled-orders/lines",
            get(tasks::list_unpack_cancelled_order_lines),
        )
        .route("/tasks/assign", post(tasks::assign))
        .route("/tasks/start-next", post(tasks::start_next))
        .route("/tasks/start", post(tasks::start))
        .route("/tasks/progress", post(tasks::progress))
        .route("/tasks/complete", post(tasks::complete))
        .route("/tasks/abort", post(tasks::abort))
        .route("/tasks/release-expired", post(tasks::release_expired))
        .route("/tasks/cancel", post(tasks::cancel))
        // Admin operations
        .route("/employees", get(employees::list))
        .route("/employees/add", post(employees::add))
        .route("/employees/update", post(employees::update))
        .route("/employees/delete", post(employees::delete))
        .route("/employees/restore", post(employees::restore))
        .route("/audits", get(audits::list))
        .route("/audits/add", post(audits::add))
        .route("/audits/update", post(audits::update))
        .route("/audits/delete", post(audits::delete))
        .route("/audits/restore", post(audits::restore))
        .route("/audits/:audit_id/counts", get(audits::counts));

    let security = state.security.clone();
    let mut app = Router::new()
        .route("/health", get(|| async { "ok" }))
        .nest("/api", api)
        .layer(TraceLayer::new_for_http())
        .layer(DefaultBodyLimit::max(security.max_request_body_bytes));

    if !security.cors_allowed_origins.is_empty() {
        app = app.layer(
            CorsLayer::new()
                .allow_origin(security.cors_allowed_origins)
                .allow_methods([Method::GET, Method::POST])
                .allow_headers([
                    header::AUTHORIZATION,
                    header::CONTENT_TYPE,
                    HeaderName::from_static(crate::auth::TENANT_ID_HEADER),
                ]),
        );
    }

    app.with_state(state)
}
