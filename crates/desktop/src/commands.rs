//! Tauri commands backing the desktop UI.

use crate::approval::ApprovalBroker;
use crate::ask::AskBroker;
use anyhow::Context;
use oxide_core::agent::{AgentEvent, Cancel, Steering};
use oxide_core::auth::{self, AuthStore};
use oxide_core::cli::{event_json, session_header};
use oxide_core::config::Config;
use oxide_core::diff::{Diff, LineKind};
use oxide_core::llm::LlmClient;
use oxide_core::llm::Message;
use oxide_core::llm::{ContentPart, MessageContent};
use oxide_core::session::{SessionLog, SessionSummary};
use oxide_core::snapshots::Snapshots;
use oxide_core::theme_view;
use oxide_desktop::at::{AtAnswer, PathCache};
use oxide_desktop::manager::{expand_project_path, DesktopManager, ProjectView};
use oxide_desktop::turn::{notify_finished, open_session, start_turn, Turn};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::Mutex;
use tokio::task::AbortHandle;

type CmdResult<T> = Result<T, String>;

/// A session's first snapshot baseline: `None` for a project that must not be
/// snapshotted (see [`Snapshots::baseline`]), so a turn there has no change card.
type SessionBaseline = Option<(Snapshots, String)>;

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
    runs: Arc<Mutex<HashMap<u64, RunHandle>>>,
    next_run: AtomicU64,
    /// The first baseline of each open session, so a later turn's change card
    /// lists the whole session's files together and its Undo puts the session
    /// back rather than only the latest turn.
    session_baselines: Arc<Mutex<HashMap<String, SessionBaseline>>>,
}

impl DesktopState {
    pub fn new(manager: DesktopManager, app: AppHandle) -> Self {
        Self {
            manager: Mutex::new(manager),
            approvals: Arc::new(ApprovalBroker::new(app.clone())),
            questions: Arc::new(AskBroker::new(app)),
            at: PathCache::default(),
            runs: Arc::new(Mutex::new(HashMap::new())),
            next_run: AtomicU64::new(1),
            session_baselines: Arc::new(Mutex::new(HashMap::new())),
        }
    }
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

/// An image/PDF the desktop attached from the clipboard or a file picker. A
/// pasted image has no on-disk path in the webview, so the bytes travel as a
/// data URL.
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentInput {
    pub data_url: String,
    #[serde(default)]
    pub name: Option<String>,
}

/// A refused attachment fails the send instead of vanishing from the message:
/// a type neither the provider nor the webview takes, a payload that is not
/// base64, or one past the limit the core enforces.
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
                        "{name} could not be attached: attach a PNG, JPEG, GIF, WebP or BMP image or a PDF of at most {limit} MB"
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
#[tauri::command]
pub async fn list_projects(state: State<'_, DesktopState>) -> CmdResult<Vec<ProjectView>> {
    state.manager.lock().await.overview().map_err(err)
}

#[tauri::command]
pub async fn add_project(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<AddProjectResult> {
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

/// Opens the platform folder chooser. Used by the desktop's **Add** button when
/// the path field is empty, so adding a project does not require typing an
/// absolute path from memory.
///
/// The panel is the app's own (the dialog plugin's `NSOpenPanel`/GTK/Windows
/// equivalent), not a chooser shelled out to `osascript`/`zenity`: a child
/// process's panel opens as a background app, which can put it behind the
/// window — or never show it at all where the platform refuses the request —
/// and the user is left with a button that appears to do nothing.
#[tauri::command]
pub async fn pick_folder(app: AppHandle) -> CmdResult<Option<String>> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Add a project to Oxide")
        .pick_folder(move |path| {
            let _ = tx.send(path.map(|path| path.to_string()));
        });
    rx.await.map_err(err)
}

/// Opens an external link in the platform browser. The transcript renders
/// URLs as anchors, but the webview cannot navigate to a remote page, so a
/// click is routed here instead of relying on `target="_blank"`.
#[tauri::command]
pub fn open_url(url: String) -> CmdResult<()> {
    let url = url.trim();
    if !is_openable_url(url) {
        return Err("Only http(s) links can be opened".to_string());
    }
    open_in_browser(url).map_err(err)
}

fn is_openable_url(url: &str) -> bool {
    let scheme = url.to_ascii_lowercase();
    scheme.starts_with("https://") || scheme.starts_with("http://")
}

fn open_in_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    let spawned = std::process::Command::new("open").arg(url).spawn();
    // `cmd /C start` would let a URL with quotes or shell metacharacters be
    // read as command text, so hand the URL to a handler that takes it as a
    // plain argument instead.
    #[cfg(target_os = "windows")]
    let spawned = std::process::Command::new("rundll32")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let spawned = std::process::Command::new("xdg-open").arg(url).spawn();
    #[cfg(not(any(unix, target_os = "windows")))]
    let spawned: std::io::Result<std::process::Child> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "opening links is not supported on this platform",
    ));
    spawned.map(|_| ())
}

