use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;
use thiserror::Error;

pub const RETCODE_OK: i32 = 0;
pub const RETCODE_RATE_LIMITED: i32 = 1200;
pub const RETCODE_INVALID_REQUEST: i32 = 1400;
pub const RETCODE_UNAUTHORIZED: i32 = 1401;
pub const RETCODE_FORBIDDEN: i32 = 1403;
pub const RETCODE_NOT_FOUND: i32 = 1404;
pub const RETCODE_INTERNAL: i32 = 1500;

#[derive(Debug, Error)]
pub enum AdapterError {
    #[error("invalid JSON: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("invalid protocol payload: {0}")]
    InvalidPayload(String),
    #[error("invalid or missing action parameter: {0}")]
    InvalidParameter(String),
    #[error("protocol adapter is not implemented: {0}")]
    UnsupportedProtocol(String),
}

#[derive(Debug, Clone, Error)]
#[error("{message}")]
pub struct ActionError {
    pub retcode: i32,
    pub message: String,
}

impl ActionError {
    pub fn new(retcode: i32, message: impl Into<String>) -> Self {
        Self {
            retcode,
            message: message.into(),
        }
    }

    pub fn invalid(message: impl Into<String>) -> Self {
        Self::new(RETCODE_INVALID_REQUEST, message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(RETCODE_FORBIDDEN, message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(RETCODE_NOT_FOUND, message)
    }

    pub fn unsupported(action: &str) -> Self {
        Self::new(
            RETCODE_NOT_FOUND,
            format!("action '{action}' is not supported by this prototype"),
        )
    }
}

#[derive(Debug, Clone, Error)]
#[error("unauthorized: {0}")]
pub struct AuthError(pub String);

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
    }

    pub fn forbidden(message: impl Into<String>) -> Self {
        Self::new(StatusCode::FORBIDDEN, "forbidden", message)
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::new(StatusCode::CONFLICT, "conflict", message)
    }

    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", message)
    }

    pub fn registration_disabled() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "registration_disabled",
            "registration is disabled because REGISTER_KEY is not configured",
        )
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        if self.status.is_server_error() {
            tracing::error!(code = self.code, message = %self.message, "request failed");
        }
        (
            self.status,
            Json(json!({
                "error": {
                    "code": self.code,
                    "message": self.message,
                }
            })),
        )
            .into_response()
    }
}

impl From<sqlx::Error> for ApiError {
    fn from(value: sqlx::Error) -> Self {
        tracing::error!(error = %value, "database error");
        Self::internal("database operation failed")
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
