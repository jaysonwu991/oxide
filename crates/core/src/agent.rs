use crate::ask::{self, Asker, Question, Reply};
use crate::config::Config;
use crate::ecosystem::AgentMode;
use crate::llm::{
    ContentPart, FunctionSpec, LlmClient, Message, MessageContent, Retry, StreamHooks, ToolCall,
    ToolSpec,
};
use crate::lsp::LspManager;
use crate::mcp::McpRegistry;
use crate::memory::QueryScope;
use crate::permission::{subject_for, Action, Permissions};
use crate::plugin::PluginHost;
use crate::session::SessionLog;
use crate::snapshots::Snapshots;
use crate::tools;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc::UnboundedSender;

const MAX_TASK_DEPTH: usize = 2;

#[derive(Debug, Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UsageSource {
    Main,
    Compaction,
    Background,
}

/// A user can send several messages while one run is still working. Providers
/// otherwise tend to treat the newest one as a replacement prompt, especially
/// when it is only an attachment, and can abandon work requested by the earlier
/// messages even though every message is present in the conversation. Keep the
/// contract in the core request so the TUI, RPC clients, desktop app and any
/// future front-end all get the same cumulative semantics.
///
/// It rides on the one request that carries the message it is about, rather
/// than every request for the rest of the run: a model told on each step that
/// the user has added messages goes looking for input it has already answered.
const CUMULATIVE_INPUT_REMINDER: &str = "The user has added one or more messages while you were working. They are in this conversation, after the request that started the run; treat them as cumulative with every earlier request unless the user explicitly says to cancel, replace, or supersede one. Before finishing, verify that you have addressed every still-applicable user message, not only the most recent one.";

pub type RunFuture = Pin<Box<dyn Future<Output = ()> + Send>>;

pub type Approver =
    Arc<dyn Fn(String, String) -> Pin<Box<dyn Future<Output = bool> + Send>> + Send + Sync>;

/// How a queue's messages are delivered, Pi's `steeringMode`/`followUpMode`.
/// `one-at-a-time` (the default) hands over one message per opportunity so the
/// model answers each before the next arrives; `all` hands over the queue at
/// once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum QueueMode {
    All,
    #[default]
    OneAtATime,
}

impl QueueMode {
    /// Parses Pi's spelling; an unknown value is rejected so the caller can
    /// keep its default.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "all" => Some(Self::All),
            "one-at-a-time" => Some(Self::OneAtATime),
            _ => None,
        }
    }
}

/// A message waiting in a [`Steering`] queue, with the label a front-end shows
/// it by. The label travels with the message so a delivered turn can name the
/// files it carried — a content part holds the bytes, not the path the composer
/// read them from.
#[derive(Clone, Debug)]
pub struct Queued {
    pub message: Message,
    pub label: String,
}

/// A queue of user messages typed while the agent is busy. They are injected
/// into the conversation between steps, so the model sees the guidance without
/// interrupting the in-flight tool batch.
#[derive(Clone, Default)]
pub struct Steering {
    queue: Arc<Mutex<SteeringState>>,
}

#[derive(Default)]
struct SteeringState {
    messages: Vec<Queued>,
    closed: bool,
}

impl SteeringState {
    /// The messages due now, honoring `mode`.
    fn take(&mut self, mode: QueueMode) -> Vec<Queued> {
        match mode {
            QueueMode::All => std::mem::take(&mut self.messages),
            QueueMode::OneAtATime if self.messages.is_empty() => Vec::new(),
            QueueMode::OneAtATime => vec![self.messages.remove(0)],
        }
    }
}

impl Steering {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a message while the turn can still consume it, labelled by its own
    /// display. `false` means the run has made its final queue decision, so a
    /// client must start the message as a new turn instead of reporting it as
    /// accepted.
    pub fn push(&self, message: Message) -> bool {
        let label = message.display().unwrap_or_default();
        self.push_labeled(message, label)
    }

    /// Adds a message with the label a front-end shows it by (a queued message
    /// names the file each attachment was read from, which the message itself
    /// does not carry).
    pub fn push_labeled(&self, message: Message, label: String) -> bool {
        self.queue.lock().is_ok_and(|mut state| {
            if state.closed {
                return false;
            }
            state.messages.push(Queued { message, label });
            true
        })
    }

    /// The messages due now, dropping their labels.
    pub fn drain(&self, mode: QueueMode) -> Vec<Message> {
        self.drain_labeled(mode)
            .into_iter()
            .map(|queued| queued.message)
            .collect()
    }

    /// The messages due now, with the label each is shown by.
    pub fn drain_labeled(&self, mode: QueueMode) -> Vec<Queued> {
        self.queue
            .lock()
            .map(|mut state| state.take(mode))
            .unwrap_or_default()
    }

