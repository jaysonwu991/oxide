//! Commands backing the desktop window.

use crate::approval::ApprovalBroker;
use crate::ask::AskBroker;
use crate::bridge::{EventSink, Host};
use anyhow::Context;
use oxide_core::agent::{AgentEvent, Cancel, Steering};
use oxide_core::auth::{self, AuthStore};
use oxide_core::catalog;
use oxide_core::cli::{event_json, session_header};
use oxide_core::config::Config;
use oxide_core::diff::{Diff, LineKind};
use oxide_core::llm::LlmClient;
use oxide_core::llm::Message;
use oxide_core::llm::{ContentPart, MessageContent};
use oxide_core::session::{SessionLog, SessionSummary};
use oxide_core::snapshots::Snapshots;
use oxide_core::theme_view;
use oxide_core::update_notice;
use oxide_core::updates::Component;
use oxide_desktop::at::{AtAnswer, PathCache};
use oxide_desktop::manager::{expand_project_path, DesktopManager, ProjectView};
use oxide_desktop::turn::{notify_finished, open_session, start_turn, Turn};
use oxide_desktop::update;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::AbortHandle;

type CmdResult<T> = Result<T, String>;

fn err(error: impl std::fmt::Display) -> String {
    error.to_string()
}

/// A running turn, reachable by id from the cancel/steer commands.
struct RunHandle {
    abort: AbortHandle,
    steering: Steering,
    follow_ups: Steering,
    cancel: Cancel,
    /// The project the run was started in, so a message steered into it reads
    /// its own `@path` references against the same directory.
    cwd: PathBuf,
}

pub struct DesktopState {
    pub manager: Mutex<DesktopManager>,
    pub approvals: Arc<ApprovalBroker>,
    pub questions: Arc<AskBroker>,
    /// The `@path` completion's listing of the open project, walked once and
    /// dropped when a turn ends (see `oxide_desktop::at`).
    pub at: PathCache,
    /// The newest word the launch's own install has for the window, as the event
    /// it would have heard. See `LaunchUpdate`.
    pub launch_update: std::sync::Mutex<Option<LaunchUpdate>>,
    runs: Arc<Mutex<HashMap<u64, RunHandle>>>,
    next_run: AtomicU64,
    events: EventSink,
}

impl DesktopState {
    pub fn new(manager: DesktopManager, events: EventSink) -> Self {
        Self {
            manager: Mutex::new(manager),
            approvals: Arc::new(ApprovalBroker::new(events.clone())),
            questions: Arc::new(AskBroker::new(events.clone())),
            at: PathCache::default(),
            launch_update: std::sync::Mutex::new(None),
            runs: Arc::new(Mutex::new(HashMap::new())),
            next_run: AtomicU64::new(1),
            events,
        }
    }

    /// Keeps the newest word from the install this process is running and hands
    /// it to the window, which may not be listening for it yet.
    fn announce_launch_update(&self, event: &'static str, payload: Value) {
        let mut slot = self
            .launch_update
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *slot = Some(LaunchUpdate {
            event,
            payload: payload.clone(),
        });
        let _ = self.events.emit(event, payload);
    }
}

/// The newest thing the install this process is running has said, as the window
/// would have heard it.
///
/// The launch's own install starts from `setup`, before the page has loaded and
/// subscribed to the event channel, so its first events can be emitted into a
/// window that has nothing listening — and the one that must not be lost is the
/// report that a release is in place, since the restart that runs it is offered
/// nowhere else. Keeping the newest event beside the state lets a window that
/// has just started listening ask what it missed and paint exactly what it
/// would have heard. The dialog's own install travels the same way, so a window
/// never has two accounts of one install.
#[derive(Clone, Debug)]
pub struct LaunchUpdate {
    /// The event's own name: `update-available`, `update-progress`,
    /// `update-ready` or `update-failed`.
    pub event: &'static str,
    pub payload: Value,
}

fn message_view(message: &Message) -> Value {
    json!({
        "role": message.role,
        "content": message.display().unwrap_or_default(),
        "attachments": message_attachments(message),
        "toolCalls": message
            .tool_calls
            .as_ref()
            .map(|calls| calls
                .iter()
                .map(|call| json!({
                    "id": call.id,
                    "name": call.function.name,
                    "arguments": call.function.arguments,
                }))
                .collect::<Vec<_>>())
            .unwrap_or_default(),
        "toolCallId": message.tool_call_id,
    })
}

/// Media the message carried, so a reopened thread can still preview it instead
/// of reducing it to the `[image]` marker in `content`.
fn message_attachments(message: &Message) -> Vec<Value> {
    let Some(MessageContent::Parts(parts)) = &message.content else {
        return Vec::new();
    };
    parts
        .iter()
        .filter_map(|part| match part {
            ContentPart::ImageUrl { image_url } => Some(json!({
                "name": "image",
                "dataUrl": image_url.url,
            })),
            ContentPart::File { file } => Some(json!({
                "name": file.filename.clone().unwrap_or_else(|| "document".into()),
                "dataUrl": file.file_data,
            })),
            ContentPart::Text { .. } => None,
        })
        .collect()
}

// ---------- projects ----------

/// A media or text file the desktop attached from the clipboard or a file
/// picker. A pasted image has no on-disk path in the webview, so the bytes
/// travel as a data URL.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInput {
    pub data_url: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// The system clipboard as an attachment for the page to add, in the shape of
/// the attachments it already sends back (a data URL and the name it is shown
/// by).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardAttachment {
    pub name: String,
    pub data_url: String,
}

/// The system clipboard as an attachment, for a paste the webview could not
/// read itself: macOS refuses a webview's read of a file in the Desktop,
/// Documents or Downloads folder, but the harness reads the pasteboard through
/// the same `media::clipboard` the terminal's Ctrl+V uses, so what the
/// pasteboard itself carries is attached instead. `None` for a text paste or an
/// empty clipboard.
pub fn read_clipboard() -> CmdResult<Option<ClipboardAttachment>> {
    Ok(oxide_core::media::clipboard_media()
        .map(|(name, data_url)| ClipboardAttachment { name, data_url }))
}