#[tauri::command]
pub async fn remove_project(
    id: String,
    state: State<'_, DesktopState>,
) -> CmdResult<Vec<ProjectView>> {
    let mut manager = state.manager.lock().await;
    manager.remove_project(&id).map_err(err)?;
    manager.overview().map_err(err)
}

/// Provider/model resolved from the same `config.json` the CLI uses, plus the
/// project's trust state so the UI can prompt before loading project resources.
#[tauri::command]
pub async fn project_info(project: String, state: State<'_, DesktopState>) -> CmdResult<Value> {
    let manager = state.manager.lock().await;
    let path = PathBuf::from(&project);
    let config = manager.config_for(&path).map_err(err)?;
    Ok(project_info_value(&config, &path))
}

/// Saves a trust decision for a project (the desktop equivalent of the CLI's
/// `/trust`) and returns the refreshed project info.
#[tauri::command]
pub async fn set_project_trust(
    project: String,
    trusted: bool,
    state: State<'_, DesktopState>,
) -> CmdResult<Value> {
    let path = PathBuf::from(&project);
    oxide_desktop::manager::set_project_trust(&path, trusted).map_err(err)?;
    let manager = state.manager.lock().await;
    let config = manager.config_for(&path).map_err(err)?;
    Ok(project_info_value(&config, &path))
}

fn project_info_value(config: &Config, project: &Path) -> Value {
    let trust = oxide_desktop::manager::project_trust(config, project);
    json!({
        "provider": config.provider,
        "model": config.model,
        "reasoning": config.reasoning.label(),
        "supportsReasoning": config.supports_reasoning(),
        "contextWindow": config.context_window(),
        "hasKey": !config.api_key.is_empty(),
        "trust": trust,
    })
}

// ---------- mcp servers ----------

/// The MCP servers visible from `project` with their connection state: what
/// `/mcps` lists. The view is the same one the CLI prints and the VS Code
/// extension draws, so all three agree on names, transports and statuses.
#[tauri::command]
pub async fn mcp_servers(project: String) -> CmdResult<Vec<oxide_core::mcp_config::ServerView>> {
    let cwd = project_dir(&project)?;
    Ok(oxide_core::mcp_config::server_views(&cwd).await)
}

