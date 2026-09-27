//! Starting an agent turn for the desktop.
//!
//! Loads the *same* configuration the CLI uses for a project, resolves the
//! session, and streams `AgentEvent`s back to the caller. The GUI layer forwards
//! those over Tauri events; keeping the logic here means it can be exercised
//! without a webview.

use anyhow::{Context, Result};
use oxide_core::agent::{AgentEvent, Approver, Cancel, Steering};
use oxide_core::ask::Asker;
use oxide_core::llm::ContentPart;
use oxide_core::runner::{self, AgentRun};
use oxide_core::session::{SessionLog, SessionSummary};
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
/// `config.auto_approve`. `ask` is how the model's questions reach the user, and
/// `None` leaves the `ask` tool out of the run. `reasoning` overrides the stored
/// value for this run.
pub async fn start_turn(
    project: &Path,
    prompt: &str,
    session: Option<SessionLog>,
    approve: Option<Approver>,
    ask: Option<Asker>,
    reasoning: Option<String>,
    inline: Vec<ContentPart>,
) -> Result<Turn> {
    let config = crate::manager::load_project_config_with(project, reasoning)
        .with_context(|| format!("loading configuration for {}", project.display()))?;
    config.require_api_key()?;

    // A leading `/command` is resolved here exactly as the CLI resolves it for
    // `-p` and rpc, so a command from the `/` menu (or typed by hand) reaches
    // the agent as its expanded prompt, with the agent routing its frontmatter
    // asked for.
    let resolved = runner::resolve_command(&config, prompt);
    let prompt = resolved.text;
    let command_agent = resolved.agent;
    let subtask = resolved.subtask;

    let ephemeral = config.ephemeral;
    let (history, log) = runner::begin_session(project, session, ephemeral, &prompt, &[], &inline)?;
    let session_id = log.as_ref().map(|entry| entry.id().to_string());

    let steering = Steering::new();
    let follow_ups = Steering::new();
    let cancel = Cancel::new();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let run = AgentRun {
        config,
        cwd: project.to_path_buf(),
        history,
        prompt,
        subtask,
        command_agent,
        session: log,
        approve,
        ask,
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

/// Raises the completion toast the terminal sends too, so a turn that finishes
/// while the window is elsewhere is announced. It carries the thread's
/// summarized title — the same label the sidebar and the CLI's session picker
/// show for it — since a notification is the one place the app has to say which
/// conversation just finished. `stopped` marks a turn the user ended
/// themselves, which has no outcome to announce; a missing or malformed session
/// leaves the plain body rather than failing the turn.
pub fn notify_finished(cwd: &Path, session_id: Option<&str>, stopped: bool) {
    let notify = oxide_core::notify::load_config(cwd);
    if stopped || !notify.on_complete {
        return;
    }
    let body = session_id
        .and_then(|id| summarized_title(cwd, id))
        .unwrap_or_else(|| "Turn complete".to_string());
    oxide_core::notify::send("Oxide", &body, notify.sound);
}

/// The thread's summarized title: its name when one was set, else the first
/// thing the user sent. `None` when the session is gone, was never written to,
/// or has nothing to name it with.
fn summarized_title(cwd: &Path, id: &str) -> Option<String> {
    let summary = SessionLog::open_ref(cwd, id).ok()?.summary().ok()?;
    session_label(&summary)
}

/// The label the sidebar shows a session under, used as the toast's body.
fn session_label(summary: &SessionSummary) -> Option<String> {
    let label = summary
        .name
        .clone()
        .unwrap_or_else(|| summary.preview.clone());
    let label = label.trim();
    (!label.is_empty()).then(|| label.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn session(name: Option<&str>, preview: &str) -> SessionSummary {
        SessionSummary {
            id: "s1".to_string(),
            name: name.map(|value| value.to_string()),
            cwd: "/tmp/project".to_string(),
            created_at: 0,
            modified_at: 0,
            message_count: 1,
            preview: preview.to_string(),
            path: PathBuf::from("/tmp/s1.jsonl"),
        }
    }

    #[test]
    fn the_toast_names_the_thread() {
        // The same label the sidebar and the CLI's picker show: a name when one
        // was set, else the summarized first message.
        assert_eq!(
            session_label(&session(Some("Ship the parser"), "ignored")).unwrap(),
            "Ship the parser"
        );
        assert_eq!(
            session_label(&session(None, "Fix the flaky test")).unwrap(),
            "Fix the flaky test"
        );
        assert_eq!(
            session_label(&session(Some("  "), "  ")),
            None,
            "an empty label leaves the plain body"
        );
    }
}
