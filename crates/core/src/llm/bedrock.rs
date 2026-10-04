use crate::config::{Config, Reasoning};
use crate::llm::types::{
    repair_tool_pairs, AssistantTurn, ContentPart, FunctionCall, Message, MessageContent, ToolCall,
    ToolSpec,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

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
    if let Some(object) = value.as_object() {
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
        if let Some(message) = object.get("message").and_then(Value::as_str) {
            bail!("bedrock error: {message}");
        }
    }

    if let Some(usage) = value["metadata"]["usage"].as_object() {
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

    if let Some(reason) = value["messageStop"]["stopReason"].as_str() {
        // Bedrock spells the output limit `max_tokens`; oxide's escalation path
        // keys off the OpenAI spelling.
        turn.finish_reason = Some(match reason {
            "max_tokens" => "length".to_string(),
            other => other.to_string(),
        });
    }

    // A block's number lives inside the event that names it.
    let Some(start) = value["contentBlockStart"].get("start") else {
        if let Some(change) = value.get("contentBlockDelta") {
            let index = change["contentBlockIndex"].as_u64().unwrap_or(0) as usize;
            return apply_delta(
                &change["delta"],
                index,
                partials,
                turn,
                on_text,
                on_thinking,
            );
        }
        return Ok(());
    };
    if let Some(call) = start.get("toolUse").and_then(Value::as_object) {
        let index = value["contentBlockStart"]["contentBlockIndex"]
            .as_u64()
            .unwrap_or(0) as usize;
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

    #[test]
    fn streams_text_and_a_tool_call_split_across_deltas() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        let mut text = String::new();
        let mut on_text = |value: String| text.push_str(&value);

        for event in [
            r#"{"contentBlockDelta":{"delta":{"text":"he"},"contentBlockIndex":0}}"#,
            r#"{"contentBlockDelta":{"delta":{"text":"llo"},"contentBlockIndex":0}}"#,
            r#"{"contentBlockStart":{"contentBlockIndex":1,"start":{"toolUse":{"toolUseId":"t1","name":"read"}}}}"#,
            // The arguments arrive in fragments, so the block is only a call
            // once the last one has landed.
            r#"{"contentBlockDelta":{"contentBlockIndex":1,"delta":{"toolUse":{"input":"{\"pa"}}}}"#,
            r#"{"contentBlockDelta":{"contentBlockIndex":1,"delta":{"toolUse":{"input":"th\":\"a.rs\"}"}}}}"#,
            r#"{"contentBlockStop":{"contentBlockIndex":1}}"#,
            r#"{"messageStop":{"stopReason":"tool_use"}}"#,
            r#"{"metadata":{"usage":{"inputTokens":10,"outputTokens":4,"cacheReadInputTokens":6}}}"#,
        ] {
            apply_event(event, &mut turn, &mut partials, &mut on_text, &mut |_| {}).unwrap();
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
        for event in [
            r#"{"contentBlockDelta":{"contentBlockIndex":0,"delta":{"reasoningContent":{"text":"step "}}}}"#,
            r#"{"contentBlockDelta":{"contentBlockIndex":0,"delta":{"reasoningContent":{"text":"one"}}}}"#,
            r#"{"contentBlockDelta":{"contentBlockIndex":0,"delta":{"reasoningContent":{"signature":"sig"}}}}"#,
        ] {
            apply_event(
                event,
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
    fn reads_the_output_limit_as_truncation() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        apply_event(
            r#"{"messageStop":{"stopReason":"max_tokens"}}"#,
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
            r#"{"message":"the model is not available"}"#,
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
