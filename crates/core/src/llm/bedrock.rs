use crate::config::{Config, Reasoning};
use crate::llm::types::{
    repair_tool_pairs, AssistantTurn, ContentPart, FunctionCall, Message, MessageContent, ToolCall,
    ToolSpec,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The event that ends a Converse stream, after which no other event arrives.
pub const STOP_EVENT: &str = "messageStop";

/// The fixed parts of an event-stream frame: the prelude's two lengths and its
/// CRC at the front, and the message CRC at the end.
const PRELUDE: usize = 12;
const TRAILER: usize = 4;

/// The largest frame the protocol allows. A prelude naming a longer one is not
/// an event stream at all — an SSE line read as a prelude asks for a gigabyte —
/// so it is refused rather than waited on.
const MAX_FRAME: usize = 16 * 1024 * 1024;

/// The tool calls Bedrock streams. Unlike the OpenAI wire its arguments arrive
/// as JSON fragments split across deltas, so each one is accumulated as text and
/// parsed once the block closes.
#[derive(Debug, Default)]
pub(crate) struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Translate the internal OpenAI-style conversation into a Converse request
/// body.
///
/// Bedrock names the tool result by the call id it answers, so the ids pass
/// through unchanged here rather than being resolved back to a function name
/// the way Gemini needs. The system prompt and the inference limits sit in
/// fields of their own.
pub fn request_body(config: &Config, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let mut system: Vec<Value> = Vec::new();
    let mut contents: Vec<Value> = Vec::new();

    for message in &repair_tool_pairs(messages) {
        match message.role.as_str() {
            "system" => {
                if let Some(content) = &message.content {
                    let text = content.display();
                    if !text.is_empty() {
                        system.push(json!({ "text": text }));
                    }
                }
            }
            "user" => push_content(&mut contents, "user", user_parts(message)),
            "assistant" => {
                let parts = assistant_parts(message);
                if !parts.is_empty() {
                    push_content(&mut contents, "assistant", parts);
                }
            }
            "tool" => {
                let id = message.tool_call_id.clone().unwrap_or_default();
                let result = json!({
                    "toolResult": {
                        "toolUseId": id,
                        "content": [{ "text": result_text(message) }],
                    }
                });
                push_content(&mut contents, "user", vec![result]);
            }
            _ => {}
        }
    }

    let mut body = json!({
        "messages": contents,
        "inferenceConfig": { "maxTokens": config.max_tokens },
    });
    if !system.is_empty() {
        body["system"] = Value::Array(system);
    }
    if !tools.is_empty() {
        let specs: Vec<Value> = tools
            .iter()
            .map(|spec| {
                json!({
                    "toolSpec": {
                        "name": spec.function.name,
                        "description": spec.function.description,
                        "inputSchema": { "json": spec.function.parameters },
                    }
                })
            })
            .collect();
        body["toolConfig"] = json!({ "tools": specs });
    }
    // A Claude model on Bedrock takes its thinking budget where the Anthropic
    // API puts it, through the fields the model's own API adds.
    if let Some(thinking) = thinking_fields(config) {
        body["additionalModelRequestFields"] = thinking;
    }
    body
}

/// Apply one event of the Converse stream to the in-progress assistant turn.
///
/// The event names itself in the frame's `:event-type` header rather than in
/// its payload, so it is passed in; a payload that does carry its own name —
/// the shape AWS documents these events in — unwraps to the same thing.
pub fn apply_event(
    event: &str,
    payload: &Value,
    turn: &mut AssistantTurn,
    partials: &mut BTreeMap<usize, PartialToolCall>,
    on_text: &mut dyn FnMut(String),
    on_thinking: &mut dyn FnMut(String),
) -> Result<()> {
    if let Some(object) = payload.as_object() {
        // A rejected request arrives as an event named after the exception,
        // which is what has to fail the turn rather than be read as an empty
        // step.
        for (name, body) in object {
            if name.ends_with("Exception") {
                bail!(
                    "bedrock error: {}",
                    body["message"].as_str().unwrap_or(name.as_str())
                );
            }
        }
    }
    if let Some(message) = payload["message"].as_str() {
        bail!("bedrock error: {message}");
    }

    // Both shapes of the same event — the payload AWS puts on the wire, and one
    // that wraps itself in the name of the union member — reduce to the body,
    // whose fields are named the same either way.
    let value = payload.get(event).unwrap_or(payload);

    if let Some(usage) = value["usage"].as_object() {
        let input = usage
            .get("inputTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let cached = usage
            .get("cacheReadInputTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        turn.usage.input = input.saturating_sub(cached);
        turn.usage.cache_read = cached;
        turn.usage.cache_write = usage
            .get("cacheWriteInputTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        turn.usage.output = usage
            .get("outputTokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    }

    if let Some(reason) = value["stopReason"].as_str() {
        // Bedrock spells the output limit `max_tokens`; oxide's escalation path
        // keys off the OpenAI spelling.
        turn.finish_reason = Some(match reason {
            "max_tokens" => "length".to_string(),
            other => other.to_string(),
        });
    }

    // A block's number lives inside the event that names it.
    let Some(start) = value.get("start") else {
        if let Some(delta) = value.get("delta") {
            let index = value["contentBlockIndex"].as_u64().unwrap_or(0) as usize;
            return apply_delta(delta, index, partials, turn, on_text, on_thinking);
        }
        return Ok(());
    };
    if let Some(call) = start.get("toolUse").and_then(Value::as_object) {
        let index = value["contentBlockIndex"].as_u64().unwrap_or(0) as usize;
        partials.insert(
            index,
            PartialToolCall {
                id: call
                    .get("toolUseId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                name: call
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                arguments: String::new(),
            },
        );
    }
    Ok(())
}

fn apply_delta(
    delta: &Value,
    index: usize,
    partials: &mut BTreeMap<usize, PartialToolCall>,
    turn: &mut AssistantTurn,
    on_text: &mut dyn FnMut(String),
    on_thinking: &mut dyn FnMut(String),
) -> Result<()> {
    if let Some(text) = delta["text"].as_str() {
        if !text.is_empty() {
            turn.content.push_str(text);
            on_text(text.to_string());
        }
    }
    if let Some(text) = delta["reasoningContent"]["text"].as_str() {
        if !text.is_empty() {
            crate::llm::types::push_thinking(&mut turn.thinking, text);
            on_thinking(text.to_string());
        }
    }
    if let Some(signature) = delta["reasoningContent"]["signature"].as_str() {
        crate::llm::types::set_thinking_signature(&mut turn.thinking, signature);
    }
    if let Some(fragment) = delta["toolUse"]["input"].as_str() {
        partials
            .entry(index)
            .or_default()
            .arguments
            .push_str(fragment);
    }
    Ok(())
}

/// Hand every complete frame of an AWS event-stream body to `on_event`, keeping
/// what is left of a frame that has not fully arrived for the next chunk.
///
/// The Converse stream is not SSE: `converse-stream` answers with AWS's binary
/// event-stream framing (`application/vnd.amazon.eventstream`) — a prelude of
/// two lengths and a CRC, then headers, then a payload and a second CRC — so
/// reading the response as text loses every event it holds. Each frame's
/// `:event-type` header names the event and its payload is the JSON that event
/// carries; both are handed over together. The CRCs are not checked: a corrupt
/// payload does not parse into the event it claims to be, and an event that
/// does not parse leaves the turn without the stop reason it needs, which is
/// already an error where it is read.
pub fn drain_frames<F>(buffer: &mut Vec<u8>, mut on_event: F) -> Result<()>
where
    F: FnMut(&str, &Value) -> Result<()>,
{
    let mut consumed = 0;
    while let Some((total, headers_len)) = frame_lengths(&buffer[consumed..])? {
        let end = consumed + total;
        if end > buffer.len() {
            break;
        }
        let frame = &buffer[consumed..end];
        let headers = &frame[PRELUDE..PRELUDE + headers_len];
        let payload = &frame[PRELUDE + headers_len..total - TRAILER];
        let value = serde_json::from_slice::<Value>(payload).unwrap_or(Value::Null);
        on_event(header(headers, ":event-type").unwrap_or_default(), &value)?;
        consumed = end;
    }
    buffer.drain(..consumed);
    Ok(())
}

/// Whether `buffer` holds at least one whole frame — the lengths of its prelude
/// when it does, and nothing while the frame is still arriving. A prelude that
/// cannot be a frame longer than itself is a stream that is not this protocol.
fn frame_lengths(buffer: &[u8]) -> Result<Option<(usize, usize)>> {
    if buffer.len() < PRELUDE {
        return Ok(None);
    }
    let total = u32::from_be_bytes([buffer[0], buffer[1], buffer[2], buffer[3]]) as usize;
    let headers = u32::from_be_bytes([buffer[4], buffer[5], buffer[6], buffer[7]]) as usize;
    if !(PRELUDE + TRAILER..=MAX_FRAME).contains(&total) || headers > total - PRELUDE - TRAILER {
        bail!("bedrock returned a malformed event-stream frame");
    }
    Ok(Some((total, headers)))
}

/// The value of one header, if the frame carries it. A header is its name's
/// length and bytes, a type byte, and a value whose bytes depend on that type —
/// a bool has none, a fixed-width number has its width, and a string has a
/// length and the bytes.
fn header<'a>(headers: &'a [u8], wanted: &str) -> Option<&'a str> {
    let mut rest = headers;
    while rest.len() > 1 {
        let name_len = rest[0] as usize;
        if rest.len() < name_len + 2 {
            return None;
        }
        let name = std::str::from_utf8(&rest[1..1 + name_len]).ok()?;
        let kind = rest[1 + name_len];
        let mut at = name_len + 2;
        let text = match kind {
            0 | 1 => None,
            2 => {
                at += 1;
                None
            }
            3 => {
                at += 2;
                None
            }
            4 => {
                at += 4;
                None
            }
            5 | 8 => {
                at += 8;
                None
            }
            6 | 7 | 9 => {
                if rest.len() < at + 2 {
                    return None;
                }
                let len = u16::from_be_bytes([rest[at], rest[at + 1]]) as usize;
                at += 2;
                if rest.len() < at + len {
                    return None;
                }
                let value = std::str::from_utf8(&rest[at..at + len]).ok();
                at += len;
                if kind == 7 {
                    value
                } else {
                    None
                }
            }
            _ => return None,
        };
        if name == wanted {
            return text;
        }
        if rest.len() < at {
            return None;
        }
        rest = &rest[at..];
    }
    None
}

/// One event-stream frame, laid out the way `converse-stream` sends it: the two
/// lengths of the prelude with its CRC, the headers, the payload and the message
/// CRC. The CRCs are left zero, since nothing reads them — a frame whose payload
/// did not arrive intact does not parse into the event it claims to be.
#[cfg(test)]
pub(crate) fn frame(event: &str, payload: &str) -> Vec<u8> {
    let mut headers = Vec::new();
    for (name, value) in [
        (":event-type", event),
        (":content-type", "application/json"),
    ] {
        headers.push(name.len() as u8);
        headers.extend_from_slice(name.as_bytes());
        headers.push(7);
        headers.extend_from_slice(&(value.len() as u16).to_be_bytes());
        headers.extend_from_slice(value.as_bytes());
    }
    let total = PRELUDE + headers.len() + payload.len() + TRAILER;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(total as u32).to_be_bytes());
    bytes.extend_from_slice(&(headers.len() as u32).to_be_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes.extend_from_slice(&headers);
    bytes.extend_from_slice(payload.as_bytes());
    bytes.extend_from_slice(&0u32.to_be_bytes());
    bytes
}

pub fn into_tool_calls(partials: BTreeMap<usize, PartialToolCall>) -> Vec<ToolCall> {
    partials
        .into_values()
        .filter(|partial| !partial.name.is_empty())
        .map(|partial| ToolCall {
            // Bedrock mints the id and hands it back with the result, so it is
            // kept rather than replaced the way Gemini's name-only calls are.
            id: if partial.id.is_empty() {
                format!("call_{}", partial.name)
            } else {
                partial.id
            },
            kind: "function".to_string(),
            signature: None,
            function: FunctionCall {
                name: partial.name,
                arguments: if partial.arguments.trim().is_empty() {
                    "{}".to_string()
                } else {
                    partial.arguments
                },
            },
        })
        .collect()
}

/// The model ids `ListFoundationModels` reports. Only the ones the Converse API
/// serves are offered, since an image or embedding model cannot answer a turn.
pub fn parse_model_list(value: &Value) -> Vec<String> {
    let mut models: Vec<String> = value["modelSummaries"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter(|model| {
                    let id = model["modelId"].as_str().unwrap_or_default();
                    id.contains("anthropic")
                        || id.contains("amazon.nova")
                        || id.contains("meta.llama")
                        || id.contains("mistral")
                        || id.contains("cohere.command")
                        || id.contains("deepseek")
                        || id.contains("qwen")
                })
                .filter_map(|model| model["modelId"].as_str())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    models.dedup();
    models
}

/// Appends parts, folding them into the previous message when it already holds
/// this role: Converse rejects two messages in a row with the same role, and the
/// results of a batch of calls belong to one.
fn push_content(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = contents.last_mut() {
        if last["role"] == json!(role) {
            if let Some(existing) = last["content"].as_array_mut() {
                existing.extend(parts);
                return;
            }
        }
    }
    contents.push(json!({ "role": role, "content": parts }));
}

fn user_parts(message: &Message) -> Vec<Value> {
    match &message.content {
        None => Vec::new(),
        Some(MessageContent::Text(text)) => vec![json!({ "text": text })],
        Some(MessageContent::Parts(parts)) => parts.iter().map(content_part).collect(),
    }
}

/// Bedrock takes an image as its bytes, so a data URL is split into the format
/// and the base64 the API asks for. A file has no image block to sit in and is
/// handed over as text naming it.
fn content_part(part: &ContentPart) -> Value {
    match part {
        ContentPart::Text { text } => json!({ "text": text }),
        ContentPart::ImageUrl { image_url } => {
            let Some(rest) = image_url.url.strip_prefix("data:") else {
                return json!({ "text": image_url.url });
            };
            let Some((meta, data)) = rest.split_once(',') else {
                return json!({ "text": image_url.url });
            };
            let format = meta
                .strip_suffix(";base64")
                .unwrap_or(meta)
                .split_once('/')
                .map(|(_, subtype)| subtype.to_string())
                .unwrap_or_else(|| "png".to_string());
            json!({
                "image": {
                    "format": format,
                    "source": { "bytes": data },
                }
            })
        }
        ContentPart::File { file } => json!({
            "text": format!(
                "[file: {}]",
                file.filename.clone().unwrap_or_else(|| "attachment".into())
            )
        }),
    }
}

fn result_text(message: &Message) -> String {
    match &message.content {
        Some(content) => content.display(),
        None => String::new(),
    }
}

fn assistant_parts(message: &Message) -> Vec<Value> {
    let mut parts = Vec::new();
    if let Some(content) = &message.content {
        let text = content.display();
        if !text.is_empty() {
            parts.push(json!({ "text": text }));
        }
    }
    if let Some(calls) = &message.tool_calls {
        for call in calls {
            // The input travels as an object here rather than as JSON text.
            let input = serde_json::from_str::<Value>(&call.function.arguments)
                .unwrap_or_else(|_| json!({}));
            parts.push(json!({
                "toolUse": {
                    "toolUseId": call.id,
                    "name": call.function.name,
                    "input": input,
                }
            }));
        }
    }
    parts
}

/// A thinking budget for a Claude model on Bedrock, which is where the
/// Anthropic API's `thinking` field goes. Other model families have no such
/// field and take the request as it stands.
fn thinking_fields(config: &Config) -> Option<Value> {
    if !config.model.to_ascii_lowercase().contains("anthropic") {
        return None;
    }
    let budget = match config.reasoning {
        Reasoning::Auto => return None,
        // Thinking cannot be turned off here, so the smallest budget the API
        // accepts stands in for `off`.
        Reasoning::Off => 1024,
        level => level.budget_tokens(config.max_tokens)?,
    };
    Some(json!({ "thinking": { "type": "enabled", "budget_tokens": budget } }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> Config {
        Config {
            provider: "bedrock".to_string(),
            model: "anthropic.claude-3-5-sonnet-20241022-v2:0".to_string(),
            max_tokens: 8192,
            ..Default::default()
        }
    }

    #[test]
    fn keeps_the_system_prompt_out_of_the_conversation() {
        let body = request_body(
            &config(),
            &[Message::system("be brief"), Message::user("hello")],
            &[],
        );
        assert_eq!(body["system"][0]["text"], "be brief");
        assert_eq!(body["messages"][0]["role"], "user");
        assert_eq!(body["messages"][0]["content"][0]["text"], "hello");
        assert_eq!(body["inferenceConfig"]["maxTokens"], 8192);
    }

    #[test]
    fn answers_a_call_by_its_id() {
        let assistant = Message::assistant(
            "",
            vec![ToolCall {
                id: "tooluse_x".to_string(),
                kind: "function".to_string(),
                signature: None,
                function: FunctionCall {
                    name: "read".to_string(),
                    arguments: "{\"path\":\"a.rs\"}".to_string(),
                },
            }],
        );
        let result = Message::tool("tooluse_x", "contents");

        let body = request_body(&config(), &[Message::user("go"), assistant, result], &[]);
        assert_eq!(
            body["messages"][1]["content"][0]["toolUse"]["toolUseId"],
            "tooluse_x"
        );
        assert_eq!(
            body["messages"][2]["content"][0]["toolResult"]["toolUseId"],
            "tooluse_x"
        );
        assert_eq!(
            body["messages"][2]["content"][0]["toolResult"]["content"][0]["text"],
            "contents"
        );
    }

    #[test]
    fn declares_tools_with_their_input_schema() {
        let tools = vec![ToolSpec {
            kind: "function",
            function: crate::llm::types::FunctionSpec {
                name: "grep".to_string(),
                description: "search".to_string(),
                parameters: json!({ "type": "object" }),
            },
        }];
        let body = request_body(&config(), &[Message::user("go")], &tools);
        assert_eq!(body["toolConfig"]["tools"][0]["toolSpec"]["name"], "grep");
        assert_eq!(
            body["toolConfig"]["tools"][0]["toolSpec"]["inputSchema"]["json"]["type"],
            "object"
        );
    }

    /// Hands `buffer` to the decoder, keeping every event it takes out.
    fn decoded(buffer: &mut Vec<u8>) -> Vec<(String, Value)> {
        let mut seen = Vec::new();
        drain_frames(buffer, |event, payload| {
            seen.push((event.to_string(), payload.clone()));
            Ok(())
        })
        .unwrap();
        seen
    }

    #[test]
    fn decodes_the_event_stream_frames_a_converse_stream_sends() {
        let mut buffer = Vec::new();
        buffer.extend_from_slice(&frame("messageStart", r#"{"role":"assistant"}"#));
        let last = frame(
            "contentBlockDelta",
            r#"{"contentBlockIndex":0,"delta":{"text":"hi"}}"#,
        );
        // A frame whose last bytes have not arrived is not an event yet.
        buffer.extend_from_slice(&last[..last.len() - 2]);

        let seen = decoded(&mut buffer);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "messageStart");
        assert_eq!(seen[0].1["role"], "assistant");
        assert!(!buffer.is_empty(), "the partial frame is kept");

        buffer.extend_from_slice(&last[last.len() - 2..]);
        let seen = decoded(&mut buffer);
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, "contentBlockDelta");
        assert_eq!(seen[0].1["delta"]["text"], "hi");
        assert!(buffer.is_empty());
    }

    #[test]
    fn refuses_a_stream_that_is_not_framed() {
        // A response read as SSE, or a proxy's own page, is not an event stream:
        // its first bytes name a frame no one is going to send.
        let mut buffer = b"data: {\"contentBlockDelta\": {}}\n\n".to_vec();
        let error = drain_frames(&mut buffer, |_, _| Ok(())).unwrap_err();
        assert!(error.to_string().contains("malformed"), "{error}");

        let mut buffer = Vec::new();
        buffer.extend_from_slice(&64u32.to_be_bytes());
        buffer.extend_from_slice(&200u32.to_be_bytes());
        buffer.extend_from_slice(&[0; 64]);
        let error = drain_frames(&mut buffer, |_, _| Ok(())).unwrap_err();
        assert!(error.to_string().contains("malformed"), "{error}");
    }

    #[test]
    fn streams_text_and_a_tool_call_split_across_deltas() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        let mut text = String::new();
        let mut on_text = |value: String| text.push_str(&value);
        let mut buffer = Vec::new();

        for (event, payload) in [
            ("messageStart", r#"{"role":"assistant"}"#),
            (
                "contentBlockDelta",
                r#"{"delta":{"text":"he"},"contentBlockIndex":0}"#,
            ),
            (
                "contentBlockDelta",
                r#"{"delta":{"text":"llo"},"contentBlockIndex":0}"#,
            ),
            (
                "contentBlockStart",
                r#"{"contentBlockIndex":1,"start":{"toolUse":{"toolUseId":"t1","name":"read"}}}"#,
            ),
            // The arguments arrive in fragments, so the block is only a call
            // once the last one has landed.
            (
                "contentBlockDelta",
                r#"{"contentBlockIndex":1,"delta":{"toolUse":{"input":"{\"pa"}}}"#,
            ),
            (
                "contentBlockDelta",
                r#"{"contentBlockIndex":1,"delta":{"toolUse":{"input":"th\":\"a.rs\"}"}}}"#,
            ),
            ("contentBlockStop", r#"{"contentBlockIndex":1}"#),
            (STOP_EVENT, r#"{"stopReason":"tool_use"}"#),
            (
                "metadata",
                r#"{"usage":{"inputTokens":10,"outputTokens":4,"cacheReadInputTokens":6}}"#,
            ),
        ] {
            buffer.extend_from_slice(&frame(event, payload));
            drain_frames(&mut buffer, |event, payload| {
                apply_event(
                    event,
                    payload,
                    &mut turn,
                    &mut partials,
                    &mut on_text,
                    &mut |_| {},
                )
            })
            .unwrap();
        }

        assert_eq!(text, "hello");
        assert_eq!(turn.content, "hello");
        assert_eq!(turn.finish_reason.as_deref(), Some("tool_use"));
        assert_eq!(turn.usage.input, 4);
        assert_eq!(turn.usage.cache_read, 6);
        let calls = into_tool_calls(partials);
        assert_eq!(calls[0].id, "t1");
        assert_eq!(calls[0].function.arguments, r#"{"path":"a.rs"}"#);
    }

    #[test]
    fn reads_reasoning_blocks() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        let mut thinking = String::new();
        let mut on_thinking = |value: String| thinking.push_str(&value);
        for delta in [
            json!({"reasoningContent":{"text":"step "}}),
            json!({"reasoningContent":{"text":"one"}}),
            json!({"reasoningContent":{"signature":"sig"}}),
        ] {
            apply_event(
                "contentBlockDelta",
                &json!({"contentBlockIndex":0,"delta":delta}),
                &mut turn,
                &mut partials,
                &mut |_| {},
                &mut on_thinking,
            )
            .unwrap();
        }
        assert_eq!(thinking, "step one");
        assert_eq!(turn.thinking[0]["thinking"], "step one");
        assert_eq!(turn.thinking[0]["signature"], "sig");
    }

    #[test]
    fn reads_either_shape_of_the_same_event() {
        // The payload of an event is the body it carries, but an event that
        // names itself is read the same way.
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        apply_event(
            "contentBlockDelta",
            &json!({"contentBlockDelta":{"contentBlockIndex":0,"delta":{"text":"hi"}}}),
            &mut turn,
            &mut partials,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(turn.content, "hi");
    }

    #[test]
    fn reads_the_output_limit_as_truncation() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        apply_event(
            STOP_EVENT,
            &json!({"stopReason":"max_tokens"}),
            &mut turn,
            &mut partials,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(turn.finish_reason.as_deref(), Some("length"));
    }

    #[test]
    fn reports_a_stream_error() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        let error = apply_event(
            "validationException",
            &json!({"message":"the model is not available"}),
            &mut turn,
            &mut partials,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("not available"), "{error}");
    }

    #[test]
    fn offers_only_the_models_that_can_answer_a_turn() {
        let models = parse_model_list(&json!({
            "modelSummaries": [
                { "modelId": "amazon.titan-embed-text-v2:0" },
                { "modelId": "anthropic.claude-3-5-sonnet-20241022-v2:0" },
                { "modelId": "amazon.nova-lite-v1:0" },
            ]
        }));
        assert_eq!(
            models,
            vec![
                "amazon.nova-lite-v1:0",
                "anthropic.claude-3-5-sonnet-20241022-v2:0"
            ]
        );
    }

    #[test]
    fn asks_a_claude_model_for_a_thinking_budget() {
        let mut claude = config();
        claude.reasoning = Reasoning::High;
        let body = request_body(&claude, &[Message::user("go")], &[]);
        let budget = body["additionalModelRequestFields"]["thinking"]["budget_tokens"]
            .as_u64()
            .expect("a budget is asked for");
        assert!(budget > 1024 && budget < 8192, "{budget}");
        assert_eq!(
            body["additionalModelRequestFields"]["thinking"]["type"],
            "enabled"
        );
        // A model family with no such field is not sent one.
        let mut llama = config();
        llama.model = "meta.llama3-70b-instruct-v1:0".to_string();
        llama.reasoning = Reasoning::High;
        let body = request_body(&llama, &[Message::user("go")], &[]);
        assert!(body.get("additionalModelRequestFields").is_none());
    }

    #[test]
    fn inlines_an_image_as_bytes() {
        let body = request_body(
            &config(),
            &[Message::user_parts(
                "",
                vec![ContentPart::ImageUrl {
                    image_url: crate::llm::types::ImageUrl {
                        url: "data:image/png;base64,AAAA".to_string(),
                        detail: None,
                    },
                }],
            )],
            &[],
        );
        assert_eq!(body["messages"][0]["content"][0]["image"]["format"], "png");
        assert_eq!(
            body["messages"][0]["content"][0]["image"]["source"]["bytes"],
            "AAAA"
        );
    }
}
