mod install;
mod theme;
mod tui;
mod uninstall;
mod update;

// The shared agent core (config, providers, tools, MCP, sessions, snapshots,
// plugins, agent loop) is re-exported at the crate root so existing `crate::`
// paths keep resolving with the same names as before the workspace split.
pub use oxide_core::{
    agent, approval, approvals, at, auth, catalog, cli, clipboard, commands, compact, config, diff,
    ecosystem, html, llm, lsp, mcp, mcp_config, mcp_oauth, media, memory, notice, notify,
    permission, plugin, plugin_registry, portkey_usage, pricing, runner, session, sessions,
    snapshots, tools, trust, update_notice,
};

use agent::{AgentEvent, Approver, Cancel, Steering};
use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use cli::RpcRequest;
use config::{Config, Reasoning};
use serde_json::json;
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

    /// Reasoning effort: auto (default), off, minimal, low, medium, high,
    /// xhigh, or max
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

    /// Let the model put a question to the user through the `ask` tool, which
    /// `--mode rpc` answers with a `question` frame. Without it the tool is not
    /// offered, so a run with nobody to answer never asks
    #[arg(long = "ask-questions")]
    ask_questions: bool,

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

    /// Attach an image, a PDF or a text file to the prompt (repeatable)
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
    /// Update the CLI to the latest release
    Update {
        /// Report the newest release without installing it
        #[arg(long)]
        check: bool,
        /// Install a specific version (a tag like v0.26.0, or a bare 0.26.0)
        #[arg(long, value_name = "VERSION")]
        version: Option<String>,
        /// Install even when the running version is already current
        #[arg(short = 'f', long)]
        force: bool,
        /// Print the check as JSON for a front-end (with --check)
        #[arg(long, requires = "check")]
        json: bool,
        /// Check another component's release train instead of the CLI's, for a
        /// front-end that cannot link oxide-core (with --check)
        #[arg(
            long,
            value_name = "COMPONENT",
            requires = "check",
            value_parser = ["cli", "desktop", "extension"]
        )]
        component: Option<String>,
        /// The version the caller is running, for the component it checks
        /// (with --check)
        #[arg(long, value_name = "VERSION", requires = "check")]
        current: Option<String>,
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
    /// Review the tools a project allows without prompting
    Approvals {
        #[command(subcommand)]
        action: ApprovalsAction,
    },
    /// Forget a provider's stored credentials
    Logout {
        /// Provider name, as `oxide providers` lists it (the active one by
        /// default)
        provider: Option<String>,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Decide whether a project's own resources load
    Trust {
        #[command(subcommand)]
        action: TrustAction,
    },
    /// List models available from connected providers
    Models {
        /// Print provider catalogs as JSON
        #[arg(long)]
        json: bool,
        /// Query only the active provider (for latency-sensitive clients)
        #[arg(long)]
        active: bool,
    },
    /// Print the active model's reasoning levels and current choice
    Reasoning {
        /// Print the listing as JSON
        #[arg(long)]
        json: bool,
        /// Warm the model cache from the provider before answering, so a
        /// front-end that never opens the model picker still learns the
        /// model's own levels
        #[arg(long)]
        refresh: bool,
        /// Read the levels for this model rather than the active one
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
    },
    /// Print the context window a run resolves for the active model
    Context {
        /// Print the window as JSON
        #[arg(long)]
        json: bool,
        /// Read the window for this model rather than the active one
        #[arg(long, value_name = "MODEL")]
        model: Option<String>,
    },
    /// List the providers a client can connect
    Providers {
        /// Print the listing as JSON
        #[arg(long)]
        json: bool,
    },
    /// Connect a provider, storing its credential
    Login {
        /// Provider name, as `oxide providers` lists it
        provider: String,
        /// Read the API key from stdin (one line)
        #[arg(long)]
        key_stdin: bool,
        /// Model to use with this provider
        #[arg(long)]
        model: Option<String>,
        /// Endpoint to use with this provider
        #[arg(long, value_name = "URL")]
        base_url: Option<String>,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
    /// Read the files a run changed, from its shadow snapshots
    Changes {
        #[command(subcommand)]
        action: ChangesAction,
    },
    /// Read the system clipboard as an attachment for a front-end
    Clipboard {
        /// Print the attachment as JSON (`{"name":…,"dataUrl":…}`, or `null`)
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ApprovalsAction {
    /// List the tools this project allows without prompting
    List {
        /// Project directory (defaults to the current one)
        #[arg(long)]
        project: Option<PathBuf>,
        /// Print the listing as JSON
        #[arg(long)]
        json: bool,
    },
    /// Forget every saved rule for this project
    Clear {
        /// Project directory (defaults to the current one)
        #[arg(long)]
        project: Option<PathBuf>,
        /// Print the result as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum TrustAction {
    /// Report the decision this project resolves to
    Show {
        /// Project directory (defaults to the current one)
        #[arg(long)]
        project: Option<PathBuf>,
        /// Print the decision as JSON
        #[arg(long)]
        json: bool,
    },
    /// Save a decision for this project, the way the terminal's `/trust` does
    Set {
        /// `trusted` or `untrusted`
        decision: String,
        /// Project directory (defaults to the current one)
        #[arg(long)]
        project: Option<PathBuf>,
        /// Print the decision as JSON
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand, Debug)]
enum ChangesAction {
    /// Print one file as a run's baseline recorded it
    Show {
        /// Path relative to the project
        path: String,
        /// The revision the run started from, as `agent_end` reported it
        #[arg(long)]
        baseline: String,
        /// Project directory (defaults to the current one)
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Put a project back to the state a run started from
    Undo {
        /// The revision the run started from, as `agent_end` reported it
        #[arg(long)]
        baseline: String,
        /// The revision the run left, as `agent_end` reported it: the work tree
        /// must still hold it, or a change made since would go too
        #[arg(long)]
        after: Option<String>,
        /// Project directory (defaults to the current one)
        #[arg(long)]
        project: Option<PathBuf>,
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
    /// Print one saved session's conversation
    Show {
        /// Session id or path
        id: String,
        /// Only the newest N messages
        #[arg(long)]
        tail: Option<usize>,
        /// Print the session as JSON, for a client that draws it
        #[arg(long)]
        json: bool,
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
            Command::Update {
                check,
                version,
                force,
                json,
                component,
                current,
            } => {
                let component = component
                    .as_deref()
                    .and_then(oxide_core::updates::Component::parse)
                    .unwrap_or(oxide_core::updates::Component::Cli);
                update::run(update::Options {
                    check,
                    version,
                    force,
                    json,
                    component,
                    current,
                })
                .await
            }
            Command::Sessions { action } => {
                let current_dir = std::env::current_dir().context("resolving current directory")?;
                match action {
                    SessionsAction::List { all, older_than } => {
                        sessions::list(&current_dir, all, older_than)
                    }
                    SessionsAction::Show { id, tail, json } => {
                        sessions::show(&current_dir, &id, tail, json)
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
            Command::Approvals { action } => match action {
                ApprovalsAction::List { project, json } => {
                    list_approvals(project_dir(project)?, json)
                }
                ApprovalsAction::Clear { project, json } => {
                    clear_approvals(project_dir(project)?, json)
                }
            },
            Command::Logout { provider, json } => logout(provider, json),
            Command::Trust { action } => match action {
                TrustAction::Show { project, json } => show_trust(project_dir(project)?, json),
                TrustAction::Set {
                    decision,
                    project,
                    json,
                } => set_trust(project_dir(project)?, &decision, json),
            },
            Command::Models { json, active } => {
                let current_dir = std::env::current_dir().context("resolving current directory")?;
                list_models(&current_dir, json, active).await
            }
            Command::Reasoning {
                json,
                refresh,
                model,
            } => {
                let current_dir = std::env::current_dir().context("resolving current directory")?;
                list_reasoning(&current_dir, json, refresh, model).await
            }
            Command::Context { json, model } => {
                let current_dir = std::env::current_dir().context("resolving current directory")?;
                show_context(&current_dir, json, model)
            }
            Command::Providers { json } => list_providers(json),
            Command::Clipboard { json } => read_clipboard(json),
            Command::Login {
                provider,
                key_stdin,
                model,
                base_url,
                json,
            } => connect_provider(&provider, key_stdin, model, base_url, json),
            Command::Changes { action } => match action {
                ChangesAction::Show {
                    path,
                    baseline,
                    project,
                } => {
                    let cwd = match project {
                        Some(path) => path,
                        None => std::env::current_dir().context("resolving current directory")?,
                    };
                    let snapshots = snapshots::Snapshots::open(&cwd)?;
                    let content = snapshots.content_at(&baseline, &path)?;
                    io::stdout().write_all(&content)?;
                    Ok(())
                }
                ChangesAction::Undo {
                    baseline,
                    after,
                    project,
                } => {
                    let cwd = match project {
                        Some(path) => path,
                        None => std::env::current_dir().context("resolving current directory")?,
                    };
                    let snapshots = snapshots::Snapshots::open(&cwd)?;
                    snapshots.restore_turn(&baseline, after.as_deref())?;
                    // Said on stdout, since it is the command's result rather
                    // than progress: a front-end that only wanted the restore
                    // done reads the exit code and says its own piece.
                    println!("restored {} to {}", cwd.display(), baseline);
                    Ok(())
                }
            },
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
    // A run that names a level explicitly keeps it; a resumed session restores
    // the one it was last left at instead of reverting to the stored default.
    let reasoning_explicit = cli.reasoning.is_some()
        || std::env::var("OXIDE_REASONING")
            .map(|value| !value.trim().is_empty())
            .unwrap_or(false);
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
    if !reasoning_explicit {
        if let Some(level) = session.as_ref().and_then(SessionLog::thinking_level) {
            if let Some(parsed) = Reasoning::parse(&level) {
                config.reasoning = parsed;
            }
        }
    }

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
            cli.ask_questions,
        )
        .await;
    }

    // Only rpc mode and the interactive TUI can carry an approval answer; every
    // other mode has no channel, so the flag would silently run the tool it was
    // meant to gate.
    if cli.ask_approvals || cli.no_ask_approvals {
        if explicit_prompt || mode != "print" {
            anyhow::bail!(
                "--ask-approvals/--no-ask-approvals need the interactive TUI or --mode rpc, where the question can be answered"
            );
        }
        config.auto_approve =
            resolve_auto_approve(cli.ask_approvals, cli.no_ask_approvals, config.auto_approve);
    }
    // Questions are answered by a client on the rpc channel, and by nobody else:
    // the terminal has no dialog for one, so the tool that would ask is not
    // offered there.
    if cli.ask_questions {
        anyhow::bail!(
            "--ask-questions needs --mode rpc, where the answer travels on the same channel"
        );
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

/// The project directory a subcommand acts on: the one it was given, or the
/// process's own working directory, which is what a front-end that runs the CLI
/// in a folder expects.
fn project_dir(project: Option<PathBuf>) -> Result<PathBuf> {
    match project {
        Some(path) => Ok(path),
        None => std::env::current_dir().context("resolving current directory"),
    }
}

/// The tools a project allows without prompting, and forgetting them again.
/// The rules live in the shared `approvals.json`, so a front-end that cannot
/// link `oxide-core` — the VS Code panel — reads and edits them here rather
/// than keeping a copy of the store.
fn list_approvals(project: PathBuf, json_output: bool) -> Result<()> {
    let mut store = oxide_core::approvals::ApprovalStore::load();
    let tools = store.list(&project);
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "project": project.display().to_string(),
                "tools": tools,
            }))?
        );
        return Ok(());
    }
    if tools.is_empty() {
        println!("{}: no tools allowed without prompting", project.display());
        return Ok(());
    }
    println!("{}:", project.display());
    for tool in tools {
        println!("  {tool}");
    }
    Ok(())
}

fn clear_approvals(project: PathBuf, json_output: bool) -> Result<()> {
    let mut store = oxide_core::approvals::ApprovalStore::load();
    let tools = store.list(&project);
    store.clear(&project)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "project": project.display().to_string(),
                "cleared": tools.len(),
            }))?
        );
        return Ok(());
    }
    println!(
        "forgot {} allowed tool{} for {}",
        tools.len(),
        if tools.len() == 1 { "" } else { "s" },
        project.display()
    );
    Ok(())
}