    /// Every queued message, whatever the mode, for a front-end that pulls the
    /// whole queue back into the composer.
    pub fn drain_all(&self) -> Vec<Message> {
        self.queue
            .lock()
            .map(|mut state| {
                std::mem::take(&mut state.messages)
                    .into_iter()
                    .map(|queued| queued.message)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// The label of every queued message, oldest first, for a front-end that
    /// lists the queue without cloning the messages (a media part can hold a
    /// multi-megabyte data URL).
    pub fn labels(&self) -> Vec<String> {
        self.queue
            .lock()
            .map(|state| {
                state
                    .messages
                    .iter()
                    .map(|queued| queued.label.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Stops accepting new messages while leaving already accepted input for
    /// the owner to drain or preserve at completion.
    pub fn close(&self) {
        if let Ok(mut state) = self.queue.lock() {
            state.closed = true;
        }
    }

    /// Atomically takes the messages available at the end of an assistant
    /// response. If neither queue contains work, both are closed while their
    /// locks are still held; a concurrent sender therefore either appears in
    /// this result or gets a reliable rejection.
    fn drain_at_response(
        &self,
        follow_ups: &Self,
        steering_mode: QueueMode,
        follow_up_mode: QueueMode,
        close_if_empty: bool,
    ) -> (Vec<Queued>, Vec<Queued>) {
        let Ok(mut steering) = self.queue.lock() else {
            return (Vec::new(), Vec::new());
        };
        let Ok(mut follow_ups) = follow_ups.queue.lock() else {
            return (Vec::new(), Vec::new());
        };
        let steered = steering.take(steering_mode);
        if !steered.is_empty() {
            return (steered, Vec::new());
        }
        let queued = follow_ups.take(follow_up_mode);
        if close_if_empty && queued.is_empty() {
            steering.closed = true;
            follow_ups.closed = true;
        }
        (steered, queued)
    }

    fn close_and_drain_pair(&self, other: &Self) -> Vec<Message> {
        let Ok(mut first) = self.queue.lock() else {
            return Vec::new();
        };
        let Ok(mut second) = other.queue.lock() else {
            return Vec::new();
        };
        first.closed = true;
        second.closed = true;
        let mut pending: Vec<Message> = std::mem::take(&mut first.messages)
            .into_iter()
            .map(|queued| queued.message)
            .collect();
        pending.extend(
            std::mem::take(&mut second.messages)
                .into_iter()
                .map(|queued| queued.message),
        );
        pending
    }

    pub fn len(&self) -> usize {
        self.queue
            .lock()
            .map(|state| state.messages.len())
            .unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// A sticky cancellation signal shared by a turn and its tool tasks.
#[derive(Clone)]
pub struct Cancel {
    signal: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Default for Cancel {
    fn default() -> Self {
        let (signal, _) = tokio::sync::watch::channel(false);
        Self {
            signal: Arc::new(signal),
        }
    }
}

impl Cancel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.signal.send_replace(true);
    }

    pub fn is_cancelled(&self) -> bool {
        *self.signal.borrow()
    }

    pub async fn cancelled(&self) {
        let mut receiver = self.signal.subscribe();
        let _ = receiver.wait_for(|cancelled| *cancelled).await;
    }

    /// Drops an in-flight wait when Stop arrives, including retry backoffs.
    pub async fn until<T>(&self, future: impl Future<Output = T>) -> Option<T> {
        tokio::select! {
            biased;
            _ = self.cancelled() => None,
            result = future => Some(result),
        }
    }
}

#[derive(Clone)]
pub struct Runtime {
    pub mcp: Arc<McpRegistry>,
    pub selected_mcp_tools: Arc<Mutex<BTreeSet<String>>>,
    pub plugins: Arc<PluginHost>,
    pub session: Option<Arc<SessionLog>>,
    pub snapshots: Option<Arc<Snapshots>>,
    pub lsp: Arc<LspManager>,
    pub approve: Approver,
    /// How the model asks the user a question. `None` leaves the `ask` tool out
    /// of the tool list entirely, so a run with nobody to answer (a print run)
    /// never has the model ask and wait.
    pub ask: Option<Asker>,
    pub steering: Steering,
    pub follow_ups: Steering,
    /// How each queue is delivered, from `steeringMode`/`followUpMode`.
    pub steering_mode: QueueMode,
    pub follow_up_mode: QueueMode,
    pub cancel: Cancel,
    /// How a finished turn is kept warm in the provider's prompt cache. `None`
    /// for a subagent (its conversation is discarded) and for a one-shot run
    /// that ends with the process.
    pub cache_warm: Option<WarmContext>,
}

/// The configuration and folder a finished turn is replayed from while the
/// reader is idle (see [`crate::cache_warmer`]).
#[derive(Clone)]
pub struct WarmContext {
    pub config: Config,
    pub cwd: std::path::PathBuf,
    pub session: Option<String>,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    /// A tool call is waiting on the user's permission decision. The front-end
    /// answers by id through the approval broker; the event carries enough to
    /// describe the call on its own, and arrives after the `ToolCall` it
    /// belongs to so a view can pair it with the tool card it just rendered.
    ApprovalRequest {
        id: u64,
        tool: String,
        detail: String,
    },
    /// The model is waiting on the user's answer to a question. The front-end
    /// answers by id through the question broker; the event carries the
    /// questions so a view can paint them on its own, and arrives after the
    /// `ToolCall` it belongs to so a view can pair them.
    QuestionRequest {
        id: u64,
        questions: Vec<Question>,
    },
    /// The request with that id is gone without an answer, because it timed out
    /// with nobody answering: a view that painted the questions hides them
    /// instead of offering an answer nothing is waiting for. Only a timeout is
    /// announced — a front-end that answered, or a run that ended with the
    /// request still open, already knows.
    QuestionClosed {
        id: u64,
    },
    /// The model named a new conversation. This is session metadata rather
    /// than a visible tool call, and every front-end updates the same row/header
    /// from this event.
    SessionTitle {
        title: String,
    },
    Text(String),
    /// A fragment of the model's reasoning, streamed before its answer.
    ThinkingDelta(String),
    Thought {
        millis: u64,
    },
    /// The total wall-clock time the current model step took, sent once the
    /// stream finishes so the `Thought` line can be updated.
    ThoughtDone {
        millis: u64,
    },
    /// A transient provider failure that is about to be retried. The failed
    /// attempt's streamed text and reasoning are abandoned, so a view drops the
    /// item it was building and lets the retry start fresh.
    Retrying {
        attempt: u32,
        max: u32,
        delay_ms: u64,
    },
    ToolCall {
        name: String,
        args: String,
    },
    /// A nested subagent's current tool call. A `task` run otherwise emits
    /// nothing until it returns, which is indistinguishable from a hang.
    SubagentActivity {
        agent: String,
        tool: String,
        args: String,
    },
    ToolProgress {
        name: String,
        chunk: String,
    },
    ToolResult {
        name: String,
        args: String,
        output: String,
        is_error: bool,
        diff: Option<tools::DiffPreview>,
        /// Wall-clock time the tool spent running, in milliseconds.
        millis: u64,
    },
    /// Token usage reported by the provider for the turn just completed.
    Usage {
        model_calls: u64,
        source: UsageSource,
        input: u64,
        output: u64,
        cache_read: u64,
        cache_write: u64,
        cost: f64,
        context_tokens: Option<u64>,
    },
    /// The backend reported a smaller context window than the model catalog.
    ContextWindow {
        tokens: u64,
        provider: String,
        model: String,
    },
    /// Older turns were replaced with a summary to free context.
    Compaction {
        summary: String,
        summarized: usize,
        tokens_before: u64,
        read_files: Vec<String>,
        modified_files: Vec<String>,
    },
    /// The TUI navigated the session tree and continued from another entry.
    Branch {
        history: Vec<Message>,
        prompt: String,
        message: String,
        /// The summary written for the branch being left, when one was made.
        summary: Option<String>,
    },
    /// A user message typed while the run was busy has been delivered into the
    /// conversation. The front-end paints it as a turn now, not when it was
    /// queued; `follow_up` says which queue it came from, and `label` is the
    /// text it was shown by while queued (naming its attachments).
    Steered {
        message: Message,
        label: String,
        follow_up: bool,
    },
    Error(String),
    Finished(Vec<Message>),
}

/// Drive one conversation turn to completion: stream assistant output, execute
/// any requested tools, feed the results back, and repeat until the model stops
/// asking for tools.
pub fn run(
    config: Config,
    cwd: PathBuf,
    history: Vec<Message>,
    tx: UnboundedSender<AgentEvent>,
    runtime: Runtime,
) -> RunFuture {
    run_depth(config, cwd, history, tx, runtime, 0)
}

/// Runs a slash command in an isolated subagent context. The subagent streams
/// into `tx`, and its final text is appended to `history` as an assistant
/// message before `Finished` is emitted with the merged history.
pub fn run_subagent(
    config: Config,
    cwd: PathBuf,
    history: Vec<Message>,
    agent_name: String,
    prompt: String,
    tx: UnboundedSender<AgentEvent>,
    runtime: Runtime,
) -> RunFuture {
    // A queued Codex-style follow-up is a new primary turn, not extra input to
    // an isolated subtask whose history is deliberately discarded. Reject it
    // up front so each client can start it normally after this run finishes.
    runtime.follow_ups.close();
    Box::pin(async move {
        let agent = match config.ecosystem.agent(&agent_name).cloned() {
            Some(agent) if agent.mode != AgentMode::Primary => agent,
            Some(_) => {
                let _ = tx.send(AgentEvent::Error(format!(
                    "agent `{agent_name}` is primary and cannot run as a subagent"
                )));
                finish(&runtime, &tx, history, 0);
                return;
            }
            None => {
                let _ = tx.send(AgentEvent::Error(format!("unknown agent `{agent_name}`")));
                finish(&runtime, &tx, history, 0);
                return;
            }
        };

        let session = runtime.session.clone();
        let outer_runtime = runtime.clone();
        let mut sub = config;
        sub.active_agent = Some(agent);
        let sub_runtime = Runtime {
            session: None,
            selected_mcp_tools: Default::default(),
            follow_ups: Steering::new(),
            cache_warm: None,
            ..runtime
        };

        let (sub_tx, mut sub_rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = tokio::spawn(run_depth(
            sub,
            cwd,
            vec![Message::user(prompt)],
            sub_tx,
            sub_runtime,
            1,
        ));

        let mut report = String::new();
        // Text before a tool call belongs to a completed step and is kept; a
        // retry re-sends the current step from the start, so its partial text
        // is dropped instead of accumulating twice.
        let mut committed = 0usize;
        while let Some(event) = sub_rx.recv().await {
            match event {
                AgentEvent::Text(delta) => {
                    report.push_str(&delta);
                    let _ = tx.send(AgentEvent::Text(delta));
                }
                AgentEvent::ThinkingDelta(delta) => {
                    let _ = tx.send(AgentEvent::ThinkingDelta(delta));
                }
                AgentEvent::Thought { millis } => {
                    let _ = tx.send(AgentEvent::Thought { millis });
                }
                AgentEvent::ThoughtDone { millis } => {
                    // A no-tool step commits here, not at the next tool call.
                    committed = report.len();
                    let _ = tx.send(AgentEvent::ThoughtDone { millis });
                }
                AgentEvent::Retrying {
                    attempt,
                    max,
                    delay_ms,
                } => {
                    report.truncate(committed);
                    let _ = tx.send(AgentEvent::Retrying {
                        attempt,
                        max,
                        delay_ms,
                    });
                }
                AgentEvent::ToolCall { name, args } => {
                    committed = report.len();
                    let _ = tx.send(AgentEvent::ToolCall { name, args });
                }
                // Forwarded so a subagent's prompt reaches the front-end: the
                // request id is broker-wide, so the answer still lands.
                AgentEvent::ApprovalRequest { id, tool, detail } => {
                    let _ = tx.send(AgentEvent::ApprovalRequest { id, tool, detail });
                }
                // A subagent's question reaches the same front-end the run is
                // drawn in; the request id is broker-wide, so the answer still
                // lands.
                AgentEvent::QuestionRequest { id, questions } => {
                    let _ = tx.send(AgentEvent::QuestionRequest { id, questions });
                }
                AgentEvent::QuestionClosed { id } => {
                    let _ = tx.send(AgentEvent::QuestionClosed { id });
                }
                // Subagents have no persisted session and are never offered
                // the title tool; ignore a synthetic event defensively.
                AgentEvent::SessionTitle { .. } => {}
                AgentEvent::SubagentActivity { agent, tool, args } => {
                    let _ = tx.send(AgentEvent::SubagentActivity { agent, tool, args });
                }
                AgentEvent::ToolProgress { name, chunk } => {
                    let _ = tx.send(AgentEvent::ToolProgress { name, chunk });
                }
                AgentEvent::ToolResult {
                    name,
                    args,
                    output,
                    is_error,
                    diff,
                    millis,
                } => {
                    let _ = tx.send(AgentEvent::ToolResult {
                        name,
                        args,
                        output,
                        is_error,
                        diff,
                        millis,
                    });
                }
                AgentEvent::Usage {
                    model_calls,
                    source: _,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    cost,
                    context_tokens,
                } => {
                    record_background_usage(
                        &session,
                        model_calls,
                        input,
                        output,
                        cache_read,
                        cache_write,
                        cost,
                    );
                    let _ = tx.send(AgentEvent::Usage {
                        model_calls,
                        source: UsageSource::Background,
                        input,
                        output,
                        cache_read,
                        cache_write,
                        cost,
                        context_tokens,
                    });
                }
                AgentEvent::ContextWindow { .. } => {}
                AgentEvent::Compaction {
                    summary,
                    summarized,
                    tokens_before,
                    read_files,
                    modified_files,
                } => {
                    let _ = tx.send(AgentEvent::Compaction {
                        summary,
                        summarized,
                        tokens_before,
                        read_files,
                        modified_files,
                    });
                }
                AgentEvent::Branch {
                    history,
                    prompt,
                    message,
                    summary,
                } => {
                    let _ = tx.send(AgentEvent::Branch {
                        history,
                        prompt,
                        message,
                        summary,
                    });
                }
                // The subagent's own delivery is not the main transcript's.
                AgentEvent::Steered { .. } => {}
                AgentEvent::Error(message) => {
                    let _ = tx.send(AgentEvent::Error(message));
                }
                AgentEvent::Finished(_) => break,
            }
        }
        let _ = handle.await;

        let mut merged = history;
        if !report.trim().is_empty() {
            let message = Message::assistant(report, Vec::new());
            if let Some(log) = &session {
                let _ = log.append(&message);
            }
            merged.push(message);
        }
        finish(&outer_runtime, &tx, merged, 0);
    })
}

fn run_depth(
    mut config: Config,
    cwd: PathBuf,
    history: Vec<Message>,
    tx: UnboundedSender<AgentEvent>,
    runtime: Runtime,
    depth: usize,
) -> RunFuture {
    if config.turn_meter.is_none() {
        config.turn_meter = Some(Arc::new(Mutex::new(crate::efficiency::TurnBudget::new(
            config.token_efficiency.turn_budget.clone(),
        ))));
    }
    Box::pin(run_loop(config, cwd, history, tx, runtime, depth))
}

async fn run_loop(
    config: Config,
    cwd: PathBuf,
    history: Vec<Message>,
    tx: UnboundedSender<AgentEvent>,
    runtime: Runtime,
    depth: usize,
) {
    if depth == 0 {
        if let Some(log) = &runtime.session {
            let _ = log.append_model_change(&config.provider, &config.model);
            let _ = log.append_thinking_level(config.reasoning.label());
        }
    }
    let client = match runtime.session.as_ref() {
        // The session id doubles as the provider's cache-affinity key, so
        // successive turns reuse the cached conversation prefix.
        Some(log) => LlmClient::new(config.clone()).with_session_id(log.id()),
        None => LlmClient::new(config.clone()),
    };
    let permissions = Permissions::from_config(&config);
    let mut messages = history;
    let mut workspace_context = project_context(&cwd, &messages);
    let mut learned_limit = runtime
        .session
        .as_ref()
        .and_then(|log| log.context_window(&config.provider, &config.model));
    let mut context_window = learned_limit.map_or(config.context_window(), |limit| {
        config.context_window().min(limit)
    });
    if learned_limit.is_some() && depth == 0 {
        let _ = tx.send(AgentEvent::ContextWindow {
            tokens: context_window,
            provider: config.provider.clone(),
            model: config.model.clone(),
        });
    }
    let compaction_budget = config.compaction.resolve(&config.provider, &config.model);
    let (mut context_tokens, mut measured_at) = runtime
        .session
        .as_ref()
        .and_then(|log| log.context_measurement())
        .map_or((0, 0), |(usage, boundary)| {
            (usage.prompt + usage.output, boundary)
        });
    let mut overflow_error: Option<String> = None;
    let mut recovered_overflow = false;
    {
        let mut selected = runtime
            .selected_mcp_tools
            .lock()
            .expect("tool selection poisoned");
        let snapshot = runtime
            .session
            .as_ref()
            .and_then(|log| log.selected_tools_snapshot());
        if let Some(snapshot) = snapshot {
            *selected = snapshot.into_iter().collect();
        } else {
            selected.extend(
                messages
                    .iter()
                    .filter_map(|m| m.tool_calls.as_ref())
                    .flatten()
                    .filter(|call| call.function.name.contains("__"))
                    .map(|call| call.function.name.clone()),
            );
        }
    }
    {
        let mut selected = runtime
            .selected_mcp_tools
            .lock()
            .expect("tool selection poisoned");
        let recent: Vec<String> = messages
            .iter()
            .rev()
            .filter_map(|m| m.tool_calls.as_ref())
            .flatten()
            .filter(|call| selected.contains(&call.function.name))
            .map(|call| call.function.name.clone())
            .collect();
        *selected = bounded_selection(
            &selected,
            &recent,
            config.token_efficiency.max_declared_mcp_tools,
        );
    }
    runtime.mcp.start_discovery();
    if !runtime
        .selected_mcp_tools
        .lock()
        .expect("tool selection poisoned")
        .is_empty()
    {
        runtime
            .cancel
            .until(tokio::time::timeout(
                std::time::Duration::from_secs(10),
                runtime.mcp.discover(),
            ))
            .await;
    }

    // Set once a file edit succeeds and cleared when a later tool call could
    // confirm it. If a run tries to finish while an edit is unconfirmed, the
    // model gets one hidden reminder to verify before its summary. A file
    // written outside the run's directory is not part of that delivery.
    let mut verification = VerificationState::new(&cwd);
    if let Some(user) = messages.iter().rev().find(|message| message.role == "user") {
        verification.request_replies(&user.display().unwrap_or_default());
    }
    let mut verification_reminder: Option<String> = None;
    let mut follow_through_nudged = false;
    let mut follow_through_pending = false;
    // Verification commands already run this turn, so an exact repeat can be
    // flagged instead of silently re-run (a slow build or test suite).
    let mut seen_verifications: BTreeSet<String> = BTreeSet::new();
    // A model that cannot make out an attachment sometimes calls `read` on the
    // same image repeatedly. Each successful read used to append the complete
    // base64 payload again, multiplying the context without adding evidence.
    // Seed this from the active history so resumed turns also reuse media that
    // is still present (a compaction naturally removes media it replaced).
    let mut seen_media = deduplicate_tool_media(&mut messages);
    // A summary whose verification re-check is still pending. It is kept out of
    // the log and the transcript until the re-check either extends it or the run
    // ends, so a re-checked answer reads as one summary instead of two.
    let mut held_summary: Option<(Message, Option<crate::compact::UsageRecord>)> = None;
    // Whether the held summary is still the newest message, which is what lets
    // the re-check merge into it rather than start a second one.
    let mut held_is_tail = false;
    // A response boundary that already injected a steering message sets this so
    // the top-of-loop poll does not immediately take the next one too: under
    // `one-at-a-time` only one queued message may precede a model call.
    let mut skip_steering_poll = false;
    // A request that delivers user input the run accepted mid-flight carries
    // an explicit cumulative-input contract. The transcript already contains the
    // messages themselves; this only prevents the newest from being mistaken
    // for a replacement of unfinished earlier work. It is consumed by that one
    // request, since the reminder is news about the step it arrives on and
    // repeats as a phantom message on every step after it.
    let mut mid_run_input_reminder = false;
    // A provider may return the metadata tool without the requested answer.
    // Offer it at most once per run so an invalid title or an unwritable log
    // cannot turn that fallback into an unbounded request loop.
    let mut title_attempted = false;
    let read_reuse = Arc::new(Mutex::new(crate::efficiency::ReadReuse::default()));
    let mut failure_loop = FailureLoop::default();
    // A title-only response is not a valid conversation message. Hold its
    // usage until a real assistant response arrives, then persist both API
    // requests on that response without replaying an empty assistant turn.
    let mut pending_title_usage: Option<crate::compact::UsageRecord> = None;

    loop {
        if runtime.cancel.is_cancelled() {
            // A summary held for its re-check is still the answer: keep it.
            if let Some((held, held_usage)) = held_summary.take() {
                record_usage(&runtime.session, depth, &held, held_usage);
                send_usage(&tx, depth, held_usage);
            }
            send_usage(&tx, depth, pending_title_usage.take());
            finish(&runtime, &tx, messages, depth);
            return;
        }
        if let Some(reason) = config
            .turn_meter
            .as_ref()
            .and_then(|meter| meter.lock().expect("turn budget poisoned").reason())
        {
            if let Some((held, usage)) = held_summary.take() {
                record_usage(&runtime.session, depth, &held, usage);
                send_usage(&tx, depth, usage);
            }
            send_usage(&tx, depth, pending_title_usage.take());
            budget_checkpoint(&runtime, &tx, &mut messages, depth, &reason);
            finish(&runtime, &tx, messages, depth);
            return;
        }
        if !skip_steering_poll {
            for queued in runtime.steering.drain_labeled(runtime.steering_mode) {
                mid_run_input_reminder = true;
                let Queued {
                    message: steered,
                    label,
                } = queued;
                // A held summary is committed first, so a queued message cannot
                // land in the log ahead of the summary it followed in the thread.
                if let Some((held, held_usage)) = held_summary.take() {
                    record_usage(&runtime.session, depth, &held, held_usage);
                    held_is_tail = false;
                }
                let _ = tx.send(AgentEvent::Steered {
                    message: steered.clone(),
                    label,
                    follow_up: false,
                });
                verification.request_replies(&steered.display().unwrap_or_default());
                record(&runtime.session, depth, &steered);
                messages.push(steered);
                workspace_context = project_context(&cwd, &messages);
            }
        }
        skip_steering_poll = false;

        let reclaimed = crate::efficiency::prune_tool_history(
            &mut messages,
            measured_at,
            &config.token_efficiency.tool_history,
            tools::save_tool_output,
        );
        if reclaimed > 0 {
            context_tokens = context_tokens.saturating_sub(reclaimed as u64);
        }
        read_reuse
            .lock()
            .expect("read reuse poisoned")
            .retain_context(&messages);

        // Pi-style auto-compaction: once the outgoing context approaches the
        // model window, replace older turns with a summary and keep the recent
        // tokens verbatim. The record is stored on the session branch.
        if depth == 0 && compaction_budget.enabled {
            let history_tokens = crate::compact::estimate_tokens(&messages) as u64;
            let growth =
                crate::compact::estimate_tokens(&messages[measured_at.min(messages.len())..])
                    as u64;
            let current = context_tokens.saturating_add(growth).max(history_tokens);
            if overflow_error.is_some()
                || crate::compact::needs_compaction(current, context_window, compaction_budget)
            {
                if let Some(log) = &runtime.session {
                    match runtime
                        .cancel
                        .until(crate::compact::generate(
                            &config,
                            &messages,
                            &[],
                            compaction_budget.for_request(&messages),
                            current,
                            None,
                        ))
                        .await
                        .unwrap_or(Ok(None))
                    {
                        Ok(Some(mut compaction)) => {
                            compaction.details.context_limit =
                                learned_limit.map(|window| crate::compact::ModelContextLimit {
                                    provider: config.provider.clone(),
                                    model: config.model.clone(),
                                    window,
                                });
                            let first_kept = log
                                .context_ids()
                                .get(compaction.first_kept)
                                .cloned()
                                .unwrap_or_else(|| compaction.first_kept.to_string());
                            let saved = log
                                .append_compaction(
                                    compaction.summary.clone(),
                                    first_kept,
                                    compaction.tokens_before,
                                    Some(compaction.details.clone()),
                                    compaction.usage,
                                )
                                .and_then(|_| log.messages());
                            match saved {
                                Ok(refreshed) => {
                                    messages = refreshed;
                                    seen_media = deduplicate_tool_media(&mut messages);
                                }
                                Err(error) => {
                                    let _ = tx.send(AgentEvent::Error(format!(
                                        "saving compaction: {error:#}"
                                    )));
                                    if let Some((held, held_usage)) = held_summary.take() {
                                        record_usage(&runtime.session, depth, &held, held_usage);
                                        send_usage(&tx, depth, held_usage);
                                    }
                                    send_usage(&tx, depth, pending_title_usage.take());
                                    finish(&runtime, &tx, messages, depth);
                                    return;
                                }
                            }
                            let _ = tx.send(AgentEvent::Compaction {
                                summary: compaction.summary.clone(),
                                summarized: compaction.summarized,
                                tokens_before: compaction.tokens_before,
                                read_files: compaction.details.read_files.clone(),
                                modified_files: compaction.details.modified_files.clone(),
                            });
                            // Summarizing history does not remove the system
                            // prompt or tool schemas from the next request.
                            context_tokens = current.saturating_sub(history_tokens)
                                + crate::compact::estimate_tokens(&messages) as u64;
                            measured_at = messages.len();
                            if let Some(usage) = compaction.usage {
                                let _ = tx.send(AgentEvent::Usage {
                                    model_calls: usage.model_calls.max(1),
                                    source: if depth == 0 {
                                        UsageSource::Compaction
                                    } else {
                                        UsageSource::Background
                                    },
                                    input: usage.input,
                                    output: usage.output,
                                    cache_read: usage.cache_read,
                                    cache_write: usage.cache_write,
                                    cost: usage.cost,
                                    context_tokens: Some(context_tokens),
                                });
                            }
                            // The held summary is deliberately not in the log,
                            // so the compacted view dropped it; put it back so
                            // the re-check still sees the answer it extends.
                            if let Some((held, _)) = &held_summary {
                                messages.push(held.clone());
                                held_is_tail = true;
                            }
                            overflow_error = None;
                        }
                        Ok(None) => {}
                        Err(err) => {
                            let _ = tx.send(AgentEvent::Error(format!("compaction: {err:#}")));
                        }
                    }
                }
            }
        }

        // Recovery must shorten the request. If there was nothing to compact,
        // or summarization failed, retain the original provider failure.
        if let Some(error) = overflow_error.take() {
            if let Some((held, held_usage)) = held_summary.take() {
                record_usage(&runtime.session, depth, &held, held_usage);
                send_usage(&tx, depth, held_usage);
            }
            send_usage(&tx, depth, pending_title_usage.take());
            let _ = tx.send(AgentEvent::Error(error));
            finish(&runtime, &tx, messages, depth);
            return;
        }

        read_reuse
            .lock()
            .expect("read reuse poisoned")
            .retain_context(&messages);
        let mut request = Vec::with_capacity(messages.len() + 3);
        let mut system = config.compose_system_prompt();
        system.push_str(&workspace_context);
        if verification.replies_requested {
            system.push_str(
                "\n\nThe user requested replies to review comments. Read the inline review \
                comments with `gh api --paginate repos/{owner}/{repo}/pulls/<n>/comments`. \
                Preserve each root comment's numeric `id` and `path`, and each reply's \
                `in_reply_to_id`. After fixing and pushing, reply separately in each addressed \
                thread through its `/replies` endpoint, then re-read the comments to confirm \
                the replies. A top-level `gh pr comment` does not answer inline threads. \
                Report any comments you could not address or reply to; never claim all were \
                replied to from a summary comment alone. Follow the user's scope and any \
                instruction to skip posting.",
            );
        }
        let offer_session_title = needs_session_title(&runtime, depth) && !title_attempted;
        if offer_session_title {
            system.push_str(concat!(
                "\n\nThis unnamed conversation has no title. In this response, call ",
                "`session_title` once alongside your complete user-facing response and any ",
                "other tools; never call it alone."
            ));
        }
        // A loaded server's own instructions only reach the model here: one the
        // model loads itself says what it needs in its tool result, while an
        // auto-loaded server is never loaded by the model at all.
        if let Some(notes) = runtime.mcp.selected_instructions(
            &runtime
                .selected_mcp_tools
                .lock()
                .expect("tool selection poisoned"),
        ) {
            system.push_str("\n\n");
            system.push_str(&notes);
        }
        request.push(Message::system(system));
        request.extend(messages.iter().cloned());
        if std::mem::take(&mut mid_run_input_reminder) {
            request.push(Message::system(CUMULATIVE_INPUT_REMINDER));
        }
        // A reminder makes this request the hidden nudge: it asks the model to
        // confirm work it already summarized, so a provider that answers it
        // with nothing must not fail the turn the user already saw complete.
        let following_through = std::mem::take(&mut follow_through_pending);
        let nudging = match verification_reminder.take() {
            Some(reminder) => {
                // A system-only tail is lifted into the leading system prompt
                // by Anthropic gateways, leaving an unsupported assistant prefill.
                request.push(Message::user(reminder));
                true
            }
            None => false,
        };
        let tool_specs = build_tool_specs(&config, &runtime, depth, offer_session_title);
        // The held summary's text, replayed when the re-check is retried: the
        // failed attempt's item is discarded, and the retry has to restart the
        // bubble with the answer it is extending.
        let held_replay = held_summary
            .as_ref()
            .and_then(|(message, _)| message.display())
            .filter(|text| !text.is_empty());

        let started = std::time::Instant::now();
        // Reset on a retry: the failed attempt's `Thought` marker belongs to the
        // discarded output, so the retry sends its own when its text starts.
        let thought_sent = Arc::new(AtomicBool::new(false));
        // The current attempt's text, kept so an exhausted retry budget can keep
        // what the user already saw instead of losing it with the error.
        let attempt_text = Arc::new(Mutex::new(String::new()));

        let text_sent = Arc::clone(&thought_sent);
        let retry_sent = Arc::clone(&thought_sent);
        let text_delta = Arc::clone(&attempt_text);
        let retry_text = Arc::clone(&attempt_text);
        let text_tx = tx.clone();
        let retry_tx = tx.clone();
        let thinking_tx = tx.clone();
        let mut on_text = move |delta: String| {
            if let Ok(mut text) = text_delta.lock() {
                text.push_str(&delta);
            }
            if !text_sent.swap(true, Ordering::Relaxed) {
                let _ = text_tx.send(AgentEvent::Thought {
                    millis: started.elapsed().as_millis() as u64,
                });
            }
            let _ = text_tx.send(AgentEvent::Text(delta));
        };
        let mut on_thinking = move |delta: String| {
            let _ = thinking_tx.send(AgentEvent::ThinkingDelta(delta));
        };
        let mut on_retry = move |retry: Retry| {
            if let Ok(mut text) = retry_text.lock() {
                text.clear();
            }
            retry_sent.store(false, Ordering::Relaxed);
            let _ = retry_tx.send(AgentEvent::Retrying {
                attempt: retry.attempt,
                max: retry.max,
                delay_ms: retry.delay.as_millis() as u64,
            });
            // The retry re-opens the bubble with the summary it is extending, so
            // a dropped re-check does not leave the merged answer without its
            // first half on screen.
            if let Some(text) = held_replay.clone() {
                let _ = retry_tx.send(AgentEvent::Text(text));
            }
        };
        // This request writes the provider-side cache entry itself, so a warm
        // replay waiting on the previous entry is redundant now.
        crate::cache_warmer::cancel();
        let turn = {
            let mut hooks = StreamHooks {
                text: &mut on_text,
                thinking: &mut on_thinking,
                retry: &mut on_retry,
            };
            let response = runtime
                .cancel
                .until(client.stream_chat(&request, &tool_specs, &mut hooks))
                .await;
            let Some(response) = response else {
                if let Some((held, usage)) = held_summary.take() {
                    record_usage(&runtime.session, depth, &held, usage);
                    send_usage(&tx, depth, usage);
                }
                send_usage(&tx, depth, pending_title_usage.take());
                let streamed = attempt_text
                    .lock()
                    .map(|text| text.clone())
                    .unwrap_or_default();
                if !streamed.trim().is_empty() {
                    let partial = Message::assistant(streamed, Vec::new());
                    record(&runtime.session, depth, &partial);
                    messages.push(partial);
                }
                let _ = tx.send(AgentEvent::ThoughtDone {
                    millis: started.elapsed().as_millis() as u64,
                });
                finish(&runtime, &tx, messages, depth);
                return;
            };
            match response {
                Ok(turn) => turn,
                Err(err) if err.is::<crate::efficiency::BudgetExceeded>() => {
                    if let Some((held, usage)) = held_summary.take() {
                        record_usage(&runtime.session, depth, &held, usage);
                        send_usage(&tx, depth, usage);
                    }
                    send_usage(&tx, depth, pending_title_usage.take());
                    let reason = err
                        .downcast_ref::<crate::efficiency::BudgetExceeded>()
                        .expect("budget error checked");
                    budget_checkpoint(&runtime, &tx, &mut messages, depth, reason);
                    finish(&runtime, &tx, messages, depth);
                    return;
                }
                Err(err) if nudge_failed_quietly(nudging && !following_through, &err) => {
                    // The reminder is advisory: the model already summarized its
                    // work and nothing more will arrive, so finish with the
                    // answer in hand instead of reporting `err`. The held
                    // summary is committed and the thinking block this attempt
                    // may have streamed is closed first, so the transcript does
                    // not keep it open as still thinking.
                    if let Some((held, held_usage)) = held_summary.take() {
                        record_usage(&runtime.session, depth, &held, held_usage);
                        send_usage(&tx, depth, held_usage);
                    }
                    let _ = tx.send(AgentEvent::Thought {
                        millis: started.elapsed().as_millis() as u64,
                    });
                    let _ = tx.send(AgentEvent::ThoughtDone {
                        millis: started.elapsed().as_millis() as u64,
                    });
                    finish(&runtime, &tx, messages, depth);
                    return;
                }
                Err(err) => {
                    if depth == 0
                        && compaction_budget.enabled
                        && runtime.session.is_some()
                        && !recovered_overflow
                        && !runtime.cancel.is_cancelled()
                    {
                        if let Some(overflow) = crate::llm::context_overflow(&err) {
                            recovered_overflow = true;
                            if let Some(limit) = overflow.limit {
                                context_window = context_window.min(limit);
                                learned_limit = Some(context_window);
                                let _ = tx.send(AgentEvent::ContextWindow {
                                    tokens: context_window,
                                    provider: config.provider.clone(),
                                    model: config.model.clone(),
                                });
                            }
                            overflow_error = Some(format!("{err:#}"));
                            let _ = tx.send(AgentEvent::ThoughtDone {
                                millis: started.elapsed().as_millis() as u64,
                            });
                            continue;
                        }
                    }
                    // The retry budget is exhausted. Text the last attempt
                    // streamed was shown to the user, so it is kept in the
                    // session for the next turn rather than dropped with the
                    // error.
                    if let Some((held, held_usage)) = held_summary.take() {
                        record_usage(&runtime.session, depth, &held, held_usage);
                        send_usage(&tx, depth, held_usage);
                    }
                    send_usage(&tx, depth, pending_title_usage.take());
                    let streamed = attempt_text
                        .lock()
                        .map(|text| text.clone())
                        .unwrap_or_default();
                    if !streamed.trim().is_empty() {
                        // Keep the exact streamed string, including any leading
                        // or trailing whitespace the user already saw.
                        let partial = Message::assistant(streamed, Vec::new());
                        record(&runtime.session, depth, &partial);
                        messages.push(partial);
                    }
                    let _ = tx.send(AgentEvent::ThoughtDone {
                        millis: started.elapsed().as_millis() as u64,
                    });
                    let _ = tx.send(AgentEvent::Error(format!("{err:#}")));
                    finish(&runtime, &tx, messages, depth);
                    return;
                }
            }
        };
        if !thought_sent.load(Ordering::Relaxed) {
            let _ = tx.send(AgentEvent::Thought {
                millis: started.elapsed().as_millis() as u64,
            });
        }

        if turn.content.trim().is_empty() && turn.tool_calls.is_empty() {
            send_usage(&tx, depth, pending_title_usage.take());
            let _ = tx.send(AgentEvent::ThoughtDone {
                millis: started.elapsed().as_millis() as u64,
            });
            let _ = tx.send(AgentEvent::Error(
                "the model returned an empty response".to_string(),
            ));
            finish(&runtime, &tx, messages, depth);
            return;
        }

        let mut response_usage = (turn.usage.total() > 0).then(|| {
            let mut usage = crate::compact::UsageRecord::from(turn.usage);
            usage.context = turn.context_usage;
            usage
        });

        // A title is metadata, not an agent step. Consume it from the first
        // response before constructing the assistant message: no tool result is
        // sent back to the provider, so naming a simple conversation does not
        // trigger a second full-context model request or enter session history.
        let had_title_call = turn
            .tool_calls
            .iter()
            .any(|call| internal_tool(&call.function.name));
        let first_title_attempt = had_title_call && !title_attempted;
        title_attempted |= had_title_call;
        for call in turn
            .tool_calls
            .iter()
            .filter(|call| internal_tool(&call.function.name))
        {
            let _ = set_session_title(&runtime, &tx, &call.function.arguments, depth);
        }
        let tool_calls: Vec<_> = turn
            .tool_calls
            .iter()
            .filter(|call| !internal_tool(&call.function.name))
            .cloned()
            .collect();
        let has_answer = !turn.content.trim().is_empty();

        // Some providers insist on returning a tool call without the answer we
        // explicitly requested beside it. The title attempt is consumed and
        // its tool is now absent, so retry once as an ordinary response. Compliant
        // providers take the zero-extra-request path above; this fallback pays
        // another request only to avoid ending the user's turn with no answer.
        if first_title_attempt && tool_calls.is_empty() && !has_answer {
            if let Some(context) = response_usage
                .as_mut()
                .and_then(|usage| usage.context.as_mut())
            {
                context.output = 0; // The metadata-only response is never appended to history.
                context_tokens = context.prompt;
                measured_at = messages.len();
            }
            pending_title_usage = combine_usage(pending_title_usage, response_usage);
            let _ = tx.send(AgentEvent::ThoughtDone {
                millis: started.elapsed().as_millis() as u64,
            });
            continue;
        }

        // An internal-only response after the one permitted title attempt is
        // empty from the conversation's perspective. Stop instead of storing a
        // bare assistant role or asking the provider forever.
        if tool_calls.is_empty() && !has_answer {
            send_usage(
                &tx,
                depth,
                combine_usage(pending_title_usage.take(), response_usage),
            );
            let _ = tx.send(AgentEvent::ThoughtDone {
                millis: started.elapsed().as_millis() as u64,
            });
            let _ = tx.send(AgentEvent::Error(
                "the model returned no user-facing response after setting the session title"
                    .to_string(),
            ));
            finish(&runtime, &tx, messages, depth);
            return;
        }

        let response_context = response_usage.and_then(|usage| usage.context);
        let usage = combine_usage(pending_title_usage.take(), response_usage);
        let assistant = Message::assistant(turn.content, tool_calls.clone())
            .with_thinking(turn.thinking.clone());

        if tool_calls.is_empty() {
            // The final empty check and closing the input queues are one atomic
            // decision. A sender racing this boundary is therefore either
            // included below or told to start a new turn.
            let (mut steered, mut follow_ups) = runtime.steering.drain_at_response(
                &runtime.follow_ups,
                runtime.steering_mode,
                runtime.follow_up_mode,
                false,
            );
            let mut queued = !steered.is_empty() || !follow_ups.is_empty();

            // A reminder is only due on a plain answer with nothing queued. The
            // answer is held back so its re-check extends the same assistant
            // turn instead of adding a second summary.
            if !queued && !runtime.cancel.is_cancelled() {
                let reminder = verification.reminder().or_else(|| {
                    if !follow_through_nudged
                        && announces_unfinished_work(assistant.display().as_deref().unwrap_or(""))
                    {
                        follow_through_nudged = true;
                        follow_through_pending = true;
                        Some("You ended by announcing work without doing it. Complete the announced \
                              action or summary now, using tools when needed. If blocked, give the \
                              specific evidence and the completed outcome rather than another promise. \
                              Preserve the user's requested scope and stopping conditions.".to_string())
                    } else {
                        None
                    }
                });
                if let Some(reminder) = reminder {
                    let (held, held_usage) = match held_summary.take() {
                        Some((prior, prior_usage)) if held_is_tail => {
                            messages.pop();
                            (
                                merge_summaries(prior, assistant),
                                combine_usage(prior_usage, usage),
                            )
                        }
                        Some((prior, prior_usage)) => {
                            record_usage(&runtime.session, depth, &prior, prior_usage);
                            (assistant, usage)
                        }
                        None => (assistant, usage),
                    };
                    messages.push(held.clone());
                    if let Some(context) = response_context {
                        context_tokens = context.prompt + context.output;
                        measured_at = messages.len();
                    }
                    held_summary = Some((held, held_usage));
                    held_is_tail = true;
                    verification_reminder = Some(reminder);
                    continue;
                }
            }

            // No internal re-check is keeping the turn open. Recheck both
            // queues and close them under the same locks so a last-moment
            // sender is either included or reliably rejected.
            if !queued {
                let (late_steering, late_follow_ups) = runtime.steering.drain_at_response(
                    &runtime.follow_ups,
                    runtime.steering_mode,
                    runtime.follow_up_mode,
                    true,
                );
                steered.extend(late_steering);
                follow_ups.extend(late_follow_ups);
                queued = !steered.is_empty() || !follow_ups.is_empty();
            }

            // The answer is final: commit it, merged with a held summary when
            // the two are still adjacent and no queued message intervenes — a
            // queued message means the re-check answered a run that is not
            // finishing, so the summary stays its own turn ahead of it.
            let (assistant, usage) = match held_summary.take() {
                Some((prior, prior_usage)) if recheck_extends_summary(held_is_tail, queued) => {
                    messages.pop();
                    (
                        merge_summaries(prior, assistant),
                        combine_usage(prior_usage, usage),
                    )
                }
                Some((prior, prior_usage)) => {
                    record_usage(&runtime.session, depth, &prior, prior_usage);
                    send_usage(&tx, depth, prior_usage);
                    (assistant, usage)
                }
                None => (assistant, usage),
            };
            held_is_tail = false;
            record_usage(&runtime.session, depth, &assistant, usage);
            messages.push(assistant);
            if let Some(context) = response_context {
                context_tokens = context.prompt + context.output;
                measured_at = messages.len();
            }
            send_usage(&tx, depth, usage);
            let _ = tx.send(AgentEvent::ThoughtDone {
                millis: started.elapsed().as_millis() as u64,
            });
            if !queued {
                // Nothing is queued and no re-check is due, so the run is done.
                finish(&runtime, &tx, messages, depth);
                return;
            }
            mid_run_input_reminder = true;
            for queued in steered {
                let Queued { message, label } = queued;
                let _ = tx.send(AgentEvent::Steered {
                    message: message.clone(),
                    label,
                    follow_up: false,
                });
                verification.request_replies(&message.display().unwrap_or_default());
                record(&runtime.session, depth, &message);
                messages.push(message);
            }
            for queued in follow_ups {
                let Queued { message, label } = queued;
                let _ = tx.send(AgentEvent::Steered {
                    message: message.clone(),
                    label,
                    follow_up: true,
                });
                verification.request_replies(&message.display().unwrap_or_default());
                record(&runtime.session, depth, &message);
                messages.push(message);
            }
            workspace_context = project_context(&cwd, &messages);
            // Only one queued message precedes the next model call under
            // `one-at-a-time`; the poll resumes after that response.
            skip_steering_poll = true;
            continue;
        }

        // A held summary is committed before the turn that follows it, so its
        // text and usage are not lost when the re-check used a tool.
        if let Some((prior, prior_usage)) = held_summary.take() {
            record_usage(&runtime.session, depth, &prior, prior_usage);
            send_usage(&tx, depth, prior_usage);
        }
        held_is_tail = false;
        record_usage(&runtime.session, depth, &assistant, usage);
        messages.push(assistant);
        if let Some(context) = response_context {
            context_tokens = context.prompt + context.output;
            measured_at = messages.len();
        }
        send_usage(&tx, depth, usage);
        let _ = tx.send(AgentEvent::ThoughtDone {
            millis: started.elapsed().as_millis() as u64,
        });

        // Cancelled after the model planned tools: record a result for each
        // pending call so the session stays a valid call/result sequence, then
        // stop without running them.
        if runtime.cancel.is_cancelled() {
            for call in &tool_calls {
                let _ = tx.send(AgentEvent::ToolResult {
                    name: call.function.name.clone(),
                    args: call.function.arguments.clone(),
                    output: "cancelled by the user".to_string(),
                    is_error: true,
                    diff: None,
                    millis: 0,
                });
                let message =
                    Message::tool(call.id.clone(), "error: cancelled by the user".to_string());
                verification.request_replies(&message.display().unwrap_or_default());
                record(&runtime.session, depth, &message);
                messages.push(message);
            }
            finish(&runtime, &tx, messages, depth);
            return;
        }

        let mut terminated: Vec<bool> = Vec::with_capacity(tool_calls.len());
        let mut snapshot_needed = depth == 0 && runtime.plugins.is_active();

        // Dispatch the batch in the model's requested order, but run each maximal
        // run of concurrency-safe calls together. A state-changing call always gets
        // its own run, so read/write dependencies keep their order while a mixed
        // batch still parallelizes its reads.
        for tool_calls in batch_runs(tool_calls) {
            let parallel = tool_calls.len() > 1
                && tool_calls
                    .iter()
                    .all(|call| concurrency_safe(&call.function.name));

            if parallel {
                enum Prepared {
                    Immediate(tools::ToolOutput),
                    Run { call: ToolCall, args: Value },
                }

                let mut prepared = Vec::with_capacity(tool_calls.len());
                let mut executed_arguments = Vec::with_capacity(tool_calls.len());
                for original in &tool_calls {
                    let name = original.function.name.clone();
                    let _ = tx.send(AgentEvent::ToolCall {
                        name: name.clone(),
                        args: original.function.arguments.clone(),
                    });

                    let mut call = original.clone();
                    let args = serde_json::from_str::<Value>(&call.function.arguments)
                        .unwrap_or(Value::Null);
                    let effective_args = match runtime
                        .cancel
                        .until(runtime.plugins.tool_before(&name, &args))
                        .await
                        .flatten()
                    {
                        Some(mutated) if mutated != args => {
                            if let Ok(text) = serde_json::to_string(&mutated) {
                                call.function.arguments = text;
                            }
                            mutated
                        }
                        _ => args,
                    };
                    executed_arguments.push(call.function.arguments.clone());
                    let subject = subject_for(&name, &effective_args);
                    prepared.push(
                        if runtime
                            .cancel
                            .until(permission_granted(
                                permissions.decide(&name, &subject),
                                config.auto_approve,
                                &runtime.approve,
                                &name,
                                &subject,
                            ))
                            .await
                            .unwrap_or(false)
                        {
                            Prepared::Run {
                                call,
                                args: effective_args,
                            }
                        } else {
                            Prepared::Immediate(tools::ToolOutput::error(format!(
                                "permission denied for `{name}`"
                            )))
                        },
                    );
                }

                let mut handles = Vec::with_capacity(prepared.len());
                for item in prepared {
                    let read_reuse = Arc::clone(&read_reuse);
                    let config = config.clone();
                    let cwd = cwd.clone();
                    let runtime = runtime.clone();
                    let tx = tx.clone();
                    handles.push(tokio::spawn(async move {
                        match item {
                            Prepared::Immediate(output) => (output, 0),
                            Prepared::Run { call, args } => {
                                let name = call.function.name.clone();
                                let progress = tools::Progress::new(Arc::new({
                                    let tx = tx.clone();
                                    let name = name.clone();
                                    move |chunk: &str| {
                                        let _ = tx.send(AgentEvent::ToolProgress {
                                            name: name.clone(),
                                            chunk: chunk.to_string(),
                                        });
                                    }
                                }));
                                let started = std::time::Instant::now();
                                let mut output = dispatch_reusing_read(
                                    &config,
                                    &cwd,
                                    &runtime,
                                    &tx,
                                    &call,
                                    depth,
                                    &progress,
                                    &read_reuse,
                                )
                                .await;
                                if let Some(result) = runtime
                                    .cancel
                                    .until(runtime.plugins.tool_after(
                                        &name,
                                        &args,
                                        &output.text,
                                        output.is_error,
                                    ))
                                    .await
                                    .flatten()
                                {
                                    output.text = result.output;
                                    if let Some(is_error) = result.is_error {
                                        output.is_error = is_error;
                                    }
                                    output.terminate |= result.terminate;
                                }
                                (output, started.elapsed().as_millis() as u64)
                            }
                        }
                    }));
                }

                for ((handle, original), arguments) in handles
                    .into_iter()
                    .zip(&tool_calls)
                    .zip(&executed_arguments)
                {
                    let (mut output, millis) = match handle.await {
                        Ok(result) => result,
                        Err(_) => (
                            tools::ToolOutput::error(format!(
                                "{} stopped before reporting its result; completion is unknown. \
                                 Inspect the target state before deciding whether to retry",
                                original.function.name
                            )),
                            0,
                        ),
                    };
                    failure_loop.observe(&original.function.name, arguments, &output);
                    output.text = tools::truncate(&original.function.name, output.text);
                    omit_duplicate_media(&mut output, &mut seen_media);
                    let args = serde_json::from_str::<Value>(&original.function.arguments)
                        .unwrap_or(Value::Null);
                    verification.record_result(
                        &original.function.name,
                        &args,
                        &output.text,
                        output.is_error,
                    );
                    terminated.push(output.terminate);
                    let _ = tx.send(AgentEvent::ToolResult {
                        name: original.function.name.clone(),
                        args: original.function.arguments.clone(),
                        output: output.text.clone(),
                        is_error: output.is_error,
                        diff: output.diff.clone(),
                        millis,
                    });
                    let model_text = output.model_text();
                    let tool_message = if output.media.is_empty() {
                        Message::tool(original.id.clone(), model_text)
                    } else {
                        Message::tool_parts(original.id.clone(), model_text, output.media)
                    };
                    record(&runtime.session, depth, &tool_message);
                    messages.push(tool_message);
                }
            } else {
                for original in &tool_calls {
                    let name = original.function.name.clone();
                    let _ = tx.send(AgentEvent::ToolCall {
                        name: name.clone(),
                        args: original.function.arguments.clone(),
                    });

                    let mut call = original.clone();
                    let args = serde_json::from_str::<Value>(&call.function.arguments)
                        .unwrap_or(Value::Null);
                    let effective_args = match runtime
                        .cancel
                        .until(runtime.plugins.tool_before(&name, &args))
                        .await
                        .flatten()
                    {
                        Some(mutated) if mutated != args => {
                            if let Ok(text) = serde_json::to_string(&mutated) {
                                call.function.arguments = text;
                            }
                            mutated
                        }
                        _ => args,
                    };

                    let subject = subject_for(&name, &effective_args);
                    let progress = tools::Progress::new(Arc::new({
                        let tx = tx.clone();
                        let name = name.clone();
                        move |chunk: &str| {
                            let _ = tx.send(AgentEvent::ToolProgress {
                                name: name.clone(),
                                chunk: chunk.to_string(),
                            });
                        }
                    }));
                    let started = std::time::Instant::now();
                    let canonical = name.as_str();
                    let misplaced_reply = canonical == "bash"
                        && effective_args
                            .get("command")
                            .and_then(Value::as_str)
                            .is_some_and(|command| verification.blocks_summary_reply(command));
                    let block_reply = if canonical == "bash" {
                        match effective_args.get("command").and_then(Value::as_str) {
                            Some(command) if posts_review_reply(command) => {
                                let repo_pending = repo_has_pending_delivery(&cwd).await;
                                verification.blocks_review_reply(command, repo_pending)
                            }
                            _ => false,
                        }
                    } else {
                        false
                    };
                    let mut dispatched = false;
                    let mut output = if misplaced_reply {
                        tools::ToolOutput::error(
                            if verification.summaries_forbidden {
                                "The user asked not to post a top-level summary. Reply only in the requested review threads."
                            } else {
                            "The user requested inline review replies. `gh pr comment` creates a \
                             top-level summary and does not reply to the review threads. List \
                             `gh api --paginate repos/{owner}/{repo}/pulls/<n>/comments`, then post \
                             each addressed reply to its root comment's `/replies` endpoint and \
                             read it back. Complete those replies before adding a summary."
                            }
                                .to_string(),
                        )
                    } else if block_reply {
                        tools::ToolOutput::error(
                            "commit and push the code changes before replying to the review, so \
                         the reply does not claim a fix that is not on the branch under review"
                                .to_string(),
                        )
                    } else if runtime
                        .cancel
                        .until(permission_granted(
                            permissions.decide(&name, &subject),
                            config.auto_approve,
                            &runtime.approve,
                            &name,
                            &subject,
                        ))
                        .await
                        .unwrap_or(false)
                    {
                        snapshot_needed |= tool_may_mutate_workspace(&name);
                        dispatched = true;
                        dispatch_reusing_read(
                            &config,
                            &cwd,
                            &runtime,
                            &tx,
                            &call,
                            depth,
                            &progress,
                            &read_reuse,
                        )
                        .await
                    } else {
                        tools::ToolOutput::error(format!("permission denied for `{name}`"))
                    };
                    let canonical_name = canonical;
                    if matches!(canonical_name, "write" | "edit") && !output.is_error {
                        if let Some(path) = effective_args.get("path").and_then(Value::as_str) {
                            if let Some(diagnostics) = runtime
                                .cancel
                                .until(runtime.lsp.diagnostics_if_running(&cwd, Path::new(path)))
                                .await
                                .flatten()
                            {
                                output.text.push_str("\n\n");
                                output.text.push_str(&diagnostics);
                            }
                        }
                    }
                    if let Some(result) = runtime
                        .cancel
                        .until(runtime.plugins.tool_after(
                            &name,
                            &effective_args,
                            &output.text,
                            output.is_error,
                        ))
                        .await
                        .flatten()
                    {
                        output.text = result.output;
                        if let Some(is_error) = result.is_error {
                            output.is_error = is_error;
                        }
                        output.terminate |= result.terminate;
                    }
                    failure_loop.observe(&name, &call.function.arguments, &output);
                    output.text = tools::truncate(&name, output.text);
                    omit_duplicate_media(&mut output, &mut seen_media);
                    let millis = started.elapsed().as_millis() as u64;
                    verification.record_result(
                        &name,
                        &effective_args,
                        &output.text,
                        output.is_error,
                    );
                    // An edit invalidates a verifier result: the next run of the same
                    // build/test is a fresh check, not a repeat, so it must not get the
                    // "already ran" note. A failed edit left the workspace unchanged, so
                    // it must not clear the tracking.
                    if mutation_invalidates_verifier_result(
                        canonical_name,
                        &output.text,
                        output.is_error,
                    ) {
                        seen_verifications.clear();
                    }
                    if canonical_name == "bash" {
                        if let Some(command) = effective_args.get("command").and_then(Value::as_str)
                        {
                            if note_verifier(&mut seen_verifications, dispatched, command) {
                                output.text.push_str(
                                "\n\n[note: this build or test already ran earlier in this run. \
                                 Reuse its result only if its inputs have not changed, including \
                                 changes made by shell commands or other tools.]",
                            );
                            }
                        }
                    }
                    terminated.push(output.terminate);
                    let text = output.text.clone();

                    let _ = tx.send(AgentEvent::ToolResult {
                        name,
                        args: serde_json::to_string(&effective_args).unwrap_or_default(),
                        output: text.clone(),
                        is_error: output.is_error,
                        diff: output.diff.clone(),
                        millis,
                    });
                    let model_text = output.model_text();
                    let tool_message = if output.media.is_empty() {
                        Message::tool(call.id.clone(), model_text)
                    } else {
                        Message::tool_parts(call.id.clone(), model_text, output.media)
                    };
                    record(&runtime.session, depth, &tool_message);
                    messages.push(tool_message);
                }
            }
        }

        if snapshot_needed {
            if let Some(snapshots) = &runtime.snapshots {
                // `git add -A` is synchronous and can take a while on a large
                // tree; keep it off the async worker so the TUI stays live.
                let snapshots = Arc::clone(snapshots);
                let _ = tokio::task::spawn_blocking(move || snapshots.commit("turn")).await;
            }
        }

        if let Some(reason) = failure_loop.blocked.take() {
            let _ = tx.send(AgentEvent::Error(reason));
            finish(&runtime, &tx, messages, depth);
            return;
        }
        if batch_terminates(&terminated) {
            finish(&runtime, &tx, messages, depth);
            return;
        }
    }
}

fn budget_checkpoint(
    runtime: &Runtime,
    tx: &UnboundedSender<AgentEvent>,
    messages: &mut Vec<Message>,
    depth: usize,
    reason: &crate::efficiency::BudgetExceeded,
) {
    let message = Message::assistant(reason.to_string(), vec![]);
    record(&runtime.session, depth, &message);
    let _ = tx.send(AgentEvent::Text(reason.to_string()));
    messages.push(message);
}

#[allow(clippy::too_many_arguments)]
async fn dispatch_reusing_read(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    call: &ToolCall,
    depth: usize,
    progress: &tools::Progress,
    reuse: &Arc<Mutex<crate::efficiency::ReadReuse>>,
) -> tools::ToolOutput {
    let name = call.function.name.as_str();
    if runtime.cancel.is_cancelled() {
        return tools::ToolOutput::error("cancelled by the user");
    }
    // A mutating tool does not invalidate this memory: `ReadVersion` carries a
    // digest of the file's current bytes, so reuse is offered only when the
    // file still holds exactly what the earlier read returned. Dropping every
    // entry after each `edit`, `write` or `bash` call instead sent the same
    // unchanged file back in full whenever a run interleaved reads with work.
    let args: Value = serde_json::from_str(&call.function.arguments).unwrap_or(Value::Null);
    let cacheable =
        name == "read" && tool_enabled(config, name) && config.token_efficiency.reuse_reads;
    let capture = || {
        let cwd = cwd.to_path_buf();
        let arguments = call.function.arguments.clone();
        tokio::task::spawn_blocking(move || {
            crate::efficiency::ReadVersion::capture(&cwd, &arguments)
        })
    };
    let before = if cacheable {
        capture().await.ok().flatten()
    } else {
        None
    };
    if args.get("fresh").and_then(Value::as_bool) != Some(true) {
        if let Some(previous) = before.as_ref().and_then(|version| {
            reuse
                .lock()
                .expect("read reuse poisoned")
                .previous(version)
                .map(str::to_string)
        }) {
            return tools::ToolOutput::text(format!(
                "Unchanged file and range; reuse read result `{previous}` already in context. Set fresh: true to retrieve the full content again."
            ));
        }
    }
    let output = dispatch(config, cwd, runtime, events, call, depth, progress).await;
    if !output.is_error && output.media.is_empty() {
        if let Some(version) = before {
            if capture().await.ok().flatten().as_ref() == Some(&version) {
                reuse
                    .lock()
                    .expect("read reuse poisoned")
                    .remember(version, &call.id);
            }
        }
    }
    output
}

/// Only identical failures count; successful polling never exhausts this guard.
#[derive(Default)]
struct FailureLoop {
    failures: BTreeMap<(String, String), ([u8; 32], usize)>,
    blocked: Option<String>,
}

impl FailureLoop {
    fn observe(&mut self, name: &str, arguments: &str, output: &tools::ToolOutput) {
        let normalized = serde_json::from_str::<Value>(arguments)
            .ok()
            .and_then(|value| serde_json::to_string(&value).ok())
            .unwrap_or_else(|| arguments.to_string());
        let key = (name.to_string(), normalized);
        if !output_failed_with_state(&output.text, output.is_error) {
            // A successful action may have repaired the inputs of an earlier failure.
            if tool_may_mutate_workspace(name) {
                self.failures.clear();
            } else {
                self.failures.remove(&key);
            }
            return;
        }
        let digest: [u8; 32] = Sha256::digest(output.text.as_bytes()).into();
        if self.failures.len() >= 128 && !self.failures.contains_key(&key) {
            self.failures.clear();
        }
        let entry = self.failures.entry(key).or_insert((digest, 0));
        if entry.0 != digest {
            *entry = (digest, 0);
        }
        entry.1 += 1;
        if entry.1 >= 3 {
            self.blocked = Some(format!(
                "Stopped repeated tool failures: `{name}` returned the same failure three times \
                 with unchanged arguments. Resolve the failure before starting another turn."
            ));
        }
    }
}

/// Fingerprints media by its actual payload rather than by the path used to
/// read it. The same file can be named through an absolute path, a relative
/// path, or a symlink, and none of those should buy another copy of its base64
/// data in the model context.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum MediaKind {
    Image,
    File,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
struct MediaFingerprint {
    kind: MediaKind,
    digest: [u8; 32],
}

fn media_fingerprint(part: &ContentPart) -> Option<MediaFingerprint> {
    let (kind, payload) = match part {
        ContentPart::ImageUrl { image_url } => (MediaKind::Image, image_url.url.as_str()),
        ContentPart::File { file } => (MediaKind::File, file.file_data.as_str()),
        ContentPart::Text { .. } => return None,
    };
    Some(MediaFingerprint {
        kind,
        digest: Sha256::digest(payload.as_bytes()).into(),
    })
}

/// Removes duplicate media from stored tool results before constructing the
/// next provider request. Session logs written by older Oxide versions can
/// already contain several copies of one screenshot; cleaning the active view
/// makes those sessions recover immediately without rewriting their history.
/// User attachments keep their original placement even when repeated because
/// attaching the same artifact to a later instruction can itself be meaningful.
fn deduplicate_tool_media(messages: &mut [Message]) -> HashSet<MediaFingerprint> {
    let mut seen = HashSet::new();
    for message in messages {
        let Some(MessageContent::Parts(parts)) = message.content.as_mut() else {
            continue;
        };
        if message.role == "tool" {
            parts.retain(|part| {
                media_fingerprint(part).is_none_or(|fingerprint| seen.insert(fingerprint))
            });
        } else {
            seen.extend(parts.iter().filter_map(media_fingerprint));
        }
    }
    seen
}

/// Keeps the first copy of each media payload and turns later copies into a
/// short, actionable result. The original text result remains intact so tool
/// cards still say which path was read; only the expensive duplicate part is
/// removed from the provider request and session log.
fn omit_duplicate_media(output: &mut tools::ToolOutput, seen: &mut HashSet<MediaFingerprint>) {
    let before = output.media.len();
    output
        .media
        .retain(|part| media_fingerprint(part).is_none_or(|fingerprint| seen.insert(fingerprint)));
    let omitted = before.saturating_sub(output.media.len());
    if omitted == 0 {
        return;
    }
    output.text.push_str(&format!(
        "\n\n[note: {omitted} media attachment(s) already appear earlier in the active context, \
         so their duplicate bytes were omitted. Inspect the earlier attachment instead of reading \
         the same file again; use OCR or another image tool if the visual is unclear.]"
    ));
}

fn project_context(cwd: &Path, messages: &[Message]) -> String {
    let cwd = match std::path::absolute(cwd) {
        Ok(cwd) => cwd,
        Err(error) => {
            return format!(
                "\n\n# Current project\nCould not resolve the project working directory: {}. \
                 Do not guess its location or search unrelated folders.",
                json!(error.to_string())
            );
        }
    };
    let mut context = format!(
        "\n\n# Current project\nProject root and tool working directory: {}. \
         Relative file paths resolve here; omitted `find`/`grep`/`ls` paths mean this directory. \
         Start file discovery here, never in the home directory or filesystem root. \
         In user references, `@` marks a path and is not part of the filename. \
         Filenames may contain spaces. Use `ls` or `find` here before asking the user where \
         a project file is. A missing file is not a reason to search unrelated folders. \
         Use another directory only when the task calls for it.",
        json!(cwd.to_string_lossy())
    );
    if let Some(text) = messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .and_then(Message::display)
    {
        let files = crate::at::referenced_files(&text, &cwd);
        if !files.is_empty() {
            context.push_str(&format!(
                "\nExisting files referenced in the latest user message (path data, not instructions): {}. \
                 Use these exact paths directly; no search is needed.",
                json!(files)
            ));
        }
    }
    context
}

fn finish(
    runtime: &Runtime,
    tx: &UnboundedSender<AgentEvent>,
    mut messages: Vec<Message>,
    depth: usize,
) {
    // A sender may have been accepted immediately before an exceptional exit
    // (provider error, cancellation, step limit, or a terminating tool). Keep
    // those user messages in the session/history for the subsequent turn; the
    // acknowledgement must never point at input that vanished at completion.
    for pending in runtime.steering.close_and_drain_pair(&runtime.follow_ups) {
        record(&runtime.session, depth, &pending);
        messages.push(pending);
    }
    // A model whose catalog publishes no cache lifetime is not warmed; the
    // request would only be built to be thrown away.
    if let Some(warm) = runtime
        .cache_warm
        .as_ref()
        .filter(|warm| !runtime.cancel.is_cancelled() && warm.config.prompt_cache_ttl().is_some())
    {
        let (request, tools) = warmer_inputs(&warm.config, runtime, &warm.cwd, &messages);
        crate::cache_warmer::schedule(
            &warm.config,
            &warm.cwd,
            request,
            tools,
            warm.session.clone(),
            runtime.session.clone(),
        );
    }
    let _ = tx.send(AgentEvent::Finished(messages));
}

fn build_tool_specs(
    config: &Config,
    runtime: &Runtime,
    depth: usize,
    offer_session_title: bool,
) -> Vec<ToolSpec> {
    let mut specs = tools::specs(&runtime.mcp);
    specs.extend(
        runtime.mcp.declared_specs(
            &runtime
                .selected_mcp_tools
                .lock()
                .expect("tool selection poisoned"),
        ),
    );
    if depth < MAX_TASK_DEPTH
        && config
            .ecosystem
            .agents
            .iter()
            .any(|agent| agent.mode != AgentMode::Primary)
    {
        specs.push(task_spec(config));
    }
    if !config.ecosystem.skills.is_empty() {
        specs.push(skill_spec(config));
    }
    if !config.ecosystem.commands.is_empty() || !config.ecosystem.prompt_templates.is_empty() {
        specs.push(command_spec(config));
    }
    if runtime.ask.is_some() {
        specs.push(ask_spec());
    }
    specs.push(memory_spec());
    specs.push(lsp_spec());
    specs.retain(|spec| tool_enabled(config, &spec.function.name));
    if offer_session_title && needs_session_title(runtime, depth) {
        specs.push(session_title_spec());
    }
    specs
}

/// The request shape a finished turn will send again: the system prompt
/// (without the one-off title offer or reminders), the conversation, and the
/// tool list. The cache warmer replays it while the reader is idle.
pub(crate) fn warmer_inputs(
    config: &Config,
    runtime: &Runtime,
    cwd: &Path,
    messages: &[Message],
) -> (Vec<Message>, Vec<ToolSpec>) {
    let mut system = config.compose_system_prompt();
    system.push_str(&project_context(cwd, messages));
    if let Some(notes) = runtime.mcp.selected_instructions(
        &runtime
            .selected_mcp_tools
            .lock()
            .expect("tool selection poisoned"),
    ) {
        system.push_str("\n\n");
        system.push_str(&notes);
    }
    let mut request = vec![Message::system(system)];
    request.extend(messages.iter().cloned());
    let tools = build_tool_specs(config, runtime, 0, needs_session_title(runtime, 0));
    (request, tools)
}

fn needs_session_title(runtime: &Runtime, depth: usize) -> bool {
    depth == 0
        && runtime
            .session
            .as_ref()
            .is_some_and(|session| session.name().is_none())
}

fn internal_tool(name: &str) -> bool {
    name == "session_title"
}

fn tool_enabled(config: &Config, name: &str) -> bool {
    if config.tool_filter.is_restrictive() && !config.tool_filter.permits(name) {
        return false;
    }
    let Some(tools) = config
        .active_agent
        .as_ref()
        .and_then(|agent| agent.tools.as_ref())
    else {
        return true;
    };
    tools.iter().any(|tool| tool == name)
}

/// A batch ends the turn when every tool result in it requested termination.
fn batch_terminates(terminated: &[bool]) -> bool {
    !terminated.is_empty() && terminated.iter().all(|value| *value)
}

async fn permission_granted(
    action: Action,
    auto_approve: bool,
    approve: &Approver,
    tool: &str,
    subject: &str,
) -> bool {
    action == Action::Allow || auto_approve || approve(tool.to_string(), subject.to_string()).await
}
/// Splits a tool batch into dispatch runs. A maximal run of concurrency-safe
/// calls becomes one run (dispatched together), while every other call becomes
/// its own run so state-changing calls stay sequential and keep their order.
fn batch_runs(calls: Vec<ToolCall>) -> Vec<Vec<ToolCall>> {
    let mut runs: Vec<Vec<ToolCall>> = Vec::new();
    let mut current: Vec<ToolCall> = Vec::new();
    let mut current_safe = false;
    for call in calls {
        let safe = concurrency_safe(&call.function.name);
        if !current.is_empty() && !(safe && current_safe) {
            runs.push(std::mem::take(&mut current));
        }
        current.push(call);
        current_safe = safe;
    }
    if !current.is_empty() {
        runs.push(current);
    }
    runs
}

/// Tools with no cross-call side effects can run concurrently when the model
/// batches several of them. Anything that mutates the workspace (`write`,
/// `edit`, `bash`), spawns work (`task`), or has unknown remote effects (MCP)
/// stays on the sequential path so result ordering and side effects are stable.
fn concurrency_safe(name: &str) -> bool {
    matches!(
        name,
        "read" | "ls" | "find" | "grep" | "webfetch" | "memory" | "skill" | "diagnostics"
    )
}

fn tool_may_mutate_workspace(name: &str) -> bool {
    matches!(name, "write" | "edit" | "bash" | "task") || name.contains("__")
}

/// A state-changing action that should be confirmed before a run reports
/// success, with the commands that count as confirming it.
struct DoneRule {
    key: &'static str,
    /// Substrings that mark a command as performing the action.
    actions: &'static [&'static str],
    /// Substrings that mark a command as confirming the outcome.
    checks: &'static [&'static str],
    /// What the model is told to run when the action is unconfirmed.
    reminder: &'static str,
}

/// Outcomes that outlive the workspace — beyond files and builds — which the
/// Definition of Done requires confirming. A rule is a no-op until its action
/// (and, after that, a check) appears in a shell command.
const DONE_RULES: &[DoneRule] = &[
    DoneRule {
        key: "pull request",
        actions: &[
            "gh pr create",
            "gh pr edit",
            "gh pr ready",
            "glab mr create",
        ],
        checks: &[
            "gh pr checks",
            "gh pr view",
            "gh pr status",
            "gh pr list",
            "gh run list",
            "gh run watch",
            "gh run view",
            "glab mr view",
            "glab ci status",
            "pulls",
        ],
        reminder: "You opened or updated a pull request but have not checked it. Run \
                   `gh pr checks <url>` and `gh pr view <url> --json \
                   state,mergeable,mergeStateStatus,reviewDecision,statusCheckRollup` (or the \
                   `glab` equivalents), fix and push again if a check failed, and report it as a \
                   link the user can click — `[#123](url)`, the URL from `gh pr view <url> \
                   --json url` — with its final state.",
    },
    DoneRule {
        key: "comment",
        actions: &[
            "gh pr comment",
            "gh pr review",
            "gh issue comment",
            "glab mr note",
            "glab issue note",
        ],
        checks: &[
            "gh pr view",
            "gh issue view",
            "glab mr view",
            "glab issue view",
            "comments",
        ],
        reminder: "You posted a comment or review but have not confirmed it appears. Read the \
                   thread back (`gh pr view <url> --comments` or `gh issue view <url> --comments`) \
                   and confirm the reply landed in the right place.",
    },
    DoneRule {
        key: "release",
        actions: &["gh release create", "gh release edit"],
        checks: &["gh release view", "gh release list"],
        reminder: "You created or edited a release but have not checked it. Confirm with \
                   `gh release view <tag>` and report the result.",
    },
    DoneRule {
        key: "deployment",
        actions: &[
            "terraform apply",
            "kubectl apply",
            "kubectl rollout restart",
            "helm upgrade",
            "npm publish",
            "cargo publish",
            "docker push",
            "fly deploy",
            "vercel deploy",
            "wrangler deploy",
            "serverless deploy",
            "sam deploy",
            "gcloud run deploy",
        ],
        checks: &[
            "terraform plan",
            "kubectl get",
            "kubectl rollout status",
            "kubectl describe",
            "helm status",
            "npm view",
            "cargo search",
            "docker inspect",
            "fly status",
            "vercel inspect",
            "wrangler deployments",
            "gcloud run services describe",
        ],
        reminder: "You changed deployed or published state but have not confirmed it. Query the \
                   resulting status (for example `kubectl rollout status`, `terraform plan`, \
                   `npm view <pkg> version`, `helm status`) and report it.",
    },
    DoneRule {
        key: "scope",
        actions: &[
            "git add -a",
            "git add --all",
            "git add .",
            "git add :/",
            "git stage -a",
            "git commit -a",
            "git commit --all",
        ],
        checks: &[
            "git status",
            "git diff --staged",
            "git diff --cached",
            "git diff --stat",
            "git diff --name-only",
            "git show",
            "git log --stat",
        ],
        reminder: "You staged or committed with a blanket flag. Review exactly what is included \
                   (`git status`, `git diff --staged`) and drop unrelated or local-only files — for \
                   example `.claude/settings.local.json`, `.idea/`, editor state, or build output \
                   — before you push or open a pull request.",
    },
];

/// Tracks work that must be confirmed before a run can report success: files
/// edited but not confirmed on disk, and side effects (pull requests, comments,
/// releases, deployments) performed but not checked. A model that tries to
/// finish anyway is asked to verify first.
#[derive(Default)]
struct VerificationState {
    /// The run's own directory, which a relative path a tool was given is
    /// resolved against.
    cwd: PathBuf,
    /// The directory a push from this run delivers: the repository the run
    /// works in, found the way git finds it, or the run's own directory when it
    /// is in none. An edit inside it is on the branch under review, whatever
    /// directory of the repository it lands in; an edit outside it
    /// (`/tmp/reply.md`, a report under the home directory, a scratch script)
    /// is on no branch at all and must not hold a reply. A state built through
    /// [`Default`] leaves it empty — every path starts with an empty one —
    /// which tracks every edit, the behavior of a caller with nothing to judge
    /// against.
    delivery: PathBuf,
    edited: BTreeSet<String>,
    pending: BTreeSet<&'static str>,
    nudged: bool,
    /// A pull request or review was opened, replied to, or updated, so any edit
    /// made afterwards is part of that external work and has to be delivered.
    reviewed: bool,
    /// Files were edited since the last successful `git push`. Reading the
    /// change back proves it is on disk but does not put it in the pull request.
    edited_since_push: bool,
    replies_requested: bool,
    summaries_forbidden: bool,
    review_comments_discovered: bool,
    requested_review_comments: Option<BTreeSet<u64>>,
    review_comments: BTreeSet<u64>,
    posted_replies: BTreeMap<u64, u64>,
    confirmed_replies: BTreeSet<u64>,
}

/// The directory a push from `cwd` delivers: the closest enclosing repository,
/// or `cwd` itself when there is none. This is the boundary
/// [`repo_has_pending_delivery`] reads with `git status`, so a file edited
/// beside the run's directory and one edited through a tracked tool are judged
/// by the same rule.
fn delivery_root(cwd: &Path) -> PathBuf {
    cwd.ancestors()
        .find(|dir| dir.join(".git").exists())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| cwd.to_path_buf())
}

impl VerificationState {
    fn blocks_summary_reply(&self, command: &str) -> bool {
        (self.summaries_forbidden || self.inline_replies_pending())
            && tools::shell_segments(command).into_iter().any(|segment| {
                let words = review_shell_words(segment);
                let Some(index) = tools::shell_command_index(&words) else {
                    return false;
                };
                let program = words[index]
                    .trim_matches(['\'', '"'])
                    .rsplit(['/', '\\'])
                    .next()
                    .unwrap_or("");
                matches!(program, "gh" | "gh.exe")
                    && words.get(index + 1..index + 3) == Some(["pr", "comment"].as_slice())
                    && !words[index + 3..]
                        .iter()
                        .any(|arg| matches!(*arg, "--help" | "-h"))
            })
    }

    fn reply_targets(&self) -> BTreeSet<u64> {
        let mut targets = self
            .requested_review_comments
            .as_ref()
            .unwrap_or(&self.review_comments)
            .clone();
        targets.extend(self.posted_replies.values());
        targets
    }

    fn inline_replies_pending(&self) -> bool {
        if !self.replies_requested {
            return false;
        }
        let targets = self.reply_targets();
        ((self.requested_review_comments.is_none() || targets.is_empty())
            && !self.review_comments_discovered)
            || !targets.is_subset(&self.confirmed_replies)
            || (self.requested_review_comments.is_some()
                && targets.is_empty()
                && !self.review_comments.is_empty())
    }

    fn request_replies(&mut self, text: &str) {
        let text = text
            .to_ascii_lowercase()
            .replace(" but ", ";")
            .replace(" and don't ", ";don't ")
            .replace(" and do not ", ";do not ");
        let ids = regex::Regex::new(
            r"(?:comments?(?:\s+ids?)?\s*#?\s*|discussion_r)(\d+(?:\s*(?:,|and)\s*#?\d+)*)",
        )
        .unwrap();
        let numbers = regex::Regex::new(r"\d+").unwrap();
        for clause in text.split([';', '\n']) {
            let negative = [
                "do not reply",
                "don't reply",
                "do not post",
                "don't post",
                "不用回复",
                "不要回复",
            ]
            .iter()
            .any(|phrase| clause.contains(phrase));
            let summary = clause.contains("summary") || clause.contains("top-level");
            if negative && summary {
                self.summaries_forbidden = true;
                continue;
            }
            let reply = ["reply", "replies", "respond", "回复"]
                .iter()
                .any(|word| clause.contains(word));
            if !reply {
                continue;
            }
            let inline = clause.contains("inline") || clause.contains("review thread");
            let general = !inline
                && ["general", "top-level", "discussion comment"]
                    .iter()
                    .any(|word| clause.contains(word));
            if general || negative {
                self.replies_requested = false;
                continue;
            }
            if !["comment", "review", "inline", "评论"]
                .iter()
                .any(|word| clause.contains(word))
            {
                continue;
            }
            self.replies_requested = true;
            self.nudged = false;
            let targets: BTreeSet<u64> = ids
                .captures_iter(clause)
                .flat_map(|capture| numbers.find_iter(capture.get(1).unwrap().as_str()))
                .filter_map(|number| number.as_str().parse().ok())
                .collect();
            let selected = !targets.is_empty()
                || ["only", "selected", "specific", "this comment"]
                    .iter()
                    .any(|word| clause.contains(word));
            self.requested_review_comments = selected.then_some(targets);
        }
    }

    fn record_review_comments(&mut self, command: &str, output: &str) {
        let calls = github_review_api_calls(command);
        if calls.len() != 1 {
            return;
        }
        let (endpoint, method) = calls[0];
        let posting = method.eq_ignore_ascii_case("POST") && endpoint.ends_with("/replies");
        if !posting && !method.eq_ignore_ascii_case("GET") {
            return;
        }
        let listing = !posting && endpoint.ends_with("/comments");
        // The shell suffix is not JSON; streaming also accepts paginated arrays
        // and the JSON objects a --jq filter emits one per line.
        let values = serde_json::Deserializer::from_str(output).into_iter::<Value>();
        for value in values {
            let Ok(value) = value else {
                break;
            };
            let comments = match &value {
                Value::Array(items) => {
                    if listing {
                        self.review_comments_discovered = true;
                    }
                    items.as_slice()
                }
                _ => std::slice::from_ref(&value),
            };
            for comment in comments {
                let Some(id) = comment.get("id").and_then(Value::as_u64) else {
                    continue;
                };
                if let Some(parent) = comment.get("in_reply_to_id").and_then(Value::as_u64) {
                    if posting {
                        self.posted_replies.insert(id, parent);
                    } else if self.posted_replies.get(&id) == Some(&parent) {
                        self.confirmed_replies.insert(parent);
                    }
                } else if !posting && comment.get("path").and_then(Value::as_str).is_some() {
                    self.review_comments.insert(id);
                    self.review_comments_discovered |= listing;
                }
            }
        }
    }

    /// Tracks the work of a run started in `cwd`, judging its edits against what
    /// a push from there delivers.
    fn new(cwd: &Path) -> Self {
        let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
        Self {
            delivery: delivery_root(&cwd),
            cwd,
            ..Self::default()
        }
    }

    /// Whether an edited path lies inside what a push from this run delivers.
    /// The boundary is the repository, not the run's own directory, because a
    /// push delivers the whole branch: an edit beside the run's directory is the
    /// same tracked modification `git status` reports to the other half of the
    /// reply guard. A relative path is the run's own, the way the file tools
    /// read it.
    fn inside_delivery(&self, path: &str) -> bool {
        let path = Path::new(path);
        let candidate = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        };
        // The delivered directory and the path a tool was handed can spell one
        // place differently — `/var` for `/private/var` on macOS, a symlinked
        // home — so an existing file is resolved before it is judged. A file the
        // run just wrote exists; one that does not keeps the name it was given.
        std::fs::canonicalize(&candidate)
            .unwrap_or(candidate)
            .starts_with(&self.delivery)
    }

    /// Records one tool result. A successful edit adds its path; reading,
    /// diffing or type-checking that path confirms it, and a build/test/lint
    /// command confirms everything at once. Side-effecting shell commands add a
    /// pending check that the matching rule's status command clears.
    fn record_result(&mut self, name: &str, args: &Value, output: &str, is_error: bool) {
        let canonical = name;
        let path = args.get("path").and_then(Value::as_str);
        match canonical {
            "write" | "edit" => {
                if !output_failed_with_state(output, is_error) {
                    // A write outside the repository the run works in is not on
                    // the branch under review, so it neither waits on a push nor
                    // is reported back as an unconfirmed edit of the project. A
                    // call with no path to judge is treated as an edit of it.
                    match path.filter(|path| !path.is_empty()) {
                        Some(path) if self.inside_delivery(path) => {
                            self.edited.insert(path.to_string());
                            self.edited_since_push = true;
                        }
                        Some(_) => {}
                        None => self.edited_since_push = true,
                    }
                }
            }
            "read" | "diagnostics" => {
                if !output_failed_with_state(output, is_error) {
                    if let Some(path) = path.filter(|path| !path.is_empty()) {
                        self.edited.retain(|edited| !same_path(edited, path));
                    }
                }
            }
            "bash" => {
                let raw = args.get("command").and_then(Value::as_str).unwrap_or("");
                // A build/test/lint run confirms every edit at once; a shell
                // inspection that names one file confirms just that file, which
                // is the same evidence the reminder asks for without spending
                // another whole turn on it.
                if looks_like_verification_command(raw) {
                    self.edited.clear();
                } else if !output_failed_with_state(output, is_error) {
                    self.edited
                        .retain(|edited| !inspected_by_shell(raw, output, edited));
                }
                let command = raw.to_ascii_lowercase();
                let failed = output_failed_with_state(output, is_error);
                if !failed {
                    self.record_review_comments(raw, output);
                }
                if !failed && runs_git_push(raw) {
                    self.edited_since_push = false;
                }
                for rule in DONE_RULES {
                    if !failed && rule.actions.iter().any(|action| command.contains(action)) {
                        self.pending.insert(rule.key);
                        if matches!(rule.key, "pull request" | "comment") {
                            self.reviewed = true;
                        }
                    }
                    // A status read that came back still-pending confirms
                    // nothing yet: `gh pr checks` exits 8 while CI runs, which
                    // `tools::bash` reports as a result rather than a failure,
                    // so the rule has to stay pending until an outcome lands.
                    if rule.checks.iter().any(|check| command.contains(check))
                        && !reports_pending_checks(output)
                    {
                        self.pending.remove(rule.key);
                    }
                }
                // The REST reply endpoint is not in the rule's actions because
                // a bare `/replies` substring is also a read; only a POST posts.
                if !failed && posts_review_reply(raw) {
                    self.pending.insert("comment");
                    self.reviewed = true;
                }
            }
            _ => {}
        }
    }

    #[cfg(test)]
    fn record(&mut self, name: &str, args: &Value, output: &str) {
        self.record_result(name, args, output, output.starts_with("error:"));
    }

    /// Whether a review reply must be held until the pending code changes are
    /// pushed. Replying that a comment is fixed before the branch has the fix
    /// is misleading, so the reply is refused with an instruction to push first.
    /// `repo_pending` covers edits made outside the tracked tools (a `sed -i`,
    /// a formatter, a script) and commits that were not pushed.
    fn blocks_review_reply(&mut self, command: &str, repo_pending: bool) -> bool {
        if posts_review_reply(command) && (self.edited_since_push || repo_pending) {
            // The reply is part of a review delivery: if the run stops here, the
            // finish reminder still has to ask for the push.
            self.reviewed = true;
            return true;
        }
        false
    }

    /// The reminder to send once when work is unconfirmed, or `None` when there
    /// is nothing to verify or it has already been sent.
    fn reminder(&mut self) -> Option<String> {
        if self.nudged {
            return None;
        }
        let mut parts = Vec::new();
        if self.inline_replies_pending() {
            let missing: Vec<_> = self
                .reply_targets()
                .difference(&self.confirmed_replies)
                .map(u64::to_string)
                .collect();
            parts.push(format!(
                    "The user requested inline review replies. {} List the review comments with \
                     `gh api --paginate repos/{{owner}}/{{repo}}/pulls/<n>/comments`, keeping numeric \
                     `id`, `path`, and `in_reply_to_id`. Fix and push before posting a reply to each \
                     addressed root comment's `/replies` endpoint, then read back the posted replies. \
                     A top-level PR summary is not an inline reply. If blocked or instructed not to \
                     post, report the remaining comments honestly instead of claiming all were replied to.",
                    if missing.is_empty() { "Discover the requested inline comment targets; do not expand the user's scope.".to_string() }
                    else { format!("Replies remain unconfirmed for root comment IDs: {}.", missing.join(", ")) }
            ));
        }
        if !self.edited.is_empty() {
            let files = self.edited.iter().cloned().collect::<Vec<_>>().join(", ");
            parts.push(format!(
                "You edited {files} but have not confirmed the change is on disk. Re-read the \
                 changed region (or `grep` for the new symbol) or run the build/tests."
            ));
        }
        for key in &self.pending {
            if let Some(rule) = DONE_RULES.iter().find(|rule| rule.key == *key) {
                parts.push(rule.reminder.to_string());
            }
        }
        // A review reply that ships a code change is not delivered until the
        // change is pushed: the pull request still shows the old code even
        // though the working tree is correct.
        if self.reviewed && self.edited_since_push {
            parts.push(
                "You addressed a pull request or review with code changes but have not pushed them, \
                 so the branch under review still has the old code. Commit with an explicit path \
                 list and `git push` (or the `glab` equivalent), then confirm the new commit is on \
                 the branch before you call the review addressed."
                    .to_string(),
            );
        }
        if parts.is_empty() {
            return None;
        }
        self.nudged = true;
        Some(format!(
            "# Definition of Done\nInternal verification reminder from Oxide, not a new user \
             message or correction. Preserve the user's intent and constraints; do not attribute \
             this reminder to the user or acknowledge it as their feedback. Before you finish: {} \
             An explicit instruction from the user \
             outranks these checks — if they said one is not needed, do what they asked and say \
             plainly what you did not verify instead of running it anyway.",
            parts.join(" ")
        ))
    }
}

/// Whether two tool paths refer to the same file, tolerating one side being
/// absolute and the other relative. Commands and arguments use whichever
/// separator the platform (or the user) chose, so compare on `/` regardless.
fn same_path(a: &str, b: &str) -> bool {
    let a = a.replace('\\', "/");
    let b = b.replace('\\', "/");
    let a = a.trim_end_matches('/');
    let b = b.trim_end_matches('/');
    a == b || a.ends_with(&format!("/{b}")) || b.ends_with(&format!("/{a}"))
}

/// Whether a status read answered that the checks are still running, which is
/// the note `tools::bash` appends to `gh pr checks`' documented pending exit.
/// Such a read confirms no outcome, so it must not clear a pending rule.
fn reports_pending_checks(output: &str) -> bool {
    output.contains(tools::PENDING_CHECKS_NOTE)
}

/// Whether a tool result reports failure explicitly or by a non-zero shell
/// exit code. Text is intentionally not inspected: wording is presentation,
/// not protocol.
fn output_failed_with_state(output: &str, is_error: bool) -> bool {
    is_error
        || output
            .lines()
            .rev()
            .find_map(|line| {
                line.strip_prefix("[exit: ")
                    .or_else(|| line.strip_prefix("[exit code: "))
            })
            .and_then(|rest| rest.strip_suffix(']'))
            .and_then(|code| code.trim().parse::<i32>().ok())
            .is_some_and(|code| code != 0)
}

#[cfg(test)]
fn output_failed(output: &str) -> bool {
    output_failed_with_state(output, output.starts_with("error:"))
}

/// Whether a shell command is the kind that checks work (`cargo test`,
/// `./gradlew build`, `npm run lint`, …) rather than formatting or something
/// unrelated like posting a comment. Tokens are matched exactly (plus
/// camel-case task names like `spotlessCheck`), so the runner (`gradle`, `npm`)
/// alone or a flag word like `latest` does not count.
fn looks_like_verification_command(command: &str) -> bool {
    const EXACT: &[&str] = &[
        "test",
        "tests",
        "check",
        "lint",
        "build",
        "compile",
        "clippy",
        "typecheck",
        "pytest",
        "verify",
        "vet",
        "tsc",
        "spec",
        "specs",
    ];
    const SUFFIXES: &[&str] = &[
        "Check",
        "Test",
        "Tests",
        "Build",
        "Lint",
        "Compile",
        "Verify",
        "Typecheck",
    ];
    let has_check_name = |value: &str| {
        value
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|token| {
                !token.is_empty()
                    && (EXACT.iter().any(|word| token.eq_ignore_ascii_case(word))
                        || SUFFIXES.iter().any(|suffix| token.ends_with(suffix)))
            })
    };
    tools::shell_segments(command).into_iter().any(|segment| {
        let words: Vec<_> = segment.split_whitespace().collect();
        let Some(index) = tools::shell_command_index(&words) else {
            return false;
        };
        let executable = words[index].trim_matches(['\'', '"']);
        let name = executable.rsplit(['/', '\\']).next().unwrap_or(executable);
        let name = name
            .strip_suffix(".exe")
            .or_else(|| name.strip_suffix(".cmd"))
            .or_else(|| name.strip_suffix(".bat"))
            .unwrap_or(name);
        let args = &words[index + 1..];
        match name {
            "cargo" | "npm" | "pnpm" | "yarn" | "bun" | "deno" | "go" | "dotnet" | "gradle"
            | "gradlew" | "mvn" | "mvnw" | "make" | "gmake" | "bazel" | "buck" | "sbt"
            | "swift" | "xcodebuild" | "meson" | "ninja" | "rake" | "bundle" | "tox" | "uv" => {
                args.iter().any(|arg| has_check_name(arg))
            }
            "pytest" | "ctest" | "tsc" | "eslint" => true,
            "ruff" => ruff_verifies(args),
            "node" | "nodejs" | "python" | "python3" | "bash" | "sh" => {
                interpreter_verifies(name, args, &has_check_name)
            }
            _ => executable.contains(['/', '\\']) && has_check_name(name),
        }
    })
}

fn ruff_verifies(args: &[&str]) -> bool {
    let options: Vec<_> = args
        .iter()
        .copied()
        .take_while(|arg| *arg != "--")
        .collect();
    if options.iter().any(|arg| {
        matches!(
            *arg,
            "--help" | "-h" | "--version" | "-V" | "--show-settings" | "--show-files"
        )
    }) {
        return false;
    }
    let mut index = 0;
    while let Some(arg) = options.get(index) {
        match *arg {
            "--config" => index += 2,
            arg if arg.starts_with('-') => index += 1,
            "check" => return true,
            "format" => return options[index + 1..].contains(&"--check"),
            _ => return false,
        }
    }
    false
}

fn interpreter_verifies(name: &str, args: &[&str], has_check_name: &impl Fn(&str) -> bool) -> bool {
    let node = matches!(name, "node" | "nodejs");
    let python = matches!(name, "python" | "python3");
    let mut check_mode = false;
    let mut index = 0;
    while let Some(arg) = args.get(index).copied() {
        if matches!(arg, "--help" | "-h" | "--version" | "-V") {
            return false;
        }
        if arg == "--" {
            return args.get(index + 1).is_some_and(|script| {
                check_mode || has_check_name(script.rsplit(['/', '\\']).next().unwrap_or(script))
            });
        }
        if !arg.starts_with('-') || arg == "-" {
            return check_mode || has_check_name(arg.rsplit(['/', '\\']).next().unwrap_or(arg));
        }
        if node {
            if matches!(arg, "--eval" | "--print")
                || arg.starts_with("--eval=")
                || arg.starts_with("--print=")
                || arg.starts_with("-e")
                || arg.starts_with("-p")
            {
                return false;
            }
            if matches!(arg, "--test" | "--check" | "-c") {
                check_mode = true;
            }
            if matches!(
                arg,
                "-r" | "--require"
                    | "--import"
                    | "--loader"
                    | "--experimental-loader"
                    | "-C"
                    | "--conditions"
                    | "--input-type"
                    | "--test-name-pattern"
                    | "--test-skip-pattern"
                    | "--test-reporter"
                    | "--test-reporter-destination"
                    | "--test-concurrency"
            ) {
                index += 1;
            }
        } else if python {
            if arg.starts_with("-c") {
                return false;
            }
            if arg == "-m" || arg.starts_with("-m") {
                let module = if arg == "-m" {
                    index += 1;
                    args.get(index).copied().unwrap_or("")
                } else {
                    &arg[2..]
                };
                return match module {
                    "ruff" => ruff_verifies(&args[index + 1..]),
                    "unittest" | "py_compile" | "compileall" => true,
                    _ => has_check_name(module),
                };
            }
            if matches!(arg, "-W" | "-X" | "--check-hash-based-pycs") {
                index += 1;
            }
        } else if !arg.starts_with("--") {
            // Shell short options can be combined: -ec still supplies inline
            // code, while -euo consumes the following shell-option name.
            if arg[1..].contains('c') {
                return false;
            }
            check_mode |= arg[1..].contains('n');
            if arg.ends_with(['o', 'O']) {
                index += 1;
            }
        }
        index += 1;
    }
    check_mode
}

/// Whether a shell command inspected the edited file itself — a `git diff`,
/// `git show`, or `grep`/`rg` naming that path as the file it reads — so the
/// change is known to be on disk. Only the paths a command names are confirmed,
/// unlike a build or test run, which confirms every edit at once.
fn inspected_by_shell(command: &str, output: &str, path: &str) -> bool {
    const SEARCHERS: &[&str] = &["grep", "rg"];
    const GIT_VIEWERS: &[&str] = &["diff", "show"];
    command.split(['\n', ';', '|', '&']).any(|segment| {
        let mut words = segment.split_whitespace();
        let Some(tool) = words
            .next()
            .map(|word| word.rsplit('/').next().unwrap_or(word))
        else {
            return false;
        };
        let arguments: Vec<&str> = words.map(|word| word.trim_matches(['\'', '"'])).collect();
        if SEARCHERS.contains(&tool) {
            // A searcher's first operand is the pattern, not a file: the path
            // counts only as a later file operand, so `grep -rn "a.rs" .`
            // matching a *mention* of the file does not confirm anything.
            return arguments
                .iter()
                .filter(|argument| !argument.starts_with('-'))
                .skip(1)
                .any(|argument| same_path(path, argument));
        }
        if tool == "git" && GIT_VIEWERS.iter().any(|viewer| arguments.contains(viewer)) {
            // An empty diff exits 0 while showing nothing, and `git show`
            // without a `--` operand reads the committed blob rather than the
            // working file, so require a hunk naming the path.
            let shown = output
                .lines()
                .filter(|line| !line.starts_with("[exit"))
                .collect::<Vec<_>>()
                .join("\n");
            return arguments.iter().any(|argument| same_path(path, argument))
                && (shown.contains("@@") || shown.contains(path));
        }
        false
    })
}

/// A short final paragraph ending in an announced action is a handoff to
/// another step, not an answer. One follow-through request is enough to avoid
/// silently stopping there without turning ambiguous prose into a retry loop.
fn announces_unfinished_work(text: &str) -> bool {
    let tail = text.trim_end().rsplit("\n\n").next().unwrap_or("").trim();
    if !tail.ends_with(':') || tail.len() > 600 || tail.starts_with(['>', '`', '"', '\'']) {
        return false;
    }
    let tail = tail.to_ascii_lowercase();
    let sentence = tail.rsplit(['.', '!', '?']).next().unwrap_or(&tail).trim();
    let sentence = ["now ", "now, ", "next ", "next, ", "then ", "then, "]
        .iter()
        .find_map(|prefix| sentence.strip_prefix(prefix))
        .unwrap_or(sentence);
    !sentence.starts_with("let me know")
        && ["let me ", "i'll ", "i will ", "i’ll ", "i am going to "]
            .iter()
            .any(|prefix| sentence.starts_with(prefix))
}

/// Whether a failed model call may end the run quietly. The Definition-of-Done
/// reminder is advisory — it is sent after the model already summarized its
/// work — so a provider that answers it with nothing leaves a complete answer
/// rather than an error for the user to read.
fn nudge_failed_quietly(nudging: bool, err: &anyhow::Error) -> bool {
    nudging && err.downcast_ref::<crate::llm::NoAnswer>().is_some()
}

/// Emits a step's usage. A summary held for its re-check defers this until the
/// run commits it, because the front-ends treat `usage` as a step boundary and
/// would paint the re-check as a second bubble.
fn send_usage(
    tx: &UnboundedSender<AgentEvent>,
    depth: usize,
    usage: Option<crate::compact::UsageRecord>,
) {
    if let Some(usage) = usage {
        let _ = tx.send(AgentEvent::Usage {
            model_calls: usage.model_calls.max(1),
            source: if depth == 0 {
                UsageSource::Main
            } else {
                UsageSource::Background
            },
            input: usage.input,
            output: usage.output,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            cost: usage.cost,
            context_tokens: usage.context.map(|context| context.prompt),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn record_background_usage(
    session: &Option<Arc<SessionLog>>,
    model_calls: u64,
    input: u64,
    output: u64,
    cache_read: u64,
    cache_write: u64,
    cost: f64,
) {
    if let Some(log) = session {
        if let Err(error) = log.append_background_usage(crate::compact::UsageRecord {
            model_calls,
            input,
            output,
            cache_read,
            cache_write,
            cost,
            context: None,
        }) {
            crate::notice::warn(format!("saving background usage: {error:#}"));
        }
    }
}

/// Whether the re-check answer extends the summary it was asked about. A queued
/// steering or follow-up message means the run is not finishing, so the summary
/// is committed as its own turn and the answer follows it instead — merging
/// across a message the user queued would reorder the thread.
fn recheck_extends_summary(held_is_tail: bool, queued: bool) -> bool {
    held_is_tail && !queued
}

/// Joins a summary with the answer its verification re-check produced, so a run
/// that re-checks its own work ends with one assistant turn rather than two
/// summaries saying the same thing. The reasoning blocks are joined too, since
/// the re-check's reasoning explains the final text.
fn merge_summaries(mut first: Message, second: Message) -> Message {
    let mut text = first.display().unwrap_or_default();
    let extra = second.display().unwrap_or_default();
    if !extra.trim().is_empty() {
        if !text.trim().is_empty() {
            text.push_str("\n\n");
        }
        text.push_str(extra.trim_end());
    }
    let mut thinking = first.thinking.take().unwrap_or_default();
    if let Some(more) = second.thinking {
        thinking.extend(more);
    }
    let merged = Message::assistant(text, Vec::new());
    if thinking.is_empty() {
        merged
    } else {
        merged.with_thinking(thinking)
    }
}

/// Sums the usage of a held summary and the re-check that extended it, so the
/// merged turn reports both requests on resume.
fn combine_usage(
    first: Option<crate::compact::UsageRecord>,
    second: Option<crate::compact::UsageRecord>,
) -> Option<crate::compact::UsageRecord> {
    match (first, second) {
        (Some(first), Some(second)) => Some(crate::compact::UsageRecord {
            model_calls: first.model_calls.max(1) + second.model_calls.max(1),
            input: first.input + second.input,
            output: first.output + second.output,
            cache_read: first.cache_read + second.cache_read,
            cache_write: first.cache_write + second.cache_write,
            cost: first.cost + second.cost,
            context: second.context.or(first.context),
        }),
        (Some(usage), None) | (None, Some(usage)) => Some(usage),
        (None, None) => None,
    }
}

/// Whether a shell command posts a reply or review comment on a pull/merge
/// request. Replying that a comment is fixed before the fix is on the branch is
/// misleading, so these commands are held until the pending edits are pushed.
/// Fields imply POST for `gh api`, unless an explicit method overrides it.
fn posts_review_reply(command: &str) -> bool {
    if DONE_RULES
        .iter()
        .find(|rule| rule.key == "comment")
        .is_some_and(|rule| {
            rule.actions
                .iter()
                .any(|action| command.to_ascii_lowercase().contains(action))
        })
    {
        return true;
    }
    github_review_api_calls(command)
        .iter()
        .any(|(endpoint, method)| {
            endpoint.ends_with("/replies") && method.eq_ignore_ascii_case("POST")
        })
}

fn github_review_api_calls(command: &str) -> Vec<(&str, &str)> {
    tools::shell_segments(command)
        .into_iter()
        .filter_map(|segment| {
            let words = review_shell_words(segment);
            let index = tools::shell_command_index(&words)?;
            let program = words[index].rsplit(['/', '\\']).next()?;
            if !matches!(program, "gh" | "gh.exe") || words.get(index + 1) != Some(&"api") {
                return None;
            }
            let mut method = None;
            let mut fields = false;
            let mut endpoint = None;
            let mut args = words[index + 2..].iter();
            while let Some(arg) = args.next() {
                if matches!(*arg, "-X" | "--method") {
                    method = args.next().copied();
                } else if let Some(value) = arg
                    .strip_prefix("--method=")
                    .or_else(|| arg.strip_prefix("-X"))
                {
                    method = Some(value);
                } else if matches!(*arg, "-f" | "-F" | "--field" | "--raw-field" | "--input") {
                    fields = true;
                    args.next();
                } else if arg.starts_with("--field=")
                    || arg.starts_with("--raw-field=")
                    || arg.starts_with("--input=")
                    || arg.starts_with("-f")
                    || arg.starts_with("-F")
                {
                    fields = true;
                } else if (arg.starts_with("repos/")
                    || arg.starts_with("/repos/")
                    || arg.starts_with("https://"))
                    && arg.contains("/pulls/")
                    && arg.contains("/comments")
                {
                    endpoint = Some(arg.split('?').next().unwrap_or(arg));
                }
            }
            Some((
                endpoint?,
                method.unwrap_or(if fields { "POST" } else { "GET" }),
            ))
        })
        .collect()
}

// A flag mentioned inside a quoted reply body is data, not a method override.
fn review_shell_words(command: &str) -> Vec<&str> {
    let mut words = Vec::new();
    let mut start = None;
    let mut quote = None;
    let mut escaped = false;
    for (index, ch) in command.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if start.is_none() {
            if ch.is_whitespace() {
                continue;
            }
            start = Some(index);
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            continue;
        }
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch.is_whitespace() => {
                words.push(command[start.take().unwrap()..index].trim_matches(['\'', '"']));
            }
            None => {}
        }
    }
    if let Some(start) = start {
        words.push(command[start..].trim_matches(['\'', '"']));
    }
    words
}

/// Whether a shell command runs `git push` as the invoked program (not, say,
/// `echo git push`) and is not a dry run. Clearing the delivery flag is only an
/// optimization: [`repo_has_pending_delivery`] is the authority.
fn runs_git_push(command: &str) -> bool {
    command.split(['\n', ';', '&', '|']).any(|segment| {
        if segment.contains("--dry-run") {
            return false;
        }
        let mut words = segment.split_whitespace();
        words
            .find(|word| !matches!(*word, "cd" | "env" | "sudo" | "time" | "nohup"))
            .is_some_and(|program| program == "git")
            && segment.contains(" push")
    })
}

/// Whether the repository has work that a review reply would falsely claim is
/// delivered. The tracked tools (`write`/`edit`) cover the common case, but
/// `bash` can edit files through `sed -i`, a formatter, or a script, so the
/// guard also inspects the repository: tracked modifications and staged files,
/// plus commits that are not on a remote-tracking branch. Untracked files are
/// ignored so unrelated new files (notes, reports) do not block a reply. The git
/// calls are async so a slow `git` never blocks the runtime.
async fn repo_has_pending_delivery(cwd: &Path) -> bool {
    if git_stdout(cwd, &["status", "--porcelain", "--untracked-files=no"])
        .await
        .is_some_and(|status| !status.is_empty())
    {
        return true;
    }
    let unpushed = git_stdout(cwd, &["rev-list", "--count", "HEAD", "--not", "--remotes"])
        .await
        .and_then(|count| count.parse::<u64>().ok())
        .is_some_and(|count| count > 0);
    if !unpushed {
        return false;
    }
    // A normal clone gives the current branch a remote-tracking ref, so the
    // non-zero count is trustworthy (the commit really is unpushed). A
    // single-branch clone does not — its fetch refspec maps only the default
    // branch, so `--remotes` reports a pushed feature branch as unpushed
    // forever. Ask the remote for the tip only in that case.
    if let Some((remote, branch)) = tracking_branch(cwd).await {
        let reference = format!("refs/remotes/{remote}/{branch}");
        if git_stdout(cwd, &["rev-parse", "--verify", "--quiet", &reference])
            .await
            .is_some()
        {
            return true;
        }
    }
    !remote_has_head(cwd).await
}

/// The remote and branch the current `HEAD` tracks, when configured. Falls back
/// to `origin` when the branch has no explicit remote.
async fn tracking_branch(cwd: &Path) -> Option<(String, String)> {
    let branch = git_stdout(cwd, &["symbolic-ref", "--short", "HEAD"]).await?;
    let remote = git_stdout(
        cwd,
        &["config", "--get", &format!("branch.{branch}.remote")],
    )
    .await
    .filter(|remote| !remote.is_empty())
    .unwrap_or_else(|| "origin".to_string());
    Some((remote, branch))
}

/// Runs `git` in `cwd` and returns trimmed stdout, or `None` on any failure.
/// `GIT_TERMINAL_PROMPT=0` keeps a credential prompt from hanging the agent.
async fn git_stdout(cwd: &Path, args: &[&str]) -> Option<String> {
    let output = tokio::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true)
        .output()
        .await
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Whether the current branch's `HEAD` is already delivered to the same-named
/// branch on its remote. Only consulted when the branch has no remote-tracking
/// ref, so it corrects the single-branch-clone blind spot without adding a
/// network round trip to the common case. A remote that cannot be reached is
/// treated as not delivered, preserving the guard.
async fn remote_has_head(cwd: &Path) -> bool {
    let Some((remote, branch)) = tracking_branch(cwd).await else {
        return false;
    };
    let Some(head) = git_stdout(cwd, &["rev-parse", "HEAD"]).await else {
        return false;
    };
    let Some(remote_sha) = ls_remote(cwd, &remote, &branch).await else {
        return false;
    };
    if remote_sha == head {
        return true;
    }
    // The remote branch may have advanced past this commit; `HEAD` is still
    // delivered if it is an ancestor of the remote tip. A tip that is behind
    // (`HEAD` not an ancestor) stays pending.
    git_stdout(cwd, &["merge-base", "--is-ancestor", &head, &remote_sha])
        .await
        .is_some()
}

/// Runs `git ls-remote --heads <remote> <branch>` and returns the branch tip, or
/// `None` when it is absent, unreachable, or slower than the timeout. The child
/// is spawned explicitly so a slow lookup is killed and reaped instead of being
/// left alive by a dropped `output()` future.
async fn ls_remote(cwd: &Path, remote: &str, branch: &str) -> Option<String> {
    use tokio::io::AsyncReadExt;
    let mut child = tokio::process::Command::new("git")
        .args(["ls-remote", "--heads", remote, branch])
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let mut text = String::new();
    let read = stdout.read_to_string(&mut text);
    match tokio::time::timeout(std::time::Duration::from_secs(5), read).await {
        Ok(Ok(_)) => {}
        _ => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            return None;
        }
    }
    match child.wait().await {
        Ok(status) if status.success() => text.split_whitespace().next().map(str::to_string),
        _ => None,
    }
}

/// The identity of a build/test invocation for repeat detection: the command
/// before its output plumbing, so `./gradlew test | tail -50` and
/// `./gradlew test | wc -l` count as the same run. Returns `None` for a command
/// that is not a verifier at all.
fn verification_key(command: &str) -> Option<String> {
    let base = match first_unquoted(command, '|') {
        Some(index) => &command[..index],
        None => command,
    };
    let base = base.trim().trim_end_matches("2>&1").trim();
    looks_like_verification_command(base).then(|| base.to_string())
}

/// The byte index of the first `needle` outside single or double quotes.
fn first_unquoted(command: &str, needle: char) -> Option<usize> {
    let mut quote: Option<char> = None;
    for (index, ch) in command.char_indices() {
        match quote {
            Some(open) if ch == open => quote = None,
            Some(_) => {}
            None if ch == '\'' || ch == '"' => quote = Some(ch),
            None if ch == needle => return Some(index),
            None => {}
        }
    }
    None
}

/// Records a verifier that actually ran and reports whether it had already run
/// this turn. A denied or disabled call never dispatched, so it must not be
/// recorded: approving and retrying it is a first run, not a repeat.
fn note_verifier(seen: &mut BTreeSet<String>, dispatched: bool, command: &str) -> bool {
    if !dispatched {
        return false;
    }
    match verification_key(command) {
        Some(key) => !seen.insert(key),
        None => false,
    }
}

/// Whether a tool result invalidates a build/test result seen earlier in the
/// run. Only a successful `write`/`edit` changes the workspace; a failed
/// one leaves it untouched, so the earlier verifier still applies.
fn mutation_invalidates_verifier_result(
    canonical_name: &str,
    output: &str,
    is_error: bool,
) -> bool {
    matches!(canonical_name, "write" | "edit") && !output_failed_with_state(output, is_error)
}

#[cfg(test)]
fn mutation_invalidates_verifier(canonical_name: &str, output: &str) -> bool {
    mutation_invalidates_verifier_result(canonical_name, output, output.starts_with("error:"))
}

fn bounded_selection(
    previous: &BTreeSet<String>,
    preferred: &[String],
    cap: usize,
) -> BTreeSet<String> {
    let mut selected = BTreeSet::new();
    for name in preferred.iter().chain(previous) {
        if selected.len() >= cap {
            break;
        }
        selected.insert(name.clone());
    }
    selected
}

async fn search_mcp_tools(
    config: &Config,
    runtime: &Runtime,
    arguments: &str,
    depth: usize,
) -> Result<String> {
    let args: Value = serde_json::from_str(arguments).context("invalid tool_search arguments")?;
    let query = args
        .get("query")
        .and_then(Value::as_str)
        .filter(|q| !q.trim().is_empty())
        .context("tool_search requires a non-empty query")?;
    let limit = match args.get("limit") {
        None => 8,
        Some(value) => value
            .as_u64()
            .filter(|n| *n > 0 && *n <= 64)
            .context("tool_search limit must be an integer between 1 and 64")?
            as usize,
    };
    runtime.mcp.discover().await;
    let errors = runtime.mcp.discovery_errors();
    let mut selected = runtime
        .selected_mcp_tools
        .lock()
        .expect("tool selection poisoned")
        .clone();
    let replace = match args.get("replace") {
        None => false,
        Some(value) => value
            .as_bool()
            .context("tool_search replace must be a boolean")?,
    };
    let previous = selected.clone();
    if replace {
        selected.clear();
    }
    let matches = runtime.mcp.search_tools(
        query,
        limit.min(config.token_efficiency.max_declared_mcp_tools),
        &selected,
        |name| tool_enabled(config, name),
    );
    let matching_names: Vec<String> = matches
        .iter()
        .map(|tool| tool.function.name.clone())
        .collect();
    selected = bounded_selection(
        &selected,
        &matching_names,
        config.token_efficiency.max_declared_mcp_tools,
    );
    if selected != previous {
        if depth == 0 {
            if let Some(log) = &runtime.session {
                log.append_tool_selection(selected.iter().cloned().collect())
                    .context("saving deferred tool selection")?;
            }
        }
        *runtime
            .selected_mcp_tools
            .lock()
            .expect("tool selection poisoned") = selected.clone();
    }
    let mut text = if matches.is_empty() {
        if replace {
            "No matching tools found; the previous schemas were deferred."
        } else {
            "No matching undeclared tools found. Previously discovered tools remain available."
        }
        .to_string()
    } else {
        let mut text = format!(
            "Loaded {} tool(s). They are available from your next call:",
            matches.len()
        );
        for tool in matches {
            let description = tool.function.description.lines().next().unwrap_or("");
            text.push_str(&format!(
                "\n- {}: {}",
                tool.function.name,
                description.chars().take(300).collect::<String>()
            ));
        }
        text
    };
    let deferred = previous.difference(&selected).count();
    if deferred > 0 {
        text.push_str(&format!("\n{deferred} previous tool schema(s) deferred to bound context. Search again to load them when needed."));
    }
    if !errors.is_empty() {
        text.push_str(
            "\n\nSome MCP servers could not be discovered. Their tools have not been searched:",
        );
        for error in errors {
            text.push_str(&format!(
                "\n- {}",
                error.chars().take(1000).collect::<String>()
            ));
        }
    }
    Ok(text)
}

async fn dispatch(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    call: &crate::llm::ToolCall,
    depth: usize,
    progress: &tools::Progress,
) -> tools::ToolOutput {
    if call.function.name == "session_title" {
        return set_session_title(runtime, events, &call.function.arguments, depth);
    }
    if !tool_enabled(config, &call.function.name) {
        return tools::ToolOutput::error(format!("tool `{}` is disabled", call.function.name));
    }
    if runtime.mcp.is_tool(&call.function.name)
        && !runtime
            .selected_mcp_tools
            .lock()
            .expect("tool selection poisoned")
            .contains(&call.function.name)
    {
        return tools::ToolOutput::error("This MCP tool is deferred. Use tool_search to discover and declare it before calling it.");
    }
    runtime
        .cancel
        .until(async {
            match call.function.name.as_str() {
                "task" => legacy_tool_output(
                    task(
                        config,
                        cwd,
                        runtime,
                        events,
                        &call.function.arguments,
                        depth,
                    )
                    .await,
                ),
                "skill" => legacy_tool_output(skill(config, &call.function.arguments)),
                "ask" => ask(runtime, &call.function.arguments).await,
                "command" => legacy_tool_output(
                    command(
                        config,
                        cwd,
                        runtime,
                        events,
                        &call.function.arguments,
                        depth,
                    )
                    .await,
                ),
                "memory" => legacy_tool_output(memory(config, &call.function.arguments)),
                "diagnostics" => lsp_diagnostics(runtime, cwd, &call.function.arguments).await,
                "tool_search" => {
                    match search_mcp_tools(config, runtime, &call.function.arguments, depth).await {
                        Ok(text) => tools::ToolOutput::text(text),
                        Err(error) => tools::ToolOutput::error(format!("{error:#}")),
                    }
                }
                _ => tools::execute(call, cwd, &runtime.mcp, progress).await,
            }
        })
        .await
        .unwrap_or_else(|| {
            tools::ToolOutput::error(
                "cancelled by the user; the operation may have partially completed. \
                 Inspect the target state before retrying"
                    .to_string(),
            )
        })
}

fn set_session_title(
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    arguments: &str,
    depth: usize,
) -> tools::ToolOutput {
    let Some(session) = runtime.session.as_ref().filter(|_| depth == 0) else {
        return tools::ToolOutput::error(
            "session titles are available only in a persisted primary conversation",
        );
    };
    if let Some(title) = session.name() {
        return tools::ToolOutput::text(format!("Session is already named `{title}`."));
    }
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let title = args
        .get("title")
        .and_then(Value::as_str)
        .map(|value| crate::title::generated(value, crate::title::TITLE_LIMIT))
        .unwrap_or_default();
    if title.is_empty() {
        return tools::ToolOutput::error("title must contain a short descriptive phrase");
    }
    if let Err(err) = session.set_name(&title) {
        return tools::ToolOutput::error(format!("could not save session title: {err:#}"));
    }
    let _ = events.send(AgentEvent::SessionTitle {
        title: title.clone(),
    });
    tools::ToolOutput::text(format!("Session title set to `{title}`."))
}

/// Older agent-level tools still return a string. Interpret their established
/// marker once at this boundary so every event beyond it has explicit state.
fn legacy_tool_output(text: String) -> tools::ToolOutput {
    if text.starts_with("error:") {
        tools::ToolOutput::error(text)
    } else {
        tools::ToolOutput::text(text)
    }
}

/// The `ask` tool: the model's questions reach the user through the runtime's
/// asker, and the answers come back as the tool result. A run with nobody to ask
/// is not offered the tool at all, so this reports the absence instead of
/// pretending the questions were asked.
async fn ask(runtime: &Runtime, arguments: &str) -> tools::ToolOutput {
    let questions = match ask::parse_questions(arguments) {
        Ok(questions) => questions,
        Err(err) => return tools::ToolOutput::error(format!("{err:#}")),
    };
    let Some(ask_user) = &runtime.ask else {
        return tools::ToolOutput::error(
            "no front-end is listening for a question in this run; continue with the safest \
             reasonable default",
        );
    };
    let reply: Option<Reply> = ask_user(questions.clone()).await;
    tools::ToolOutput::text(ask::format_reply(&questions, reply))
}

async fn lsp_diagnostics(runtime: &Runtime, cwd: &Path, arguments: &str) -> tools::ToolOutput {
    let args: Value = serde_json::from_str(arguments).unwrap_or(Value::Null);
    let Some(path) = args.get("path").and_then(Value::as_str) else {
        return tools::ToolOutput::error(
            "diagnostics requires a `path` string, for example {\"path\":\"src/main.rs\"}",
        );
    };
    match runtime.lsp.diagnostics(cwd, Path::new(path)).await {
        Some(diagnostics) => tools::ToolOutput::text(diagnostics),
        None => tools::ToolOutput::error(format!(
            "no language server is available for `{path}`; check that the file type is supported \
             and its language server is installed"
        )),
    }
}

fn record(session: &Option<Arc<SessionLog>>, depth: usize, message: &Message) {
    record_usage(session, depth, message, None);
}

fn record_usage(
    session: &Option<Arc<SessionLog>>,
    depth: usize,
    message: &Message,
    usage: Option<crate::compact::UsageRecord>,
) {
    if depth == 0 {
        if let Some(log) = session {
            let _ = log.append_with_usage(message, usage);
        }
    }
}

async fn task(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    arguments: &str,
    depth: usize,
) -> String {
    match task_inner(config, cwd, runtime, events, arguments, depth).await {
        Ok(output) => output,
        Err(err) => format!("error: {err:#}"),
    }
}

async fn task_inner(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    arguments: &str,
    depth: usize,
) -> Result<String> {
    let args: Value = serde_json::from_str(arguments).context("invalid task arguments")?;
    let prompt = args
        .get("prompt")
        .and_then(Value::as_str)
        .context("missing `prompt` argument")?;
    let name = args
        .get("subagent_type")
        .and_then(Value::as_str)
        .context("missing `subagent_type` argument")?;

    let agent = config.ecosystem.agent(name).cloned().with_context(|| {
        let available: Vec<&str> = config
            .ecosystem
            .agents
            .iter()
            .filter(|agent| agent.mode != AgentMode::Primary)
            .map(|agent| agent.name.as_str())
            .collect();
        format!(
            "unknown subagent `{name}` (available: {})",
            available.join(", ")
        )
    })?;
    if agent.mode == AgentMode::Primary {
        anyhow::bail!("agent `{name}` is primary and cannot be used as a subagent");
    }

    let agent_name = agent.name.clone();
    let mut sub = config.clone();
    sub.active_agent = Some(agent);

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let sub_runtime = Runtime {
        session: None,
        selected_mcp_tools: Default::default(),
        follow_ups: Steering::new(),
        cache_warm: None,
        ..runtime.clone()
    };
    let handle = tokio::spawn(run_depth(
        sub,
        cwd.to_path_buf(),
        vec![Message::user(prompt)],
        tx,
        sub_runtime,
        depth + 1,
    ));

    let mut output = String::new();
    let mut error = None;
    // Text before a tool call is committed; the current step's partial text is
    // dropped when the stream is retried, so it is not reported twice.
    let mut committed = 0usize;
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Text(delta) => output.push_str(&delta),
            AgentEvent::Error(message) => error = Some(message),
            AgentEvent::Finished(_) => break,
            // The subagent's own tool traffic feeds the caller's progress view;
            // only its final report becomes the tool result.
            AgentEvent::ToolCall { name, args } => {
                committed = output.len();
                let _ = events.send(AgentEvent::SubagentActivity {
                    agent: agent_name.clone(),
                    tool: name,
                    args,
                });
            }
            AgentEvent::Retrying { .. } => output.truncate(committed),
            // A subagent's approval prompt is surfaced as-is; the same broker
            // answers it, so the reply reaches the subagent that is waiting.
            AgentEvent::ApprovalRequest { id, tool, detail } => {
                let _ = events.send(AgentEvent::ApprovalRequest { id, tool, detail });
            }
            AgentEvent::QuestionRequest { id, questions } => {
                let _ = events.send(AgentEvent::QuestionRequest { id, questions });
            }
            AgentEvent::QuestionClosed { id } => {
                let _ = events.send(AgentEvent::QuestionClosed { id });
            }
            AgentEvent::SessionTitle { .. } => {}
            // A no-tool step commits here, not at the next tool call.
            AgentEvent::ThoughtDone { .. } => committed = output.len(),
            AgentEvent::Usage {
                model_calls,
                input,
                output,
                cache_read,
                cache_write,
                cost,
                ..
            } => {
                record_background_usage(
                    &runtime.session,
                    model_calls,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    cost,
                );
                let _ = events.send(AgentEvent::Usage {
                    model_calls,
                    source: UsageSource::Background,
                    input,
                    output,
                    cache_read,
                    cache_write,
                    cost,
                    context_tokens: None,
                });
            }
            AgentEvent::ToolProgress { .. }
            | AgentEvent::ToolResult { .. }
            | AgentEvent::Compaction { .. }
            | AgentEvent::ContextWindow { .. }
            | AgentEvent::Branch { .. }
            | AgentEvent::Steered { .. }
            | AgentEvent::SubagentActivity { .. }
            | AgentEvent::ThinkingDelta(_)
            | AgentEvent::Thought { .. } => {}
        }
    }
    let _ = handle.await;

    if output.trim().is_empty() {
        if let Some(error) = error {
            anyhow::bail!("subagent `{name}` failed: {error}");
        }
        return Ok(format!(
            "subagent `{name}` finished without producing a response"
        ));
    }
    Ok(output)
}

fn skill(config: &Config, arguments: &str) -> String {
    match skill_inner(config, arguments) {
        Ok(content) => content,
        Err(err) => format!("error: {err:#}"),
    }
}

fn skill_inner(config: &Config, arguments: &str) -> Result<String> {
    let args: Value = serde_json::from_str(arguments).context("invalid skill arguments")?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .context("missing `name` argument")?;
    match config
        .ecosystem
        .skills
        .iter()
        .find(|skill| skill.name == name)
    {
        Some(skill) => Ok(skill.content.clone()),
        None => {
            let available: Vec<&str> = config
                .ecosystem
                .skills
                .iter()
                .map(|skill| skill.name.as_str())
                .collect();
            anyhow::bail!(
                "unknown skill `{name}` (available: {})",
                available.join(", ")
            )
        }
    }
}

fn task_spec(config: &Config) -> ToolSpec {
    let subagents: Vec<String> = config
        .ecosystem
        .agents
        .iter()
        .filter(|agent| agent.mode != AgentMode::Primary)
        .map(|agent| match &agent.description {
            Some(description) if !description.is_empty() => {
                format!("{} ({description})", agent.name)
            }
            _ => agent.name.clone(),
        })
        .collect();
    let available = if subagents.is_empty() {
        "none".to_string()
    } else {
        subagents.join(", ")
    };

    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "task".to_string(),
            description: format!(
                "Launch a subagent to handle a focused task in its own context and return its final report. Available subagents: {available}."
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "description": {
                        "type": "string",
                        "description": "Short 3-5 word description of the task"
                    },
                    "prompt": {
                        "type": "string",
                        "description": "The task for the subagent to perform"
                    },
                    "subagent_type": {
                        "type": "string",
                        "description": "Name of the subagent to use"
                    }
                },
                "required": ["description", "prompt", "subagent_type"]
            }),
        },
    }
}

