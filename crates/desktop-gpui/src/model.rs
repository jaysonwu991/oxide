//! What the window holds: projects, threads, conversations and the cards a
//! turn leaves behind.
//!
//! The shapes come from the engine's own JSON — `list_projects`, `all_sessions`,
//! `session_messages`, the `agent-event` stream and `agent-end` — so the model
//! here is the page's model read in Rust.

use crate::markdown::{self, Block};
use gpui::{Image, ImageFormat};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A number the engine wrote, under the name the Rust struct serialises it with.
/// The snapshots the page's own bridge answers with used camelCase for a few of
/// these, so the older spelling stays as a fallback rather than being dropped.
fn number(value: &Value, snake: &str, camel: &str) -> u64 {
    value
        .get(snake)
        .or_else(|| value.get(camel))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

/// The projects `list_projects` answers with: a bare array, not an object.
pub fn projects(value: &Value) -> Vec<ProjectRow> {
    value
        .as_array()
        .or_else(|| value["projects"].as_array())
        .map(|projects| projects.iter().filter_map(ProjectRow::from_value).collect())
        .unwrap_or_default()
}

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
            session_count: number(value, "session_count", "sessionCount") as usize,
            last_session_at: number(value, "last_session_at", "lastSessionAt"),
            pinned: value["pinned"].as_bool().unwrap_or(false),
        })
    }
}

#[derive(Clone, Debug, Default)]
pub struct SessionRow {
    pub id: String,
    pub project: String,
    pub title: String,
    pub named: bool,
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
            named: !name.is_empty(),
            preview,
            modified_at: number(value, "modified_at", "modifiedAt"),
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
    pub app: Option<String>,
    pub status: Status,
    pub output: String,
    pub diff: Option<Value>,
    pub media: Vec<Attachment>,
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

/// One file a turn changed, as the transcript's card lists it.
#[derive(Clone, Debug, Default)]
pub struct EditedFile {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub status: String,
    pub binary: bool,
    pub diff: String,
}

/// What a turn changed, kept in the transcript next to the answer that changed
/// it: the page's own card, with the totals and the undo the baseline allows.
#[derive(Clone, Debug)]
pub struct EditedCard {
    pub project: String,
    pub baseline: Option<String>,
    pub after: Option<String>,
    pub files: Vec<EditedFile>,
    pub added: usize,
    pub removed: usize,
    /// Only the newest turn can be put back: a later one may sit on top of it.
    pub latest: bool,
    pub undone: bool,
}

impl EditedCard {
    pub fn from_value(changes: &Value) -> Option<Self> {
        let files = changes["files"].as_array()?;
        if files.is_empty() {
            return None;
        }
        Some(Self {
            project: String::new(),
            baseline: None,
            after: None,
            files: files
                .iter()
                .map(|file| EditedFile {
                    path: file["path"].as_str().unwrap_or_default().to_string(),
                    added: file["added"].as_u64().unwrap_or(0) as usize,
                    removed: file["removed"].as_u64().unwrap_or(0) as usize,
                    status: file["status"].as_str().unwrap_or_default().to_string(),
                    binary: file["binary"].as_bool().unwrap_or(false),
                    diff: file["diff"].as_str().unwrap_or_default().to_string(),
                })
                .collect(),
            added: changes["added"].as_u64().unwrap_or(0) as usize,
            removed: changes["removed"].as_u64().unwrap_or(0) as usize,
            latest: true,
            undone: false,
        })
    }
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
        attachments: Vec<Attachment>,
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
    Edited(EditedCard),
    Error(String),
    Info(String),
    Compaction {
        summary: String,
        summarized: u64,
        tokens: u64,
    },
    Approval(ApprovalCard),
    Question(QuestionCard),
    RunTime {
        elapsed: Duration,
    },
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
    pub image: Option<Arc<Image>>,
}

impl Attachment {
    pub fn new(name: String, data_url: String) -> Self {
        let image = image_from_data_url(&data_url);
        Self {
            name,
            data_url,
            image,
        }
    }
}

#[derive(Clone, Debug)]
pub struct ComputerUseActivity {
    pub app: Option<String>,
    pub action: String,
    pub running: bool,
    pub stopped: bool,
    pub failed: bool,
    pub image: Option<Attachment>,
    pub image_app: Option<String>,
    pub permissions: Option<ComputerUsePermissions>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ComputerUsePermissions {
    pub accessibility: bool,
    pub screen_recording: bool,
}

impl ComputerUsePermissions {
    pub fn available(&self) -> bool {
        self.accessibility && self.screen_recording
    }
}

impl ComputerUseActivity {
    pub fn needs_permissions_attention(&self) -> bool {
        self.permissions
            .as_ref()
            .is_some_and(|permissions| !permissions.available())
            && (self.action == "permissions" || self.failed)
    }

