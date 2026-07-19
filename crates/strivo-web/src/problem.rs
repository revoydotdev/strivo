//! RFC 9457 Problem Details — one error envelope for the whole API
//! (roadmap Phase 2 item 7).
//!
//! Every error response is a single shape served as
//! `application/problem+json`:
//!
//! ```json
//! { "type": "about:blank", "title": "Unauthorized", "status": 401,
//!   "detail": "…", "instance": null }
//! ```
//!
//! `type` defaults to `about:blank` (RFC 9457 §4.2.1: callers then use
//! `title`/`status`); `title` is the canonical reason phrase for the
//! status; `detail` is the human-readable specific. Construct with the
//! helpers and `return problem.into_response()` from any handler.

use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

#[derive(Debug, Clone)]
pub struct Problem {
    status: StatusCode,
    detail: String,
}

impl Problem {
    /// Arbitrary status + human-readable detail.
    pub fn new(status: StatusCode, detail: impl Into<String>) -> Self {
        Self {
            status,
            detail: detail.into(),
        }
    }

    pub fn unauthorized() -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "authentication required")
    }

    /// 401 with a caller-supplied reason — used where the generic
    /// "authentication required" detail would hide *why* (e.g. licence
    /// signature/claim verification failures).
    pub fn unauthorized_detail(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, detail)
    }

    pub fn bad_request(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, detail)
    }

    pub fn not_found(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, detail)
    }

    pub fn internal(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, detail)
    }

    /// Daemon IPC unreachable / not yet ready.
    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, detail)
    }

    /// HTTP 402 — the resource exists but the caller's licence does
    /// not cover it. Used by Pro-plugin data routes when the licence
    /// gate refuses entitlement.
    pub fn payment_required(detail: impl Into<String>) -> Self {
        Self::new(StatusCode::PAYMENT_REQUIRED, detail)
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let body = Json(json!({
            "type": "about:blank",
            "title": self.status.canonical_reason().unwrap_or("Error"),
            "status": self.status.as_u16(),
            "detail": self.detail,
            "instance": serde_json::Value::Null,
        }));
        let mut resp = (self.status, body).into_response();
        resp.headers_mut().insert(
            header::CONTENT_TYPE,
            header::HeaderValue::from_static("application/problem+json"),
        );
        resp
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    #[tokio::test]
    async fn renders_rfc9457_envelope() {
        let resp = Problem::unauthorized().into_response();
        assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(
            resp.headers().get(header::CONTENT_TYPE).unwrap(),
            "application/problem+json"
        );
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["status"], 401);
        assert_eq!(v["title"], "Unauthorized");
        assert_eq!(v["type"], "about:blank");
        assert!(v["detail"].is_string());
        assert!(v["instance"].is_null());
    }

    #[tokio::test]
    async fn custom_detail_and_status_preserved() {
        let resp = Problem::bad_request("channel_key must be Platform:id").into_response();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = to_bytes(resp.into_body(), 64 * 1024).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["status"], 400);
        assert_eq!(v["detail"], "channel_key must be Platform:id");
    }
}