/// A refused attachment fails the send instead of vanishing from the message:
/// a payload that is not base64, one past the limit the core enforces, or a
/// payload that is neither an image, a PDF nor text (a video, a tarball).
///
/// An `@path` reference to an image or a PDF that is in the project rides along
/// the same way, which is what the terminal does with one — the reference stays
/// in the text either way, so the model sees the path and, where the file is
/// media, the file itself.
fn attachment_parts(
    attachments: Option<Vec<AttachmentInput>>,
    message: &str,
    cwd: &Path,
) -> Result<Vec<ContentPart>, String> {
    let mut parts = Vec::new();
    for attachment in attachments.unwrap_or_default() {
        let name = attachment
            .name
            .clone()
            .unwrap_or_else(|| "attachment".to_string());
        let part =
            oxide_core::media::content_part_from_data_url(attachment.data_url, attachment.name)
                .ok_or_else(|| {
                    let limit = oxide_core::media::MAX_ATTACHMENT_BYTES / (1024 * 1024);
                    format!(
                        "{name} could not be attached: attach an image, a PDF or a text file of at most {limit} MB"
                    )
                })?;
        parts.push(part);
    }
    for path in oxide_core::media::referenced_attachments(message, cwd) {
        let name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let part = oxide_core::media::load_attachment(&path)
            .map_err(|error| format!("{name} could not be attached: {error}"))?;
        parts.push(part);
    }
    Ok(parts)
}

/// Every project: folders added here plus ones discovered from sessions.
pub async fn list_projects(state: &DesktopState) -> CmdResult<Vec<ProjectView>> {
    state.manager.lock().await.overview().map_err(err)
}

pub async fn add_project(path: String, state: &DesktopState) -> CmdResult<AddProjectResult> {
    let mut manager = state.manager.lock().await;
    let project = manager
        .add_project(&expand_project_path(&path))
        .map_err(err)?;
    let projects = manager.overview().map_err(err)?;
    Ok(AddProjectResult {
        projects,
        added: project.id,
    })
}

/// The refreshed project list plus the id of the folder that was added, so the
/// UI can select it without guessing from the typed path.
#[derive(serde::Serialize)]
pub struct AddProjectResult {
    pub projects: Vec<ProjectView>,
    pub added: String,
}

pub async fn remove_project(id: String, state: &DesktopState) -> CmdResult<Vec<ProjectView>> {
    let mut manager = state.manager.lock().await;
    manager.remove_project(&id).map_err(err)?;
    manager.overview().map_err(err)
}

/// Provider/model resolved from the same `config.json` the CLI uses, plus the
/// project's trust state so the UI can prompt before loading project resources.
pub async fn project_info(project: String, state: &DesktopState) -> CmdResult<Value> {
    let manager = state.manager.lock().await;
    let path = PathBuf::from(&project);
    let config = manager.config_for(&path).map_err(err)?;
    Ok(project_info_value(&config, &path))
}

/// Saves a trust decision for a project (the desktop equivalent of the CLI's
/// `/trust`) and returns the refreshed project info.
pub async fn set_project_trust(
    project: String,
    trusted: bool,
    state: &DesktopState,
) -> CmdResult<Value> {
    let path = PathBuf::from(&project);
    oxide_desktop::manager::set_project_trust(&path, trusted).map_err(err)?;
    let manager = state.manager.lock().await;
    let config = manager.config_for(&path).map_err(err)?;
    Ok(project_info_value(&config, &path))
}

/// The levels the active model's own listing advertised, or `null` when it
/// advertised none (so a front-end keeps its built-in set rather than treating
/// the name heuristic's guess as if the model had said it).
fn advertised_reasoning_levels(config: &Config) -> Value {
    let advertised = config
        .reasoning_supported
        .as_ref()
        .is_some_and(|meta| !meta.supported.is_empty());
    if advertised {
        json!(config
            .reasoning_levels()
            .iter()
            .map(|level| level.label())
            .collect::<Vec<_>>())
    } else {
        Value::Null
    }
}

fn project_info_value(config: &Config, project: &Path) -> Value {
    let trust = oxide_desktop::manager::project_trust(config, project);
    json!({
        "provider": config.provider,
        "model": config.model,
        "reasoning": config.reasoning.label(),
        "reasoningLevels": advertised_reasoning_levels(config),
        "supportsReasoning": config.supports_reasoning(),
        "contextWindow": config.context_window(),
        "hasKey": !config.api_key.is_empty(),
        "trust": trust,
    })
}

/// The active model's reasoning levels, for the picker the thinking chip opens.
/// The model cache is warmed from the provider's listing when it is cold, so the
/// picker narrows the first time it is opened rather than only after a model
/// catalog was fetched.
pub async fn reasoning_levels(project: String, state: &DesktopState) -> CmdResult<Value> {
    let mut config = {
        let manager = state.manager.lock().await;
        manager.config_for(&PathBuf::from(&project)).map_err(err)?
    };
    if config.reasoning_supported.is_none() {
        // Bypasses the TTL cache: a fresh entry the provider answered without
        // effort metadata must not keep the model's own levels hidden.
        let _ = LlmClient::new(config.clone()).refresh_models().await;
        config.reasoning_supported =
            oxide_core::llm::cached_model_reasoning(&config, &config.model);
    }
    Ok(json!({
        "reasoning": config.reasoning.label(),
        "reasoningLevels": advertised_reasoning_levels(&config),
        "supportsReasoning": config.supports_reasoning(),
    }))
}

// ---------- mcp servers ----------

/// The MCP servers visible from `project` with their connection state: what
/// `/mcp` lists. The view is the same one the CLI prints and the VS Code
/// extension draws, so all three agree on names, transports and statuses.
pub async fn mcp_servers(project: String) -> CmdResult<Vec<oxide_core::mcp_config::ServerView>> {
    let cwd = project_dir(&project)?;
    Ok(oxide_core::mcp_config::server_views(&cwd).await)
}

/// Turns an MCP server off (or back on) in the file that defines it, then
/// returns the re-probed list so the modal can redraw from one round trip.
pub async fn set_mcp_server(
    project: String,
    name: String,
    enabled: bool,
) -> CmdResult<Vec<oxide_core::mcp_config::ServerView>> {
    let cwd = project_dir(&project)?;
    oxide_core::mcp_config::set_enabled(&cwd, None, name, enabled).map_err(err)?;
    Ok(oxide_core::mcp_config::server_views(&cwd).await)
}

// ---------- slash commands ----------

/// The `/` palette: the built-in client commands plus the agents, commands and
/// skills this project loads. Draws the same catalog the CLI's autocomplete and
/// the extension's menu do, from the shared `oxide_core::commands`.
pub async fn list_commands(project: String) -> CmdResult<Vec<oxide_core::commands::CommandEntry>> {
    palette_entries(&project)
}

/// The rows for a project, or — with none open, which is where the window starts
/// — the built-ins alone. A command, a prompt template and a skill are read from
/// a folder, while the built-ins are what the app performs itself, so the home
/// state's `/` lists those instead of answering with nothing.
fn palette_entries(project: &str) -> Result<Vec<oxide_core::commands::CommandEntry>, String> {
    if project.trim().is_empty() {
        return Ok(oxide_core::commands::builtin_entries());
    }
    Ok(oxide_core::commands::palette(&project_dir(project)?))
}

