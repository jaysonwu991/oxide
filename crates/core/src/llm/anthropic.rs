use crate::config::{supports_adaptive_thinking, Config, Reasoning};
use crate::llm::types::{
    push_thinking, set_thinking_signature, AssistantTurn, ContentPart, FunctionCall, Message,
    MessageContent, ToolCall, ToolSpec,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub const API_VERSION: &str = "2023-06-01";

#[derive(Debug, Default)]
pub(crate) struct PartialToolCall {
    id: String,
    name: String,
    arguments: String,
}

/// Translate the internal OpenAI-style conversation into an Anthropic
/// Messages API request body.
pub fn request_body(config: &Config, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let mut system = String::new();
    let mut converted: Vec<Value> = Vec::new();

    for message in messages {
        match message.role.as_str() {
            "system" => {
                if let Some(content) = &message.content {
                    let text = content.display();
                    if !text.is_empty() {
                        if !system.is_empty() {
                            system.push_str("\n\n");
                        }
                        system.push_str(&text);
                    }
                }
            }
            "user" => converted.push(json!({
                "role": "user",
                "content": user_blocks(message),
            })),
            "assistant" => {
                let blocks = assistant_blocks(message);
                if !blocks.is_empty() {
                    converted.push(json!({ "role": "assistant", "content": blocks }));
                }
            }
            "tool" => {
                let block = tool_result_block(message);
                match converted.last_mut() {
                    Some(last) if is_tool_result(last) => {
                        if let Some(content) = last["content"].as_array_mut() {
                            content.push(block);
                        }
                    }
                    _ => converted.push(json!({ "role": "user", "content": [block] })),
                }
            }
            _ => {}
        }
    }

    let mut merged = merge_adjacent(converted);
    // Cache the whole conversation prefix up to the latest turn, matching Pi.
    // Anthropic caching is opt-in per request via `cache_control`, so without
    // this every turn re-processes (and re-bills) the entire context.
    if let Some(last) = merged.last_mut() {
        apply_cache_breakpoint(last);
    }
    let mut body = json!({
        "model": config.model,
        "max_tokens": config.max_tokens,
        "messages": merged,
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = json!([{
            "type": "text",
            "text": system,
            "cache_control": cache_control(),
        }]);
    }
    if !tools.is_empty() {
        let mut specs: Vec<Value> = tools.iter().map(tool_schema).collect();
        // A breakpoint on the last tool caches the system prompt and the tool
        // definitions, which are identical on every turn.
        if let Some(last) = specs.last_mut() {
            last["cache_control"] = cache_control();
        }
        body["tools"] = json!(specs);
    }
    let adaptive = supports_adaptive_thinking(&config.model);
    match config.reasoning {
        Reasoning::Off => {}
        Reasoning::Auto if adaptive => {
            body["thinking"] = json!({ "type": "adaptive" });
        }
        Reasoning::Auto => {}
        level if adaptive => {
            body["thinking"] = json!({ "type": "adaptive" });
            body["output_config"] = json!({ "effort": level.effort() });
        }
        level => {
            if let Some(budget) = level.budget_tokens(config.max_tokens) {
                body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
            }
        }
    }
    body
}

/// Apply one parsed SSE event to the in-progress assistant turn.
pub fn apply_event(
    data: &str,
    turn: &mut AssistantTurn,
    partials: &mut BTreeMap<usize, PartialToolCall>,
    on_text: &mut dyn FnMut(String),
    on_thinking: &mut dyn FnMut(String),
) -> Result<()> {
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return Ok(());
    };

    match value["type"].as_str() {
        Some("message_start") => {
            let usage = &value["message"]["usage"];
            if let Some(tokens) = usage["input_tokens"].as_u64() {
                turn.usage.input = tokens;
            }
            if let Some(tokens) = usage["output_tokens"].as_u64() {
                turn.usage.output = tokens;
            }
            if let Some(tokens) = usage["cache_read_input_tokens"].as_u64() {
                turn.usage.cache_read = tokens;
            }
            if let Some(tokens) = usage["cache_creation_input_tokens"].as_u64() {
                turn.usage.cache_write = tokens;
            }
        }
        Some("message_delta") => {
            if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                if !reason.is_empty() {
                    turn.finish_reason = Some(reason.to_string());
                }
            }
            let usage = &value["usage"];
            if let Some(tokens) = usage["output_tokens"].as_u64() {
                turn.usage.output = tokens;
            }
            if let Some(tokens) = usage["input_tokens"].as_u64() {
                turn.usage.input = tokens;
            }
            if let Some(tokens) = usage["cache_read_input_tokens"].as_u64() {
                turn.usage.cache_read = tokens;
            }
            if let Some(tokens) = usage["cache_creation_input_tokens"].as_u64() {
                turn.usage.cache_write = tokens;
            }
        }
        Some("content_block_start") => {
            let index = value["index"].as_u64().unwrap_or(0) as usize;
            let block = &value["content_block"];
            match block["type"].as_str() {
                Some("tool_use") => {
                    let partial = partials.entry(index).or_default();
                    if let Some(id) = block["id"].as_str() {
                        partial.id = id.to_string();
                    }
                    if let Some(name) = block["name"].as_str() {
                        partial.name = name.to_string();
                    }
                }
                Some("thinking") => {
                    turn.thinking
                        .push(json!({ "type": "thinking", "thinking": "", "signature": "" }));
                }
                Some("redacted_thinking") => {
                    turn.thinking.push(block.clone());
                }
                _ => {}
            }
        }
        Some("content_block_delta") => {
            let index = value["index"].as_u64().unwrap_or(0) as usize;
            let delta = &value["delta"];
            match delta["type"].as_str() {
                Some("text_delta") => {
                    if let Some(text) = delta["text"].as_str() {
                        if !text.is_empty() {
                            on_text(text.to_string());
                            turn.content.push_str(text);
                        }
                    }
                }
                Some("input_json_delta") => {
                    if let Some(fragment) = delta["partial_json"].as_str() {
                        partials
                            .entry(index)
                            .or_default()
                            .arguments
                            .push_str(fragment);
                    }
                }
                Some("thinking_delta") => {
                    if let Some(fragment) = delta["thinking"].as_str() {
                        push_thinking(&mut turn.thinking, fragment);
                        if !fragment.is_empty() {
                            on_thinking(fragment.to_string());
                        }
                    }
                }
                Some("signature_delta") => {
                    if let Some(signature) = delta["signature"].as_str() {
                        set_thinking_signature(&mut turn.thinking, signature);
                    }
                }
                _ => {}
            }
        }
        Some("error") => {
            let message = value["error"]["message"]
                .as_str()
                .unwrap_or("unknown error");
            bail!("anthropic error: {message}");
        }
        _ => {}
    }

    Ok(())
}

