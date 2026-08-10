use wareboxes_api_contract::v1::ExpectedReceivingSessionResponse;

use super::{internal_get, ApiError};

pub async fn expected_receiving_session(
    load_id: i64,
) -> Result<ExpectedReceivingSessionResponse, ApiError> {
    internal_get(&expected_receiving_session_path(load_id)).await
}

fn expected_receiving_session_path(load_id: i64) -> String {
    format!("/api/v1/expected-receiving/loads/{load_id}")
}

#[cfg(test)]
mod tests {
    use super::expected_receiving_session_path;

    #[test]
    fn session_path_targets_the_scoped_v1_read_model() {
        assert_eq!(
            expected_receiving_session_path(42),
            "/api/v1/expected-receiving/loads/42"
        );
    }
}
