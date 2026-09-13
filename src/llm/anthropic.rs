use crate::config::Config;
use crate::llm::types::{
    AssistantTurn, ContentPart, FunctionCall, Message, MessageContent, ToolCall, ToolSpec,
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

    let mut body = json!({
        "model": config.model,
        "max_tokens": config.max_tokens,
        "messages": merge_adjacent(converted),
        "stream": true,
    });
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if !tools.is_empty() {
        let specs: Vec<Value> = tools.iter().map(tool_schema).collect();
        body["tools"] = json!(specs);
    }
    if let Some(budget) = config
        .effective_reasoning()
        .budget_tokens(config.max_tokens)
    {
        body["thinking"] = json!({ "type": "enabled", "budget_tokens": budget });
    }
    body
}

/// Apply one parsed SSE event to the in-progress assistant turn.
pub fn apply_event(
    data: &str,
    turn: &mut AssistantTurn,
    partials: &mut BTreeMap<usize, PartialToolCall>,
    on_text: &mut dyn FnMut(String),
) -> Result<()> {
    let Ok(value) = serde_json::from_str::<Value>(data) else {
        return Ok(());
    };

    match value["type"].as_str() {
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
                        if let Some(block) = turn.thinking.last_mut() {
                            let mut text =
                                block["thinking"].as_str().unwrap_or_default().to_string();
                            text.push_str(fragment);
                            block["thinking"] = json!(text);
                        }
                    }
                }
                Some("signature_delta") => {
                    if let Some(signature) = delta["signature"].as_str() {
                        if let Some(block) = turn.thinking.last_mut() {
                            block["signature"] = json!(signature);
                        }
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
        blocks.extend(thinking.iter().cloned());
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
    use crate::config::Reasoning;
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
        assert_eq!(body["system"], "be helpful");
        assert_eq!(body["max_tokens"], 4096);
        assert_eq!(body["tools"][0]["name"], "bash");
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
        let events = [
            r#"{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"step"}}"#,
            r#"{"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"abc"}}"#,
        ];
        for event in events {
            apply_event(event, &mut turn, &mut partials, &mut |_| {}).unwrap();
        }
        assert_eq!(turn.thinking.len(), 1);
        assert_eq!(turn.thinking[0]["thinking"], "step");
        assert_eq!(turn.thinking[0]["signature"], "abc");
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
            apply_event(event, &mut turn, &mut partials, &mut |delta| {
                text.push_str(&delta)
            })
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
        )
        .unwrap_err();
        assert!(err.to_string().contains("busy"));
    }
}
