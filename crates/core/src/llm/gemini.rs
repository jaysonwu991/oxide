use crate::config::{Config, Reasoning};
use crate::llm::types::{
    push_thinking, repair_tool_pairs, set_thinking_signature, AssistantTurn, ContentPart,
    FunctionCall, Message, MessageContent, ToolCall, ToolSpec,
};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Function calls Gemini streams, keyed by the order the parts arrive in.
/// Gemini sends each call whole (its arguments are an object, not a JSON
/// fragment), so a partial here is only ever written once. The signature a
/// thinking-capable model puts on the call belongs to it and is carried through
/// to the request that answers it.
#[derive(Debug, Default)]
pub(crate) struct PartialToolCall {
    name: String,
    arguments: String,
    signature: Option<String>,
}

/// Translate the internal OpenAI-style conversation into a Gemini
/// `generateContent` request body.
///
/// The shapes differ in more than naming: Gemini keeps the system prompt in
/// `systemInstruction`, calls the assistant `model`, returns tool results as
/// `functionResponse` parts named by the *function* rather than by an id, and
/// has no `tool_call_id` to quote back. The conversation is walked once so the
/// id of each call can be matched to the name its result must carry.
pub fn request_body(config: &Config, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let mut system = String::new();
    let mut contents: Vec<Value> = Vec::new();
    let mut names: BTreeMap<String, String> = BTreeMap::new();

    for message in &repair_tool_pairs(messages) {
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
            "user" => push_content(&mut contents, "user", user_parts(message)),
            "assistant" => {
                for call in message.tool_calls.iter().flatten() {
                    names.insert(call.id.clone(), call.function.name.clone());
                }
                let parts = assistant_parts(message);
                if !parts.is_empty() {
                    push_content(&mut contents, "model", parts);
                }
            }
            "tool" => {
                let id = message.tool_call_id.clone().unwrap_or_default();
                let name = names.get(&id).cloned().unwrap_or(id);
                let response = match &message.content {
                    Some(MessageContent::Parts(parts)) => {
                        json!({ "output": parts.iter().map(part_text).collect::<String>() })
                    }
                    Some(MessageContent::Text(text)) => json!({ "output": text }),
                    None => json!({ "output": "" }),
                };
                push_content(
                    &mut contents,
                    "user",
                    vec![json!({ "functionResponse": { "name": name, "response": response } })],
                );
            }
            _ => {}
        }
    }

    let mut body = json!({
        "contents": contents,
        "generationConfig": { "maxOutputTokens": config.max_tokens },
    });
    if !system.is_empty() {
        body["systemInstruction"] = json!({ "parts": [{ "text": system }] });
    }
    if !tools.is_empty() {
        let declarations: Vec<Value> = tools.iter().map(tool_declaration).collect();
        body["tools"] = json!([{ "functionDeclarations": declarations }]);
    }
    if let Some(thinking) = thinking_config(config) {
        body["generationConfig"]["thinkingConfig"] = thinking;
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
    if let Some(error) = value["error"].as_object() {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        bail!("gemini error: {message}");
    }

    let usage = &value["usageMetadata"];
    if let Some(tokens) = usage["promptTokenCount"].as_u64() {
        let cached = usage["cachedContentTokenCount"].as_u64().unwrap_or(0);
        turn.usage.input = tokens.saturating_sub(cached);
        turn.usage.cache_read = cached;
    }
    if let Some(tokens) = usage["candidatesTokenCount"].as_u64() {
        turn.usage.output = tokens;
    }
    if let Some(tokens) = usage["thoughtsTokenCount"].as_u64() {
        turn.usage.reasoning = tokens;
    }

    let Some(candidate) = value["candidates"].as_array().and_then(|list| list.first()) else {
        return Ok(());
    };
    if let Some(reason) = candidate["finishReason"].as_str() {
        if !reason.is_empty() {
            // Gemini reports the output limit as `MAX_TOKENS`; oxide's own
            // escalation path keys off the OpenAI/Anthropic spelling.
            turn.finish_reason = Some(if reason == "MAX_TOKENS" {
                "length".to_string()
            } else {
                reason.to_ascii_lowercase()
            });
        }
    }

    let Some(parts) = candidate["content"]["parts"].as_array() else {
        return Ok(());
    };
    for (index, part) in parts.iter().enumerate() {
        match part {
            _ if part["functionCall"].is_object() => {
                let call = &part["functionCall"];
                let name = call["name"].as_str().unwrap_or_default().to_string();
                if name.is_empty() {
                    continue;
                }
                let arguments = match call["args"].as_object() {
                    // The arguments arrive as an object here, while the rest of
                    // oxide keeps them as the JSON text a tool call carries.
                    Some(_) => call["args"].to_string(),
                    None => "{}".to_string(),
                };
                // The signature on a function call belongs to that part and has
                // to be handed back on it, so it is kept with the call rather
                // than with the thinking text.
                let signature = part["thoughtSignature"].as_str().map(str::to_string);
                partials.insert(
                    index,
                    PartialToolCall {
                        name,
                        arguments,
                        signature,
                    },
                );
            }
            _ if part["text"].is_string() => {
                let text = part["text"].as_str().unwrap_or_default();
                let thought = part["thought"] == json!(true);
                if !text.is_empty() {
                    if thought {
                        push_thinking(&mut turn.thinking, text);
                        on_thinking(text.to_string());
                    } else {
                        turn.content.push_str(text);
                        on_text(text.to_string());
                    }
                }
                // The signature is recorded after the fragment it belongs to,
                // so it lands on the block that fragment created. A signature on
                // a function call is not replayed here: it belongs to that part,
                // and the call carries it.
                if let Some(signature) = part["thoughtSignature"].as_str() {
                    set_thinking_signature(&mut turn.thinking, signature);
                }
            }
            _ => {}
        }
    }

    Ok(())
}