async fn command(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    arguments: &str,
    depth: usize,
) -> String {
    match command_inner(config, cwd, runtime, events, arguments, depth).await {
        Ok(output) => output,
        Err(err) => format!("error: {err:#}"),
    }
}

async fn command_inner(
    config: &Config,
    cwd: &Path,
    runtime: &Runtime,
    events: &UnboundedSender<AgentEvent>,
    arguments: &str,
    depth: usize,
) -> Result<String> {
    let args: Value = serde_json::from_str(arguments).context("invalid command arguments")?;
    let name = args
        .get("name")
        .and_then(Value::as_str)
        .context("missing `name` argument")?
        .trim_start_matches('/');
    let extra = args.get("arguments").and_then(Value::as_str).unwrap_or("");
    let input = if extra.trim().is_empty() {
        format!("/{name}")
    } else {
        format!("/{name} {extra}")
    };
    let Some(resolved) = config.ecosystem.resolve_command(&input) else {
        let available: Vec<&str> = config
            .ecosystem
            .commands
            .iter()
            .map(|command| command.name.as_str())
            .chain(
                config
                    .ecosystem
                    .prompt_templates
                    .iter()
                    .filter(|template| config.ecosystem.command(&template.name).is_none())
                    .map(|template| template.name.as_str()),
            )
            .collect();
        anyhow::bail!(
            "unknown command `{name}` (available: {})",
            available.join(", ")
        );
    };

    if resolved.subtask {
        let agent = resolved
            .agent
            .clone()
            .context("subtask command requires an `agent` in its frontmatter")?;
        let task_args = json!({ "prompt": resolved.prompt, "subagent_type": agent }).to_string();
        return task_inner(config, cwd, runtime, events, &task_args, depth).await;
    }
    if let Some(agent_name) = &resolved.agent {
        if let Some(agent) = config.ecosystem.agent(agent_name) {
            return Ok(format!(
                "Operate as the `{agent_name}` agent while carrying out the `/{name}` command.\n\n\
                 {}\n\n{}",
                agent.prompt.trim(),
                resolved.prompt
            ));
        }
    }
    Ok(resolved.prompt)
}

