use super::*;
use wareboxes_api_contract::v1::LaborActivityKind;

#[test]
fn date_filter_uses_explicit_utc_day_boundaries() {
    assert_eq!(
        date_bound("2026-08-15"),
        Some("2026-08-15T00:00:00Z".into())
    );
    assert_eq!(date_bound("  "), None);
}

#[test]
fn quantity_validation_rejects_negative_and_invalid_values() {
    assert_eq!(optional_nonnegative("", "quantity"), Ok(None));
    assert_eq!(optional_nonnegative("17", "quantity"), Ok(Some(17)));
    assert!(optional_nonnegative("-1", "quantity").is_err());
    assert!(optional_nonnegative("abc", "quantity").is_err());
}

#[test]
fn work_classification_separates_direct_and_indirect_activity() {
    assert!(is_direct(LaborActivityKind::Picking));
    assert!(is_direct(LaborActivityKind::ValueAddedWork));
    assert!(!is_direct(LaborActivityKind::Break));
    assert!(!is_direct(LaborActivityKind::Delay));
}

#[test]
fn ratio_basis_points_handles_empty_denominator() {
    assert_eq!(ratio_basis_points(1, 0), None);
    assert_eq!(ratio_basis_points(1_800, 3_600), Some(5_000));
}

#[test]
fn zero_exception_time_clears_reason_and_detail() {
    assert_eq!(
        completion_exception(
            0,
            Some(LaborExceptionReason::Equipment),
            "stale detail".into(),
        ),
        Ok((None, None))
    );
    assert!(completion_exception(30, None, String::new()).is_err());
    assert_eq!(
        completion_exception(
            30,
            Some(LaborExceptionReason::Congestion),
            "aisle blocked".into(),
        ),
        Ok((
            Some(LaborExceptionReason::Congestion),
            Some("aisle blocked".into())
        ))
    );
}