    pub fn permissions_ready(&self) -> bool {
        self.action == "permissions"
            && self
                .permissions
                .as_ref()
                .is_some_and(ComputerUsePermissions::available)
    }

    pub fn permission_caption(&self) -> Option<&'static str> {
        let permissions = self.permissions.as_ref()?;
        Some(match (permissions.accessibility, permissions.screen_recording) {
            (true, true) => "App control and screen capture permissions are available.",
            (false, true) => {
                "Enable Oxide under System Settings → Privacy & Security → Device Control & Data Access."
            }
            (true, false) => {
                "Enable Oxide under System Settings → Privacy & Security → Screen & System Audio Recording."
            }
            (false, false) => {
                "Enable Oxide under both Device Control & Data Access and Screen & System Audio Recording in System Settings → Privacy & Security."
            }
        })
    }

    pub fn status_label(&self) -> &'static str {
        if self.stopped {
            "Stopped"
        } else if self.needs_permissions_attention() {
            "Permissions needed"
        } else if self.failed {
            "Action failed"
        } else if self.running {
            "Using computer"
        } else if self.permissions_ready() {
            "Ready"
        } else {
            "Finished"
        }
    }

    pub fn caption(&self) -> &'static str {
        if self.action == "permissions" || self.needs_permissions_attention() {
            return self.permission_caption().unwrap_or(
                "No screenshot captured yet. Accessibility actions do not always return an image.",
            );
        }
        if self.image.is_some() {
            "Last captured screenshot"
        } else {
            "No screenshot captured yet. Accessibility actions do not always return an image."
        }
    }
}

/// Whether a tool belongs to an Oxide or Codex Computer Use server. MCP tool
/// names are server-qualified, and a project may use either the standalone
/// `computer-use` registration or Oxide's `oxide-computer-use` one.
pub fn is_computer_tool(name: &str) -> bool {
    let Some((server, tool)) = name.rsplit_once("__") else {
        return false;
    };
    !tool.is_empty()
        && (server == "computer-use"
            || server == "computer_use"
            || server.ends_with("-computer-use")
            || server.ends_with("_computer_use"))
}

fn computer_action(name: &str) -> String {
    name.rsplit_once("__")
        .map(|(_, action)| action.replace('_', " "))
        .unwrap_or_else(|| name.replace('_', " "))
}

fn argument(value: &Value) -> Value {
    if value.is_object() {
        value.clone()
    } else {
        value
            .as_str()
            .and_then(|raw| serde_json::from_str(raw).ok())
            .unwrap_or(Value::Null)
    }
}

