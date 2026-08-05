//! `/v1/responses` - the untranslated endpoint.
//!
//! Bodies go upstream essentially as written, which is what makes native
//! Responses clients (and anything built on the Codex-style agent loop) work
//! without a translation layer in the way. Two things are still rewritten:
//! the model is clamped to what this login can serve, and `stream` is forced
//! true upstream because the backend serves nothing else - a client asking
//! for a whole body gets one assembled here.

use super::chat::Recorder;
use super::{check_key, ApiError, AppState};
use crate::upstream::stream::{EventKind, Usage};
use axum::body::Bytes;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::Json;
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::convert::Infallible;

pub async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let caller = check_key(&state.config, &headers)?;

    let mut request: Value = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request(format!("could not parse the request body: {e}")))?;
    if !request.is_object() {
        return Err(ApiError::bad_request("the request body must be a JSON object"));
    }

    let client_wants_stream = request
        .get("stream")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let model = state
        .config
        .resolve_model(request.get("model").and_then(Value::as_str));
    request["model"] = json!(model);

    let recorder = Recorder::new("/v1/responses", model.clone(), client_wants_stream, caller);

    let cred = crate::auth::valid_credential().await.map_err(|e| {
        recorder.finish(Usage::default(), Some(e.clone()));
        ApiError::unauthorized(e)
    })?;

    let upstream = crate::upstream::post_responses(&cred, request)
        .await
        .map_err(|e| {
            recorder.finish(Usage::default(), Some(e.clone()));
            ApiError::upstream(e)
        })?;

    if client_wants_stream {
        Ok(passthrough_stream(upstream, recorder))
    } else {
        aggregate(upstream, model, recorder).await
    }
}

/// Relays upstream events verbatim, only watching them in passing for the
/// usage numbers.
fn passthrough_stream(upstream: reqwest::Response, recorder: Recorder) -> Response {
    let stream = async_stream::stream! {
        let mut events = Box::pin(crate::upstream::stream::events(upstream));
        let mut usage = Usage::default();
        let mut failure: Option<String> = None;

        while let Some(event) = events.next().await {
            let event = match event {
                Ok(event) => event,
                Err(e) => { failure = Some(e); break }
            };
            match &event.kind {
                EventKind::Completed { usage: u } => usage = *u,
                EventKind::Failed(message) => failure = Some(message.clone()),
                _ => {}
            }
            // The SSE event name is set from the payload's own `type` because
            // that is how the OpenAI Responses clients dispatch; the JSON is
            // forwarded untouched either way.
            let mut sse = Event::default().data(event.raw.to_string());
            if let Some(kind) = event.raw.get("type").and_then(Value::as_str) {
                sse = sse.event(kind);
            }
            yield Ok::<_, Infallible>(sse);
        }

        recorder.finish(usage, failure);
        yield Ok(Event::default().data("[DONE]"));
    };

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}

/// Rebuilds a whole Response object for a client that asked for one.
///
/// The output items come from `response.output_item.done` rather than from
/// the `response.completed` event's own `output` array, which is unreliable
/// on this backend and frequently arrives empty.
async fn aggregate(
    upstream: reqwest::Response,
    model: String,
    recorder: Recorder,
) -> Result<Response, ApiError> {
    let mut events = Box::pin(crate::upstream::stream::events(upstream));
    let mut items: Vec<Value> = Vec::new();
    let mut usage = Usage::default();
    let mut response_id = String::new();

    while let Some(event) = events.next().await {
        let event = event.map_err(|e| {
            recorder.finish(usage, Some(e.clone()));
            ApiError::upstream(e)
        })?;
        match event.kind {
            EventKind::Created { response_id: id } => response_id = id,
            EventKind::ItemDone(item) => items.push(item),
            EventKind::Completed { usage: u } => usage = u,
            EventKind::Failed(message) => {
                recorder.finish(usage, Some(message.clone()));
                return Err(ApiError::upstream(message));
            }
            _ => {}
        }
    }

    recorder.finish(usage, None);
    if response_id.is_empty() {
        response_id = format!("resp_{}", uuid::Uuid::new_v4().simple());
    }

    Ok(Json(json!({
        "id": response_id,
        "object": "response",
        "created_at": chrono::Local::now().timestamp(),
        "model": model,
        "status": "completed",
        "output": items,
        "usage": {
            "input_tokens": usage.prompt_tokens,
            "input_tokens_details": {"cached_tokens": usage.cached_tokens},
            "output_tokens": usage.completion_tokens,
            "output_tokens_details": {"reasoning_tokens": usage.reasoning_tokens},
            "total_tokens": usage.total_tokens,
        },
    }))
    .into_response())
}
