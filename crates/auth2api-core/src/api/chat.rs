//! `/v1/chat/completions` - the translated endpoint.
//!
//! Upstream is always SSE (the ChatGPT backend refuses `stream: false`), so
//! both branches here consume the same event stream; they differ only in
//! whether each event is forwarded as a chunk or folded into one body.

use super::{check_key, ApiError, AppState};
use crate::keys::Caller;
use crate::stats::Record;
use crate::translate::{chat_to_responses, AssembledMessage, ChatRequest};
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
use std::time::Instant;

fn completion_id() -> String {
    format!("chatcmpl-{}", uuid::Uuid::new_v4().simple())
}

fn now() -> i64 {
    chrono::Local::now().timestamp()
}

fn usage_json(usage: &Usage) -> Value {
    json!({
        "prompt_tokens": usage.prompt_tokens,
        "completion_tokens": usage.completion_tokens,
        "total_tokens": usage.total_tokens,
        "prompt_tokens_details": {"cached_tokens": usage.cached_tokens},
        "completion_tokens_details": {"reasoning_tokens": usage.reasoning_tokens},
    })
}

/// Collects what a request cost and writes exactly one usage row for it.
pub(crate) struct Recorder {
    started: Instant,
    endpoint: &'static str,
    model: String,
    stream: bool,
    caller: Caller,
}

impl Recorder {
    pub(crate) fn new(
        endpoint: &'static str,
        model: String,
        stream: bool,
        caller: Caller,
    ) -> Self {
        Self {
            started: Instant::now(),
            endpoint,
            model,
            stream,
            caller,
        }
    }

    pub(crate) fn finish(&self, usage: Usage, error: Option<String>) {
        crate::stats::append(&Record {
            ts: now(),
            endpoint: self.endpoint.to_string(),
            model: self.model.clone(),
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            total_tokens: usage.total_tokens,
            cached_tokens: usage.cached_tokens,
            reasoning_tokens: usage.reasoning_tokens,
            stream: self.stream,
            ok: error.is_none(),
            duration_ms: self.started.elapsed().as_millis() as u64,
            error,
            key_id: self.caller.key_id.clone(),
            key_name: self.caller.key_name.clone(),
        });
    }
}

pub async fn completions(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    let caller = check_key(&state.config, &headers)?;

    let request: ChatRequest = serde_json::from_slice(&body)
        .map_err(|e| ApiError::bad_request(format!("could not parse the request body: {e}")))?;
    if request.messages.is_empty() {
        return Err(ApiError::bad_request("'messages' must not be empty"));
    }

    let translated = chat_to_responses(&request, &state.config);
    let recorder = Recorder::new(
        "/v1/chat/completions",
        translated.model.clone(),
        translated.stream,
        caller,
    );

    let cred = crate::auth::valid_credential().await.map_err(|e| {
        recorder.finish(Usage::default(), Some(e.clone()));
        ApiError::unauthorized(e)
    })?;

    let upstream = crate::upstream::post_responses(&cred, translated.body)
        .await
        .map_err(|e| {
            recorder.finish(Usage::default(), Some(e.clone()));
            ApiError::upstream(e)
        })?;

    if translated.stream {
        Ok(stream_response(
            upstream,
            translated.model,
            translated.include_usage,
            recorder,
        ))
    } else {
        aggregate_response(upstream, translated.model, recorder).await
    }
}