fn tool_attachments(value: &Value) -> Vec<Attachment> {
    value["attachments"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let data_url = item["dataUrl"]
                        .as_str()
                        .or_else(|| item["data_url"].as_str())?;
                    if !data_url.starts_with("data:image/") {
                        return None;
                    }
                    Some(Attachment::new(
                        item["name"].as_str().unwrap_or("Tool image").to_string(),
                        data_url.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_permissions(text: &str) -> Option<ComputerUsePermissions> {
    let status = serde_json::from_str::<Value>(text.trim()).ok()?;
    Some(ComputerUsePermissions {
        accessibility: status["accessibility"].as_bool()?,
        screen_recording: status["screen_recording"].as_bool()?,
    })
}

#[derive(Clone, Debug)]
pub struct QueuedMessage {
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub follow_up: bool,
}

pub struct Chat {
    pub id: u64,
    pub project: String,
    pub session: Option<String>,
    pub title: String,
    pub provisional_title: bool,
    pub entries: Vec<Entry>,
    pub run: Option<u64>,
    pub stop_requested: bool,
    /// A turn was asked for and its id has not come back yet. The engine's
    /// `agent-start` and `agent-event` packets can arrive before `send_prompt`
    /// answers, so the chat claims the first run the stream names.
    pub awaiting_run: bool,
    pub reasoning: Option<String>,
    pub agent: Option<String>,
    pub attachments: Vec<Attachment>,
    pub queued: Vec<QueuedMessage>,
    pub usage: Option<Usage>,
    pub changes: Option<Value>,
    pub baseline: Option<String>,
    pub after: Option<String>,
    pub loaded: bool,
    pub loading: bool,
    pub scroll_to_end: bool,
    pub follow_latest: bool,
    pub context_tokens: Option<u64>,
    pub context_provider: String,
    pub context_model: String,
    pub run_started_at: Option<Instant>,
    pub scroll: gpui::ScrollHandle,
}

impl Chat {
    pub fn new(project: String, title: String) -> Self {
        Self {
            id: 0,
            project,
            session: None,
            title,
            provisional_title: true,
            entries: Vec::new(),
            run: None,
            stop_requested: false,
            awaiting_run: false,
            reasoning: None,
            agent: None,
            attachments: Vec::new(),
            queued: Vec::new(),
            usage: None,
            changes: None,
            baseline: None,
            after: None,
            loaded: true,
            loading: false,
            scroll_to_end: true,
            follow_latest: true,
            context_tokens: None,
            context_provider: String::new(),
            context_model: String::new(),
            run_started_at: None,
            scroll: gpui::ScrollHandle::new(),
        }
    }

    pub fn running(&self) -> bool {
        self.run.is_some() || self.awaiting_run
    }

    /// The latest Computer Use call and the latest screenshot any Computer Use
    /// call in this conversation returned. An accessibility-only result keeps
    /// the earlier picture, matching Codex's persistent preview.
    pub fn computer_use(&self) -> Option<ComputerUseActivity> {
        let mut latest = None;
        let mut image = None;
        let mut permissions = None;
        let mut permissions_seen = false;
        for entry in self.entries.iter().rev() {
            let Entry::Tool(card) = entry else {
                continue;
            };
            if !is_computer_tool(&card.name) {
                continue;
            }
            if !permissions_seen && card.name.ends_with("__permissions") {
                permissions_seen = true;
                permissions = parse_permissions(&card.output);
            }
            if image.is_none() {
                if let Some(attachment) = card.media.last() {
                    image = Some((attachment.clone(), card.app.clone()));
                }
            }
            if latest.is_none() {
                latest = Some((
                    card.app.clone(),
                    computer_action(&card.name),
                    card.status == Status::Running,
                    card.status == Status::Failed,
                ));
            }
            if image.is_some() && latest.is_some() && permissions_seen {
                break;
            }
        }
        let (app, action, running, failed) = latest?;
        let (image, image_app) = image.map_or((None, None), |(image, app)| (Some(image), app));
        Some(ComputerUseActivity {
            app,
            action,
            running: running && self.running(),
            stopped: self.stop_requested && !self.running(),
            failed,
            image,
            image_app,
            permissions,
        })
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
    /// The card a turn's change listing becomes: the page appends one to the
    /// transcript when a run ends, and only its newest one can be undone.
    pub fn record_turn(&mut self, payload: &Value) {
        let Some(card) = EditedCard::from_value(&payload["changes"]) else {
            return;
        };
        let mut card = card;
        card.project = self.project.clone();
        card.baseline = payload["baseline"].as_str().map(str::to_string);
        card.after = payload["after"].as_str().map(str::to_string);
        for entry in self.entries.iter_mut() {
            if let Entry::Edited(existing) = entry {
                existing.latest = false;
            }
        }
        self.entries.push(Entry::Edited(card));
        self.scroll_to_end = true;
    }

    pub fn finish_run(&mut self) {
        if let Some(started) = self.run_started_at.take() {
            self.entries.push(Entry::RunTime {
                elapsed: started.elapsed(),
            });
        }
    }

    pub fn mark_undone(&mut self) {
        for entry in self.entries.iter_mut().rev() {
            if let Entry::Edited(card) = entry {
                card.undone = true;
                card.latest = false;
                return;
            }
        }
    }

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
                let args = argument(&payload["arguments"]);
                let detail = describe_call(&name, &args);
                let id = self.entries.len() as u64;
                self.entries.push(Entry::Tool(ToolCard {
                    id,
                    name,
                    detail,
                    app: args["app"].as_str().map(str::to_string),
                    status: Status::Running,
                    output: String::new(),
                    diff: None,
                    media: Vec::new(),
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
                    card.media = tool_attachments(payload);
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
            let results = messages
                .iter()
                .filter(|message| message["role"].as_str() == Some("tool"))
                .filter_map(|message| {
                    Some((message["toolCallId"].as_str()?.to_string(), message.clone()))
                })
                .collect::<HashMap<_, _>>();
            let mut restored = HashSet::new();
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
                                    .filter_map(|item| {
                                        Some(Attachment::new(
                                            item["name"].as_str()?.to_string(),
                                            item["dataUrl"].as_str()?.to_string(),
                                        ))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        self.entries.push(Entry::User { text, attachments });
                    }
                    "assistant" => {
                        let thinking = message["thinking"].as_str().unwrap_or_default();
                        if !thinking.trim().is_empty() {
                            self.entries.push(Entry::Thinking {
                                text: thinking.to_string(),
                                seconds: None,
                                done: true,
                            });
                        }
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
                                let args = argument(&call["arguments"]);
                                let result = call["id"]
                                    .as_str()
                                    .and_then(|id| results.get(id))
                                    .cloned()
                                    .unwrap_or(Value::Null);
                                if let Some(id) = call["id"].as_str() {
                                    if results.contains_key(id) {
                                        restored.insert(id.to_string());
                                    }
                                }
                                let output =
                                    result["content"].as_str().unwrap_or_default().to_string();
                                let failed = output.starts_with("error:");
                                self.entries.push(Entry::Tool(ToolCard {
                                    id: self.entries.len() as u64,
                                    detail: describe_call(&name, &args),
                                    name,
                                    app: args["app"].as_str().map(str::to_string),
                                    status: if failed { Status::Failed } else { Status::Done },
                                    output: output
                                        .strip_prefix("error:")
                                        .unwrap_or(&output)
                                        .trim()
                                        .to_string(),
                                    diff: None,
                                    media: tool_attachments(&result),
                                }));
                            }
                        }
                    }
                    "tool" => {
                        if message["toolCallId"]
                            .as_str()
                            .is_some_and(|id| restored.contains(id))
                        {
                            continue;
                        }
                        let output = text.clone();
                        let id = self.entries.len() as u64;
                        self.entries.push(Entry::Tool(ToolCard {
                            id,
                            name: "tool".to_string(),
                            detail: String::new(),
                            app: None,
                            status: Status::Done,
                            output,
                            diff: None,
                            media: tool_attachments(message),
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

pub(crate) fn image_from_data_url(data_url: &str) -> Option<Arc<Image>> {
    let (meta, payload) = data_url.strip_prefix("data:")?.split_once(',')?;
    let mime = meta.split(';').next()?;
    if !meta.split(';').any(|part| part == "base64") {
        return None;
    }
    let format = ImageFormat::from_mime_type(mime)?;
    let bytes = oxide_core::media::base64_decode(payload)?;
    Some(Arc::new(Image::from_bytes(format, bytes)))
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const PNG: &str = "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Zb4kAAAAASUVORK5CYII=";

    #[test]
    fn computer_tool_names_are_server_qualified() {
        assert!(is_computer_tool("computer-use__click"));
        assert!(is_computer_tool("oxide-computer-use__get_app_state"));
        assert!(is_computer_tool("computer_use__screenshot"));
        assert!(!is_computer_tool("other__computer-use"));
        assert!(!is_computer_tool("computer-use"));
    }

    #[test]
    fn computer_use_keeps_the_last_screenshot_through_ax_only_results() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.run = Some(7);
        chat.apply(&json!({
            "type": "tool_call",
            "toolName": "oxide-computer-use__click",
            "arguments": { "app": "Edge" },
        }));
        let running = chat.computer_use().unwrap();
        assert!(running.running);
        assert_eq!(running.app.as_deref(), Some("Edge"));
        assert_eq!(running.action, "click");

        chat.apply(&json!({
            "type": "tool_execution_end",
            "toolName": "oxide-computer-use__click",
            "result": "Clicked",
            "isError": false,
            "attachments": [{ "name": "Edge", "dataUrl": PNG }],
        }));
        chat.apply(&json!({
            "type": "tool_call",
            "toolName": "oxide-computer-use__get_app_state",
            "arguments": { "app": "Finder" },
        }));
        chat.apply(&json!({
            "type": "tool_execution_end",
            "toolName": "oxide-computer-use__get_app_state",
            "result": "Accessibility tree",
            "isError": false,
            "attachments": [],
        }));

        let activity = chat.computer_use().unwrap();
        assert!(!activity.running);
        assert_eq!(activity.app.as_deref(), Some("Finder"));
        assert_eq!(activity.action, "get app state");
        assert_eq!(activity.image_app.as_deref(), Some("Edge"));
        assert_eq!(activity.image.unwrap().data_url, PNG);
    }

    #[test]
    fn permission_diagnostics_become_an_actionable_computer_use_state() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.apply(&json!({
            "type": "tool_call",
            "toolName": "oxide-computer-use__permissions",
            "arguments": {},
        }));
        chat.apply(&json!({
            "type": "tool_execution_end",
            "toolName": "oxide-computer-use__permissions",
            "result": r#"{
                "accessibility": false,
                "screen_recording": true,
                "launching_app": "/Applications/Oxide.app",
                "guidance": "Grant access in System Settings."
            }"#,
            "isError": false,
        }));

        let activity = chat.computer_use().unwrap();
        assert_eq!(
            activity.permissions,
            Some(ComputerUsePermissions {
                accessibility: false,
                screen_recording: true,
            })
        );
        assert!(activity.needs_permissions_attention());
        assert_eq!(activity.status_label(), "Permissions needed");
        assert_eq!(
            activity.caption(),
            "Enable Oxide under System Settings → Privacy & Security → Device Control & Data Access."
        );

        chat.apply(&json!({
            "type": "tool_call",
            "toolName": "oxide-computer-use__permissions",
            "arguments": {},
        }));
        chat.apply(&json!({
            "type": "tool_execution_end",
            "toolName": "oxide-computer-use__permissions",
            "result": r#"{"accessibility": true, "screen_recording": true}"#,
            "isError": false,
        }));

        let activity = chat.computer_use().unwrap();
        assert!(!activity.needs_permissions_attention());
        assert!(activity.permissions_ready());
        assert_eq!(activity.status_label(), "Ready");
        assert_eq!(
            activity.caption(),
            "App control and screen capture permissions are available."
        );
    }

    #[test]
    fn stored_tool_results_keep_their_call_and_image() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.load(&json!({
            "messages": [
                {
                    "role": "assistant",
                    "content": "",
                    "toolCalls": [{
                        "id": "shot",
                        "name": "computer-use__screenshot",
                        "arguments": "{\"app\":\"Edge\"}"
                    }]
                },
                {
                    "role": "tool",
                    "toolCallId": "shot",
                    "content": "Captured screenshot",
                    "attachments": [{ "name": "Edge", "dataUrl": PNG }]
                }
            ]
        }));
        assert_eq!(chat.entries.len(), 1);
        let activity = chat.computer_use().unwrap();
        assert_eq!(activity.action, "screenshot");
        assert_eq!(activity.image.unwrap().data_url, PNG);
    }

    #[test]
    fn a_turn_change_listing_becomes_a_card() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.record_turn(&json!({
            "baseline": "before-turn",
            "after": "after-turn",
            "changes": {
                "files": [
                    { "path": "src/main.rs", "status": "modified", "added": 12, "removed": 3, "binary": false, "diff": "+changed" },
                    { "path": "asset.bin", "status": "added", "added": 0, "removed": 0, "binary": true, "diff": "" }
                ],
                "added": 52,
                "removed": 3
            }
        }));
        match &chat.entries[0] {
            Entry::Edited(card) => {
                assert_eq!(card.files.len(), 2);
                assert_eq!(card.added, 52);
                assert_eq!(card.removed, 3);
                assert_eq!(card.project, "/tmp");
                assert_eq!(card.baseline.as_deref(), Some("before-turn"));
                assert_eq!(card.after.as_deref(), Some("after-turn"));
                assert_eq!(card.files[0].diff, "+changed");
                assert!(card.files[1].binary);
                assert!(card.latest);
            }
            other => panic!("expected an edited card, got {other:?}"),
        }

        chat.record_turn(&json!({ "changes": {
            "files": [{ "path": "src/lib.rs", "status": "modified", "added": 1, "removed": 1 }],
            "added": 1,
            "removed": 1
        }}));
        let latest: Vec<bool> = chat
            .entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Edited(card) => Some(card.latest),
                _ => None,
            })
            .collect();
        assert_eq!(latest, vec![false, true]);

        chat.record_turn(&json!({ "changes": { "files": [], "added": 0, "removed": 0 } }));
        assert_eq!(chat.entries.len(), 2);
    }

    #[test]
    fn a_finished_run_clock_stays_above_its_change_card() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.run_started_at = Some(Instant::now());
        chat.finish_run();
        chat.record_turn(&json!({
            "changes": {
                "files": [{ "path": "src/main.rs", "added": 2, "removed": 1 }],
                "added": 2,
                "removed": 1
            }
        }));
        assert!(matches!(chat.entries[0], Entry::RunTime { .. }));
        assert!(matches!(chat.entries[1], Entry::Edited(_)));
    }

    #[test]
    fn a_project_row_reads_the_engines_own_names() {
        let projects = projects(&json!([{
            "display_path": "~/Projects/oxide",
            "exists": true,
            "id": "/Users/jayson/Projects/oxide",
            "last_opened_at": 1794656871u64,
            "last_session_at": 1794657536u64,
            "name": "oxide",
            "path": "/Users/jayson/Projects/oxide",
            "pinned": false,
            "registered": true,
            "session_count": 4
        }]));
        assert_eq!(projects.len(), 1);
        let project = &projects[0];
        assert_eq!(project.name, "oxide");
        assert_eq!(project.display_path, "~/Projects/oxide");
        assert_eq!(project.session_count, 4);
        assert_eq!(project.last_session_at, 1_794_657_536);
    }

    #[test]
    fn a_session_row_reads_the_engines_own_names() {
        let session = SessionRow::from_value(&json!({
            "created_at": 1794656133u64,
            "cwd": "/Users/jayson/Projects/oxide",
            "id": "ce4b20dcbe82236f",
            "message_count": 469,
            "modified_at": 1794657550u64,
            "name": "Align GPUI window with the Electron app",
            "path": "sessions/1a99b86e8b6de1c6/1794656133745_ce4b20dcbe82236f.jsonl",
            "pinned": false,
            "preview": "The gap is so large between GPUI desktop and Electron desktop"
        }))
        .expect("a session row");
        assert_eq!(
            session.title,
            "Align GPUI window with the Electron app"
        );
        assert!(session.named);
        let unnamed = SessionRow::from_value(&json!({
            "cwd": "/tmp",
            "id": "unnamed",
            "name": "",
            "preview": "Prompt"
        }))
        .expect("an unnamed session row");
        assert!(!unnamed.named);
        assert_eq!(session.project, "/Users/jayson/Projects/oxide");
        assert_eq!(session.modified_at, 1_794_657_550);
    }

    #[test]
    fn a_project_list_reads_the_bare_array_and_the_wrapped_one() {
        assert!(projects(&json!([])).is_empty());
        assert!(projects(&json!({ "projects": [], "added": "id" })).is_empty());
        assert!(projects(&json!(null)).is_empty());
    }

    #[test]
    fn a_stored_image_attachment_keeps_its_preview_data() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.load(&json!({
            "messages": [{
                "role": "user",
                "content": "",
                "attachments": [{
                    "name": "shot.png",
                    "dataUrl": "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAusB9Y9Zb4kAAAAASUVORK5CYII="
                }]
            }]
        }));
        match &chat.entries[0] {
            Entry::User {
                text, attachments, ..
            } => {
                assert!(text.is_empty());
                assert_eq!(attachments[0].name, "shot.png");
                assert!(attachments[0].image.is_some());
            }
            other => panic!("expected a user message, got {other:?}"),
        }
    }

    #[test]
    fn stored_thinking_is_replayed_before_the_assistant_text() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.load(&json!({
            "messages": [{
                "role": "assistant",
                "content": "Done.",
                "thinking": "Inspect the files first.",
                "toolCalls": []
            }]
        }));
        assert!(matches!(
            &chat.entries[..],
            [
                Entry::Thinking {
                    text,
                    seconds: None,
                    done: true
                },
                Entry::Assistant { text: answer, .. }
            ] if text == "Inspect the files first." && answer == "Done."
        ));
    }
}
