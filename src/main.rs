mod agent;
mod auth;
mod cli;
mod clipboard;
mod compact;
mod config;
mod dcp;
mod diff;
mod ecosystem;
mod html;
mod llm;
mod lsp;
mod mcp;
mod mcp_config;
mod mcp_oauth;
mod media;
mod memory;
mod permission;
mod plugin;
mod plugin_registry;
mod session;
mod sessions;
mod snapshots;
mod theme;
mod tools;
mod trust;
mod tui;
mod uninstall;

use agent::{AgentEvent, Approver, Runtime};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use config::Config;
use llm::Message;
use lsp::LspManager;
use mcp::McpRegistry;
use plugin::PluginHost;
use session::SessionLog;
use snapshots::Snapshots;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc::unbounded_channel;

#[derive(Parser, Debug)]
#[command(
    name = "oxide",
    version,
    about = "A native Rust AI coding agent CLI",
    args_conflicts_with_subcommands = true
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Prompt words and `@file` references (Pi-style `oxide @file "message"`)
    #[arg(
        value_name = "PROMPT",
        trailing_var_arg = true,
        allow_hyphen_values = true
    )]
    messages: Vec<String>,

    /// Model to use (overrides config)
    #[arg(short, long)]
    model: Option<String>,

    /// Provider name (overrides config)
    #[arg(long)]
    provider: Option<String>,

    /// Agent to run (from .oxide/agents or .claude/agents)
    #[arg(long)]
    agent: Option<String>,

    /// Mode: build/plan/auto-edit (permissions) or print/json/rpc (output)
    #[arg(long, value_name = "MODE")]
    mode: Option<String>,

    /// Reasoning effort: auto (default), off, low, medium, or high
    #[arg(long, value_name = "LEVEL")]
    reasoning: Option<String>,

    /// Append text to the system prompt (repeatable)
    #[arg(long = "append-system-prompt", value_name = "TEXT")]
    append_system_prompt: Vec<String>,

    /// Replace the default system prompt
    #[arg(long = "system-prompt", value_name = "TEXT")]
    system_prompt: Option<String>,

    /// Disable AGENTS.md and CLAUDE.md context file discovery
    #[arg(long = "no-context-files")]
    no_context_files: bool,

    /// Theme name for the TUI (dark, light, or a custom .oxide/themes file)
    #[arg(long = "use-theme", value_name = "NAME")]
    theme: Option<String>,

    /// Trust project-local resources for this run
    #[arg(short = 'a', long = "approve", conflicts_with = "no_approve")]
    approve: bool,

    /// Ignore project-local resources for this run
    #[arg(long = "no-approve", conflicts_with = "approve")]
    no_approve: bool,

    /// Allowlist specific tools (comma-separated); accepts Pi and legacy names
    #[arg(long = "tools", short = 't', value_name = "LIST")]
    tools: Option<String>,

    /// Disable specific tools (comma-separated)
    #[arg(long = "exclude-tools", short = 'x', value_name = "LIST")]
    exclude_tools: Option<String>,

    /// Use a specific session file or ID
    #[arg(long, value_name = "PATH|ID")]
    session: Option<String>,

    /// Set the session display name at startup
    #[arg(long, short = 'n', value_name = "NAME")]
    name: Option<String>,

    /// Ephemeral mode: do not save the session
    #[arg(long = "no-session")]
    no_session: bool,

    /// Print the response and exit instead of launching the TUI
    #[arg(short = 'p', long)]
    print: bool,

    /// Resume the most recent session for this project
    #[arg(short = 'c', long = "continue")]
    continue_session: bool,

    /// Browse and select a past session to resume (Pi-style `-r`)
    #[arg(short = 'r', long = "resume")]
    resume: bool,

    /// Fork a session file or id into a new session
    #[arg(long, value_name = "PATH|ID")]
    fork: Option<String>,

    /// Attach an image or PDF file to the prompt (repeatable)
    #[arg(long = "image", value_name = "PATH")]
    image: Vec<PathBuf>,

    /// Working directory for the agent
    #[arg(short = 'C', long)]
    cwd: Option<PathBuf>,
}

