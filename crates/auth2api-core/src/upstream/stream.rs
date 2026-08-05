//! Turns the upstream Responses SSE body into a typed event stream.
//!
//! Every consumer in this crate reads the upstream through here: the
//! `/v1/responses` passthrough uses each event's `raw` JSON, while
//! `/v1/chat/completions` reads the normalized `kind` and never touches the
//! wire shape. Both need the same stateful bookkeeping (the call-id mapping,
//! the failure-as-an-event handling), which is why it lives in one place.

use futures_util::{Stream, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy, Default, Serialize, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_tokens: u64,
    pub reasoning_tokens: u64,
}

#[derive(Debug, Clone)]
pub enum EventKind {
    /// The upstream response object was created; carries its id.
    Created { response_id: String },
    TextDelta(String),
    /// Reasoning summary text. Not part of the Chat Completions spec, but
    /// widely consumed as `delta.reasoning_content`, so it is surfaced rather
    /// than dropped.
    ReasoningDelta(String),
    ToolCallStart {
        index: usize,
        call_id: String,
        name: String,
    },
    ToolCallArgsDelta {
        index: usize,
        delta: String,
    },
    /// A finished output item (`message`, `function_call`, ...). This is what
    /// non-streaming callers aggregate, because the `response.completed`
    /// event's own `response.output` array is unreliable on this backend and
    /// frequently arrives empty.
    ItemDone(Value),
    Completed {
        usage: Usage,
    },
    /// A remote failure. It arrives as an ordinary event rather than an HTTP
    /// status, because the headers were already sent with 200 by the time the
    /// model failed.
    Failed(String),
    /// Anything we do not interpret. Passthrough consumers still forward it.
    Other,
}

pub struct ParsedEvent {
    pub raw: Value,
    pub kind: EventKind,
}

