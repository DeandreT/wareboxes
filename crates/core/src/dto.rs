//! Request/response payloads — the wire contract shared by the Axum server
//! and the egui client. Validation rules mirror the Zod schemas in the
//! original `app/utils/*.ts` files.

use serde::{Deserialize, Serialize};
use validator::Validate;

use crate::models::{
    InventoryStatus, LoadFileCategory, LoadStatus, LoadType, Order, OrderStatus, TenantAccess,
    Timestamp, User,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorResponse {
    pub errors: Vec<String>,
}

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

// ---------------------------------------------------------------------------
// Orders
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct NewOrder {
    #[validate(length(min = 1, message = "Order key is required"))]
    pub order_key: String,
    pub rush: Option<bool>,
    pub ship_by: Option<Timestamp>,
    pub line1: Option<String>,
    pub line2: Option<String>,
    pub city: String,
    pub state: String,
    pub postal_code: String,
    pub country: String,
    pub inventory_owner_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct OrderUpdate {
    #[validate(range(min = 1, message = "Invalid order ID"))]
    pub order_id: i64,
    pub order_key: Option<String>,
    pub status: Option<OrderStatus>,
    pub rush: Option<bool>,
    pub confirmed: Option<Timestamp>,
    pub closed: Option<Timestamp>,
    pub ship_by: Option<Timestamp>,
    pub wave_id: Option<i64>,
    pub inventory_owner_id: Option<i64>,
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
    pub receive_completed: Option<bool>,
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
pub struct ReceiveLoadLine {
    #[validate(range(min = 1, message = "Invalid load line ID"))]
    pub load_line_id: i64,
    #[validate(range(min = 1, message = "Invalid receiving location ID"))]
    pub to_location_id: i64,
    #[validate(range(min = 0, message = "Received quantity cannot be negative"))]
    pub received_qty: i64,
    #[validate(range(min = 0, message = "Rejected quantity cannot be negative"))]
    pub rejected_qty: i64,
    #[validate(range(min = 0, message = "Missing quantity cannot be negative"))]
    pub missing_qty: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub reason: Option<String>,
    #[validate(length(min = 1, max = 200, message = "Idempotency key is required"))]
    pub idempotency_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ReceiveInboundLine {
    #[validate(range(min = 1, message = "Invalid receiving location ID"))]
    pub to_location_id: i64,
    #[validate(range(min = 0, message = "Received quantity cannot be negative"))]
    pub received_qty: i64,
    #[validate(range(min = 0, message = "Rejected quantity cannot be negative"))]
    pub rejected_qty: i64,
    #[validate(range(min = 0, message = "Missing quantity cannot be negative"))]
    pub missing_qty: Option<i64>,
    pub license_plate_id: Option<i64>,
    pub license_plate_barcode: Option<String>,
    pub lot: Option<String>,
    pub serial: Option<String>,
    pub expiration: Option<Timestamp>,
    pub reason: Option<String>,
    #[validate(length(min = 1, max = 200, message = "Idempotency key is required"))]
    pub idempotency_key: String,
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
pub struct CreateItemLocationCycleCountTask {
    #[validate(range(min = 1, message = "Invalid location ID"))]
    pub location_id: i64,
    #[validate(range(min = 1, message = "Invalid item ID"))]
    pub item_id: i64,
    pub source: Option<String>,
    pub order_id: Option<i64>,
    pub order_item_id: Option<i64>,
    pub inventory_balance_id: Option<i64>,
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
pub struct ReceiveInventory {
    #[validate(range(min = 1, message = "Invalid item batch ID"))]
    pub item_batch_id: i64,
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
pub struct ReserveInventory {
    #[validate(range(min = 1, message = "Invalid order ID"))]
    pub order_id: i64,
    pub order_item_id: Option<i64>,
    #[validate(range(min = 1, message = "Invalid inventory balance ID"))]
    pub inventory_balance_id: i64,
    #[validate(range(min = 1, message = "Quantity must be positive"))]
    pub qty: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Validate)]
pub struct ReservationIdRequest {
    #[validate(range(min = 1, message = "Invalid reservation ID"))]
    pub reservation_id: i64,
}
