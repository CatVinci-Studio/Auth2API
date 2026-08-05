//! Chat Completions <-> Responses translation.
//!
//! Both formats express the same capabilities - text, images, tool calling -
//! with different field names and shapes, so this module is pure mapping. The
//! awkward parts are all on the tool-calling side:
//!
//! | | Chat Completions | Responses |
//! |---|---|---|
//! | declaring a tool | `tools[].function.{name,parameters}` | `tools[].{name,parameters}` |
//! | the model calling one | `message.tool_calls[]` | output item `type:"function_call"` |
//! | returning a result | `{role:"tool", tool_call_id, content}` | input item `type:"function_call_output"` |

use crate::config::Config;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

// --- inbound: Chat Completions request ------------------------------------

#[derive(Deserialize, Debug, Default)]
pub struct ChatRequest {
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub messages: Vec<ChatMessage>,
    #[serde(default)]
    pub stream: Option<bool>,
    #[serde(default)]
    pub stream_options: Option<StreamOptions>,
    #[serde(default)]
    pub tools: Option<Vec<Value>>,
    #[serde(default)]
    pub tool_choice: Option<Value>,
    #[serde(default)]
    pub parallel_tool_calls: Option<bool>,
    #[serde(default)]
    pub reasoning_effort: Option<String>,
    #[serde(default)]
    pub max_completion_tokens: Option<u64>,
    #[serde(default)]
    pub max_tokens: Option<u64>,
}

#[derive(Deserialize, Debug, Default)]
pub struct StreamOptions {
    #[serde(default)]
    pub include_usage: bool,
}

#[derive(Deserialize, Debug, Default)]
pub struct ChatMessage {
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub content: Option<Value>,
    #[serde(default)]
    pub tool_calls: Option<Vec<ChatToolCall>>,
    #[serde(default)]
    pub tool_call_id: Option<String>,
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct ChatToolCall {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default = "function_type")]
    pub kind: String,
    #[serde(default)]
    pub function: ChatFunctionCall,
}

fn function_type() -> String {
    "function".to_string()
}

#[derive(Deserialize, Serialize, Debug, Clone, Default)]
pub struct ChatFunctionCall {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub arguments: String,
}

/// Flattens Chat Completions' content field, which is either a plain string
/// or an array of typed parts.
fn content_to_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(parts)) => parts
            .iter()
            .filter_map(|p| p.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

/// Maps user content into Responses input parts, keeping images.
fn user_content_parts(content: Option<&Value>) -> Vec<Value> {
    match content {
        Some(Value::Array(parts)) => {
            let mut out = Vec::new();
            for part in parts {
                match part.get("type").and_then(Value::as_str) {
                    Some("image_url") => {
                        // Accept both {image_url:{url}} (spec) and
                        // {image_url:"..."} (seen in the wild).
                        let url = part
                            .get("image_url")
                            .and_then(|iu| iu.get("url").and_then(Value::as_str).or_else(|| iu.as_str()));
                        if let Some(url) = url {
                            out.push(json!({"type": "input_image", "image_url": url}));
                        }
                    }
                    _ => {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            out.push(json!({"type": "input_text", "text": text}));
                        }
                    }
                }
            }
            out
        }
        other => vec![json!({"type": "input_text", "text": content_to_text(other)})],
    }
}

/// Normalizes a tool declaration into the Responses shape.
///
/// Accepts the nested Chat Completions form and the already-flat Responses
/// form, because clients pointed at an OpenAI-compatible endpoint send both
/// and rejecting one of them would just look like a broken server.
fn tool_to_responses(tool: &Value) -> Option<Value> {
    let inner = tool.get("function").unwrap_or(tool);
    let name = inner.get("name").and_then(Value::as_str)?;
    Some(json!({
        "type": "function",
        "name": name,
        "description": inner.get("description").and_then(Value::as_str).unwrap_or(""),
        "parameters": inner
            .get("parameters")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
    }))
}

fn tool_choice_to_responses(choice: &Value) -> Value {
    match choice {
        Value::String(_) => choice.clone(),
        Value::Object(_) => {
            let name = choice
                .get("function")
                .and_then(|f| f.get("name"))
                .or_else(|| choice.get("name"))
                .and_then(Value::as_str);
            match name {
                Some(name) => json!({"type": "function", "name": name}),
                None => json!("auto"),
            }
        }
        _ => json!("auto"),
    }
}

pub struct TranslatedRequest {
    pub body: Value,
    pub model: String,
    pub stream: bool,
    pub include_usage: bool,
}

