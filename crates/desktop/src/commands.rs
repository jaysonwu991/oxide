//! Tauri commands backing the desktop UI.

use crate::approval::ApprovalBroker;
use oxide_core::agent::{AgentEvent, Cancel, Steering};
use oxide_core::auth::{self, AuthStore};
use oxide_core::cli::{event_json, session_header};
use oxide_core::config::Config;
use oxide_core::llm::LlmClient;
use oxide_core::llm::Message;
use oxide_core::session::{SessionLog, SessionSummary};
use oxide_core::theme_view;
use oxide_desktop::manager::{DesktopManager, ProjectView};
use oxide_desktop::turn::{open_session, start_turn, Turn};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, Manager, State};
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
}

pub struct DesktopState {
    pub manager: Mutex<DesktopManager>,
    pub approvals: Arc<ApprovalBroker>,
    runs: Arc<Mutex<HashMap<u64, RunHandle>>>,
    next_run: AtomicU64,
}

impl DesktopState {
    pub fn new(manager: DesktopManager, app: AppHandle) -> Self {
        Self {
            manager: Mutex::new(manager),
            approvals: Arc::new(ApprovalBroker::new(app)),
            runs: Arc::new(Mutex::new(HashMap::new())),
            next_run: AtomicU64::new(1),
        }
    }
}

fn message_view(message: &Message) -> Value {
    json!({
        "role": message.role,
        "content": message.display().unwrap_or_default(),
    })
}

// ---------- projects ----------

/// Every project: folders added here plus ones discovered from sessions.
#[tauri::command]
pub async fn list_projects(state: State<'_, DesktopState>) -> CmdResult<Vec<ProjectView>> {
    state.manager.lock().await.overview().map_err(err)
}

#[tauri::command]
pub async fn add_project(
    path: String,
    state: State<'_, DesktopState>,
) -> CmdResult<Vec<ProjectView>> {
    let mut manager = state.manager.lock().await;
    manager.add_project(&PathBuf::from(path)).map_err(err)?;
    manager.overview().map_err(err)
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

/// Provider/model resolved from the same `config.json` the CLI uses.
#[tauri::command]
pub async fn project_info(project: String, state: State<'_, DesktopState>) -> CmdResult<Value> {
    let manager = state.manager.lock().await;
    let config = manager.config_for(&PathBuf::from(&project)).map_err(err)?;
    Ok(json!({
        "provider": config.provider,
        "model": config.model,
        "reasoning": config.reasoning.label(),
        "supportsReasoning": config.supports_reasoning(),
        "contextWindow": config.context_window(),
        "hasKey": !config.api_key.is_empty(),
    }))
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
) -> CmdResult<u64> {
    let (run_id, approvals, runs) = {
        let state = app.state::<DesktopState>();
        (
            state.next_run.fetch_add(1, Ordering::Relaxed),
            state.approvals.clone(),
            state.runs.clone(),
        )
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = drive_turn(
            app, run_id, project, prompt, session, reasoning, approvals, runs,
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
    approvals: Arc<ApprovalBroker>,
    runs: Arc<Mutex<HashMap<u64, RunHandle>>>,
) -> anyhow::Result<()> {
    let cwd = PathBuf::from(project);
    let reference = session.as_deref().unwrap_or("latest");
    let log = open_session(&cwd, reference)?;
    let approver = approvals.approver(cwd.clone());
    let turn = start_turn(&cwd, &prompt, log, Some(approver), reasoning).await?;
    let Turn {
        session_id,
        mut events,
        handle,
        steering,
        follow_ups,
        cancel,
    } = turn;
    runs.lock().await.insert(
        run_id,
        RunHandle {
            abort: handle.abort_handle(),
            steering,
            follow_ups,
            cancel,
        },
    );
    let _ = app.emit(
        "agent-start",
        json!({ "runId": run_id, "sessionId": session_id }),
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
    let _ = app.emit(
        "agent-end",
        json!({ "runId": run_id, "sessionId": session_id }),
    );
    Ok(())
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
        queue.push(Message::user(message));
    }
    Ok(())
}

/// Answers a pending `approval-request`.
#[tauri::command]
pub async fn resolve_approval(id: u64, decision: String, app: AppHandle) -> CmdResult<()> {
    let approvals = app.state::<DesktopState>().approvals.clone();
    approvals.resolve(id, &decision).await;
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