/// Forgets a provider's stored credential. Removing the provider in use switches
/// to another logged-in one the way the terminal's `/logout` does, so a session
/// is never left pointing at a provider with nothing to sign with.
fn logout(provider: Option<String>, json_output: bool) -> Result<()> {
    let config = Config::load(&std::env::current_dir()?, None, None, None, None)?;
    let requested = match provider {
        Some(name) => auth::canonical_provider(&name),
        None => auth::canonical_provider(&config.provider),
    };
    if requested.is_empty() {
        anyhow::bail!("no provider connected — run /connect to add one");
    }
    let mut store = auth::AuthStore::load()?;
    if !store.remove(&requested) {
        if json_output {
            println!(
                "{}",
                serde_json::to_string(&json!({
                    "provider": requested,
                    "removed": false,
                    "message": format!("no stored credentials for {requested}"),
                }))?
            );
            return Ok(());
        }
        anyhow::bail!("no stored credentials for {requested}");
    }
    store.save()?;
    let was_active = auth::canonical_provider(&config.provider) == requested;
    let next = if was_active {
        // The next stored provider the way the terminal picks it: the first the
        // store lists. Nothing left leaves the selection alone, and the next run
        // says what is missing.
        match auth::stored_providers().into_iter().next() {
            Some(next) => auth::select_stored(&next).ok().map(|(name, _)| name),
            None => None,
        }
    } else {
        None
    };
    let label = auth::provider_label(&requested);
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "provider": requested,
                "label": label,
                "removed": true,
                "active": was_active,
                "switchedTo": next,
            }))?
        );
        return Ok(());
    }
    match (&next, was_active) {
        (Some(name), _) => println!(
            "logged out of {label} — switched to {}",
            auth::provider_label(name)
        ),
        (None, true) => println!("logged out of {label} — run /connect to reconnect"),
        (None, false) => println!("logged out of {label}"),
    }
    Ok(())
}