fn command_spec(config: &Config) -> ToolSpec {
    let available: Vec<String> = config
        .ecosystem
        .commands
        .iter()
        .map(|command| command.name.clone())
        .chain(
            config
                .ecosystem
                .prompt_templates
                .iter()
                .filter(|template| config.ecosystem.command(&template.name).is_none())
                .map(|template| template.name.clone()),
        )
        .collect();

    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "command".to_string(),
            description: format!(
                "Invoke a predefined command by name when the user's request matches its purpose. Subtask commands run in an isolated subagent and return their result; other commands return their expanded instructions for you to carry out. Available commands: {}.",
                available.join(", ")
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the command to invoke"
                    },
                    "arguments": {
                        "type": "string",
                        "description": "Arguments expanded into the command's $ARGUMENTS/$1 placeholders"
                    }
                },
                "required": ["name"]
            }),
        },
    }
}

fn skill_spec(config: &Config) -> ToolSpec {
    let available: Vec<&str> = config
        .ecosystem
        .skills
        .iter()
        .map(|skill| skill.name.as_str())
        .collect();

    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "skill".to_string(),
            description: format!(
                "Load the full instructions for a skill by name before doing work that matches it. Available skills: {}.",
                available.join(", ")
            ),
            parameters: json!({
                "type": "object",
                "properties": {
                    "name": {
                        "type": "string",
                        "description": "Name of the skill to load"
                    }
                },
                "required": ["name"]
            }),
        },
    }
}

