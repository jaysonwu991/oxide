use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<MessageContent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
    /// Anthropic extended-thinking blocks captured from the assistant turn so
    /// they can be replayed with tool results. Never sent to OpenAI as blocks;
    /// DeepSeek receives their text as `reasoning_content` instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Vec<Value>>,
}

/// Message content is either a plain string (the common case) or an ordered
/// list of parts when the message carries images or documents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
    File { file: FileData },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    pub file_data: String,
}

impl MessageContent {
    /// A human-readable rendering that marks media parts (used by the TUI).
    pub fn display(&self) -> String {
        match self {
            MessageContent::Text(text) => text.clone(),
            MessageContent::Parts(parts) => {
                let mut out = String::new();
                for part in parts {
                    let piece = match part {
                        ContentPart::Text { text } => text.clone(),
                        ContentPart::ImageUrl { .. } => "[image]".to_string(),
                        ContentPart::File { file } => format!(
                            "[file: {}]",
                            file.filename.clone().unwrap_or_else(|| "attachment".into())
                        ),
                    };
                    if piece.is_empty() {
                        continue;
                    }
                    if !out.is_empty() {
                        out.push('\n');
                    }
                    out.push_str(&piece);
                }
                out
            }
        }
    }

    pub fn from_text_and_parts(text: String, mut parts: Vec<ContentPart>) -> Self {
        if parts.is_empty() {
            return MessageContent::Text(text);
        }
        let mut all = Vec::with_capacity(parts.len() + 1);
        if !text.is_empty() {
            all.push(ContentPart::Text { text });
        }
        all.append(&mut parts);
        MessageContent::Parts(all)
    }
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: "system".into(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            thinking: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".into(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: None,
            thinking: None,
        }
    }

    /// A user message with attached media parts (images/documents).
    pub fn user_parts(text: impl Into<String>, parts: Vec<ContentPart>) -> Self {
        Self {
            role: "user".into(),
            content: Some(MessageContent::from_text_and_parts(text.into(), parts)),
            tool_calls: None,
            tool_call_id: None,
            thinking: None,
        }
    }

    pub fn assistant(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        let content = content.into();
        Self {
            role: "assistant".into(),
            content: if content.is_empty() {
                None
            } else {
                Some(MessageContent::Text(content))
            },
            tool_calls: if tool_calls.is_empty() {
                None
            } else {
                Some(tool_calls)
            },
            tool_call_id: None,
            thinking: None,
        }
    }

    pub fn tool(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: "tool".into(),
            content: Some(MessageContent::Text(content.into())),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            thinking: None,
        }
    }

    /// A tool result carrying media parts (e.g. an image read by `read_file`).
    pub fn tool_parts(
        tool_call_id: impl Into<String>,
        text: impl Into<String>,
        parts: Vec<ContentPart>,
    ) -> Self {
        Self {
            role: "tool".into(),
            content: Some(MessageContent::from_text_and_parts(text.into(), parts)),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
            thinking: None,
        }
    }

    /// The message text with media parts rendered as markers.
    pub fn display(&self) -> Option<String> {
        self.content.as_ref().map(MessageContent::display)
    }

    /// Attaches Anthropic thinking blocks when the turn produced any.
    pub fn with_thinking(mut self, thinking: Vec<Value>) -> Self {
        if !thinking.is_empty() {
            self.thinking = Some(thinking);
        }
        self
    }

    /// The reasoning text captured from an OpenAI-compatible provider, flattened
    /// from the stored thinking blocks so it can be replayed as
    /// `reasoning_content`. Returns `None` when the message carries no
    /// reasoning (or only signed/redacted Anthropic blocks).
    pub fn reasoning_content(&self) -> Option<String> {
        let mut text = String::new();
        for block in self.thinking.as_deref().unwrap_or_default() {
            if let Some(piece) = block["thinking"].as_str() {
                text.push_str(piece);
            }
        }
        (!text.is_empty()).then_some(text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub function: FunctionSpec,
}

#[derive(Debug, Clone, Serialize)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<Value>,
    pub stream: bool,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_config: Option<Value>,
    /// Cache-affinity key for direct OpenAI, so a follow-up turn hits the same
    /// prompt cache as the previous one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prompt_cache_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream_options: Option<StreamOptions>,
}

/// Asks OpenAI-compatible providers to include token usage on the stream.
#[derive(Debug, Serialize)]
pub struct StreamOptions {
    pub include_usage: bool,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    /// Output tokens the provider billed as hidden reasoning. OpenAI-family
    /// models never stream the text, so this is the only signal that an empty
    /// turn spent its budget thinking.
    pub reasoning: u64,
    /// Cost in USD, computed from the model's price table after the turn.
    pub cost: f64,
}

impl Usage {
    pub fn total(&self) -> u64 {
        self.input + self.output
    }
}