fn parse_usage(response: &Value) -> Usage {
    let usage = response.get("usage");
    let get = |key: &str| -> u64 {
        usage
            .and_then(|u| u.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    let nested = |outer: &str, key: &str| -> u64 {
        usage
            .and_then(|u| u.get(outer))
            .and_then(|d| d.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };

    let prompt_tokens = get("input_tokens");
    let completion_tokens = get("output_tokens");
    Usage {
        prompt_tokens,
        completion_tokens,
        // The backend does send total_tokens, but not on every shape of
        // response - deriving it when absent keeps the accounting honest
        // instead of silently logging a zero.
        total_tokens: match get("total_tokens") {
            0 => prompt_tokens + completion_tokens,
            total => total,
        },
        cached_tokens: nested("input_tokens_details", "cached_tokens"),
        reasoning_tokens: nested("output_tokens_details", "reasoning_tokens"),
    }
}

/// Classifies one decoded SSE payload, updating the cross-event state:
/// `call_ids` maps an output item's `id` to its `call_id` (arguments deltas
/// are keyed by the former, everything downstream by the latter), and
/// `next_index` hands out the positional index Chat Completions requires in
/// `delta.tool_calls[]`.
fn classify(
    event: &Value,
    call_ids: &mut HashMap<String, (usize, String)>,
    next_index: &mut usize,
) -> EventKind {
    let Some(kind) = event.get("type").and_then(Value::as_str) else {
        return EventKind::Other;
    };

    match kind {
        "response.created" => EventKind::Created {
            response_id: event
                .get("response")
                .and_then(|r| r.get("id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        },

        "response.output_text.delta" => event
            .get("delta")
            .and_then(Value::as_str)
            .map(|d| EventKind::TextDelta(d.to_string()))
            .unwrap_or(EventKind::Other),

        "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => event
            .get("delta")
            .and_then(Value::as_str)
            .map(|d| EventKind::ReasoningDelta(d.to_string()))
            .unwrap_or(EventKind::Other),

        "response.output_item.added" => {
            let Some(item) = event.get("item") else {
                return EventKind::Other;
            };
            if item.get("type").and_then(Value::as_str) != Some("function_call") {
                return EventKind::Other;
            }
            let call_id = item
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let name = item
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let index = *next_index;
            *next_index += 1;
            if let Some(id) = item.get("id").and_then(Value::as_str) {
                call_ids.insert(id.to_string(), (index, call_id.clone()));
            }
            EventKind::ToolCallStart {
                index,
                call_id,
                name,
            }
        }

        "response.function_call_arguments.delta" => {
            let Some(delta) = event.get("delta").and_then(Value::as_str) else {
                return EventKind::Other;
            };
            let item_id = event
                .get("item_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match call_ids.get(item_id) {
                Some((index, _)) => EventKind::ToolCallArgsDelta {
                    index: *index,
                    delta: delta.to_string(),
                },
                None => EventKind::Other,
            }
        }

        "response.output_item.done" => event
            .get("item")
            .map(|item| EventKind::ItemDone(item.clone()))
            .unwrap_or(EventKind::Other),

        "response.completed" => EventKind::Completed {
            usage: event.get("response").map(parse_usage).unwrap_or_default(),
        },

        "response.failed" | "error" => {
            let error = event
                .get("response")
                .and_then(|r| r.get("error"))
                .or_else(|| event.get("error"))
                .cloned()
                .unwrap_or_else(|| event.clone());
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| error.to_string());
            EventKind::Failed(message)
        }

        _ => EventKind::Other,
    }
}

/// Consumes the upstream SSE body, yielding one [`ParsedEvent`] per decoded
/// `data:` payload. Transport errors surface as `Err`; model-side failures
/// surface in-band as [`EventKind::Failed`] and are left for the caller to
/// act on, since by then a streaming caller has already sent 200 headers.
pub fn events(res: reqwest::Response) -> impl Stream<Item = Result<ParsedEvent, String>> {
    async_stream::try_stream! {
        let mut body = res.bytes_stream();
        let mut lines = super::http::SseLineBuffer::default();
        let mut call_ids: HashMap<String, (usize, String)> = HashMap::new();
        let mut next_index = 0usize;

        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| format!("upstream stream failed: {e}"))?;
            // Collected rather than yielded inline because `push` takes a
            // synchronous callback and cannot await.
            let mut batch = Vec::new();
            lines.push(&chunk, |data| {
                if data.trim() == "[DONE]" {
                    return;
                }
                if let Ok(value) = serde_json::from_str::<Value>(data) {
                    let kind = classify(&value, &mut call_ids, &mut next_index);
                    batch.push(ParsedEvent { raw: value, kind });
                }
            });
            for event in batch {
                yield event;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn classify_all(events: &[Value]) -> Vec<EventKind> {
        let mut call_ids = HashMap::new();
        let mut next_index = 0;
        events
            .iter()
            .map(|e| classify(e, &mut call_ids, &mut next_index))
            .collect()
    }

    /// The whole reason this bookkeeping exists: arguments deltas are keyed by
    /// the item id, but Chat Completions needs the positional index that was
    /// only established when the item was added.
    #[test]
    fn argument_deltas_resolve_to_the_index_from_the_added_event() {
        let kinds = classify_all(&[
            json!({"type": "response.output_item.added", "item": {
                "type": "function_call", "id": "item_1", "call_id": "call_abc", "name": "get_weather"}}),
            json!({"type": "response.output_item.added", "item": {
                "type": "function_call", "id": "item_2", "call_id": "call_def", "name": "get_time"}}),
            json!({"type": "response.function_call_arguments.delta",
                   "item_id": "item_2", "delta": "{\"tz\""}),
        ]);

        assert!(matches!(&kinds[0], EventKind::ToolCallStart { index: 0, call_id, name }
            if call_id == "call_abc" && name == "get_weather"));
        assert!(matches!(&kinds[1], EventKind::ToolCallStart { index: 1, .. }));
        // Second tool call's arguments must land on index 1, not 0.
        assert!(matches!(&kinds[2], EventKind::ToolCallArgsDelta { index: 1, delta }
            if delta == "{\"tz\""));
    }

    /// A delta for an item we never saw added would otherwise be attributed to
    /// an arbitrary tool call and corrupt its arguments JSON.
    #[test]
    fn an_orphan_argument_delta_is_ignored() {
        let kinds = classify_all(&[json!({
            "type": "response.function_call_arguments.delta",
            "item_id": "never_announced", "delta": "{}"
        })]);
        assert!(matches!(kinds[0], EventKind::Other));
    }

    #[test]
    fn usage_is_read_including_its_nested_details() {
        let kinds = classify_all(&[json!({"type": "response.completed", "response": {"usage": {
            "input_tokens": 100, "output_tokens": 20, "total_tokens": 120,
            "input_tokens_details": {"cached_tokens": 64},
            "output_tokens_details": {"reasoning_tokens": 12}
        }}})]);
        let EventKind::Completed { usage } = &kinds[0] else {
            panic!("expected Completed");
        };
        assert_eq!(
            *usage,
            Usage {
                prompt_tokens: 100,
                completion_tokens: 20,
                total_tokens: 120,
                cached_tokens: 64,
                reasoning_tokens: 12,
            }
        );
    }

    #[test]
    fn a_missing_total_is_derived_rather_than_logged_as_zero() {
        let kinds = classify_all(&[json!({"type": "response.completed", "response": {"usage": {
            "input_tokens": 7, "output_tokens": 3
        }}})]);
        let EventKind::Completed { usage } = &kinds[0] else {
            panic!("expected Completed");
        };
        assert_eq!(usage.total_tokens, 10);
    }

    #[test]
    fn a_failure_event_carries_the_message_not_the_whole_envelope() {
        let kinds = classify_all(&[json!({"type": "response.failed", "response": {
            "error": {"message": "model overloaded", "code": "server_error"}}})]);
        assert!(matches!(&kinds[0], EventKind::Failed(m) if m == "model overloaded"));
    }
}
