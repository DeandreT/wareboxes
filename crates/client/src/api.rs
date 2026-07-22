//! Thin async API client. Uses `ehttp` (works on native *and* wasm) and
//! delivers results back to the egui app over an `mpsc` channel, requesting a
//! repaint on completion.

use std::sync::mpsc::Sender;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use wareboxes_core::dto::{ErrorResponse, OrderPage, SessionUser, UserSettings};
use wareboxes_core::models::{
    AuditWave, Employee, Facility, InventoryBalance, InventoryOwner, InventoryTransaction, Item,
    ItemBatch, LicensePlate, Load, Location, Order, Permission, Role, User,
};
use wareboxes_domain::TenantId;

#[derive(Debug)]
pub enum ApiEvent {
    LoggedIn(Box<SessionUser>),
    Users(Vec<User>),
    Roles(Vec<Role>),
    Permissions(Vec<Permission>),
    InventoryOwners(Vec<InventoryOwner>),
    Facilities(Vec<Facility>),
    Orders(OrderPage),
    OrderDetail(Order),
    Items(Vec<Item>),
    Locations(Vec<Location>),
    Loads(Vec<Load>),
    LoadChunk {
        loads: Vec<Load>,
        offset: usize,
        limit: usize,
    },
    LoadDetail(Load),
    ItemBatches(Vec<ItemBatch>),
    InventoryBalances(Vec<InventoryBalance>),
    InventoryTransactions(Vec<InventoryTransaction>),
    LicensePlates(Vec<LicensePlate>),
    LicensePlateLookup(Option<LicensePlate>),
    Employees(Vec<Employee>),
    Audits(Vec<AuditWave>),
    /// A mutation succeeded; carries a toast message + the screen to refresh.
    ActionDone(String, Screen),
    SettingsSaved(UserSettings),
    Error(String),
    LoggedOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Screen {
    Orders,
    Users,
    Roles,
    Permissions,
    InventoryOwners,
    Facilities,
    Items,
    Locations,
    Loads,
    Inventory,
    LicensePlates,
    Employees,
    Audits,
}

impl Screen {
    pub const ALL: [Screen; 13] = [
        Screen::Orders,
        Screen::Items,
        Screen::Locations,
        Screen::Loads,
        Screen::Inventory,
        Screen::LicensePlates,
        Screen::Users,
        Screen::Roles,
        Screen::Permissions,
        Screen::InventoryOwners,
        Screen::Facilities,
        Screen::Employees,
        Screen::Audits,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Screen::Orders => "Orders",
            Screen::Users => "Users",
            Screen::Roles => "Roles",
            Screen::Permissions => "Permissions",
            Screen::InventoryOwners => "Inventory Owners",
            Screen::Facilities => "Facilities",
            Screen::Items => "Item Master",
            Screen::Locations => "Locations",
            Screen::Loads => "Loads",
            Screen::Inventory => "Inventory",
            Screen::LicensePlates => "License Plates",
            Screen::Employees => "Employees",
            Screen::Audits => "Audits",
        }
    }

    /// Permission required to see the panel.
    pub fn permission(self) -> &'static str {
        match self {
            Screen::Orders => "orders",
            Screen::Items
            | Screen::Locations
            | Screen::Loads
            | Screen::Inventory
            | Screen::LicensePlates => "wms",
            _ => "admin",
        }
    }
}

#[derive(Clone)]
pub struct ApiClient {
    pub base_url: String,
    pub token: Option<String>,
    pub tenant_id: Option<TenantId>,
    tx: Sender<ApiEvent>,
    ctx: egui::Context,
}

impl ApiClient {
    pub fn new(base_url: String, tx: Sender<ApiEvent>, ctx: egui::Context) -> Self {
        Self {
            base_url,
            token: None,
            tenant_id: None,
            tx,
            ctx,
        }
    }

    fn headers(&self) -> ehttp::Headers {
        let mut h = ehttp::Headers::new(&[("Content-Type", "application/json")]);
        if let Some(t) = &self.token {
            h.insert("Authorization", format!("Bearer {t}"));
        }
        if let Some(tenant_id) = self.tenant_id {
            h.insert("X-Wareboxes-Tenant-Id", tenant_id.to_string());
        }
        h
    }