pub fn into_tool_calls(partials: BTreeMap<usize, PartialToolCall>) -> Vec<ToolCall> {
    partials
        .into_values()
        .filter(|partial| !partial.name.is_empty())
        .enumerate()
        .map(|(index, partial)| ToolCall {
            // Gemini has no call ids, so one is minted here and resolved back to
            // the function name when the result is replayed.
            id: format!("call_{index}"),
            kind: "function".to_string(),
            signature: partial.signature,
            function: FunctionCall {
                name: partial.name,
                arguments: partial.arguments,
            },
        })
        .collect()
}

/// The model ids a Gemini listing endpoint reports. The names are qualified
/// (`models/gemini-2.0-flash`), while every other provider hands back the bare
/// id the request expects, so the prefix is taken off here.
pub fn parse_model_list(value: &Value) -> Vec<String> {
    let mut models: Vec<String> = value["models"]
        .as_array()
        .map(|list| {
            list.iter()
                .filter_map(|model| model["name"].as_str())
                .map(|name| name.strip_prefix("models/").unwrap_or(name).to_string())
                .collect()
        })
        .unwrap_or_default();
    models.sort();
    models.dedup();
    models
}

/// Appends parts to the contents, folding them into the previous turn when it
/// already carries this role: Gemini rejects two consecutive contents with the
/// same role, and a model turn whose function calls each got their own result
/// would otherwise become one.
fn push_content(contents: &mut Vec<Value>, role: &str, parts: Vec<Value>) {
    if parts.is_empty() {
        return;
    }
    if let Some(last) = contents.last_mut() {
        if last["role"] == json!(role) {
            if let Some(existing) = last["parts"].as_array_mut() {
                existing.extend(parts);
                return;
            }
        }
    }
    contents.push(json!({ "role": role, "parts": parts }));
}

fn user_parts(message: &Message) -> Vec<Value> {
    match &message.content {
        None => Vec::new(),
        Some(MessageContent::Text(text)) => vec![json!({ "text": text })],
        Some(MessageContent::Parts(parts)) => parts.iter().map(content_part).collect(),
    }
}

fn content_part(part: &ContentPart) -> Value {
    match part {
        ContentPart::Text { text } => json!({ "text": text }),
        ContentPart::ImageUrl { image_url } => inline_data("image_url", &image_url.url),
        ContentPart::File { file } => inline_data("file_data", &file.file_data),
    }
}

/// Gemini takes media as bytes on the request rather than as a URL, so both an
/// image and a document become one `inlineData` part. A part that is not a data
/// URL has no bytes to inline and is handed over as text so the model can still
/// see what was attached.
fn inline_data(_field: &str, value: &str) -> Value {
    let Some(rest) = value.strip_prefix("data:") else {
        return json!({ "text": value });
    };
    let Some((meta, data)) = rest.split_once(',') else {
        return json!({ "text": value });
    };
    let mime_type = meta.strip_suffix(";base64").unwrap_or(meta);
    json!({ "inlineData": { "mimeType": mime_type, "data": data } })
}