/// A project root a command can read. An empty one has no folder behind it, and
/// `PathBuf::from("")` is the process's own working directory — a relative
/// path, so the listing (and a toggle) would land in whatever directory the app
/// was launched from instead of the project the window shows.
fn project_dir(project: &str) -> Result<PathBuf, String> {
    if project.trim().is_empty() {
        return Err("select a project first".to_string());
    }
    Ok(PathBuf::from(project))
}

// ---------- sessions ----------

pub async fn list_sessions(
    project: String,
    state: &DesktopState,
) -> CmdResult<Vec<SessionSummary>> {
    state
        .manager
        .lock()
        .await
        .sessions_for(&PathBuf::from(project))
        .map_err(err)
}

/// Sessions across every project, newest first (the cross-repo view).
pub async fn all_sessions(state: &DesktopState) -> CmdResult<Vec<SessionSummary>> {
    state.manager.lock().await.all_sessions().map_err(err)
}

/// The stored transcript for one session.
pub async fn session_messages(project: String, id: String) -> CmdResult<Value> {
    let cwd = PathBuf::from(project);
    let log = SessionLog::open_ref(&cwd, &id).map_err(err)?;
    let messages = log.messages().map_err(err)?;
    let totals = log.usage_totals();
    Ok(json!({
        "header": session_header(&log),
        "messages": messages.iter().map(message_view).collect::<Vec<_>>(),
        "usage": {
            "input": totals.input,
            "output": totals.output,
            "cacheRead": totals.cache_read,
            "cacheWrite": totals.cache_write,
            "cost": totals.cost,
            "cacheHitRate": totals.cache_hit_rate,
            "messageCount": messages.len(),
        },
    }))
}

pub async fn rename_session(project: String, id: String, name: String) -> CmdResult<()> {
    SessionLog::rename(&PathBuf::from(project), &id, &name).map_err(err)
}

pub async fn delete_session(project: String, id: String) -> CmdResult<()> {
    SessionLog::delete(&PathBuf::from(project), &id).map_err(err)
}

// ---------- providers ----------

/// Known providers with their stored-credential state, as the core's own
/// picker listing has it: the desktop app, the VS Code panel and the terminal's
/// `/connect` draw the same rows.
pub async fn list_providers() -> CmdResult<Vec<auth::ProviderView>> {
    Ok(auth::provider_views())
}

/// Stores (or reuses) a provider credential and persists the selection in the
/// same `auth.json` / `config.json` the CLI uses.
pub async fn login(
    provider: String,
    key: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
) -> CmdResult<auth::LoginOutcome> {
    auth::login_provider(
        &provider,
        key.as_deref().unwrap_or_default(),
        model.as_deref(),
        base_url.as_deref(),
        Path::new("."),
    )
    .map_err(err)
}

pub async fn logout(provider: String) -> CmdResult<bool> {
    let mut store = AuthStore::load().map_err(err)?;
    let removed = store.remove(&provider);
    store.save().map_err(err)?;
    Ok(removed)
}

// ---------- turns ----------

/// Starts a turn in the background and returns its run id immediately, so the
/// UI can cancel or steer it while it streams.
pub async fn send_prompt(
    state: Arc<DesktopState>,
    project: String,
    prompt: String,
    session: Option<String>,
    reasoning: Option<String>,
    attachments: Option<Vec<AttachmentInput>>,
) -> CmdResult<u64> {
    let run_id = state.next_run.fetch_add(1, Ordering::Relaxed);
    let attachments = attachment_parts(attachments, &prompt, Path::new(&project))?;
    tokio::spawn(async move {
        let _ = drive_turn(
            state,
            run_id,
            project,
            prompt,
            session,
            reasoning,
            attachments,
        )
        .await;
    });
    Ok(run_id)
}

#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    state: Arc<DesktopState>,
    run_id: u64,
    project: String,
    prompt: String,
    session: Option<String>,
    reasoning: Option<String>,
    attachments: Vec<ContentPart>,
) -> anyhow::Result<()> {
    let cwd = PathBuf::from(project);
    let reference = session.as_deref().unwrap_or("latest");
    let log = open_session(&cwd, reference)?;
    // The state the run starts from, so the files it changes can be listed and
    // undone once it ends. `None` when the project must not be snapshotted — a
    // directory that holds everything (the home directory or an ancestor of it,
    // the config directory) or one too large to hash — in which case the window
    // shows no change card for the turn rather than failing to start it. A
    // project that is not a git clone is snapshotted all the same.
    let baseline = mark_baseline(&cwd).await;
    let approver = state.approvals.approver(cwd.clone());
    let asker = state.questions.asker_for(run_id);
    let turn = start_turn(
        &cwd,
        &prompt,
        log,
        Some(approver),
        Some(asker),
        reasoning,
        attachments,
    )
    .await?;
    let Turn {
        session_id,
        title,
        mut events,
        handle,
        steering,
        follow_ups,
        cancel,
    } = turn;
    let stopped = cancel.clone();
    state.runs.lock().await.insert(
        run_id,
        RunHandle {
            abort: handle.abort_handle(),
            steering,
            follow_ups,
            cancel,
            cwd: cwd.clone(),
        },
    );
    let _ = state.events.emit(
        "agent-start",
        json!({ "runId": run_id, "sessionId": session_id, "title": title }),
    );

    while let Some(event) = events.recv().await {
        if let Some(mut value) = event_json(&event) {
            if let AgentEvent::ToolResult {
                diff: Some(diff), ..
            } = &event
            {
                value["diff"] = serde_json::to_value(diff)?;
            }
            value["runId"] = json!(run_id);
            let _ = state.events.emit("agent-event", value);
        }
        if matches!(event, AgentEvent::Finished(_)) {
            break;
        }
    }

    state.runs.lock().await.remove(&run_id);
    // The turn is over, so the files it wrote are on disk: drop the completion's
    // listing rather than offering paths from before the work. A question it
    // left waiting can never be answered either: drop it here rather than
    // letting it sit until its timeout, and before `agent-end` so the window is
    // never told a turn ended while a request of its own is still open.
    state.at.clear();
    state.questions.clear_run(run_id).await;
    // The listing, and the state the turn left behind: the project it belongs to
    // travels with both, so a window that switched projects mid-turn can tell
    // the card is not its own and an undo can check nothing came after it.
    let (baseline, after, changes) = match baseline {
        Some((snapshots, base)) => {
            let listed = tokio::task::spawn_blocking({
                let base = base.clone();
                let snapshots = snapshots.clone();
                move || snapshots.changes_since(&base).ok()
            })
            .await
            .ok()
            .flatten();
            let after = tokio::task::spawn_blocking(move || snapshots.mark_named("turn").ok())
                .await
                .ok()
                .flatten();
            (Some(base), after, listed)
        }
        None => (None, None, None),
    };
    let _ = state.events.emit(
        "agent-end",
        json!({
            "runId": run_id,
            "sessionId": session_id,
            "project": cwd.to_string_lossy(),
            "baseline": baseline,
            "after": after,
            "changes": changes,
        }),
    );
    notify_finished(&cwd, session_id.as_deref(), stopped.is_cancelled());
    Ok(())
}

