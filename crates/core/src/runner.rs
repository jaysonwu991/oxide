//! Starting an agent turn.
//!
//! The CLI's print/JSON/RPC modes and the desktop app share this: it wires the
//! runtime (MCP, plugins, session, snapshots, LSP), resolves attachments, and
//! spawns the agent loop with events streamed to a channel.

use crate::agent::{self, AgentEvent, Approver, Cancel, Runtime, Steering};
use crate::config::Config;
use crate::llm::Message;
use crate::lsp::LspManager;
use crate::mcp::McpRegistry;
use crate::media;
use crate::plugin::PluginHost;
use crate::session::SessionLog;
use crate::snapshots::Snapshots;
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

/// Everything needed to start one agent run.
pub struct AgentRun {
    pub config: Config,
    pub cwd: PathBuf,
    pub history: Vec<Message>,
    pub prompt: String,
    pub subtask: bool,
    pub command_agent: Option<String>,
    pub session: Option<SessionLog>,
    /// Interactive approval callback. When `None`, `config.auto_approve`
    /// decides every `ask` rule.
    pub approve: Option<Approver>,
    /// Mid-run user messages injected between steps, and follow-ups queued for
    /// after the turn. Callers can share these to steer a running agent.
    pub steering: Steering,
    pub follow_ups: Steering,
    /// Cooperative cancellation for this run.
    pub cancel: Cancel,
}

/// Builds the user message, attaching inline images/PDFs referenced by the
/// prompt and any explicit attachments.
pub fn build_user_message(prompt: &str, cwd: &Path, attachments: &[PathBuf]) -> Result<Message> {
    let mut parts = Vec::new();
    for path in attachments {
        parts.push(media::load_attachment(path)?);
    }
    for path in media::referenced_attachments(prompt, cwd) {
        parts.push(media::load_attachment(&path)?);
    }
    if parts.is_empty() {
        Ok(Message::user(prompt))
    } else {
        Ok(Message::user_parts(prompt, parts))
    }
}

/// Wires the runtime and spawns the agent loop, streaming events to `tx`.
pub async fn spawn_agent(run: AgentRun, tx: UnboundedSender<AgentEvent>) -> JoinHandle<()> {
    let AgentRun {
        config,
        cwd,
        history,
        prompt,
        subtask,
        command_agent,
        session,
        approve,
        steering,
        follow_ups,
        cancel,
    } = run;
    let mcp = Arc::new(McpRegistry::new(&config.ecosystem.mcp));
    let plugins = Arc::new(PluginHost::spawn(&config.ecosystem.hooks, &cwd).await);
    let approver = approve.unwrap_or_else(|| default_approver(config.auto_approve));
    let runtime = Runtime {
        mcp,
        plugins,
        session: session.map(Arc::new),
        snapshots: Snapshots::open(&cwd).ok().map(Arc::new),
        lsp: Arc::new(LspManager::new()),
        approve: approver,
        steering,
        follow_ups,
        cancel,
    };

    if subtask {
        let agent_name = command_agent.unwrap_or_default();
        tokio::spawn(agent::run_subagent(
            config, cwd, history, agent_name, prompt, tx, runtime,
        ))
    } else {
        let mut config = config;
        if let Some(name) = command_agent {
            config.active_agent = config.ecosystem.agent(&name).cloned();
        }
        tokio::spawn(agent::run(config, cwd, history, tx, runtime))
    }
}

/// The non-interactive approver: allow when `auto_approve`, otherwise deny and
/// let the caller surface why.
pub fn default_approver(auto_approve: bool) -> Approver {
    Arc::new(move |_tool, _detail| Box::pin(async move { auto_approve }))
}

/// Creates a session unless the run is ephemeral, appends the user message, and
/// returns the resulting history plus the session handle.
pub fn begin_session(
    cwd: &Path,
    session: Option<SessionLog>,
    ephemeral: bool,
    prompt: &str,
    attachments: &[PathBuf],
) -> Result<(Vec<Message>, Option<SessionLog>)> {
    let log = match session {
        Some(log) => Some(log),
        None if ephemeral => None,
        None => Some(SessionLog::create(cwd)?),
    };
    let mut history = match &log {
        Some(log) => log.messages()?,
        None => Vec::new(),
    };
    let user = build_user_message(prompt, cwd, attachments)
        .with_context(|| "building the user message")?;
    if let Some(log) = &log {
        log.append(&user)?;
    }
    history.push(user);
    Ok((history, log))
}