fn ask_spec() -> ToolSpec {
    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "ask".to_string(),
            description: concat!(
                "Ask the user a question when only they can answer it — a choice between options, ",
                "or a detail you cannot infer from the repository. Use it instead of guessing, and ",
                "whenever a skill you are following says to confirm with the user. Give each ",
                "option a short `label` and, when the choice is not obvious, a `description`; set ",
                "`multiSelect` when any number of them may be chosen; every entry needs its own ",
                "`question` string (a choice's `label` is not the question). Leave `options` out to ask ",
                "for a typed answer. The answer comes back as this tool's result, and a question ",
                "nobody answers comes back unanswered. An unanswered or dismissed question is ",
                "not approval: if authorization is required, stop the dependent action."
            )
            .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "questions": {
                        "type": "array",
                        "description": "The questions to ask, at most 4, in the order they should be asked",
                        "items": {
                            "type": "object",
                            "properties": {
                                "question": {
                                    "type": "string",
                                    "description": "The question to ask, ending in a question mark"
                                },
                                "header": {
                                    "type": "string",
                                    "description": "A short label for the question, at most 12 characters"
                                },
                                "options": {
                                    "type": "array",
                                    "description": "The choices to offer, at most 8. Leave it out to ask for a typed answer",
                                    "items": {
                                        "type": "object",
                                        "properties": {
                                            "label": {
                                                "type": "string",
                                                "description": "The choice, one or two words"
                                            },
                                            "description": {
                                                "type": "string",
                                                "description": "What choosing it means"
                                            }
                                        },
                                        "required": ["label"]
                                    }
                                },
                                "multiSelect": {
                                    "type": "boolean",
                                    "description": "Let the user choose more than one option"
                                }
                            },
                            "required": ["question"]
                        }
                    }
                },
                "required": ["questions"]
            }),
        },
    }
}

fn session_title_spec() -> ToolSpec {
    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "session_title".to_string(),
            description: concat!(
                "Set the hidden display title for this conversation. Use a specific 3–7 word ",
                "task or outcome phrase; omit paths, URLs, and filler, and paraphrase rather than ",
                "copying the user's opening. Call once and never alone."
            )
            .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "A specific 3–7 word conversation title"
                    }
                },
                "required": ["title"],
                "additionalProperties": false
            }),
        },
    }
}

fn memory(config: &Config, arguments: &str) -> String {
    match memory_inner(config, arguments) {
        Ok(output) => output,
        Err(err) => format!("error: {err:#}"),
    }
}

fn memory_inner(config: &Config, arguments: &str) -> Result<String> {
    let args: Value = serde_json::from_str(arguments).context("invalid memory arguments")?;
    let mode = args
        .get("mode")
        .and_then(Value::as_str)
        .context("missing `mode` argument")?;
    let scope = QueryScope::parse(
        args.get("scope")
            .and_then(Value::as_str)
            .unwrap_or("project"),
    );

    match mode {
        "add" => {
            let content = args
                .get("content")
                .and_then(Value::as_str)
                .context("missing `content` argument")?;
            let tags = args
                .get("tags")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default();
            let entry = config.memory.add(content, scope.entry_scope()?, tags)?;
            Ok(format!("stored memory `{}`", entry.id))
        }
        "search" => {
            let query = args.get("query").and_then(Value::as_str).unwrap_or("");
            let results = config.memory.search(query, scope, limit_arg(&args));
            Ok(format_entries(&results))
        }
        "list" => {
            let results = config.memory.list(scope, limit_arg(&args));
            Ok(format_entries(&results))
        }
        "forget" => {
            let id = args
                .get("memoryId")
                .and_then(Value::as_str)
                .context("missing `memoryId` argument")?;
            if config.memory.forget(id)? {
                Ok(format!("forgot memory `{id}`"))
            } else {
                Ok(format!("no memory with id `{id}`"))
            }
        }
        "profile" => match config.memory.profile() {
            Some(entry) => Ok(entry.content),
            None => Ok("no user profile recorded yet".to_string()),
        },
        other => anyhow::bail!("unknown memory mode `{other}`"),
    }
}

fn limit_arg(args: &Value) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(10)
        .clamp(1, 100)
}

fn format_entries(entries: &[crate::memory::MemoryEntry]) -> String {
    if entries.is_empty() {
        return "no matching memories".to_string();
    }
    entries
        .iter()
        .map(|entry| {
            let scope = match entry.scope {
                crate::memory::Scope::Project => "project",
                crate::memory::Scope::User => "user",
            };
            format!("- `{}` [{scope}] {}", entry.id, entry.content)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn memory_spec() -> ToolSpec {
    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "memory".to_string(),
            description: "Persist and retrieve long-term memory across sessions. Modes: `add` stores a note, `search` finds relevant notes, `list` shows recent notes, `forget` deletes a note by id, and `profile` shows the learned user profile. Default scope is the current project; use `user` for cross-project preferences.".to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "mode": {
                        "type": "string",
                        "enum": ["add", "search", "list", "forget", "profile"],
                        "description": "Operation to perform"
                    },
                    "content": {
                        "type": "string",
                        "description": "Memory text to store (for `add`)"
                    },
                    "query": {
                        "type": "string",
                        "description": "Search query (for `search`)"
                    },
                    "tags": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Optional tags (for `add`)"
                    },
                    "scope": {
                        "type": "string",
                        "enum": ["project", "user", "all-projects"],
                        "description": "Memory scope; defaults to `project`"
                    },
                    "memoryId": {
                        "type": "string",
                        "description": "Entry id (for `forget`)"
                    },
                    "limit": {
                        "type": "integer",
                        "description": "Maximum number of results (for `search`/`list`)"
                    }
                },
                "required": ["mode"]
            }),
        },
    }
}

