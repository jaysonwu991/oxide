mod theme;
mod tui;
mod uninstall;

// The shared agent core (config, providers, tools, MCP, sessions, snapshots,
// plugins, agent loop) is re-exported at the crate root so existing `crate::`
// paths keep resolving with the same names as before the workspace split.
pub use oxide_core::{
    agent, approval, approvals, auth, cli, clipboard, commands, compact, config, diff, ecosystem,
    html, llm, lsp, mcp, mcp_config, mcp_oauth, media, memory, notify, permission, plugin,
    plugin_registry, portkey_usage, pricing, runner, session, sessions, snapshots, tools, trust,
};

use agent::{AgentEvent, Approver, Cancel, Steering};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cli::RpcRequest;
use config::Config;
use session::SessionLog;
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

    /// Output mode: `print`, `json`, or `rpc` (defaults to print for prompts)
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

    /// Ask before running a tool whose permission rule requires approval
    /// (the TUI, and `--mode rpc` where the front-end answers over its channel)
    #[arg(long = "ask-approvals", conflicts_with = "no_ask_approvals")]
    ask_approvals: bool,

    /// Run permission-gated tools without asking
    #[arg(long = "no-ask-approvals", conflicts_with = "ask_approvals")]
    no_ask_approvals: bool,

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
    /// List the slash commands a client can offer
    Commands {
        /// Print the listing as JSON
        #[arg(long)]
        json: bool,
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
    #[command(visible_alias = "remove")]
    Uninstall {
        /// Plugin name, optionally `name@marketplace`
        name: String,
    },
    /// Enable a disabled plugin
    Enable {
        /// Plugin name, optionally `name@marketplace`
        name: String,
    },
    /// Disable a plugin
    Disable {
        /// Plugin name, optionally `name@marketplace`
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
    /// Fetch the latest manifest from a marketplace's git remote
    Update {
        /// Marketplace name
        name: String,
    },
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
    List {
        /// Print the listing as JSON
        #[arg(long)]
        json: bool,
    },
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
        /// Limit the removal to project or global (default: any source)
        #[arg(short, long)]
        scope: Option<String>,
    },
    /// Authorize an OAuth-protected remote server
    Auth {
        /// Server name
        name: String,
        /// Limit the lookup to project or global (default: any source)
        #[arg(short, long)]
        scope: Option<String>,
    },
    /// Turn a server off without removing its configuration
    Disable {
        /// Server name
        name: String,
        /// Limit the change to project or global (default: the source that defines it)
        #[arg(short, long)]
        scope: Option<String>,
    },
    /// Turn a disabled server back on
    Enable {
        /// Server name
        name: String,
        /// Limit the change to project or global (default: the source that defines it)
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
                    McpAction::List { json } => {
                        if json {
                            mcp_config::list_json(&current_dir).await
                        } else {
                            mcp_config::list(&current_dir).await
                        }
                    }
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
                    McpAction::Enable { name, scope } => {
                        mcp_config::set_enabled(&current_dir, scope, name, true)
                    }
                    McpAction::Disable { name, scope } => {
                        mcp_config::set_enabled(&current_dir, scope, name, false)
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
                        let config = Config::load(&current_dir, None, None, None, None)?;
                        config.require_api_key()?;
                        sessions::compact_sessions(&current_dir, &config, id, all).await
                    }
                    SessionsAction::Merge { a, b, summarize } => {
                        let config = if summarize {
                            let config = Config::load(&current_dir, None, None, None, None)?;
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
                        MarketplaceAction::Update { name } => {
                            println!("{}", plugin_registry::update_marketplace(&name).await?);
                        }
                        MarketplaceAction::Remove { name } => {
                            println!("{}", plugin_registry::remove_marketplace(&name)?);
                        }
                    },
                }
                Ok(())
            }
            Command::Commands { json } => {
                let current_dir = std::env::current_dir().context("resolving current directory")?;
                commands::list(&current_dir, json)
            }
        };
    }
    let cwd = match &cli.cwd {
        Some(path) => path.clone(),
        None => std::env::current_dir().context("resolving current directory")?,
    };
    // `--mode` selects the output mode, like Pi: `json` or `rpc` (and `print`
    // for compatibility; `-p`/`--print` also selects print mode). There is no
    // permission mode; use `--tools`/`--exclude-tools` for a read-only run.
    let mode = parse_mode(cli.mode.as_deref())?;
    let config = Config::load(&cwd, cli.model, cli.provider, cli.agent, cli.reasoning)?;
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
        return run_rpc_mode(
            config,
            cwd,
            session,
            tool_filter,
            cli.ask_approvals,
            cli.no_ask_approvals,
        )
        .await;
    }

    // Only rpc mode and the interactive TUI can carry an answer; every other
    // mode has no channel, so the flag would silently run the tool it was meant
    // to gate.
    if cli.ask_approvals || cli.no_ask_approvals {
        if explicit_prompt || mode != "print" {
            anyhow::bail!(
                "--ask-approvals/--no-ask-approvals need the interactive TUI or --mode rpc, where the question can be answered"
            );
        }
        config.auto_approve =
            resolve_auto_approve(cli.ask_approvals, cli.no_ask_approvals, config.auto_approve);
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
        tui::run(
            config,
            cwd,
            session,
            cli.resume && !cli.no_session,
            theme_name,
        )
        .await
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
    let resolved = runner::resolve_command(&config, &prompt);
    let prompt = resolved.text;
    let command_agent = resolved.agent;
    let subtask = resolved.subtask;
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
    let user = runner::build_user_message(&prompt, &cwd, &attachments, &[])?;
    if let Some(log) = &log {
        log.append(&user)?;
    }
    history.push(user);

    let (tx, rx) = unbounded_channel();
    let run = runner::AgentRun {
        config: config.clone(),
        cwd: cwd.clone(),
        history,
        prompt,
        subtask,
        command_agent,
        session: log.clone(),
        approve: Some(cli_approver(config.auto_approve)),
        steering: Steering::new(),
        follow_ups: Steering::new(),
        cancel: Cancel::new(),
    };
    runner::spawn_agent(run, tx).await;

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
    // Text is held until the step commits (a tool call, or the turn ending): a
    // dropped stream is retried from scratch, so a partial attempt flushed the
    // moment it arrived would be duplicated once the retry re-sends it.
    let mut pending = String::new();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Text(delta) => pending.push_str(&delta),
            AgentEvent::Thought { .. } => {}
            // A no-tool step commits here, so a later retry only clears the
            // current step instead of text already written to stdout.
            AgentEvent::ThoughtDone { .. } => flush_stdout(&mut stdout, &mut pending)?,
            AgentEvent::ThinkingDelta(_) => {}
            AgentEvent::SubagentActivity { agent, tool, args } => {
                eprintln!("[{agent}] {tool} {args}");
            }
            AgentEvent::Retrying {
                attempt,
                max,
                delay_ms,
            } => {
                pending.clear();
                eprintln!("[retry {attempt}/{max} in {delay_ms}ms]");
            }
            AgentEvent::ToolCall { name, args } => {
                flush_stdout(&mut stdout, &mut pending)?;
                eprintln!("\n[tool] {name} {args}");
            }
            AgentEvent::ToolProgress { chunk, .. } => {
                eprintln!("{chunk}");
            }
            AgentEvent::ToolResult { name, output, .. } => {
                eprintln!("[result: {name}] {} bytes", output.len());
            }
            AgentEvent::Usage { .. } => {}
            // Print mode has no way to ask: it always runs a non-interactive
            // approver, so a request here can only be a no-op.
            AgentEvent::ApprovalRequest { .. } => {}
            AgentEvent::Compaction {
                summarized,
                tokens_before,
                ..
            } => {
                eprintln!(
                    "[compaction] summarized {summarized} messages (~{tokens_before} tokens)"
                );
            }
            AgentEvent::Branch { .. } => {}
            AgentEvent::Error(message) => {
                flush_stdout(&mut stdout, &mut pending)?;
                eprintln!("\nerror: {message}");
            }
            AgentEvent::Finished(_) => {
                flush_stdout(&mut stdout, &mut pending)?;
                break;
            }
        }
    }
    println!();
    Ok(())
}

