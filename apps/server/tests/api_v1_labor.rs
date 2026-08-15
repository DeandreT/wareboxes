mod common;
#[path = "api_v1_labor/corrections_and_concurrency.rs"]
mod corrections_and_concurrency;

use std::time::Duration;

use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use common::*;
use serde::Serialize;
use tower::ServiceExt;
use wareboxes_api::auth::TENANT_ID_HEADER;
use wareboxes_api::request_context::IDEMPOTENCY_KEY_HEADER;
use wareboxes_api::{routes, state::AppState};
use wareboxes_api_contract::v1::{
    AttendanceAdjustmentResponse, AttendanceIntervalResponse, AttendanceStatus,
    CancelLaborActivityRequest, CertifyEmployeeRequest, ChangeEquipmentStatusRequest,
    ClockInRequest, ClockOutRequest, CompleteLaborActivityRequest, ConfigureEquipmentClassRequest,
    ConfigureLaborSkillRequest, ConfigureLaborStandardRequest, CorrectAttendanceRequest,
    CorrectLaborActivityRequest, CreateEquipmentAssetRequest, EmployeeCertificationResponse,
    EquipmentAssetResponse, EquipmentClassResponse, EquipmentStatus,
    LaborActivityAdjustmentResponse, LaborActivityKind, LaborActivityResponse, LaborActivityStatus,
    LaborCorrectionReason, LaborExceptionReason, LaborQuantityBasis, LaborSkillResponse,
    LaborStandardResponse, LaborWorkspaceResponse, RevokeEmployeeCertificationRequest,
    StartLaborActivityRequest,
};
use wareboxes_core::dto::UpdateUserAccessScope;

const SUPERVISOR_PERMISSIONS: &[&str] = &[
    "admin",
    "labor_view",
    "labor_configure",
    "labor_certify",
    "labor_equipment",
    "labor_supervise",
    "wms",
];
const OPERATOR_PERMISSIONS: &[&str] = &["labor_view", "labor_execute", "wms"];

struct Rig {
    fixture: Fixture,
    tenant_id: TenantId,
    supervisor_id: i64,
    supervisor_token: String,
    operator_id: i64,
    operator_token: String,
    facility_id: i64,
    hidden_facility_id: i64,
    owner_id: i64,
    operator_employee_id: i64,
    other_employee_id: i64,
    hidden_employee_id: i64,
    app: axum::Router,
}

struct ConfiguredLabor {
    skill: LaborSkillResponse,
    class: EquipmentClassResponse,
    asset: EquipmentAssetResponse,
    standard: LaborStandardResponse,
    certification: EmployeeCertificationResponse,
}

