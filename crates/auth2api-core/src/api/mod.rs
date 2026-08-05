//! The local HTTP surface: an OpenAI-shaped API in front of a ChatGPT login.

pub mod chat;
pub mod models;
pub mod responses;
pub mod usage;

use crate::config::Config;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Json;
use serde_json::json;
use std::sync::Arc;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
}

/// An error rendered in OpenAI's error envelope, because clients parse it -
/// several surface `error.message` directly to their users and show nothing
/// at all for a bare string body.
pub struct ApiError {
    pub status: StatusCode,
    pub message: String,
    pub kind: &'static str,
}

impl ApiError {
    pub fn new(status: StatusCode, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
        }
    }
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_request_error", message)
    }
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNAUTHORIZED, "invalid_request_error", message)
    }
    pub fn upstream(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, "upstream_error", message)
    }

    pub fn to_json(&self) -> serde_json::Value {
        json!({"error": {
            "message": self.message,
            "type": self.kind,
            "param": null,
            "code": null,
        }})
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.to_json())).into_response()
    }
}

/// Identifies the caller from its API key.
///
/// Accepts `Authorization: Bearer` and `x-api-key`. When no keys exist at all
/// the caller is anonymous and allowed through - that is the zero-setup
/// loopback case, and `check_bind_safety` is what stops it from ever being
/// the network-exposed one.
pub fn check_key(config: &Config, headers: &HeaderMap) -> Result<crate::keys::Caller, ApiError> {
    crate::keys::authenticate(config, presented_secret(headers)).map_err(ApiError::unauthorized)
}

/// Pulls the presented secret out of whichever header carried it.
fn presented_secret(headers: &HeaderMap) -> &str {
    headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer ").or_else(|| v.strip_prefix("bearer ")))
        .or_else(|| headers.get("x-api-key").and_then(|v| v.to_str().ok()))
        .unwrap_or_default()
        .trim()
}

async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let signed_in = crate::auth::load().ok().flatten();
    Json(json!({
        "status": "ok",
        "signed_in": signed_in.is_some(),
        "account_id": signed_in.as_ref().map(|c| c.account_id.clone()),
        "plan": signed_in.as_ref().and_then(|c| c.plan.clone()),
        "models": state.config.models,
        "default_model": state.config.default_model,
    }))
}

pub fn router(config: Arc<Config>) -> axum::Router {
    let state = AppState { config };
    axum::Router::new()
        .route("/health", get(health))
        .route("/v1/models", get(models::list))
        .route("/v1/models/{model}", get(models::retrieve))
        .route("/v1/chat/completions", post(chat::completions))
        .route("/v1/responses", post(responses::create))
        .route("/v1/usage", get(usage::report))
        .layer(tower_http::cors::CorsLayer::permissive())
        .layer(tower_http::trace::TraceLayer::new_for_http())
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for (name, value) in pairs {
            headers.insert(
                axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        headers
    }

    /// Clients disagree on which header to use, and a server that only reads
    /// one of them looks broken to half of them.
    #[test]
    fn the_secret_is_read_from_either_header() {
        assert_eq!(
            presented_secret(&headers(&[("authorization", "Bearer sk-a2a-abc")])),
            "sk-a2a-abc"
        );
        assert_eq!(
            presented_secret(&headers(&[("authorization", "bearer sk-a2a-abc")])),
            "sk-a2a-abc"
        );
        assert_eq!(
            presented_secret(&headers(&[("x-api-key", "sk-a2a-abc")])),
            "sk-a2a-abc"
        );
        assert_eq!(presented_secret(&headers(&[])), "");
    }

    /// A bare token with no scheme is not a Bearer credential; treating it as
    /// one would accept `Authorization: sk-...` from a misconfigured client
    /// and hide the misconfiguration.
    #[test]
    fn an_authorization_header_without_the_bearer_scheme_yields_nothing() {
        assert_eq!(presented_secret(&headers(&[("authorization", "sk-a2a-abc")])), "");
    }
}
