use axum::http::{Method, StatusCode};
use serde_json::json;
use wareboxes_api_contract::v1::{ConfigurationResponse, InventoryRotation};

use super::*;
use crate::common::tenant_tx;

impl PickShortageFixture {
    pub(crate) async fn activate_allocation_policy(
        &self,
        rotation: InventoryRotation,
        allow_partial: bool,
        require_complete_line: bool,
    ) -> ConfigurationResponse {
        let suffix = format!("shortage-policy-{}", self.order_id);
        grant_permissions(
            &self.fixture.db,
            self.access.tenant_id,
            self.access.user_id.get(),
            &suffix,
            &["admin"],
        )
        .await;
        let approver = self
            .fixture
            .user(&format!("{suffix}-approver@test.local"))
            .await;
        let mut tx = tenant_tx(&self.fixture.db, self.access.tenant_id).await;
        sqlx::query("INSERT INTO tenant_memberships(tenant_id,user_id) VALUES ($1,$2)")
            .bind(self.access.tenant_id.get())
            .bind(approver.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        grant_permissions(
            &self.fixture.db,
            self.access.tenant_id,
            approver.id,
            &format!("{suffix}-approver"),
            &["admin"],
        )
        .await;
        let approver_token = auth::create_session(&self.fixture.db, approver.id)
            .await
            .unwrap();

        let created = self
            .request(
                Method::POST,
                "/api/v1/configurations",
                Some(&format!("{suffix}-create")),
                Some(json!({
                    "scope": {
                        "level": "owner_facility",
                        "inventory_owner_id": self.inventory_owner_id,
                        "facility_id": self.facility_id
                    },
                    "effective_from": "2026-01-01T00:00:00Z",
                    "rule": {
                        "kind": "allocation",
                        "rotation": rotation,
                        "allow_partial": allow_partial,
                        "require_complete_line": require_complete_line
                    }
                })),
            )
            .await;
        let created = expect_status(created, StatusCode::OK, "create shortage policy").await;
        let created: ConfigurationResponse = response_json(created).await;
        let submitted = self
            .request(
                Method::POST,
                &format!(
                    "/api/v1/configurations/{}/submissions",
                    created.configuration_id
                ),
                Some(&format!("{suffix}-submit")),
                Some(json!({"expected_revision": created.revision})),
            )
            .await;
        let submitted = expect_status(submitted, StatusCode::OK, "submit shortage policy").await;
        let submitted: ConfigurationResponse = response_json(submitted).await;
        let approved = send(
            &self.app,
            &approver_token,
            self.access.tenant_id,
            Method::POST,
            &format!(
                "/api/v1/configurations/{}/approvals",
                created.configuration_id
            ),
            Some(&format!("{suffix}-approve")),
            Some(json!({"expected_revision": submitted.revision})),
        )
        .await;
        let approved = expect_status(approved, StatusCode::OK, "approve shortage policy").await;
        let approved: ConfigurationResponse = response_json(approved).await;
        let active = self
            .request(
                Method::POST,
                &format!(
                    "/api/v1/configurations/{}/activations",
                    created.configuration_id
                ),
                Some(&format!("{suffix}-activate")),
                Some(json!({"expected_revision": approved.revision})),
            )
            .await;
        let active = expect_status(active, StatusCode::OK, "activate shortage policy").await;
        response_json(active).await
    }

    pub(crate) async fn retire_allocation_policy(
        &self,
        policy: &ConfigurationResponse,
    ) -> ConfigurationResponse {
        let retired = self
            .request(
                Method::POST,
                &format!(
                    "/api/v1/configurations/{}/retirements",
                    policy.configuration_id
                ),
                Some(&format!("shortage-policy-{}-retire", self.order_id)),
                Some(json!({"expected_revision": policy.revision})),
            )
            .await;
        let retired = expect_status(retired, StatusCode::OK, "retire shortage policy").await;
        response_json(retired).await
    }
}