/// Turns an MCP server off (or back on) in the file that defines it, then
/// returns the re-probed list so the modal can redraw from one round trip.
#[tauri::command]
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
#[tauri::command]
pub async fn list_commands(project: String) -> CmdResult<Vec<oxide_core::commands::CommandEntry>> {
    Ok(oxide_core::commands::palette(&project_dir(&project)?))
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

#[tauri::command]
pub async fn list_sessions(
    project: String,
    state: State<'_, DesktopState>,
) -> CmdResult<Vec<SessionSummary>> {
    state
        .manager
        .lock()
        .await
        .sessions_for(&PathBuf::from(project))
        .map_err(err)
}

/// Sessions across every project, newest first (the cross-repo view).
#[tauri::command]
pub async fn all_sessions(state: State<'_, DesktopState>) -> CmdResult<Vec<SessionSummary>> {
    state.manager.lock().await.all_sessions().map_err(err)
}

/// The stored transcript for one session.
#[tauri::command]
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

#[tauri::command]
pub async fn rename_session(project: String, id: String, name: String) -> CmdResult<()> {
    SessionLog::rename(&PathBuf::from(project), &id, &name).map_err(err)
}

#[tauri::command]
pub async fn delete_session(project: String, id: String) -> CmdResult<()> {
    SessionLog::delete(&PathBuf::from(project), &id).map_err(err)
}

// ---------- providers ----------

/// Known providers with their stored-credential state.
#[tauri::command]
pub async fn list_providers() -> CmdResult<Vec<Value>> {
    let stored = auth::stored_providers();
    Ok(auth::KNOWN_PROVIDERS
        .iter()
        .map(|option| {
            json!({
                "name": option.name,
                "label": option.label,
                "description": option.description,
                "keyUrl": option.key_url,
                "stored": stored.iter().any(|name| name == option.name),
            })
        })
        .collect())
}

/// Stores (or reuses) a provider credential and persists the selection in the
/// same `auth.json` / `config.json` the CLI uses.
#[tauri::command]
pub async fn login(
    provider: String,
    key: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
) -> CmdResult<Value> {
    let name = match key.as_deref().filter(|value| !value.trim().is_empty()) {
        Some(key) => auth::connect(&provider, key).map_err(err)?,
        None => auth::select_stored(&provider).map_err(err)?.0,
    };
    let mut config = Config::load(Path::new("."), None, None, None, None).map_err(err)?;
    if let Some(model) = model.filter(|value| !value.is_empty()) {
        config.model = model;
    }
    if let Some(url) = base_url.filter(|value| !value.is_empty()) {
        config.base_url = url;
    }
    config
        .persist_selection_at(&Config::config_path())
        .map_err(err)?;
    Ok(json!({ "provider": name, "model": config.model }))
}

#[tauri::command]
pub async fn logout(provider: String) -> CmdResult<bool> {
    let mut store = AuthStore::load().map_err(err)?;
    let removed = store.remove(&provider);
    store.save().map_err(err)?;
    Ok(removed)
}

// ---------- turns ----------

/// Starts a turn in the background and returns its run id immediately, so the
/// UI can cancel or steer it while it streams.
#[tauri::command]
pub async fn send_prompt(
    app: AppHandle,
    project: String,
    prompt: String,
    session: Option<String>,
    reasoning: Option<String>,
    attachments: Option<Vec<AttachmentInput>>,
) -> CmdResult<u64> {
    let (run_id, approvals, questions, runs) = {
        let state = app.state::<DesktopState>();
        (
            state.next_run.fetch_add(1, Ordering::Relaxed),
            state.approvals.clone(),
            state.questions.clone(),
            state.runs.clone(),
        )
    };
    let attachments = attachment_parts(attachments, &prompt, Path::new(&project))?;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = drive_turn(
            app,
            run_id,
            project,
            prompt,
            session,
            reasoning,
            attachments,
            approvals,
            questions,
            runs,
        )
        .await;
    });
    Ok(run_id)
}