/// Reports and saves a project's own trust decision, the same `trust.json` the
/// terminal's `/trust` and the desktop app write, so a decision made in one
/// front-end is the one the others read.
fn show_trust(project: PathBuf, json_output: bool) -> Result<()> {
    let decision = oxide_core::trust::project_decision(&project);
    let trusted = decision.is_trusted();
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "project": project.display().to_string(),
                "trusted": trusted,
                "requiresTrust": oxide_core::trust::requires_trust(&project),
            }))?
        );
        return Ok(());
    }
    println!(
        "{}: {}",
        project.display(),
        if trusted { "trusted" } else { "untrusted" }
    );
    Ok(())
}

/// The decision a `trust set` argument names. The words a reader may reach for
/// are accepted, and anything else is refused with the two that are canonical
/// rather than written down as a decision nobody made.
fn parse_trust_decision(value: &str) -> Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "trusted" | "trust" | "always" | "yes" => Ok(true),
        "untrusted" | "never" | "no" => Ok(false),
        other => anyhow::bail!("unknown decision `{other}` — use `trusted` or `untrusted`"),
    }
}

fn set_trust(project: PathBuf, decision: &str, json_output: bool) -> Result<()> {
    let trusted = parse_trust_decision(decision)?;
    let mut store = oxide_core::trust::TrustStore::load()?;
    store.set(&project, trusted);
    store.save()?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "project": project.display().to_string(),
                "trusted": trusted,
            }))?
        );
        return Ok(());
    }
    println!(
        "{}: {}",
        project.display(),
        if trusted { "trusted" } else { "untrusted" }
    );
    Ok(())
}