fn part_text(part: &ContentPart) -> String {
    match part {
        ContentPart::Text { text } => text.clone(),
        ContentPart::ImageUrl { .. } => "[image]".to_string(),
        ContentPart::File { file } => format!(
            "[file: {}]",
            file.filename.clone().unwrap_or_else(|| "attachment".into())
        ),
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
            let args = serde_json::from_str::<Value>(&call.function.arguments)
                .unwrap_or_else(|_| json!({}));
            let mut part = json!({
                "functionCall": { "name": call.function.name, "args": args },
            });
            // A signature the model put on this call is handed back on the same
            // part, since a thinking-capable model refuses a call whose
            // signature went missing.
            if let Some(signature) = call.signature.as_deref().filter(|s| !s.is_empty()) {
                part["thoughtSignature"] = json!(signature);
            }
            parts.push(part);
        }
    }
    // A thought signature is replayed on the first part of the model turn it
    // came from, which is where Gemini expects to find it. It belongs to the
    // text it was streamed with, so it is never written over a call's own.
    if let Some(signature) = message
        .thinking
        .iter()
        .flatten()
        .find_map(|block| block["signature"].as_str())
        .filter(|signature| !signature.is_empty())
    {
        if let Some(first) = parts
            .first_mut()
            .filter(|part| part["functionCall"].is_null())
        {
            first["thoughtSignature"] = json!(signature);
        }
    }
    parts
}

fn tool_declaration(spec: &ToolSpec) -> Value {
    json!({
        "name": spec.function.name,
        "description": spec.function.description,
        "parameters": sanitize_schema(spec.function.parameters.clone()),
    })
}

/// Narrows a JSON Schema to the OpenAPI 3.0 subset Gemini accepts. The keywords
/// outside that subset are rejected with a 400 rather than ignored, and the
/// schema oxide ships is written for the OpenAI-compatible wire, so each one
/// that has a Gemini spelling is rewritten and the rest are dropped.
fn sanitize_schema(schema: Value) -> Value {
    match schema {
        Value::Array(items) => Value::Array(items.into_iter().map(sanitize_schema).collect()),
        Value::Object(mut map) => {
            let mut cleaned = serde_json::Map::new();
            for (key, value) in map.iter_mut() {
                match key.as_str() {
                    // Rejected outright by Gemini, or meaningful only to JSON
                    // Schema's own validators.
                    "$schema"
                    | "$id"
                    | "additionalProperties"
                    | "strict"
                    | "title"
                    | "default"
                    | "examples"
                    | "exclusiveMinimum"
                    | "exclusiveMaximum"
                    | "const" => continue,
                    // Gemini spells a nullable type as OpenAPI 3.0 does: one
                    // type, plus a `nullable` flag.
                    "type" => {
                        if let Some(types) = value.as_array() {
                            let nullable = types.iter().any(|entry| entry == "null");
                            let named = types
                                .iter()
                                .filter(|entry| *entry != "null")
                                .find_map(Value::as_str);
                            if let Some(named) = named {
                                cleaned.insert("type".to_string(), json!(named));
                            }
                            if nullable {
                                cleaned.insert("nullable".to_string(), json!(true));
                            }
                            continue;
                        }
                    }
                    _ => {}
                }
                cleaned.insert(key.clone(), sanitize_schema(value.clone()));
            }
            Value::Object(cleaned)
        }
        other => other,
    }
}