#[allow(clippy::too_many_arguments)]
async fn drive_turn(
    app: AppHandle,
    run_id: u64,
    project: String,
    prompt: String,
    session: Option<String>,
    reasoning: Option<String>,
    attachments: Vec<ContentPart>,
    approvals: Arc<ApprovalBroker>,
    questions: Arc<AskBroker>,
    runs: Arc<Mutex<HashMap<u64, RunHandle>>>,
) -> anyhow::Result<()> {
    let cwd = PathBuf::from(project);
    let reference = session.as_deref().unwrap_or("latest");
    let log = open_session(&cwd, reference)?;
    // The state this run starts from, which decides whether the turn changed
    // anything. The first run of a session also records the baseline its cards
    // diff against, so a later turn lists the whole session's files together.
    let turn_baseline = mark_baseline(&cwd).await;
    let session_key = log
        .as_ref()
        .map(|log| log.id().to_string())
        .unwrap_or_else(|| cwd.to_string_lossy().into_owned());
    let session_baseline = {
        let state = app.state::<DesktopState>();
        let mut baselines = state.session_baselines.lock().await;
        baselines
            .entry(session_key)
            .or_insert_with(|| turn_baseline.clone())
            .clone()
    };
    let approver = approvals.approver(cwd.clone());
    let asker = questions.asker_for(run_id);
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
    runs.lock().await.insert(
        run_id,
        RunHandle {
            abort: handle.abort_handle(),
            steering,
            follow_ups,
            cancel,
            cwd: cwd.clone(),
        },
    );
    let _ = app.emit(
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
            let _ = app.emit("agent-event", value);
        }
        if matches!(event, AgentEvent::Finished(_)) {
            break;
        }
    }

    runs.lock().await.remove(&run_id);
    // The turn is over, so the files it wrote are on disk: drop the completion's
    // listing rather than offering paths from before the work. A question it
    // left waiting can never be answered either: drop it here rather than
    // letting it sit until its timeout, and before `agent-end` so the window is
    // never told a turn ended while a request of its own is still open.
    let state = app.state::<DesktopState>();
    state.at.clear();
    questions.clear_run(run_id).await;
    // The listing, and the state the turn left behind: the project it belongs to
    // travels with both, so a window that switched projects mid-turn can tell
    // the card is not its own and an undo can check nothing came after it. The
    // listing diffs against the session's first baseline — so a later turn lists
    // the whole session's files — while the turn's own baseline only decides
    // whether the turn changed anything at all.
    let (baseline, after, changes) = match turn_changes(session_baseline, turn_baseline).await {
        Some((base, after, listed)) => (Some(base), after, Some(listed)),
        None => (None, None, None),
    };
    let _ = app.emit(
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

/// The card a finished turn carries: the session's first baseline, the state the
/// turn left behind, and the files changed since that first baseline. `None`
/// when the project has no snapshot, or when this turn changed nothing — a turn
/// that only read files says nothing even though the session already has changes.
async fn turn_changes(
    session_baseline: Option<(Snapshots, String)>,
    turn_baseline: Option<(Snapshots, String)>,
) -> Option<(String, Option<String>, oxide_core::changes::TurnChanges)> {
    let (turn_snapshots, turn_base) = turn_baseline?;
    let unchanged = tokio::task::spawn_blocking(move || turn_snapshots.unchanged_since(&turn_base))
        .await
        .ok()?
        .ok()?;
    if unchanged {
        return None;
    }
    let (snapshots, base) = session_baseline?;
    let listed = snapshots.clone();
    let marked = base.clone();
    let changes = tokio::task::spawn_blocking(move || listed.changes_since(&marked))
        .await
        .ok()?
        .ok()?;
    let after = tokio::task::spawn_blocking(move || snapshots.mark_named("turn").ok())
        .await
        .ok()
        .flatten();
    Some((base, after, changes))
}

/// Puts the project back to the state a run started from — the inverse of the
/// change card the run's end emitted — discarding what the run wrote. `after` is
/// the state that turn left behind (the `agent-end` payload's own revision): the
/// work tree still has to hold it, or the restore would also take a change made
/// after the turn — including one from a later turn whose own card is the one to
/// undo. An older card is refused with the reason rather than silently doing it.
#[tauri::command]
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
#[tauri::command]
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
#[tauri::command]
pub async fn cancel_run(run_id: u64, app: AppHandle) -> CmdResult<()> {
    let runs = app.state::<DesktopState>().runs.clone();
    if let Some(run) = runs.lock().await.remove(&run_id) {
        run.cancel.cancel();
        let abort = run.abort;
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            abort.abort();
        });
    }
    Ok(())
}

