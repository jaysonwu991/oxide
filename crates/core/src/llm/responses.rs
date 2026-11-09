use super::types::{
    push_thinking, repair_tool_pairs, AssistantTurn, ContentPart, FunctionCall, Message,
    MessageContent, ToolCall, ToolSpec, Usage,
};
use crate::config::{Config, Reasoning};
use anyhow::{bail, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

pub fn supports_model(model: &str) -> bool {
    let lower = model.to_ascii_lowercase();
    let name = crate::config::vendorless(&lower);
    ["gpt-5", "o1", "o3", "o4"].iter().any(|family| {
        name == *family
            || name
                .strip_prefix(family)
                .is_some_and(|suffix| suffix.starts_with(['.', '-']))
    })
}

pub fn request_body(config: &Config, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let mut input = Vec::new();
    let messages = repair_tool_pairs(messages);
    for (index, message) in messages.iter().enumerate() {
        if message.role == "tool" {
            continue;
        }
        if message.role == "assistant" {
            for item in message.thinking.iter().flatten() {
                if item["type"] == "reasoning" && item["_oxide_model"] == config.model {
                    let mut item = item.clone();
                    item.as_object_mut().unwrap().remove("_oxide_model");
                    input.push(item);
                }
            }
        }
        let parts = content(message, message.role == "assistant");
        if !parts.is_empty() {
            let mut item = json!({"role":message.role,"content":parts});
            if message.role == "assistant" {
                item["phase"] = json!(if message
                    .tool_calls
                    .as_ref()
                    .is_some_and(|calls| !calls.is_empty())
                {
                    "commentary"
                } else {
                    "final_answer"
                });
            }
            input.push(item);
        }
        for call in message.tool_calls.iter().flatten() {
            input.push(json!({"type":"function_call", "call_id":call.id,
                "name":call.function.name,"arguments":call.function.arguments}));
            // Gateways may translate each call into a separate assistant turn.
            // Pair its result before the next call so that translated history
            // never contains an unanswered assistant tool call.
            if let Some(result) = messages[index + 1..]
                .iter()
                .take_while(|result| result.role == "tool")
                .find(|result| result.tool_call_id.as_deref() == Some(call.id.as_str()))
            {
                input.push(json!({"type":"function_call_output", "call_id":call.id,
                    "output":tool_output(result)}));
            }
        }
    }
    let mut body = json!({"model":config.model,"input":input,"stream":true,"store":false,
        "max_output_tokens":config.max_tokens,"include":["reasoning.encrypted_content"],
        "reasoning":{"summary":"auto"}});
    match config.effective_reasoning() {
        Reasoning::Auto => {}
        Reasoning::Off => {
            body["reasoning"] = json!({"effort":"none"});
        }
        level => {
            body["reasoning"]["effort"] = json!(level.effort());
        }
    }
    if !tools.is_empty() {
        body["tools"] = json!(tools
            .iter()
            .map(|tool| {
                let mut function = serde_json::to_value(&tool.function).unwrap();
                function["type"] = json!("function");
                // Existing tools have optional arguments and use non-strict schemas.
                function["strict"] = json!(false);
                function
            })
            .collect::<Vec<_>>());
    }
    body
}

fn tool_output(message: &Message) -> Value {
    // Gateways may translate Responses to Chat Completions without converting
    // input_text blocks in tool results. Plain text uses the shared string form.
    match &message.content {
        None => json!(""),
        Some(MessageContent::Text(text)) => json!(text),
        Some(MessageContent::Parts(parts))
            if parts
                .iter()
                .all(|part| matches!(part, ContentPart::Text { .. })) =>
        {
            json!(message.content.as_ref().unwrap().display())
        }
        _ => json!(content(message, false)),
    }
}

fn content(message: &Message, assistant: bool) -> Vec<Value> {
    let text =
        |value: &str| json!({"type":if assistant {"output_text"} else {"input_text"},"text":value});
    match &message.content {
        Some(MessageContent::Text(value)) if !value.is_empty() => vec![text(value)],
        Some(MessageContent::Parts(parts)) => parts.iter().map(|part| match part {
            ContentPart::Text { text: value } => text(value),
            ContentPart::ImageUrl { image_url } => json!({"type":"input_image","image_url":image_url.url,"detail":image_url.detail.as_deref().unwrap_or("auto")}),
            ContentPart::File { file } => json!({"type":"input_file","filename":file.filename.as_deref().unwrap_or("document.pdf"),"file_data":file.file_data}),
        }).collect(),
        _ => Vec::new(),
    }
}

pub fn apply_event(
    event: &Value,
    turn: &mut AssistantTurn,
    items: &mut BTreeMap<u64, Value>,
    on_text: &mut dyn FnMut(String),
    on_thinking: &mut dyn FnMut(String),
) -> Result<()> {
    match event["type"].as_str().unwrap_or_default() {
        "response.output_text.delta" => {
            if let Some(text) = event["delta"].as_str() {
                turn.content.push_str(text);
                on_text(text.into());
            }
        }
        "response.reasoning_summary_text.delta" => {
            if let Some(text) = event["delta"].as_str() {
                push_thinking(&mut turn.thinking, text);
                on_thinking(text.into());
            }
        }
        "response.output_item.done" => {
            if let Some(index) = event["output_index"].as_u64() {
                items.insert(index, event["item"].clone());
            }
        }
        "response.completed" | "response.incomplete" => {
            let response = &event["response"];
            if let Some(output) = response["output"].as_array() {
                for (index, item) in output.iter().enumerate() {
                    items.insert(index as u64, item.clone());
                }
            }
            let usage = &response["usage"];
            let cached = usage
                .pointer("/input_tokens_details/cached_tokens")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            turn.usage = Usage {
                input: usage["input_tokens"]
                    .as_u64()
                    .unwrap_or(0)
                    .saturating_sub(cached),
                output: usage["output_tokens"].as_u64().unwrap_or(0),
                cache_read: cached,
                reasoning: usage
                    .pointer("/output_tokens_details/reasoning_tokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
                ..Usage::default()
            };
            if event["type"] == "response.incomplete" {
                let reason = response
                    .pointer("/incomplete_details/reason")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown");
                if reason != "max_output_tokens" {
                    bail!("provider response incomplete: {reason}");
                }
                turn.finish_reason = Some("length".into());
            } else {
                turn.finish_reason = Some("stop".into());
            }
        }
        "error" | "response.failed" => bail!("provider Responses error: {}", event),
        _ => {}
    }
    Ok(())
}

pub fn finish(config: &Config, turn: &mut AssistantTurn, items: BTreeMap<u64, Value>) {
    for mut item in items.into_values() {
        match item["type"].as_str() {
            Some("function_call") => {
                if let (Some(id), Some(name), Some(arguments)) = (
                    item["call_id"].as_str(),
                    item["name"].as_str(),
                    item["arguments"].as_str(),
                ) {
                    turn.tool_calls.push(ToolCall {
                        signature: None,
                        id: id.into(),
                        kind: "function".into(),
                        function: FunctionCall {
                            name: name.into(),
                            arguments: arguments.into(),
                        },
                    });
                }
            }
            Some("reasoning") if item["encrypted_content"].is_string() => {
                item["_oxide_model"] = json!(config.model);
                turn.thinking.push(item);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_reasoning_families_without_changing_non_reasoning_models() {
        for model in ["gpt-5.4", "gpt-5.6-sol", "OPENAI/GPT-5.6-TERRA", "o3-mini"] {
            assert!(supports_model(model));
        }
        for model in [
            "gpt-4o",
            "gpt-4.1",
            "glm-5.2",
            "claude-sonnet-5",
            "company-alias",
            "o123",
        ] {
            assert!(!supports_model(model));
        }
    }

    #[test]
    fn text_tool_results_use_strings_for_gateway_compatibility() {
        for message in [
            Message::tool("call_1", "read result"),
            Message::tool_parts("call_1", "read result", Vec::new()),
        ] {
            assert_eq!(tool_output(&message), json!("read result"));
        }
    }

    #[test]
    fn parallel_calls_replay_with_their_own_results_before_the_next_call() {
        let call = |id: &str| ToolCall {
            id: id.into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "lookup".into(),
                arguments: "{}".into(),
            },
            signature: None,
        };
        let body = request_body(
            &Config::default(),
            &[
                Message::user("Look up both values"),
                Message::assistant("", vec![call("first"), call("second"), call("interrupted")]),
                Message::tool("second", "second value"),
                Message::tool("first", "first value"),
                Message::assistant("Both checked", vec![]),
            ],
            &[],
        );
        let input = body["input"].as_array().unwrap();
        for (index, id) in [(1, "first"), (3, "second"), (5, "interrupted")] {
            assert_eq!(input[index]["type"], "function_call");
            assert_eq!(input[index]["call_id"], id);
            assert_eq!(input[index + 1]["type"], "function_call_output");
            assert_eq!(input[index + 1]["call_id"], id);
        }
        assert_eq!(input[2]["output"], "first value");
        assert_eq!(input[4]["output"], "second value");
        assert!(input[6]["output"].as_str().unwrap().starts_with("error:"));
        assert_eq!(input[7]["phase"], "final_answer");
        assert_eq!(input.len(), 8);
    }

    #[test]
    fn requests_preserve_tool_media_and_only_replay_same_model_reasoning() {
        let config = Config {
            model: "gpt-5.6-sol".into(),
            reasoning: Reasoning::High,
            ..Config::default()
        };
        let call = ToolCall {
            signature: None,
            id: "call_1".into(),
            kind: "function".into(),
            function: FunctionCall {
                name: "read".into(),
                arguments: r#"{"path":"a.rs"}"#.into(),
            },
        };
        let assistant = Message::assistant("Checking", vec![call]).with_thinking(vec![
            json!({"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque","_oxide_model":"gpt-5.6-sol"}),
            json!({"type":"thinking","thinking":"Claude summary","signature":"claude-signature"}),
        ]);
        let messages = vec![
            Message::user("Look"),
            assistant,
            Message::tool_parts(
                "call_1",
                "image",
                vec![ContentPart::ImageUrl {
                    image_url: super::super::types::ImageUrl {
                        url: "data:image/png;base64,abc".into(),
                        detail: None,
                    },
                }],
            ),
        ];
        let body = request_body(&config, &messages, &[]);
        assert_eq!(body["reasoning"], json!({"effort":"high","summary":"auto"}));
        assert_eq!(body["store"], false);
        assert_eq!(body["input"][1]["encrypted_content"], "opaque");
        assert!(body["input"][1].get("_oxide_model").is_none());
        assert_eq!(body["input"][2]["phase"], "commentary");
        assert_eq!(body["input"][3]["type"], "function_call");
        assert_eq!(body["input"][4]["output"][1]["type"], "input_image");
        let switched = Config {
            model: "gpt-5.6-terra".into(),
            reasoning: Reasoning::Off,
            ..config
        };
        let body = request_body(&switched, &messages, &[]);
        assert_eq!(body["reasoning"], json!({"effort":"none"}));
        assert!(!body.to_string().contains("opaque"));
        assert!(!body.to_string().contains("claude-signature"));
    }

    #[test]
    fn streams_summaries_tools_encrypted_state_and_usage() {
        let mut turn = AssistantTurn::default();
        let mut items = BTreeMap::new();
        let mut text = String::new();
        let mut thinking = String::new();
        let events = [
            json!({"type":"response.reasoning_summary_text.delta","delta":"Checking"}),
            json!({"type":"response.output_text.delta","delta":"Next"}),
            json!({"type":"response.output_item.done","output_index":1,"item":{"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"}}),
            json!({"type":"response.completed","response":{"output":[{"type":"reasoning","id":"rs_1","summary":[],"encrypted_content":"opaque"},{"type":"function_call","call_id":"call_1","name":"read","arguments":"{}"}],"usage":{"input_tokens":100,"input_tokens_details":{"cached_tokens":80},"output_tokens":40,"output_tokens_details":{"reasoning_tokens":30}}}}),
        ];
        for event in events {
            apply_event(
                &event,
                &mut turn,
                &mut items,
                &mut |s| text.push_str(&s),
                &mut |s| thinking.push_str(&s),
            )
            .unwrap();
        }
        finish(&Config::default(), &mut turn, items);
        assert_eq!(text, "Next");
        assert_eq!(thinking, "Checking");
        assert_eq!(turn.tool_calls.len(), 1);
        assert_eq!(turn.tool_calls[0].id, "call_1");
        assert_eq!(turn.usage.input, 20);
        assert_eq!(turn.usage.cache_read, 80);
        assert_eq!(turn.usage.reasoning, 30);
        assert_eq!(turn.thinking.len(), 2);
        assert_eq!(turn.finish_reason.as_deref(), Some("stop"));
    }

    #[test]
    fn errors_and_incomplete_responses_remain_visible() {
        let mut turn = AssistantTurn::default();
        let mut items = BTreeMap::new();
        for event in [
            json!({"type":"error","message":"bad effort"}),
            json!({"type":"response.failed","response":{"error":{"message":"denied"}}}),
            json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"content_filter"}}}),
        ] {
            assert!(apply_event(&event, &mut turn, &mut items, &mut |_| {}, &mut |_| {}).is_err());
        }
        apply_event(&json!({"type":"response.incomplete","response":{"incomplete_details":{"reason":"max_output_tokens"}}}),&mut turn,&mut items,&mut |_|{},&mut |_|{}).unwrap();
        assert_eq!(turn.finish_reason.as_deref(), Some("length"));
    }
}