pub fn into_tool_calls(partials: BTreeMap<usize, PartialToolCall>) -> Vec<ToolCall> {
    partials
        .into_values()
        .filter(|partial| !partial.name.is_empty())
        .enumerate()
        .map(|(index, partial)| ToolCall {
            id: if partial.id.is_empty() {
                format!("call_{index}")
            } else {
                partial.id
            },
            kind: "function".to_string(),
            function: FunctionCall {
                name: partial.name,
                arguments: partial.arguments,
            },
        })
        .collect()
}

fn user_blocks(message: &Message) -> Vec<Value> {
    match &message.content {
        None => Vec::new(),
        Some(MessageContent::Text(text)) => vec![json!({ "type": "text", "text": text })],
        Some(MessageContent::Parts(parts)) => parts.iter().map(content_block).collect(),
    }
}

fn content_block(part: &ContentPart) -> Value {
    match part {
        ContentPart::Text { text } => json!({ "type": "text", "text": text }),
        ContentPart::ImageUrl { image_url } => data_block("image", &image_url.url),
        ContentPart::File { file } => data_block("document", &file.file_data),
    }
}

fn data_block(kind: &str, url: &str) -> Value {
    let (media_type, data) = split_data_url(url);
    json!({
        "type": kind,
        "source": { "type": "base64", "media_type": media_type, "data": data },
    })
}

