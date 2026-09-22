//! Starting an agent turn for the desktop.
//!
//! Loads the *same* configuration the CLI uses for a project, resolves the
//! session, and streams `AgentEvent`s back to the caller. The GUI layer forwards
//! those over Tauri events; keeping the logic here means it can be exercised
//! without a webview.

use anyhow::{Context, Result};
use oxide_core::agent::{AgentEvent, Approver, Cancel, Steering};
use oxide_core::runner::{self, AgentRun};
use oxide_core::session::SessionLog;
use std::path::Path;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::task::JoinHandle;

/// A running turn: the stream of agent events, the task handle so the
/// front-end can abort it, and the steering queues so it can be nudged mid-run.
pub struct Turn {
    pub session_id: Option<String>,
    pub events: UnboundedReceiver<AgentEvent>,
    pub handle: JoinHandle<()>,
    pub steering: Steering,
    pub follow_ups: Steering,
    /// Cooperative stop: the loop finishes the current step then ends cleanly.
    pub cancel: Cancel,
}

/// Starts a turn against `project`. `session` selects which log to continue;
/// `None` creates a fresh one (or resumes nothing, per `config.ephemeral`).
/// `approve` is the interactive approval callback; `None` falls back to
/// `config.auto_approve`. `reasoning` overrides the stored value for this run.
pub async fn start_turn(
    project: &Path,
    prompt: &str,
    session: Option<SessionLog>,
    approve: Option<Approver>,
    reasoning: Option<String>,
) -> Result<Turn> {
    let config = crate::manager::load_project_config_with(project, reasoning)
        .with_context(|| format!("loading configuration for {}", project.display()))?;
    config.require_api_key()?;

    let ephemeral = config.ephemeral;
    let (history, log) = runner::begin_session(project, session, ephemeral, prompt, &[])?;
    let session_id = log.as_ref().map(|entry| entry.id().to_string());

    let steering = Steering::new();
    let follow_ups = Steering::new();
    let cancel = Cancel::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let run = AgentRun {
        config,
        cwd: project.to_path_buf(),
        history,
        prompt: prompt.to_string(),
        subtask: false,
        command_agent: None,
        session: log,
        approve,
        steering: steering.clone(),
        follow_ups: follow_ups.clone(),
        cancel: cancel.clone(),
    };
    let handle = runner::spawn_agent(run, tx).await;
    Ok(Turn {
        session_id,
        events: rx,
        handle,
        steering,
        follow_ups,
        cancel,
    })
}

/// Resolves a session reference (`new`, `latest`, or a session id/path) for a
/// project. Kept public so the GUI can expose "resume".
pub fn open_session(project: &Path, reference: &str) -> Result<Option<SessionLog>> {
    match reference {
        "new" | "" => Ok(Some(SessionLog::create(project)?)),
        "latest" => Ok(SessionLog::latest(project).or_else(|| SessionLog::create(project).ok())),
        other => Ok(Some(SessionLog::open_ref(project, other)?)),
    }
}