#[derive(Subcommand, Debug)]
#[allow(clippy::large_enum_variant)]
enum Command {
    /// Manage MCP servers
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
    /// Uninstall Oxide and remove related files
    Uninstall {
        /// Keep configuration files
        #[arg(short = 'c', long)]
        keep_config: bool,
        /// Keep session data, memory, and snapshots
        #[arg(short = 'd', long)]
        keep_data: bool,
        /// Show what would be removed without removing it
        #[arg(long)]
        dry_run: bool,
        /// Skip the confirmation prompt
        #[arg(short = 'f', long)]
        force: bool,
    },
    /// Manage saved sessions
    Sessions {
        #[command(subcommand)]
        action: SessionsAction,
    },
    /// Manage Claude Code-style plugins and marketplaces
    Plugin {
        #[command(subcommand)]
        action: PluginAction,
    },
}

#[derive(Subcommand, Debug)]
enum SessionsAction {
    /// List saved sessions
    List {
        /// Include sessions from every project
        #[arg(long)]
        all: bool,
        /// Only show sessions older than this many days
        #[arg(long)]
        older_than: Option<u64>,
    },
    /// Delete saved sessions
    Delete {
        /// Session id to delete
        id: Option<String>,
        /// Delete every session for this project
        #[arg(long)]
        all: bool,
        /// Delete sessions older than this many days
        #[arg(long)]
        older_than: Option<u64>,
        /// Skip the confirmation prompt
        #[arg(short = 'f', long)]
        force: bool,
    },
    /// Compact a saved session into a summary plus its most recent messages
    Compact {
        /// Session id, or omit with --all
        id: Option<String>,
        /// Compact every session for this project
        #[arg(long)]
        all: bool,
    },
    /// Merge two saved sessions into a new session
    Merge {
        /// First session id or path
        a: String,
        /// Second session id or path
        b: String,
        /// Summarize the second session instead of concatenating it verbatim
        #[arg(long)]
        summarize: bool,
    },
}

#[derive(Subcommand, Debug)]
enum PluginAction {
    /// List installed plugins
    List,
    /// Install a plugin from a configured marketplace
    Install {
        /// Plugin name, optionally `name@marketplace`
        name: String,
    },
    /// Uninstall a plugin
    Uninstall {
        /// Plugin name
        name: String,
    },
    /// Enable a disabled plugin
    Enable {
        /// Plugin name
        name: String,
    },
    /// Disable a plugin
    Disable {
        /// Plugin name
        name: String,
    },
    /// Manage plugin marketplaces
    Marketplace {
        #[command(subcommand)]
        action: MarketplaceAction,
    },
}

#[derive(Subcommand, Debug)]
enum MarketplaceAction {
    /// Add a marketplace from a git URL or local path
    Add {
        /// Git URL or local path containing .claude-plugin/marketplace.json
        source: String,
    },
    /// List configured marketplaces
    List,
    /// Remove a marketplace
    Remove {
        /// Marketplace name
        name: String,
    },
}

