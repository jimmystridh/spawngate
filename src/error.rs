//! Error handling and JSON error responses for the proxy

use http_body_util::{combinators::BoxBody, BodyExt, Full};
use hyper::body::Bytes;
use hyper::{Response, StatusCode};
use serde::Serialize;

/// Error codes for proxy errors
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProxyErrorCode {
    /// Missing Host header in request
    MissingHostHeader,
    /// Unknown or unconfigured host
    UnknownHost,
    /// Backend is shutting down
    BackendShuttingDown,
    /// Backend is unhealthy
    BackendUnhealthy,
    /// Backend failed to start
    BackendStartFailed,
    /// Backend configuration error
    BackendConfigError,
    /// Request timed out waiting for backend
    RequestTimeout,
    /// Failed to connect to backend
    ConnectionFailed,
    /// Internal proxy error
    InternalError,
}

impl ProxyErrorCode {
    /// Get the default HTTP status code for this error
    pub fn status_code(&self) -> StatusCode {
        match self {
            ProxyErrorCode::MissingHostHeader => StatusCode::BAD_REQUEST,
            ProxyErrorCode::UnknownHost => StatusCode::NOT_FOUND,
            ProxyErrorCode::BackendShuttingDown => StatusCode::SERVICE_UNAVAILABLE,
            ProxyErrorCode::BackendUnhealthy => StatusCode::SERVICE_UNAVAILABLE,
            ProxyErrorCode::BackendStartFailed => StatusCode::SERVICE_UNAVAILABLE,
            ProxyErrorCode::BackendConfigError => StatusCode::INTERNAL_SERVER_ERROR,
            ProxyErrorCode::RequestTimeout => StatusCode::GATEWAY_TIMEOUT,
            ProxyErrorCode::ConnectionFailed => StatusCode::BAD_GATEWAY,
            ProxyErrorCode::InternalError => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Get the error code as a string for the X-Proxy-Error header
    pub fn as_header_value(&self) -> &'static str {
        match self {
            ProxyErrorCode::MissingHostHeader => "MISSING_HOST_HEADER",
            ProxyErrorCode::UnknownHost => "UNKNOWN_HOST",
            ProxyErrorCode::BackendShuttingDown => "BACKEND_SHUTTING_DOWN",
            ProxyErrorCode::BackendUnhealthy => "BACKEND_UNHEALTHY",
            ProxyErrorCode::BackendStartFailed => "BACKEND_START_FAILED",
            ProxyErrorCode::BackendConfigError => "BACKEND_CONFIG_ERROR",
            ProxyErrorCode::RequestTimeout => "REQUEST_TIMEOUT",
            ProxyErrorCode::ConnectionFailed => "CONNECTION_FAILED",
            ProxyErrorCode::InternalError => "INTERNAL_ERROR",
        }
    }
}

/// JSON error response body
#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    /// The error code
    pub code: ProxyErrorCode,
    /// Human-readable error message
    pub message: String,
    /// HTTP status code (for reference)
    pub status: u16,
}

impl ErrorResponse {
    /// Create a new error response
    pub fn new(code: ProxyErrorCode, message: impl Into<String>) -> Self {
        Self {
            status: code.status_code().as_u16(),
            code,
            message: message.into(),
        }
    }

    /// Convert to JSON string
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| {
            format!(
                r#"{{"code":"{}","message":"{}","status":{}}}"#,
                self.code.as_header_value(),
                self.message.replace('\"', "\\\""),
                self.status
            )
        })
    }
}

/// Create a JSON error response with X-Proxy-Error header
pub fn json_error_response(
    code: ProxyErrorCode,
    message: impl Into<String>,
) -> Response<BoxBody<Bytes, hyper::Error>> {
    let error = ErrorResponse::new(code, message);
    let status = code.status_code();
    let body = error.to_json();

    Response::builder()
        .status(status)
        .header("Content-Type", "application/json")
        .header("X-Proxy-Error", code.as_header_value())
        .body(Full::new(Bytes::from(body)).map_err(|e| match e {}).boxed())
        .expect("valid response with StatusCode enum and static headers")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_code_status_codes() {
        assert_eq!(
            ProxyErrorCode::MissingHostHeader.status_code(),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            ProxyErrorCode::UnknownHost.status_code(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            ProxyErrorCode::BackendShuttingDown.status_code(),
            StatusCode::SERVICE_UNAVAILABLE
        );
        assert_eq!(
            ProxyErrorCode::RequestTimeout.status_code(),
            StatusCode::GATEWAY_TIMEOUT
        );
        assert_eq!(
            ProxyErrorCode::ConnectionFailed.status_code(),
            StatusCode::BAD_GATEWAY
        );
    }

    #[test]
    fn test_error_response_json() {
        let error = ErrorResponse::new(ProxyErrorCode::UnknownHost, "Host not found: example.com");
        let json = error.to_json();

        assert!(json.contains("\"code\":\"UNKNOWN_HOST\""));
        assert!(json.contains("\"message\":\"Host not found: example.com\""));
        assert!(json.contains("\"status\":404"));
    }

    #[test]
    fn test_json_error_response() {
        let response = json_error_response(ProxyErrorCode::RequestTimeout, "Request timed out");

        assert_eq!(response.status(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(
            response.headers().get("Content-Type").unwrap(),
            "application/json"
        );
        assert_eq!(
            response.headers().get("X-Proxy-Error").unwrap(),
            "REQUEST_TIMEOUT"
        );
    }

    #[test]
    fn test_error_code_header_values() {
        assert_eq!(
            ProxyErrorCode::MissingHostHeader.as_header_value(),
            "MISSING_HOST_HEADER"
        );
        assert_eq!(
            ProxyErrorCode::BackendUnhealthy.as_header_value(),
            "BACKEND_UNHEALTHY"
        );
    }
}