/// The state a run starts from: the project's work tree as it stands, recorded
/// in the shadow snapshot repo `/undo` uses, so a front-end can list the files
/// the run changed — including ones no tool call named, like a formatter's or a
/// shell command's — and put them back. `None` when the snapshot cannot be
/// taken, which is a turn without a change card rather than a failed turn.
async fn mark_baseline(cwd: &Path) -> Option<(Snapshots, String)> {
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || Snapshots::baseline(&cwd))
        .await
        .ok()
        .flatten()
}

/// Puts the project back to the state a run started from — the inverse of the
/// change card the run's end emitted — discarding what the run wrote. `after` is
/// the state that turn left behind (the `agent-end` payload's own revision): the
/// work tree still has to hold it, or the restore would also take a change made
/// after the turn — including one from a later turn whose own card is the one to
/// undo. An older card is refused with the reason rather than silently doing it.
pub async fn undo_turn(project: String, baseline: String, after: Option<String>) -> CmdResult<()> {
    let cwd = PathBuf::from(project);
    tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
        let snapshots = Snapshots::open(&cwd)?;
        // The restore itself, and the check an older card is refused by, are the
        // core's — a front-end that has no snapshot repo of its own (the CLI's
        // `changes undo`, which the VS Code extension runs) takes the same path.
        snapshots.restore_turn(&baseline, after.as_deref())
    })
    .await
    .map_err(err)?
    .map_err(err)
}

/// The two sides of one changed file at the baseline a card carries: what the run
/// found — read out of the project's shadow snapshot, since that state is nowhere
/// on disk — and what is on the disk now, as the aligned lines the review paints.
/// A card's rows carry the compact preview instead, so listing what a turn
/// touched never carries every file's whole diff; the review asks for the one file
/// it is showing.
pub async fn change_sides(project: String, baseline: String, path: String) -> CmdResult<Value> {
    let root = PathBuf::from(project);
    tokio::task::spawn_blocking(move || -> anyhow::Result<Value> {
        let snapshots = Snapshots::open(&root)?;
        read_sides(&snapshots, &root, &baseline, &path)
    })
    .await
    .map_err(err)?
    .map_err(err)
}

/// Both sides of one file: what the baseline recorded, read out of the snapshot
/// repo, and what is on the disk now. A side that is not there — a file the run
/// added or deleted — reads as empty; a side that is not text makes the whole
/// file binary, which is the one answer that cannot be painted line by line. A
/// side that could not be read at all is an error rather than an empty one, so a
/// baseline the snapshot no longer holds is reported instead of the review
/// painting an unchanged file.
fn read_sides(
    snapshots: &Snapshots,
    work_tree: &Path,
    baseline: &str,
    path: &str,
) -> anyhow::Result<Value> {
    let old = snapshots
        .content_at_opt(baseline, path)
        .with_context(|| format!("reading {path} as the baseline recorded it"))?;
    let new = match std::fs::read(work_tree.join(path)) {
        Ok(bytes) => Some(bytes),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(err) => return Err(err).with_context(|| format!("reading {path}")),
    };
    let (Some(old), Some(new)) = (side_text(old), side_text(new)) else {
        return Ok(json!({ "binary": true, "omitted": false, "lines": [] }));
    };
    Ok(review_sides(&old, &new))
}

/// One side as text, or `None` for one that cannot be painted line by line:
/// bytes that are not UTF-8, or that hold a NUL — the same reading
/// [`changes::file_change`](oxide_core::changes::file_change) takes, so a file a
/// card's row calls binary is not diffed here as text. An absent side (`None`) is
/// empty text, so an added file reads as all additions and a deleted one as all
/// removals.
fn side_text(side: Option<Vec<u8>>) -> Option<String> {
    let bytes = match side {
        None => return Some(String::new()),
        Some(bytes) => bytes,
    };
    if oxide_core::changes::is_binary(&bytes) {
        return None;
    }
    String::from_utf8(bytes).ok()
}

/// The shape the window paints a review's diff from: one entry per aligned line,
/// each naming the number it holds on each side (the side it is missing from
/// carries none), or why there is nothing to paint.
fn review_sides(old: &str, new: &str) -> Value {
    match oxide_core::diff::lines(old, new) {
        Diff::Same => json!({ "binary": false, "omitted": false, "lines": [] }),
        Diff::Omitted { .. } => json!({ "binary": false, "omitted": true, "lines": [] }),
        Diff::Lines(lines) => json!({
            "binary": false,
            "omitted": false,
            "lines": lines
                .iter()
                .map(|line| json!({
                    "kind": match line.kind {
                        LineKind::Context => "context",
                        LineKind::Add => "add",
                        LineKind::Remove => "remove",
                    },
                    "old": line.old,
                    "new": line.new,
                    "text": line.text,
                }))
                .collect::<Vec<_>>(),
        }),
    }
}

/// Asks a running turn to stop. The loop finishes the in-flight step (so the
/// session stays a valid call/result sequence) and then ends; if it is still
/// running after a grace period it is force-aborted.
pub async fn cancel_run(run_id: u64, state: &DesktopState) -> CmdResult<()> {
    let runs = state.runs.clone();
    if let Some(run) = runs.lock().await.remove(&run_id) {
        run.cancel.cancel();
        let abort = run.abort;
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            abort.abort();
        });
    }
    Ok(())
}

/// Queues a message into a running turn: interleaved guidance, or a follow-up
/// for after the current turn finishes.
pub async fn steer_run(
    run_id: u64,
    message: String,
    follow_up: Option<bool>,
    attachments: Option<Vec<AttachmentInput>>,
    state: &DesktopState,
) -> CmdResult<bool> {
    let runs = state.runs.clone();
    let runs = runs.lock().await;
    if let Some(run) = runs.get(&run_id) {
        let queue = if follow_up.unwrap_or(false) {
            &run.follow_ups
        } else {
            &run.steering
        };
        let parts = attachment_parts(attachments, &message, &run.cwd)?;
        return Ok(queue.push(if parts.is_empty() {
            Message::user(message)
        } else {
            Message::user_parts(message, parts)
        }));
    }
    Ok(false)
}

