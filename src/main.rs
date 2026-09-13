mod agent;
mod auth;
mod compact;
mod config;
mod dcp;
mod ecosystem;
mod llm;
mod lsp;
mod mcp;
mod mcp_config;
mod mcp_oauth;
mod media;
mod memory;
mod permission;
mod plugin;
mod session;
mod snapshots;
mod tools;
mod tui;

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
use std::io::{self, Read, Write};
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

    /// Prompt to run in non-interactive mode
    prompt: Option<String>,

    /// Model to use (overrides config)
    #[arg(short, long)]
    model: Option<String>,

    /// Provider name (overrides config)
    #[arg(long)]
    provider: Option<String>,

    /// Agent to run (from .oxide/agents or .claude/agents)
    #[arg(long)]
    agent: Option<String>,

    /// Print the response and exit instead of launching the TUI
    #[arg(short = 'p', long)]
    print: bool,

    /// Resume the most recent session for this project
    #[arg(short = 'c', long = "continue")]
    continue_session: bool,

    /// Resume a specific session by id
    #[arg(long)]
    resume: Option<String>,

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
    /// Manage provider credentials
    Auth {
        #[command(subcommand)]
        action: AuthAction,
    },
    /// Manage MCP servers
    Mcp {
        #[command(subcommand)]
        action: McpAction,
    },
}

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

#[derive(Subcommand, Debug)]
enum AuthAction {
    /// Store an API key for a provider
    Login {
        /// Provider name (openai, deepseek, anthropic); prompts when omitted
        provider: Option<String>,
        /// API key; prompts when omitted
        #[arg(long)]
        key: Option<String>,
    },
    /// List stored credentials
    List,
    /// Remove stored credentials
    Logout {
        /// Provider name; prompts when omitted
        provider: Option<String>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        return match command {
            Command::Auth { action } => match action {
                AuthAction::Login { provider, key } => auth::login(provider, key),
                AuthAction::List => auth::list(),
                AuthAction::Logout { provider } => auth::logout(provider),
            },
            Command::Mcp { action } => {
                let current_dir = std::env::current_dir().context("resolving current directory")?;
                match action {
                    McpAction::List => mcp_config::list(&current_dir),
                    McpAction::Get { name } => mcp_config::get(&current_dir, &name),
                    McpAction::Add {
                        name,
                        command,
                        transport,
                        env,
                        header,
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
        };
    }
    let cwd = match &cli.cwd {
        Some(path) => path.clone(),
        None => std::env::current_dir().context("resolving current directory")?,
    };
    let config = Config::load(&cwd, cli.model, cli.provider, cli.agent)?;
    let session = if cli.continue_session || cli.resume.is_some() {
        Some(match &cli.resume {
            Some(id) => SessionLog::open_id(&cwd, id)?,
            None => {
                SessionLog::latest(&cwd).context("no previous session found for this project")?
            }
        })
    } else {
        None
    };

    if cli.print || cli.prompt.is_some() {
        let prompt = match cli.prompt {
            Some(prompt) => prompt,
            None => {
                let mut buffer = String::new();
                io::stdin()
                    .read_to_string(&mut buffer)
                    .context("reading prompt from stdin")?;
                buffer
            }
        };
        run_print(config, cwd, prompt, session, cli.image).await
    } else {
        config.require_api_key()?;
        tui::run(config, cwd, session).await
    }
}

async fn run_print(
    config: Config,
    cwd: PathBuf,
    prompt: String,
    session: Option<SessionLog>,
    attachments: Vec<PathBuf>,
) -> Result<()> {
    config.require_api_key()?;
    let resolved = config.resolve_command(&prompt);
    let prompt = resolved
        .as_ref()
        .map(|command| command.prompt.clone())
        .unwrap_or(prompt);
    let command_agent = resolved.as_ref().and_then(|command| command.agent.clone());
    let subtask = resolved.as_ref().is_some_and(|command| command.subtask);
    let log = match session {
        Some(log) => log,
        None => SessionLog::create(&cwd)?,
    };
    let mut history = log.messages()?;
    let user = build_user_message(&prompt, &cwd, &attachments)?;
    log.append(&user)?;
    history.push(user);

    let (tx, mut rx) = unbounded_channel();
    let mcp = Arc::new(McpRegistry::connect(&config.ecosystem.mcp).await);
    let plugins = Arc::new(PluginHost::spawn(&config.ecosystem.plugins, &cwd).await);
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
        session: Some(Arc::new(log)),
        snapshots: Snapshots::open(&cwd).ok().map(Arc::new),
        lsp: Arc::new(LspManager::new()),
        approve,
        steering: crate::agent::Steering::new(),
    };

    if subtask {
        let agent_name = command_agent.unwrap_or_default();
        tokio::spawn(agent::run_subagent(
            config, cwd, history, agent_name, prompt, tx, runtime,
        ));
    } else {
        let mut config = config;
        if let Some(name) = command_agent {
            config.active_agent = config.ecosystem.agent(&name).cloned();
        }
        tokio::spawn(agent::run(config, cwd, history, tx, runtime));
    }

    let mut stdout = io::stdout();
    while let Some(event) = rx.recv().await {
        match event {
            AgentEvent::Text(delta) => {
                print!("{delta}");
                stdout.flush()?;
            }
            AgentEvent::ToolCall { name, args } => {
                eprintln!("\n[tool] {name} {args}");
            }
            AgentEvent::ToolProgress { chunk, .. } => {
                eprintln!("{chunk}");
            }
            AgentEvent::ToolResult { name, output } => {
                eprintln!("[result: {name}] {} bytes", output.len());
            }
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
