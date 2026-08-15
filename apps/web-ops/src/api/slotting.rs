use wareboxes_api_contract::v1::{
    AcceptSlottingRecommendationRequest, ConfigureSlottingProfileRequest,
    DismissSlottingRecommendationRequest, RunSlottingRequest, SlottingProfilePage,
    SlottingProfilePageRequest, SlottingProfileResponse, SlottingRecommendationPage,
    SlottingRecommendationPageRequest, SlottingRecommendationResponse, SlottingRunResponse,
};

use super::ApiError;

#[cfg(target_arch = "wasm32")]
pub async fn slotting_profiles(
    request: &SlottingProfilePageRequest,
) -> Result<SlottingProfilePage, ApiError> {
    super::browser::get(&profile_page_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn slotting_profiles(
    _request: &SlottingProfilePageRequest,
) -> Result<SlottingProfilePage, ApiError> {
    Err(ApiError::unavailable())
}

#[cfg(target_arch = "wasm32")]
pub async fn slotting_recommendations(
    request: &SlottingRecommendationPageRequest,
) -> Result<SlottingRecommendationPage, ApiError> {
    super::browser::get(&recommendation_page_path(request)).await
}

#[cfg(not(target_arch = "wasm32"))]
pub async fn slotting_recommendations(
    _request: &SlottingRecommendationPageRequest,
) -> Result<SlottingRecommendationPage, ApiError> {
    Err(ApiError::unavailable())
}

macro_rules! command {
    ($name:ident, $request:ty, $response:ty, $path:literal) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            request: &$request,
            idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            super::browser::post($path, request, idempotency_key).await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _request: &$request,
            _idempotency_key: &str,
        ) -> Result<$response, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

command!(
    configure_slotting_profile,
    ConfigureSlottingProfileRequest,
    SlottingProfileResponse,
    "/api/v1/slotting/profiles"
);
command!(
    run_slotting,
    RunSlottingRequest,
    SlottingRunResponse,
    "/api/v1/slotting/runs"
);

macro_rules! recommendation_command {
    ($name:ident, $request:ty, $suffix:literal) => {
        #[cfg(target_arch = "wasm32")]
        pub async fn $name(
            recommendation_id: i64,
            request: &$request,
            idempotency_key: &str,
        ) -> Result<SlottingRecommendationResponse, ApiError> {
            super::browser::post(
                &format!(
                    "/api/v1/slotting/recommendations/{recommendation_id}/{}",
                    $suffix
                ),
                request,
                idempotency_key,
            )
            .await
        }

        #[cfg(not(target_arch = "wasm32"))]
        pub async fn $name(
            _recommendation_id: i64,
            _request: &$request,
            _idempotency_key: &str,
        ) -> Result<SlottingRecommendationResponse, ApiError> {
            Err(ApiError::unavailable())
        }
    };
}

recommendation_command!(
    accept_slotting_recommendation,
    AcceptSlottingRecommendationRequest,
    "acceptances"
);
recommendation_command!(
    dismiss_slotting_recommendation,
    DismissSlottingRecommendationRequest,
    "dismissals"
);

#[cfg(any(target_arch = "wasm32", test))]
fn profile_page_path(request: &SlottingProfilePageRequest) -> String {
    let mut path = format!(
        "/api/v1/slotting/profiles?limit={}&include_history={}",
        request.limit.get(),
        request.include_history
    );
    append_scope(&mut path, request.inventory_owner_id, request.facility_id);
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn recommendation_page_path(request: &SlottingRecommendationPageRequest) -> String {
    let mut path = format!(
        "/api/v1/slotting/recommendations?limit={}",
        request.limit.get()
    );
    append_scope(&mut path, request.inventory_owner_id, request.facility_id);
    if let Some(run_id) = request.slotting_run_id {
        path.push_str(&format!("&slotting_run_id={run_id}"));
    }
    if let Some(status) = request.status {
        path.push_str("&status=");
        path.push_str(match status {
            wareboxes_api_contract::v1::SlottingRecommendationStatus::Pending => "pending",
            wareboxes_api_contract::v1::SlottingRecommendationStatus::Accepted => "accepted",
            wareboxes_api_contract::v1::SlottingRecommendationStatus::Dismissed => "dismissed",
        });
    }
    append_cursor(&mut path, request.cursor.as_ref());
    path
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_scope(path: &mut String, inventory_owner_id: Option<i64>, facility_id: Option<i64>) {
    if let Some(value) = inventory_owner_id {
        path.push_str(&format!("&inventory_owner_id={value}"));
    }
    if let Some(value) = facility_id {
        path.push_str(&format!("&facility_id={value}"));
    }
}

#[cfg(any(target_arch = "wasm32", test))]
fn append_cursor(path: &mut String, cursor: Option<&wareboxes_api_contract::v1::OpaqueCursor>) {
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&urlencoding::encode(cursor.as_str()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wareboxes_api_contract::v1::{OpaqueCursor, PageLimit, SlottingRecommendationStatus};

    #[test]
    fn profile_path_preserves_scope_history_and_cursor() {
        let request = SlottingProfilePageRequest {
            inventory_owner_id: Some(12),
            facility_id: Some(8),
            include_history: true,
            cursor: Some(OpaqueCursor::new("sp1.scope/a+b").unwrap()),
            limit: PageLimit::new(75).unwrap(),
        };
        assert_eq!(
            profile_page_path(&request),
            "/api/v1/slotting/profiles?limit=75&include_history=true&inventory_owner_id=12&facility_id=8&cursor=sp1.scope%2Fa%2Bb"
        );
    }

    #[test]
    fn recommendation_path_preserves_decision_filters() {
        let request = SlottingRecommendationPageRequest {
            inventory_owner_id: Some(12),
            facility_id: Some(8),
            slotting_run_id: Some(99),
            status: Some(SlottingRecommendationStatus::Dismissed),
            cursor: None,
            limit: PageLimit::new(25).unwrap(),
        };
        assert_eq!(
            recommendation_page_path(&request),
            "/api/v1/slotting/recommendations?limit=25&inventory_owner_id=12&facility_id=8&slotting_run_id=99&status=dismissed"
        );
    }
}
