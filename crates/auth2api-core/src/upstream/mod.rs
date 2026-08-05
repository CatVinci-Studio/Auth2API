//! Talking to the ChatGPT account backend.
//!
//! The Codex OAuth token only works against ChatGPT's own backend API (the
//! one the chatgpt.com web app and the Codex CLI use), not the public
//! api.openai.com Responses API - the endpoints look alike but the token is
//! rejected by the latter.

pub mod http;
pub mod stream;

use crate::auth::Credential;
use serde_json::Value;

const RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";

/// Sends one Responses-API request upstream and hands back the raw streaming
/// response for the caller to interpret.
///
/// `stream: true` is forced into the body regardless of what the caller asked
/// for: this backend rejects `stream: false` outright with "Stream must be
/// set to true". Serving a non-streaming client is therefore always a matter
/// of consuming the SSE here and aggregating, never of asking for a whole
/// body upstream.
pub async fn post_responses(cred: &Credential, mut body: Value) -> Result<reqwest::Response, String> {
    body["stream"] = Value::Bool(true);

    let res = http::streaming_client()
        .post(RESPONSES_URL)
        .bearer_auth(&cred.access)
        .header("chatgpt-account-id", &cred.account_id)
        .header("originator", crate::auth::ORIGINATOR)
        .header("OpenAI-Beta", "responses=experimental")
        .header("accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("upstream request failed: {e}"))?;

    if !res.status().is_success() {
        let status = res.status();
        let text = res.text().await.unwrap_or_default();
        return Err(format!("upstream returned {status}: {text}"));
    }
    Ok(res)
}