/// Queues a message into a running turn: interleaved guidance, or a follow-up
/// for after the current turn finishes.
#[tauri::command]
pub async fn steer_run(
    run_id: u64,
    message: String,
    follow_up: Option<bool>,
    attachments: Option<Vec<AttachmentInput>>,
    app: AppHandle,
) -> CmdResult<()> {
    let runs = app.state::<DesktopState>().runs.clone();
    let runs = runs.lock().await;
    if let Some(run) = runs.get(&run_id) {
        let queue = if follow_up.unwrap_or(false) {
            &run.follow_ups
        } else {
            &run.steering
        };
        let parts = attachment_parts(attachments, &message, &run.cwd)?;
        queue.push(if parts.is_empty() {
            Message::user(message)
        } else {
            Message::user_parts(message, parts)
        });
    }
    Ok(())
}

/// Answers the composer's `@path` completion: the project's own files and
/// folders for the reference at the caret. `text` and `caret` are the message
/// box's value and caret as the webview counts them, and the returned range is
/// in those same indices, so the view only ever splices a row in.
///
/// An empty answer (`rows` with nothing in it) means there is no reference under
/// the caret, which is how the composer closes its list.
#[tauri::command]
pub async fn at_suggestions(
    project: String,
    text: String,
    caret: usize,
    state: State<'_, DesktopState>,
) -> CmdResult<AtAnswer> {
    let cwd = project_dir(&project)?;
    Ok(oxide_desktop::at::suggestions(
        &cwd, &state.at, &text, caret,
    ))
}

/// Answers a pending `approval-request`.
#[tauri::command]
pub async fn resolve_approval(id: u64, decision: String, app: AppHandle) -> CmdResult<()> {
    let approvals = app.state::<DesktopState>().approvals.clone();
    approvals.resolve(id, &decision).await;
    Ok(())
}

/// Answers a pending `question-request`. An empty `answers` list is a dismissed
/// dialog, which the agent reports to the model as unanswered.
#[tauri::command]
pub async fn resolve_question(
    id: u64,
    answers: Vec<oxide_core::ask::Answer>,
    app: AppHandle,
) -> CmdResult<()> {
    let questions = app.state::<DesktopState>().questions.clone();
    questions.resolve(id, answers).await;
    Ok(())
}

/// Tools that will be auto-approved for a project without prompting again.
#[tauri::command]
pub async fn list_approvals(project: String, app: AppHandle) -> CmdResult<Vec<String>> {
    let approvals = app.state::<DesktopState>().approvals.clone();
    Ok(approvals.list(Path::new(&project)).await)
}

/// Forgets the saved approval rules for a project.
#[tauri::command]
pub async fn clear_approvals(project: String, app: AppHandle) -> CmdResult<()> {
    let approvals = app.state::<DesktopState>().approvals.clone();
    approvals.clear(Path::new(&project)).await
}

// ---------- models ----------