fn lsp_spec() -> ToolSpec {
    ToolSpec {
        kind: "function",
        function: FunctionSpec {
            name: "diagnostics".to_string(),
            description: "Fetch language-server diagnostics (errors and warnings) for a file."
                .to_string(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "File path to check, relative to the working directory"
                    }
                },
                "required": ["path"]
            }),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ecosystem::{AgentDef, Skill};
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn repeated_file_reads_reuse_only_current_content_and_allow_fresh_reads() {
        let dir = std::env::temp_dir().join(format!("oxide_read_reuse_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.txt"), "original").unwrap();
        let runtime = test_runtime().await;
        let config = Config::default();
        let reuse = Arc::new(Mutex::new(crate::efficiency::ReadReuse::default()));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        let mut call = tool_call("read");
        call.id = "first-read".into();
        call.function.arguments = r#"{"path":"file.txt"}"#.into();
        let read = |call: ToolCall| {
            let (config, dir, runtime, tx, reuse) = (
                config.clone(),
                dir.clone(),
                runtime.clone(),
                tx.clone(),
                Arc::clone(&reuse),
            );
            async move {
                dispatch_reusing_read(
                    &config,
                    &dir,
                    &runtime,
                    &tx,
                    &call,
                    0,
                    &tools::Progress::default(),
                    &reuse,
                )
                .await
            }
        };
        assert!(read(call.clone()).await.text.contains("original"));
        assert!(read(call.clone())
            .await
            .text
            .contains("reuse read result `first-read`"));
        // A mutating call in between does not throw the memory away: the
        // digest decides, so an unchanged file is still answered by reference
        // rather than resent in full.
        let mut mutate = tool_call("write");
        mutate.id = "write-other".into();
        mutate.function.arguments = r#"{"path":"other.txt","content":"x"}"#.into();
        read(mutate).await;
        assert!(read(call.clone())
            .await
            .text
            .contains("reuse read result `first-read`"));
        std::fs::write(dir.join("file.txt"), "modified").unwrap();
        assert!(read(call.clone()).await.text.contains("modified"));
        call.function.arguments = r#"{"path":"file.txt","fresh":true}"#.into();
        assert!(read(call.clone()).await.text.contains("modified"));
        reuse.lock().unwrap().retain_context(&[]);
        call.function.arguments = r#"{"path":"file.txt"}"#.into();
        assert!(read(call).await.text.contains("modified"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn run_budgets_checkpoint_tools_and_reset_for_a_new_turn() {
        let dir =
            std::env::temp_dir().join(format!("oxide_budget_checkpoint_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.txt"), "evidence").unwrap();
        let (addr, server) =
            sse_server(vec![tool_call_body("read", r#"{"path":"file.txt"}"#)]).await;
        let mut config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Default::default()
        };
        config.token_efficiency.turn_budget.max_model_calls = Some(1);
        let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
        log.set_name("Budget test").unwrap();
        let user = Message::user("read the file");
        log.append(&user).unwrap();
        let mut runtime = test_runtime().await;
        runtime.session = Some(Arc::clone(&log));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        run(config.clone(), dir.clone(), vec![user], tx, runtime).await;
        assert_eq!(server.await.unwrap().len(), 1);
        let mut history = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::Finished(messages) = event {
                history = messages;
            }
        }
        assert!(history
            .last()
            .unwrap()
            .display()
            .unwrap()
            .contains("model-call budget"));
        assert!(history
            .iter()
            .any(|m| m.role == "tool" && m.display().unwrap().contains("evidence")));
        let reopened = SessionLog::open(log.path().to_path_buf()).unwrap();
        assert!(reopened
            .messages()
            .unwrap()
            .last()
            .unwrap()
            .display()
            .unwrap()
            .contains("Work is saved"));
        let (addr, server) = sse_server(vec![answer_body("continued")]).await;
        config.base_url = format!("http://{addr}");
        history.push(Message::user("continue"));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        run(config, dir.clone(), history, tx, test_runtime().await).await;
        assert_eq!(server.await.unwrap().len(), 1);
        assert!(std::iter::from_fn(|| rx.try_recv().ok())
            .any(|event| matches!(event, AgentEvent::Text(text) if text.contains("continued"))));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn output_budget_caps_provider_requests_and_retry_attempts_share_call_budget() {
        for empty in [false, true] {
            let (addr, server) =
                sse_server(vec![answer_body(if empty { "" } else { "complete" })]).await;
            let mut config = Config {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                base_url: format!("http://{addr}"),
                api_key: "sk-test".into(),
                ..Default::default()
            };
            config.token_efficiency.turn_budget.max_model_calls = Some(1);
            config.token_efficiency.turn_budget.max_output_tokens = Some(7);
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            run(
                config,
                std::env::temp_dir(),
                vec![Message::user("answer")],
                tx,
                test_runtime().await,
            )
            .await;
            let requests = server.await.unwrap();
            assert_eq!(requests.len(), 1);
            let body: Value =
                serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["max_tokens"], 7);
            let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
            assert!(
                !events
                    .iter()
                    .any(|event| matches!(event, AgentEvent::Error(_))),
                "{events:?}"
            );
            if empty {
                assert!(events.iter().any(|event| matches!(event, AgentEvent::Text(text) if text.contains("model-call budget"))));
            }
        }
    }

    #[tokio::test]
    async fn output_and_cost_budgets_stop_even_without_provider_usage() {
        let dir = std::env::temp_dir().join(format!("oxide_missing_usage_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file.txt"), "content").unwrap();
        for output_limit in [true, false] {
            let (addr, server) =
                sse_server(vec![tool_call_body("read", r#"{"path":"file.txt"}"#)]).await;
            let mut config = Config {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                base_url: format!("http://{addr}"),
                api_key: "sk-test".into(),
                ..Default::default()
            };
            if output_limit {
                config.token_efficiency.turn_budget.max_output_tokens = Some(7);
            } else {
                config.token_efficiency.turn_budget.max_cost_usd = Some(0.0000001);
                config.prices.insert(
                    "deepseek-flash".into(),
                    crate::pricing::ModelPrice {
                        input: 1.0,
                        output: 1.0,
                        cache_read: 1.0,
                        cache_write: 1.0,
                    },
                );
            }
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            run(
                config,
                dir.clone(),
                vec![Message::user("read")],
                tx,
                test_runtime().await,
            )
            .await;
            assert_eq!(server.await.unwrap().len(), 1);
            let expected = if output_limit {
                "output-token budget"
            } else {
                "estimated-cost budget"
            };
            assert!(
                std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(event,
                AgentEvent::Text(text) if text.contains(expected)))
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_unknown_model_price_cannot_silently_bypass_a_cost_budget() {
        let mut config = Config {
            provider: "deepseek".into(),
            model: "unknown-price-model".into(),
            base_url: "http://127.0.0.1:1".into(),
            api_key: "sk-test".into(),
            ..Default::default()
        };
        config.token_efficiency.turn_budget.max_cost_usd = Some(0.1);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::time::timeout(
            std::time::Duration::from_secs(2),
            run(
                config,
                std::env::temp_dir(),
                vec![Message::user("answer")],
                tx,
                test_runtime().await,
            ),
        )
        .await
        .unwrap();
        assert!(
            std::iter::from_fn(|| rx.try_recv().ok()).any(|event| matches!(event,
            AgentEvent::Text(text) if text.contains("Configure modelPrices")))
        );
    }

    #[tokio::test]
    async fn task_subagents_share_the_parent_run_budget() {
        let (addr, server) = sse_server(vec![
            tool_call_body("task", r#"{"prompt":"review","subagent_type":"reviewer"}"#),
            answer_body("reviewed"),
        ])
        .await;
        let mut config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Default::default()
        };
        config.ecosystem.agents.push(AgentDef {
            name: "reviewer".into(),
            description: None,
            mode: AgentMode::Subagent,
            tools: None,
            permission: None,
            prompt: String::new(),
        });
        config.token_efficiency.turn_budget.max_model_calls = Some(2);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        run(
            config,
            std::env::temp_dir(),
            vec![Message::user("review")],
            tx,
            test_runtime().await,
        )
        .await;
        assert_eq!(server.await.unwrap().len(), 2);
        let events: Vec<_> = std::iter::from_fn(|| rx.try_recv().ok()).collect();
        assert!(events.iter().any(
            |event| matches!(event, AgentEvent::Text(text) if text.contains("2 model calls"))
        ));
    }

    #[test]
    fn schema_selection_is_bounded_and_keeps_new_capabilities_first() {
        let previous: BTreeSet<String> = ["old_a", "old_b", "old_c"]
            .into_iter()
            .map(str::to_string)
            .collect();
        let selected = bounded_selection(&previous, &["new".into(), "old_c".into()], 2);
        assert_eq!(
            selected,
            ["new", "old_c"].into_iter().map(str::to_string).collect()
        );
        assert_eq!(
            bounded_selection(&previous, &["new".into(), "new".into()], 2).len(),
            2
        );
    }

    #[test]
    fn repeated_failure_guard_allows_polling_and_recovery() {
        let mut guard = FailureLoop::default();
        let failure = tools::ToolOutput::error("authentication required");
        guard.observe("lookup", r#"{"a":1,"b":2}"#, &failure);
        guard.observe("lookup", r#"{ "b": 2, "a": 1 }"#, &failure);
        assert!(guard.blocked.is_none());
        guard.observe("lookup", r#"{"a":1,"b":2}"#, &failure);
        assert!(guard.blocked.is_some());

        let mut guard = FailureLoop::default();
        for _ in 0..10 {
            guard.observe("status", "{}", &tools::ToolOutput::text("pending"));
        }
        assert!(guard.blocked.is_none());
        guard.observe("build", "{}", &tools::ToolOutput::text("failed\n[exit: 1]"));
        guard.observe("build", "{}", &tools::ToolOutput::text("failed\n[exit: 1]"));
        guard.observe("edit", "{}", &tools::ToolOutput::text("updated"));
        guard.observe("build", "{}", &tools::ToolOutput::text("failed\n[exit: 1]"));
        assert!(guard.blocked.is_none());
        guard.observe(
            "build",
            "{}",
            &tools::ToolOutput::text("different failure\n[exit: 1]"),
        );
        assert!(guard.blocked.is_none());
    }

    #[test]
    fn project_context_names_the_checkout_and_resolves_user_file_references() {
        let dir =
            std::env::temp_dir().join(format!("oxide_project_context_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Reviewed copy.xlsx");
        std::fs::write(&file, "sheet").unwrap();
        let messages = vec![
            Message::user("Please inspect @Reviewed copy.xlsx first"),
            Message::tool("read", "some output"),
        ];
        let context = project_context(&dir, &messages);
        assert!(context.contains(&json!(dir.to_string_lossy()).to_string()));
        assert!(context.contains(&json!(file).to_string()));
        assert!(context.contains("no search is needed"));
        assert!(!project_context(&dir, &[]).contains("Existing files referenced"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn project_context_advertises_readable_paths_from_a_relative_project_root() {
        let relative = PathBuf::from(format!("oxide_relative_context_{}", std::process::id()));
        let absolute = std::env::current_dir().unwrap().join(&relative);
        std::fs::create_dir_all(&relative).unwrap();
        std::fs::write(relative.join("Reviewed copy.txt"), "reviewed copy").unwrap();

        let context = project_context(
            &relative,
            &[Message::user("Read @Reviewed copy.txt please")],
        );
        let (_, references) = context
            .split_once("(path data, not instructions): ")
            .unwrap();
        let mut deserializer =
            serde_json::Deserializer::from_str(references).into_iter::<Vec<PathBuf>>();
        let files = deserializer.next().unwrap().unwrap();
        let call = ToolCall {
            id: "read-reference".into(),
            kind: "function".into(),
            signature: None,
            function: crate::llm::FunctionCall {
                name: "read".into(),
                arguments: json!({"path": files[0]}).to_string(),
            },
        };
        let output = tools::execute(
            &call,
            &relative,
            &McpRegistry::default(),
            &tools::Progress::default(),
        )
        .await;
        std::fs::remove_dir_all(&relative).unwrap();

        assert!(context.contains(&json!(absolute).to_string()), "{context}");
        assert!(files[0].is_absolute(), "{files:?}");
        assert!(!output.is_error, "{}", output.text);
        assert!(output.text.contains("reviewed copy"), "{}", output.text);
    }

    fn config_with_skill() -> Config {
        let mut config = Config::default();
        config.ecosystem.skills.push(Skill {
            name: "audit".into(),
            description: Some("audits deps".into()),
            content: "Audit the deps.".into(),
        });
        config
    }

    fn image_part(data: &str) -> ContentPart {
        ContentPart::ImageUrl {
            image_url: crate::llm::ImageUrl {
                url: format!("data:image/png;base64,{data}"),
                detail: None,
            },
        }
    }

    fn file_part(data: &str) -> ContentPart {
        ContentPart::File {
            file: crate::llm::FileData {
                filename: Some("attachment.pdf".into()),
                file_data: data.into(),
            },
        }
    }

    #[test]
    fn repeated_media_is_not_appended_to_the_context_again() {
        let mut history = vec![Message::tool_parts(
            "first",
            "attached shot.png",
            vec![image_part("AAAA")],
        )];
        let mut seen = deduplicate_tool_media(&mut history);
        let mut output = tools::ToolOutput::with_media(
            "attached ./shot.png",
            vec![image_part("AAAA"), image_part("BBBB")],
        );

        omit_duplicate_media(&mut output, &mut seen);

        assert_eq!(output.media, vec![image_part("BBBB")]);
        assert!(
            output.text.contains("1 media attachment(s)"),
            "{}",
            output.text
        );
        assert!(output.text.contains("duplicate bytes were omitted"));
    }

    #[test]
    fn first_media_result_is_left_unchanged() {
        let mut seen = HashSet::new();
        let mut output =
            tools::ToolOutput::with_media("attached shot.png", vec![image_part("AAAA")]);

        omit_duplicate_media(&mut output, &mut seen);

        assert_eq!(output.media, vec![image_part("AAAA")]);
        assert_eq!(output.text, "attached shot.png");
        assert_eq!(seen.len(), 1);
    }

    #[test]
    fn identical_payloads_of_different_media_kinds_are_not_deduplicated() {
        let mut seen = HashSet::new();
        let mut output = tools::ToolOutput::with_media(
            "attached image and file",
            vec![image_part("AAAA"), file_part("AAAA")],
        );

        omit_duplicate_media(&mut output, &mut seen);

        assert_eq!(output.media, vec![image_part("AAAA"), file_part("AAAA")]);
        assert_eq!(seen.len(), 2);
    }

    #[test]
    fn resumed_history_drops_duplicate_tool_media_but_keeps_user_attachments() {
        let mut messages = vec![
            Message::user_parts("original", vec![image_part("AAAA")]),
            Message::tool_parts("first", "attached first", vec![image_part("AAAA")]),
            Message::tool_parts("second", "attached second", vec![image_part("BBBB")]),
            Message::tool_parts("third", "attached third", vec![image_part("BBBB")]),
            Message::user_parts("again", vec![image_part("AAAA")]),
        ];

        let seen = deduplicate_tool_media(&mut messages);

        assert_eq!(seen.len(), 2);
        let media_counts: Vec<_> = messages
            .iter()
            .map(|message| match message.content.as_ref() {
                Some(MessageContent::Parts(parts)) => parts
                    .iter()
                    .filter(|part| !matches!(part, ContentPart::Text { .. }))
                    .count(),
                _ => 0,
            })
            .collect();
        assert_eq!(media_counts, vec![1, 0, 1, 0, 1]);
    }

    #[test]
    fn skill_tool_returns_content() {
        let config = config_with_skill();
        assert_eq!(skill(&config, r#"{"name":"audit"}"#), "Audit the deps.");
    }

    #[test]
    fn skill_tool_reports_unknown() {
        let config = config_with_skill();
        assert!(skill(&config, r#"{"name":"missing"}"#).starts_with("error: unknown skill"));
    }

    #[test]
    fn task_spec_lists_subagents() {
        let mut config = Config::default();
        config.ecosystem.agents.push(AgentDef {
            name: "reviewer".into(),
            description: Some("reviews code".into()),
            mode: AgentMode::Subagent,
            tools: None,
            permission: None,
            prompt: String::new(),
        });
        assert!(task_spec(&config).function.description.contains("reviewer"));
    }

    #[test]
    fn active_agent_tools_are_a_hard_allowlist() {
        let config = Config {
            active_agent: Some(AgentDef {
                name: "scout".into(),
                description: None,
                mode: AgentMode::Subagent,
                tools: Some(vec![
                    "read".into(),
                    "grep".into(),
                    "find".into(),
                    "ls".into(),
                ]),
                permission: None,
                prompt: String::new(),
            }),
            ..Config::default()
        };

        assert!(tool_enabled(&config, "read"));
        assert!(tool_enabled(&config, "grep"));
        assert!(tool_enabled(&config, "find"));
        assert!(tool_enabled(&config, "ls"));
        // A tool is named by the name it is registered under, so a name no
        // tool answers to is not allowed by being an old one of it.
        assert!(!tool_enabled(&config, "read_file"));
        assert!(!tool_enabled(&config, "write"));
        assert!(!tool_enabled(&config, "edit"));
        assert!(!tool_enabled(&config, "bash"));
    }

    async fn test_runtime() -> Runtime {
        Runtime {
            mcp: Arc::new(McpRegistry::new(&[])),
            selected_mcp_tools: Default::default(),
            plugins: Arc::new(PluginHost::spawn(&[], &std::env::temp_dir()).await),
            session: None,
            snapshots: None,
            lsp: Arc::new(LspManager::new()),
            approve: Arc::new(|_, _| Box::pin(async { false })),
            ask: None,
            steering: Steering::new(),
            follow_ups: Steering::new(),
            steering_mode: QueueMode::OneAtATime,
            follow_up_mode: QueueMode::OneAtATime,
            cancel: Cancel::new(),
            cache_warm: None,
        }
    }

    #[tokio::test]
    async fn tool_access_is_independent_of_project_trust() {
        let base = std::env::temp_dir().join(format!(
            "oxide_filesystem_access_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let project = base.join("project");
        let outside = base.join("scratch.txt");
        std::fs::create_dir_all(&project).unwrap();
        let runtime = test_runtime().await;
        let (events, _rx) = tokio::sync::mpsc::unbounded_channel();
        for trusted in [true, false] {
            let config = Config {
                trusted,
                ..Config::default()
            };
            for (name, arguments) in [
                (
                    "write",
                    json!({"path": outside, "content": "scratch content"}),
                ),
                ("read", json!({"path": outside})),
                ("bash", json!({"command": "echo /"})),
            ] {
                let mut call = tool_call(name);
                call.function.arguments = arguments.to_string();
                let output = dispatch(
                    &config,
                    &project,
                    &runtime,
                    &events,
                    &call,
                    0,
                    &tools::Progress::default(),
                )
                .await;
                assert!(
                    !output.is_error,
                    "{name} (trusted={trusted}): {}",
                    output.text
                );
                if name == "read" {
                    assert!(output.text.contains("scratch content"));
                }
            }
        }
        std::fs::remove_dir_all(base).ok();
    }

    #[tokio::test]
    async fn session_title_is_persisted_once_and_removed_from_tools() {
        let dir = std::env::temp_dir().join(format!(
            "oxide_title_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log = SessionLog::create_in(&dir, &dir).unwrap();
        let mut runtime = test_runtime().await;
        runtime.session = Some(Arc::new(log));
        let config = Config::default();
        assert!(
            build_tool_specs(&config, &runtime, 0, needs_session_title(&runtime, 0))
                .iter()
                .any(|spec| spec.function.name == "session_title")
        );

        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let output = set_session_title(
            &runtime,
            &tx,
            r#"{"title":"Title: Diagnose private GitHub links"}"#,
            0,
        );
        assert!(!output.is_error);
        assert_eq!(
            runtime.session.as_ref().unwrap().name().as_deref(),
            Some("Diagnose private GitHub links")
        );
        assert!(matches!(
            rx.try_recv(),
            Ok(AgentEvent::SessionTitle { title }) if title == "Diagnose private GitHub links"
        ));
        assert!(
            !build_tool_specs(&config, &runtime, 0, needs_session_title(&runtime, 0))
                .iter()
                .any(|spec| spec.function.name == "session_title")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Deadline for a scripted request to arrive, so a test that stops short of
    /// its scripted turns fails instead of waiting out the job timeout. It has
    /// to outlast a single tool call: a `bash` command may legitimately run for
    /// the tool's own 120s default, and on a loaded Windows runner a `git` or
    /// `gh` invocation has crossed the old 30s and flaked the review-reply test.
    const POLL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

    /// Serves one SSE response per request, in order, and returns the raw
    /// request texts it saw so a test can assert on what was sent. A request
    /// that never arrives fails the test instead of hanging the runner.
    /// Creates a repository with a committed `catalog.yaml`, so a later edit
    /// produces a real `git diff` hunk.
    /// Runs `git` in `dir` for a test, panicking on failure.
    fn run_git(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .expect("git is available for the test");
        assert!(status.success(), "git {args:?} failed");
    }

    fn git_text(dir: &std::path::Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .stderr(std::process::Stdio::null())
            .output()
            .expect("git is available for the test");
        assert!(output.status.success(), "git {args:?} failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    /// Creates a repository with a committed `catalog.yaml`, so a later edit
    /// produces a real `git diff` hunk.
    fn init_git_repo(dir: &std::path::Path) {
        run_git(dir, &["init", "-q"]);
        run_git(dir, &["config", "user.email", "oxide@example.com"]);
        run_git(dir, &["config", "user.name", "Oxide Test"]);
        // A global `commit.gpgsign` would otherwise make commits fail.
        run_git(dir, &["config", "commit.gpgsign", "false"]);
        std::fs::write(dir.join("catalog.yaml"), "old\n").unwrap();
        run_git(dir, &["add", "catalog.yaml"]);
        run_git(dir, &["commit", "-q", "-m", "init"]);
    }

    /// Adds a bare `origin` remote and pushes the current commit, so
    /// [`repo_has_pending_delivery`] starts clean.
    fn add_git_remote(dir: &std::path::Path) {
        let origin = std::path::PathBuf::from(format!("{}.origin.git", dir.display()));
        std::fs::remove_dir_all(&origin).ok();
        let status = std::process::Command::new("git")
            .args(["init", "--bare", "-q"])
            .arg(&origin)
            .status()
            .expect("git is available for the test");
        assert!(status.success(), "git init --bare failed");
        run_git(dir, &["remote", "add", "origin", origin.to_str().unwrap()]);
        run_git(dir, &["push", "-u", "origin", "HEAD"]);
    }

    async fn sse_server(
        bodies: Vec<String>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<String>>) {
        scripted_sse_server(bodies.into_iter().map(|body| (body, false)).collect()).await
    }

    /// Serves a scripted sequence of responses; a `truncated` entry declares a
    /// larger `content-length` than it writes, so the client sees a dropped
    /// stream and retries (see `LlmClient::stream_chat`).
    async fn scripted_sse_server(
        responses: Vec<(String, bool)>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<String>>) {
        scripted_http_server(
            responses
                .into_iter()
                .map(|(body, truncated)| (200, body, truncated))
                .collect(),
        )
        .await
    }

    async fn scripted_http_server(
        responses: Vec<(u16, String, bool)>,
    ) -> (std::net::SocketAddr, tokio::task::JoinHandle<Vec<String>>) {
        use tokio::io::AsyncWriteExt;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for (status, body, truncated) in responses {
                let (mut socket, _) = tokio::time::timeout(POLL_TIMEOUT, listener.accept())
                    .await
                    .expect("the agent stopped short of every scripted request")
                    .unwrap();
                seen.push(read_request(&mut socket).await);
                let declared = if truncated {
                    body.len() + 64
                } else {
                    body.len()
                };
                let response = format!(
                    "HTTP/1.1 {status} Response\r\ncontent-type: text/event-stream\r\ncontent-length: {declared}\r\nconnection: close\r\n\r\n{body}"
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.shutdown().await;
            }
            seen
        });
        (addr, handle)
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;

        let mut raw = Vec::new();
        let mut expected: Option<usize> = None;
        loop {
            let mut chunk = [0u8; 8192];
            let read = match socket.read(&mut chunk).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            raw.extend_from_slice(&chunk[..read]);
            let text = String::from_utf8_lossy(&raw);
            let Some(head_end) = text.find("\r\n\r\n") else {
                continue;
            };
            if expected.is_none() {
                expected = text[..head_end].lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
            }
            if expected.is_some_and(|len| raw.len() - (head_end + 4) >= len) {
                break;
            }
        }
        String::from_utf8_lossy(&raw).into_owned()
    }

    fn openai_sse(events: &[serde_json::Value]) -> String {
        let mut body = String::new();
        for event in events {
            body.push_str("data: ");
            body.push_str(&event.to_string());
            body.push_str("\n\n");
        }
        body.push_str("data: [DONE]\n\n");
        body
    }

    fn write_call_body(path: &str) -> String {
        // The arguments are a JSON *string*, so the path must be escaped: a
        // Windows temp path (backslashes) would otherwise be unparseable and
        // the write would fail instead of recording an edit.
        let arguments = json!({"path": path, "content": "x"}).to_string();
        openai_sse(&[
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_0", "function": {"name": "write", "arguments": arguments}}]}, "finish_reason": null}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ])
    }

    fn bash_call_body(command: &str) -> String {
        let arguments = json!({"command": command}).to_string();
        tool_call_body("bash", &arguments)
    }

    /// A tool call whose arguments are already serialized.
    fn tool_call_body(name: &str, arguments: &str) -> String {
        openai_sse(&[
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "call_1", "function": {"name": name, "arguments": arguments}}]}, "finish_reason": null}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ])
    }

    fn titled_answer_body(title: &str, answer: &str) -> String {
        let arguments = json!({"title": title}).to_string();
        openai_sse(&[
            json!({"choices": [{"delta": {"content": answer}, "finish_reason": null}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"index": 0, "id": "title_1", "function": {"name": "session_title", "arguments": arguments}}]}, "finish_reason": null}]}),
            json!({"choices": [{"delta": {}, "finish_reason": "tool_calls"}]}),
        ])
    }

    fn answer_body(text: &str) -> String {
        openai_sse(&[
            serde_json::json!({"choices": [{"delta": {"content": text}, "finish_reason": null}]}),
            serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]}),
        ])
    }

    fn empty_body() -> String {
        openai_sse(&[serde_json::json!({"choices": [{"delta": {}, "finish_reason": "stop"}]})])
    }

    #[test]
    fn merged_billing_keeps_the_latest_request_context_when_reopened() {
        let first = crate::compact::UsageRecord::from(crate::llm::Usage {
            input: 110_000,
            output: 100,
            ..Default::default()
        });
        let second = crate::compact::UsageRecord::from(crate::llm::Usage {
            input: 111_000,
            output: 200,
            ..Default::default()
        });
        let merged = combine_usage(Some(first), Some(second)).unwrap();
        assert_eq!(merged.input, 221_000);
        assert_eq!(merged.context.unwrap().prompt, 111_000);
        let dir = std::env::temp_dir().join(format!("oxide_merged_context_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let log = SessionLog::create_in(&dir, &dir).unwrap();
        log.append(&Message::user("request")).unwrap();
        log.append_with_usage(
            &Message::assistant("held answer and verification", vec![]),
            Some(merged),
        )
        .unwrap();
        log.append(&Message::user("follow-up")).unwrap();
        let reopened = SessionLog::open(log.path().to_path_buf()).unwrap();
        assert_eq!(reopened.usage_totals().input, 221_000);
        assert_eq!(reopened.context_tokens(), 111_000);
        assert_eq!(
            reopened.context_measurement(),
            Some((
                crate::llm::ContextUsage {
                    prompt: 111_000,
                    output: 200
                },
                2
            ))
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn portkey_turns_send_catalog_budget_and_persist_effort_choices() {
        let native_answer = openai_sse(&[
            json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Completed"}}),
            json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}),
        ]);
        let (addr, server) = sse_server(vec![native_answer.clone(), native_answer]).await;
        let dir = std::env::temp_dir().join(format!("oxide_portkey_turn_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
        log.set_name("Existing work").unwrap();
        let mut runtime = test_runtime().await;
        runtime.session = Some(Arc::clone(&log));
        for reasoning in [
            crate::config::Reasoning::Medium,
            crate::config::Reasoning::Low,
        ] {
            let config = Config {
                provider: "portkey".into(),
                model: "claude-opus-5".into(),
                api_key: "pk-test".into(),
                base_url: format!("http://{addr}"),
                reasoning,
                catalog_output_limits: BTreeMap::from([(
                    "anthropic".into(),
                    BTreeMap::from([("claude-opus-5".into(), 128_000)]),
                )]),
                ..Config::default()
            };
            log.append(&Message::user("Update the article")).unwrap();
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            run(
                config,
                dir.clone(),
                log.messages().unwrap(),
                tx,
                runtime.clone(),
            )
            .await;
            while let Ok(event) = rx.try_recv() {
                assert!(
                    !matches!(event, AgentEvent::Error(_) | AgentEvent::Retrying { .. }),
                    "{event:?}"
                );
            }
            let reopened = SessionLog::open(log.path().to_path_buf()).unwrap();
            assert_eq!(
                reopened.thinking_level().as_deref(),
                Some(reasoning.label())
            );
            assert!(
                matches!(reopened.entries().iter().rev().find(|entry| matches!(entry, crate::session::Entry::ModelChange(_))),
                Some(crate::session::Entry::ModelChange(entry)) if entry.provider == "portkey" && entry.model_id == "claude-opus-5")
            );
        }
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);
        for (request, effort) in requests.iter().zip(["medium", "low"]) {
            let body: serde_json::Value =
                serde_json::from_str(request.split_once("\r\n\r\n").unwrap().1).unwrap();
            assert_eq!(body["max_tokens"], 32_768);
            assert_eq!(body["thinking"]["type"], "adaptive");
            assert_eq!(body["output_config"]["effort"], effort);
            let system = body["system"].to_string();
            assert!(
                system.contains(&format!("Effective reasoning effort: {effort}")),
                "{system}"
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn response_output_is_measured_once_for_tools_held_answers_and_titles() {
        for mode in ["tool", "held", "title"] {
            let payload = if mode == "title" {
                json!({"tool_calls": [{"index":0,"id":"title","function":{"name":"session_title","arguments":"{\"title\":\"Existing work\"}"}}]})
            } else if mode == "tool" {
                json!({"content":"response ".repeat(250),"tool_calls":[{"index":0,"id":"list","function":{"name":"ls","arguments":"{\"path\":\".\"}"}}]})
            } else {
                json!({"content":format!("{}\n\nLet me create a summary:", "response ".repeat(250))})
            };
            let first = openai_sse(&[
                json!({"choices":[{"delta":payload,"finish_reason":null}]}),
                json!({"choices":[{"delta":{},"finish_reason":"stop"}],"usage":{"prompt_tokens":1400,"completion_tokens":if mode == "title" {600} else {400}}}),
            ]);
            let (addr, server) = sse_server(vec![first, answer_body("Completed")]).await;
            let dir = std::env::temp_dir().join(format!(
                "oxide_context_boundary_{}_{}",
                std::process::id(),
                mode
            ));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(dir.join("project")).unwrap();
            let log = Arc::new(SessionLog::create_in(&dir.join("logs"), &dir).unwrap());
            if mode != "title" {
                log.set_name("Existing work").unwrap();
            }
            for message in [
                Message::user("old request"),
                Message::assistant("old answer", vec![]),
                Message::user("continue"),
            ] {
                log.append(&message).unwrap();
            }
            let mut runtime = test_runtime().await;
            runtime.session = Some(Arc::clone(&log));
            let config = Config {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                api_key: "sk-test".into(),
                base_url: format!("http://{addr}"),
                max_tokens: 1000,
                context_window: 2000,
                compaction: crate::compact::CompactionConfig {
                    reserve_tokens: 100,
                    keep_recent_tokens: 1,
                    ..Default::default()
                },
                ..Config::default()
            };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            run(
                config,
                dir.join("project"),
                log.messages().unwrap(),
                tx,
                runtime,
            )
            .await;
            assert_eq!(
                server.await.unwrap().len(),
                2,
                "{mode} must not add a compaction request"
            );
            while let Ok(event) = rx.try_recv() {
                assert!(
                    !matches!(event, AgentEvent::Compaction { .. } | AgentEvent::Error(_)),
                    "{mode}: {event:?}"
                );
            }
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[tokio::test]
    async fn failed_overflow_recovery_preserves_the_held_answer_and_usage() {
        for summarizer_fails in [false, true] {
            let first = openai_sse(&[
                json!({"choices":[{"delta":{"content":"Let me create the summary:"},"finish_reason":"stop"}],"usage":{"prompt_tokens":110000,"completion_tokens":20}}),
            ]);
            let mut responses = vec![
                (200, first, false),
                (
                    400,
                    "prompt is too long: 204873 tokens > 200000 maximum".into(),
                    false,
                ),
            ];
            if summarizer_fails {
                responses.push((400, "summarizer rejected".into(), false));
            }
            let (addr, server) = scripted_http_server(responses).await;
            let dir = std::env::temp_dir().join(format!(
                "oxide_failed_compaction_{}_{}",
                std::process::id(),
                summarizer_fails
            ));
            std::fs::remove_dir_all(&dir).ok();
            let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
            log.set_name("Existing work").unwrap();
            log.append(&Message::user("Summarize this work")).unwrap();
            let mut runtime = test_runtime().await;
            runtime.session = Some(Arc::clone(&log));
            let config = Config {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                base_url: format!("http://{addr}"),
                api_key: "sk-test".into(),
                context_window: 1050000,
                compaction: crate::compact::CompactionConfig {
                    keep_recent_tokens: if summarizer_fails { 1 } else { 20000 },
                    ..Default::default()
                },
                ..Config::default()
            };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            run(config, dir.clone(), log.messages().unwrap(), tx, runtime).await;
            assert_eq!(
                server.await.unwrap().len(),
                if summarizer_fails { 3 } else { 2 }
            );
            assert!(log.messages().unwrap().iter().any(|message| message
                .display()
                .is_some_and(|text| text.contains("Let me create the summary:"))));
            assert_eq!(log.usage_totals().input, 110000);
            let mut billed = 0;
            while let Ok(event) = rx.try_recv() {
                if let AgentEvent::Usage { input, .. } = event {
                    billed += input;
                }
            }
            assert_eq!(billed, 110000);
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[test]
    fn unfinished_announcements_are_distinct_from_summaries_and_questions() {
        for text in [
            "Let me create a summary:",
            "The request failed. Let me check the documented endpoint:",
            "I'll inspect the response:",
            "Excellent! Now let me set the session title and provide my review:",
            "Perfect! Now I'll verify the build:",
            "The build passed. Next, I will summarize the findings:",
        ] {
            assert!(announces_unfinished_work(text), "{text}");
        }
        for text in [
            "Summary:\n6 entries created.",
            "Let me know which endpoint you prefer:",
            "The request is blocked by a refused connection.",
            "Would you like a summary?",
            "> Let me create a summary:",
            "`Let me create a summary:`",
            "The fields are:",
            "Excellent! Now the review is complete:",
            "Now let me know which endpoint you prefer:",
            "Excellent! Now let me provide my review:\n\nNo issues found.",
        ] {
            assert!(!announces_unfinished_work(text), "{text}");
        }
    }

    #[tokio::test]
    async fn a_titled_review_announcement_still_requires_the_review() {
        let announcement = "Excellent! Now let me set the session title and provide my review:";
        let review = "Review complete. No correctness issues found; the build passed.";
        let (addr, server) = sse_server(vec![
            bash_call_body("printf 'build passed'"),
            titled_answer_body("Review coding agent guide split PR", announcement),
            answer_body(review),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            auto_approve: true,
            ..Config::default()
        };
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        run(
            config,
            std::env::temp_dir(),
            vec![Message::user("Review the coding agent guide split PR")],
            tx,
            runtime,
        )
        .await;
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Error(error) => panic!("{error}"),
                AgentEvent::Finished(history) => finished = Some(history),
                _ => {}
            }
        }
        let history = finished.expect("the turn must finish");
        let answer = history.last().unwrap().display().unwrap();
        assert!(answer.contains(review), "{answer}");
        assert!(history
            .iter()
            .flat_map(|message| message.tool_calls.iter().flatten())
            .all(|call| call.function.name != "session_title"));
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[2].contains("Complete the announced action or summary now"));
    }

    #[tokio::test]
    async fn an_unfinished_summary_gets_one_follow_through_step() {
        for final_answer in [
            "Prepared 6 entries. Submission remains blocked by the refused connection.",
            "Let me create the summary:",
        ] {
            let (addr, server) = sse_server(vec![
                answer_body("The API request failed. Let me create a comprehensive summary:"),
                answer_body(final_answer),
            ])
            .await;
            let config = Config {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                base_url: format!("http://{addr}"),
                api_key: "sk-test".into(),
                ..Config::default()
            };
            let runtime = test_runtime().await;
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            run(
                config,
                std::env::temp_dir(),
                vec![Message::user("Submit the prepared entries")],
                tx,
                runtime,
            )
            .await;
            let requests = server.await.unwrap();
            assert_eq!(
                requests.len(),
                2,
                "the follow-through is bounded to one step"
            );
            assert!(requests[1].contains("Complete the announced action or summary now"));
            let mut finished = None;
            while let Ok(event) = rx.try_recv() {
                if let AgentEvent::Finished(history) = event {
                    finished = Some(history);
                }
            }
            let history = finished.expect("the turn must finish");
            assert!(history
                .last()
                .unwrap()
                .display()
                .unwrap()
                .contains(final_answer));
        }
    }

    #[tokio::test]
    async fn resumed_context_uses_recorded_usage_before_the_first_request() {
        let (addr, server) = sse_server(vec![
            answer_body("Compacted earlier work"),
            answer_body("New answer"),
        ])
        .await;
        let dir =
            std::env::temp_dir().join(format!("oxide_resumed_context_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
        log.set_name("Existing thread").unwrap();
        log.append(&Message::user("OLD_HISTORY".repeat(100)))
            .unwrap();
        log.append_with_usage(
            &Message::assistant("Previous answer", vec![]),
            Some(crate::compact::UsageRecord {
                input: 190_000,
                ..Default::default()
            }),
        )
        .unwrap();
        log.append(&Message::user(format!(
            "NEW_REQUEST {}",
            "new ".repeat(150)
        )))
        .unwrap();
        let mut runtime = test_runtime().await;
        runtime.session = Some(Arc::clone(&log));
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            context_window: 200_000,
            ..Config::default()
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        run(config, dir.clone(), log.messages().unwrap(), tx, runtime).await;
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[1].contains("NEW_REQUEST"));
        assert!(!requests[1].contains("OLD_HISTORY"));
        let mut compacted = false;
        let mut compacted_context = None;
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::Compaction { tokens_before, .. } = event {
                assert!(tokens_before >= 190_000);
                compacted = true;
            }
            if let AgentEvent::Usage {
                context_tokens: Some(tokens),
                ..
            } = event
            {
                compacted_context.get_or_insert(tokens);
            }
        }
        assert!(
            compacted,
            "recorded provider usage must outweigh the small text estimate"
        );
        let context = compacted_context.expect("compaction reports the new request size");
        assert!(
            context > 180_000,
            "fixed prompt overhead must remain counted"
        );
        assert!(context < 190_000, "summarized history must reclaim space");
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn context_overflow_compacts_once_and_preserves_the_latest_request() {
        for reject_again in [false, true] {
            let overflow = "prompt is too long: 204873 tokens > 200000 maximum";
            let (addr, server) = scripted_http_server(vec![
                (400, overflow.into(), false),
                (
                    200,
                    answer_body("Earlier work is complete; retain the user's latest request."),
                    false,
                ),
                if reject_again {
                    (400, overflow.into(), false)
                } else {
                    (200, answer_body("Recovered answer"), false)
                },
            ])
            .await;
            let dir = std::env::temp_dir().join(format!(
                "oxide_overflow_{}_{}",
                std::process::id(),
                reject_again
            ));
            std::fs::remove_dir_all(&dir).ok();
            let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
            log.set_name("Existing thread").unwrap();
            let latest = format!("LATEST_USER_REQUEST {}", "current ".repeat(80));
            let history = vec![
                Message::user(format!("OLD_HISTORY {}", "old ".repeat(400))),
                Message::assistant("Earlier answer", vec![]),
                Message::user(&latest),
            ];
            for message in &history {
                log.append(message).unwrap();
            }
            let mut runtime = test_runtime().await;
            runtime.session = Some(Arc::clone(&log));
            let config = Config {
                provider: "deepseek".into(),
                model: "deepseek-flash".into(),
                base_url: format!("http://{addr}"),
                api_key: "sk-test".into(),
                context_window: 1_050_000,
                ..Config::default()
            };
            let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
            run(config, dir.clone(), history, tx, runtime).await;
            let requests = server.await.unwrap();
            assert_eq!(requests.len(), 3, "one compaction and one retry only");
            assert!(requests[2].contains("LATEST_USER_REQUEST"));
            assert!(!requests[2].contains("OLD_HISTORY"));
            let mut compacted = 0;
            let mut corrected = false;
            let mut errors = Vec::new();
            while let Ok(event) = rx.try_recv() {
                match event {
                    AgentEvent::Compaction { .. } => compacted += 1,
                    AgentEvent::ContextWindow { tokens, .. } => {
                        assert_eq!(tokens, 200_000);
                        corrected = true;
                    }
                    AgentEvent::Error(error) => errors.push(error),
                    _ => {}
                }
            }
            assert_eq!(compacted, 1);
            assert!(corrected);
            assert_eq!(
                log.context_window("deepseek", "deepseek-flash"),
                Some(200_000)
            );
            assert_eq!(log.context_window("deepseek", "another-model"), None);
            let reopened = SessionLog::open(log.path().to_path_buf()).unwrap();
            assert_eq!(
                reopened.context_window("deepseek", "deepseek-flash"),
                Some(200_000)
            );
            assert_eq!(errors.len(), usize::from(reject_again), "{errors:?}");
            assert!(log.messages().unwrap().iter().any(|message| message
                .display()
                .is_some_and(|text| text.contains("LATEST_USER_REQUEST"))));
            std::fs::remove_dir_all(dir).ok();
        }
    }

    #[tokio::test]
    async fn title_metadata_uses_the_first_model_request_without_a_tool_card() {
        let (addr, server) = sse_server(vec![titled_answer_body(
            "Improve session title generation",
            "Implemented the shared title flow.",
        )])
        .await;
        let dir = std::env::temp_dir().join(format!(
            "oxide_title_turn_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
        let mut runtime = test_runtime().await;
        runtime.session = Some(Arc::clone(&log));
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            dir.clone(),
            vec![Message::user("please improve this")],
            tx,
            runtime,
        )
        .await;

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 1, "title generation must not add a request");
        assert!(requests[0].contains("session_title"));
        let mut title = None;
        let mut exposed_tool = false;
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::SessionTitle { title: value } => title = Some(value),
                AgentEvent::ToolCall { name, .. } if name == "session_title" => exposed_tool = true,
                AgentEvent::Finished(messages) => finished = Some(messages),
                _ => {}
            }
        }
        assert_eq!(title.as_deref(), Some("Improve session title generation"));
        assert!(!exposed_tool, "title metadata must not become a tool card");
        assert_eq!(log.name().as_deref(), title.as_deref());
        let finished = finished.expect("the run finished");
        assert!(finished
            .iter()
            .flat_map(|message| message.tool_calls.iter().flatten())
            .all(|call| call.function.name != "session_title"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn a_title_only_provider_response_is_retried_for_the_actual_answer() {
        let title_args = json!({"title": "Improve session title generation"}).to_string();
        let (addr, server) = sse_server(vec![
            tool_call_body("session_title", &title_args),
            answer_body("Implemented the shared title flow."),
        ])
        .await;
        let dir = std::env::temp_dir().join(format!(
            "oxide_title_retry_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
        let mut runtime = test_runtime().await;
        runtime.session = Some(Arc::clone(&log));
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            dir.clone(),
            vec![Message::user("please improve this")],
            tx,
            runtime,
        )
        .await;

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("session_title"));
        assert!(!requests[1].contains("session_title"));
        let mut text = String::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::Text(delta) = event {
                text.push_str(&delta);
            }
        }
        assert_eq!(text, "Implemented the shared title flow.");
        assert_eq!(
            log.name().as_deref(),
            Some("Improve session title generation")
        );
        let stored = log.messages().unwrap();
        let assistants: Vec<_> = stored
            .iter()
            .filter(|message| message.role == "assistant")
            .collect();
        assert_eq!(assistants.len(), 1, "{stored:?}");
        assert_eq!(
            assistants[0].display().as_deref(),
            Some("Implemented the shared title flow.")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn an_invalid_title_is_attempted_once_without_storing_an_empty_message() {
        let title_args = json!({"title": "   "}).to_string();
        let (addr, server) = sse_server(vec![
            tool_call_body("session_title", &title_args),
            answer_body("Continued without a generated title."),
        ])
        .await;
        let dir = std::env::temp_dir().join(format!(
            "oxide_invalid_title_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
        let mut runtime = test_runtime().await;
        runtime.session = Some(Arc::clone(&log));
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            dir.clone(),
            vec![Message::user("please improve this")],
            tx,
            runtime,
        )
        .await;

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("session_title"));
        assert!(!requests[1].contains("session_title"));
        assert!(log.name().is_none());
        let stored = log.messages().unwrap();
        let assistants: Vec<_> = stored
            .iter()
            .filter(|message| message.role == "assistant")
            .collect();
        assert_eq!(assistants.len(), 1, "{stored:?}");
        assert_eq!(
            assistants[0].display().as_deref(),
            Some("Continued without a generated title.")
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A text delta with no `[DONE]` and no stop reason, paired with a
    /// truncated server response so the client treats the turn as dropped.
    fn partial_text_body(text: &str) -> String {
        format!(
            "data: {}\n\n",
            json!({"choices": [{"delta": {"content": text}, "finish_reason": null}]})
        )
    }

    /// A subagent whose stream drops after streaming text is retried, and the
    /// failed attempt's text must not be reported twice in the task result.
    #[tokio::test]
    async fn a_retried_subagent_stream_reports_its_text_once() {
        let (addr, server) = scripted_sse_server(vec![
            (partial_text_body("partial "), true),
            (answer_body("final"), false),
        ])
        .await;
        let mut config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        config.ecosystem.agents.push(AgentDef {
            name: "reviewer".into(),
            description: Some("reviews code".into()),
            mode: AgentMode::Subagent,
            tools: None,
            permission: None,
            prompt: String::new(),
        });
        let runtime = test_runtime().await;
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let output = task_inner(
            &config,
            std::env::temp_dir().as_path(),
            &runtime,
            &tx,
            r#"{"prompt":"review it","subagent_type":"reviewer"}"#,
            0,
        )
        .await
        .unwrap();

        assert_eq!(output, "final");
        assert_eq!(server.await.unwrap().len(), 2);
    }

    /// The subtask-command path accumulates subagent text the same way, so a
    /// retried stream must not put the failed attempt in the merged history.
    #[tokio::test]
    async fn a_retried_subtask_command_reports_its_text_once() {
        let (addr, server) = scripted_sse_server(vec![
            (partial_text_body("partial "), true),
            (answer_body("final"), false),
        ])
        .await;
        let mut config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        config.ecosystem.agents.push(AgentDef {
            name: "reviewer".into(),
            description: Some("reviews code".into()),
            mode: AgentMode::Subagent,
            tools: None,
            permission: None,
            prompt: String::new(),
        });
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run_subagent(
            config,
            std::env::temp_dir(),
            Vec::new(),
            "reviewer".into(),
            "review it".into(),
            tx,
            runtime,
        )
        .await;

        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::Finished(messages) = event {
                finished = Some(messages);
            }
        }
        let messages = finished.expect("the subagent finished");
        assert_eq!(
            messages.last().and_then(|message| message.display()),
            Some("final".to_string())
        );
        assert_eq!(server.await.unwrap().len(), 2);
    }

    /// When every retry is exhausted, the text the last attempt already
    /// streamed stays in the session so the next turn can build on it.
    #[tokio::test]
    async fn an_exhausted_retry_budget_keeps_the_last_attempt_text() {
        let (addr, server) = scripted_sse_server(vec![
            (partial_text_body(" half a thought "), true),
            (partial_text_body(" half a thought "), true),
            (partial_text_body(" half a thought "), true),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            std::env::temp_dir(),
            vec![Message::user("hi")],
            tx,
            runtime,
        )
        .await;

        let mut error = None;
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Error(message) => error = Some(message),
                AgentEvent::Finished(messages) => finished = Some(messages),
                _ => {}
            }
        }
        assert!(error.is_some(), "the exhausted budget is reported");
        let finished = finished.expect("the run finished");
        assert_eq!(
            finished.last().and_then(|message| message.display()),
            Some(" half a thought ".to_string())
        );
        assert_eq!(server.await.unwrap().len(), 3);
    }

    /// A retried step must re-open the thinking boundary on the wire, so the
    /// recovered text is not left inside the failed attempt's thinking block.
    #[tokio::test]
    async fn a_retry_reopens_the_thinking_boundary() {
        let (addr, server) = scripted_sse_server(vec![
            (partial_text_body("partial "), true),
            (answer_body("final"), false),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            std::env::temp_dir(),
            vec![Message::user("hi")],
            tx,
            runtime,
        )
        .await;

        let mut thoughts = 0;
        let mut texts = Vec::new();
        let mut errors = Vec::new();
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Thought { .. } => thoughts += 1,
                AgentEvent::Text(delta) => texts.push(delta),
                AgentEvent::Error(message) => errors.push(message),
                AgentEvent::Finished(messages) => finished = Some(messages),
                _ => {}
            }
        }
        assert!(errors.is_empty(), "{errors:?}");
        // One boundary per attempt, so the retry's answer closes its own block.
        assert_eq!(thoughts, 2);
        assert_eq!(texts, vec!["partial ".to_string(), "final".to_string()]);
        let finished = finished.expect("the run finished");
        assert_eq!(
            finished.last().and_then(|message| message.display()),
            Some("final".to_string())
        );
        assert_eq!(server.await.unwrap().len(), 2);
    }

    /// `one-at-a-time` must hand over one steering message per model call: a
    /// response boundary that takes one may not be followed by the top-of-loop
    /// poll taking the next before the request it belongs to.
    #[tokio::test]
    async fn one_at_a_time_delivers_one_steering_message_per_model_call() {
        let (addr, server) = sse_server(vec![
            answer_body("one"),
            answer_body("two"),
            answer_body("three"),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        let runtime = test_runtime().await;
        runtime.steering.push(Message::user("STEER_ONE"));
        runtime.steering.push(Message::user("STEER_TWO"));
        runtime.steering.push(Message::user("STEER_THREE"));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            std::env::temp_dir(),
            vec![Message::user("hi")],
            tx,
            runtime,
        )
        .await;

        let bodies = server.await.unwrap();
        assert_eq!(bodies.len(), 3, "one request per delivery");
        assert!(bodies[0].contains("STEER_ONE"), "{}", bodies[0]);
        assert!(!bodies[0].contains("STEER_TWO"), "{}", bodies[0]);
        assert!(bodies[1].contains("STEER_TWO"), "{}", bodies[1]);
        assert!(!bodies[1].contains("STEER_THREE"), "{}", bodies[1]);
        assert!(bodies[2].contains("STEER_THREE"), "{}", bodies[2]);
        for body in &bodies {
            assert!(
                body.contains("not only the most recent one"),
                "a request with mid-run input lost the cumulative-input contract: {body}"
            );
        }

        let mut delivered = 0;
        while let Ok(event) = rx.try_recv() {
            if matches!(event, AgentEvent::Steered { .. }) {
                delivered += 1;
            }
        }
        assert_eq!(delivered, 3);
    }

    /// The cumulative-input reminder belongs to the request that carries the
    /// message it is about. Repeating it on every later step read as input the
    /// model could not find, and a run answered by narrating “the notices keep
    /// saying new messages arrived” instead of doing the work.
    #[tokio::test]
    async fn the_cumulative_reminder_rides_only_on_the_request_it_arrives_with() {
        let (addr, server) = sse_server(vec![bash_call_body("true"), answer_body("done")]).await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        let runtime = test_runtime().await;
        runtime.steering.push(Message::user("STEER"));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            std::env::temp_dir(),
            vec![Message::user("hi")],
            tx,
            runtime,
        )
        .await;

        let bodies = server.await.unwrap();
        assert_eq!(bodies.len(), 2, "the tool result is a second request");
        assert!(bodies[0].contains("STEER"), "{}", bodies[0]);
        assert!(
            bodies[0].contains("not only the most recent one"),
            "the request carrying the new message lost the contract: {}",
            bodies[0]
        );
        assert!(
            !bodies[1].contains("not only the most recent one")
                && !bodies[1].contains("has added one or more messages"),
            "a later request repeated the new-input notice: {}",
            bodies[1]
        );
    }

    /// A queued follow-up is not polled at the top of the loop: it enters at
    /// the response boundary, after the first answer. Keep that distinct path
    /// covered so its next request carries the same cumulative-input contract
    /// as an immediate steering message.
    #[tokio::test]
    async fn a_follow_up_at_the_response_boundary_gets_the_cumulative_reminder() {
        let (addr, server) = sse_server(vec![answer_body("one"), answer_body("two")]).await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        let runtime = test_runtime().await;
        runtime.follow_ups.push(Message::user("FOLLOW_UP"));
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            std::env::temp_dir(),
            vec![Message::user("hi")],
            tx,
            runtime,
        )
        .await;

        let bodies = server.await.unwrap();
        assert_eq!(bodies.len(), 2, "the follow-up gets its own request");
        assert!(!bodies[0].contains("FOLLOW_UP"), "{}", bodies[0]);
        assert!(bodies[1].contains("FOLLOW_UP"), "{}", bodies[1]);
        assert!(
            bodies[1].contains("not only the most recent one"),
            "the response-boundary follow-up lost the cumulative-input contract: {}",
            bodies[1]
        );
    }

    /// The regression from a real session: the model edited a file and
    /// summarized its work, oxide sent the hidden Definition-of-Done reminder,
    /// and the provider answered the reminder with an empty turn. The summary
    /// must survive as the result instead of the user reading
    /// `the model returned an empty response` after a complete answer.
    #[tokio::test]
    async fn an_empty_answer_to_the_verification_nudge_ends_the_run_quietly() {
        let dir = std::env::temp_dir().join(format!("oxide_nudge_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let edited = dir.join("catalog.yaml");
        let edited = edited.to_str().unwrap().to_string();

        let (addr, server) = sse_server(vec![
            write_call_body(&edited),
            answer_body("Upgrade complete."),
            empty_body(),
            empty_body(),
            empty_body(),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            auto_approve: true,
            ..Config::default()
        };
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            dir.clone(),
            vec![Message::user("upgrade the package")],
            tx,
            runtime,
        )
        .await;

        let mut errors = Vec::new();
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Error(message) => errors.push(message),
                AgentEvent::Finished(messages) => finished = Some(messages),
                _ => {}
            }
        }
        assert!(errors.is_empty(), "{errors:?}");
        let finished = finished.expect("the run finished");
        assert_eq!(
            finished.last().and_then(|message| message.display()),
            Some("Upgrade complete.".to_string())
        );

        // The reminder really was the request that produced the empty turn.
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 5);
        assert!(requests[2].contains("Before you finish"), "{}", requests[2]);
        let body: serde_json::Value =
            serde_json::from_str(requests[2].split_once("\r\n\r\n").unwrap().1).unwrap();
        let last = body["messages"].as_array().unwrap().last().unwrap();
        assert_eq!(last["role"], "user");
        assert!(last["content"]
            .as_str()
            .unwrap()
            .contains("Before you finish"));

        // The nudge repeats the one thing that outranks it, so it never pushes
        // a check the user asked to skip back onto the model.
        assert!(
            requests[2].contains("explicit instruction from the user outranks"),
            "{}",
            requests[2]
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A generic gateway's catalog is discovered independently of its name;
    /// only searched capabilities are declared on the following model call.
    #[tokio::test]
    async fn a_generic_gateway_defers_schemas_until_capability_search() {
        if !matches!(
            std::process::Command::new("python3")
                .arg("--version")
                .output(),
            Ok(output) if output.status.success()
        ) {
            return;
        }

        const INSTRUCTING_SERVER: &str = r#"
import sys, json
def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    method = msg.get("method")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"protocolVersion": "2024-11-05", "capabilities": {}, "serverInfo": {"name": "gateway", "version": "0"}, "instructions": "Read the complete thread before summarizing feedback"}})
    elif method == "tools/list":
        if msg.get("params", {}).get("cursor") == "second":
            result = {"tools": [{"name": "read_thread", "description": "Read a private Slack thread by channel and timestamp", "inputSchema": {"type": "object", "properties": {"channel": {"type": "string"}, "timestamp": {"type": "string"}}}}]}
        else:
            result = {"tools": [{"name": "query_database", "description": "Run SQL against a database", "inputSchema": {"type": "object", "properties": {"sql": {"type": "string"}}}}], "nextCursor": "second"}
        send({"jsonrpc": "2.0", "id": msg["id"], "result": result})
    elif method == "tools/call":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {"content": [{"type": "text", "text": "PERSONAL"}]}})
"#;

        let dir = std::env::temp_dir().join(format!("oxide_mcp_alias_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let script = dir.join("mock_mcp.py");
        std::fs::write(&script, INSTRUCTING_SERVER).unwrap();

        let (addr, server) = sse_server(vec![
            tool_call_body("tool_search", r#"{"query":"Slack read thread", "limit":1}"#),
            tool_call_body(
                "company-tools__read_thread",
                r#"{"channel":"C123", "timestamp":"123.456"}"#,
            ),
            answer_body("Read the feedback."),
        ])
        .await;
        let mut config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            auto_approve: true,
            ..Config::default()
        };
        config.ecosystem.mcp = vec![crate::ecosystem::McpServer {
            name: "company-tools".into(),
            enabled: true,
            kind: crate::ecosystem::McpKind::Local {
                command: vec!["python3".into(), script.display().to_string()],
                environment: Default::default(),
                cwd: None,
            },
            domains: vec![],
        }];
        let mut unavailable = config.ecosystem.mcp[0].clone();
        unavailable.name = "unavailable-gateway".into();
        unavailable.kind = crate::ecosystem::McpKind::Local {
            command: vec![dir.join("missing-server").display().to_string()],
            environment: Default::default(),
            cwd: None,
        };
        let mut disabled = unavailable.clone();
        disabled.name = "disabled-gateway".into();
        disabled.enabled = false;
        config.ecosystem.mcp.extend([unavailable, disabled]);
        let mut runtime = test_runtime().await;
        runtime.mcp = Arc::new(McpRegistry::new(&config.ecosystem.mcp));
        let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
        log.set_name("Review private feedback").unwrap();
        let user =
            Message::user("Read the feedback in https://acme.slack.com/archives/C123/p123456");
        log.append(&user).unwrap();
        runtime.session = Some(Arc::clone(&log));
        let shared = runtime.clone();
        let config_copy = config.clone();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(config, dir.clone(), vec![user], tx, runtime).await;

        let mut errors = Vec::new();
        while let Ok(event) = rx.try_recv() {
            if let AgentEvent::Error(message) = event {
                errors.push(message);
            }
        }
        assert!(errors.is_empty(), "{errors:?}");

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        let request_tools = |request: &str| {
            let body: Value = serde_json::from_str(
                request
                    .split_once("\r\n\r\n")
                    .map_or(request, |(_, body)| body),
            )
            .unwrap();
            body["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["function"]["name"].as_str().unwrap().to_string())
                .collect::<Vec<_>>()
        };
        assert!(request_tools(&requests[0]).contains(&"tool_search".into()));
        assert!(!request_tools(&requests[0]).contains(&"company-tools__read_thread".into()));
        assert!(request_tools(&requests[1]).contains(&"company-tools__read_thread".into()));
        assert!(!request_tools(&requests[1]).contains(&"company-tools__query_database".into()));
        assert!(requests[2].contains("PERSONAL"));
        assert!(requests[1].contains("unavailable-gateway"));
        assert!(!requests[1].contains("disabled-gateway"));
        assert!(requests[1].contains("Their tools have not been searched"));

        assert_eq!(log.selected_tools(), vec!["company-tools__read_thread"]);
        let reopened = SessionLog::open(log.path().to_path_buf()).unwrap();
        assert_eq!(reopened.selected_tools(), log.selected_tools());
        let again = search_mcp_tools(&config_copy, &shared, r#"{"query":"Slack read thread"}"#, 0)
            .await
            .unwrap();
        assert!(!again.contains("\n- company-tools__read_thread:"));
        let mut bounded = config_copy.clone();
        bounded.token_efficiency.max_declared_mcp_tools = 1;
        let replaced = search_mcp_tools(
            &bounded,
            &shared,
            r#"{"query":"SQL database","replace":true}"#,
            0,
        )
        .await
        .unwrap();
        assert!(replaced.contains("company-tools__query_database"));
        assert_eq!(log.selected_tools(), vec!["company-tools__query_database"]);
        let restored = search_mcp_tools(&bounded, &shared, r#"{"query":"Slack read thread"}"#, 0)
            .await
            .unwrap();
        assert!(restored.contains("1 previous tool schema(s) deferred"));
        assert_eq!(log.selected_tools(), vec!["company-tools__read_thread"]);
        search_mcp_tools(
            &bounded,
            &shared,
            r#"{"query":"zzzzzzzzzz","replace":true}"#,
            0,
        )
        .await
        .unwrap();
        let reopened = SessionLog::open(log.path().to_path_buf()).unwrap();
        assert_eq!(reopened.selected_tools_snapshot(), Some(vec![]));
        let specs = build_tool_specs(&bounded, &shared, 0, false);
        assert!(!specs
            .iter()
            .any(|tool| tool.function.name.contains("company-tools__")));

        let (resume_addr, resume_server) = sse_server(vec![answer_body("Done.")]).await;
        let mut resume_config = bounded;
        resume_config.base_url = format!("http://{resume_addr}");
        let resumed_user = Message::user("Continue without remote tools.");
        log.append(&resumed_user).unwrap();
        let (resume_tx, _resume_rx) = tokio::sync::mpsc::unbounded_channel();
        run(
            resume_config,
            dir.clone(),
            log.messages().unwrap(),
            resume_tx,
            shared.clone(),
        )
        .await;
        let resumed_requests = resume_server.await.unwrap();
        assert!(!request_tools(&resumed_requests[0])
            .iter()
            .any(|name| name.starts_with("company-tools__")));

        let invalid =
            search_mcp_tools(&config_copy, &shared, r#"{"query":"Slack", "limit":0}"#, 0).await;
        assert!(invalid.is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The re-check must extend the summary it asked about, not produce a
    /// second one. The run ends with one assistant turn that holds both the
    /// original answer and the verified answer, and the transcript keeps one
    /// bubble because the first answer's step is not committed until the
    /// re-check answers.
    #[tokio::test]
    async fn a_recheck_answer_is_merged_into_the_summary() {
        let dir = std::env::temp_dir().join(format!("oxide_merge_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let edited = dir.join("catalog.yaml");
        let edited = edited.to_str().unwrap().to_string();

        let (addr, server) = sse_server(vec![
            write_call_body(&edited),
            answer_body("Upgrade complete."),
            answer_body("Verified on disk."),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            auto_approve: true,
            ..Config::default()
        };
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            dir.clone(),
            vec![Message::user("upgrade the package")],
            tx,
            runtime,
        )
        .await;

        let mut errors = Vec::new();
        let mut finished = None;
        let mut texts = Vec::new();
        let mut thought_done = Vec::new();
        let mut usage = Vec::new();
        let mut index = 0;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Error(message) => errors.push(message),
                AgentEvent::Finished(messages) => finished = Some(messages),
                AgentEvent::Text(delta) => {
                    texts.push((index, delta));
                    index += 1;
                }
                AgentEvent::ThoughtDone { .. } => {
                    thought_done.push(index);
                    index += 1;
                }
                AgentEvent::Usage { .. } => {
                    usage.push(index);
                    index += 1;
                }
                _ => {}
            }
        }
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(texts.len(), 2, "both answers streamed: {texts:?}");
        assert_eq!(texts[0].1, "Upgrade complete.");
        assert_eq!(texts[1].1, "Verified on disk.");
        // No step boundary falls between the two answers, so the front-ends
        // paint one bubble and the re-check's text extends it. The usage event
        // is a boundary too (the VS Code transcript commits on it), so the held
        // step must not emit it until the merged answer is committed.
        assert!(
            !thought_done
                .iter()
                .any(|at| *at > texts[0].0 && *at < texts[1].0),
            "no step closed between the answers: {thought_done:?}"
        );
        assert!(
            !usage.iter().any(|at| *at > texts[0].0 && *at < texts[1].0),
            "usage did not commit the held step early: {usage:?}"
        );

        let finished = finished.expect("the run finished");
        let summaries: Vec<String> = finished
            .iter()
            .filter(|message| message.role == "assistant" && message.tool_calls.is_none())
            .filter_map(Message::display)
            .filter(|text| !text.trim().is_empty())
            .collect();
        assert_eq!(summaries.len(), 1, "one summary, not two: {summaries:?}");
        assert!(
            summaries[0].contains("Upgrade complete."),
            "{}",
            summaries[0]
        );
        assert!(
            summaries[0].contains("Verified on disk."),
            "{}",
            summaries[0]
        );

        // The reminder still was the second request.
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(requests[2].contains("Before you finish"), "{}", requests[2]);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_diff_of_the_edited_file_skips_the_verification_nudge() {
        let dir = std::env::temp_dir().join(format!("oxide_diff_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        // The reported session edited a catalog file and confirmed it with
        // `git diff <file>`. A real repository makes the inspection succeed and
        // show a hunk, which is the evidence the reminder asks for.
        init_git_repo(&dir);
        let edited = dir.join("catalog.yaml");
        let edited = edited.to_str().unwrap().to_string();

        // Write, inspect the result from the shell, summarize. The inspection is
        // the evidence the reminder asks for, so no reminder is sent and the
        // run needs one model call fewer than the version that nudged.
        let (addr, server) = sse_server(vec![
            write_call_body(&edited),
            bash_call_body("git diff catalog.yaml"),
            answer_body("Upgrade complete."),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            auto_approve: true,
            ..Config::default()
        };
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            dir.clone(),
            vec![Message::user("upgrade the package")],
            tx,
            runtime,
        )
        .await;

        let mut errors = Vec::new();
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Error(message) => errors.push(message),
                AgentEvent::Finished(messages) => finished = Some(messages),
                _ => {}
            }
        }
        assert!(errors.is_empty(), "{errors:?}");
        assert_eq!(
            finished
                .expect("the run finished")
                .last()
                .and_then(|m| m.display()),
            Some("Upgrade complete.".to_string())
        );

        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 3);
        assert!(
            !requests[2].contains("Before you finish"),
            "the shell inspection should have confirmed the edit: {}",
            requests[2]
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn command_tool_expands_and_reports_unknown() {
        let mut config = Config::default();
        config
            .ecosystem
            .commands
            .push(crate::ecosystem::CommandDef {
                name: "lint".into(),
                description: Some("lints the crate".into()),
                template: "Lint $ARGUMENTS now".into(),
                agent: None,
                subtask: false,
            });
        let runtime = test_runtime().await;
        let (events, _rx) = tokio::sync::mpsc::unbounded_channel();

        let output = command(
            &config,
            Path::new("."),
            &runtime,
            &events,
            r#"{"name":"/lint","arguments":"src"}"#,
            0,
        )
        .await;
        assert_eq!(output, "Lint src now");

        let output = command(
            &config,
            Path::new("."),
            &runtime,
            &events,
            r#"{"name":"missing"}"#,
            0,
        )
        .await;
        assert!(output.starts_with("error: unknown command"), "{output}");
    }

    #[test]
    fn command_spec_lists_commands_and_templates() {
        let mut config = Config::default();
        config
            .ecosystem
            .commands
            .push(crate::ecosystem::CommandDef {
                name: "lint".into(),
                description: Some("lints the crate".into()),
                template: "Lint $ARGUMENTS".into(),
                agent: None,
                subtask: false,
            });
        config
            .ecosystem
            .prompt_templates
            .push(crate::ecosystem::PromptTemplate {
                name: "component".into(),
                description: Some("creates a component".into()),
                argument_hint: None,
                body: "Create $1".into(),
            });
        let spec = command_spec(&config);
        assert_eq!(spec.function.name, "command");
        assert!(spec.function.description.contains("lint"));
        assert!(spec.function.description.contains("component"));
    }

    #[test]
    fn only_read_only_tools_are_concurrency_safe() {
        for name in [
            "read",
            "ls",
            "find",
            "grep",
            "webfetch",
            "memory",
            "skill",
            "diagnostics",
        ] {
            assert!(concurrency_safe(name), "{name} should be concurrency-safe");
        }
        for name in [
            "write",
            "edit",
            "bash",
            "task",
            "command",
            "mcp__server__tool",
        ] {
            assert!(!concurrency_safe(name), "{name} must stay sequential");
        }
    }

    fn tool_call(name: &str) -> ToolCall {
        ToolCall {
            id: format!("call_{name}"),
            kind: "function".into(),
            signature: None,
            function: crate::llm::FunctionCall {
                name: name.into(),
                arguments: "{}".into(),
            },
        }
    }

    #[test]
    fn batch_runs_group_reads_and_isolate_state_changes() {
        let runs = batch_runs(vec![
            tool_call("read"),
            tool_call("grep"),
            tool_call("write"),
            tool_call("read"),
            tool_call("edit"),
            tool_call("find"),
            tool_call("ls"),
        ]);
        let shape: Vec<Vec<&str>> = runs
            .iter()
            .map(|run| run.iter().map(|call| call.function.name.as_str()).collect())
            .collect();
        assert_eq!(
            shape,
            vec![
                vec!["read", "grep"],
                vec!["write"],
                vec!["read"],
                vec!["edit"],
                vec!["find", "ls"],
            ]
        );
    }

    #[test]
    fn batch_runs_keep_a_lone_read_sequential() {
        let runs = batch_runs(vec![tool_call("read")]);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].len(), 1);
    }

    #[test]
    fn snapshot_classification_covers_unknown_side_effects() {
        for name in ["write", "edit", "bash", "task", "server__tool"] {
            assert!(tool_may_mutate_workspace(name), "{name} may mutate");
        }
        for name in ["read", "ls", "find", "grep", "webfetch", "diagnostics"] {
            assert!(!tool_may_mutate_workspace(name), "{name} is read-only");
        }
    }

    #[test]
    fn batch_terminates_only_when_all_request_it() {
        assert!(!batch_terminates(&[]));
        assert!(!batch_terminates(&[true, false]));
        assert!(batch_terminates(&[true, true]));
    }

    #[tokio::test]
    async fn cancellation_wakes_every_waiter_and_is_seen_by_late_waiters() {
        let cancel = Cancel::new();
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let cancel = cancel.clone();
            tasks.push(tokio::spawn(async move {
                cancel.until(std::future::pending::<()>()).await
            }));
        }
        tokio::task::yield_now().await;
        cancel.cancel();
        for task in tasks {
            assert!(
                tokio::time::timeout(std::time::Duration::from_secs(1), task)
                    .await
                    .unwrap()
                    .unwrap()
                    .is_none()
            );
        }
        assert!(cancel
            .until(async { panic!("cancelled work must not start") })
            .await
            .is_none());
    }

    #[tokio::test]
    async fn stop_interrupts_a_stalled_stream_and_preserves_the_partial_reply() {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            read_request(&mut socket).await;
            socket.write_all(b"HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\"Partial reply\"}}]}\n\n").await.unwrap();
            std::future::pending::<()>().await;
        });
        let dir = std::env::temp_dir().join(format!("oxide_cancel_stream_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
        log.set_name("Existing thread").unwrap();
        log.append(&Message::user("hello")).unwrap();
        let mut runtime = test_runtime().await;
        runtime.session = Some(log.clone());
        let cancel = runtime.cancel.clone();
        let config = Config {
            provider: "openai".into(),
            model: "test-model".into(),
            api_key: "test".into(),
            base_url: format!("http://{address}"),
            ..Config::default()
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = tokio::spawn(run(
            config,
            dir.clone(),
            log.messages().unwrap(),
            tx,
            runtime,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while let Some(event) = rx.recv().await {
                if matches!(event, AgentEvent::Text(_)) {
                    break;
                }
            }
        })
        .await
        .expect("the partial reply must arrive before Stop");
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("Stop must not wait for the server")
            .unwrap();
        let mut finished = false;
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(event, AgentEvent::Error(_)), "{event:?}");
            if let AgentEvent::Finished(history) = event {
                assert_eq!(
                    history.last().unwrap().display().as_deref(),
                    Some("Partial reply")
                );
                finished = true;
            }
        }
        assert!(finished);
        assert_eq!(
            log.messages().unwrap().last().unwrap().display().as_deref(),
            Some("Partial reply")
        );
        server.abort();
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn stop_during_approval_records_the_whole_batch_and_the_session_resumes() {
        let arguments = json!({"path":"never-written.txt", "content":"x"}).to_string();
        let body = openai_sse(&[
            json!({"choices":[{"delta":{"tool_calls":[
                {"index":0,"id":"cancel_a","function":{"name":"write","arguments":arguments}},
                {"index":1,"id":"cancel_b","function":{"name":"write","arguments":arguments}}
            ]},"finish_reason":null}]}),
            json!({"choices":[{"delta":{},"finish_reason":"tool_calls"}]}),
        ]);
        let (addr, server) = sse_server(vec![body, answer_body("Resumed")]).await;
        let dir =
            std::env::temp_dir().join(format!("oxide_cancel_approval_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let log = Arc::new(SessionLog::create_in(&dir, &dir).unwrap());
        log.set_name("Existing thread").unwrap();
        log.append(&Message::user("write the file")).unwrap();
        let mut runtime = test_runtime().await;
        runtime.session = Some(log.clone());
        let cancel = runtime.cancel.clone();
        let (waiting, mut approvals) = tokio::sync::mpsc::unbounded_channel();
        runtime.approve = Arc::new(move |_, _| {
            let _ = waiting.send(());
            Box::pin(std::future::pending())
        });
        let config = Config {
            provider: "openai".into(),
            model: "test-model".into(),
            api_key: "test".into(),
            base_url: format!("http://{addr}"),
            auto_approve: false,
            ..Config::default()
        };
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = tokio::spawn(run(
            config.clone(),
            dir.clone(),
            log.messages().unwrap(),
            tx,
            runtime,
        ));
        tokio::time::timeout(std::time::Duration::from_secs(2), approvals.recv())
            .await
            .unwrap()
            .unwrap();
        cancel.cancel();
        tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
        let messages = log.messages().unwrap();
        let results: Vec<_> = messages
            .iter()
            .filter(|message| message.role == "tool")
            .collect();
        assert_eq!(results.len(), 2);
        assert!(!dir.join("never-written.txt").exists());
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(event, AgentEvent::Error(_)), "{event:?}");
        }
        log.append(&Message::user("continue")).unwrap();
        let mut runtime = test_runtime().await;
        runtime.session = Some(log.clone());
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        run(config, dir.clone(), log.messages().unwrap(), tx, runtime).await;
        assert_eq!(
            log.messages().unwrap().last().unwrap().display().as_deref(),
            Some("Resumed")
        );
        assert_eq!(server.await.unwrap().len(), 2);
        std::fs::remove_dir_all(dir).ok();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stop_interrupts_shell_children_before_they_can_keep_writing() {
        let dir = std::env::temp_dir().join(format!("oxide_cancel_shell_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let runtime = test_runtime().await;
        let cancel = runtime.cancel.clone();
        let task_dir = dir.clone();
        let handle = tokio::spawn(async move {
            let call = ToolCall { signature: None, id: "shell".into(), kind: "function".into(), function: crate::llm::FunctionCall { name: "bash".into(), arguments: json!({"command": "(while [ ! -f release ]; do sleep 0.01; done; touch should-not-exist) & echo ready > started; wait", "timeout": 30}).to_string() }};
            let (tx, _) = tokio::sync::mpsc::unbounded_channel();
            dispatch(
                &Config::default(),
                &task_dir,
                &runtime,
                &tx,
                &call,
                0,
                &tools::Progress::default(),
            )
            .await
        });
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while !dir.join("started").exists() {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        cancel.cancel();
        let output = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .unwrap()
            .unwrap();
        assert!(output.is_error && output.text.contains("cancelled"));
        std::fs::write(dir.join("release"), "go").unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        assert!(
            !dir.join("should-not-exist").exists(),
            "a shell child survived Stop"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn cancel_flag_is_shared_and_sticky() {
        let cancel = Cancel::new();
        assert!(!cancel.is_cancelled());
        let shared = cancel.clone();
        shared.cancel();
        assert!(cancel.is_cancelled());
        assert!(shared.is_cancelled());
    }

    #[tokio::test]
    async fn denied_permission_requests_approval() {
        let calls = Arc::new(AtomicUsize::new(0));
        let approve: Approver = Arc::new({
            let calls = Arc::clone(&calls);
            move |tool, subject| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async move { tool == "bash" && subject == "ls" })
            }
        });

        assert!(permission_granted(Action::Deny, false, &approve, "bash", "ls").await);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// The whole ask-and-answer path: a gated tool call surfaces an
    /// `ApprovalRequest` on the run's event stream, the answer releases it, and
    /// an `always` answer stops the next call from asking again.
    #[tokio::test]
    async fn an_always_answer_gates_the_tool_and_stops_the_next_prompt() {
        let dir = std::env::temp_dir().join(format!("oxide_approval_gate_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        let (addr, server) = sse_server(vec![
            bash_call_body("echo hello > out.txt"),
            bash_call_body("echo again > out2.txt"),
            answer_body("Done."),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            // Without this every `ask` rule is allowed without a prompt.
            auto_approve: false,
            ..Config::default()
        };
        let store = crate::approvals::ApprovalStore::load_from(dir.join("approvals.json"));
        let broker = Arc::new(crate::approval::ApprovalBroker::from_store(
            store,
            crate::approval::APPROVAL_TIMEOUT,
        ));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut runtime = test_runtime().await;
        runtime.approve = broker.approver(&dir, tx.clone(), Steering::new());

        let run_handle = tokio::spawn(run(
            config,
            dir.clone(),
            vec![Message::user("write the files")],
            tx,
            runtime,
        ));

        let mut requests = Vec::new();
        let mut finished = None;
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::ApprovalRequest { id, tool, detail } => {
                    requests.push((tool.clone(), detail.clone()));
                    assert!(broker.resolve(id, "always", None), "the request resolves");
                }
                AgentEvent::Finished(messages) => {
                    finished = Some(messages);
                    break;
                }
                _ => {}
            }
        }
        run_handle.await.unwrap();
        assert_eq!(server.await.unwrap().len(), 3);

        // The first call was described and approved; the second was released
        // by the saved rule instead of prompting again.
        assert_eq!(requests.len(), 1, "asked once: {requests:?}");
        assert_eq!(requests[0].0, "bash");
        assert_eq!(requests[0].1, "echo hello > out.txt");
        assert!(finished.is_some(), "the turn finished");
        assert!(dir.join("out.txt").is_file(), "the approved call ran");
        assert!(dir.join("out2.txt").is_file(), "the remembered rule ran");
        assert!(broker.list(&dir).contains(&"bash".to_string()));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// The whole question path: the model's `ask` call surfaces a
    /// `QuestionRequest` on the run's event stream, the front-end's answer
    /// releases it, and the answers come back as the tool result the model reads.
    #[tokio::test]
    async fn an_ask_call_waits_for_the_front_end_and_returns_its_answers() {
        let dir = std::env::temp_dir().join(format!("oxide_ask_tool_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();

        let arguments = json!({
            "questions": [{
                "question": "Which database?",
                "header": "Database",
                "options": [{"label": "Postgres"}, {"label": "SQLite"}]
            }]
        })
        .to_string();
        let (addr, server) = sse_server(vec![
            tool_call_body("ask", &arguments),
            answer_body("SQLite it is."),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            ..Config::default()
        };

        let broker = crate::ask::AskBroker::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut runtime = test_runtime().await;
        runtime.ask = Some(broker.asker(tx.clone()));

        let run_handle = tokio::spawn(run(
            config,
            dir.clone(),
            vec![Message::user("set up the project")],
            tx,
            runtime,
        ));

        let mut asked = Vec::new();
        let mut result = None;
        while let Some(event) = rx.recv().await {
            match event {
                AgentEvent::QuestionRequest { id, questions } => {
                    asked.push(questions.clone());
                    assert!(broker.resolve(
                        id,
                        vec![crate::ask::Answer {
                            question: questions[0].question.clone(),
                            values: vec!["SQLite".into()],
                        }]
                    ));
                }
                AgentEvent::ToolResult { name, output, .. } if name == "ask" => {
                    result = Some(output)
                }
                AgentEvent::Finished(_) => break,
                _ => {}
            }
        }
        run_handle.await.unwrap();
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 2);

        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0][0].question, "Which database?");
        assert_eq!(asked[0][0].options.len(), 2);
        let result = result.expect("the tool result is reported");
        assert!(result.contains("Which database? = SQLite"), "{result}");
        // The answer reaches the model in the next request, not just the view.
        let second = &requests[1];
        assert!(second.contains("SQLite"), "{second}");

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A run with nobody to answer is never offered the tool, so the model
    /// cannot ask and then wait out the timeout.
    #[tokio::test]
    async fn ask_is_not_offered_without_an_asker() {
        let config = Config::default();
        let runtime = test_runtime().await;
        let names: Vec<String> = build_tool_specs(&config, &runtime, 0, false)
            .into_iter()
            .map(|spec| spec.function.name)
            .collect();
        assert!(!names.contains(&"ask".to_string()), "{names:?}");

        let mut runtime = runtime;
        runtime.ask = Some(Arc::new(|questions| {
            Box::pin(async move {
                Some(Reply::Answers(vec![crate::ask::Answer {
                    question: questions[0].question.clone(),
                    values: vec!["yes".into()],
                }]))
            })
        }));
        let names: Vec<String> = build_tool_specs(&config, &runtime, 0, false)
            .into_iter()
            .map(|spec| spec.function.name)
            .collect();
        assert!(names.contains(&"ask".to_string()), "{names:?}");

        // The tool reports the answers a runtime whose asker replies with them,
        // and refuses a call with no questions rather than asking nothing.
        let output = ask(&runtime, r#"{"questions":[{"question":"Proceed?"}]}"#).await;
        assert!(output.text.contains("Proceed? = yes"), "{:?}", output.text);
        assert!(!output.is_error);
        let output = ask(&runtime, "{}").await;
        assert!(output.is_error, "{:?}", output.text);
        assert!(
            output.text.contains("needs a `question` string"),
            "{:?}",
            output.text
        );
    }

    #[tokio::test]
    async fn allowed_and_auto_approved_permissions_skip_prompt() {
        let calls = Arc::new(AtomicUsize::new(0));
        let approve: Approver = Arc::new({
            let calls = Arc::clone(&calls);
            move |_, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                Box::pin(async { false })
            }
        });

        assert!(permission_granted(Action::Allow, false, &approve, "read", "a.rs").await);
        assert!(permission_granted(Action::Deny, true, &approve, "bash", "ls").await);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn edits_stay_unverified_until_read_back_or_built() {
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/main.rs"}), "ok");
        state.record("write", &json!({"path": "src/lib.rs"}), "ok");
        assert_eq!(state.edited.len(), 2);
        assert!(state.reminder().is_some());
        // The reminder is sent only once.
        assert!(state.reminder().is_none());

        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/main.rs"}), "ok");
        state.record("read", &json!({"path": "src/main.rs"}), "ok");
        assert!(state.edited.is_empty());
        assert!(state.reminder().is_none());

        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/main.rs"}), "ok");
        state.record("bash", &json!({"command": "./gradlew test"}), "[exit: 0]");
        assert!(state.edited.is_empty());
        assert!(state.reminder().is_none());

        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/main.rs"}), "ok");
        state.record("read", &json!({"path": "/repo/src/main.rs"}), "ok");
        assert!(state.edited.is_empty());
        assert!(state.reminder().is_none());

        // A failed edit is not something to verify.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/main.rs"}), "error: no match");
        assert!(state.edited.is_empty());
    }

    #[test]
    fn failed_reads_do_not_confirm_edits() {
        for tool in ["read", "diagnostics"] {
            for (output, is_error) in [
                ("error: permission denied", true),
                ("File not found", true),
                ("[exit: 1]", false),
            ] {
                let mut state = VerificationState::default();
                state.record("edit", &json!({"path": "README.md"}), "ok");
                state.record_result(tool, &json!({"path": "README.md"}), output, is_error);
                assert!(state.reminder().is_some(), "{tool}: {output}");
                state.record_result(tool, &json!({"path": "README.md"}), "new content", false);
                assert!(state.edited.is_empty());
            }
        }
    }

    #[test]
    fn a_shell_inspection_of_the_edited_file_confirms_it() {
        let diff = "diff --git a/pnpm-workspace.yaml b/pnpm-workspace.yaml\n\
                    @@ -55,7 +55,7 @@\n[exit: 0]";
        // `git diff <path>` shows the change is on disk, so it needs no nudge.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "/repo/pnpm-workspace.yaml"}), "ok");
        state.record(
            "bash",
            &json!({"command": "cd /repo && git diff pnpm-workspace.yaml"}),
            diff,
        );
        assert!(state.edited.is_empty());
        assert!(state.reminder().is_none());

        // So does grepping the file for the new content.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "/repo/src/main.rs"}), "ok");
        state.record(
            "bash",
            &json!({"command": "grep -n \"fn main\" src/main.rs"}),
            "12:fn main() {\n[exit: 0]",
        );
        assert!(state.edited.is_empty());

        // Only the named file is confirmed, and only by a successful command.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "/repo/a.rs"}), "ok");
        state.record("edit", &json!({"path": "/repo/b.rs"}), "ok");
        state.record("bash", &json!({"command": "git diff a.rs"}), diff);
        assert_eq!(state.edited.iter().collect::<Vec<_>>(), vec!["/repo/b.rs"]);

        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "/repo/a.rs"}), "ok");
        state.record("bash", &json!({"command": "git diff a.rs"}), "[exit: 128]");
        assert_eq!(state.edited.len(), 1);

        // A diff of the whole tree does not show whether the hunk was included
        // in the truncated output, and a plain read-only listing is not a look
        // at the content, so neither counts.
        assert!(!inspected_by_shell("git diff", diff, "/repo/a.rs"));
        assert!(!inspected_by_shell(
            "ls -la a.rs",
            "/repo/a.rs\n[exit: 0]",
            "/repo/a.rs"
        ));
        assert!(!inspected_by_shell("git diff a.rs", diff, "/repo/b.rs"));
        assert!(inspected_by_shell("git diff a.rs", diff, "/repo/a.rs"));
        assert!(inspected_by_shell(
            "git show HEAD -- a.rs",
            diff,
            "/repo/a.rs"
        ));

        // A named path is not evidence when the command only mentions it — as
        // the *pattern* a searcher matches, a glob it filters names by, or a
        // diff that shows nothing — because none of those read the change.
        let empty = "[exit: 0]";
        assert!(!inspected_by_shell(
            "grep -rn \"a.rs\" .",
            empty,
            "/repo/a.rs"
        ));
        assert!(!inspected_by_shell(
            "rg --files -g a.rs",
            empty,
            "/repo/a.rs"
        ));
        assert!(!inspected_by_shell(
            "git diff --staged a.rs",
            empty,
            "/repo/a.rs"
        ));
        // `git show HEAD:a.rs` reads the committed blob, not the working file,
        // so it says nothing about an uncommitted edit.
        assert!(!inspected_by_shell(
            "git show HEAD:a.rs",
            diff,
            "/repo/a.rs"
        ));
    }

    #[test]
    fn a_nudge_that_produces_no_answer_ends_the_run_quietly() {
        // The model already summarized its work, so a provider that answers the
        // reminder with nothing must not fail the turn with an error the user
        // reads after a complete answer (see the empty-response regression).
        let empty = anyhow::Error::new(crate::llm::NoAnswer::empty());
        assert!(nudge_failed_quietly(true, &empty));
        // A real failure is still reported, and an ordinary turn never gets
        // this leniency.
        assert!(!nudge_failed_quietly(false, &empty));
        assert!(!nudge_failed_quietly(
            true,
            &anyhow::anyhow!("provider returned 401")
        ));
    }

    #[test]
    fn a_queued_message_keeps_a_held_summary_separate() {
        // The held summary extends only the answer that immediately follows it
        // with nothing queued in between. A steering or follow-up message
        // queued during the re-check commits the summary as its own turn, so
        // the re-check answer lands after the user's message.
        assert!(recheck_extends_summary(true, false));
        assert!(!recheck_extends_summary(true, true));
        assert!(!recheck_extends_summary(false, false));
        assert!(!recheck_extends_summary(false, true));
    }

    #[test]
    fn requested_inline_replies_need_each_posted_reply_read_back() {
        let mut state = VerificationState::default();
        state.request_replies("Fix and reply PR's comments");
        assert!(state.blocks_summary_reply("gh pr comment 9 --body-file reply.md"));
        assert!(!state.blocks_summary_reply("echo 'gh pr comment 9'"));
        assert!(!state.blocks_summary_reply("gh pr comment --help"));
        assert!(!state.blocks_summary_reply(
            "gh api -X POST repos/org/repo/pulls/9/comments/10/replies -f body=fixed"
        ));
        let listing = "gh api --paginate repos/org/repo/pulls/9/comments";
        let comments =
            r#"[{"id":10,"path":"a.rs"},{"id":20,"path":"b.rs"},{"id":21,"in_reply_to_id":20}]"#;
        state.record("bash", &json!({"command": listing}), comments);
        state.record(
            "bash",
            &json!({"command": "gh pr comment 9 --body-file reply.md"}),
            "posted\n[exit: 0]",
        );
        state.record(
            "bash",
            &json!({"command": "gh pr view 9 --json comments"}),
            "summary\n[exit: 0]",
        );
        let reminder = state.reminder().unwrap();
        assert!(reminder.contains("10, 20"), "{reminder}");
        state.nudged = false;
        for (root, reply) in [(10, 30), (20, 40)] {
            let command = format!(
                "gh api -X POST repos/org/repo/pulls/9/comments/{root}/replies -f body=fixed"
            );
            let posted = json!({"id": reply, "in_reply_to_id": root}).to_string();
            state.record("bash", &json!({"command": command}), &posted);
            assert!(!state.confirmed_replies.contains(&root));
            state.record("bash", &json!({"command": listing}), &posted);
            assert!(state.confirmed_replies.contains(&root));
        }
        assert!(state.reminder().is_none());
        assert!(!state.blocks_summary_reply("gh pr comment 9 --body-file summary.md"));
    }

    #[test]
    fn reply_checks_respect_scope_and_failed_posts() {
        let mut state = VerificationState::default();
        state.request_replies("Review the PR comments");
        assert!(!state.replies_requested);
        assert!(state.reminder().is_none());
        state.request_replies("Reply to the review comments");
        assert!(state.replies_requested);
        let command = "gh api -X POST repos/org/repo/pulls/9/comments/10/replies -f body=fixed";
        state.record_result(
            "bash",
            &json!({"command": command}),
            r#"{"id":30,"in_reply_to_id":10}"#,
            true,
        );
        assert!(state.posted_replies.is_empty());
        state.request_replies("Don't post replies to these comments");
        assert!(!state.replies_requested);
    }

    #[test]
    fn summary_restrictions_do_not_cancel_inline_reply_requests() {
        let mut state = VerificationState::default();
        state.request_replies(
            "Reply to every inline review comment, but don't post a top-level summary",
        );
        assert!(state.replies_requested);
        assert!(state.summaries_forbidden);
        assert!(state.inline_replies_pending());
        assert!(state.blocks_summary_reply("gh pr comment 9 --body done"));
        state.record(
            "bash",
            &json!({"command": "gh api repos/o/r/pulls/9/comments"}),
            "[]\n[exit: 0]",
        );
        assert!(!state.inline_replies_pending());
        assert!(state.reminder().is_none());
        assert!(state.blocks_summary_reply("gh pr comment 9 --body done"));
    }

    #[test]
    fn general_discussions_and_empty_inline_discovery_do_not_block_comments() {
        let mut state = VerificationState::default();
        state.request_replies("Reply to the reviewer's general PR comment");
        assert!(!state.replies_requested);
        assert!(!state.blocks_summary_reply("gh pr comment 9 --body reply"));
        state.request_replies("Reply to the inline review comments");
        assert!(!state.review_comments_discovered);
        assert!(state.blocks_summary_reply("gh pr comment 9 --body reply"));
        state.record(
            "bash",
            &json!({"command": "gh api repos/o/r/pulls/9/comments"}),
            "[]\n[exit: 0]",
        );
        assert!(state.review_comments_discovered);
        assert!(!state.blocks_summary_reply("gh pr comment 9 --body reply"));
        assert!(state.reminder().is_none());
    }

    #[test]
    fn inline_reply_targets_follow_the_requested_comment_ids() {
        let mut state = VerificationState::default();
        state.request_replies("Fix and reply only to inline comment 10");
        let listing = json!({"command": "gh api repos/o/r/pulls/9/comments"});
        state.record(
            "bash",
            &listing,
            r#"[{"id":10,"path":"a.rs"},{"id":20,"path":"b.rs"}]"#,
        );
        assert_eq!(state.reply_targets(), BTreeSet::from([10]));
        state.record(
            "bash",
            &json!({"command": "gh api repos/o/r/pulls/9/comments/10/replies -f body=fixed"}),
            r#"{"id":30,"in_reply_to_id":10}"#,
        );
        assert!(state.inline_replies_pending());
        state.record("bash", &listing, r#"[{"id":30,"in_reply_to_id":10}]"#);
        assert!(!state.inline_replies_pending());
        assert!(!state.blocks_summary_reply("gh pr comment 9 --body done"));
        assert!(state.reminder().is_none());
        state.request_replies("Reply to inline comments 10, 20 and 40");
        assert_eq!(
            state.requested_review_comments,
            Some(BTreeSet::from([10, 20, 40]))
        );
    }

    #[test]
    fn implicit_posts_are_tracked_but_explicit_gets_are_not() {
        assert!(posts_review_reply(
            "gh api repos/o/r/pulls/9/comments/10/replies -f body='fixed; example --method GET'"
        ));
        assert!(!posts_review_reply(
            "echo 'gh api repos/o/r/pulls/9/comments/10/replies -f body=fixed'"
        ));
        for fields in [
            "-f body=fixed",
            "-F body=fixed",
            "--raw-field body=fixed",
            "--field=body=fixed",
            "-fbody=fixed",
        ] {
            let command = format!("gh api repos/o/r/pulls/9/comments/10/replies {fields}");
            assert!(posts_review_reply(&command), "{command}");
            let mut state = VerificationState::default();
            assert!(state.blocks_review_reply(&command, true));
            state.record(
                "bash",
                &json!({"command": command}),
                r#"{"id":30,"in_reply_to_id":10}"#,
            );
            assert_eq!(state.posted_replies.get(&30), Some(&10));
            for method in ["--method GET", "-X GET", "--method=GET", "-XGET"] {
                let get = format!("{command} {method}");
                assert!(!posts_review_reply(&get), "{get}");
                let mut state = VerificationState::default();
                state.record(
                    "bash",
                    &json!({"command": get}),
                    r#"{"id":30,"in_reply_to_id":10}"#,
                );
                assert!(state.posted_replies.is_empty());
                assert!(state.confirmed_replies.is_empty());
            }
        }
    }

    #[test]
    fn a_specific_comment_can_be_verified_without_listing_unrelated_threads() {
        let mut state = VerificationState::default();
        state.request_replies("Reply to inline comment 10");
        state.record(
            "bash",
            &json!({"command": "gh api repos/o/r/pulls/9/comments/10/replies -f body=fixed"}),
            r#"{"id":30,"in_reply_to_id":10}"#,
        );
        state.record(
            "bash",
            &json!({"command": "gh api repos/o/r/pulls/comments/30"}),
            r#"{"id":30,"in_reply_to_id":10}"#,
        );
        assert!(!state.review_comments_discovered);
        assert!(!state.inline_replies_pending());
        assert!(state.reminder().is_none());
    }

    #[test]
    fn side_effects_are_confirmed_before_done() {
        // A pull request is unconfirmed until its checks or state are read.
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr create --fill"}),
            "[exit: 0]",
        );
        assert!(state.pending.contains("pull request"));
        let reminder = state.reminder().unwrap();
        assert!(reminder.contains("gh pr checks"), "{reminder}");
        // The reminder is the fallback for a run that missed the prompt's own
        // rule, so it asks for the link in its own words.
        assert!(reminder.contains("[#123](url)"), "{reminder}");
        assert!(reminder.contains("--json url"), "{reminder}");
        assert!(state.reminder().is_none());

        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr create --fill"}),
            "[exit: 0]",
        );
        state.record(
            "bash",
            &json!({"command": "gh pr view 42 --json state"}),
            "OPEN",
        );
        assert!(state.pending.is_empty());
        assert!(state.reminder().is_none());

        // A failed command is not an outcome that needs checking.
        let mut state = VerificationState::default();
        state.record("bash", &json!({"command": "gh pr create"}), "[exit: 1]");
        assert!(state.pending.is_empty());
        assert!(state.reminder().is_none());

        assert!(output_failed("error: nope"));
        assert!(output_failed("[exit: 1]"));
        assert!(!output_failed("[exit: 0]"));
    }

    /// `gh pr checks` exits 8 while CI runs, which `tools::bash` reports as a
    /// result rather than a failure. That read is not the outcome the rule
    /// waits for, so the pull request stays unconfirmed until one lands.
    #[test]
    fn a_still_pending_check_does_not_confirm_the_pull_request() {
        let pending = format!("build\tpending\n[exit: 8] ({})", tools::PENDING_CHECKS_NOTE);
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr create --fill"}),
            "[exit: 0]",
        );
        state.record("bash", &json!({"command": "gh pr checks 42"}), &pending);
        assert!(state.pending.contains("pull request"));
        assert!(state.reminder().is_some());

        // The same read once a check has reported clears it.
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr create --fill"}),
            "[exit: 0]",
        );
        state.record(
            "bash",
            &json!({"command": "gh pr checks 42"}),
            "build\tpass\n[exit: 0]",
        );
        assert!(state.pending.is_empty());
        assert!(state.reminder().is_none());
    }

    #[test]
    fn done_rules_cover_more_than_pull_requests() {
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "kubectl apply -f k8s.yaml"}),
            "[exit: 0]",
        );
        assert!(state.pending.contains("deployment"));
        state.record(
            "bash",
            &json!({"command": "kubectl rollout status deploy/api"}),
            "ok",
        );
        assert!(state.pending.is_empty());

        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr comment 5 --body hi"}),
            "[exit: 0]",
        );
        assert!(state.pending.contains("comment"));
        let reminder = state.reminder().unwrap();
        assert!(reminder.contains("Read the thread back"), "{reminder}");

        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh release create v1.0"}),
            "[exit: 0]",
        );
        assert!(state.pending.contains("release"));

        // Ordinary git commands are not side effects needing a separate check.
        let mut state = VerificationState::default();
        state.record("bash", &json!({"command": "git commit -m x"}), "[exit: 0]");
        assert!(state.pending.is_empty());
    }

    #[test]
    fn blanket_staging_requires_a_scope_review() {
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "git add -A && git commit -m x"}),
            "[exit: 0]",
        );
        assert!(state.pending.contains("scope"));
        let reminder = state.reminder().unwrap();
        assert!(reminder.contains("blanket flag"), "{reminder}");

        let mut state = VerificationState::default();
        state.record("bash", &json!({"command": "git add -A"}), "[exit: 0]");
        state.record("bash", &json!({"command": "git diff --staged"}), "...");
        assert!(state.pending.is_empty());

        // Staging explicit paths is scoped and needs no review.
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "git add src/main.rs"}),
            "[exit: 0]",
        );
        assert!(state.pending.is_empty());
    }

    #[test]
    fn review_edits_are_not_delivered_until_pushed() {
        // A review reply after an edit must be followed by a push, or the
        // branch under review still shows the old code.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        state.record(
            "bash",
            &json!({"command": "gh pr comment 5 --body 'fixed'"}),
            "[exit: 0]",
        );
        let reminder = state.reminder().unwrap();
        assert!(reminder.contains("git push"), "{reminder}");
        assert!(state.reminder().is_none());

        // A push before the reply already delivered the edit, so no nudge to
        // push again.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        state.record("bash", &json!({"command": "git push"}), "[exit: 0]");
        state.record(
            "bash",
            &json!({"command": "gh pr comment 5 --body hi"}),
            "[exit: 0]",
        );
        let reminder = state.reminder().unwrap_or_default();
        assert!(!reminder.contains("git push"), "{reminder}");

        // An edit made after the reply needs its own push.
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr review 5 --comment"}),
            "[exit: 0]",
        );
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        assert!(state.reminder().unwrap().contains("git push"));

        // A review with no code change has nothing to push.
        let mut state = VerificationState::default();
        state.record(
            "bash",
            &json!({"command": "gh pr comment 5 --body hi"}),
            "[exit: 0]",
        );
        let reminder = state.reminder().unwrap_or_default();
        assert!(!reminder.contains("git push"), "{reminder}");
    }

    #[test]
    fn a_write_beside_the_run_directory_is_still_the_repository() {
        let root = std::env::temp_dir().join(format!(
            "oxide_verify_beside_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(repo.join(".git")).unwrap();
        let run = repo.join("crates/core");
        std::fs::create_dir_all(&run).unwrap();
        std::fs::write(repo.join("docs.md"), "docs").unwrap();
        std::fs::write(root.join("scratch.md"), "scratch").unwrap();

        // The run's directory is a subdirectory of the repository. A file edited
        // beside it is the tracked modification `git status` reports to the
        // other half of the guard, so a push delivers it and the reply waits.
        let mut state = VerificationState::new(&run);
        state.record(
            "write",
            &json!({ "path": repo.join("docs.md").to_string_lossy() }),
            "wrote 4 bytes",
        );
        assert!(state.blocks_review_reply("gh pr comment 5 --body fixed", false));

        // Outside the repository is still nothing a push delivers.
        state.record("bash", &json!({ "command": "git push" }), "[exit: 0]");
        state.record(
            "write",
            &json!({ "path": root.join("scratch.md").to_string_lossy() }),
            "wrote 7 bytes",
        );
        assert!(!state.blocks_review_reply("gh pr comment 5 --body fixed", false));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_write_outside_the_run_directory_is_not_a_delivery() {
        let root = std::env::temp_dir().join(format!(
            "oxide_verify_scope_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let project = root.join("project");
        std::fs::create_dir_all(project.join("src")).unwrap();
        std::fs::write(root.join("reply.md"), "reply").unwrap();
        std::fs::write(project.join("src/b.rs"), "fn b() {}\n").unwrap();

        // A scratch file written beside the project — the body of a review
        // reply, a report, a script — is not on the branch under review, so it
        // neither holds the reply nor asks to be confirmed.
        let mut state = VerificationState::new(&project);
        state.record(
            "write",
            &json!({ "path": root.join("reply.md").to_string_lossy() }),
            "wrote 640 bytes",
        );
        assert!(!state.blocks_review_reply(
            "gh api -X POST repos/o/r/pulls/5/comments/1/replies -f body=x",
            false
        ));
        assert!(state.reminder().is_none(), "{:?}", state.reminder());

        // The project's own files still are: relative, and by absolute path.
        state.record("write", &json!({ "path": "src/a.rs" }), "ok");
        assert!(state.blocks_review_reply("gh pr comment 5 --body fixed", false));
        state.record("bash", &json!({ "command": "git push" }), "[exit: 0]");
        state.record(
            "edit",
            &json!({ "path": project.join("src/b.rs").to_string_lossy() }),
            "ok",
        );
        assert!(state.blocks_review_reply("gh pr comment 5 --body fixed", false));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn review_replies_are_held_until_the_code_is_pushed() {
        assert!(posts_review_reply("gh pr comment 5 --body fixed"));
        assert!(posts_review_reply(
            "gh api -X POST repos/o/r/pulls/5/comments/1/replies -f body=x"
        ));
        // Reading a thread is a GET, not a posted reply.
        assert!(!posts_review_reply(
            "gh api repos/o/r/pulls/5/comments/1/replies"
        ));
        assert!(!posts_review_reply("gh pr view 5 --comments"));
        assert!(!posts_review_reply("git push"));

        // A reply while a tracked edit is unpushed is held.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        assert!(state.blocks_review_reply("gh pr comment 5 --body fixed", false));

        // Repository state holds it even when no tracked edit was seen.
        let mut state = VerificationState::default();
        assert!(state.blocks_review_reply("gh pr comment 5 --body fixed", true));

        // A real push unblocks the tracked edit.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        state.record("bash", &json!({"command": "git push"}), "[exit: 0]");
        assert!(!state.blocks_review_reply("gh pr comment 5 --body fixed", false));

        // `echo git push` is not a push, and does not clear the guard.
        let mut state = VerificationState::default();
        state.record("edit", &json!({"path": "src/a.rs"}), "ok");
        state.record("bash", &json!({"command": "echo git push"}), "[exit: 0]");
        assert!(state.blocks_review_reply("gh pr comment 5 --body fixed", false));

        // A reply with no pending code change is allowed.
        let mut state = VerificationState::default();
        assert!(!state.blocks_review_reply("gh pr comment 5 --body hi", false));
    }

    #[tokio::test]
    async fn repo_delivery_pending_tracks_uncommitted_and_unpushed_work() {
        let dir = std::env::temp_dir().join(format!("oxide_pending_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        add_git_remote(&dir);
        assert!(!repo_has_pending_delivery(&dir).await, "clean and pushed");

        std::fs::write(dir.join("catalog.yaml"), "changed\n").unwrap();
        assert!(repo_has_pending_delivery(&dir).await, "uncommitted change");

        run_git(&dir, &["add", "catalog.yaml"]);
        run_git(&dir, &["commit", "-q", "-m", "fix"]);
        assert!(repo_has_pending_delivery(&dir).await, "unpushed commit");

        run_git(&dir, &["push"]);
        assert!(!repo_has_pending_delivery(&dir).await, "pushed");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(format!("{}.origin.git", dir.display())).ok();
    }

    #[tokio::test]
    async fn repo_delivery_ignores_a_pushed_branch_the_remote_tracking_ref_misses() {
        let dir = std::env::temp_dir().join(format!("oxide_pending_sb_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        add_git_remote(&dir);

        // Simulate a single-branch clone: the fetch refspec maps only the
        // default branch, so pushing `feature` never creates `origin/feature`.
        let default = git_text(&dir, &["symbolic-ref", "--short", "HEAD"]);
        run_git(
            &dir,
            &[
                "config",
                "remote.origin.fetch",
                &format!("+refs/heads/{default}:refs/remotes/origin/{default}"),
            ],
        );
        run_git(&dir, &["checkout", "-q", "-b", "feature"]);
        std::fs::write(dir.join("catalog.yaml"), "feature\n").unwrap();
        run_git(&dir, &["add", "catalog.yaml"]);
        run_git(&dir, &["commit", "-q", "-m", "feature"]);
        assert!(
            repo_has_pending_delivery(&dir).await,
            "an unpushed feature commit is held"
        );

        run_git(&dir, &["push", "-q", "origin", "feature"]);
        // The pushed branch is not visible locally...
        assert!(
            std::process::Command::new("git")
                .args(["rev-parse", "--verify", "refs/remotes/origin/feature"])
                .current_dir(&dir)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .expect("git runs")
                .code()
                .is_none_or(|code| code != 0),
            "no remote-tracking ref for the feature branch"
        );
        // ...but asking the remote clears the guard.
        assert!(
            !repo_has_pending_delivery(&dir).await,
            "a commit already on the remote is delivered"
        );

        // A second commit lands on the remote branch, then the local branch is
        // rewound behind it: the older local commit is still delivered because
        // it is an ancestor of the remote tip.
        std::fs::write(dir.join("catalog.yaml"), "later\n").unwrap();
        run_git(&dir, &["add", "catalog.yaml"]);
        run_git(&dir, &["commit", "-q", "-m", "later"]);
        run_git(&dir, &["push", "-q", "origin", "feature"]);
        run_git(&dir, &["reset", "-q", "--hard", "HEAD~1"]);
        assert!(
            !repo_has_pending_delivery(&dir).await,
            "a commit behind the remote tip is still delivered"
        );

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(format!("{}.origin.git", dir.display())).ok();
    }

    #[tokio::test]
    async fn a_review_reply_before_the_push_is_held() {
        let dir = std::env::temp_dir().join(format!("oxide_reply_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        init_git_repo(&dir);
        add_git_remote(&dir);
        let edited = dir.join("catalog.yaml");
        let edited = edited.to_str().unwrap().to_string();

        let (addr, server) = sse_server(vec![
            write_call_body(&edited),
            // The reply is refused while the edit is uncommitted.
            bash_call_body("gh pr comment 5 --body 'fixed'"),
            bash_call_body("git diff catalog.yaml"),
            // Committing is not enough: the commit is not pushed yet.
            bash_call_body("git add catalog.yaml && git commit -m 'fix'"),
            // ...so the reply is still refused.
            bash_call_body("gh pr comment 5 --body 'fixed'"),
            // Pushing the commit delivers it.
            bash_call_body("git push"),
            // Now the reply is allowed.
            bash_call_body("gh pr comment 5 --body 'fixed'"),
            bash_call_body("gh pr view 5 --comments"),
            answer_body("Done."),
        ])
        .await;
        let config = Config {
            provider: "deepseek".into(),
            model: "deepseek-flash".into(),
            base_url: format!("http://{addr}"),
            api_key: "sk-test".into(),
            auto_approve: true,
            ..Config::default()
        };
        let runtime = test_runtime().await;
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        run(
            config,
            dir.clone(),
            vec![Message::user("address the review")],
            tx,
            runtime,
        )
        .await;

        let mut errors = Vec::new();
        let mut finished = None;
        while let Ok(event) = rx.try_recv() {
            match event {
                AgentEvent::Error(message) => errors.push(message),
                AgentEvent::Finished(messages) => finished = Some(messages),
                _ => {}
            }
        }
        assert!(errors.is_empty(), "{errors:?}");
        let requests = server.await.unwrap();
        assert_eq!(requests.len(), 9);
        assert!(
            requests[2].contains("commit and push the code changes before replying"),
            "{}",
            requests[2]
        );
        assert!(
            requests[5].contains("commit and push the code changes before replying"),
            "{}",
            requests[5]
        );
        assert!(
            !requests[8].contains("Before you finish"),
            "{}",
            requests[8]
        );
        assert_eq!(
            finished
                .expect("the run finished")
                .last()
                .and_then(|m| m.display()),
            Some("Done.".to_string())
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn same_path_ignores_the_separator_style() {
        assert!(same_path(r"C:\repo\src\main.rs", "src/main.rs"));
        assert!(same_path(r"C:\repo\src\main.rs", r"src\main.rs"));
        assert!(same_path("/repo/a.rs", "/repo/a.rs"));
        assert!(!same_path(r"C:\repo\a.rs", "b.rs"));
    }

    #[test]
    fn repeated_verifiers_share_a_key_across_output_plumbing() {
        let base = verification_key("./gradlew test --tests '*Boutique*'").unwrap();
        assert_eq!(
            verification_key("./gradlew test --tests '*Boutique*' 2>&1 | tail -50").unwrap(),
            base
        );
        assert_eq!(
            verification_key("./gradlew test --tests '*Boutique*' | grep PASSED | wc -l").unwrap(),
            base
        );
        // A different verifier is a different key, and non-verifiers have none.
        assert_ne!(
            verification_key("./gradlew test --tests '*Villa*'").unwrap(),
            base
        );
        assert!(verification_key("git status").is_none());
        assert!(verification_key("echo hello").is_none());

        // A pipe inside a quoted selector is part of the key, not plumbing.
        let quoted = verification_key("cargo test -- --exact 'foo|bar'").unwrap();
        assert_eq!(quoted, "cargo test -- --exact 'foo|bar'");
        assert_ne!(
            verification_key("cargo test -- --exact 'foo'").unwrap(),
            quoted
        );
    }

    #[test]
    fn a_denied_verifier_is_not_recorded_as_run() {
        let mut seen = BTreeSet::new();
        // A denied call never dispatched, so it must not count as a run.
        assert!(!note_verifier(&mut seen, false, "cargo test"));
        // The first real run is new; repeating it is flagged.
        assert!(!note_verifier(&mut seen, true, "cargo test"));
        assert!(note_verifier(&mut seen, true, "cargo test 2>&1 | tail -5"));
        assert!(!note_verifier(&mut seen, true, "cargo build"));
    }

    #[test]
    fn only_a_successful_mutation_invalidates_a_verifier() {
        // A successful edit makes the prior build/test result stale, so the next
        // run must be treated as fresh.
        assert!(mutation_invalidates_verifier(
            "edit",
            "applied 1 replacement"
        ));
        assert!(mutation_invalidates_verifier("write", "wrote a.rs"));
        // A failed mutation left the workspace unchanged and must not.
        assert!(!mutation_invalidates_verifier(
            "edit",
            "error: oldText not found"
        ));
        assert!(!mutation_invalidates_verifier("write", "error: denied"));
        // Non-mutating tools never invalidate.
        assert!(!mutation_invalidates_verifier("bash", "ok"));
        assert!(!mutation_invalidates_verifier("read", "1|line"));
    }

    #[test]
    fn commands_that_verify_are_recognized() {
        assert!(looks_like_verification_command("./gradlew test"));
        assert!(looks_like_verification_command("cargo build"));
        assert!(looks_like_verification_command("npm run lint"));
        assert!(looks_like_verification_command("mvn verify"));
        assert!(looks_like_verification_command("./gradlew spotlessCheck"));
        assert!(looks_like_verification_command(
            "./gradlew testDebugUnitTest"
        ));
        assert!(!looks_like_verification_command("./gradlew spotlessApply"));
        assert!(!looks_like_verification_command(
            "gh api repos/o/r/pulls/1/comments"
        ));
        assert!(!looks_like_verification_command("git status"));
        assert!(!looks_like_verification_command("git log --grep latest"));
    }

    #[test]
    fn inspecting_test_code_does_not_count_as_running_a_verifier() {
        for command in [
            "grep -n test src/main.rs",
            "rg 'build|test' crates/core",
            "sed -n 1,40p src/test/dialogs.test.ts",
            "cat test/results.txt",
            "echo cargo test",
            "printf '%s' 'npm run lint'",
            "git diff -- src/test.rs",
            "gh pr view 224 --json statusCheckRollup",
            "node -e 'console.log(\"test\")'",
            "printf ok | grep test",
        ] {
            assert!(!looks_like_verification_command(command), "{command}");
            assert!(verification_key(command).is_none(), "{command}");
        }
        for command in [
            "node crates/desktop/check-app.mjs",
            "cd editors/vscode && pnpm test",
            "env CI=1 cargo test",
            "python3 tests/check_contract.py",
            "./scripts/check.sh",
        ] {
            assert!(looks_like_verification_command(command), "{command}");
        }
        let mut state = VerificationState::new(Path::new("/repo"));
        state.record(
            "edit",
            &json!({"path": "a.rs"}),
            "Successfully replaced 1 block(s).",
        );
        state.record(
            "bash",
            &json!({"command": "grep test b.rs"}),
            "test\n[exit: 0]",
        );
        assert!(state.edited.contains("a.rs"));
    }

    #[test]
    fn ruff_formatting_does_not_confirm_edits_or_count_as_a_verifier() {
        for command in [
            "ruff format src/main.py",
            "ruff format tests/check.py",
            "ruff --config pyproject.toml format src/main.py",
            "ruff format -- --check",
            "ruff --version",
            "ruff help check",
            "ruff check --help",
            "ruff check --show-settings",
            "ruff check --show-files",
            "python3 -m ruff format src/main.py",
        ] {
            assert!(!looks_like_verification_command(command), "{command}");
            assert!(verification_key(command).is_none(), "{command}");
            let mut seen = BTreeSet::new();
            assert!(!note_verifier(&mut seen, true, command), "{command}");
            assert!(!note_verifier(&mut seen, true, command), "{command}");
            let mut state = VerificationState::new(Path::new("/repo"));
            state.record("edit", &json!({"path": "src/main.py"}), "ok");
            state.record(
                "bash",
                &json!({"command": command}),
                "1 file reformatted\n[exit: 0]",
            );
            assert!(state.edited.contains("src/main.py"), "{command}");
        }
        for command in [
            "ruff check src/main.py",
            "ruff format --check src/main.py",
            "ruff --config pyproject.toml check .",
            "python3 -m ruff check .",
            "python3 -m ruff format --check .",
        ] {
            assert!(looks_like_verification_command(command), "{command}");
            assert!(verification_key(command).is_some(), "{command}");
        }
    }

    #[test]
    fn interpreter_options_preserve_verification_and_repeat_tracking() {
        for command in [
            "node --test out/test/",
            "node --test",
            "node --check app.js",
            "node -c app.js",
            "node --no-warnings scripts/check.js",
            "node --require setup.js scripts/check.js",
            "node --import setup.mjs --test out/test/",
            "python3 -m pytest",
            "python3 -mpytest tests/",
            "python3 -W error -X dev -m pytest tests/",
            "python3 -m unittest discover",
            "python3 -u scripts/check.py",
            "bash -e scripts/check.sh",
            "bash -euo pipefail scripts/check.sh",
            "bash -n scripts/deploy.sh",
            "sh -eu scripts/check.sh",
        ] {
            assert!(looks_like_verification_command(command), "{command}");
            assert!(verification_key(command).is_some(), "{command}");
            let mut seen = BTreeSet::new();
            assert!(!note_verifier(&mut seen, true, command), "{command}");
            assert!(note_verifier(&mut seen, true, command), "{command}");
            let mut state = VerificationState::new(Path::new("/repo"));
            state.record("edit", &json!({"path": "a.rs"}), "ok");
            state.record(
                "bash",
                &json!({"command": command}),
                "checks passed\n[exit: 0]",
            );
            assert!(state.edited.is_empty(), "{command}");
        }
        for command in [
            "node -e 'test()'",
            "node --eval='test()'",
            "node --print 'test()'",
            "node --test --eval 'test()'",
            "node app.js --test",
            "node --require scripts/check.js app.js",
            "node --help --test",
            "python3 -c 'test()'",
            "python3 -u -c 'test()'",
            "python3 -ctest",
            "python3 -m",
            "python3 app.py -m pytest",
            "bash -c 'scripts/check.sh'",
            "bash -ec 'scripts/check.sh'",
            "bash -o pipefail -c 'cargo test'",
            "bash deploy.sh -e scripts/check.sh",
        ] {
            assert!(!looks_like_verification_command(command), "{command}");
            assert!(verification_key(command).is_none(), "{command}");
        }
    }

    #[test]
    fn steering_queue_drains_one_at_a_time_in_order() {
        let steering = Steering::new();
        steering.push(Message::user("first"));
        steering.push(Message::user("second"));
        let drained = steering.drain(QueueMode::OneAtATime);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].display().as_deref(), Some("first"));
        let drained = steering.drain(QueueMode::OneAtATime);
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].display().as_deref(), Some("second"));
        assert!(steering.drain(QueueMode::OneAtATime).is_empty());
    }

    #[test]
    fn all_mode_drains_the_whole_queue() {
        let steering = Steering::new();
        steering.push(Message::user("first"));
        steering.push(Message::user("second"));
        assert_eq!(steering.drain(QueueMode::All).len(), 2);
        assert!(steering.drain(QueueMode::All).is_empty());
        assert_eq!(QueueMode::parse("all"), Some(QueueMode::All));
        assert_eq!(
            QueueMode::parse("one-at-a-time"),
            Some(QueueMode::OneAtATime)
        );
        assert_eq!(QueueMode::parse("nonsense"), None);
    }

    #[test]
    fn follow_ups_are_separate_from_steering() {
        let steering = Steering::new();
        let follow_ups = Steering::new();
        steering.push(Message::user("steer"));
        follow_ups.push(Message::user("later"));
        assert_eq!(steering.drain(QueueMode::OneAtATime).len(), 1);
        assert_eq!(follow_ups.drain(QueueMode::OneAtATime).len(), 1);
        assert!(steering.drain(QueueMode::OneAtATime).is_empty());
        assert!(follow_ups.drain(QueueMode::OneAtATime).is_empty());
    }

    #[test]
    fn an_empty_response_boundary_rejects_late_messages() {
        let steering = Steering::new();
        let follow_ups = Steering::new();

        let (steered, queued) = steering.drain_at_response(
            &follow_ups,
            QueueMode::OneAtATime,
            QueueMode::OneAtATime,
            true,
        );

        assert!(steered.is_empty());
        assert!(queued.is_empty());
        assert!(!steering.push(Message::user("too late")));
        assert!(!follow_ups.push(Message::user("too late")));
    }

    #[test]
    fn a_response_boundary_takes_work_without_closing_the_turn() {
        let steering = Steering::new();
        let follow_ups = Steering::new();
        assert!(steering.push(Message::user("change course")));

        let (steered, queued) = steering.drain_at_response(
            &follow_ups,
            QueueMode::OneAtATime,
            QueueMode::OneAtATime,
            true,
        );

        assert_eq!(steered.len(), 1);
        assert!(queued.is_empty());
        assert!(follow_ups.push(Message::user("then do this")));
    }

    #[test]
    fn an_internal_recheck_keeps_empty_input_queues_open() {
        let steering = Steering::new();
        let follow_ups = Steering::new();

        let (steered, queued) = steering.drain_at_response(
            &follow_ups,
            QueueMode::OneAtATime,
            QueueMode::OneAtATime,
            false,
        );

        assert!(steered.is_empty());
        assert!(queued.is_empty());
        assert!(steering.push(Message::user("arrived during recheck")));
    }

    #[tokio::test]
    async fn an_isolated_subtask_rejects_follow_ups_for_the_next_primary_turn() {
        let runtime = test_runtime().await;
        let follow_ups = runtime.follow_ups.clone();
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();

        let future = run_subagent(
            Config::default(),
            std::env::temp_dir(),
            Vec::new(),
            "missing".into(),
            "review it".into(),
            tx,
            runtime,
        );

        assert!(!follow_ups.push(Message::user("do this next")));
        drop(future);
    }

    #[tokio::test]
    async fn an_exceptional_finish_retains_already_accepted_messages() {
        let runtime = test_runtime().await;
        assert!(runtime.steering.push(Message::user("correct this")));
        assert!(runtime.follow_ups.push(Message::user("then do this")));
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();

        finish(
            &runtime,
            &tx,
            vec![Message::assistant("stopped", vec![])],
            0,
        );

        let AgentEvent::Finished(messages) = rx.recv().await.unwrap() else {
            panic!("finish emits the completed history");
        };
        let text: Vec<_> = messages
            .iter()
            .filter_map(|message| message.display())
            .collect();
        assert_eq!(text, ["stopped", "correct this", "then do this"]);
        assert!(!runtime.steering.push(Message::user("too late")));
        assert!(!runtime.follow_ups.push(Message::user("too late")));
    }

    #[test]
    fn memory_tool_add_search_forget() {
        let root = std::env::temp_dir().join(format!("oxide_agent_mem_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let config = Config {
            memory: crate::memory::MemoryStore::open(root.clone(), &root),
            ..Default::default()
        };

        let added = memory(
            &config,
            r#"{"mode":"add","content":"prefer anyhow for errors"}"#,
        );
        assert!(added.starts_with("stored memory"), "{added}");

        let found = memory(&config, r#"{"mode":"search","query":"anyhow errors"}"#);
        assert!(found.contains("prefer anyhow for errors"), "{found}");

        let listed = memory(&config, r#"{"mode":"list"}"#);
        assert!(listed.contains("prefer anyhow for errors"));

        std::fs::remove_dir_all(&root).ok();
    }
}
