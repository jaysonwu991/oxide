use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    /// they can be replayed with tool results. Never sent to OpenAI.
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
    pub messages: Vec<Message>,
    pub stream: bool,
    pub max_tokens: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Vec<ToolSpec>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
}

#[derive(Debug, Default)]
pub struct AssistantTurn {
    pub content: String,
    pub tool_calls: Vec<ToolCall>,
    pub thinking: Vec<Value>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChunk {
    #[serde(default)]
    pub choices: Vec<StreamChoice>,
}

#[derive(Debug, Deserialize)]
pub struct StreamChoice {
    #[serde(default)]
    pub delta: Delta,
}

#[derive(Debug, Default, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Option<Vec<DeltaToolCall>>,
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
}