/// Model catalogs for every logged-in provider, plus the active selection.
#[tauri::command]
pub async fn list_models(project: String, state: State<'_, DesktopState>) -> CmdResult<Value> {
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
#[tauri::command]
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
#[tauri::command]
pub async fn list_themes(project: String) -> CmdResult<Value> {
    Ok(json!({
        "current": current_theme_name(),
        "names": theme_view::names(Path::new(&project)),
    }))
}

/// Resolves a theme to CSS-ready colors (the same files the CLI reads).
#[tauri::command]
pub async fn theme_colors(project: String, name: String) -> CmdResult<Value> {
    let theme = theme_view::load(Path::new(&project), &name);
    Ok(json!({ "name": theme.name, "colors": theme.colors }))
}

/// Persists a theme choice and returns its colors.
#[tauri::command]
pub async fn set_theme(project: String, name: String) -> CmdResult<Value> {
    Config::set_theme_at(&Config::config_path(), &name).map_err(err)?;
    let theme = theme_view::load(Path::new(&project), &name);
    Ok(json!({ "name": theme.name, "colors": theme.colors }))
}

/// Creates a new project with the given name and adds it to the registry.
/// Optionally accepts source folders to add.
#[tauri::command]
pub async fn create_project(
    name: String,
    folders: Option<Vec<String>>,
    state: State<'_, DesktopState>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::llm::{FunctionCall, ToolCall};

    #[test]
    fn open_url_only_accepts_web_links() {
        assert!(is_openable_url("https://github.com/o/r/pull/7"));
        assert!(is_openable_url("http://localhost:3000"));
        assert!(is_openable_url("HTTPS://example.com"));
        assert!(!is_openable_url("file:///etc/passwd"));
        assert!(!is_openable_url("javascript:alert(1)"));
        assert!(!is_openable_url(""));
    }

    #[test]
    fn a_refused_attachment_names_the_types_and_the_limit() {
        let refused = attachment_parts(
            Some(vec![AttachmentInput {
                data_url: "data:image/tiff;base64,AAAA".to_string(),
                name: Some("scan.tif".to_string()),
            }]),
            "look at scan.tif",
            Path::new("."),
        )
        .expect_err("a TIFF is not attachable");
        assert_eq!(
            refused,
            "scan.tif could not be attached: attach a PNG, JPEG, GIF, WebP or BMP image or a PDF \
             of at most 20 MB"
        );
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

    #[tokio::test]
    async fn a_later_turn_lists_the_sessions_files_together() {
        let root = std::env::temp_dir().join(format!("oxide_session_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        std::fs::write(work.join("b.txt"), "one\n").unwrap();
        let snapshots = Snapshots::at(root.join("shadow"), work.clone()).unwrap();

        // The first turn's baseline is the session's baseline; the first turn
        // changes a.txt and leaves its state committed behind it.
        let session = snapshots.mark().unwrap();
        std::fs::write(work.join("a.txt"), "two\n").unwrap();
        snapshots.mark_named("turn").unwrap();

        // The second turn starts from that state and changes b.txt; its card
        // still diffs against the session's first baseline.
        let turn = snapshots.mark().unwrap();
        std::fs::write(work.join("b.txt"), "two\n").unwrap();
        let (baseline, _after, changes) = turn_changes(
            Some((snapshots.clone(), session.clone())),
            Some((snapshots, turn)),
        )
        .await
        .unwrap();
        assert_eq!(baseline, session);
        let paths: Vec<&str> = changes
            .files
            .iter()
            .map(|file| file.path.as_str())
            .collect();
        assert_eq!(paths, vec!["a.txt", "b.txt"]);

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
        // the review reads has to be refused lines the same way the row is.
        std::fs::write(work.join("nul.txt"), "a\u{0}b\n").unwrap();
        let baseline = snapshots.mark().unwrap();
        std::fs::write(work.join("a.txt"), "one\nthree\n").unwrap();
        std::fs::write(work.join("nul.txt"), "a\u{0}c\n").unwrap();
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
            read_sides(&snapshots, &work, &baseline, "nul.txt").unwrap()["binary"],
            json!(true)
        );

        // A baseline the snapshot no longer holds is reported rather than read
        // as a file with nothing on either side.
        let gone = read_sides(&snapshots, &work, "0000000", "a.txt").unwrap_err();
        assert!(gone.to_string().contains("reading a.txt"), "{gone}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn message_view_exposes_tool_calls_for_replay() {
        let call = ToolCall {
            id: "call_1".into(),
            kind: "function".into(),
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
