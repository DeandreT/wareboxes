//! Internal request and response payloads shared by the server and operator
//! applications.

use serde::{Deserialize, Serialize};
use validator::{Validate, ValidationError};

use crate::models::{
    InventoryHoldReason, InventoryStatus, InventoryStatusChangeReason, LoadFileCategory,
    LoadStatus, LoadType, Order, TenantAccess, Timestamp, User,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Paged<T> {
    pub items: Vec<T>,
    pub total: i64,
    pub limit: i64,
    pub offset: i64,
}

impl<T> Paged<T> {
    pub fn new(items: Vec<T>, total: i64, limit: i64, offset: i64) -> Self {
        Self {
            items,
            total,
            limit,
            offset,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SummaryCount {
    pub key: String,
    pub label: String,
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderPage {
    pub page: Paged<Order>,
    pub summaries: Vec<SummaryCount>,
}

// ---------------------------------------------------------------------------
// Auth
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LoginRequest {
    #[validate(email(message = "Invalid email"))]
    pub email: String,
    #[validate(length(min = 1, message = "Password is required"))]
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct RegisterRequest {
    #[validate(email(message = "Invalid email"))]
    pub email: String,
    #[validate(length(min = 8, message = "Password must be at least 8 characters"))]
    pub password: String,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
}

/// What the client holds after a successful login.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionUser {
    pub token: String,
    pub user: User,
    pub active_tenant: TenantAccess,
    #[serde(default)]
    pub settings: UserSettings,
}

/// Browser-safe authenticated context. The web session token is held only in
/// an HTTP-only cookie and is intentionally absent from this projection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSessionContext {
    pub user: User,
    pub active_tenant: TenantAccess,
    pub available_tenants: Vec<TenantAccess>,
    #[serde(default)]
    pub settings: UserSettings,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct SelectTenantRequest {
    #[validate(range(min = 1, message = "Invalid tenant ID"))]
    pub tenant_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate, Default)]
pub struct UserSettings {
    pub light_mode: bool,
}

// ---------------------------------------------------------------------------
// Users
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct UserUpdate {
    #[validate(range(min = 1, message = "Invalid user ID"))]
    pub user_id: i64,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub nick_name: Option<String>,
    pub phone: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct UserIdRequest {
    #[validate(range(min = 1, message = "Invalid user ID"))]
    pub user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddDeleteUserRole {
    #[validate(range(min = 1, message = "Invalid user ID"))]
    pub user_id: i64,
    #[validate(range(min = 1, message = "Invalid role ID"))]
    pub role_id: i64,
}

/// Replaces a tenant member's complete facility and inventory-owner scope.
/// An `all_*` flag and a non-empty corresponding ID list are mutually exclusive.
#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct UpdateUserAccessScope {
    #[validate(range(min = 1, message = "Invalid user ID"))]
    pub user_id: i64,
    pub all_facilities: bool,
    pub facility_ids: Vec<i64>,
    pub all_inventory_owners: bool,
    pub inventory_owner_ids: Vec<i64>,
}

// ---------------------------------------------------------------------------
// Roles
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddRole {
    #[validate(length(min = 1, message = "Role name is required"))]
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct UpdateRole {
    #[validate(range(min = 1, message = "Invalid role ID"))]
    pub role_id: i64,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct RoleIdRequest {
    #[validate(range(min = 1, message = "Invalid role ID"))]
    pub role_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddDeleteChildRole {
    #[validate(range(min = 1, message = "Invalid role ID"))]
    pub role_id: i64,
    #[validate(range(min = 1, message = "Invalid role ID"))]
    pub child_role_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddDeleteRolePermission {
    #[validate(range(min = 1, message = "Invalid role ID"))]
    pub role_id: i64,
    #[validate(range(min = 1, message = "Invalid permission ID"))]
    pub permission_id: i64,
}

// ---------------------------------------------------------------------------
// Permissions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddPermission {
    #[validate(length(min = 3, message = "Name must be at least 3 characters"))]
    pub name: String,
    #[validate(length(min = 3, message = "Description must be at least 3 characters"))]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct UpdatePermission {
    #[validate(range(min = 1, message = "Invalid permission ID"))]
    pub permission_id: i64,
    #[validate(length(min = 3, message = "Name must be at least 3 characters"))]
    pub name: Option<String>,
    #[validate(length(min = 3, message = "Description must be at least 3 characters"))]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct PermissionIdRequest {
    #[validate(range(min = 1, message = "Invalid permission ID"))]
    pub permission_id: i64,
}

// ---------------------------------------------------------------------------
// Inventory Owners
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddInventoryOwner {
    #[validate(length(min = 3, message = "Inventory owner name is required"))]
    pub name: String,
    #[validate(email(message = "Invalid email"))]
    pub email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct InventoryOwnerUpdate {
    #[validate(range(min = 1, message = "Invalid inventory owner ID"))]
    pub inventory_owner_id: i64,
    #[validate(length(min = 3, message = "Inventory owner name is required"))]
    pub name: Option<String>,
    #[validate(email(message = "Invalid email"))]
    pub email: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct InventoryOwnerIdRequest {
    #[validate(range(min = 1, message = "Invalid inventory owner ID"))]
    pub inventory_owner_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ReplaceInventoryOwnerFacilities {
    #[validate(range(min = 1, message = "Invalid inventory owner ID"))]
    pub inventory_owner_id: i64,
    pub facility_ids: Vec<i64>,
}

// ---------------------------------------------------------------------------
// Orders
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct OrderUpdate {
    #[validate(range(min = 1, message = "Invalid order ID"))]
    pub order_id: i64,
    pub order_key: Option<String>,
    pub rush: Option<bool>,
    pub ship_by: Option<Timestamp>,
    pub line1: Option<String>,
    pub line2: Option<String>,
    pub city: Option<String>,
    pub state: Option<String>,
    pub postal_code: Option<String>,
    pub country: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct OrderIdRequest {
    #[validate(range(min = 1, message = "Invalid order ID"))]
    pub order_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct CancelOrder {
    #[validate(range(min = 1, message = "Invalid order ID"))]
    pub order_id: i64,
    #[validate(range(min = 1, message = "Invalid facility ID"))]
    pub facility_id: i64,
}

// ---------------------------------------------------------------------------
// Items
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddItem {
    #[validate(length(min = 1, message = "Description is required"))]
    pub description: String,
    #[validate(length(min = 1, message = "Packaging unit is required"))]
    pub packaging_unit: String,
    pub notes: Option<String>,
    pub length: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub length_uom: Option<String>,
    pub weight: Option<i64>,
    pub weight_uom: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ItemUpdate {
    #[validate(range(min = 1, message = "Invalid item ID"))]
    pub item_id: i64,
    pub description: Option<String>,
    pub notes: Option<String>,
    pub packaging_unit: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ItemIdRequest {
    #[validate(range(min = 1, message = "Invalid item ID"))]
    pub item_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddItemPackLink {
    #[validate(range(min = 1, message = "Invalid master item ID"))]
    pub master_item_id: i64,
    #[validate(range(min = 1, message = "Invalid single item ID"))]
    pub single_item_id: i64,
    #[validate(range(min = 2, message = "Inner quantity must be at least 2"))]
    pub inner_qty: i64,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ItemPackLinkIdRequest {
    #[validate(range(min = 1, message = "Invalid item pack link ID"))]
    pub item_pack_link_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddSku {
    #[validate(range(min = 1, message = "Invalid item ID"))]
    pub item_id: i64,
    #[validate(length(min = 1, message = "SKU name is required"))]
    pub name: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddBarcode {
    #[validate(range(min = 1, message = "Invalid item ID"))]
    pub item_id: i64,
    #[validate(length(min = 1, message = "Barcode is required"))]
    pub name: String,
    #[validate(length(min = 1, message = "Barcode type is required"))]
    pub r#type: String,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct BarcodeIdRequest {
    #[validate(range(min = 1, message = "Invalid barcode ID"))]
    pub barcode_id: i64,
}

// ---------------------------------------------------------------------------
// Locations
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddLocation {
    #[validate(range(min = 1, message = "Invalid facility ID"))]
    pub facility_id: i64,
    pub parent_location_id: Option<i64>,
    pub barcode: Option<String>,
    pub name: Option<String>,
    #[validate(length(min = 1, message = "Location type is required"))]
    pub r#type: String,
    pub active: Option<bool>,
    pub pickable: Option<bool>,
    pub receivable: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LocationUpdate {
    #[validate(range(min = 1, message = "Invalid location ID"))]
    pub location_id: i64,
    pub parent_location_id: Option<i64>,
    pub barcode: Option<String>,
    pub name: Option<String>,
    pub r#type: Option<String>,
    pub active: Option<bool>,
    pub pickable: Option<bool>,
    pub receivable: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LocationIdRequest {
    #[validate(range(min = 1, message = "Invalid location ID"))]
    pub location_id: i64,
}

// ---------------------------------------------------------------------------
// Employees
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddEmployee {
    #[validate(length(min = 1, message = "First name is required"))]
    pub first_name: String,
    #[validate(length(min = 1, message = "Last name is required"))]
    pub last_name: String,
    #[validate(length(min = 1, message = "Title is required"))]
    pub title: String,
    #[validate(length(min = 1, message = "Type is required"))]
    pub r#type: String,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub hired: Option<Timestamp>,
    #[validate(length(min = 1, message = "At least one facility is required"))]
    pub facility_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct EmployeeUpdate {
    #[validate(range(min = 1, message = "Invalid employee ID"))]
    pub employee_id: i64,
    pub first_name: Option<String>,
    pub last_name: Option<String>,
    pub title: Option<String>,
    pub r#type: Option<String>,
    pub email: Option<String>,
    pub phone: Option<String>,
    pub terminated: Option<Timestamp>,
    #[validate(length(min = 1, message = "At least one facility is required"))]
    pub facility_ids: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct EmployeeIdRequest {
    #[validate(range(min = 1, message = "Invalid employee ID"))]
    pub employee_id: i64,
}

// ---------------------------------------------------------------------------
// License plates
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddLicensePlate {
    #[validate(range(min = 1, message = "Invalid inventory owner ID"))]
    pub inventory_owner_id: i64,
    #[validate(range(min = 1, message = "Invalid facility ID"))]
    pub facility_id: i64,
    pub barcode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LicensePlateUpdate {
    #[validate(range(min = 1, message = "Invalid license plate ID"))]
    pub license_plate_id: i64,
    pub barcode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LicensePlateIdRequest {
    #[validate(range(min = 1, message = "Invalid license plate ID"))]
    pub license_plate_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct MoveLicensePlate {
    #[validate(range(min = 1, message = "Invalid license plate ID"))]
    pub license_plate_id: i64,
    #[validate(range(min = 1, message = "Invalid destination location ID"))]
    pub to_location_id: i64,
    pub reason: Option<String>,
    #[validate(length(min = 1, max = 200, message = "Idempotency key is required"))]
    pub idempotency_key: String,
}

// ---------------------------------------------------------------------------
// Loads
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddLoad {
    #[validate(range(min = 1, message = "Invalid facility ID"))]
    pub facility_id: i64,
    #[validate(range(min = 1, message = "Invalid inventory owner ID"))]
    pub inventory_owner_id: i64,
    pub r#type: LoadType,
    pub reference_number: Option<String>,
    pub invoice_number: Option<String>,
    pub carrier: Option<String>,
    pub trailer_number: Option<String>,
    pub seal_number: Option<String>,
    pub dock_door_location_id: Option<i64>,
    pub expected_time: Option<Timestamp>,
    pub appointment_time: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LoadUpdate {
    #[validate(range(min = 1, message = "Invalid load ID"))]
    pub load_id: i64,
    pub status: Option<LoadStatus>,
    pub r#type: Option<LoadType>,
    pub reference_number: Option<String>,
    pub invoice_number: Option<String>,
    pub carrier: Option<String>,
    pub trailer_number: Option<String>,
    pub seal_number: Option<String>,
    pub dock_door_location_id: Option<i64>,
    pub expected_time: Option<Timestamp>,
    pub appointment_time: Option<Timestamp>,
    pub actual_time: Option<Timestamp>,
    pub arrival: Option<Timestamp>,
    pub departure: Option<Timestamp>,
    pub rejected: Option<Timestamp>,
    pub closed: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ArriveLoad {
    pub invoice_number: Option<String>,
    pub arrival: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LoadIdRequest {
    #[validate(range(min = 1, message = "Invalid load ID"))]
    pub load_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddLoadNote {
    #[validate(range(min = 1, message = "Invalid load ID"))]
    pub load_id: i64,
    #[validate(length(min = 1, message = "Note is required"))]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LoadNoteIdRequest {
    #[validate(range(min = 1, message = "Invalid load note ID"))]
    pub load_note_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddLoadLine {
    #[validate(range(min = 1, message = "Invalid load ID"))]
    pub load_id: i64,
    #[validate(range(min = 1, message = "Invalid item ID"))]
    pub item_id: i64,
    pub sku_id: Option<i64>,
    #[validate(range(min = 1, message = "Expected quantity must be positive"))]
    pub expected_qty: i64,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddLoadFile {
    #[validate(range(min = 1, message = "Invalid load ID"))]
    pub load_id: i64,
    #[validate(length(min = 1, message = "Original file name is required"))]
    pub original_name: String,
    #[validate(length(min = 1, message = "Stored file name is required"))]
    pub name: String,
    #[validate(length(min = 1, message = "File path is required"))]
    pub path: String,
    pub content_type: Option<String>,
    pub category: Option<LoadFileCategory>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct LoadFileIdRequest {
    #[validate(range(min = 1, message = "Invalid file ID"))]
    pub file_id: i64,
}

// ---------------------------------------------------------------------------
// Audits
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddAuditWave {
    #[validate(range(min = 1, message = "Invalid facility ID"))]
    pub facility_id: i64,
    #[validate(range(min = 1, message = "Invalid inventory owner ID"))]
    pub inventory_owner_id: i64,
    #[validate(length(min = 1, message = "Name is required"))]
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AuditWaveUpdate {
    #[validate(range(min = 1, message = "Invalid audit wave ID"))]
    pub audit_wave_id: i64,
    pub name: Option<String>,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AuditWaveIdRequest {
    #[validate(range(min = 1, message = "Invalid audit wave ID"))]
    pub audit_wave_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct AddAuditLocationCount {
    #[validate(range(min = 1, message = "Invalid audit wave ID"))]
    pub audit_wave_id: i64,
    #[validate(range(min = 1, message = "Invalid location ID"))]
    pub location_id: i64,
    #[validate(range(min = 1, message = "Invalid item ID"))]
    pub item_id: i64,
    #[validate(length(min = 1, message = "UOM is required"))]
    pub uom: String,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub serial: Option<String>,
    #[validate(range(min = 0, message = "Count quantity cannot be negative"))]
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct AuditLocationCountUpdate {
    #[validate(range(min = 1, message = "Invalid audit location count ID"))]
    pub audit_location_count_id: i64,
    #[validate(range(min = 1, message = "Invalid expected revision"))]
    pub expected_revision: i64,
    #[validate(range(min = 0, message = "Count quantity cannot be negative"))]
    pub count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AuditLocationCountIdRequest {
    #[validate(range(min = 1, message = "Invalid audit location count ID"))]
    pub audit_location_count_id: i64,
    #[validate(range(min = 1, message = "Invalid expected revision"))]
    pub expected_revision: i64,
}

// ---------------------------------------------------------------------------
// Work tasks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct WorkTaskIdRequest {
    #[validate(range(min = 1, message = "Invalid task ID"))]
    pub task_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate, Default)]
pub struct StartNextWorkTask {
    pub task_type: Option<crate::models::WorkTaskType>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AssignWorkTask {
    #[validate(range(min = 1, message = "Invalid task ID"))]
    pub task_id: i64,
    #[validate(range(min = 1, message = "Invalid user ID"))]
    pub assigned_user_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct RecordWorkTaskProgress {
    #[validate(range(min = 1, message = "Invalid task ID"))]
    pub task_id: i64,
    #[validate(range(min = 1, message = "Invalid task line ID"))]
    pub task_line_id: Option<i64>,
    #[serde(default)]
    pub action: crate::models::WorkTaskProgressAction,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub qty_completed: i64,
    #[validate(range(min = 1, message = "Invalid from location ID"))]
    pub from_location_id: Option<i64>,
    #[validate(range(min = 1, message = "Invalid to location ID"))]
    pub to_location_id: Option<i64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct CompleteWorkTask {
    #[validate(range(min = 1, message = "Invalid task ID"))]
    pub task_id: i64,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub qty_completed: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct CreatePutawayTask {
    #[validate(range(min = 1, message = "Invalid source inventory balance ID"))]
    pub source_inventory_balance_id: i64,
    #[validate(range(min = 1, message = "Invalid destination location ID"))]
    pub destination_location_id: i64,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub quantity: i64,
    #[validate(range(min = 0, message = "Priority must be zero or greater"))]
    pub priority: Option<i64>,
    #[validate(range(min = 1, message = "Invalid user ID"))]
    pub assigned_user_id: Option<i64>,
    pub scheduled_for: Option<Timestamp>,
    pub due_at: Option<Timestamp>,
    #[validate(length(
        min = 1,
        max = 1000,
        message = "Instructions must contain between 1 and 1000 characters"
    ))]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct CreateLicensePlatePutawayTask {
    #[validate(range(min = 1, message = "Invalid license plate ID"))]
    pub license_plate_id: i64,
    #[validate(range(min = 1, message = "Invalid destination location ID"))]
    pub destination_location_id: i64,
    #[validate(range(min = 0, message = "Priority must be zero or greater"))]
    pub priority: Option<i64>,
    #[validate(range(min = 1, message = "Invalid user ID"))]
    pub assigned_user_id: Option<i64>,
    pub scheduled_for: Option<Timestamp>,
    pub due_at: Option<Timestamp>,
    #[validate(length(
        min = 1,
        max = 1000,
        message = "Instructions must contain between 1 and 1000 characters"
    ))]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct ConfirmPutaway {
    #[validate(range(min = 1, message = "Invalid task ID"))]
    pub task_id: i64,
    #[validate(range(min = 1, message = "Invalid destination location ID"))]
    pub destination_location_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct CreateItemLocationCycleCountTask {
    #[validate(range(min = 1, message = "Invalid location ID"))]
    pub location_id: i64,
    #[validate(range(min = 1, message = "Invalid item ID"))]
    pub item_id: i64,
    pub source: Option<String>,
    pub order_id: Option<i64>,
    pub order_item_id: Option<i64>,
    #[validate(range(min = 1, message = "Invalid inventory balance ID"))]
    pub inventory_balance_id: i64,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
pub struct ConfirmItemLocationCycleCount {
    #[validate(range(min = 1, message = "Invalid task ID"))]
    pub task_id: i64,
    #[validate(range(min = 0, message = "Counted quantity cannot be negative"))]
    pub counted_quantity: i64,
    #[validate(length(
        min = 1,
        max = 1000,
        message = "Note must contain between 1 and 1000 characters"
    ))]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct CreateLocationCycleCountTask {
    #[validate(range(min = 1, message = "Invalid location ID"))]
    pub location_id: i64,
    #[validate(range(min = 0, message = "Priority must be zero or greater"))]
    pub priority: Option<i64>,
    pub assigned_user_id: Option<i64>,
    pub scheduled_for: Option<Timestamp>,
    pub due_at: Option<Timestamp>,
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct CreateBreakMasterPackTask {
    #[validate(range(min = 1, message = "Invalid master item ID"))]
    pub master_item_id: i64,
    #[validate(range(min = 1, message = "Invalid single item ID"))]
    pub single_item_id: i64,
    #[validate(range(min = 1, message = "Invalid location ID"))]
    pub location_id: i64,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub qty: i64,
    #[validate(range(min = 0, message = "Priority must be zero or greater"))]
    pub priority: Option<i64>,
    pub assigned_user_id: Option<i64>,
    pub scheduled_for: Option<Timestamp>,
    pub due_at: Option<Timestamp>,
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct CreateUnpackCancelledOrderTask {
    #[validate(range(min = 1, message = "Invalid order ID"))]
    pub order_id: i64,
    #[validate(range(min = 1, message = "Invalid facility ID"))]
    pub facility_id: i64,
    #[validate(range(min = 0, message = "Priority must be zero or greater"))]
    pub priority: Option<i64>,
    pub assigned_user_id: Option<i64>,
    pub scheduled_for: Option<Timestamp>,
    pub due_at: Option<Timestamp>,
    pub instructions: Option<String>,
}

// ---------------------------------------------------------------------------
// Inventory
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AddItemBatch {
    #[validate(range(min = 1, message = "Invalid inventory owner ID"))]
    pub inventory_owner_id: i64,
    #[validate(range(min = 1, message = "Invalid item ID"))]
    pub item_id: i64,
    pub load_id: Option<i64>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ItemBatchIdRequest {
    #[validate(range(min = 1, message = "Invalid item batch ID"))]
    pub item_batch_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct MoveInventory {
    #[validate(range(min = 1, message = "Invalid item batch ID"))]
    pub item_batch_id: i64,
    #[validate(range(min = 1, message = "Invalid source location ID"))]
    pub from_location_id: i64,
    #[validate(range(min = 1, message = "Invalid destination location ID"))]
    pub to_location_id: i64,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub qty: i64,
    pub status: Option<InventoryStatus>,
    pub reason: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    #[validate(length(min = 1, max = 200, message = "Idempotency key is required"))]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct SplitMoveInventoryDestination {
    #[validate(range(min = 1, message = "Invalid destination location ID"))]
    pub to_location_id: i64,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub qty: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct SplitMoveInventory {
    #[validate(range(min = 1, message = "Invalid source inventory balance ID"))]
    pub from_inventory_balance_id: i64,
    #[validate(length(min = 1, message = "At least one destination is required"))]
    #[validate(nested)]
    pub destinations: Vec<SplitMoveInventoryDestination>,
    pub reason: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
    #[validate(length(min = 1, max = 200, message = "Idempotency key is required"))]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct CreateInventoryReservation {
    #[validate(range(min = 1, message = "Invalid order ID"))]
    pub order_id: i64,
    #[validate(range(min = 1, message = "Invalid order item ID"))]
    pub order_item_id: i64,
    #[validate(range(min = 1, message = "Invalid facility ID"))]
    pub facility_id: i64,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub qty: i64,
    #[validate(length(min = 1, max = 200, message = "Idempotency key is required"))]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct AllocateInventory {
    #[validate(range(min = 1, message = "Invalid reservation ID"))]
    pub reservation_id: i64,
    #[validate(range(min = 1, message = "Invalid inventory balance ID"))]
    pub inventory_balance_id: i64,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub qty: i64,
    #[validate(length(min = 1, max = 200, message = "Idempotency key is required"))]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct CancelInventoryAllocation {
    #[validate(range(min = 1, message = "Invalid inventory allocation ID"))]
    pub allocation_id: i64,
    #[validate(length(min = 1, max = 200, message = "Idempotency key is required"))]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct CancelInventoryReservation {
    #[validate(range(min = 1, message = "Invalid reservation ID"))]
    pub reservation_id: i64,
    #[validate(length(min = 1, max = 200, message = "Idempotency key is required"))]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
#[validate(schema(function = "validate_change_inventory_status"))]
pub struct ChangeInventoryStatus {
    #[validate(range(min = 1, message = "Invalid inventory balance ID"))]
    pub inventory_balance_id: i64,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub qty: i64,
    pub to_status: InventoryStatus,
    pub reason: InventoryStatusChangeReason,
    #[validate(length(
        min = 1,
        max = 1000,
        message = "Note must contain between 1 and 1000 characters"
    ))]
    pub note: Option<String>,
    #[validate(length(
        min = 1,
        max = 100,
        message = "Reference type must contain between 1 and 100 characters"
    ))]
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
}

fn validate_change_inventory_status(value: &ChangeInventoryStatus) -> Result<(), ValidationError> {
    if let Some(note) = value.note.as_deref() {
        if note.trim() != note || note.is_empty() {
            return Err(ValidationError::new("invalid_status_change_note")
                .with_message("Status change note must be trimmed and nonempty".into()));
        }
    }
    if value.reason == InventoryStatusChangeReason::Other && value.note.is_none() {
        return Err(
            ValidationError::new("other_status_change_reason_requires_note")
                .with_message("A note is required when the status change reason is other".into()),
        );
    }
    if !value.reason.allows_target_status(value.to_status) {
        return Err(
            ValidationError::new("status_change_reason_target_mismatch").with_message(
                "Status change reason does not permit the requested target status".into(),
            ),
        );
    }

    match (&value.reference_type, value.reference_id) {
        (None, None) => Ok(()),
        (Some(reference_type), Some(reference_id))
            if reference_type.trim() == reference_type
                && !reference_type.is_empty()
                && reference_id > 0 =>
        {
            Ok(())
        }
        _ => Err(
            ValidationError::new("invalid_status_change_reference").with_message(
                "Status change reference type and positive ID must be provided together".into(),
            ),
        ),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
#[validate(schema(function = "validate_place_inventory_hold"))]
pub struct PlaceInventoryHold {
    #[validate(range(min = 1, message = "Invalid inventory balance ID"))]
    pub inventory_balance_id: i64,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub qty: i64,
    pub reason: InventoryHoldReason,
    pub note: Option<String>,
    pub reference_type: Option<String>,
    pub reference_id: Option<i64>,
}

fn validate_place_inventory_hold(value: &PlaceInventoryHold) -> Result<(), ValidationError> {
    if value.reason == InventoryHoldReason::Other
        && !matches!(value.note.as_deref(), Some(note) if !note.trim().is_empty())
    {
        return Err(ValidationError::new("other_hold_reason_requires_note")
            .with_message("A note is required when the hold reason is other".into()));
    }

    match (&value.reference_type, value.reference_id) {
        (None, None) => Ok(()),
        (Some(reference_type), Some(reference_id))
            if !reference_type.trim().is_empty() && reference_id > 0 =>
        {
            Ok(())
        }
        _ => Err(ValidationError::new("invalid_hold_reference")
            .with_message("Hold reference type and positive ID must be provided together".into())),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ReleaseInventoryHold {
    #[validate(range(min = 1, message = "Invalid inventory hold ID"))]
    pub hold_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateInventoryReservationResult {
    pub reservation_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AllocateInventoryResult {
    pub allocation_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CancelInventoryAllocationResult {
    pub allocation_id: i64,
    pub released_qty: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CancelInventoryReservationResult {
    pub reservation_id: i64,
    pub released_qty: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlaceInventoryHoldResult {
    pub hold_id: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseInventoryHoldResult {
    pub hold_id: i64,
    pub released_qty: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ChangeInventoryStatusResult {
    pub inventory_transaction_id: i64,
    pub source_inventory_balance_id: i64,
    pub target_inventory_balance_id: i64,
    pub qty: i64,
    pub from_status: InventoryStatus,
    pub to_status: InventoryStatus,
}

#[cfg(test)]
mod inventory_status_change_dto_tests {
    use super::*;

    fn valid_request() -> ChangeInventoryStatus {
        ChangeInventoryStatus {
            inventory_balance_id: 42,
            qty: 5,
            to_status: InventoryStatus::Quarantine,
            reason: InventoryStatusChangeReason::QualityInspection,
            note: Some("Awaiting inspection".into()),
            reference_type: Some("receipt".into()),
            reference_id: Some(81),
        }
    }

    #[test]
    fn status_change_request_uses_header_idempotency_contract() {
        let request = valid_request();
        request.validate().unwrap();

        let value = serde_json::to_value(request).unwrap();
        assert!(value.get("idempotency_key").is_none());
        assert_eq!(value["to_status"], "quarantine");
        assert_eq!(value["reason"], "quality_inspection");
        assert!(
            serde_json::from_value::<ChangeInventoryStatus>(serde_json::json!({
                "inventory_balance_id": 42,
                "qty": 5,
                "to_status": "quarantine",
                "reason": "quality_inspection",
                "note": "Awaiting inspection",
                "reference_type": "receipt",
                "reference_id": 81,
                "idempotency_key": "must-be-a-header"
            }))
            .is_err()
        );
    }

    #[test]
    fn status_change_request_validates_quantity_note_and_reference() {
        let invalid_quantities = [
            ChangeInventoryStatus {
                inventory_balance_id: 0,
                ..valid_request()
            },
            ChangeInventoryStatus {
                qty: 0,
                ..valid_request()
            },
        ];
        for request in invalid_quantities {
            assert!(request.validate().is_err());
        }

        assert!(ChangeInventoryStatus {
            reason: InventoryStatusChangeReason::Other,
            note: None,
            ..valid_request()
        }
        .validate()
        .is_err());
        assert!(ChangeInventoryStatus {
            note: Some(" untrimmed".into()),
            ..valid_request()
        }
        .validate()
        .is_err());
        assert!(ChangeInventoryStatus {
            to_status: InventoryStatus::Damaged,
            reason: InventoryStatusChangeReason::QualityInspection,
            ..valid_request()
        }
        .validate()
        .is_err());
        assert!(ChangeInventoryStatus {
            reference_id: None,
            ..valid_request()
        }
        .validate()
        .is_err());
        assert!(ChangeInventoryStatus {
            reference_type: None,
            ..valid_request()
        }
        .validate()
        .is_err());
    }
}