/// Answers the composer's `@path` completion: the project's own files and
/// folders for the reference at the caret. `text` and `caret` are the message
/// box's value and caret as the webview counts them, and the returned range is
/// in those same indices, so the view only ever splices a row in.
///
/// An empty answer (`rows` with nothing in it) means there is no reference under
/// the caret, which is how the composer closes its list.
pub async fn at_suggestions(
    project: String,
    text: String,
    caret: usize,
    state: &DesktopState,
) -> CmdResult<AtAnswer> {
    let cwd = project_dir(&project)?;
    Ok(oxide_desktop::at::suggestions(
        &cwd, &state.at, &text, caret,
    ))
}

/// Answers a pending `approval-request`.
pub async fn resolve_approval(id: u64, decision: String, state: &DesktopState) -> CmdResult<()> {
    let approvals = state.approvals.clone();
    approvals.resolve(id, &decision).await;
    Ok(())
}

/// Answers a pending `question-request`. An empty `answers` list is a dismissed
/// dialog, which the agent reports to the model as unanswered.
pub async fn resolve_question(
    id: u64,
    answers: Vec<oxide_core::ask::Answer>,
    state: &DesktopState,
) -> CmdResult<()> {
    let questions = state.questions.clone();
    questions.resolve(id, answers).await;
    Ok(())
}

/// Tools that will be auto-approved for a project without prompting again.
pub async fn list_approvals(project: String, state: &DesktopState) -> CmdResult<Vec<String>> {
    let approvals = state.approvals.clone();
    Ok(approvals.list(Path::new(&project)).await)
}

/// Forgets the saved approval rules for a project.
pub async fn clear_approvals(project: String, state: &DesktopState) -> CmdResult<()> {
    let approvals = state.approvals.clone();
    approvals.clear(Path::new(&project)).await
}

// ---------- models ----------

/// Model catalogs for every logged-in provider, plus the active selection.
pub async fn list_models(project: String, state: &DesktopState) -> CmdResult<Value> {
    let config = {
        let manager = state.manager.lock().await;
        manager.config_for(&PathBuf::from(&project)).map_err(err)?
    };
    let active = auth::canonical_provider(&config.provider);
    let providers = oxide_core::config::provider_configs(&config);
    if providers.is_empty() {
        return Err("no provider connected — use Connect to add an API key".to_string());
    }
    let mut result = Vec::new();
    for (name, provider_config) in providers {
        let current = provider_config.model.clone();
        let models = LlmClient::new(provider_config)
            .list_models()
            .await
            .unwrap_or_default();
        result.push(json!({
            "provider": name,
            "active": name == active,
            "current": current,
            "models": models,
        }));
    }
    Ok(json!({ "active": active, "current": config.model, "providers": result }))
}

/// Switches the active model (and provider, if needed) in `config.json`.
pub async fn set_model(provider: String, model: String) -> CmdResult<()> {
    let mut config = Config::load(Path::new("."), None, None, None, None).map_err(err)?;
    let name = auth::canonical_provider(&provider);
    if name != auth::canonical_provider(&config.provider) {
        let key = AuthStore::load()
            .ok()
            .and_then(|store| store.key(&name).map(str::to_string))
            .unwrap_or_default();
        config.apply_provider(&name, &key);
    }
    config.apply_login_options(&model, "", "");
    config
        .persist_selection_at(&Config::config_path())
        .map_err(err)?;
    Ok(())
}

// ---------- themes ----------

/// Theme names available for a project, plus the selected one from `config.json`.
pub async fn list_themes(project: String) -> CmdResult<Value> {
    Ok(json!({
        "current": current_theme_name(),
        "names": theme_view::names(Path::new(&project)),
    }))
}

/// Resolves a theme to CSS-ready colors (the same files the CLI reads).
pub async fn theme_colors(project: String, name: String) -> CmdResult<Value> {
    let theme = theme_view::load(Path::new(&project), &name);
    Ok(json!({ "name": theme.name, "colors": theme.colors }))
}

/// Persists a theme choice and returns its colors.
pub async fn set_theme(project: String, name: String) -> CmdResult<Value> {
    Config::set_theme_at(&Config::config_path(), &name).map_err(err)?;
    let theme = theme_view::load(Path::new(&project), &name);
    Ok(json!({ "name": theme.name, "colors": theme.colors }))
}

// ---------- updates ----------

/// Checks the app's own release train (`desktop-v*`). The rules that resolve a
/// release — which tag belongs to the app, which artifact this platform
/// installs, whether it is newer — are `oxide_core::updates`, shared with the
/// terminal's `oxide update` and the VS Code panel, and the app resolves its own
/// rather than asking the installed CLI, whose answer would be a CLI release.
pub async fn check_updates() -> CmdResult<Value> {
    let check = update::check().await.map_err(err)?;
    serde_json::to_value(check).map_err(err)
}

/// Installs the newest release of the app from its own train: the bundle this
/// platform installs is downloaded, verified against the digest its release
/// published, and put in this installation's place, so the window offers the
/// update the app itself would run.
///
/// The install reports itself as it goes, the same events the launch's own
/// install emits, because the window paints one progress bar for whichever of
/// them is running — and a reader who put the dialog away is still waiting on
/// the answer below. A cancel the window asked for is an answer of its own
/// rather than a failure: nothing went wrong, and the release is where it was.
pub async fn install_update(state: &DesktopState) -> CmdResult<Value> {
    let answer = update::install_reporting(|progress| {
        state.announce_launch_update(
            "update-progress",
            json!({
                "stage": progress.stage,
                "version": progress.version,
                "received": progress.received,
                "total": progress.total,
            }),
        );
    })
    .await;
    match answer {
        Ok(answer) => {
            state.announce_launch_update("update-ready", answer.clone());
            Ok(answer)
        }
        // The dialog the reader cancelled in is already gone, so this is not
        // news to the window that asked — but it is what the install last said,
        // and a window that starts listening afterwards would otherwise read a
        // download that stopped as one still going.
        Err(error) if update::was_cancelled(&error) => {
            state.announce_launch_update("update-failed", json!({ "cancelled": true }));
            Ok(json!({ "ok": false, "cancelled": true }))
        }
        Err(error) => Err(error.to_string()),
    }
}

/// Stops the install in flight, which is what the dialog's Cancel asks for: the
/// download stops where it is, nothing is put in place, and the release is
/// offered again by the next check.
pub fn cancel_update() -> CmdResult<Value> {
    update::cancel();
    Ok(json!({}))
}

