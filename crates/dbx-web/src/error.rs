use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use dbx_core::backend_error::BackendError;
use std::fmt;

#[derive(Debug)]
pub struct AppError {
    pub message: String,
    pub status: StatusCode,
    pub error: BackendError,
}

impl fmt::Display for AppError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl AppError {
    pub fn internal(msg: impl Into<String>) -> Self {
        Self::with_status(msg.into(), StatusCode::INTERNAL_SERVER_ERROR)
    }

    pub fn bad_request(msg: impl Into<String>) -> Self {
        Self::with_status(msg.into(), StatusCode::BAD_REQUEST)
    }

    pub fn not_found(msg: impl Into<String>) -> Self {
        Self::with_status(msg.into(), StatusCode::NOT_FOUND)
    }

    fn with_status(message: String, status: StatusCode) -> Self {
        let error = BackendError::from_legacy_string(&message);
        Self { message, status, error }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, Json(self.error)).into_response()
    }
}

impl From<String> for AppError {
    fn from(s: String) -> Self {
        Self::internal(s)
    }
}

impl From<&str> for AppError {
    fn from(s: &str) -> Self {
        Self::internal(s)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_errors_are_exposed_as_the_shared_backend_envelope() {
        let error = AppError::internal("database failed");
        assert_eq!(error.status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(error.error.version(), 1);
        assert_eq!(error.error.code(), "DBX-LEGACY-0001");
        assert_eq!(error.message, "database failed");
    }

    #[test]
    fn structured_agent_text_keeps_catalog_identity_at_http_boundary() {
        let error = AppError::from(
            "Agent RPC error (-1): timed out\nDBX_AGENT_ERROR_DATA:{\"category\":\"timeout\",\"stage\":\"execute\"}",
        );
        assert_eq!(error.error.code(), "DBX-JDBC-9001");
        assert_eq!(error.error.source(), dbx_core::backend_error::BackendErrorSource::JdbcAgentLegacy);
    }

    #[tokio::test]
    async fn http_response_is_json_backend_error() {
        let response = AppError::internal("database failed").into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(response.headers()[axum::http::header::CONTENT_TYPE], "application/json");
        let body = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload["version"], 1);
        assert_eq!(payload["code"], "DBX-LEGACY-0001");
        assert_eq!(payload["messageKey"], "backendErrors.legacy");
    }
}
