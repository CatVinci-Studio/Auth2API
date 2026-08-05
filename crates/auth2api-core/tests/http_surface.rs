//! End-to-end checks of the HTTP surface that do not touch OpenAI.
//!
//! These drive the real router, so they cover the wiring the unit tests
//! cannot: that a route exists at the path clients actually call, that the
//! key check runs before the handler, and that errors come back in the
//! envelope an OpenAI client can parse.
//!
//! Everything runs against a temporary `AUTH2API_HOME`, so a developer's own
//! login, keys, and usage log are never read or written. That override is
//! process-global, which is why this file is one test - `cargo test` runs
//! separate files in parallel threads that would otherwise fight over it.

use auth2api_core::{api, config, keys, Config};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use std::sync::Arc;
use tower::ServiceExt;

struct TempHome(std::path::PathBuf);

impl TempHome {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("auth2api-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::env::set_var(config::HOME_ENV, &dir);
        Self(dir)
    }
}

impl Drop for TempHome {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

async fn get(config: &Config, path: &str, bearer: Option<&str>) -> (StatusCode, Value) {
    let mut request = Request::builder().uri(path);
    if let Some(token) = bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = api::router(Arc::new(config.clone()))
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, body)
}

async fn post(config: &Config, path: &str, bearer: Option<&str>, body: Value) -> (StatusCode, Value) {
    let mut request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = bearer {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = api::router(Arc::new(config.clone()))
        .oneshot(request.body(Body::from(body.to_string())).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    let parsed = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, parsed)
}

#[tokio::test]
async fn the_local_api_surface_behaves() {
    let _home = TempHome::new("surface");
    let config = Config::default();

    // --- with no keys at all, a loopback caller is served anonymously -----
    let (status, body) = get(&config, "/v1/models", None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["object"], "list");
    let listed: Vec<&str> = body["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["id"].as_str().unwrap())
        .collect();
    assert_eq!(listed, config.models);

    let (status, body) = get(&config, "/health", None).await;
    assert_eq!(status, StatusCode::OK);
    // No credential exists in this temp home, so the app must say so rather
    // than claim a login it does not have.
    assert_eq!(body["signed_in"], false);

    // --- creating a key closes the door ----------------------------------
    let key = keys::create("test client").unwrap();
    let (status, body) = get(&config, "/v1/models", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    // Clients read `error.message`; a bare string body shows them nothing.
    assert!(body["error"]["message"].is_string());

    let (status, _) = get(&config, "/v1/models", Some("sk-a2a-wrong")).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);

    let (status, _) = get(&config, "/v1/models", Some(&key.secret)).await;
    assert_eq!(status, StatusCode::OK);

    // --- an unknown model is a 404, not a fallback ------------------------
    let (status, _) = get(&config, "/v1/models/gpt-4o", Some(&key.secret)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, body) = get(
        &config,
        &format!("/v1/models/{}", config.default_model),
        Some(&key.secret),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["id"], config.default_model);

    // --- request validation happens before anything reaches upstream ------
    let (status, body) = post(
        &config,
        "/v1/chat/completions",
        Some(&key.secret),
        serde_json::json!({"messages": []}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["type"], "invalid_request_error");

    let (status, _) = post(
        &config,
        "/v1/responses",
        Some(&key.secret),
        serde_json::json!("not an object"),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // A well-formed request with no ChatGPT login must fail as unauthorized
    // against *OpenAI*, not 500 - the user's fix is `auth2api login`.
    let (status, body) = post(
        &config,
        "/v1/chat/completions",
        Some(&key.secret),
        serde_json::json!({"messages": [{"role": "user", "content": "hi"}]}),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body["error"]["message"]
        .as_str()
        .unwrap()
        .contains("not signed in"));

    // --- that failure is still accounted for, and attributed to the key ---
    let (status, body) = get(&config, "/v1/usage", Some(&key.secret)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["totals"]["requests"], 1);
    assert_eq!(body["totals"]["failed"], 1);
    assert_eq!(body["by_key"][0]["key"], key.id);
    assert_eq!(body["by_key"][0]["label"], "test client");
    // Nothing was priced, so there must be no cost figure at all.
    assert!(body["totals"]["estimated_cost_usd"].is_null());

    // --- revoking locks the key out but keeps its history -----------------
    keys::revoke(&key.id).unwrap();
    let (status, _) = get(&config, "/v1/models", Some(&key.secret)).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