/// Builds the upstream Responses body for a Chat Completions request.
///
/// Sampling knobs (`temperature`, `top_p`, `presence_penalty`, ...) are
/// deliberately dropped rather than forwarded: the ChatGPT backend rejects
/// them for these reasoning models, and a client that sets a harmless default
/// temperature should not get a hard 400 for it.
pub fn chat_to_responses(req: &ChatRequest, config: &Config) -> TranslatedRequest {
    let model = config.resolve_model(req.model.as_deref());

    let mut instructions = Vec::new();
    let mut input = Vec::new();

    for message in &req.messages {
        match message.role.as_str() {
            "system" | "developer" => instructions.push(content_to_text(message.content.as_ref())),

            "user" => input.push(json!({
                "type": "message",
                "role": "user",
                "content": user_content_parts(message.content.as_ref()),
            })),

            "assistant" => {
                let text = content_to_text(message.content.as_ref());
                if !text.is_empty() {
                    input.push(json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{"type": "output_text", "text": text}],
                    }));
                }
                // Tool calls the model made on a previous turn have to be
                // replayed as their own input items, or the tool results
                // below refer to a call the model has no record of making.
                for call in message.tool_calls.iter().flatten() {
                    input.push(json!({
                        "type": "function_call",
                        "call_id": call.id,
                        "name": call.function.name,
                        "arguments": call.function.arguments,
                    }));
                }
            }

            "tool" | "function" => input.push(json!({
                "type": "function_call_output",
                "call_id": message.tool_call_id.clone().unwrap_or_default(),
                "output": content_to_text(message.content.as_ref()),
            })),

            _ => {}
        }
    }

    let instructions = match instructions
        .iter()
        .filter(|s| !s.is_empty())
        .cloned()
        .collect::<Vec<_>>()
    {
        parts if parts.is_empty() => config.default_instructions.clone(),
        parts => parts.join("\n\n"),
    };

    let tools: Vec<Value> = req
        .tools
        .iter()
        .flatten()
        .filter_map(tool_to_responses)
        .collect();

    let mut body = json!({
        "model": model,
        "store": false,
        "stream": true,
        "instructions": instructions,
        "input": input,
    });

    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = req
            .tool_choice
            .as_ref()
            .map(tool_choice_to_responses)
            .unwrap_or_else(|| json!("auto"));
        body["parallel_tool_calls"] = json!(req.parallel_tool_calls.unwrap_or(true));
    }

    if let Some(effort) = &req.reasoning_effort {
        body["reasoning"] = json!({"effort": effort});
    }
    if let Some(max) = req.max_completion_tokens.or(req.max_tokens) {
        body["max_output_tokens"] = json!(max);
    }

    TranslatedRequest {
        stream: req.stream.unwrap_or(false),
        include_usage: req
            .stream_options
            .as_ref()
            .map(|o| o.include_usage)
            .unwrap_or(false),
        body,
        model,
    }
}

// --- outbound: assembling a Chat Completions message ----------------------

#[derive(Default, Debug)]
pub struct AssembledMessage {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ChatToolCall>,
}