fn split_data_url(url: &str) -> (String, String) {
    if let Some(rest) = url.strip_prefix("data:") {
        if let Some((meta, data)) = rest.split_once(',') {
            let media_type = meta.strip_suffix(";base64").unwrap_or(meta);
            return (media_type.to_string(), data.to_string());
        }
    }
    ("application/octet-stream".to_string(), url.to_string())
}

fn assistant_blocks(message: &Message) -> Vec<Value> {
    let mut blocks = Vec::new();
    if let Some(thinking) = &message.thinking {
        // Anthropic rejects replayed thinking blocks without a signature, which
        // is how reasoning captured from OpenAI-compatible providers (GLM,
        // DeepSeek) arrives, so those are dropped rather than sent.
        blocks.extend(thinking.iter().filter(|block| signed(block)).cloned());
    }
    if let Some(content) = &message.content {
        let text = content.display();
        if !text.is_empty() {
            blocks.push(json!({ "type": "text", "text": text }));
        }
    }
    if let Some(calls) = &message.tool_calls {
        for call in calls {
            let input = serde_json::from_str::<Value>(&call.function.arguments)
                .unwrap_or_else(|_| json!({}));
            blocks.push(json!({
                "type": "tool_use",
                "id": call.id,
                "name": call.function.name,
                "input": input,
            }));
        }
    }
    blocks
}

fn signed(block: &Value) -> bool {
    block["type"] == "redacted_thinking"
        || block["signature"]
            .as_str()
            .is_some_and(|signature| !signature.is_empty())
}

fn tool_result_block(message: &Message) -> Value {
    let content = match &message.content {
        None => json!(""),
        Some(MessageContent::Text(text)) => json!(text),
        Some(MessageContent::Parts(parts)) => {
            let blocks: Vec<Value> = parts
                .iter()
                .filter_map(|part| match part {
                    ContentPart::Text { text } => Some(json!({ "type": "text", "text": text })),
                    ContentPart::ImageUrl { image_url } => {
                        Some(data_block("image", &image_url.url))
                    }
                    ContentPart::File { .. } => None,
                })
                .collect();
            json!(blocks)
        }
    };
    json!({
        "type": "tool_result",
        "tool_use_id": message.tool_call_id.clone().unwrap_or_default(),
        "content": content,
    })
}

fn tool_schema(spec: &ToolSpec) -> Value {
    json!({
        "name": spec.function.name,
        "description": spec.function.description,
        "input_schema": spec.function.parameters,
    })
}

fn cache_control() -> Value {
    json!({ "type": "ephemeral" })
}

/// Marks the last cacheable block of the final user/tool-result message, so the
/// whole prefix up to the newest turn is served from the prompt cache.
fn apply_cache_breakpoint(message: &mut Value) {
    if !matches!(message["role"].as_str(), Some("user") | Some("system")) {
        return;
    }
    let Some(blocks) = message["content"].as_array_mut() else {
        return;
    };
    let Some(last) = blocks.last_mut() else {
        return;
    };
    if matches!(
        last["type"].as_str(),
        Some("text") | Some("image") | Some("tool_result")
    ) {
        last["cache_control"] = cache_control();
    }
}

fn is_tool_result(message: &Value) -> bool {
    message["content"]
        .as_array()
        .and_then(|blocks| blocks.first())
        .map(|block| block["type"] == "tool_result")
        .unwrap_or(false)
}