impl Rig {
    async fn new(suffix: &str) -> Self {
        let fixture = Fixture::new().await;
        let supervisor = fixture
            .user(&format!("labor-supervisor-{suffix}@test.local"))
            .await;
        let tenant_id = tenant_for_user(&fixture.db, supervisor.id).await;
        let operator = fixture
            .user(&format!("labor-operator-{suffix}@test.local"))
            .await;

        let mut membership_tx = tenant_tx(&fixture.db, tenant_id).await;
        sqlx::query("INSERT INTO tenant_memberships (tenant_id,user_id) VALUES($1,$2)")
            .bind(tenant_id.get())
            .bind(operator.id)
            .execute(&mut *membership_tx)
            .await
            .unwrap();
        membership_tx.commit().await.unwrap();

        grant_permissions(
            &fixture,
            tenant_id,
            supervisor.id,
            &format!("labor-supervisor-{suffix}"),
            SUPERVISOR_PERMISSIONS,
        )
        .await;
        grant_permissions(
            &fixture,
            tenant_id,
            operator.id,
            &format!("labor-operator-{suffix}"),
            OPERATOR_PERMISSIONS,
        )
        .await;

        let facility_id = fixture
            .facility(tenant_id, &format!("Labor Facility {suffix}"))
            .await;
        let hidden_facility_id = fixture
            .facility(tenant_id, &format!("Hidden Labor Facility {suffix}"))
            .await;
        let owner_id = fixture
            .inventory_owner(tenant_id, &format!("Labor Owner {suffix}"))
            .await;
        fixture
            .assign_owner_to_facility(tenant_id, owner_id, facility_id)
            .await;

        let supervisor_access =
            repo::tenants::access_for_user(&fixture.db, supervisor.id, tenant_id)
                .await
                .unwrap()
                .unwrap();
        let hired = db::now_iso() - Duration::from_secs(86_400);
        let operator_employee_id = repo::employees::add_employee(
            &fixture.db,
            tenant_id,
            &supervisor_access.site_scope,
            &repo::employees::NewEmployee {
                first_name: "Alex",
                last_name: "Operator",
                title: "Warehouse Associate",
                employee_type: "hourly",
                email: Some(&operator.email),
                phone: None,
                hired,
                facility_ids: &[facility_id],
            },
        )
        .await
        .unwrap();
        let other_employee_id = repo::employees::add_employee(
            &fixture.db,
            tenant_id,
            &supervisor_access.site_scope,
            &repo::employees::NewEmployee {
                first_name: "Morgan",
                last_name: "Associate",
                title: "Warehouse Associate",
                employee_type: "hourly",
                email: None,
                phone: None,
                hired,
                facility_ids: &[facility_id],
            },
        )
        .await
        .unwrap();
        let hidden_employee_id = repo::employees::add_employee(
            &fixture.db,
            tenant_id,
            &supervisor_access.site_scope,
            &repo::employees::NewEmployee {
                first_name: "Hidden",
                last_name: "Associate",
                title: "Warehouse Associate",
                employee_type: "hourly",
                email: None,
                phone: None,
                hired,
                facility_ids: &[hidden_facility_id],
            },
        )
        .await
        .unwrap();
        assert!(repo::tenants::update_user_access_scope(
            &fixture.db,
            tenant_id,
            &UpdateUserAccessScope {
                user_id: operator.id,
                all_facilities: false,
                facility_ids: vec![facility_id],
                // Facility-shared count tasks intentionally carry no owner dimension, so an
                // executor must have the tenant's owner-neutral task scope.
                all_inventory_owners: true,
                inventory_owner_ids: Vec::new(),
            },
        )
        .await
        .unwrap());
        repo::employees::link_employee_identity(
            &fixture.db,
            &supervisor_access,
            &wareboxes_application::CommandContext {
                tenant_id,
                actor_id: wareboxes_domain::UserId::new(supervisor.id).unwrap(),
                request_id: format!("labor-identity-link-{suffix}"),
                idempotency_key: Some(format!("labor-identity-link-{suffix}")),
            },
            &wareboxes_application::workforce_identity::LinkEmployeeIdentityCommand {
                employee_id: wareboxes_domain::EmployeeId::new(operator_employee_id).unwrap(),
                user_id: wareboxes_domain::UserId::new(operator.id).unwrap(),
                expected_user_id: None,
                reason: wareboxes_domain::EmployeeIdentityReason::new(
                    "enable labor self-service acceptance workflow",
                )
                .unwrap(),
            },
        )
        .await
        .unwrap();

        let supervisor_token = wareboxes_api::auth::create_session(&fixture.db, supervisor.id)
            .await
            .unwrap();
        let operator_token = wareboxes_api::auth::create_session(&fixture.db, operator.id)
            .await
            .unwrap();
        let app = routes::app(AppState::new(fixture.db.clone()));
        Self {
            fixture,
            tenant_id,
            supervisor_id: supervisor.id,
            supervisor_token,
            operator_id: operator.id,
            operator_token,
            facility_id,
            hidden_facility_id,
            owner_id,
            operator_employee_id,
            other_employee_id,
            hidden_employee_id,
            app,
        }
    }