#[derive(Debug, Default)]
pub struct AssistantTurn {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub thinking: Vec<Value>,
    pub usage: Usage,
    /// Why the provider stopped the turn (`stop`, `length`, `tool_calls`, or
    /// Anthropic's `max_tokens`/`end_turn`). A reasoning model can consume the
    /// whole output budget on hidden reasoning and stop with `length` before
    /// emitting any content, which is worth reporting differently from a
    /// transient empty reply.
    pub finish_reason: Option<String>,
}

/// Appends a streamed reasoning fragment to the turn's thinking block, creating
/// the block on the first fragment.
///
/// Providers stream reasoning a fragment at a time, while the block is kept as a
/// `Value` because it is replayed to Anthropic verbatim. Appending in place
/// keeps that streaming linear: rebuilding the block per fragment copies the
/// whole trace each time, which costs milliseconds on a long trace.
pub fn push_thinking(blocks: &mut Vec<Value>, fragment: &str) {
    match blocks.last_mut() {
        Some(Value::Object(map)) => match map.get_mut("thinking") {
            Some(Value::String(text)) => text.push_str(fragment),
            _ => {
                map.insert("thinking".to_string(), Value::String(fragment.to_string()));
            }
        },
        _ => blocks.push(json!({ "type": "thinking", "thinking": fragment })),
    }
}

/// Records the signature a provider attaches to the block currently streaming.
pub fn set_thinking_signature(blocks: &mut [Value], signature: &str) {
    if let Some(Value::Object(map)) = blocks.last_mut() {
        map.insert(
            "signature".to_string(),
            Value::String(signature.to_string()),
        );
    }
}

#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
    #[serde(default)]
    pub usage: Option<StreamUsage>,
    /// Some OpenAI-compatible providers deliver errors mid-stream as
    /// `{"error": {...}}` with HTTP 200; surface them instead of ignoring.
    #[serde(default)]
    pub error: Option<StreamError>,
}

/// An in-band error payload from an OpenAI-compatible stream.
#[derive(Debug, Deserialize)]
pub struct StreamError {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default, rename = "type")]
    pub kind: Option<String>,
}

impl StreamError {
    pub fn describe(&self) -> String {
        match (&self.kind, &self.message) {
            (Some(kind), Some(message)) => format!("{kind}: {message}"),
            (None, Some(message)) => message.clone(),
            (Some(kind), None) => kind.clone(),
            (None, None) => "unknown provider error".to_string(),
        }
    }
}

/// Token counts as reported by OpenAI-compatible providers.
#[derive(Debug, Default, Deserialize)]
pub struct StreamUsage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub prompt_tokens_details: Option<PromptTokensDetails>,
    #[serde(default)]
    pub completion_tokens_details: Option<CompletionTokensDetails>,
}

#[derive(Debug, Default, Deserialize)]
pub struct CompletionTokensDetails {
    #[serde(default)]
    pub reasoning_tokens: u64,
}

#[derive(Debug, Default, Deserialize)]
pub struct PromptTokensDetails {
    #[serde(default)]
    pub cached_tokens: u64,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    #[serde(default)]
    pub delta: Delta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
    /// Reasoning text, which OpenAI-compatible providers name differently:
    /// DeepSeek, GLM/Z.AI and llama.cpp use `reasoning_content`, some gateways
    /// send `reasoning` or `reasoning_text` instead.
    #[serde(default)]
    pub reasoning_content: Option<String>,
    #[serde(default)]
    pub reasoning: Option<String>,
    #[serde(default)]
    pub reasoning_text: Option<String>,
}

impl Delta {
    /// The reasoning fragment this delta carries, if any. A few gateways send
    /// the same text under more than one name, so the first non-empty field
    /// wins.
    pub fn reasoning(&self) -> Option<&str> {
        [
            &self.reasoning_content,
            &self.reasoning,
            &self.reasoning_text,
        ]
        .into_iter()
        .flatten()
        .map(String::as_str)
        .find(|text| !text.is_empty())
    }
}

#[derive(Debug, Deserialize)]
pub struct DeltaToolCall {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<DeltaFunction>,
}

#[derive(Debug, Deserialize)]
pub struct DeltaFunction {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_message_serializes_as_string() {
        let message = Message::user("hello");
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["content"], serde_json::json!("hello"));

