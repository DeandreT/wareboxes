//! Customer-facing visibility read models.
//!
//! These projections intentionally omit warehouse-internal locations, containers,
//! task state, employee identities, and persistence metadata.

use serde::{Deserialize, Serialize};
use wareboxes_domain::{OrderStatus, ShipmentDocumentType, ShipmentStatus, Timestamp};

pub const CUSTOMER_PORTAL_PERMISSION: &str = "customer_portal";
pub const MAX_CUSTOMER_PORTAL_RESULTS: i64 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerPortalQuery {
    pub inventory_owner_id: Option<i64>,
    pub facility_id: Option<i64>,
    pub search: Option<String>,
    pub include_history: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerPortalInventoryLine {
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub item_id: i64,
    pub item_description: Option<String>,
    pub primary_sku: Option<String>,
    pub lot: Option<String>,
    pub expiration: Option<Timestamp>,
    pub uom: String,
    pub status: String,
    pub on_hand: i64,
    pub reserved: i64,
    pub held: i64,
    pub available: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerPortalOrder {
    pub order_id: i64,
    pub order_key: String,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: Option<i64>,
    pub facility_name: Option<String>,
    pub status: OrderStatus,
    pub rush: bool,
    pub ordered_quantity: i64,
    pub ship_by: Option<Timestamp>,
    pub created_at: Timestamp,
    pub destination_company: Option<String>,
    pub destination_city: Option<String>,
    pub destination_region: Option<String>,
    pub destination_country: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerPortalShipment {
    pub shipment_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub inventory_owner_id: i64,
    pub inventory_owner_name: String,
    pub facility_id: i64,
    pub facility_name: String,
    pub status: ShipmentStatus,
    pub carton_count: i64,
    pub shipped_quantity: i64,
    pub carrier: Option<String>,
    pub service: Option<String>,
    pub tracking_numbers: Vec<String>,
    pub created_at: Timestamp,
    pub manifested_at: Option<Timestamp>,
    pub departed_at: Option<Timestamp>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerPortalDocument {
    pub document_id: i64,
    pub shipment_id: i64,
    pub order_id: i64,
    pub order_key: String,
    pub inventory_owner_id: i64,
    pub facility_id: i64,
    pub document_type: ShipmentDocumentType,
    pub file_name: String,
    pub media_type: String,
    pub content_length: i64,
    pub content_sha256: String,
    pub generated_at: Timestamp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomerPortalWorkspace {
    pub inventory: Vec<CustomerPortalInventoryLine>,
    pub orders: Vec<CustomerPortalOrder>,
    pub shipments: Vec<CustomerPortalShipment>,
    pub documents: Vec<CustomerPortalDocument>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomerPortalDocumentContent {
    pub document: CustomerPortalDocument,
    pub content: Vec<u8>,
}