    /// Fire a request and hand the decoded payload to `on_ok`, or surface a
    /// unified error event from the HTTP status and error body.
    fn send<T, F>(&self, mut req: ehttp::Request, on_ok: F)
    where
        T: DeserializeOwned + 'static,
        F: FnOnce(T) -> ApiEvent + Send + 'static,
    {
        req.headers = self.headers();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        ehttp::fetch(req, move |res| {
            let ev = match res {
                Err(e) => ApiEvent::Error(e),
                Ok(resp) if (200..300).contains(&resp.status) => {
                    match serde_json::from_slice::<T>(&resp.bytes) {
                        Ok(data) => on_ok(data),
                        Err(_) => ApiEvent::Error(format!(
                            "HTTP {} — {}",
                            resp.status,
                            String::from_utf8_lossy(&resp.bytes)
                        )),
                    }
                }
                Ok(resp) => {
                    let message = serde_json::from_slice::<ErrorResponse>(&resp.bytes)
                        .ok()
                        .map(|body| body.errors.join("; "))
                        .filter(|message| !message.is_empty())
                        .unwrap_or_else(|| String::from_utf8_lossy(&resp.bytes).to_string());
                    ApiEvent::Error(format!("HTTP {} — {message}", resp.status))
                }
            };
            let _ = tx.send(ev);
            ctx.request_repaint();
        });
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url.trim_end_matches('/'), path)
    }

    pub fn login(&self, email: &str, password: &str) {
        let body = serde_json::json!({ "email": email, "password": password });
        let req = ehttp::Request::post(self.url("/api/auth/login"), body.to_string().into_bytes());
        self.send::<SessionUser, _>(req, |s| ApiEvent::LoggedIn(Box::new(s)));
    }

    pub fn logout(&self) {
        let req = ehttp::Request::post(self.url("/api/auth/logout"), Vec::new());
        // Fire-and-forget; we drop the session client-side regardless.
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let mut req = req;
        req.headers = self.headers();
        ehttp::fetch(req, move |_| {
            let _ = tx.send(ApiEvent::LoggedOut);
            ctx.request_repaint();
        });
    }

    pub fn get_list(&self, screen: Screen) {
        let path = match screen {
            Screen::Orders => "/api/orders",
            Screen::Users => "/api/users",
            Screen::Roles => "/api/roles?show_self=true",
            Screen::Permissions => "/api/permissions",
            Screen::InventoryOwners => "/api/inventory-owners",
            Screen::Facilities => "/api/facilities",
            Screen::Items => "/api/items",
            Screen::Locations => "/api/locations",
            Screen::Loads => "/api/loads",
            Screen::Inventory => "/api/inventory/batches",
            Screen::LicensePlates => "/api/license-plates",
            Screen::Employees => "/api/employees",
            Screen::Audits => "/api/audits",
        };
        let req = ehttp::Request::get(self.url(path));
        match screen {
            Screen::Orders => self.get_orders_page(100, 0, None, None),
            Screen::Users => self.send::<Vec<User>, _>(req, ApiEvent::Users),
            Screen::Roles => self.send::<Vec<Role>, _>(req, ApiEvent::Roles),
            Screen::Permissions => self.send::<Vec<Permission>, _>(req, ApiEvent::Permissions),
            Screen::InventoryOwners => {
                self.send::<Vec<InventoryOwner>, _>(req, ApiEvent::InventoryOwners)
            }
            Screen::Facilities => self.send::<Vec<Facility>, _>(req, ApiEvent::Facilities),
            Screen::Items => self.send::<Vec<Item>, _>(req, ApiEvent::Items),
            Screen::Locations => self.send::<Vec<Location>, _>(req, ApiEvent::Locations),
            Screen::Loads => self.send::<Vec<Load>, _>(req, ApiEvent::Loads),
            Screen::Inventory => self.send::<Vec<ItemBatch>, _>(req, ApiEvent::ItemBatches),
            Screen::LicensePlates => {
                self.send::<Vec<LicensePlate>, _>(req, ApiEvent::LicensePlates)
            }
            Screen::Employees => self.send::<Vec<Employee>, _>(req, ApiEvent::Employees),
            Screen::Audits => self.send::<Vec<AuditWave>, _>(req, ApiEvent::Audits),
        }
    }

    pub fn get_inventory_transactions(&self) {
        let req = ehttp::Request::get(self.url("/api/inventory/transactions"));
        self.send::<Vec<InventoryTransaction>, _>(req, ApiEvent::InventoryTransactions);
    }

    pub fn get_inventory_balances(&self) {
        let req = ehttp::Request::get(self.url("/api/inventory/balances"));
        self.send::<Vec<InventoryBalance>, _>(req, ApiEvent::InventoryBalances);
    }

    pub fn get_orders_page(
        &self,
        limit: i64,
        offset: i64,
        search: Option<&str>,
        status: Option<&str>,
    ) {
        let mut params = vec![
            format!("limit={}", limit.max(1)),
            format!("offset={}", offset.max(0)),
        ];
        if let Some(search) = search.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("search={}", Self::query_encode(search)));
        }
        if let Some(status) = status.map(str::trim).filter(|value| !value.is_empty()) {
            params.push(format!("status={}", Self::query_encode(status)));
        }
        let req = ehttp::Request::get(self.url(&format!("/api/orders?{}", params.join("&"))));
        self.send::<OrderPage, _>(req, ApiEvent::Orders);
    }

    pub fn get_loads_chunk(&self, offset: usize, limit: usize) {
        let req =
            ehttp::Request::get(self.url(&format!("/api/loads?offset={offset}&limit={limit}")));
        self.send::<Vec<Load>, _>(req, move |loads| ApiEvent::LoadChunk {
            loads,
            offset,
            limit,
        });
    }

    pub fn get_license_plate_by_barcode(&self, barcode: &str) {
        let encoded = barcode.trim().replace('/', "%2F");
        let req = ehttp::Request::get(self.url(&format!("/api/license-plates/barcode/{encoded}")));
        self.send::<Option<LicensePlate>, _>(req, ApiEvent::LicensePlateLookup);
    }

    pub fn get_load_detail(&self, load_id: i64) {
        let req = ehttp::Request::get(self.url(&format!("/api/loads/{load_id}")));
        self.send::<Option<Load>, _>(req, move |load| match load {
            Some(load) => ApiEvent::LoadDetail(load),
            None => ApiEvent::Error(format!("Load #{load_id} not found")),
        });
    }

    pub fn get_order_detail(&self, order_id: i64) {
        let req = ehttp::Request::get(self.url(&format!("/api/orders/{order_id}")));
        self.send::<Option<Order>, _>(req, move |order| match order {
            Some(order) => ApiEvent::OrderDetail(order),
            None => ApiEvent::Error(format!("Order #{order_id} not found")),
        });
    }

    pub fn save_settings(&self, light_mode: bool) {
        let body = serde_json::json!({ "light_mode": light_mode });
        let req = ehttp::Request::post(
            self.url("/api/auth/settings"),
            body.to_string().into_bytes(),
        );
        self.send::<UserSettings, _>(req, ApiEvent::SettingsSaved);
    }

    /// POST a JSON action and, on success, emit `ActionDone` so the caller can
    /// toast + refresh the originating screen.
    pub fn action(&self, path: &str, body: Value, refresh: Screen, msg: &str) {
        let req = ehttp::Request::post(self.url(path), body.to_string().into_bytes());
        let msg = msg.to_string();
        self.send::<Value, _>(req, move |value| {
            if value == Value::Bool(false) {
                ApiEvent::Error("Action was not applied".to_owned())
            } else {
                ApiEvent::ActionDone(msg, refresh)
            }
        });
    }

    fn query_encode(value: &str) -> String {
        let mut encoded = String::new();
        for byte in value.bytes() {
            match byte {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    encoded.push(byte as char);
                }
                _ => encoded.push_str(&format!("%{byte:02X}")),
            }
        }
        encoded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authenticated_headers_include_selected_tenant() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut client =
            ApiClient::new("http://localhost".to_owned(), tx, egui::Context::default());
        client.token = Some("session-token".to_owned());
        client.tenant_id = Some(TenantId::new(42).unwrap());

        let headers = client.headers();
        assert_eq!(headers.get("Authorization"), Some("Bearer session-token"));
        assert_eq!(headers.get("X-Wareboxes-Tenant-Id"), Some("42"));
    }
}