#[allow(clippy::large_enum_variant)]
#[derive(Subcommand, Debug)]
enum McpAction {
    /// List configured MCP servers
    List,
    /// Show a server's configuration
    Get {
        /// Server name
        name: String,
    },
    /// Add an MCP server
    Add {
        /// Server name
        name: String,
        /// Command and arguments (stdio) or URL (http)
        #[arg(allow_hyphen_values = true)]
        command: Vec<String>,
        /// Transport: stdio (default) or http
        #[arg(long)]
        transport: Option<String>,
        /// Environment variable KEY=VALUE (repeatable, stdio)
        #[arg(long = "env", value_name = "KEY=VALUE")]
        env: Vec<String>,
        /// HTTP header KEY=VALUE (repeatable, http)
        #[arg(long = "header", value_name = "KEY=VALUE")]
        header: Vec<String>,
        /// Routing domains owned by the server (repeatable or comma-separated)
        #[arg(long = "domains", value_name = "DOMAIN")]
        domains: Vec<String>,
        /// Working directory for a stdio server
        #[arg(long)]
        cwd: Option<String>,
        /// OAuth client ID (remote server)
        #[arg(long)]
        oauth_client_id: Option<String>,
        /// OAuth client secret (remote server)
        #[arg(long)]
        oauth_client_secret: Option<String>,
        /// OAuth loopback callback port (remote server)
        #[arg(long)]
        callback_port: Option<u16>,
        /// OAuth scope (repeatable, remote server)
        #[arg(long = "oauth-scope", value_name = "SCOPE")]
        oauth_scope: Vec<String>,
        /// OAuth redirect URI override (remote server)
        #[arg(long)]
        redirect_uri: Option<String>,
        /// Where to store the server: project (default) or global
        #[arg(short, long)]
        scope: Option<String>,
    },
    /// Add a server from a JSON object
    AddJson {
        /// Server name
        name: String,
        /// JSON object with `command` (stdio) or `url` (http)
        json: String,
        /// Where to store the server: project (default) or global
        #[arg(short, long)]
        scope: Option<String>,
    },
    /// Remove an MCP server
    Remove {
        /// Server name
        name: String,
        /// Where to remove from: project (default) or global
        #[arg(short, long)]
        scope: Option<String>,
    },
    /// Authorize an OAuth-protected remote server
    Auth {
        /// Server name
        name: String,
        /// Where to look for the server: project (default) or global
        #[arg(short, long)]
        scope: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        return match command {
            Command::Mcp { action } => {
                let current_dir = std::env::current_dir().context("resolving current directory")?;
                match action {
                    McpAction::List => mcp_config::list(&current_dir).await,
                    McpAction::Get { name } => mcp_config::get(&current_dir, &name),
                    McpAction::Add {
                        name,
                        command,
                        transport,
                        env,
                        header,
                        domains,
                        cwd,
                        oauth_client_id,
                        oauth_client_secret,
                        callback_port,
                        oauth_scope,
                        redirect_uri,
                        scope,
                    } => mcp_config::add(
                        &current_dir,
                        mcp_config::AddRequest {
                            scope,
                            transport,
                            name,
                            command,
                            env,
                            header,
                            domains,
                            cwd,
                            oauth_client_id,
                            oauth_client_secret,
                            callback_port,
                            oauth_scope,
                            redirect_uri,
                        },
                    ),
                    McpAction::AddJson { name, json, scope } => {
                        mcp_config::add_json(&current_dir, scope, name, &json)
                    }
                    McpAction::Remove { name, scope } => {
                        mcp_config::remove(&current_dir, scope, name)
                    }
                    McpAction::Auth { name, scope } => {
                        mcp_config::auth(&current_dir, scope, name).await
                    }
                }
            }
            Command::Uninstall {
                keep_config,
                keep_data,
                dry_run,
                force,
            } => uninstall::run(uninstall::Options {
                keep_config,
                keep_data,
                dry_run,
                force,
            }),
            Command::Sessions { action } => {
                let current_dir = std::env::current_dir().context("resolving current directory")?;
                match action {
                    SessionsAction::List { all, older_than } => {
                        sessions::list(&current_dir, all, older_than)
                    }
                    SessionsAction::Delete {
                        id,
                        all,
                        older_than,
                        force,
                    } => sessions::delete(&current_dir, id, all, older_than, force),
                    SessionsAction::Compact { id, all } => {
                        let config = Config::load(&current_dir, None, None, None, None, None)?;
                        config.require_api_key()?;
                        sessions::compact_sessions(&current_dir, &config, id, all).await
                    }
                    SessionsAction::Merge { a, b, summarize } => {
                        let config = if summarize {
                            let config = Config::load(&current_dir, None, None, None, None, None)?;
                            config.require_api_key()?;
                            Some(config)
                        } else {
                            None
                        };
                        sessions::merge(&current_dir, config.as_ref(), &a, &b, summarize).await
                    }
                }
            }
            Command::Plugin { action } => {
                match action {
                    PluginAction::List => println!("{}", plugin_registry::list()?),
                    PluginAction::Install { name } => {
                        let (name, marketplace) = plugin_registry::split_ref(&name);
                        println!(
                            "{}",
                            plugin_registry::install(&name, marketplace.as_deref()).await?
                        );
                    }
                    PluginAction::Uninstall { name } => {
                        println!("{}", plugin_registry::uninstall(&name)?);
                    }
                    PluginAction::Enable { name } => {
                        println!("{}", plugin_registry::set_enabled(&name, true)?);
                    }
                    PluginAction::Disable { name } => {
                        println!("{}", plugin_registry::set_enabled(&name, false)?);
                    }
                    PluginAction::Marketplace { action } => match action {
                        MarketplaceAction::Add { source } => {
                            println!("{}", plugin_registry::add_marketplace(&source).await?);
                        }
                        MarketplaceAction::List => {
                            println!("{}", plugin_registry::list_marketplaces()?);
                        }
                        MarketplaceAction::Remove { name } => {
                            println!("{}", plugin_registry::remove_marketplace(&name)?);
                        }
                    },
                }
                Ok(())
            }
        };
    }
    let cwd = match &cli.cwd {
        Some(path) => path.clone(),
        None => std::env::current_dir().context("resolving current directory")?,
    };
    // `--mode` carries either a permission mode (build/plan/auto-edit) or an
    // output mode (print/json/rpc). Pi names its output modes this way, so we
    // disambiguate by value and forward each to the right place.
    let (permission_mode, mode) = split_mode(cli.mode.as_deref());
    let config = Config::load(
        &cwd,
        cli.model,
        cli.provider,
        cli.agent,
        permission_mode,
        cli.reasoning,
    )?;
    let mut config = config;
    config.ephemeral = cli.no_session;
    config.load_context_files = !cli.no_context_files;
    let theme_name = cli.theme.clone().unwrap_or_else(|| {
        std::fs::read_to_string(Config::config_path())
            .ok()
            .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
            .and_then(|value| {
                value
                    .get("theme")
                    .and_then(|t| t.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "dark".to_string())
    });
    config.theme = theme::load(&cwd, &theme_name);
    if cli.no_context_files {
        config.ecosystem = ecosystem::load_with(&cwd, false);
    }
    // Resolve project trust. Non-interactive modes never prompt: they use a
    // saved decision, else `defaultProjectTrust` (`ask`/`never` ignore project
    // resources, `always` trusts them). `--approve`/`--no-approve` override.
    let override_decision = if cli.approve {
        Some(true)
    } else if cli.no_approve {
        Some(false)
    } else {
        None
    };
    let trust_store = trust::TrustStore::load().unwrap_or_default();
    config.trusted = trust::resolve(
        &trust_store,
        &cwd,
        override_decision,
        config.default_project_trust,
    )
    .is_trusted();
    if !config.trusted {
        config.reload_ecosystem(&cwd);
    }
    if let Some(prompt) = &cli.system_prompt {
        config.system_prompt = prompt.clone();
    }
    for extra in &cli.append_system_prompt {
        config.system_prompt.push_str("\n\n");
        config.system_prompt.push_str(extra);
    }
    let tool_filter = cli::ToolFilter::new(cli.tools.clone(), cli.exclude_tools.clone());

    let session = if cli.no_session {
        None
    } else if let Some(reference) = &cli.session {
        Some(SessionLog::open_ref(&cwd, reference)?)
    } else if let Some(reference) = &cli.fork {
        let source = SessionLog::open_ref(&cwd, reference)?;
        let messages = source.messages()?;
        Some(SessionLog::fork(&cwd, &messages)?)
    } else if cli.continue_session {
        Some(SessionLog::latest(&cwd).context("no previous session found for this project")?)
    } else {
        None
    };
    // `--name` sets a display name, creating a session to hold it unless the
    // run is ephemeral.
    let session = match (cli.name.as_deref(), session) {
        (Some(name), Some(log)) => {
            log.set_name(name)?;
            Some(log)
        }
        (Some(name), None) if !cli.no_session => {
            let log = SessionLog::create(&cwd)?;
            log.set_name(name)?;
            Some(log)
        }
        (_, session) => session,
    };

    let mode = mode.as_str();
    let positional = cli.messages;
    let explicit_prompt = cli.print || !positional.is_empty();

    if cli.resume {
        if mode == "rpc" {
            anyhow::bail!(
                "-r/--resume opens the interactive session picker and cannot be used with --mode rpc"
            );
        }
        if explicit_prompt {
            anyhow::bail!(
                "-r/--resume opens the interactive session picker; to resume a specific session with a prompt use --session <id>"
            );
        }
    }

    if mode == "rpc" {
        return run_rpc_mode(config, cwd, session).await;
    }

    if explicit_prompt {
        let (mut prompt, attachments) = build_prompt(&cwd, positional, &cli.image)?;
        // Merge piped stdin into the prompt (Pi's print-mode behavior). When
        // there is no positional prompt, stdin becomes the whole prompt.
        if !io::stdin().is_terminal() {
            let mut buffer = String::new();
            io::stdin()
                .read_to_string(&mut buffer)
                .context("reading prompt from stdin")?;
            let buffer = buffer.trim_end();
            if !buffer.is_empty() {
                if prompt.trim().is_empty() {
                    prompt = buffer.to_string();
                } else {
                    prompt.push_str("\n\n");
                    prompt.push_str(buffer);
                }
            }
        }
        let output = match mode {
            "json" => cli::OutputMode::Json,
            "print" | "text" => cli::OutputMode::Print,
            other => {
                anyhow::bail!("unknown --mode `{other}` (expected print, json, or rpc)")
            }
        };
        run_print(
            config,
            cwd,
            prompt,
            session,
            attachments,
            output,
            tool_filter,
        )
        .await
    } else {
        if mode != "print" {
            anyhow::bail!("--mode {mode} requires an initial prompt");
        }
        tui::run(config, cwd, session, cli.resume && !cli.no_session).await
    }
}

async fn run_print(
    mut config: Config,
    cwd: PathBuf,
    prompt: String,
    session: Option<SessionLog>,
    attachments: Vec<PathBuf>,
    output: cli::OutputMode,
    tool_filter: cli::ToolFilter,
) -> Result<()> {
    config.require_api_key()?;
    config.tool_filter = tool_filter.clone();
    let resolved = config.resolve_command(&prompt);
    let prompt = resolved
        .as_ref()
        .map(|command| command.prompt.clone())
        .unwrap_or(prompt);
    let command_agent = resolved.as_ref().and_then(|command| command.agent.clone());
    let subtask = resolved.as_ref().is_some_and(|command| command.subtask);
    // `--no-session` gives a throwaway log that is never persisted, so the run
    // behaves like Pi's ephemeral mode while keeping the agent loop unchanged.
    let ephemeral = config.ephemeral;
    let log = match session {
        Some(log) => Some(log),
        None if ephemeral => None,
        None => Some(SessionLog::create(&cwd)?),
    };
    let mut history = match &log {
        Some(log) => log.messages()?,
        None => Vec::new(),
    };
    let user = build_user_message(&prompt, &cwd, &attachments)?;
    if let Some(log) = &log {
        log.append(&user)?;
    }
    history.push(user);

    let (tx, rx) = unbounded_channel();
    let request = RunRequest {
        config: &config,
        cwd: &cwd,
        history,
        prompt,
        subtask,
        command_agent,
        log: log.clone(),
    };
    spawn_agent(request, tx).await;

    match output {
        cli::OutputMode::Print => run_print_text(rx).await,
        cli::OutputMode::Json => {
            let header = log.as_ref().map(cli::session_header);
            cli::run_json(rx, header).await
        }
    }
}

async fn run_print_text(mut rx: tokio::sync::mpsc::UnboundedReceiver<AgentEvent>) -> Result<()> {
    let mut stdout = io::stdout();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Text(delta) => {
                print!("{delta}");
                stdout.flush()?;
            }
            AgentEvent::Thought { .. } => {}
            AgentEvent::ToolCall { name, args } => {
                eprintln!("\n[tool] {name} {args}");
            }
            AgentEvent::ToolProgress { chunk, .. } => {
                eprintln!("{chunk}");
            }
            AgentEvent::ToolResult { name, output, .. } => {
                eprintln!("[result: {name}] {} bytes", output.len());
            }
            AgentEvent::Usage { .. } => {}
            AgentEvent::Error(message) => {
                eprintln!("\nerror: {message}");
            }
            AgentEvent::Finished(_) => break,
        }
    }
    println!();
    Ok(())
}