/// Anthropic requires strictly alternating roles, so fold any adjacent
/// same-role messages into one.
fn merge_adjacent(messages: Vec<Value>) -> Vec<Value> {
    let mut merged: Vec<Value> = Vec::new();
    for message in messages {
        if let Some(last) = merged.last_mut() {
            if last["role"] == message["role"] {
                if let (Some(target), Some(extra)) = (
                    last["content"].as_array_mut(),
                    message["content"].as_array(),
                ) {
                    target.extend(extra.iter().cloned());
                    continue;
                }
            }
        }
        merged.push(message);
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::{FunctionSpec, ImageUrl};

    fn config() -> Config {
        Config {
            provider: "anthropic".into(),
            model: "claude-3-5-sonnet-latest".into(),
            max_tokens: 4096,
            ..Config::default()
        }
    }

    fn spec(name: &str) -> ToolSpec {
        ToolSpec {
            kind: "function",
            function: FunctionSpec {
                name: name.into(),
                description: "does a thing".into(),
                parameters: json!({"type": "object", "properties": {}}),
            },
        }
    }

    fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: name.into(),
                arguments: arguments.into(),
            },
        }
    }

    #[test]
    fn builds_system_tool_and_tool_result_blocks() {
        let messages = vec![
            Message::system("be helpful"),
            Message::user("hi"),
            Message::assistant("", vec![call("toolu_1", "bash", "{\"command\":\"ls\"}")]),
            Message::tool("toolu_1", "files"),
        ];
        let body = request_body(&config(), &messages, &[spec("bash")]);
        assert_eq!(body["system"][0]["text"], "be helpful");
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["tools"][0]["name"], "bash");
        assert_eq!(body["tools"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");

        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["content"][0]["type"], "tool_use");
        assert_eq!(msgs[1]["content"][0]["input"]["command"], "ls");
        assert_eq!(msgs[2]["role"], "user");
        assert_eq!(msgs[2]["content"][0]["type"], "tool_result");
        assert_eq!(msgs[2]["content"][0]["tool_use_id"], "toolu_1");
    }

    #[test]
    fn caches_the_conversation_prefix_for_anthropic() {
        let messages = vec![
            Message::system("be helpful"),
            Message::user("first"),
            Message::assistant("ok", vec![]),
            Message::tool("toolu_1", "files"),
        ];
        let body = request_body(&config(), &messages, &[spec("bash"), spec("read")]);

        // The system prompt and tool list get a breakpoint.
        assert_eq!(body["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(body["tools"][1]["cache_control"]["type"], "ephemeral");
        assert!(body["tools"][0].get("cache_control").is_none());

        // The last tool result carries the final breakpoint.
        let msgs = body["messages"].as_array().unwrap();
        let last = msgs.last().unwrap();
        assert_eq!(last["role"], "user");
        let block = last["content"].as_array().unwrap().last().unwrap();
        assert_eq!(block["type"], "tool_result");
        assert_eq!(block["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn converts_image_data_url_to_base64_source() {
        let image = ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "data:image/png;base64,AAAA".into(),
                detail: None,
            },
        };
        let body = request_body(&config(), &[Message::user_parts("look", vec![image])], &[]);
        let block = &body["messages"][0]["content"][1];
        assert_eq!(block["type"], "image");
        assert_eq!(block["source"]["type"], "base64");
        assert_eq!(block["source"]["media_type"], "image/png");
        assert_eq!(block["source"]["data"], "AAAA");
    }

    #[test]
    fn enables_thinking_when_reasoning_requested() {
        let cfg = Config {
            model: "claude-sonnet-4".into(),
            max_tokens: 8192,
            reasoning: Reasoning::High,
            ..config()
        };
        let body = request_body(&cfg, &[Message::user("hi")], &[]);
        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["thinking"]["budget_tokens"], 7168);
    }

    #[test]
    fn adaptive_models_use_native_thinking_and_effort() {
        let cfg = Config {
            model: "claude-opus-4-8".into(),
            reasoning: Reasoning::Auto,
            ..config()
        };
        let body = request_body(&cfg, &[Message::user("hi")], &[]);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(body.get("output_config").is_none());

        let cfg = Config {
            reasoning: Reasoning::Medium,
            ..cfg
        };
        let body = request_body(&cfg, &[Message::user("hi")], &[]);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert_eq!(body["output_config"]["effort"], "medium");
    }

    #[test]
    fn omits_thinking_when_reasoning_off() {
        let cfg = Config {
            reasoning: Reasoning::Off,
            ..config()
        };
        let body = request_body(&cfg, &[Message::user("hi")], &[]);
        assert!(body.get("thinking").is_none());
    }

    #[test]
    fn replays_thinking_blocks_before_tool_use() {
        let thinking = vec![json!({
            "type": "thinking",
            "thinking": "hmm",
            "signature": "sig"
        })];
        let message =
            Message::assistant("", vec![call("toolu_1", "bash", "{}")]).with_thinking(thinking);
        let body = request_body(&config(), &[message], &[spec("bash")]);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], "sig");
        assert_eq!(content[1]["type"], "tool_use");
    }

    #[test]
    fn parses_thinking_stream_events() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        let mut streamed = String::new();
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"step"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":" two"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"abc"}}"#,
        ];
        for event in events {
            apply_event(event, &mut turn, &mut partials, &mut |_| {}, &mut |delta| {
                streamed.push_str(&delta)
            })
            .unwrap();
        }
        assert_eq!(turn.thinking.len(), 1);
        assert_eq!(turn.thinking[0]["thinking"], "step two");
        assert_eq!(turn.thinking[0]["signature"], "abc");
        assert_eq!(streamed, "step two");
    }

    #[test]
    fn unsigned_thinking_blocks_are_not_replayed() {
        let signed = json!({
            "type": "thinking",
            "thinking": "native reasoning",
            "signature": "sig"
        });
        let unsigned = json!({"type": "thinking", "thinking": "glm reasoning"});
        let message = Message::assistant("answer", vec![call("toolu_1", "bash", "{}")])
            .with_thinking(vec![signed, unsigned]);
        let body = request_body(&config(), &[message], &[spec("bash")]);
        let content = body["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content.len(), 3);
        assert_eq!(content[0]["thinking"], "native reasoning");
        assert_eq!(content[1]["type"], "text");
    }

    #[test]
    fn merges_consecutive_tool_results() {
        let messages = vec![
            Message::assistant("", vec![call("a", "x", "{}"), call("b", "y", "{}")]),
            Message::tool("a", "one"),
            Message::tool("b", "two"),
        ];
        let body = request_body(&config(), &messages, &[]);
        let msgs = body["messages"].as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        let results = msgs[1]["content"].as_array().unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[1]["tool_use_id"], "b");
    }

    #[test]
    fn thinking_deltas_accumulate_without_a_start_event() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        let event = r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"orphan"}}"#;
        apply_event(event, &mut turn, &mut partials, &mut |_| {}, &mut |_| {}).unwrap();
        assert_eq!(turn.thinking.len(), 1);
        assert_eq!(turn.thinking[0]["thinking"], "orphan");
    }

    #[test]
    fn records_the_stop_reason_from_message_delta() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        apply_event(
            r#"{"type":"message_delta","delta":{"stop_reason":"max_tokens"},"usage":{"output_tokens":8192}}"#,
            &mut turn,
            &mut partials,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();
        assert_eq!(turn.finish_reason.as_deref(), Some("max_tokens"));
        assert_eq!(turn.usage.output, 8192);
    }

    #[test]
    fn parses_text_and_tool_stream_events() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        let mut text = String::new();
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hello"}}"#,
            r#"{"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_9","name":"bash"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}"#,
            r#"{"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}"#,
        ];
        for event in events {
            apply_event(
                event,
                &mut turn,
                &mut partials,
                &mut |delta| text.push_str(&delta),
                &mut |_| {},
            )
            .unwrap();
        }
        assert_eq!(turn.content, "Hello");
        assert_eq!(text, "Hello");
        let calls = into_tool_calls(partials);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].id, "toolu_9");
        assert_eq!(calls[0].function.name, "bash");
        assert_eq!(calls[0].function.arguments, "{\"command\":\"ls\"}");
    }

    #[test]
    fn surfaces_stream_errors() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        let err = apply_event(
            r#"{"type":"error","error":{"type":"overloaded_error","message":"busy"}}"#,
            &mut turn,
            &mut partials,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap_err();
        assert!(err.to_string().contains("busy"));
    }
}
