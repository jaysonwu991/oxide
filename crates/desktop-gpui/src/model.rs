//! What the window holds: projects, threads, conversations and the cards a
//! turn leaves behind.
//!
//! The shapes come from the engine's own JSON — `list_projects`, `all_sessions`,
//! `session_messages`, the `agent-event` stream and `agent-end` — so the model
//! here is the page's model read in Rust.

use crate::markdown::{self, Block};
use serde_json::Value;

#[derive(Clone, Debug, Default)]
pub struct ProjectRow {
    pub id: String,
    pub path: String,
    pub name: String,
    pub display_path: String,
    pub registered: bool,
    pub exists: bool,
    pub session_count: usize,
    pub last_session_at: u64,
    pub pinned: bool,
}

impl ProjectRow {
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(Self {
            id: value["id"].as_str()?.to_string(),
            path: value["path"].as_str()?.to_string(),
            name: value["name"].as_str().unwrap_or_default().to_string(),
            display_path: value["displayPath"]
                .as_str()
                .or_else(|| value["display_path"].as_str())
                .unwrap_or_default()
                .to_string(),
            registered: value["registered"].as_bool().unwrap_or(false),
            exists: value["exists"].as_bool().unwrap_or(true),
            session_count: value["sessionCount"].as_u64().unwrap_or(0) as usize,
            last_session_at: value["lastSessionAt"].as_u64().unwrap_or(0),
            pinned: value["pinned"].as_bool().unwrap_or(false),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct SessionRow {
    pub id: String,
    pub project: String,
    pub title: String,
    pub preview: String,
    pub modified_at: u64,
    pub pinned: bool,
    pub worktree: Option<String>,
}

impl SessionRow {
    pub fn from_value(value: &Value) -> Option<Self> {
        let id = value["id"].as_str()?.to_string();
        let name = value["name"].as_str().unwrap_or_default();
        let preview = value["preview"].as_str().unwrap_or_default().to_string();
        let title = if name.is_empty() {
            let line = preview.lines().next().unwrap_or_default().trim();
            if line.is_empty() {
                "New chat".to_string()
            } else {
                line.chars().take(80).collect()
            }
        } else {
            name.to_string()
        };
        Some(Self {
            id,
            project: value["cwd"].as_str().unwrap_or_default().to_string(),
            title,
            preview,
            modified_at: value["modifiedAt"].as_u64().unwrap_or(0),
            pinned: value["pinned"].as_bool().unwrap_or(false),
            worktree: value["worktree"].as_str().map(str::to_string),
        })
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Pending,
    Running,
    Done,
    Failed,
}

#[derive(Clone, Debug, Default)]
pub struct ToolCard {
    pub id: u64,
    pub name: String,
    pub detail: String,
    pub status: Status,
    pub output: String,
    pub diff: Option<Value>,
}

#[derive(Clone, Debug)]
pub struct ApprovalCard {
    pub id: u64,
    pub tool: String,
    pub detail: String,
    pub decision: Option<String>,
}

#[derive(Clone, Debug)]
pub struct QuestionCard {
    pub id: u64,
    pub questions: Value,
    pub answered: bool,
    pub answers: Value,
}

#[derive(Clone, Debug, Default)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub cost: f64,
    pub context_tokens: u64,
    pub model_calls: u64,
}

impl Usage {
    pub fn from_value(value: &Value) -> Self {
        Self {
            input: value["input"].as_u64().unwrap_or(0),
            output: value["output"].as_u64().unwrap_or(0),
            cache_read: value["cacheRead"].as_u64().unwrap_or(0),
            cache_write: value["cacheWrite"].as_u64().unwrap_or(0),
            cost: value["cost"].as_f64().unwrap_or(0.0),
            context_tokens: value["contextTokens"].as_u64().unwrap_or(0),
            model_calls: value["modelCalls"].as_u64().unwrap_or(0),
        }
    }
}

#[derive(Clone, Debug)]
pub enum Entry {
    User {
        text: String,
        attachments: Vec<String>,
    },
    Assistant {
        text: String,
        blocks: Vec<Block>,
        streaming: bool,
    },
    Thinking {
        text: String,
        seconds: Option<u64>,
        done: bool,
    },
    Tool(ToolCard),
    Error(String),
    Info(String),
    Compaction {
        summary: String,
        summarized: u64,
        tokens: u64,
    },
    Approval(ApprovalCard),
    Question(QuestionCard),
}

impl Entry {
    pub fn assistant() -> Self {
        Entry::Assistant {
            text: String::new(),
            blocks: Vec::new(),
            streaming: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Attachment {
    pub name: String,
    pub data_url: String,
}

pub struct Chat {
    pub id: u64,
    pub project: String,
    pub session: Option<String>,
    pub title: String,
    pub entries: Vec<Entry>,
    pub run: Option<u64>,
    /// A turn was asked for and its id has not come back yet. The engine's
    /// `agent-start` and `agent-event` packets can arrive before `send_prompt`
    /// answers, so the chat claims the first run the stream names.
    pub awaiting_run: bool,
    pub reasoning: Option<String>,
    pub attachments: Vec<Attachment>,
    pub usage: Option<Usage>,
    pub changes: Option<Value>,
    pub baseline: Option<String>,
    pub after: Option<String>,
    pub loaded: bool,
    pub loading: bool,
    pub scroll_to_end: bool,
    pub context_tokens: Option<u64>,
    pub context_provider: String,
    pub context_model: String,
    pub scroll: gpui::ScrollHandle,
}

impl Chat {
    pub fn new(project: String, title: String) -> Self {
        Self {
            id: 0,
            project,
            session: None,
            title,
            entries: Vec::new(),
            run: None,
            awaiting_run: false,
            reasoning: None,
            attachments: Vec::new(),
            usage: None,
            changes: None,
            baseline: None,
            after: None,
            loaded: true,
            loading: false,
            scroll_to_end: true,
            context_tokens: None,
            context_provider: String::new(),
            context_model: String::new(),
            scroll: gpui::ScrollHandle::new(),
        }
    }

    pub fn running(&self) -> bool {
        self.run.is_some() || self.awaiting_run
    }

    /// Appends text to the answer at the end of the conversation, opening one
    /// when the last card is not an answer still streaming.
    fn append_text(&mut self, delta: &str) {
        if let Some(Entry::Assistant {
            text,
            blocks,
            streaming,
        }) = self.entries.last_mut()
        {
            if *streaming {
                text.push_str(delta);
                *blocks = markdown::parse(text);
                return;
            }
        }
        let mut entry = Entry::assistant();
        if let Entry::Assistant {
            text,
            blocks,
            streaming,
        } = &mut entry
        {
            text.push_str(delta);
            *blocks = markdown::parse(text);
            let _ = streaming;
        }
        self.entries.push(entry);
    }

    fn append_thinking(&mut self, delta: &str) {
        if let Some(Entry::Thinking { text, done, .. }) = self.entries.last_mut() {
            if !*done {
                text.push_str(delta);
                return;
            }
        }
        self.entries.push(Entry::Thinking {
            text: delta.to_string(),
            seconds: None,
            done: false,
        });
    }

    /// One `agent-event`.
    pub fn apply(&mut self, payload: &Value) {
        let kind = payload["type"].as_str().unwrap_or_default();
        match kind {
            "message_update" => {
                let (delta_kind, delta) = (
                    payload["assistantMessageEvent"]["type"]
                        .as_str()
                        .unwrap_or_default(),
                    payload["assistantMessageEvent"]["delta"]
                        .as_str()
                        .unwrap_or_default(),
                );
                match delta_kind {
                    "text_delta" => self.append_text(delta),
                    "thinking_delta" => self.append_thinking(delta),
                    _ => {}
                }
            }
            "thinking" => {
                if !matches!(
                    self.entries.last(),
                    Some(Entry::Thinking { done: false, .. })
                ) {
                    self.entries.push(Entry::Thinking {
                        text: String::new(),
                        seconds: None,
                        done: false,
                    });
                }
            }
            "thinking_done" => {
                if let Some(Entry::Thinking { seconds, done, .. }) = self.entries.last_mut() {
                    *seconds = payload["millis"].as_u64().map(|millis| millis / 1000);
                    *done = true;
                }
            }
            "tool_call" => {
                let name = payload["toolName"].as_str().unwrap_or_default().to_string();
                let detail = describe_call(&name, &payload["arguments"]);
                let id = self.entries.len() as u64;
                self.entries.push(Entry::Tool(ToolCard {
                    id,
                    name,
                    detail,
                    status: Status::Running,
                    output: String::new(),
                    diff: None,
                }));
            }
            "tool_execution_update" => {
                let chunk = payload["partialResult"].as_str().unwrap_or_default();
                if let Some(Entry::Tool(card)) = self.last_tool(payload["toolName"].as_str()) {
                    card.output.push_str(chunk);
                    card.status = Status::Running;
                }
            }
            "tool_execution_end" => {
                let result = payload["result"].as_str().unwrap_or_default().to_string();
                let failed = payload["isError"].as_bool().unwrap_or(false);
                if let Some(Entry::Tool(card)) = self.last_tool(payload["toolName"].as_str()) {
                    if !result.is_empty() {
                        card.output = result;
                    }
                    card.status = if failed { Status::Failed } else { Status::Done };
                    card.diff = payload.get("diff").cloned();
                }
            }
            "usage" => {
                self.usage = Some(Usage::from_value(&payload["usage"]));
            }
            "context_window" => {
                self.context_tokens = payload["tokens"].as_u64();
                self.context_provider =
                    payload["provider"].as_str().unwrap_or_default().to_string();
                self.context_model = payload["model"].as_str().unwrap_or_default().to_string();
            }
            "compaction" => {
                self.entries.push(Entry::Compaction {
                    summary: payload["summary"].as_str().unwrap_or_default().to_string(),
                    summarized: payload["summarized"].as_u64().unwrap_or(0),
                    tokens: payload["tokensBefore"].as_u64().unwrap_or(0),
                });
            }
            "error" => {
                self.entries.push(Entry::Error(
                    payload["message"].as_str().unwrap_or_default().to_string(),
                ));
            }
            "auto_retry_start" => {
                let attempt = payload["attempt"].as_u64().unwrap_or(0);
                let max = payload["maxAttempts"].as_u64().unwrap_or(0);
                self.entries.push(Entry::Info(format!(
                    "Retrying ({attempt}/{max}) after the provider answered with an error"
                )));
            }
            "question_closed" => {
                let id = payload["id"].as_u64().unwrap_or_default();
                for entry in self.entries.iter_mut() {
                    if let Entry::Question(question) = entry {
                        if question.id == id {
                            question.answered = true;
                        }
                    }
                }
            }
            "agent_end" => {
                if let Some(Entry::Assistant { streaming, .. }) = self.entries.last_mut() {
                    *streaming = false;
                }
            }
            _ => {}
        }
    }

    fn last_tool(&mut self, name: Option<&str>) -> Option<&mut Entry> {
        let name = name.map(str::to_string);
        let is_match = |entry: &Entry| match entry {
            Entry::Tool(card) => match name.as_deref() {
                Some(name) => card.name == name,
                None => true,
            },
            _ => false,
        };
        if self.entries.last().map(is_match).unwrap_or(false) {
            return self.entries.last_mut();
        }
        self.entries.iter_mut().rev().find(|entry| is_match(entry))
    }

    /// The stored transcript, `session_messages` answered.
    pub fn load(&mut self, value: &Value) {
        self.entries.clear();
        if let Some(messages) = value["messages"].as_array() {
            for message in messages {
                let role = message["role"].as_str().unwrap_or_default();
                let text = message["content"].as_str().unwrap_or_default().to_string();
                match role {
                    "user" => {
                        let attachments = message["attachments"]
                            .as_array()
                            .map(|items| {
                                items
                                    .iter()
                                    .filter_map(|item| item["name"].as_str().map(str::to_string))
                                    .collect()
                            })
                            .unwrap_or_default();
                        self.entries.push(Entry::User { text, attachments });
                    }
                    "assistant" => {
                        if !text.is_empty() {
                            self.entries.push(Entry::Assistant {
                                blocks: markdown::parse(&text),
                                text,
                                streaming: false,
                            });
                        }
                        if let Some(calls) = message["toolCalls"].as_array() {
                            for call in calls {
                                let name = call["name"].as_str().unwrap_or_default().to_string();
                                let args = call["arguments"]
                                    .as_str()
                                    .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
                                    .unwrap_or(Value::Null);
                                self.entries.push(Entry::Tool(ToolCard {
                                    id: self.entries.len() as u64,
                                    detail: describe_call(&name, &args),
                                    name,
                                    status: Status::Done,
                                    output: String::new(),
                                    diff: None,
                                }));
                            }
                        }
                    }
                    "tool" => {
                        let output = text.clone();
                        let id = self.entries.len() as u64;
                        self.entries.push(Entry::Tool(ToolCard {
                            id,
                            name: "tool".to_string(),
                            detail: String::new(),
                            status: Status::Done,
                            output,
                            diff: None,
                        }));
                    }
                    _ => {}
                }
            }
        }
        let usage = Usage::from_value(&value["usage"]);
        if usage.input > 0 || usage.output > 0 {
            self.usage = Some(usage);
        }
        self.loaded = true;
        self.loading = false;
        self.scroll_to_end = true;
    }
}

/// A tool card's one-line summary: the command, path or query a call names.
pub fn describe_call(_name: &str, args: &Value) -> String {
    let pick = [
        "command", "path", "file", "query", "pattern", "url", "prompt",
    ];
    for key in pick {
        if let Some(value) = args[key].as_str() {
            let line = value.lines().next().unwrap_or_default();
            if !line.is_empty() {
                return line.chars().take(160).collect();
            }
        }
    }
    if let Some(value) = args["edits"].as_array() {
        if let Some(path) = value.first().and_then(|edit| edit["filePath"].as_str()) {
            return path.to_string();
        }
    }
    if let Some(array) = args.as_array() {
        if let Some(first) = array.first().and_then(Value::as_str) {
            return first.to_string();
        }
    }
    String::new()
}
