//! Starting an agent turn.
//!
//! The CLI's print/JSON/RPC modes and the desktop app share this: it wires the
//! runtime (MCP, plugins, session, snapshots, LSP), resolves attachments, and
//! spawns the agent loop with events streamed to a channel.

use crate::agent::{self, AgentEvent, Approver, Cancel, Runtime, Steering};
use crate::ask::Asker;
use crate::config::Config;
use crate::llm::{ContentPart, Message};
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

/// The parts of a run that outlive one turn: connected MCP servers, the plugin
/// host, and language servers. A long-lived front-end — the desktop harness and
/// an `--mode rpc` process — keeps one per project and hands it to each turn, so
/// a server is not started again for every prompt and a language server warmed
/// by one edit can answer the next.
#[derive(Clone)]
pub struct TurnEnvironment {
    pub mcp: Arc<McpRegistry>,
    pub plugins: Arc<PluginHost>,
    pub lsp: Arc<LspManager>,
    key: String,
}

impl TurnEnvironment {
    pub async fn new(config: &Config, cwd: &Path) -> Self {
        Self {
            mcp: Arc::new(McpRegistry::new(&config.ecosystem.mcp)),
            plugins: Arc::new(PluginHost::spawn(&config.ecosystem.hooks, cwd).await),
            lsp: Arc::new(LspManager::new()),
            key: environment_key(config, cwd),
        }
    }

    /// Whether this environment was built for the same project and the same MCP
    /// servers, hook plugins and installed packages. A configuration that
    /// gained or lost one needs a new host, so a front-end rebuilds instead of
    /// reusing a server list that no longer matches.
    pub fn suits(&self, config: &Config, cwd: &Path) -> bool {
        self.key == environment_key(config, cwd)
    }
}

/// The identity of an environment: the project, its MCP endpoints, its hook
/// files and its plugin packages. The rest of the configuration is per-turn and
/// reaches the run through `AgentRun::config`.
fn environment_key(config: &Config, cwd: &Path) -> String {
    let cwd = std::fs::canonicalize(cwd).unwrap_or_else(|_| cwd.to_path_buf());
    // The whole entry, not just its endpoint: a changed header, environment or
    // OAuth block is a different server, and an enabled/disabled toggle changes
    // what the run should see.
    let mcp: Vec<String> = config
        .ecosystem
        .mcp
        .iter()
        .map(|server| format!("{server:?}"))
        .collect();
    let plugins: Vec<String> = config
        .ecosystem
        .plugins
        .iter()
        .map(|plugin| format!("{plugin:?}"))
        .collect();
    format!(
        "{}\u{0}{mcp:?}\u{0}{:?}\u{0}{plugins:?}",
        cwd.display(),
        config.ecosystem.hooks
    )
}

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
    /// How the model's questions reach the user. `None` leaves the `ask` tool
    /// out of the run entirely, so a front-end that cannot answer one never has
    /// the model ask and then wait out the timeout.
    pub ask: Option<Asker>,
    /// Mid-run user messages injected between steps, and follow-ups queued for
    /// after the turn. Callers can share these to steer a running agent.
    pub steering: Steering,
    pub follow_ups: Steering,
    /// Cooperative cancellation for this run.
    pub cancel: Cancel,
}

/// A prompt a front-end is about to send, with any leading `/command` already
/// expanded.
pub struct Prompt {
    pub text: String,
    /// The agent the command's frontmatter asked for, if any.
    pub agent: Option<String>,
    /// Whether the command runs as a subagent rather than in this transcript.
    pub subtask: bool,
}

/// Resolves a leading `/command` against the ecosystem the way every front-end
/// does, so a command from a client's `/` menu and one typed by hand behave
/// identically. A prompt that names no command passes through unchanged, and an
/// unknown `/name` is sent as written rather than swallowed.
pub fn resolve_command(config: &Config, prompt: &str) -> Prompt {
    match config.resolve_command(prompt) {
        Some(resolved) => Prompt {
            text: resolved.prompt,
            agent: resolved.agent,
            subtask: resolved.subtask,
        },
        None => Prompt {
            text: prompt.to_string(),
            agent: None,
            subtask: false,
        },
    }
}

/// Builds the user message, attaching the inline media and text referenced by
/// the prompt, explicit attachment paths, and already-loaded content parts (the
/// desktop sends pasted files as data URLs, which have no path on disk).
pub fn build_user_message(
    prompt: &str,
    cwd: &Path,
    attachments: &[PathBuf],
    inline: &[ContentPart],
) -> Result<Message> {
    build_user_message_scoped(prompt, cwd, attachments, inline, false)
}