/// Prints the providers a picker draws, from the single provider table in the
/// core, with the state a row shows beside it. A front-end that cannot link
/// `oxide-core` — the VS Code panel drives this binary — reads the listing here
/// instead of keeping its own copy of the table.
fn list_providers(json_output: bool) -> Result<()> {
    let views = auth::provider_views();
    if json_output {
        let active = views.iter().find(|view| view.active).map(|view| view.name);
        println!(
            "{}",
            serde_json::to_string(&json!({ "active": active, "providers": views }))?
        );
        return Ok(());
    }
    for view in &views {
        let mut marks = Vec::new();
        if view.active {
            marks.push("in use");
        }
        if view.stored {
            marks.push("stored");
        }
        if view.local {
            marks.push("no key needed");
        } else if view.credential == auth::CredentialMode::External {
            // A provider that signs with the machine's own identity is as usable
            // without a key as a server on this machine, and says so here rather
            // than reading as one that is waiting for a paste.
            marks.push("machine credential");
        }
        let mark = if marks.is_empty() {
            String::new()
        } else {
            format!("  [{}]", marks.join(", "))
        };
        println!(
            "{}\t{} — {}{}",
            view.name, view.label, view.description, mark
        );
    }
    Ok(())
}

/// Reads the system clipboard as an attachment for a front-end that draws its
/// own composer but cannot link `oxide-core` — the VS Code panel. A webview's
/// own paste reads a copied file's bytes directly, which macOS refuses for the
/// Desktop, Documents and Downloads folders; the read behind this command goes
/// through the same `media::clipboard` the terminal's Ctrl+V uses, so the
/// pasteboard's own picture is attached rather than the paste failing. Nothing
/// is printed on stdout for a text paste: a text file rides the normal paste
/// and an empty clipboard has nothing to attach.
fn read_clipboard(json_output: bool) -> Result<()> {
    match oxide_core::media::clipboard_media() {
        Some((name, data_url)) => {
            if json_output {
                println!(
                    "{}",
                    serde_json::to_string(&json!({ "name": name, "dataUrl": data_url }))?
                );
            } else {
                println!("{name}");
            }
        }
        None => {
            if json_output {
                println!("null");
            } else {
                println!("the clipboard holds no image or PDF to attach");
            }
        }
    }
    Ok(())
}

/// Connects a provider from a front-end that has no terminal dialog to run: the
/// credential lands in the same `auth.json` the terminal writes, so a login here
/// is a login there. The key is read from stdin rather than an argument, which
/// would be visible in the process listing and kept in the shell's history.
fn connect_provider(
    provider: &str,
    key_stdin: bool,
    model: Option<String>,
    base_url: Option<String>,
    json_output: bool,
) -> Result<()> {
    let key = if key_stdin {
        let mut line = String::new();
        io::stdin()
            .read_line(&mut line)
            .context("reading the API key from stdin")?;
        line.trim().to_string()
    } else {
        String::new()
    };
    let cwd = std::env::current_dir().context("resolving current directory")?;
    let outcome =
        auth::login_provider(provider, &key, model.as_deref(), base_url.as_deref(), &cwd)?;
    if json_output {
        println!("{}", serde_json::to_string(&outcome)?);
    } else {
        println!(
            "Connected {} ({}) — model {}",
            outcome.label, outcome.provider, outcome.model
        );
    }
    Ok(())
}

/// Prints the same provider catalogs the TUI's `/models` and desktop model
/// picker use. Front-ends call this rather than reimplementing authentication,
/// provider endpoints, bundled fallbacks or catalog normalization.
async fn list_models(current_dir: &Path, json_output: bool, active_only: bool) -> Result<()> {
    let config = Config::load(current_dir, None, None, None, None)?;
    let active = auth::canonical_provider(&config.provider);
    let mut providers = config::provider_configs(&config);
    if active_only {
        providers.retain(|(name, _)| auth::canonical_provider(name) == active);
    }
    if providers.is_empty() {
        anyhow::bail!("no provider connected — run /connect to add an API key");
    }
    let fetched = futures::future::join_all(providers.into_iter().map(
        |(name, provider_config)| async move {
            let current = provider_config.model.clone();
            let result = llm::LlmClient::new(provider_config).list_models().await;
            (name, current, result)
        },
    ))
    .await;
    let catalogs = fetched
        .into_iter()
        .map(|(provider, current, result)| match result {
            Ok(models) => json!({
                "active": provider == active,
                "provider": provider,
                "current": current,
                "models": models,
            }),
            Err(error) => json!({
                "active": provider == active,
                "provider": provider,
                "current": current,
                "models": [],
                "error": format!("{error:#}"),
            }),
        })
        .collect::<Vec<_>>();
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "active": active,
                "current": config.model,
                "providers": catalogs,
            }))?
        );
    } else {
        for catalog in catalogs {
            let provider = catalog["provider"].as_str().unwrap_or_default();
            if let Some(error) = catalog.get("error").and_then(|value| value.as_str()) {
                println!("{provider}: {error}");
                continue;
            }
            let marker = if catalog["active"].as_bool().unwrap_or(false) {
                " (active)"
            } else {
                ""
            };
            println!("{provider}{marker}");
            for model in catalog["models"].as_array().into_iter().flatten() {
                if let Some(model) = model.as_str() {
                    println!("  {model}");
                }
            }
        }
    }
    Ok(())
}