/// The update a launch installs on its own, the way a desktop app that keeps
/// itself current does.
///
/// The newest release of this app's own train is looked up in the background —
/// through the answer the shared store remembered, so a launch costs at most one
/// request every six hours, and only where `checkForUpdates` allows it — and when
/// it is newer than this build and this copy is one the app replaces in place, it
/// is downloaded, verified and put there without being asked. The window hears
/// every step rather than asking, so what has landed is the restart the dialog
/// offers rather than a promise the app cannot keep: the process running is still
/// the build that started, whatever is on disk.
///
/// A copy the app does not replace in place — a Windows setup, a distribution's
/// package, a checkout's build — is left to the dialog's own check, since only
/// the reader can decide to install one of those.
pub async fn auto_update(state: Arc<DesktopState>) {
    if !update_notice::enabled_in(None) {
        return;
    }
    // A launch is not the place to report a check that could not be made — the
    // window's own Check for Updates… is — so a lookup that fails is left to the
    // next launch and to that button.
    let Ok(notice) = update_notice::latest_notice(Component::Desktop).await else {
        return;
    };
    let installation = update::installation();
    if !update::launch_installs(&notice, update::current_version(), &installation) {
        return;
    }
    // A release the app is about to put in place is one the reader is told
    // about: the dialog opens on what is running, and its first step is either
    // the release with its notes or — while the install below is already
    // working — the download it is watching.
    state.announce_launch_update(
        "update-available",
        json!({ "version": notice.version, "tag": notice.tag, "url": notice.url }),
    );
    let steps = state.clone();
    let answer = update::install_reporting(move |progress| {
        steps.announce_launch_update(
            "update-progress",
            json!({
                "stage": progress.stage,
                "version": progress.version,
                "received": progress.received,
                "total": progress.total,
            }),
        );
    })
    .await;
    match answer {
        Ok(answer) => state.announce_launch_update("update-ready", answer),
        // A cancel is the reader's own doing, and the dialog it came from has
        // already closed — but what the install last said has to be the cancel,
        // since a window that starts listening later would otherwise read a
        // download that stopped as one still going.
        Err(error) if update::was_cancelled(&error) => {
            state.announce_launch_update("update-failed", json!({ "cancelled": true }))
        }
        Err(error) => {
            state.announce_launch_update("update-failed", json!({ "message": error.to_string() }))
        }
    }
}

/// Starts the background look for the model catalogs a launch makes when the
/// lookups are on: every provider this machine holds a credential for, so a
/// model gets the window its provider published rather than the built-in
/// table's conservative one. A window that is already open is told when the
/// answer changed, since the model chip it draws carries that window and a
/// reader keeps the app open for hours over one folder.
pub async fn auto_catalog(state: Arc<DesktopState>) {
    let providers = catalog::launch_providers(&active_provider());
    if catalog::refresh_launch(&providers, None).await.is_some() {
        let _ = state.events.emit("model-catalog", json!({}));
    }
}

/// What the install this process is running has said so far, for a window that
/// started listening after it began: the newest event it would have heard, or
/// nothing when no install has run at all.
pub fn launch_update(state: &DesktopState) -> CmdResult<Option<Value>> {
    let slot = state
        .launch_update
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    Ok(slot
        .as_ref()
        .map(|heard| json!({ "event": heard.event, "payload": heard.payload })))
}

/// Restarts the app, which is what runs a release an install has put in place:
/// the process running is still the build that started, so only a new one is the
/// new version.
///
/// A turn is work this process owns — its tools write files and its stream is
/// read here — so a restart is refused while one runs, the way the window refuses
/// to replace the thread on screen mid-turn. What the window is asked for is the
/// restart itself (the `restart` event), because a process cannot replace the
/// app it was started from: the window's own process is the one that comes back.
pub async fn restart_app(host: &Host, state: &DesktopState) -> CmdResult<()> {
    if !state.runs.lock().await.is_empty() {
        return Err("A turn is running; stop it before restarting Oxide.".to_string());
    }
    host.emit("restart", json!({}))?;
    Ok(())
}

/// Creates a new project with the given name and adds it to the registry.
/// Optionally accepts source folders to add.
pub async fn create_project(
    name: String,
    folders: Option<Vec<String>>,
    state: &DesktopState,
) -> CmdResult<AddProjectResult> {
    if name.trim().is_empty() {
        return Err("Project name cannot be empty".to_string());
    }

    let mut manager = state.manager.lock().await;
    let mut project_added = None;

    // Add each source folder as a project. The dialog's name labels the first
    // folder; any extra folders keep their own basename so one name is never
    // applied to unrelated folders.
    if let Some(folder_list) = folders {
        let mut first = true;
        for folder in folder_list {
            if !folder.is_empty() {
                let custom = if first { Some(name.as_str()) } else { None };
                first = false;
                match manager.add_project_with_name(&expand_project_path(&folder), custom) {
                    Ok(project) => {
                        if project_added.is_none() {
                            project_added = Some(project.id);
                        }
                    }
                    Err(e) => return Err(format!("Failed to add folder {}: {}", folder, e)),
                }
            }
        }
    }

    // If no folders were provided or all failed, create an empty project entry
    let projects = manager.overview().map_err(err)?;
    let added = project_added.unwrap_or(name);

    Ok(AddProjectResult { projects, added })
}

fn current_theme_name() -> String {
    std::fs::read_to_string(Config::config_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .get("theme")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "dark".to_string())
}

/// The provider this launch runs on, by the same precedence `Config::load`
/// applies: `OXIDE_PROVIDER` when it names one, else the one `config.json`
/// selects. A launch's catalog look covers it as well as every stored
/// credential, since the two providers that sign with a credential the machine
/// already holds — Bedrock and Vertex — hold no key here and are the ones a
/// catalog answers for most usefully.
fn active_provider() -> String {
    if let Some(provider) = std::env::var("OXIDE_PROVIDER")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return provider;
    }
    std::fs::read_to_string(Config::config_path())
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|value| {
            value
                .get("provider")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "openai".to_string())
}

fn arg<T: DeserializeOwned>(args: &Value, name: &str) -> CmdResult<T> {
    let value = args
        .get(name)
        .cloned()
        .ok_or_else(|| format!("missing `{name}`"))?;
    serde_json::from_value(value).map_err(|error| format!("invalid `{name}`: {error}"))
}

fn optional_arg<T: DeserializeOwned>(args: &Value, name: &str) -> CmdResult<Option<T>> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| format!("invalid `{name}`: {error}")),
    }
}