fn build_user_message(prompt: &str, cwd: &Path, attachments: &[PathBuf]) -> Result<Message> {
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

/// Builds the initial prompt from positional arguments, expanding `@file`
/// references and returning any image/PDF attachments separately.
/// Splits `--mode` into an optional permission mode and an output mode.
/// Values that name an output mode (`print`, `text`, `json`, `rpc`) go to the
/// output slot; everything else is treated as a permission mode.
fn split_mode(value: Option<&str>) -> (Option<String>, String) {
    match value {
        None => (None, "print".to_string()),
        Some(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "print" | "text" | "json" | "rpc" => (None, normalized),
                _ => (Some(raw.to_string()), "print".to_string()),
            }
        }
    }
}

fn build_prompt(
    cwd: &Path,
    positional: Vec<String>,
    images: &[PathBuf],
) -> Result<(String, Vec<PathBuf>)> {
    if positional.is_empty() {
        return Ok((String::new(), images.to_vec()));
    }
    let expanded = cli::expand_file_args(cwd, &positional)?;
    let mut attachments = expanded.attachments;
    attachments.extend(images.iter().cloned());
    Ok((expanded.text, attachments))
}

/// Everything needed to start one agent run, bundled so the print, JSON, and
/// RPC entry points share a single spawn path.
struct RunRequest<'a> {
    config: &'a Config,
    cwd: &'a Path,
    history: Vec<Message>,
    prompt: String,
    subtask: bool,
    command_agent: Option<String>,
    log: Option<SessionLog>,
}