/// Prints the active model's reasoning levels and current choice, so a front-end
/// that cannot link oxide-core (the VS Code panel) narrows its picker the way
/// the terminal and desktop do. The levels come from the model cache; with
/// `--refresh` a cold cache is warmed from the provider's listing first.
async fn list_reasoning(
    current_dir: &Path,
    json_output: bool,
    refresh: bool,
    model: Option<String>,
) -> Result<()> {
    let mut config = Config::load(current_dir, model, None, None, None)?;
    if refresh && config.reasoning_supported.is_none() {
        // Bypasses the TTL cache: a fresh entry the provider answered without
        // effort metadata must not keep the model's own levels hidden.
        let _ = llm::LlmClient::new(config.clone()).refresh_models().await;
        config.reasoning_supported = llm::cached_model_reasoning(&config, &config.model);
    }
    // Only a model whose listing actually advertised levels narrows a front-end;
    // the name heuristic's guess is not the model's own word, so it is reported
    // as `null` and the caller keeps its built-in set.
    let advertised = config
        .reasoning_supported
        .as_ref()
        .is_some_and(|meta| !meta.supported.is_empty());
    let levels: Option<Vec<&str>> = advertised.then(|| {
        config
            .reasoning_levels()
            .iter()
            .map(|level| level.label())
            .collect()
    });
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&json!({
                "current": config.reasoning.label(),
                "supportsReasoning": config.supports_reasoning(),
                "reasoningLevels": levels,
            }))?
        );
    } else if let Some(levels) = levels {
        println!(
            "thinking: {} (levels: {})",
            config.reasoning.label(),
            levels.join(", ")
        );
    } else {
        println!("thinking: {}", config.reasoning.label());
    }
    Ok(())
}

/// Prints the context window a run resolves for the active model, so a front-end
/// that cannot link oxide-core (the VS Code panel) reports the same window the
/// run measures against rather than keeping a table of its own: the
/// `OXIDE_CONTEXT_LIMIT` override, the configured window, a
/// `modelContextWindows` entry, the provider's published catalog and the
/// built-in table, in the order [`Config::context_window`] applies them.
///
/// It answers the credential and plan facts in the same breath, since a
/// front-end that has to resolve a model's window has already asked about this
/// project: whether the run would find a key at all (`hasKey`, the same
/// question [`Config::require_api_key`] settles before a turn starts, so a
/// panel can say so before the reader sends rather than after), the variables a
/// key could come from, and whether the credential is a plan rather than a
/// metered key (`subscription`, which is what the spend line marks ` (sub)`).
fn show_context(current_dir: &Path, json_output: bool, model: Option<String>) -> Result<()> {
    let config = Config::load(current_dir, model, None, None, None)?;
    if json_output {
        println!(
            "{}",
            serde_json::to_string(&context_view(&config, &|name| std::env::var(name).ok()))?
        );
    } else {
        println!(
            "context: {} (model: {})",
            config.context_window(),
            config.model
        );
    }
    Ok(())
}