    async fn send<T: Serialize>(
        &self,
        token: &str,
        method: Method,
        path: &str,
        key: Option<&str>,
        body: Option<&T>,
    ) -> axum::response::Response {
        self.app
            .clone()
            .oneshot(request(token, self.tenant_id, method, path, key, body))
            .await
            .unwrap()
    }

    async fn configure(&self, suffix: &str) -> ConfiguredLabor {
        let skill_request = ConfigureLaborSkillRequest {
            code: format!("FORKLIFT-{suffix}"),
            name: "Powered industrial truck".into(),
            certification_required: true,
        };
        let skill: LaborSkillResponse = json(
            self.send(
                &self.supervisor_token,
                Method::POST,
                "/api/v1/labor/skills",
                Some(&format!("labor-skill-{suffix}")),
                Some(&skill_request),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(skill.configured_by, self.supervisor_id);
        assert_eq!(skill.revision.get(), 1);

        let class: EquipmentClassResponse = json(
            self.send(
                &self.supervisor_token,
                Method::POST,
                "/api/v1/labor/equipment-classes",
                Some(&format!("labor-class-{suffix}")),
                Some(&ConfigureEquipmentClassRequest {
                    code: format!("REACH-{suffix}"),
                    name: "Reach truck".into(),
                    required_skill_id: Some(skill.skill_id),
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(class.required_skill_id, Some(skill.skill_id));

        let asset: EquipmentAssetResponse = json(
            self.send(
                &self.supervisor_token,
                Method::POST,
                "/api/v1/labor/equipment-assets",
                Some(&format!("labor-asset-{suffix}")),
                Some(&CreateEquipmentAssetRequest {
                    facility_id: self.facility_id,
                    equipment_class_id: class.equipment_class_id,
                    equipment_number: format!("RT-{suffix}"),
                    name: "Reach Truck".into(),
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(asset.status, EquipmentStatus::Available);

        let now = db::now_iso();
        let certification: EmployeeCertificationResponse = json(
            self.send(
                &self.supervisor_token,
                Method::POST,
                "/api/v1/labor/certifications",
                Some(&format!("labor-certification-{suffix}")),
                Some(&CertifyEmployeeRequest {
                    employee_id: self.operator_employee_id,
                    skill_id: skill.skill_id,
                    facility_id: self.facility_id,
                    certification_number: Some(format!("CERT-{suffix}")),
                    issued_at: (now - Duration::from_secs(86_400)).to_rfc3339(),
                    expires_at: Some((now + Duration::from_secs(86_400)).to_rfc3339()),
                    note: Some("Training and practical evaluation complete".into()),
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(certification.certified_by, self.supervisor_id);
        assert!(certification.revoked_at.is_none());

        let standard: LaborStandardResponse = json(
            self.send(
                &self.supervisor_token,
                Method::POST,
                "/api/v1/labor/standards",
                Some(&format!("labor-standard-{suffix}")),
                Some(&ConfigureLaborStandardRequest {
                    facility_id: self.facility_id,
                    inventory_owner_id: None,
                    code: format!("COUNT-{suffix}"),
                    name: "Location count standard".into(),
                    activity_kind: LaborActivityKind::CycleCount,
                    quantity_basis: LaborQuantityBasis::Task,
                    setup_seconds: 10,
                    seconds_per_unit: 5,
                    required_skill_id: Some(skill.skill_id),
                    required_equipment_class_id: Some(class.equipment_class_id),
                    effective_from: (now - Duration::from_secs(3_600)).to_rfc3339(),
                    effective_until: None,
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await;
        assert_eq!(standard.revision.get(), 1);
        assert!(standard.supersedes_standard_id.is_none());
        assert_eq!(standard.configured_by, self.supervisor_id);

        ConfiguredLabor {
            skill,
            class,
            asset,
            standard,
            certification,
        }
    }

    async fn assigned_location_count_task(&self, suffix: &str) -> i64 {
        let location_id = self
            .fixture
            .location(self.tenant_id, self.facility_id, &format!("COUNT-{suffix}"))
            .await;
        let task_id = repo::tasks::create_location_cycle_count_task(
            &self.fixture.db,
            self.tenant_id,
            self.supervisor_id,
            location_id,
            Some(25),
            Some(self.operator_id),
            None,
            None,
            Some(format!("Count location {suffix}")),
        )
        .await
        .unwrap();
        let access =
            repo::tenants::access_for_user(&self.fixture.db, self.operator_id, self.tenant_id)
                .await
                .unwrap()
                .unwrap();
        let started = repo::tasks::start_task_in_scope(
            &self.fixture.db,
            &access,
            &wareboxes_application::CommandContext {
                tenant_id: self.tenant_id,
                actor_id: wareboxes_domain::UserId::new(self.operator_id).unwrap(),
                request_id: format!("labor-task-claim-{suffix}"),
                idempotency_key: Some(format!("labor-task-claim-{suffix}")),
            },
            task_id,
        )
        .await
        .unwrap();
        assert!(started, "labor source task should be claimable");
        task_id
    }

    async fn clock_in_operator(&self, key: &str) -> AttendanceIntervalResponse {
        json(
            self.send(
                &self.operator_token,
                Method::POST,
                "/api/v1/labor/attendance",
                Some(key),
                Some(&ClockInRequest {
                    employee_id: self.operator_employee_id,
                    facility_id: self.facility_id,
                    note: Some("Shift start".into()),
                }),
            )
            .await,
            StatusCode::OK,
        )
        .await
    }
}

#[tokio::test]
async fn standardized_self_service_shift_is_replay_safe_audited_and_explainable() {
    let rig = Rig::new("lifecycle").await;
    let configured = rig.configure("lifecycle").await;

    let skill_replay: LaborSkillResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            "/api/v1/labor/skills",
            Some("labor-skill-lifecycle"),
            Some(&ConfigureLaborSkillRequest {
                code: "FORKLIFT-lifecycle".into(),
                name: "Powered industrial truck".into(),
                certification_required: true,
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(skill_replay, configured.skill);
    let reused = rig
        .send(
            &rig.supervisor_token,
            Method::POST,
            "/api/v1/labor/skills",
            Some("labor-skill-lifecycle"),
            Some(&ConfigureLaborSkillRequest {
                code: "FORKLIFT-lifecycle".into(),
                name: "Changed request".into(),
                certification_required: true,
            }),
        )
        .await;
    assert_status(reused, StatusCode::CONFLICT).await;

    let attendance = rig.clock_in_operator("labor-clock-in-lifecycle").await;
    assert_eq!(attendance.employee_id, rig.operator_employee_id);
    assert_eq!(attendance.clocked_in_by, rig.operator_id);
    assert_eq!(attendance.status, AttendanceStatus::Open);

    let task_id = rig.assigned_location_count_task("lifecycle").await;
    let start_request = StartLaborActivityRequest {
        attendance_interval_id: attendance.attendance_interval_id,
        inventory_owner_id: None,
        activity_kind: LaborActivityKind::CycleCount,
        quantity_basis: Some(LaborQuantityBasis::Task),
        labor_standard_id: Some(configured.standard.labor_standard_id),
        equipment_asset_id: Some(configured.asset.equipment_asset_id),
        reference_type: Some("work_task".into()),
        reference_id: Some(task_id),
        note: Some("Begin assigned location count".into()),
    };
    let started: LaborActivityResponse = json(
        rig.send(
            &rig.operator_token,
            Method::POST,
            "/api/v1/labor/activities",
            Some("labor-start-lifecycle"),
            Some(&start_request),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(started.status, LaborActivityStatus::Active);
    assert_eq!(started.inventory_owner_id, None);
    assert_eq!(started.started_by, rig.operator_id);
    assert_eq!(started.required_skill_id, Some(configured.skill.skill_id));
    assert_eq!(
        started.required_equipment_class_id,
        Some(configured.class.equipment_class_id)
    );
    assert_eq!(
        started.equipment_required_skill_id,
        Some(configured.skill.skill_id)
    );
    assert_eq!(started.standard_setup_seconds, Some(10));
    assert_eq!(started.standard_seconds_per_unit, Some(5));
    assert_eq!(started.quantity_basis, Some(LaborQuantityBasis::Task));
    assert_eq!(started.reference_quantity, Some(1));

    let replay: LaborActivityResponse = json(
        rig.send(
            &rig.operator_token,
            Method::POST,
            "/api/v1/labor/activities",
            Some("labor-start-lifecycle"),
            Some(&start_request),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(replay, started);
    let mut changed_start = start_request.clone();
    changed_start.note = Some("Different retry payload".into());
    assert_status(
        rig.send(
            &rig.operator_token,
            Method::POST,
            "/api/v1/labor/activities",
            Some("labor-start-lifecycle"),
            Some(&changed_start),
        )
        .await,
        StatusCode::CONFLICT,
    )
    .await;

    assert_status(
        rig.send(
            &rig.operator_token,
            Method::POST,
            &format!(
                "/api/v1/labor/activities/{}/completions",
                started.labor_activity_id
            ),
            Some("labor-complete-fabricated-quantity"),
            Some(&CompleteLaborActivityRequest {
                expected_revision: started.revision,
                quantity: Some(2),
                exception_seconds: 0,
                exception_reason: None,
                exception_note: None,
                note: Some("Attempt to overstate completed work".into()),
            }),
        )
        .await,
        StatusCode::CONFLICT,
    )
    .await;

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let completed: LaborActivityResponse = json(
        rig.send(
            &rig.operator_token,
            Method::POST,
            &format!(
                "/api/v1/labor/activities/{}/completions",
                started.labor_activity_id
            ),
            Some("labor-complete-lifecycle"),
            Some(&CompleteLaborActivityRequest {
                expected_revision: started.revision,
                quantity: Some(1),
                exception_seconds: 0,
                exception_reason: None,
                exception_note: None,
                note: Some("Count complete".into()),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(completed.status, LaborActivityStatus::Completed);
    assert_eq!(completed.quantity, Some(1));
    assert_eq!(completed.expected_seconds, Some(15));
    assert!(completed.actual_seconds.is_some_and(|seconds| seconds >= 1));
    assert_eq!(completed.completed_by, Some(rig.operator_id));
    assert!(completed.efficiency_basis_points.is_some());

    let closed: AttendanceIntervalResponse = json(
        rig.send(
            &rig.operator_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/clock-outs",
                attendance.attendance_interval_id
            ),
            Some("labor-clock-out-lifecycle"),
            Some(&ClockOutRequest {
                expected_revision: attendance.revision,
                note: Some("Shift complete".into()),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(closed.status, AttendanceStatus::Closed);
    assert_eq!(closed.clocked_out_by, Some(rig.operator_id));
    assert!(closed.paid_seconds.is_some_and(|seconds| seconds >= 1));

    let successor_from = db::now_iso() + Duration::from_secs(3_600);
    let successor: LaborStandardResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            "/api/v1/labor/standards",
            Some("labor-standard-lifecycle-v2"),
            Some(&ConfigureLaborStandardRequest {
                facility_id: rig.facility_id,
                inventory_owner_id: None,
                code: "COUNT-lifecycle".into(),
                name: "Location count standard v2".into(),
                activity_kind: LaborActivityKind::CycleCount,
                quantity_basis: LaborQuantityBasis::Task,
                setup_seconds: 8,
                seconds_per_unit: 4,
                required_skill_id: Some(configured.skill.skill_id),
                required_equipment_class_id: Some(configured.class.equipment_class_id),
                effective_from: successor_from.to_rfc3339(),
                effective_until: None,
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(successor.revision.get(), 2);
    assert_eq!(
        successor.supersedes_standard_id,
        Some(configured.standard.labor_standard_id)
    );
    assert_eq!(successor.configured_by, rig.supervisor_id);

    let workspace: LaborWorkspaceResponse = json(
        rig.send::<serde_json::Value>(
            &rig.supervisor_token,
            Method::GET,
            &format!(
                "/api/v1/labor/workspace?facility_id={}&include_history=true",
                rig.facility_id
            ),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let workspace_activity = workspace
        .activities
        .iter()
        .find(|activity| activity.labor_activity_id == completed.labor_activity_id)
        .unwrap();
    assert_eq!(workspace_activity, &completed);
    let retired_standard = workspace
        .standards
        .iter()
        .find(|standard| standard.labor_standard_id == configured.standard.labor_standard_id)
        .unwrap();
    assert_eq!(retired_standard.retired_by, Some(rig.supervisor_id));
    assert!(retired_standard.retired_at.is_some());
    let summary = workspace
        .summaries
        .iter()
        .find(|summary| summary.employee_id == rig.operator_employee_id)
        .unwrap();
    assert!(summary.paid_seconds >= 1);
    assert!(summary.direct_seconds >= 1);
    assert_eq!(summary.expected_seconds, 15);
    let available_asset = workspace
        .equipment_assets
        .iter()
        .find(|asset| asset.equipment_asset_id == configured.asset.equipment_asset_id)
        .unwrap();
    assert_eq!(available_asset.status, EquipmentStatus::Available);
    assert_eq!(available_asset.assigned_employee_id, None);
    assert_eq!(available_asset.status_changed_by, Some(rig.operator_id));

    let out_of_service: EquipmentAssetResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/equipment-assets/{}/status-changes",
                available_asset.equipment_asset_id
            ),
            Some("labor-equipment-oos-lifecycle"),
            Some(&ChangeEquipmentStatusRequest {
                expected_revision: available_asset.revision,
                status: EquipmentStatus::OutOfService,
                note: "Scheduled maintenance".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(out_of_service.status, EquipmentStatus::OutOfService);
    assert_eq!(out_of_service.status_changed_by, Some(rig.supervisor_id));
    assert_eq!(
        out_of_service.status_note.as_deref(),
        Some("Scheduled maintenance")
    );

    let revoked: EmployeeCertificationResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/certifications/{}/revocations",
                configured.certification.certification_id
            ),
            Some("labor-certification-revoke-lifecycle"),
            Some(&RevokeEmployeeCertificationRequest {
                expected_revision: configured.certification.revision,
                note: "Certification suspended pending refresher".into(),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(revoked.revoked_by, Some(rig.supervisor_id));
    assert!(revoked.revoked_at.is_some());
    assert_eq!(
        revoked.revocation_note.as_deref(),
        Some("Certification suspended pending refresher")
    );
}

#[tokio::test]
async fn execute_permission_is_self_only_and_scopes_are_concealed_before_labor_access() {
    let rig = Rig::new("authorization").await;

    let forbidden = rig
        .send(
            &rig.operator_token,
            Method::POST,
            "/api/v1/labor/attendance",
            Some("labor-clock-other-forbidden"),
            Some(&ClockInRequest {
                employee_id: rig.other_employee_id,
                facility_id: rig.facility_id,
                note: None,
            }),
        )
        .await;
    assert_status(forbidden, StatusCode::FORBIDDEN).await;

    let other_attendance: AttendanceIntervalResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            "/api/v1/labor/attendance",
            Some("labor-clock-other-supervisor"),
            Some(&ClockInRequest {
                employee_id: rig.other_employee_id,
                facility_id: rig.facility_id,
                note: Some("Supervisor-entered shift".into()),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(other_attendance.clocked_in_by, rig.supervisor_id);

    let break_activity: LaborActivityResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            "/api/v1/labor/activities",
            Some("labor-break-supervisor"),
            Some(&StartLaborActivityRequest {
                attendance_interval_id: other_attendance.attendance_interval_id,
                inventory_owner_id: None,
                activity_kind: LaborActivityKind::Break,
                quantity_basis: None,
                labor_standard_id: None,
                equipment_asset_id: None,
                reference_type: None,
                reference_id: None,
                note: Some("Scheduled break".into()),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(break_activity.started_by, rig.supervisor_id);

    let forbidden_cancel = rig
        .send(
            &rig.operator_token,
            Method::POST,
            &format!(
                "/api/v1/labor/activities/{}/cancellations",
                break_activity.labor_activity_id
            ),
            Some("labor-break-cancel-forbidden"),
            Some(&CancelLaborActivityRequest {
                expected_revision: break_activity.revision,
                note: "Attempt by a different employee".into(),
            }),
        )
        .await;
    assert_status(forbidden_cancel, StatusCode::FORBIDDEN).await;

    let hidden_attendance: AttendanceIntervalResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            "/api/v1/labor/attendance",
            Some("labor-clock-hidden-supervisor"),
            Some(&ClockInRequest {
                employee_id: rig.hidden_employee_id,
                facility_id: rig.hidden_facility_id,
                note: None,
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let concealed = rig
        .send(
            &rig.operator_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/clock-outs",
                hidden_attendance.attendance_interval_id
            ),
            Some("labor-clock-hidden-concealed"),
            Some(&ClockOutRequest {
                expected_revision: hidden_attendance.revision,
                note: None,
            }),
        )
        .await;
    assert_status(concealed, StatusCode::NOT_FOUND).await;
    let concealed_workspace = rig
        .send::<serde_json::Value>(
            &rig.operator_token,
            Method::GET,
            &format!(
                "/api/v1/labor/workspace?facility_id={}",
                rig.hidden_facility_id
            ),
            None,
            None,
        )
        .await;
    assert_status(concealed_workspace, StatusCode::NOT_FOUND).await;

    let visible_workspace: LaborWorkspaceResponse = json(
        rig.send::<serde_json::Value>(
            &rig.operator_token,
            Method::GET,
            &format!("/api/v1/labor/workspace?facility_id={}", rig.facility_id),
            None,
            None,
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert!(visible_workspace
        .attendance
        .iter()
        .any(|attendance| attendance.employee_id == rig.other_employee_id));
    assert!(!visible_workspace
        .attendance
        .iter()
        .any(|attendance| attendance.employee_id == rig.hidden_employee_id));

    tokio::time::sleep(Duration::from_millis(1_100)).await;
    let completed_with_exception: LaborActivityResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/activities/{}/completions",
                break_activity.labor_activity_id
            ),
            Some("labor-break-complete-supervisor"),
            Some(&CompleteLaborActivityRequest {
                expected_revision: break_activity.revision,
                quantity: None,
                exception_seconds: 1,
                exception_reason: Some(LaborExceptionReason::Safety),
                exception_note: Some("Emergency egress briefly blocked".into()),
                note: Some("Supervisor-approved safety delay".into()),
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(
        completed_with_exception.status,
        LaborActivityStatus::Completed
    );
    assert_eq!(completed_with_exception.exception_seconds, Some(1));
    assert_eq!(
        completed_with_exception.exception_reason,
        Some(LaborExceptionReason::Safety)
    );
    assert_eq!(
        completed_with_exception.exception_note.as_deref(),
        Some("Emergency egress briefly blocked")
    );
    assert_eq!(
        completed_with_exception.exception_approved_by,
        Some(rig.supervisor_id)
    );

    let closed_other: AttendanceIntervalResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/clock-outs",
                other_attendance.attendance_interval_id
            ),
            Some("labor-clock-other-out"),
            Some(&ClockOutRequest {
                expected_revision: other_attendance.revision,
                note: None,
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(closed_other.clocked_out_by, Some(rig.supervisor_id));

    let closed_hidden: AttendanceIntervalResponse = json(
        rig.send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/clock-outs",
                hidden_attendance.attendance_interval_id
            ),
            Some("labor-clock-hidden-out"),
            Some(&ClockOutRequest {
                expected_revision: hidden_attendance.revision,
                note: None,
            }),
        )
        .await,
        StatusCode::OK,
    )
    .await;
    assert_eq!(closed_hidden.status, AttendanceStatus::Closed);

    assert!(repo::tenants::update_user_access_scope(
        &rig.fixture.db,
        rig.tenant_id,
        &UpdateUserAccessScope {
            user_id: rig.supervisor_id,
            all_facilities: false,
            facility_ids: vec![rig.facility_id],
            all_inventory_owners: true,
            inventory_owner_ids: Vec::new(),
        },
    )
    .await
    .unwrap());
    let hidden_clock_in: wareboxes_domain::Timestamp = closed_hidden.clocked_in_at.parse().unwrap();
    let hidden_clock_out: wareboxes_domain::Timestamp = closed_hidden
        .clocked_out_at
        .as_deref()
        .unwrap()
        .parse()
        .unwrap();
    let concealed_correction = rig
        .send(
            &rig.supervisor_token,
            Method::POST,
            &format!(
                "/api/v1/labor/attendance/{}/corrections",
                closed_hidden.attendance_interval_id
            ),
            Some("labor-hidden-correction-concealed"),
            Some(&CorrectAttendanceRequest {
                expected_revision: closed_hidden.effective_revision,
                corrected_clocked_in_at: (hidden_clock_in - Duration::from_secs(60)).to_rfc3339(),
                corrected_clocked_out_at: hidden_clock_out.to_rfc3339(),
                reason: LaborCorrectionReason::MissedPunch,
                note: "Forgot to record the actual shift start".into(),
            }),
        )
        .await;
    assert_status(concealed_correction, StatusCode::NOT_FOUND).await;
}

async fn grant_permissions(
    fixture: &Fixture,
    tenant_id: TenantId,
    user_id: i64,
    role_name: &str,
    permission_names: &[&str],
) {
    let role = wareboxes_persistence_postgres::roles::add_role(
        &fixture.db,
        tenant_id,
        role_name,
        Some("Labor acceptance role"),
    )
    .await
    .unwrap();
    for permission_name in permission_names {
        let permission = wareboxes_persistence_postgres::permissions::add_permission(
            &fixture.db,
            tenant_id,
            permission_name,
            Some("Labor acceptance permission"),
        )
        .await
        .unwrap();
        wareboxes_persistence_postgres::roles::add_role_permission(
            &fixture.db,
            tenant_id,
            role,
            permission,
        )
        .await
        .unwrap();
    }
    wareboxes_persistence_postgres::roles::add_role_to_user(&fixture.db, tenant_id, user_id, role)
        .await
        .unwrap();
}

fn request<T: Serialize>(
    token: &str,
    tenant_id: TenantId,
    method: Method,
    path: &str,
    key: Option<&str>,
    body: Option<&T>,
) -> Request<Body> {
    let mut builder = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(TENANT_ID_HEADER, tenant_id.to_string());
    if let Some(key) = key {
        builder = builder.header(IDEMPOTENCY_KEY_HEADER, key);
    }
    let body = match body {
        Some(body) => {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(body).unwrap())
        }
        None => Body::empty(),
    };
    builder.body(body).unwrap()
}

async fn response(response: axum::response::Response) -> (StatusCode, axum::body::Bytes) {
    let status = response.status();
    let body = to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    (status, body)
}

async fn assert_status(response_value: axum::response::Response, expected: StatusCode) {
    let (status, body) = response(response_value).await;
    assert_eq!(
        status,
        expected,
        "unexpected response: {}",
        String::from_utf8_lossy(&body)
    );
}

async fn json<T: serde::de::DeserializeOwned>(
    response_value: axum::response::Response,
    expected: StatusCode,
) -> T {
    let (status, body) = response(response_value).await;
    assert_eq!(
        status,
        expected,
        "unexpected response: {}",
        String::from_utf8_lossy(&body)
    );
    serde_json::from_slice(&body).unwrap()
}
