use axum::http::StatusCode;

use super::support::{add_plate, Rig};

#[tokio::test]
async fn hierarchy_rejects_scope_mismatch_and_raw_audit_tampering() {
    let rig = Rig::new("invariants").await;
    let other_owner_id = rig
        .fixture
        .inventory_owner(rig.tenant_id, "Other Hierarchy Client")
        .await;
    rig.fixture
        .assign_owner_to_facility(rig.tenant_id, other_owner_id, rig.facility_id)
        .await;
    let other_owner_parent = add_plate(
        &rig.fixture,
        rig.tenant_id,
        other_owner_id,
        rig.facility_id,
        rig.source_location_id,
        "OTHER-OWNER-PALLET",
    )
    .await;
    assert_eq!(
        rig.change_parent(
            rig.child_id,
            Some(other_owner_parent),
            0,
            "Cross-client nesting",
            "cross-owner",
        )
        .await
        .status(),
        StatusCode::CONFLICT
    );

    let mut raw = crate::common::tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let direct_update = sqlx::query(
        r#"
        UPDATE license_plates SET parent_license_plate_id=$1,hierarchy_revision=1,
          hierarchy_updated_at=clock_timestamp(),hierarchy_updated_by_user_id=$2
        WHERE tenant_id=$3 AND id=$4
        "#,
    )
    .bind(rig.parent_id)
    .bind(rig.user_id)
    .bind(rig.tenant_id.get())
    .bind(rig.child_id)
    .execute(&mut *raw)
    .await;
    assert!(direct_update.is_err());
    raw.rollback().await.unwrap();

    super::support::change_json(
        rig.change_parent(
            rig.child_id,
            Some(rig.parent_id),
            0,
            "Valid attachment",
            "valid-attach",
        )
        .await,
        StatusCode::OK,
    )
    .await;
    let mut raw = crate::common::tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    sqlx::query("UPDATE license_plates SET location_id=$1 WHERE tenant_id=$2 AND id=$3")
        .bind(rig.destination_location_id)
        .bind(rig.tenant_id.get())
        .bind(rig.parent_id)
        .execute(&mut *raw)
        .await
        .unwrap();
    assert!(raw.commit().await.is_err());

    let mut raw = crate::common::tenant_tx(&rig.fixture.db, rig.tenant_id).await;
    let event_id: i64 = sqlx::query_scalar(
        "SELECT id FROM license_plate_hierarchy_events WHERE tenant_id=$1 AND child_license_plate_id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(rig.child_id)
    .fetch_one(&mut *raw)
    .await
    .unwrap();
    let mutated = sqlx::query(
        "UPDATE license_plate_hierarchy_events SET reason='forged' WHERE tenant_id=$1 AND id=$2",
    )
    .bind(rig.tenant_id.get())
    .bind(event_id)
    .execute(&mut *raw)
    .await;
    assert!(mutated.is_err());
    raw.rollback().await.unwrap();
}