/// The facts a run resolves for this project, as the JSON a front-end reads: the
/// window the model's requests are measured against, and the credential and plan
/// behind them. The environment is injected so the rules are testable without
/// this machine's own variables deciding the answer — the same reason
/// [`Config::require_api_key_with`] takes one.
fn context_view(config: &Config, env: &dyn Fn(&str) -> Option<String>) -> serde_json::Value {
    json!({
        "model": config.model,
        "window": config.context_window(),
        "provider": config.provider,
        // The run's own question rather than a reading of the key field: a server
        // on this machine and a provider that signs with the machine's identity
        // are as usable without one, and a key in `config.json` counts where the
        // auth store does not.
        "hasKey": config.require_api_key_with(env).is_ok(),
        "subscription": config.is_subscription(),
        "keyEnv": config
            .preset()
            .map(|preset| preset.key_envs().collect::<Vec<_>>())
            .unwrap_or_default(),
    })
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
        // A print run has no dialog to answer a question in, so the model is not
        // offered the tool that would ask one.
        ask: None,
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
                let subject = tools::search_tool_summary(&name, &args).unwrap_or(args);
                eprintln!("\n[tool] {name} {subject}");
            }
            AgentEvent::ToolProgress { chunk, .. } => {
                eprintln!("{chunk}");
            }
            AgentEvent::ToolResult { name, output, .. } => {
                if tools::is_empty_search_result(&name, &output) {
                    eprintln!("[result: {name}] 0 matches");
                } else {
                    eprintln!("[result: {name}] {} bytes", output.len());
                }
            }
            AgentEvent::Usage { .. } => {}
            // Print mode has no way to ask: it always runs a non-interactive
            // approver, so a request here can only be a no-op.
            AgentEvent::ApprovalRequest { .. } => {}
            // Print mode is given no asker, so `ask` is not offered to the
            // model and a question cannot arrive.
            AgentEvent::QuestionRequest { .. } | AgentEvent::QuestionClosed { .. } => {}
            // The persisted name is consumed by interactive clients and session
            // listings; print mode has no title chrome to update.
            AgentEvent::SessionTitle { .. } => {}
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
            // Print mode has no transcript to paint the delivered turn in.
            AgentEvent::Steered { .. } => {}
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
/// references and returning any media attachments separately.
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
/// an `approval` request answers a tool that is waiting for the user, a
/// `question` request a question the model asked.
async fn run_rpc_mode(
    mut config: Config,
    cwd: PathBuf,
    session: Option<SessionLog>,
    tool_filter: cli::ToolFilter,
    ask_approvals: bool,
    no_ask_approvals: bool,
    ask_questions: bool,
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
    // `--ask-questions` says the client understands `question_request` and will
    // answer it; without it the `ask` tool is not offered at all.
    let questions = ask_questions.then(oxide_core::ask::AskBroker::new);
    let (prompt_tx, mut prompt_rx) = tokio::sync::mpsc::unbounded_channel::<RpcRequest>();
    // One channel carries everything written to the client — the run's own
    // events, the session header and a finished turn's change listing — so the
    // frames leave in the order they were queued and a client never has to
    // guess which turn a listing belongs to.
    let (frame_tx, frame_rx) = tokio::sync::mpsc::unbounded_channel::<serde_json::Value>();
    let mut out = io::stdout();

    // `run_rpc` answers approvals from its stdin thread while the driver below
    // is busy streaming the turn that is waiting for the answer, so both hold
    // a handle on the same broker.
    let driver_approvals = approvals.clone();
    let driver_questions = questions.clone();
    let driver = tokio::spawn(async move {
        let mut pending_prompts = std::collections::VecDeque::new();
        loop {
            let request = match pending_prompts.pop_front() {
                Some(request) => request,
                None => match prompt_rx.recv().await {
                    Some(request) => request,
                    None => break,
                },
            };
            let RpcRequest::Prompt { text, images } = request else {
                continue;
            };
            if log.is_none() && !ephemeral {
                log = Some(SessionLog::create(&cwd)?);
            }
            if let Some(log) = &log {
                let _ = frame_tx.send(cli::session_header(log));
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
            // The state the run starts from, so the client can list the files it
            // changes and draw a diff against what is on disk now.
            let baseline = mark_baseline(&cwd).await;
            // The broker and the run share one steering queue, so a denial's
            // message is drained by the agent it was meant to steer (a fresh
            // queue here would swallow the guidance and let it retry blind).
            let steering = Steering::new();
            let follow_ups = Steering::new();
            let approve = match &driver_approvals {
                Some(broker) => Some(broker.approver(&cwd, run_tx.clone(), steering.clone())),
                None => Some(cli_approver(config.auto_approve)),
            };
            let ask = driver_questions
                .as_ref()
                .map(|broker| broker.asker(run_tx.clone()));
            let run = runner::AgentRun {
                config: config.clone(),
                cwd: cwd.clone(),
                history: history.clone(),
                prompt,
                subtask,
                command_agent,
                session: log.clone(),
                approve,
                ask,
                steering: steering.clone(),
                follow_ups: follow_ups.clone(),
                cancel: Cancel::new(),
            };
            runner::spawn_agent(run, run_tx).await;
            let mut input_open = true;
            loop {
                tokio::select! {
                    biased;
                    request = prompt_rx.recv(), if input_open => {
                        match request {
                            Some(RpcRequest::Steer { id, text, images, follow_up }) => {
                                match runner::build_user_message(&text, &cwd, &images, &[]) {
                                    Ok(message) => {
                                        let accepted = if follow_up {
                                            follow_ups.push(message)
                                        } else {
                                            steering.push(message)
                                        };
                                        let _ = frame_tx.send(serde_json::json!({
                                            "type": "steer_ack",
                                            "id": id,
                                            "accepted": accepted,
                                        }));
                                    }
                                    Err(error) => {
                                        let _ = frame_tx.send(serde_json::json!({
                                            "type": "steer_ack",
                                            "id": id,
                                            "accepted": false,
                                            "error": format!("{error:#}"),
                                        }));
                                    }
                                }
                            }
                            // Prompts remain ordered as future turns while the
                            // input channel stays live for steering the active one.
                            Some(request @ RpcRequest::Prompt { .. }) => {
                                pending_prompts.push_back(request);
                            }
                            Some(_) => {}
                            None => input_open = false,
                        }
                    }
                    event = run_rx.recv() => {
                        let Some(event) = event else { break };
                        let finished = matches!(event, AgentEvent::Finished(_));
                        if let AgentEvent::Finished(messages) = &event {
                            history = messages.clone();
                        }
                        if let Some(frame) = cli::event_json(&event) {
                            if frame_tx.send(frame).is_err() {
                                break;
                            }
                        }
                        if finished {
                            // Queued behind the turn's own last event, so the listing
                            // reaches the client after the run it belongs to.
                            if let Some(frame) = turn_changes(&cwd, &baseline).await {
                                let _ = frame_tx.send(frame);
                            }
                            break;
                        }
                    }
                }
            }
        }
        Ok::<(), anyhow::Error>(())
    });

    let result = cli::run_rpc(frame_rx, prompt_tx, approvals, questions).await;
    let _ = driver.await;
    writeln!(out)?;
    result
}

/// The state a run starts from, recorded in the project's shadow snapshot repo
/// (see [`snapshots::Snapshots::baseline`]). `None` when it cannot be taken,
/// which is a turn without a change listing rather than a failed turn.
async fn mark_baseline(cwd: &Path) -> Option<(snapshots::Snapshots, String)> {
    let cwd = cwd.to_path_buf();
    tokio::task::spawn_blocking(move || snapshots::Snapshots::baseline(&cwd))
        .await
        .ok()
        .flatten()
}

/// The files a finished turn changed, as the control frame a client draws them
/// from: the project the run was in — which its paths are relative to and its
/// baseline is read out of — the baseline it diffs a file against, the listing,
/// and the `+`/`-` totals a header shows. `None` for a run that has no baseline,
/// and for one that changed nothing — a turn that only read files says nothing.
async fn turn_changes(
    project: &Path,
    baseline: &Option<(snapshots::Snapshots, String)>,
) -> Option<serde_json::Value> {
    let (snapshots, base) = baseline.clone()?;
    let marked = base.clone();
    let listed = snapshots.clone();
    let changes = tokio::task::spawn_blocking(move || listed.changes_since(&marked))
        .await
        .ok()?
        .ok()?;
    if changes.is_empty() {
        return None;
    }
    // The state the turn left behind, which the client's own Undo checks the
    // work tree still holds before it puts anything back (see
    // `Snapshots::restore_turn`). Read after the listing, so both describe the
    // same state.
    let after = tokio::task::spawn_blocking(move || snapshots.mark_named("turn").ok())
        .await
        .ok()
        .flatten();
    Some(json!({
        "type": "turn_changes",
        // The folder the run started in, so a client that has moved on since —
        // another root in a multi-root window, an editor opened elsewhere —
        // opens a row against the file and the snapshot the listing names.
        "project": project,
        "baseline": base,
        "after": after,
        "changes": changes,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        context_view, parse_mode, parse_trust_decision, resolve_auto_approve, turn_changes,
        ApprovalsAction, Cli, Command, MarketplaceAction, PluginAction, TrustAction,
    };
    use oxide_core::config::Config;

    /// What `context --json` answers, which a front-end that cannot link
    /// `oxide-core` reads for three things it cannot resolve itself: the window a
    /// run measures against, whether the run would find a credential at all (said
    /// before a message is sent rather than after it fails), and whether that
    /// credential is a plan the spend line should mark ` (sub)`. The environment
    /// is injected, so this machine's own variables decide nothing here.
    #[test]
    fn context_view_carries_the_window_and_the_credential_facts() {
        let keyed = Config {
            provider: "zai".into(),
            model: "glm-5".into(),
            api_key: "sk-test".into(),
            ..Config::default()
        };
        let with_key = context_view(&keyed, &|_| None);
        assert_eq!(with_key["provider"], "zai");
        assert_eq!(with_key["model"], "glm-5");
        assert!(with_key["window"].as_u64().unwrap_or(0) > 0);
        assert_eq!(with_key["hasKey"], true);
        assert_eq!(with_key["subscription"], false);
        assert_eq!(
            with_key["keyEnv"],
            serde_json::json!(["ZAI_API_KEY", "ZHIPU_API_KEY", "GLM_API_KEY"]),
            "every variable a key could come from, in the table's own order"
        );

        // Nothing in the config: the run would refuse, and the answer says so
        // where the panel can read it before a message is sent. A key that came
        // from the environment is already in `api_key` — `Config::load` resolves
        // one before either store — so what this instance is about is the
        // credential the config does *not* hold.
        let bare = Config {
            provider: "zai".into(),
            api_key: String::new(),
            ..Config::default()
        };
        assert_eq!(context_view(&bare, &|_| None)["hasKey"], false);
        // A provider that signs with the machine's own identity is as usable
        // without one, which is the other half of the question: Bedrock's AWS
        // credentials are read where they live (the environment, else
        // `~/.aws/credentials`) rather than stored here.
        let bedrock = Config {
            provider: "bedrock".into(),
            api_key: String::new(),
            ..Config::default()
        };
        assert_eq!(context_view(&bedrock, &|_| None)["hasKey"], false);
        let signed = context_view(&bedrock, &|name| match name {
            "AWS_ACCESS_KEY_ID" => Some("AKIATEST".to_string()),
            "AWS_SECRET_ACCESS_KEY" => Some("secret".to_string()),
            _ => None,
        });
        assert_eq!(signed["hasKey"], true);

        // A plan rather than a metered key: the price table's number is what it
        // would have billed, which the panel marks ` (sub)`.
        let plan = Config {
            provider: "github-copilot".into(),
            api_key: "ghu_test".into(),
            ..Config::default()
        };
        assert_eq!(context_view(&plan, &|_| None)["subscription"], true);

        // A server on this machine needs no credential at all, so an empty key
        // is not a missing one.
        let local = Config {
            provider: "ollama".into(),
            api_key: String::new(),
            ..Config::default()
        };
        assert_eq!(context_view(&local, &|_| None)["hasKey"], true);
    }
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

    /// The listing a client draws a change card from carries the state the turn
    /// left (`after`), which its own Undo checks the work tree still holds: a
    /// client that could not undo the newest turn would offer the button and be
    /// refused every time.
    #[tokio::test]
    async fn a_turn_listing_carries_the_state_the_turn_left() {
        use oxide_core::snapshots::Snapshots;

        let root = std::env::temp_dir().join(format!("oxide_cli_turn_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        let snapshots = Snapshots::at(root.join("shadow"), work.clone()).unwrap();
        let base = snapshots.mark().unwrap();

        // A turn that wrote one file, and so has a card.
        std::fs::write(work.join("a.txt"), "two\n").unwrap();
        let frame = turn_changes(&work, &Some((snapshots.clone(), base.clone())))
            .await
            .unwrap();
        assert_eq!(frame["baseline"], serde_json::json!(base));
        assert_eq!(frame["project"], serde_json::json!(work));
        assert_eq!(frame["changes"]["files"][0]["path"], "a.txt");

        // The state it left is the work tree as it stands, so undoing this
        // turn is allowed and a second change afterwards is not.
        let after = frame["after"].as_str().unwrap().to_string();
        assert!(snapshots.unchanged_since(&after).unwrap());
        std::fs::write(work.join("a.txt"), "three\n").unwrap();
        assert!(!snapshots.unchanged_since(&after).unwrap());

        // A turn that changed nothing says nothing (no card to carry one).
        let settled = snapshots.mark_named("turn").unwrap();
        assert!(turn_changes(&work, &Some((snapshots, settled)))
            .await
            .is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn marketplaces_are_nested_under_plugin() {
        let cli =
            Cli::try_parse_from(["oxide", "plugin", "marketplace", "add", "acme/team-plugins"])
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
    fn parses_update_options() {
        let cli = Cli::try_parse_from(["oxide", "update", "--check"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Update {
                check: true,
                version: None,
                force: false,
                json: false,
                component: None,
                current: None,
            })
        ));

        let cli =
            Cli::try_parse_from(["oxide", "update", "--version", "0.25.0", "--force"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Update {
                check: false,
                version: Some(version),
                force: true,
                json: false,
                ..
            }) if version == "0.25.0"
        ));

        let cli = Cli::try_parse_from(["oxide", "update", "--check", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Update {
                check: true,
                json: true,
                ..
            })
        ));
        // A front-end that cannot link the core checks its own release train and
        // reports the version it is running.
        let cli = Cli::try_parse_from([
            "oxide",
            "update",
            "--check",
            "--json",
            "--component",
            "desktop",
            "--current",
            "0.33.0",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Update {
                component: Some(component),
                current: Some(current),
                ..
            }) if component == "desktop" && current == "0.33.0"
        ));
        // A component this command does not know, and one it cannot install
        // without a check, are both refused.
        assert!(
            Cli::try_parse_from(["oxide", "update", "--check", "--component", "cli-extra"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["oxide", "update", "--component", "desktop"]).is_err());
        // The JSON describes a check, so it is not printed for a run that would
        // install the release instead.
        assert!(Cli::try_parse_from(["oxide", "update", "--json"]).is_err());
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

    #[test]
    fn parses_model_catalog_output() {
        let cli = Cli::try_parse_from(["oxide", "models", "--json", "--active"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Models {
                json: true,
                active: true
            })
        ));
    }

    /// The three commands that let a front-end which cannot link `oxide-core`
    /// take back what it handed out: the rules an `Always allow` saved, a
    /// provider's credential, and a project's trust decision.
    #[test]
    fn parses_the_commands_a_front_end_takes_back_with() {
        let cli = Cli::try_parse_from(["oxide", "approvals", "list", "--json"]).unwrap();
        match cli.command {
            Some(Command::Approvals {
                action:
                    ApprovalsAction::List {
                        json: true,
                        project,
                    },
            }) => assert!(project.is_none()),
            other => panic!("unexpected {other:?}"),
        }

        let cli = Cli::try_parse_from(["oxide", "approvals", "clear", "--project", "/work/oxide"])
            .unwrap();
        match cli.command {
            Some(Command::Approvals {
                action: ApprovalsAction::Clear { project, .. },
            }) => assert_eq!(project.unwrap().to_str(), Some("/work/oxide")),
            other => panic!("unexpected {other:?}"),
        }

        // A logout names a provider, or leaves it to the one in use.
        let cli = Cli::try_parse_from(["oxide", "logout", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Logout {
                provider: None,
                json: true
            })
        ));

        let cli = Cli::try_parse_from(["oxide", "trust", "set", "untrusted", "--json"]).unwrap();
        match cli.command {
            Some(Command::Trust {
                action:
                    TrustAction::Set {
                        decision,
                        json: true,
                        ..
                    },
            }) => assert_eq!(decision, "untrusted"),
            other => panic!("unexpected {other:?}"),
        }

        // A decision is written in the words the store holds, and anything else
        // is refused rather than guessed at.
        assert!(parse_trust_decision("trusted").unwrap());
        assert!(parse_trust_decision(" Always ").unwrap());
        assert!(!parse_trust_decision("untrusted").unwrap());
        assert!(!parse_trust_decision("no").unwrap());
        assert!(parse_trust_decision("maybe").is_err());
    }

    #[test]
    fn parses_a_provider_listing_and_a_login() {
        let cli = Cli::try_parse_from(["oxide", "providers", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Providers { json: true })
        ));

        let cli = Cli::try_parse_from([
            "oxide",
            "login",
            "openai",
            "--key-stdin",
            "--model",
            "gpt-5.1",
            "--base-url",
            "https://proxy.example/v1",
            "--json",
        ])
        .unwrap();
        match cli.command {
            Some(Command::Login {
                provider,
                key_stdin: true,
                model,
                base_url,
                json: true,
            }) => {
                assert_eq!(provider, "openai");
                assert_eq!(model.as_deref(), Some("gpt-5.1"));
                assert_eq!(base_url.as_deref(), Some("https://proxy.example/v1"));
            }
            other => panic!("unexpected command: {other:?}"),
        }

        // A login without a key is how a stored or local provider is chosen.
        let cli = Cli::try_parse_from(["oxide", "login", "ollama"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Login {
                key_stdin: false,
                model: None,
                base_url: None,
                json: false,
                ..
            })
        ));
        // The provider is required, so an empty login fails rather than
        // silently connecting whatever was last used.
        assert!(Cli::try_parse_from(["oxide", "login"]).is_err());
    }

    #[test]
    fn parses_a_reasoning_listing() {
        let cli = Cli::try_parse_from(["oxide", "reasoning", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Reasoning {
                json: true,
                refresh: false,
                model: None,
            })
        ));

        let cli = Cli::try_parse_from([
            "oxide",
            "reasoning",
            "--json",
            "--refresh",
            "--model",
            "glm-5",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Reasoning {
                json: true,
                refresh: true,
                model: Some(model),
            }) if model == "glm-5"
        ));
    }

    #[test]
    fn parses_a_context_listing() {
        let cli = Cli::try_parse_from(["oxide", "context", "--json"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Context {
                json: true,
                model: None,
            })
        ));

        let cli = Cli::try_parse_from(["oxide", "context", "--json", "--model", "glm-5"]).unwrap();
        assert!(matches!(
            cli.command,
            Some(Command::Context {
                json: true,
                model: Some(model),
            }) if model == "glm-5"
        ));
    }
}