/// How the configured thinking level is expressed to Gemini. The 3.x family
/// takes a level, everything from 2.5 takes a token budget, and `auto` leaves
/// the model's own default in place.
fn thinking_config(config: &Config) -> Option<Value> {
    let gemini = |level: &str| Some(json!({ "thinkingLevel": level, "includeThoughts": true }));
    let budget = |tokens: u32| Some(json!({ "thinkingBudget": tokens, "includeThoughts": true }));
    let newest = config.model.to_ascii_lowercase().contains("gemini-3");
    match config.effective_reasoning() {
        Reasoning::Auto => None,
        // A model that always thinks rejects a disabled budget, so the lowest
        // one it accepts is asked for instead.
        Reasoning::Off if newest => gemini("low"),
        Reasoning::Off if config.model.to_ascii_lowercase().contains("pro") => budget(128),
        Reasoning::Off => budget(0),
        level if newest => gemini(match level {
            Reasoning::Low => "low",
            _ => "high",
        }),
        level => budget(level.budget_tokens(config.max_tokens).unwrap_or(1024)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm::types::Message;

    fn config() -> Config {
        Config {
            provider: "google".to_string(),
            model: "gemini-2.5-flash".to_string(),
            max_tokens: 8192,
            ..Default::default()
        }
    }

    #[test]
    fn moves_the_system_prompt_out_of_the_contents() {
        let messages = vec![
            Message::system("be brief"),
            Message::user("hello"),
            Message::assistant("hi", Vec::new()),
        ];
        let body = request_body(&config(), &messages, &[]);
        assert_eq!(body["systemInstruction"]["parts"][0]["text"], "be brief");
        assert_eq!(body["contents"][0]["role"], "user");
        assert_eq!(body["contents"][0]["parts"][0]["text"], "hello");
        assert_eq!(body["contents"][1]["role"], "model");
    }

    #[test]
    fn names_a_tool_result_after_the_call_it_answers() {
        let assistant = Message::assistant(
            "",
            vec![ToolCall {
                id: "call_0".to_string(),
                kind: "function".to_string(),
                signature: None,
                function: FunctionCall {
                    name: "read".to_string(),
                    arguments: "{\"path\":\"a.rs\"}".to_string(),
                },
            }],
        );
        let result = Message::tool("call_0", "done");

        let body = request_body(&config(), &[Message::user("go"), assistant, result], &[]);
        // Gemini names the function, not the call id, so the result has to be
        // resolved back through the call it answers.
        assert_eq!(
            body["contents"][2]["parts"][0]["functionResponse"]["name"],
            "read"
        );
        assert_eq!(
            body["contents"][1]["parts"][0]["functionCall"]["args"]["path"],
            "a.rs"
        );
    }

    #[test]
    fn folds_consecutive_results_into_one_turn() {
        let assistant = Message::assistant(
            "",
            vec![
                ToolCall {
                    id: "call_0".to_string(),
                    kind: "function".to_string(),
                    signature: None,
                    function: FunctionCall {
                        name: "read".to_string(),
                        arguments: "{}".to_string(),
                    },
                },
                ToolCall {
                    id: "call_1".to_string(),
                    kind: "function".to_string(),
                    signature: None,
                    function: FunctionCall {
                        name: "ls".to_string(),
                        arguments: "{}".to_string(),
                    },
                },
            ],
        );
        let first = Message::tool("call_0", "a");
        let second = Message::tool("call_1", "b");

        let body = request_body(
            &config(),
            &[Message::user("go"), assistant, first, second],
            &[],
        );
        assert_eq!(body["contents"].as_array().unwrap().len(), 3);
        assert_eq!(body["contents"][2]["parts"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn streams_text_reasoning_and_calls() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        let mut text = String::new();
        let mut thinking = String::new();
        let mut on_text = |value: String| text.push_str(&value);
        let mut on_thinking = |value: String| thinking.push_str(&value);

        apply_event(
            r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"because","thought":true},{"text":"hi"}]},"finishReason":"STOP"}],"usageMetadata":{"promptTokenCount":10,"candidatesTokenCount":4,"cachedContentTokenCount":6,"thoughtsTokenCount":2}}"#,
            &mut turn,
            &mut partials,
            &mut on_text,
            &mut on_thinking,
        )
        .unwrap();

        assert_eq!(text, "hi");
        assert_eq!(thinking, "because");
        assert_eq!(turn.thinking[0]["thinking"], "because");
        assert_eq!(turn.content, "hi");
        assert_eq!(turn.usage.input, 4);
        assert_eq!(turn.usage.cache_read, 6);
        assert_eq!(turn.usage.output, 4);
        assert_eq!(turn.usage.reasoning, 2);
        assert_eq!(turn.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn reads_the_output_limit_as_truncation() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        apply_event(
            r#"{"candidates":[{"content":{"parts":[]},"finishReason":"MAX_TOKENS"}]}"#,
            &mut turn,
            &mut partials,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();
        // The escalation path in `stream_chat` keys off the OpenAI spelling.
        assert_eq!(turn.finish_reason.as_deref(), Some("length"));
    }

    #[test]
    fn mints_a_call_for_a_function_call_part() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        apply_event(
            r#"{"candidates":[{"content":{"parts":[{"functionCall":{"name":"grep","args":{"pattern":"x"}}}]}}]}"#,
            &mut turn,
            &mut partials,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();
        let calls = into_tool_calls(partials);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].function.name, "grep");
        assert_eq!(calls[0].function.arguments, r#"{"pattern":"x"}"#);
        assert!(calls[0].signature.is_none());
    }

    /// A thinking-capable model signs the function call it returns, and the
    /// signature has to come back on that same part: a call that arrives with
    /// one and is replayed without it is refused.
    #[test]
    fn keeps_and_replays_the_signature_a_function_call_arrived_with() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        apply_event(
            r#"{"candidates":[{"content":{"role":"model","parts":[{"thoughtSignature":"c2ln","functionCall":{"name":"read","args":{"path":"a.rs"}}}]},"finishReason":"STOP"}]}"#,
            &mut turn,
            &mut partials,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();

        let calls = into_tool_calls(partials);
        assert_eq!(calls[0].signature.as_deref(), Some("c2ln"));
        // The signature rides the call, not a thinking block, so a turn that
        // only called a tool has nothing else carrying it.
        assert!(turn
            .thinking
            .iter()
            .all(|block| block["signature"].is_null()));

        let assistant = Message::assistant("", calls);
        let body = request_body(
            &config(),
            &[
                Message::user("go"),
                assistant,
                Message::tool("call_0", "done"),
            ],
            &[],
        );
        let part = &body["contents"][1]["parts"][0];
        assert_eq!(part["functionCall"]["name"], "read");
        assert_eq!(part["thoughtSignature"], "c2ln");
    }

    /// A signature streamed with thinking text stays on the text part, and the
    /// one a call arrived with is not written over by it.
    #[test]
    fn gives_each_part_back_its_own_signature() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        apply_event(
            r#"{"candidates":[{"content":{"role":"model","parts":[{"text":"weighing","thought":true,"thoughtSignature":"dGhv"},{"functionCall":{"name":"ls","args":{}},"thoughtSignature":"Y2FsbA=="}]}}]}"#,
            &mut turn,
            &mut partials,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap();

        let calls = into_tool_calls(partials);
        assert_eq!(calls[0].signature.as_deref(), Some("Y2FsbA=="));

        let assistant = Message::assistant("ack", calls).with_thinking(turn.thinking.clone());
        let body = request_body(&config(), &[Message::user("go"), assistant], &[]);
        let parts = body["contents"][1]["parts"].as_array().unwrap();
        assert_eq!(parts[0]["text"], "ack");
        assert_eq!(parts[0]["thoughtSignature"], "dGhv");
        assert_eq!(parts[1]["functionCall"]["name"], "ls");
        assert_eq!(parts[1]["thoughtSignature"], "Y2FsbA==");
    }

    #[test]
    fn reports_a_stream_error() {
        let mut turn = AssistantTurn::default();
        let mut partials = BTreeMap::new();
        let error = apply_event(
            r#"{"error":{"message":"quota exceeded"}}"#,
            &mut turn,
            &mut partials,
            &mut |_| {},
            &mut |_| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("quota exceeded"));
    }

    #[test]
    fn strips_the_models_prefix_from_a_listing() {
        let models = parse_model_list(&json!({
            "models": [{ "name": "models/gemini-2.0-flash" }, { "name": "models/gemini-2.5-pro" }]
        }));
        assert_eq!(models, vec!["gemini-2.0-flash", "gemini-2.5-pro"]);
    }

    #[test]
    fn narrows_a_schema_to_the_subset_gemini_takes() {
        let schema = json!({
            "type": ["object", "null"],
            "additionalProperties": false,
            "properties": {
                "path": { "type": "string", "title": "Path" },
            },
            "required": ["path"],
        });
        let cleaned = sanitize_schema(schema);
        assert_eq!(cleaned["type"], "object");
        assert_eq!(cleaned["nullable"], true);
        assert!(cleaned.get("additionalProperties").is_none());
        assert!(cleaned["properties"]["path"].get("title").is_none());
        assert_eq!(cleaned["properties"]["path"]["type"], "string");
    }
}