/// Wires the runtime (MCP, plugins, session, snapshots, LSP) and spawns the
/// agent loop, streaming events to `tx`.
async fn spawn_agent(request: RunRequest<'_>, tx: tokio::sync::mpsc::UnboundedSender<AgentEvent>) {
    let RunRequest {
        config,
        cwd,
        history,
        prompt,
        subtask,
        command_agent,
        log,
    } = request;
    let mcp = Arc::new(McpRegistry::new(&config.ecosystem.mcp));
    let plugins = Arc::new(PluginHost::spawn(&config.ecosystem.plugins, cwd).await);
    let auto_approve = config.auto_approve;
    let approve: Approver = Arc::new(move |tool, detail| {
        if !auto_approve {
            eprintln!(
                "permission required for `{tool}` ({detail}); denying (auto_approve is false)"
            );
        }
        Box::pin(async move { auto_approve })
    });
    let runtime = Runtime {
        mcp,
        plugins,
        session: log.map(Arc::new),
        snapshots: Snapshots::open(cwd).ok().map(Arc::new),
        lsp: Arc::new(LspManager::new()),
        approve,
        steering: crate::agent::Steering::new(),
        follow_ups: crate::agent::Steering::new(),
    };

    if subtask {
        let agent_name = command_agent.unwrap_or_default();
        tokio::spawn(agent::run_subagent(
            config.clone(),
            cwd.to_path_buf(),
            history,
            agent_name,
            prompt,
            tx,
            runtime,
        ));
    } else {
        let mut config = config.clone();
        if let Some(name) = command_agent {
            config.active_agent = config.ecosystem.agent(&name).cloned();
        }
        tokio::spawn(agent::run(config, cwd.to_path_buf(), history, tx, runtime));
    }
}