/// Folds the upstream stream into a single Chat Completions body.
async fn aggregate_response(
    upstream: reqwest::Response,
    model: String,
    recorder: Recorder,
) -> Result<Response, ApiError> {
    let mut events = Box::pin(crate::upstream::stream::events(upstream));
    let mut items: Vec<Value> = Vec::new();
    let mut usage = Usage::default();

    while let Some(event) = events.next().await {
        let event = event.map_err(|e| {
            recorder.finish(usage, Some(e.clone()));
            ApiError::upstream(e)
        })?;
        match event.kind {
            EventKind::ItemDone(item) => items.push(item),
            EventKind::Completed { usage: u } => usage = u,
            // A mid-stream failure can still become a proper HTTP error here,
            // because nothing has been written to the client yet.
            EventKind::Failed(message) => {
                recorder.finish(usage, Some(message.clone()));
                return Err(ApiError::upstream(message));
            }
            _ => {}
        }
    }

    recorder.finish(usage, None);
    let assembled = AssembledMessage::from_output_items(&items);
    Ok(Json(json!({
        "id": completion_id(),
        "object": "chat.completion",
        "created": now(),
        "model": model,
        "choices": [{
            "index": 0,
            "message": assembled.to_message_json(),
            "finish_reason": assembled.finish_reason(),
        }],
        "usage": usage_json(&usage),
    }))
    .into_response())
}

/// Forwards the upstream stream as Chat Completions chunks.
fn stream_response(
    upstream: reqwest::Response,
    model: String,
    include_usage: bool,
    recorder: Recorder,
) -> Response {
    let stream = async_stream::stream! {
        let mut events = Box::pin(crate::upstream::stream::events(upstream));
        let id = completion_id();
        let created = now();
        let mut usage = Usage::default();
        let mut saw_tool_call = false;
        let mut failure: Option<String> = None;

        let chunk = |delta: Value, finish: Option<&str>| {
            json!({
                "id": id,
                "object": "chat.completion.chunk",
                "created": created,
                "model": model,
                "choices": [{
                    "index": 0,
                    "delta": delta,
                    "finish_reason": finish,
                }],
            })
        };

        // Clients expect the role exactly once, on the first chunk.
        yield Ok::<_, Infallible>(Event::default().data(
            chunk(json!({"role": "assistant", "content": ""}), None).to_string()));

        while let Some(event) = events.next().await {
            let event = match event {
                Ok(event) => event,
                Err(e) => { failure = Some(e); break }
            };
            let delta = match event.kind {
                EventKind::TextDelta(text) => json!({"content": text}),
                EventKind::ReasoningDelta(text) => json!({"reasoning_content": text}),
                EventKind::ToolCallStart { index, call_id, name } => {
                    saw_tool_call = true;
                    json!({"tool_calls": [{
                        "index": index,
                        "id": call_id,
                        "type": "function",
                        // The name arrives once, up front; only arguments
                        // stream, so later chunks omit it entirely.
                        "function": {"name": name, "arguments": ""},
                    }]})
                }
                EventKind::ToolCallArgsDelta { index, delta } => json!({"tool_calls": [{
                    "index": index,
                    "function": {"arguments": delta},
                }]}),
                EventKind::Completed { usage: u } => { usage = u; continue }
                EventKind::Failed(message) => { failure = Some(message); break }
                _ => continue,
            };
            yield Ok(Event::default().data(chunk(delta, None).to_string()));
        }

        if let Some(message) = &failure {
            // Headers went out with 200 long ago, so the only way to report
            // this is in-band. Clients that check for an `error` key surface
            // it; the rest at least see a clean end of stream.
            yield Ok(Event::default().data(json!({"error": {
                "message": message,
                "type": "upstream_error",
            }}).to_string()));
        } else {
            let finish = if saw_tool_call { "tool_calls" } else { "stop" };
            yield Ok(Event::default().data(chunk(json!({}), Some(finish)).to_string()));

            if include_usage {
                yield Ok(Event::default().data(json!({
                    "id": id,
                    "object": "chat.completion.chunk",
                    "created": created,
                    "model": model,
                    "choices": [],
                    "usage": usage_json(&usage),
                }).to_string()));
            }
        }

        recorder.finish(usage, failure);
        yield Ok(Event::default().data("[DONE]"));
    };

    Sse::new(stream).keep_alive(KeepAlive::default()).into_response()
}
