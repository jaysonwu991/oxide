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
    /// The invocation a reader can copy. Unlike `detail`, this keeps a full
    /// multi-line shell command instead of the one-line transcript summary.
    pub copy_text: String,
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
        phase: AssistantPhase,
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
    pub fn assistant(phase: AssistantPhase) -> Self {
        Entry::Assistant {
            text: String::new(),
            blocks: Vec::new(),
            streaming: true,
            phase,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AssistantPhase {
    #[default]
    Commentary,
    FinalAnswer,
}

impl AssistantPhase {
    pub fn from_wire(value: &str) -> Self {
        if value == "final_answer" {
            Self::FinalAnswer
        } else {
            Self::Commentary
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

#[derive(Clone, Debug)]
pub struct QueuedMessage {
    pub id: u64,
    pub text: String,
    pub attachments: Vec<Attachment>,
    pub follow_up: bool,
    pub delivery: QueueDelivery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueDelivery {
    /// Waiting for the engine to answer `steer_run`.
    Waiting,
    /// The running turn holds the message and will read it.
    Accepted,
    /// The running turn had already closed its queue; start this after it.
    Rejected,
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
    /// The engine replies to `steer_run` independently of the event stream, so
    /// the turn can end before the answer arrives. Holding the queue until every
    /// answer is known prevents a rejected message from briefly becoming a
    /// transcript entry and then starting again.
    pub steer_requests: usize,
    next_queued_id: u64,
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
            steer_requests: 0,
            next_queued_id: 1,
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

    /// Adds a message to the visible steering queue and returns the local id
    /// its result will settle by.
    pub fn queue_steer(
        &mut self,
        text: String,
        attachments: Vec<Attachment>,
        follow_up: bool,
    ) -> u64 {
        let id = self.next_queued_id;
        self.next_queued_id = self.next_queued_id.wrapping_add(1);
        self.steer_requests += 1;
        self.queued.push(QueuedMessage {
            id,
            text,
            attachments,
            follow_up,
            delivery: QueueDelivery::Waiting,
        });
        id
    }

    /// Records what the engine said about one queued message. A missing id means
    /// Stop already removed the row; the request still has to leave the in-flight
    /// count so a later result can settle the remaining queue.
    pub fn finish_steer(&mut self, id: u64, accepted: bool) {
        self.steer_requests = self.steer_requests.saturating_sub(1);
        if let Some(message) = self.queued.iter_mut().find(|message| message.id == id) {
            message.delivery = if accepted {
                QueueDelivery::Accepted
            } else {
                QueueDelivery::Rejected
            };
        }
    }

    /// Paints messages the running turn really consumed and returns the first
    /// message whose run had already finished. Waiting for every reply is what
    /// keeps a turn-end packet from committing a message `steer_run` later
    /// rejects.
    pub fn settle_steers(&mut self) -> Option<QueuedMessage> {
        if self.running() || self.steer_requests != 0 {
            return None;
        }
        let mut accepted = Vec::new();
        self.queued.retain(|message| {
            if message.delivery == QueueDelivery::Accepted {
                accepted.push(message.clone());
                false
            } else {
                true
            }
        });
        for message in accepted {
            self.entries.push(Entry::User {
                text: message.text,
                attachments: message.attachments,
            });
        }
        if self
            .queued
            .first()
            .is_some_and(|message| message.delivery == QueueDelivery::Rejected)
        {
            Some(self.queued.remove(0))
        } else {
            None
        }
    }

    /// Appends text to the answer at the end of the conversation, opening one
    /// when the last card is not an answer still streaming.
    fn append_text(&mut self, delta: &str) {
        if let Some(Entry::Assistant {
            text,
            blocks,
            streaming,
            ..
        }) = self.entries.last_mut()
        {
            if *streaming {
                text.push_str(delta);
                *blocks = markdown::parse(text);
                return;
            }
        }
        let mut entry = Entry::assistant(AssistantPhase::Commentary);
        if let Entry::Assistant {
            text,
            blocks,
            streaming,
            ..
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
            // A step boundary is not transcript content. Provider reasoning is
            // private, so a row is opened only if a compatibility stream sends
            // an actual `thinking_delta`.
            "thinking" => {}
            "thinking_done" => {
                if let Some(Entry::Thinking { seconds, done, .. }) = self.entries.last_mut() {
                    *seconds = payload["millis"].as_u64().map(|millis| millis / 1000);
                    *done = true;
                }
            }
            "message_phase" => {
                let phase =
                    AssistantPhase::from_wire(payload["phase"].as_str().unwrap_or_default());
                for entry in self.entries.iter_mut().rev() {
                    if let Entry::Assistant { phase: current, .. } = entry {
                        *current = phase;
                        break;
                    }
                }
            }
            "tool_call" => {
                if let Some(Entry::Assistant { streaming, .. }) = self.entries.last_mut() {
                    *streaming = false;
                }
                let name = payload["toolName"].as_str().unwrap_or_default().to_string();
                let args = argument(&payload["arguments"]);
                let detail = describe_call(&name, &args);
                let copy_text = tool_copy_text(&name, &args, &detail);
                let id = self.entries.len() as u64;
                self.entries.push(Entry::Tool(ToolCard {
                    id,
                    name,
                    detail,
                    copy_text,
                    status: Status::Running,
                    output: String::new(),
                    diff: None,
                    media: Vec::new(),
                }));
            }
            "tool_execution_update" => {
                let chunk = payload["partialResult"].as_str().unwrap_or_default();
                if let Some(Entry::Tool(card)) = self.open_tool(payload["toolName"].as_str()) {
                    card.output.push_str(chunk);
                    card.status = Status::Running;
                }
            }
            "tool_execution_end" => {
                let result = payload["result"].as_str().unwrap_or_default().to_string();
                let failed = payload["isError"].as_bool().unwrap_or(false);
                if let Some(Entry::Tool(card)) = self.open_tool(payload["toolName"].as_str()) {
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
                for entry in &mut self.entries {
                    match entry {
                        Entry::Assistant { streaming, .. } => *streaming = false,
                        Entry::Tool(card) if card.status == Status::Running => {
                            card.status = Status::Done;
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    /// The card a progress chunk or a result belongs to. The engine runs some
    /// tools concurrently, and its events name the tool but not the call, so
    /// the oldest card of that name still in flight is the one this event
    /// closes; settling the newest would leave the earlier calls running for
    /// good and keep their activity row saying so. A late event for a call
    /// that has already closed falls back to the newest card of that name.
    fn open_tool(&mut self, name: Option<&str>) -> Option<&mut Entry> {
        let name = name.map(str::to_string);
        let named = |entry: &Entry| match entry {
            Entry::Tool(card) => match name.as_deref() {
                Some(name) => card.name == name,
                None => true,
            },
            _ => false,
        };
        let open = |entry: &Entry| {
            matches!(entry, Entry::Tool(card)
                if card.status == Status::Running || card.status == Status::Pending)
        };
        if let Some(index) = self
            .entries
            .iter()
            .position(|entry| open(entry) && named(entry))
        {
            return self.entries.get_mut(index);
        }
        let index = self.entries.iter().rposition(named)?;
        self.entries.get_mut(index)
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
                            let phase = if message["toolCalls"]
                                .as_array()
                                .is_some_and(|calls| !calls.is_empty())
                            {
                                AssistantPhase::Commentary
                            } else {
                                AssistantPhase::FinalAnswer
                            };
                            self.entries.push(Entry::Assistant {
                                blocks: markdown::parse(&text),
                                text,
                                streaming: false,
                                phase,
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
                                let detail = describe_call(&name, &args);
                                let copy_text = tool_copy_text(&name, &args, &detail);
                                self.entries.push(Entry::Tool(ToolCard {
                                    id: self.entries.len() as u64,
                                    detail,
                                    name,
                                    copy_text,
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
                            copy_text: String::new(),
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

fn tool_copy_text(name: &str, args: &Value, detail: &str) -> String {
    if name == "bash" {
        args["command"]
            .as_str()
            .filter(|command| !command.trim().is_empty())
            .unwrap_or(detail)
            .to_string()
    } else {
        detail.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tool_states(chat: &Chat) -> Vec<(Status, String)> {
        chat.entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Tool(card) => Some((card.status, card.output.clone())),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn batched_calls_of_one_tool_settle_the_card_that_was_waiting() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.run = Some(1);
        for path in ["src/a.rs", "src/b.rs"] {
            chat.apply(&json!({
                "type": "tool_call",
                "toolName": "read",
                "arguments": { "path": path },
            }));
        }
        chat.apply(&json!({
            "type": "tool_execution_end",
            "toolName": "read",
            "result": "a",
            "isError": false,
        }));
        chat.apply(&json!({
            "type": "tool_execution_end",
            "toolName": "read",
            "result": "b",
            "isError": false,
        }));
        assert_eq!(
            tool_states(&chat),
            vec![
                (Status::Done, "a".to_string()),
                (Status::Done, "b".to_string())
            ]
        );
    }

    #[test]
    fn a_turn_that_ends_closes_a_call_whose_result_never_came() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.run = Some(1);
        chat.apply(&json!({
            "type": "tool_call",
            "toolName": "read",
            "arguments": { "path": "src/a.rs" },
        }));
        chat.apply(&json!({ "type": "agent_end" }));
        assert_eq!(tool_states(&chat), vec![(Status::Done, String::new())]);
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
    fn commentary_and_the_final_answer_keep_their_codex_phases() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.apply(&json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "text_delta", "delta": "I'll inspect it." }
        }));
        chat.apply(&json!({ "type": "message_phase", "phase": "commentary" }));
        assert!(matches!(
            &chat.entries[0],
            Entry::Assistant {
                phase: AssistantPhase::Commentary,
                ..
            }
        ));

        chat.apply(&json!({ "type": "thinking_done" }));
        chat.apply(&json!({
            "type": "tool_call",
            "toolName": "read",
            "arguments": { "path": "src/main.rs" }
        }));
        chat.apply(&json!({
            "type": "message_update",
            "assistantMessageEvent": { "type": "text_delta", "delta": "It is fixed." }
        }));
        chat.apply(&json!({ "type": "message_phase", "phase": "final_answer" }));
        assert!(matches!(
            chat.entries.last(),
            Some(Entry::Assistant {
                phase: AssistantPhase::FinalAnswer,
                ..
            })
        ));
    }

    #[test]
    fn steer_replies_settle_after_the_run_by_id_not_by_prompt() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.run = Some(7);
        let rejected = chat.queue_steer("same prompt".to_string(), Vec::new(), false);
        let accepted = chat.queue_steer("same prompt".to_string(), Vec::new(), true);

        chat.finish_steer(rejected, false);
        chat.finish_steer(accepted, true);
        assert!(chat.settle_steers().is_none());

        chat.run = None;
        let next = chat.settle_steers().expect("the rejected message starts");
        assert_eq!(next.text, "same prompt");
        assert!(next.attachments.is_empty());
        assert_eq!(chat.entries.len(), 1);
        assert!(matches!(
            &chat.entries[0],
            Entry::User { text, .. } if text == "same prompt"
        ));
        assert!(chat.queued.is_empty());
    }

    #[test]
    fn a_tool_card_keeps_the_whole_command_for_copying() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.apply(&json!({
            "type": "tool_call",
            "toolName": "bash",
            "arguments": { "command": "cargo test\ncargo clippy --all-targets" },
        }));
        let Entry::Tool(card) = &chat.entries[0] else {
            panic!("expected a tool card");
        };
        assert_eq!(card.detail, "cargo test");
        assert_eq!(card.copy_text, "cargo test\ncargo clippy --all-targets");
    }

    #[test]
    fn a_tool_call_completes_the_markdown_step_before_it() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.apply(&json!({
            "type": "message_update",
            "assistantMessageEvent": {
                "type": "text_delta",
                "delta": "```mermaid\nflowchart LR\n  A --> B\n```"
            }
        }));
        chat.apply(&json!({
            "type": "tool_call",
            "toolName": "bash",
            "arguments": { "command": "true" }
        }));
        assert!(matches!(
            &chat.entries[0],
            Entry::Assistant {
                streaming: false,
                ..
            }
        ));
    }

    #[test]
    fn an_empty_step_boundary_does_not_add_a_thought_row() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.apply(&json!({ "type": "thinking" }));
        chat.apply(&json!({
            "type": "thinking_done",
            "millis": 4200,
        }));
        assert!(chat.entries.is_empty());

        chat.apply(&json!({
            "type": "message_update",
            "assistantMessageEvent": {
                "type": "thinking_delta",
                "delta": "Public compatibility summary"
            }
        }));
        assert!(matches!(
            chat.entries.as_slice(),
            [Entry::Thinking {
                text,
                done: false,
                ..
            }] if text == "Public compatibility summary"
        ));
    }

    #[test]
    fn a_run_end_waits_for_the_steer_reply_before_painting_anything() {
        let mut chat = Chat::new("/tmp".to_string(), "New chat".to_string());
        chat.run = Some(9);
        let id = chat.queue_steer("keep this".to_string(), Vec::new(), false);
        chat.run = None;

        assert!(chat.settle_steers().is_none());
        chat.finish_steer(id, false);
        let next = chat.settle_steers().expect("it becomes the next turn");
        assert_eq!(next.text, "keep this");
        assert!(chat.entries.is_empty());
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
        assert_eq!(session.title, "Align GPUI window with the Electron app");
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