/// RPC mode: reads JSONL prompts from stdin and streams JSONL events to stdout.
/// Each `prompt` request starts a fresh agent run on the session history.
async fn run_rpc_mode(config: Config, cwd: PathBuf, session: Option<SessionLog>) -> Result<()> {
    config.require_api_key()?;
    let ephemeral = config.ephemeral;
    let mut log = session;
    let mut history = match &log {
        Some(log) => log.messages()?,
        None => Vec::new(),
    };

    let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    let (event_tx, event_rx) = unbounded_channel();
    let mut out = io::stdout();

    let driver = tokio::spawn(async move {
        while let Some(prompt) = prompt_rx.recv().await {
            if prompt.is_empty() {
                break;
            }
            if log.is_none() && !ephemeral {
                log = Some(SessionLog::create(&cwd)?);
            }
            let resolved = config.resolve_command(&prompt);
            let text = resolved
                .as_ref()
                .map(|command| command.prompt.clone())
                .unwrap_or(prompt);
            let command_agent = resolved.as_ref().and_then(|command| command.agent.clone());
            let subtask = resolved.as_ref().is_some_and(|command| command.subtask);
            let user = build_user_message(&text, &cwd, &[])?;
            if let Some(log) = &log {
                log.append(&user)?;
            }
            history.push(user);
            let (run_tx, mut run_rx) = unbounded_channel();
            let request = RunRequest {
                config: &config,
                cwd: &cwd,
                history: history.clone(),
                prompt: text,
                subtask,
                command_agent,
                log: log.clone(),
            };
            spawn_agent(request, run_tx).await;
            while let Some(event) = run_rx.recv().await {
                let finished = matches!(event, AgentEvent::Finished(_));
                if let AgentEvent::Finished(messages) = &event {
                    history = messages.clone();
                }
                if event_tx.send(event).is_err() {
                    break;
                }
                if finished {
                    break;
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let result = cli::run_rpc(event_rx, prompt_tx).await;
    let _ = driver.await;
    writeln!(out)?;
    result
}

#[cfg(test)]
mod tests {
    use super::{split_mode, Cli, Command};
    use clap::Parser;

    #[test]
    fn mode_splits_permission_and_output() {
        assert_eq!(split_mode(None), (None, "print".to_string()));
        assert_eq!(split_mode(Some("json")), (None, "json".to_string()));
        assert_eq!(split_mode(Some("RPC")), (None, "rpc".to_string()));
        assert_eq!(split_mode(Some("print")), (None, "print".to_string()));
        assert_eq!(
            split_mode(Some("plan")),
            (Some("plan".to_string()), "print".to_string())
        );
        assert_eq!(
            split_mode(Some("auto-edit")),
            (Some("auto-edit".to_string()), "print".to_string())
        );
    }

    #[test]
    fn parses_uninstall_options() {
        let cli = Cli::try_parse_from([
            "oxide",
            "uninstall",
            "--keep-config",
            "--keep-data",
            "--dry-run",
            "--force",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Uninstall {
                keep_config: true,
                keep_data: true,
                dry_run: true,
                force: true,
            })
        ));
    }
}