        let back: Message = serde_json::from_value(value).unwrap();
        assert_eq!(back.display().as_deref(), Some("hello"));
    }

    #[test]
    fn multimodal_message_serializes_as_parts() {
        let image = ContentPart::ImageUrl {
            image_url: ImageUrl {
                url: "data:image/png;base64,AAAA".into(),
                detail: None,
            },
        };
        let message = Message::user_parts("look", vec![image]);
        let value = serde_json::to_value(&message).unwrap();
        let parts = value["content"].as_array().unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0]["type"], "text");
        assert_eq!(parts[1]["type"], "image_url");
        assert_eq!(parts[1]["image_url"]["url"], "data:image/png;base64,AAAA");

        let back: Message = serde_json::from_value(value).unwrap();
        assert_eq!(back.display().as_deref(), Some("look\n[image]"));
    }

    #[test]
    fn tool_message_can_carry_media() {
        let pdf = ContentPart::File {
            file: FileData {
                filename: Some("spec.pdf".into()),
                file_data: "data:application/pdf;base64,AAAA".into(),
            },
        };
        let message = Message::tool_parts("call_1", "attached pdf", vec![pdf]);
        let value = serde_json::to_value(&message).unwrap();
        assert_eq!(value["content"][1]["type"], "file");

        let back: Message = serde_json::from_value(value).unwrap();
        assert_eq!(
            back.display().as_deref(),
            Some("attached pdf\n[file: spec.pdf]")
        );
    }

    #[test]
    fn delta_reads_reasoning_from_every_known_field() {
        let parse = |json: &str| serde_json::from_str::<Delta>(json).unwrap();
        assert_eq!(
            parse(r#"{"reasoning_content":"thinking"}"#).reasoning(),
            Some("thinking")
        );
        assert_eq!(
            parse(r#"{"reasoning":"thinking"}"#).reasoning(),
            Some("thinking")
        );
        assert_eq!(
            parse(r#"{"reasoning_text":"thinking"}"#).reasoning(),
            Some("thinking")
        );
        assert_eq!(parse(r#"{"content":"hello"}"#).reasoning(), None);
        assert_eq!(parse(r#"{"reasoning_content":""}"#).reasoning(), None);
        // Gateways such as chutes.ai send the same text twice; the first
        // non-empty field wins so the block is not duplicated.
        assert_eq!(
            parse(r#"{"reasoning_content":"a","reasoning":"a"}"#).reasoning(),
            Some("a")
        );
        assert_eq!(
            parse(r#"{"reasoning_content":"","reasoning":"b"}"#).reasoning(),
            Some("b")
        );
    }

    #[test]
    fn stream_chunk_carries_the_finish_reason() {
        let chunk: StreamChunk =
            serde_json::from_str(r#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#).unwrap();
        assert_eq!(chunk.choices[0].finish_reason.as_deref(), Some("length"));

        let chunk: StreamChunk =
            serde_json::from_str(r#"{"choices":[{"delta":{"content":"hi"}}]}"#).unwrap();
        assert_eq!(chunk.choices[0].finish_reason, None);
    }

    #[test]
    fn stream_chunk_reads_billed_reasoning_tokens() {
        let chunk: StreamChunk = serde_json::from_str(
            r#"{"usage":{"prompt_tokens":10,"completion_tokens":8192,"completion_tokens_details":{"reasoning_tokens":8192}}}"#,
        )
        .unwrap();
        let usage = chunk.usage.unwrap();
        assert_eq!(usage.completion_tokens, 8192);
        assert_eq!(
            usage.completion_tokens_details.map(|d| d.reasoning_tokens),
            Some(8192)
        );
    }

    #[test]
    fn stream_chunk_surfaces_in_band_errors() {
        let chunk: StreamChunk =
            serde_json::from_str(r#"{"error":{"type":"server_error","message":"overloaded"}}"#)
                .unwrap();
        assert!(chunk.choices.is_empty());
        assert_eq!(
            chunk.error.unwrap().describe(),
            "server_error: overloaded".to_string()
        );

        let chunk: StreamChunk = serde_json::from_str(r#"{"error":{}}"#).unwrap();
        assert_eq!(chunk.error.unwrap().describe(), "unknown provider error");
    }

    #[test]
    fn reasoning_fragments_merge_into_one_block_created_on_demand() {
        let mut blocks: Vec<Value> = Vec::new();
        push_thinking(&mut blocks, "let me ");
        push_thinking(&mut blocks, "check");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["type"], "thinking");
        assert_eq!(blocks[0]["thinking"], "let me check");
    }

    #[test]
    fn reasoning_fragments_extend_the_existing_block_in_place() {
        let mut blocks: Vec<Value> = vec![json!({
            "type": "thinking",
            "thinking": "",
            "signature": ""
        })];
        push_thinking(&mut blocks, "first ");
        push_thinking(&mut blocks, "second");
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0]["thinking"], "first second");
        assert_eq!(blocks[0]["signature"], "");
    }

    #[test]
    fn a_signature_is_recorded_on_the_current_block() {
        let mut blocks: Vec<Value> = vec![json!({ "type": "thinking", "thinking": "hmm" })];
        set_thinking_signature(&mut blocks, "sig");
        assert_eq!(blocks[0]["signature"], "sig");
        assert_eq!(blocks[0]["thinking"], "hmm");

        let mut empty: Vec<Value> = Vec::new();
        set_thinking_signature(&mut empty, "ignored");
        assert!(empty.is_empty());
    }
}