/// Builds a user message while optionally applying the CLI/extension project
/// boundary to `@path` references the prompt asks Oxide to resolve. Explicit
/// attachments are user-selected inputs: the CLI validates `--image` itself,
/// while the extension validates file attachments and may use an outside temp
/// file for pasted bytes.
pub fn build_user_message_scoped(
    prompt: &str,
    cwd: &Path,
    attachments: &[PathBuf],
    inline: &[ContentPart],
    project_only: bool,
) -> Result<Message> {
    let mut parts: Vec<ContentPart> = inline.to_vec();
    for path in attachments {
        parts.push(media::load_attachment(path)?);
    }
    for path in media::referenced_attachments(prompt, cwd) {
        if project_only {
            crate::tools::ensure_project_path(cwd, &path.to_string_lossy())?;
        }
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
    let environment = TurnEnvironment::new(&run.config, &run.cwd).await;
    spawn_agent_in(run, environment, tx).await
}

/// The same, on an environment a long-lived front-end keeps across turns.
pub async fn spawn_agent_in(
    run: AgentRun,
    environment: TurnEnvironment,
    tx: UnboundedSender<AgentEvent>,
) -> JoinHandle<()> {
    let AgentRun {
        config,
        cwd,
        history,
        prompt,
        subtask,
        command_agent,
        session,
        approve,
        ask,
        steering,
        follow_ups,
        cancel,
    } = run;
    let approver = approve.unwrap_or_else(|| default_approver(config.auto_approve));
    let runtime = Runtime {
        mcp: environment.mcp,
        selected_mcp_tools: Default::default(),
        plugins: environment.plugins,
        session: session.map(Arc::new),
        snapshots: Snapshots::open(&cwd).ok().map(Arc::new),
        lsp: environment.lsp,
        approve: approver,
        ask,
        steering,
        follow_ups,
        steering_mode: config.steering_mode,
        follow_up_mode: config.follow_up_mode,
        cancel,
        cache_warm: None,
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
        let mut runtime = runtime;
        // Keep the conversation warm between turns. The agent arms this when it
        // finishes, so the approval and question events keep their place on the
        // same channel rather than passing through a wrapper.
        runtime.cache_warm = Some(agent::WarmContext {
            config: config.clone(),
            cwd: cwd.clone(),
            session: runtime
                .session
                .as_ref()
                .map(|session| session.id().to_string()),
        });
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
    inline: &[ContentPart],
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
    let user = build_user_message(prompt, cwd, attachments, inline)
        .with_context(|| "building the user message")?;
    if let Some(log) = &log {
        log.append(&user)?;
    }
    history.push(user);
    Ok((history, log))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static NEXT: AtomicUsize = AtomicUsize::new(0);

    #[test]
    fn an_environment_key_follows_the_project_and_its_servers() {
        use crate::ecosystem::{McpKind, McpServer};

        let base = std::env::temp_dir().join(format!("oxide_env_key_{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        let mut config = Config::default();
        let first = environment_key(&config, &base);
        assert_eq!(first, environment_key(&config, &base));
        assert_ne!(first, environment_key(&config, &base.join("other")));

        config.ecosystem.mcp.push(McpServer {
            name: "docs".into(),
            enabled: true,
            kind: McpKind::Remote {
                url: "https://example.com/mcp".into(),
                headers: Default::default(),
                oauth: None,
            },
            domains: Vec::new(),
        });
        let with_server = environment_key(&config, &base);
        assert_ne!(first, with_server, "a new server needs a new registry");

        config.ecosystem.mcp[0].enabled = false;
        assert_ne!(
            with_server,
            environment_key(&config, &base),
            "turning one off is a different set"
        );

        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn project_scoped_messages_refuse_at_paths_outside_the_project() {
        let base = std::env::temp_dir().join(format!(
            "oxide_runner_scope_{}_{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let project = base.join("project");
        let sibling = base.join("sibling");
        std::fs::create_dir_all(&project).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let outside = sibling.join("brief.pdf");
        std::fs::write(&outside, b"%PDF-1.4\n").unwrap();

        assert!(build_user_message_scoped(
            &format!("review @{}", outside.display()),
            &project,
            &[],
            &[],
            true,
        )
        .is_err());

        std::fs::remove_dir_all(base).ok();
    }
}