fn command_value<T: Serialize>(result: CmdResult<T>) -> CmdResult<Value> {
    result.and_then(|value| serde_json::to_value(value).map_err(err))
}

/// Dispatches the stable command contract used by the window. The window's own
/// calls — `pick_folder` and `open_url` — never arrive here, since the operating
/// system is what carries them out; the launch behind `restart_app` is announced
/// from the arm that can tell whether the app is in a state to make it; and every
/// command that touches Oxide state is here.
pub async fn dispatch(
    state: Arc<DesktopState>,
    host: &Host,
    command: &str,
    args: Value,
) -> CmdResult<Value> {
    match command {
        "list_projects" => command_value(list_projects(&state).await),
        "add_project" => command_value(add_project(arg(&args, "path")?, &state).await),
        "create_project" => command_value(
            create_project(arg(&args, "name")?, optional_arg(&args, "folders")?, &state).await,
        ),
        "remove_project" => command_value(remove_project(arg(&args, "id")?, &state).await),
        "list_sessions" => command_value(list_sessions(arg(&args, "project")?, &state).await),
        "all_sessions" => command_value(all_sessions(&state).await),
        "project_info" => command_value(project_info(arg(&args, "project")?, &state).await),
        "reasoning_levels" => command_value(reasoning_levels(arg(&args, "project")?, &state).await),
        "set_project_trust" => command_value(
            set_project_trust(arg(&args, "project")?, arg(&args, "trusted")?, &state).await,
        ),
        "mcp_servers" => command_value(mcp_servers(arg(&args, "project")?).await),
        "set_mcp_server" => command_value(
            set_mcp_server(
                arg(&args, "project")?,
                arg(&args, "name")?,
                arg(&args, "enabled")?,
            )
            .await,
        ),
        "list_commands" => command_value(list_commands(arg(&args, "project")?).await),
        "read_clipboard" => command_value(read_clipboard()),
        "at_suggestions" => command_value(
            at_suggestions(
                arg(&args, "project")?,
                arg(&args, "text")?,
                arg(&args, "caret")?,
                &state,
            )
            .await,
        ),
        "session_messages" => {
            command_value(session_messages(arg(&args, "project")?, arg(&args, "id")?).await)
        }
        "rename_session" => command_value(
            rename_session(
                arg(&args, "project")?,
                arg(&args, "id")?,
                arg(&args, "name")?,
            )
            .await,
        ),
        "delete_session" => {
            command_value(delete_session(arg(&args, "project")?, arg(&args, "id")?).await)
        }
        "list_providers" => command_value(list_providers().await),
        "login" => command_value(
            login(
                arg(&args, "provider")?,
                optional_arg(&args, "key")?,
                optional_arg(&args, "model")?,
                optional_arg(&args, "baseUrl")?,
            )
            .await,
        ),
        "logout" => command_value(logout(arg(&args, "provider")?).await),
        "send_prompt" => command_value(
            send_prompt(
                state,
                arg(&args, "project")?,
                arg(&args, "prompt")?,
                optional_arg(&args, "session")?,
                optional_arg(&args, "reasoning")?,
                optional_arg(&args, "attachments")?,
            )
            .await,
        ),
        "cancel_run" => command_value(cancel_run(arg(&args, "runId")?, &state).await),
        "steer_run" => command_value(
            steer_run(
                arg(&args, "runId")?,
                arg(&args, "message")?,
                optional_arg(&args, "followUp")?,
                optional_arg(&args, "attachments")?,
                &state,
            )
            .await,
        ),
        "undo_turn" => command_value(
            undo_turn(
                arg(&args, "project")?,
                arg(&args, "baseline")?,
                optional_arg(&args, "after")?,
            )
            .await,
        ),
        "change_sides" => command_value(
            change_sides(
                arg(&args, "project")?,
                arg(&args, "baseline")?,
                arg(&args, "path")?,
            )
            .await,
        ),
        "resolve_approval" => command_value(
            resolve_approval(arg(&args, "id")?, arg(&args, "decision")?, &state).await,
        ),
        "resolve_question" => {
            command_value(resolve_question(arg(&args, "id")?, arg(&args, "answers")?, &state).await)
        }
        "list_approvals" => command_value(list_approvals(arg(&args, "project")?, &state).await),
        "clear_approvals" => command_value(clear_approvals(arg(&args, "project")?, &state).await),
        "list_models" => command_value(list_models(arg(&args, "project")?, &state).await),
        "set_model" => {
            command_value(set_model(arg(&args, "provider")?, arg(&args, "model")?).await)
        }
        "list_themes" => command_value(list_themes(arg(&args, "project")?).await),
        "theme_colors" => {
            command_value(theme_colors(arg(&args, "project")?, arg(&args, "name")?).await)
        }
        "set_theme" => command_value(set_theme(arg(&args, "project")?, arg(&args, "name")?).await),
        "check_updates" => command_value(check_updates().await),
        "install_update" => command_value(install_update(&state).await),
        "cancel_update" => command_value(cancel_update()),
        "launch_update" => command_value(launch_update(&state)),
        "restart_app" => command_value(restart_app(host, &state).await),
        _ => Err(format!("unknown desktop command `{command}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::llm::{FunctionCall, ToolCall};

    #[test]
    fn a_refused_attachment_names_the_types_and_the_limit() {
        let refused = attachment_parts(
            Some(vec![AttachmentInput {
                data_url: "data:application/octet-stream;base64,AAAA".to_string(),
                name: Some("archive.bin".to_string()),
            }]),
            "look at archive.bin",
            Path::new("."),
        )
        .expect_err("a binary payload is not attachable");
        assert_eq!(
            refused,
            "archive.bin could not be attached: attach an image, a PDF or a text file of at most \
             20 MB"
        );
    }

    #[test]
    fn a_text_payload_travels_as_its_own_text() {
        let parts = attachment_parts(
            Some(vec![AttachmentInput {
                data_url: "data:text/plain;base64,aGVsbG8=".to_string(),
                name: Some("notes.txt".to_string()),
            }]),
            "see the notes",
            Path::new("."),
        )
        .expect("a text file is attachable");
        match parts.as_slice() {
            [ContentPart::Text { text }] => {
                assert!(text.contains("<file name=\"notes.txt\">"), "{text}");
                assert!(text.contains("hello"), "{text}");
            }
            other => panic!("expected one text part, got {other:?}"),
        }
    }

    #[test]
    fn an_undo_holds_until_the_work_tree_still_has_that_turn() {
        let root = std::env::temp_dir().join(format!("oxide_undo_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let snapshots = Snapshots::at(root.join("shadow"), work.clone()).unwrap();

        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        let baseline = snapshots.mark().unwrap();
        std::fs::write(work.join("a.txt"), "two\n").unwrap();
        let after = snapshots.mark_named("turn").unwrap();

        // A change made after the turn — a later turn's work, or the user's own
        // edit — is not this card's to take with it, so the undo is refused and
        // the file is left alone.
        std::fs::write(work.join("a.txt"), "three\n").unwrap();
        let refused = snapshots.restore_turn(&baseline, Some(&after)).unwrap_err();
        assert_eq!(
            refused.to_string(),
            "the project has changed since that turn — undo the newest turn first"
        );
        assert_eq!(
            std::fs::read_to_string(work.join("a.txt")).unwrap(),
            "three\n"
        );

        // With the state the turn left still on disk it puts the baseline back,
        // and a card that carries no marker is taken at its word.
        std::fs::write(work.join("a.txt"), "two\n").unwrap();
        snapshots.restore_turn(&baseline, Some(&after)).unwrap();
        assert_eq!(
            std::fs::read_to_string(work.join("a.txt")).unwrap(),
            "one\n"
        );
        std::fs::write(work.join("a.txt"), "two\n").unwrap();
        snapshots.restore_turn(&baseline, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(work.join("a.txt")).unwrap(),
            "one\n"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_review_reads_both_sides_out_of_the_snapshot() {
        let root = std::env::temp_dir().join(format!("oxide_sides_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let snapshots = Snapshots::at(root.join("shadow"), work.clone()).unwrap();

        std::fs::write(work.join("a.txt"), "one\ntwo\n").unwrap();
        // A file git calls binary — valid UTF-8, with a NUL in it — so the side
        // the review reads has to be refused lines the same way the row is. The
        // name may not be a Windows device name (`nul`, whatever its extension),
        // which git refuses to add at all.
        std::fs::write(work.join("zeroed.txt"), "a\u{0}b\n").unwrap();
        let baseline = snapshots.mark().unwrap();
        std::fs::write(work.join("a.txt"), "one\nthree\n").unwrap();
        std::fs::write(work.join("zeroed.txt"), "a\u{0}c\n").unwrap();
        // A file the run added has nothing behind it, and one that is not text
        // has nothing to align either.
        std::fs::write(work.join("b.txt"), "new\n").unwrap();
        std::fs::write(work.join("png"), [0x89, 0xff, 0xfe, 0x00]).unwrap();

        let changed = read_sides(&snapshots, &work, &baseline, "a.txt").unwrap();
        let kinds: Vec<&str> = changed["lines"]
            .as_array()
            .unwrap()
            .iter()
            .map(|line| line["kind"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["context", "remove", "add"]);
        // The side a line is missing from carries no number, and the numbers
        // come from the side it sits on.
        assert_eq!(changed["lines"][1]["old"], json!(2));
        assert_eq!(changed["lines"][1]["new"], Value::Null);
        assert_eq!(changed["lines"][2]["text"], json!("three"));

        let added = read_sides(&snapshots, &work, &baseline, "b.txt").unwrap();
        assert_eq!(added["lines"][0]["kind"], json!("add"));
        assert_eq!(added["lines"][0]["old"], Value::Null);
        assert_eq!(added["lines"][0]["new"], json!(1));

        let binary = read_sides(&snapshots, &work, &baseline, "png").unwrap();
        assert_eq!(binary["binary"], json!(true));
        assert!(binary["lines"].as_array().unwrap().is_empty());
        assert_eq!(
            read_sides(&snapshots, &work, &baseline, "zeroed.txt").unwrap()["binary"],
            json!(true)
        );

        // A baseline the snapshot no longer holds is reported rather than read
        // as a file with nothing on either side.
        let gone = read_sides(&snapshots, &work, "0000000", "a.txt").unwrap_err();
        assert!(gone.to_string().contains("reading a.txt"), "{gone}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_home_screen_palette_is_the_builtins() {
        // The window opens on no project and the composer is ready to type in, so
        // `/` there lists what the app performs itself rather than failing into
        // an empty menu. A folder is what the configured commands, prompt
        // templates and skills are read from, and there is none to read.
        let home = palette_entries("").unwrap();
        let names: Vec<&str> = home.iter().map(|entry| entry.name.as_str()).collect();
        assert_eq!(
            names,
            oxide_core::commands::BUILTINS
                .iter()
                .map(|command| command.name)
                .collect::<Vec<_>>()
        );
        assert!(names.contains(&"help") && names.contains(&"new") && names.contains(&"session"));
        // Whitespace is no folder either, and the other commands keep refusing it
        // rather than resolving it to the directory the app was launched in.
        assert_eq!(palette_entries("   ").unwrap().len(), home.len());
        assert!(project_dir("  ").is_err());
    }

    #[test]
    fn the_launchs_lookup_covers_the_provider_the_environment_names() {
        // The launch reads the provider the same way a turn does, so a provider
        // named in the environment is the one its lookup covers rather than the
        // one the stored configuration happens to select.
        static ENV: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _guard = ENV.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let previous = std::env::var("OXIDE_PROVIDER").ok();
        std::env::set_var("OXIDE_PROVIDER", " bedrock ");
        assert_eq!(active_provider(), "bedrock");
        // A name of nothing but whitespace names no provider, so the stored
        // selection stands.
        std::env::set_var("OXIDE_PROVIDER", "  ");
        assert_ne!(active_provider(), "  ");
        assert!(!active_provider().trim().is_empty());
        match previous {
            Some(value) => std::env::set_var("OXIDE_PROVIDER", value),
            None => std::env::remove_var("OXIDE_PROVIDER"),
        }
    }

    #[test]
    fn message_view_exposes_tool_calls_for_replay() {
        let call = ToolCall {
            id: "call_1".into(),
            kind: "function".into(),
            signature: None,
            function: FunctionCall {
                name: "bash".into(),
                arguments: "{\"command\":\"ls\"}".into(),
            },
        };
        let message = Message::assistant("checking", vec![call]);
        let view = message_view(&message);
        assert_eq!(view["role"], "assistant");
        assert_eq!(view["content"], "checking");
        assert_eq!(view["toolCalls"][0]["id"], "call_1");
        assert_eq!(view["toolCalls"][0]["name"], "bash");
        assert_eq!(view["toolCalls"][0]["arguments"], "{\"command\":\"ls\"}");

        let result = Message::tool("call_1", "file-a\nfile-b");
        let view = message_view(&result);
        assert_eq!(view["role"], "tool");
        assert_eq!(view["toolCallId"], "call_1");
        assert_eq!(view["content"], "file-a\nfile-b");
    }
}