impl AssembledMessage {
    /// Rebuilds the assistant message from the upstream's completed output
    /// items (`response.output_item.done`).
    pub fn from_output_items(items: &[Value]) -> Self {
        let mut assembled = Self::default();
        for item in items {
            match item.get("type").and_then(Value::as_str) {
                Some("function_call") => assembled.tool_calls.push(ChatToolCall {
                    id: item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_string(),
                    kind: "function".to_string(),
                    function: ChatFunctionCall {
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or_default()
                            .to_string(),
                        // An empty-argument call must still carry valid JSON;
                        // clients feed this straight into a JSON parser.
                        arguments: item
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                            .to_string(),
                    },
                }),
                Some("message") => {
                    for part in item.get("content").and_then(Value::as_array).into_iter().flatten() {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            assembled.content.push_str(text);
                        }
                    }
                }
                Some("reasoning") => {
                    for part in item.get("summary").and_then(Value::as_array).into_iter().flatten() {
                        if let Some(text) = part.get("text").and_then(Value::as_str) {
                            assembled.reasoning.push_str(text);
                        }
                    }
                }
                _ => {}
            }
        }
        assembled
    }

    pub fn finish_reason(&self) -> &'static str {
        if self.tool_calls.is_empty() {
            "stop"
        } else {
            "tool_calls"
        }
    }

    pub fn to_message_json(&self) -> Value {
        let mut message = json!({
            "role": "assistant",
            "content": if self.content.is_empty() && !self.tool_calls.is_empty() {
                Value::Null
            } else {
                json!(self.content)
            },
        });
        if !self.reasoning.is_empty() {
            message["reasoning_content"] = json!(self.reasoning);
        }
        if !self.tool_calls.is_empty() {
            message["tool_calls"] = json!(self.tool_calls);
        }
        message
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(json_str: &str) -> ChatRequest {
        serde_json::from_str(json_str).unwrap()
    }

    #[test]
    fn system_messages_become_instructions_and_the_rest_become_input() {
        let req = request(
            r#"{"messages":[
                {"role":"system","content":"be terse"},
                {"role":"user","content":"hi"},
                {"role":"assistant","content":"hello"},
                {"role":"user","content":"bye"}]}"#,
        );
        let out = chat_to_responses(&req, &Config::default());
        assert_eq!(out.body["instructions"], "be terse");
        assert_eq!(out.body["input"].as_array().unwrap().len(), 3);
        assert_eq!(out.body["input"][0]["role"], "user");
        assert_eq!(out.body["input"][1]["content"][0]["type"], "output_text");
        // The backend only serves SSE, so the upstream body always streams
        // regardless of what the client asked for.
        assert_eq!(out.body["stream"], true);
        assert!(!out.stream);
    }

    /// A full tool round-trip is the case most likely to break silently: drop
    /// the replayed `function_call` and the upstream sees a result for a call
    /// it never made.
    #[test]
    fn a_tool_round_trip_maps_to_function_call_and_function_call_output() {
        let req = request(
            r#"{"messages":[
                {"role":"user","content":"weather?"},
                {"role":"assistant","content":null,"tool_calls":[
                    {"id":"call_1","type":"function",
                     "function":{"name":"get_weather","arguments":"{\"city\":\"SF\"}"}}]},
                {"role":"tool","tool_call_id":"call_1","content":"18C"}],
              "tools":[{"type":"function","function":{
                  "name":"get_weather","description":"w","parameters":{"type":"object"}}}]}"#,
        );
        let out = chat_to_responses(&req, &Config::default());
        let input = out.body["input"].as_array().unwrap();

        assert_eq!(input.len(), 3);
        assert_eq!(input[1]["type"], "function_call");
        assert_eq!(input[1]["call_id"], "call_1");
        assert_eq!(input[1]["arguments"], "{\"city\":\"SF\"}");
        assert_eq!(input[2]["type"], "function_call_output");
        assert_eq!(input[2]["call_id"], "call_1");
        assert_eq!(input[2]["output"], "18C");

        // Tools flatten: no nested `function` object upstream.
        assert_eq!(out.body["tools"][0]["name"], "get_weather");
        assert!(out.body["tools"][0].get("function").is_none());
        assert_eq!(out.body["tool_choice"], "auto");
    }

    #[test]
    fn a_flat_tool_declaration_is_accepted_too() {
        let req = request(
            r#"{"messages":[],"tools":[{"type":"function","name":"ping","parameters":{}}]}"#,
        );
        let out = chat_to_responses(&req, &Config::default());
        assert_eq!(out.body["tools"][0]["name"], "ping");
    }

    #[test]
    fn a_named_tool_choice_flattens() {
        let req = request(
            r#"{"messages":[],"tools":[{"type":"function","name":"ping"}],
                "tool_choice":{"type":"function","function":{"name":"ping"}}}"#,
        );
        let out = chat_to_responses(&req, &Config::default());
        assert_eq!(out.body["tool_choice"], json!({"type":"function","name":"ping"}));
    }

    #[test]
    fn multimodal_user_content_keeps_images() {
        let req = request(
            r#"{"messages":[{"role":"user","content":[
                {"type":"text","text":"what is this?"},
                {"type":"image_url","image_url":{"url":"data:image/png;base64,AAAA"}}]}]}"#,
        );
        let out = chat_to_responses(&req, &Config::default());
        let content = &out.body["input"][0]["content"];
        assert_eq!(content[0]["type"], "input_text");
        assert_eq!(content[1]["type"], "input_image");
        assert_eq!(content[1]["image_url"], "data:image/png;base64,AAAA");
    }

    /// Forwarding these is a hard 400 upstream, so a client that sets a
    /// harmless default must not be punished for it.
    #[test]
    fn sampling_parameters_are_dropped_rather_than_forwarded() {
        let req = request(r#"{"messages":[],"temperature":0.7,"top_p":0.9}"#);
        let out = chat_to_responses(&req, &Config::default());
        assert!(out.body.get("temperature").is_none());
        assert!(out.body.get("top_p").is_none());
    }

    #[test]
    fn no_system_message_falls_back_to_the_configured_instructions() {
        let req = request(r#"{"messages":[{"role":"user","content":"hi"}]}"#);
        let config = Config::default();
        let out = chat_to_responses(&req, &config);
        assert_eq!(out.body["instructions"], config.default_instructions);
    }

    #[test]
    fn assembling_prefers_tool_calls_and_nulls_empty_content() {
        let items = vec![
            json!({"type": "function_call", "call_id": "c1", "name": "f", "arguments": "{\"a\":1}"}),
        ];
        let assembled = AssembledMessage::from_output_items(&items);
        assert_eq!(assembled.finish_reason(), "tool_calls");
        let message = assembled.to_message_json();
        assert!(message["content"].is_null());
        assert_eq!(message["tool_calls"][0]["id"], "c1");
        assert_eq!(message["tool_calls"][0]["function"]["name"], "f");
    }

    #[test]
    fn a_tool_call_with_no_arguments_still_yields_parseable_json() {
        let items = vec![json!({"type": "function_call", "call_id": "c1", "name": "f"})];
        let assembled = AssembledMessage::from_output_items(&items);
        assert_eq!(assembled.tool_calls[0].function.arguments, "{}");
    }

    #[test]
    fn assembling_concatenates_message_text() {
        let items = vec![json!({"type": "message", "content": [
            {"type": "output_text", "text": "part one "},
            {"type": "output_text", "text": "part two"}]})];
        let assembled = AssembledMessage::from_output_items(&items);
        assert_eq!(assembled.content, "part one part two");
        assert_eq!(assembled.finish_reason(), "stop");
    }
}