/// Writes the text buffered for the current step to stdout.
fn flush_stdout(stdout: &mut impl Write, pending: &mut String) -> Result<()> {
    if !pending.is_empty() {
        write!(stdout, "{pending}")?;
        stdout.flush()?;
        pending.clear();
    }
    Ok(())
}

/// The CLI's approval callback: allow when `auto_approve`, otherwise explain
/// the denial on stderr.
fn cli_approver(auto_approve: bool) -> Approver {
    Arc::new(move |tool, detail| {
        if !auto_approve {
            eprintln!(
                "permission required for `{tool}` ({detail}); denying (auto_approve is false)"
            );
        }
        Box::pin(async move { auto_approve })
    })
}

/// Builds the initial prompt from positional arguments, expanding `@file`
/// references and returning any image/PDF attachments separately.
/// Parses `--mode`, which selects an output mode like Pi: `json` or `rpc`
/// (plus `print`/`text` for compatibility; `-p` also selects print mode).
fn parse_mode(value: Option<&str>) -> Result<String> {
    match value {
        None => Ok("print".to_string()),
        Some(raw) => {
            let normalized = raw.trim().to_ascii_lowercase();
            match normalized.as_str() {
                "print" | "text" | "json" | "rpc" => Ok(normalized),
                other => anyhow::bail!("unknown --mode `{other}` (expected print, json, or rpc)"),
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

/// The `auto_approve` a run should use. `--ask-approvals` turns prompting on
/// (so auto-approval is off), `--no-ask-approvals` turns it off (so a gated
/// tool runs), and neither keeps the value already in the config.
fn resolve_auto_approve(ask_approvals: bool, no_ask_approvals: bool, stored: bool) -> bool {
    if ask_approvals && !no_ask_approvals {
        false
    } else if no_ask_approvals {
        true
    } else {
        stored
    }
}

/// Everything needed to start one agent run is `runner::AgentRun`.
/// RPC mode: reads JSONL requests from stdin and streams JSONL events to stdout.
/// Each `prompt` request starts a fresh agent run on the session history, and
/// an `approval` request answers a tool that is waiting for the user.
async fn run_rpc_mode(
    mut config: Config,
    cwd: PathBuf,
    session: Option<SessionLog>,
    tool_filter: cli::ToolFilter,
    ask_approvals: bool,
    no_ask_approvals: bool,
) -> Result<()> {
    config.require_api_key()?;
    config.tool_filter = tool_filter;
    // Only a front-end that can answer prompts gets the interactive broker: a
    // caller that does not understand `approval_request` would otherwise hang
    // until the request times out.
    let asking = ask_approvals && !no_ask_approvals;
    // `--ask-approvals` needs auto-approval off for the prompt to be reached;
    // `--no-ask-approvals` runs gated tools instead of inheriting a stored
    // `auto_approve: false` that would deny them; neither keeps the stored one.
    config.auto_approve =
        resolve_auto_approve(ask_approvals, no_ask_approvals, config.auto_approve);
    let ephemeral = config.ephemeral;
    let mut log = session;
    let mut history = match &log {
        Some(log) => log.messages()?,
        None => Vec::new(),
    };

    let approvals = asking.then(oxide_core::approval::ApprovalBroker::new);
    let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::unbounded_channel::<RpcRequest>();
    let (event_tx, event_rx) = unbounded_channel();
    let (control_tx, control_rx) = tokio::sync::mpsc::unbounded_channel();
    let mut out = io::stdout();

    // `run_rpc` answers approvals from its stdin thread while the driver below
    // is busy streaming the turn that is waiting for the answer, so both hold
    // a handle on the same broker.
    let driver_approvals = approvals.clone();
    let driver = tokio::spawn(async move {
        while let Some(request) = prompt_rx.recv().await {
            let RpcRequest::Prompt { text, images } = request else {
                continue;
            };
            if log.is_none() && !ephemeral {
                log = Some(SessionLog::create(&cwd)?);
            }
            if let Some(log) = &log {
                let _ = control_tx.send(cli::session_header(log));
            }
            let resolved = runner::resolve_command(&config, &text);
            let prompt = resolved.text;
            let command_agent = resolved.agent;
            let subtask = resolved.subtask;
            let user = runner::build_user_message(&prompt, &cwd, &images, &[])?;
            if let Some(log) = &log {
                log.append(&user)?;
            }
            history.push(user);
            let (run_tx, mut run_rx) = unbounded_channel();
            // The broker and the run share one steering queue, so a denial's
            // message is drained by the agent it was meant to steer (a fresh
            // queue here would swallow the guidance and let it retry blind).
            let steering = Steering::new();
            let approve = match &driver_approvals {
                Some(broker) => Some(broker.approver(&cwd, run_tx.clone(), steering.clone())),
                None => Some(cli_approver(config.auto_approve)),
            };
            let run = runner::AgentRun {
                config: config.clone(),
                cwd: cwd.clone(),
                history: history.clone(),
                prompt,
                subtask,
                command_agent,
                session: log.clone(),
                approve,
                steering,
                follow_ups: Steering::new(),
                cancel: Cancel::new(),
            };
            runner::spawn_agent(run, run_tx).await;
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

    let result = cli::run_rpc(event_rx, control_rx, prompt_tx, approvals).await;
    let _ = driver.await;
    writeln!(out)?;
    result
}

#[cfg(test)]
mod tests {
    use super::{parse_mode, resolve_auto_approve, Cli, Command, MarketplaceAction, PluginAction};
    use clap::Parser;

    #[test]
    fn mode_parses_output_modes() {
        assert_eq!(parse_mode(None).unwrap(), "print");
        assert_eq!(parse_mode(Some("json")).unwrap(), "json");
        assert_eq!(parse_mode(Some("RPC")).unwrap(), "rpc");
        assert_eq!(parse_mode(Some("print")).unwrap(), "print");
        assert!(parse_mode(Some("plan")).is_err());
        assert!(parse_mode(Some("auto-edit")).is_err());
    }

    #[test]
    fn marketplaces_are_nested_under_plugin() {
        let cli = Cli::try_parse_from([
            "oxide",
            "plugin",
            "marketplace",
            "add",
            "Skyscanner/skyscanner-claude-plugins",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Plugin {
                action: PluginAction::Marketplace {
                    action: MarketplaceAction::Add { .. }
                }
            })
        ));

        let cli = Cli::try_parse_from(["oxide", "plugin", "marketplace", "list"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Plugin {
                action: PluginAction::Marketplace {
                    action: MarketplaceAction::List
                }
            })
        ));
    }

    #[test]
    fn approval_flags_are_opposites() {
        let cli = Cli::try_parse_from(["oxide", "--mode", "rpc", "--ask-approvals"]).unwrap();
        assert!(cli.ask_approvals && !cli.no_ask_approvals);

        let cli = Cli::try_parse_from(["oxide", "--mode", "rpc", "--no-ask-approvals"]).unwrap();
        assert!(!cli.ask_approvals && cli.no_ask_approvals);

        assert!(Cli::try_parse_from(["oxide", "--ask-approvals", "--no-ask-approvals"]).is_err());
    }

    #[test]
    fn approval_flags_decide_auto_approval() {
        // Neither flag leaves the stored setting alone.
        assert!(!resolve_auto_approve(false, false, false));
        assert!(resolve_auto_approve(false, false, true));
        // Asking wins over a stored auto-approval.
        assert!(!resolve_auto_approve(true, false, true));
        // Not asking wins over a stored denial, which is the case the flag was
        // silently losing before.
        assert!(resolve_auto_approve(false, true, false));
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
