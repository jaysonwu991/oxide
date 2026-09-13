pub mod app;
pub mod ui;

use crate::agent::{self, AgentEvent, ApprovalRequest, Approver, Runtime};
use crate::config::Config;
use crate::ecosystem::AgentMode;
use crate::llm::{LlmClient, Message};
use crate::lsp::LspManager;
use crate::mcp::McpRegistry;
use crate::media;
use crate::plugin::PluginHost;
use crate::session::SessionLog;
use crate::snapshots::Snapshots;
use crate::tui::app::{App, ChatItem, CommandHint, ConnectState, ConnectStep, ModelsState};
use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, EnableBracketedPaste, Event, EventStream, KeyCode, KeyEvent,
    KeyModifiers,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

pub async fn run(config: Config, cwd: PathBuf, session: Option<SessionLog>) -> Result<()> {
    let mcp = Arc::new(McpRegistry::connect(&config.ecosystem.mcp).await);
    let plugins = Arc::new(PluginHost::spawn(&config.ecosystem.plugins, &cwd).await);
    let snapshots = Snapshots::open(&cwd).ok().map(Arc::new);
    let lsp = Arc::new(LspManager::new());

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;

    let result = event_loop(
        &mut terminal,
        config,
        cwd,
        mcp,
        plugins,
        snapshots,
        lsp,
        session,
    )
    .await;

    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

#[allow(clippy::too_many_arguments)]
async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    mut config: Config,
    cwd: PathBuf,
    mcp: Arc<McpRegistry>,
    plugins: Arc<PluginHost>,
    snapshots: Option<Arc<Snapshots>>,
    lsp: Arc<LspManager>,
    mut session: Option<SessionLog>,
) -> Result<()> {
    let mut app = App::new(
        config.model.clone(),
        cwd.display().to_string(),
        config.mode,
        config.reasoning,
    );
    app.items.push(ChatItem::Info(
        "Ask me to build, refactor, debug or explain code. Type /help for commands, Ctrl+C to quit."
            .to_string(),
    ));
    app.items.push(ChatItem::Info(format!(
        "mode: {} (Shift+Tab) · reasoning: {} → {} (Ctrl+R) · model: {}",
        app.mode.label(),
        app.reasoning.label(),
        config.effective_reasoning().label(),
        config.model
    )));
    app.items.push(ChatItem::Info(format!(
        "ecosystem: {} · mcp: {} server(s), {} tool(s) · plugins: {}{} · memory: {} entr{}",
        config.ecosystem.summary(),
        mcp.server_count(),
        mcp.tool_count(),
        plugins.plugin_count(),
        if plugins.is_active() {
            ""
        } else {
            " (runtime unavailable)"
        },
        config.memory.len(),
        if config.memory.len() == 1 { "y" } else { "ies" }
    )));
    app.items.push(ChatItem::Info(
        "tips: type / to list commands · /models switches model · @path or Ctrl+V attaches images · /undo and /redo revert changes"
            .to_string(),
    ));
    if config.api_key.trim().is_empty() {
        app.items.push(ChatItem::Info(
            "no provider connected — type /connect to add an API key".to_string(),
        ));
    }

    if let Some(log) = &session {
        match log.messages() {
            Ok(messages) => {
                for message in &messages {
                    match message.role.as_str() {
                        "user" => {
                            if let Some(content) = message.display() {
                                app.items.push(ChatItem::User(content));
                            }
                        }
                        "assistant" => {
                            if let Some(content) = message.display() {
                                app.items.push(ChatItem::Assistant(content));
                            }
                        }
                        _ => {}
                    }
                }
                app.history = messages;
                app.items.push(ChatItem::Info(format!(
                    "resumed session {} ({} message{})",
                    log.id(),
                    app.history.len(),
                    if app.history.len() == 1 { "" } else { "s" }
                )));
            }
            Err(err) => app.items.push(ChatItem::Error(format!("session: {err:#}"))),
        }
    }

    let mut reader = EventStream::new();
    let mut rx: Option<UnboundedReceiver<AgentEvent>> = None;
    let (models_tx, mut models_rx) = unbounded_channel::<Result<Vec<String>, String>>();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));

    let (approval_tx, mut approval_rx) = unbounded_channel::<ApprovalRequest>();
    let approve: Approver = Arc::new(move |tool, detail| {
        let tx = approval_tx.clone();
        Box::pin(async move {
            let (respond, response) = tokio::sync::oneshot::channel();
            if tx
                .send(ApprovalRequest {
                    tool,
                    detail,
                    respond,
                })
                .is_err()
            {
                return false;
            }
            response.await.unwrap_or(false)
        })
    });

    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        let mut got_agent_event = false;
        tokio::select! {
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => handle_key(
                        key, &mut app, &mut config, &cwd, &mut rx, &mcp, &plugins,
                        snapshots.as_ref(), &lsp, &mut session, &approve, &models_tx,
                    ),
                    Some(Ok(Event::Paste(text))) => handle_paste(text, &mut app),
                    _ => {}
                }
            }
            agent_event = recv_opt(&mut rx) => {
                if let Some(event) = agent_event {
                    handle_agent_event(event, &mut app);
                    got_agent_event = true;
                }
            }
            result = models_rx.recv() => {
                if let Some(result) = result {
                    handle_model_result(result, &mut app);
                }
            }
            approval = approval_rx.recv() => {
                if let Some(request) = approval {
                    app.status = format!("approve `{}`? y/n — {}", request.tool, request.detail);
                    app.pending_approval = Some(request);
                }
            }
            _ = tick.tick(), if app.busy => {}
        }

        if got_agent_event && !app.busy {
            rx = None;
        }
        if app.should_quit {
            break;
        }
    }
    Ok(())
}

async fn recv_opt(rx: &mut Option<UnboundedReceiver<AgentEvent>>) -> Option<AgentEvent> {
    match rx {
        Some(receiver) => receiver.recv().await,
        None => futures::future::pending().await,
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_key(
    key: KeyEvent,
    app: &mut App,
    config: &mut Config,
    cwd: &Path,
    rx: &mut Option<UnboundedReceiver<AgentEvent>>,
    mcp: &Arc<McpRegistry>,
    plugins: &Arc<PluginHost>,
    snapshots: Option<&Arc<Snapshots>>,
    lsp: &Arc<LspManager>,
    session: &mut Option<SessionLog>,
    approve: &Approver,
    models_tx: &UnboundedSender<Result<Vec<String>, String>>,
) {
    if app.connect.is_some() {
        handle_connect_key(key, app, config);
        return;
    }

    if app.models.is_some() {
        handle_models_key(key, app, config);
        return;
    }

    if let Some(request) = app.pending_approval.take() {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {
                let _ = request.respond.send(true);
                app.status = "thinking...".to_string();
            }
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                let _ = request.respond.send(false);
                app.status = "denied".to_string();
            }
            _ => app.pending_approval = Some(request),
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            escape_action(app);
            refresh_suggestions(app, config);
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
        }
        KeyCode::BackTab => {
            config.mode = config.mode.next();
            app.mode = config.mode;
            app.status = format!("mode: {}", app.mode.label());
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            config.reasoning = config.reasoning.next();
            app.reasoning = config.reasoning;
            app.status = format!(
                "reasoning: {} (effective: {})",
                app.reasoning.label(),
                config.effective_reasoning().label()
            );
        }
        KeyCode::Enter => {
            if app.busy {
                let raw = app.input.trim().to_string();
                if raw.is_empty() {
                    return;
                }
                app.input.clear();
                app.items.push(ChatItem::User(raw.clone()));
                app.auto_scroll = true;
                app.steering.push(Message::user(raw));
                app.status = "queued guidance...".to_string();
                return;
            }
            let raw = app.input.trim().to_string();
            if raw.is_empty() && app.attachments.is_empty() {
                return;
            }
            if let Some(hint) = app.suggestions.get(app.suggestion_index) {
                let completed = format!("/{}", hint.name);
                if raw != completed {
                    app.input = completed;
                    refresh_suggestions(app, config);
                    return;
                }
            }
            if raw == "/undo" || raw == "/redo" {
                app.input.clear();
                refresh_suggestions(app, config);
                let result = match snapshots {
                    Some(snapshots) => {
                        if raw == "/undo" {
                            snapshots.undo()
                        } else {
                            snapshots.redo()
                        }
                    }
                    None => {
                        app.items.push(ChatItem::Error(
                            "snapshots unavailable (git required)".to_string(),
                        ));
                        return;
                    }
                };
                match result {
                    Ok(true) => app.items.push(ChatItem::Info(format!("{raw}: restored"))),
                    Ok(false) => app
                        .items
                        .push(ChatItem::Info(format!("{raw}: nothing to restore"))),
                    Err(err) => app.items.push(ChatItem::Error(format!("{raw}: {err:#}"))),
                }
                return;
            }
            if raw == "/compact" {
                app.input.clear();
                refresh_suggestions(app, config);
                if app.busy {
                    return;
                }
                let config = config.clone();
                let history = app.history.clone();
                let fallback = history.clone();
                let (tx, new_rx) = unbounded_channel();
                *rx = Some(new_rx);
                app.busy = true;
                app.busy_since = Some(std::time::Instant::now());
                app.status = "compacting...".to_string();
                tokio::spawn(async move {
                    match crate::compact::compact(&config, history).await {
                        Ok(messages) => {
                            let _ = tx.send(AgentEvent::Finished(messages));
                        }
                        Err(err) => {
                            let _ = tx.send(AgentEvent::Error(format!("compact: {err:#}")));
                            let _ = tx.send(AgentEvent::Finished(fallback));
                        }
                    }
                });
                return;
            }
            if raw == "/connect" || raw.starts_with("/connect ") {
                app.input.clear();
                refresh_suggestions(app, config);
                let provider = raw
                    .strip_prefix("/connect")
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let mut state = ConnectState::new();
                if !provider.is_empty() {
                    state.step = ConnectStep::Key {
                        provider: resolve_provider_choice(&provider),
                    };
                }
                app.connect = Some(state);
                app.status = "connecting...".to_string();
                return;
            }
            if raw == "/models" || raw.starts_with("/models ") {
                app.input.clear();
                refresh_suggestions(app, config);
                if config.api_key.trim().is_empty() {
                    app.items.push(ChatItem::Error(
                        "no provider connected — run /connect to add an API key".to_string(),
                    ));
                    return;
                }
                let filter = raw
                    .strip_prefix("/models")
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let mut state = ModelsState::loading();
                state.filter = filter;
                app.models = Some(state);
                app.status = "loading models...".to_string();
                let config = config.clone();
                let tx = models_tx.clone();
                tokio::spawn(async move {
                    let result = LlmClient::new(config)
                        .list_models()
                        .await
                        .map_err(|err| format!("{err:#}"));
                    let _ = tx.send(result);
                });
                return;
            }
            if raw == "/help" || raw == "/?" {
                app.input.clear();
                refresh_suggestions(app, config);
                app.items.push(ChatItem::Info(help_text(config)));
                return;
            }
            app.input.clear();
            refresh_suggestions(app, config);
            if config.api_key.trim().is_empty() {
                app.items.push(ChatItem::Error(
                    "no provider connected — run /connect to add an API key".to_string(),
                ));
                return;
            }
            let resolved = config.resolve_command(&raw);
            let prompt = resolved
                .as_ref()
                .map(|command| command.prompt.clone())
                .unwrap_or_else(|| raw.clone());
            let command_agent = resolved.as_ref().and_then(|command| command.agent.clone());
            let subtask = resolved.as_ref().is_some_and(|command| command.subtask);

            if subtask && command_agent.is_none() {
                app.items.push(ChatItem::Error(
                    "subtask command requires an `agent` in its frontmatter".to_string(),
                ));
                return;
            }
            if let Some(name) = &command_agent {
                match config.ecosystem.agent(name) {
                    None => {
                        app.items
                            .push(ChatItem::Error(format!("unknown agent `{name}`")));
                        return;
                    }
                    Some(agent) if subtask && agent.mode == AgentMode::Primary => {
                        app.items.push(ChatItem::Error(format!(
                            "agent `{name}` is primary and cannot run as a subagent"
                        )));
                        return;
                    }
                    Some(_) => {}
                }
            }

            let mut parts = std::mem::take(&mut app.attachments);
            for path in media::referenced_attachments(&raw, cwd) {
                match media::load_attachment(&path) {
                    Ok(part) => parts.push(part),
                    Err(err) => app
                        .items
                        .push(ChatItem::Error(format!("attachment: {err:#}"))),
                }
            }
            let media_count = parts.len();
            let user = if parts.is_empty() {
                Message::user(prompt.clone())
            } else {
                Message::user_parts(prompt.clone(), parts)
            };

            let log = match ensure_session(session, cwd) {
                Ok(log) => log,
                Err(err) => {
                    app.items.push(ChatItem::Error(format!("session: {err:#}")));
                    return;
                }
            };
            if let Err(err) = log.append(&user) {
                app.items.push(ChatItem::Error(format!("session: {err:#}")));
                return;
            }
            let shown = if media_count > 0 {
                format!("{raw}\n[{media_count} attachment(s)]")
            } else {
                raw
            };
            app.items.push(ChatItem::User(shown));
            app.history.push(user);
            app.busy = true;
            app.busy_since = Some(std::time::Instant::now());
            app.auto_scroll = true;
            app.assistant_open = false;
            app.status = "thinking...".to_string();

            let (tx, new_rx) = unbounded_channel();
            *rx = Some(new_rx);
            let config = config.clone();
            let cwd = cwd.to_path_buf();
            let history = app.history.clone();
            let runtime = Runtime {
                mcp: Arc::clone(mcp),
                plugins: Arc::clone(plugins),
                session: session.clone().map(Arc::new),
                snapshots: snapshots.cloned(),
                lsp: Arc::clone(lsp),
                approve: Arc::clone(approve),
                steering: app.steering.clone(),
            };
            tokio::spawn(async move {
                if subtask {
                    let agent_name = command_agent.unwrap_or_default();
                    agent::run_subagent(config, cwd, history, agent_name, prompt, tx, runtime)
                        .await;
                } else {
                    let mut config = config;
                    if let Some(name) = command_agent {
                        config.active_agent = config.ecosystem.agent(&name).cloned();
                    }
                    agent::run(config, cwd, history, tx, runtime).await;
                }
            });
        }
        KeyCode::Char('v') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            match media::clipboard_image() {
                Some(part) => {
                    app.attachments.push(part);
                    app.status = format!("{} attachment(s) pending", app.attachments.len());
                }
                None => {
                    app.status = "no image found on clipboard".to_string();
                }
            }
        }
        KeyCode::Char(ch) => {
            app.input.push(ch);
            app.auto_scroll = true;
            refresh_suggestions(app, config);
        }
        KeyCode::Backspace => {
            app.input.pop();
            refresh_suggestions(app, config);
        }
        KeyCode::Tab if !app.suggestions.is_empty() => {
            if let Some(hint) = app.suggestions.get(app.suggestion_index) {
                app.input = format!("/{}", hint.name);
                refresh_suggestions(app, config);
            }
        }
        KeyCode::Up => {
            if !app.suggestions.is_empty() {
                app.suggestion_index = app.suggestion_index.saturating_sub(1);
            } else {
                app.scroll = app.scroll.saturating_sub(1);
                app.auto_scroll = false;
            }
        }
        KeyCode::Down => {
            if !app.suggestions.is_empty() {
                let last = app.suggestions.len().saturating_sub(1);
                app.suggestion_index = (app.suggestion_index + 1).min(last);
            } else {
                app.scroll = app.scroll.saturating_add(1);
                app.auto_scroll = false;
            }
        }
        _ => {}
    }
}

fn escape_action(app: &mut App) {
    if app.input.is_empty() {
        app.should_quit = true;
    } else {
        app.input.clear();
    }
}

fn help_text(config: &Config) -> String {
    let mut lines = vec![
        "built-in commands:".to_string(),
        "  /help                 show this help".to_string(),
        "  /connect [provider]   connect a provider and save its API key".to_string(),
        "  /models [filter]      list and switch the active model".to_string(),
        "  /undo, /redo          revert or reapply the agent's file changes".to_string(),
        "  /compact              summarize the conversation to free context".to_string(),
        "keys: Enter send · Shift+Tab mode · Ctrl+R reasoning · Ctrl+V image · ↑/↓ scroll · Ctrl+C quit"
            .to_string(),
    ];
    if !config.ecosystem.commands.is_empty() {
        let names: Vec<String> = config
            .ecosystem
            .commands
            .iter()
            .map(|command| format!("/{}", command.name))
            .collect();
        lines.push(format!("commands: {}", names.join(", ")));
    }
    if !config.ecosystem.agents.is_empty() {
        let names: Vec<&str> = config
            .ecosystem
            .agents
            .iter()
            .map(|agent| agent.name.as_str())
            .collect();
        lines.push(format!("agents: {}", names.join(", ")));
    }
    if !config.ecosystem.skills.is_empty() {
        let names: Vec<&str> = config
            .ecosystem
            .skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect();
        lines.push(format!("skills: {}", names.join(", ")));
    }
    lines.join("\n")
}

fn resolve_provider_choice(value: &str) -> String {
    match value.trim() {
        "1" => "openai".to_string(),
        "2" => "deepseek".to_string(),
        "3" => "anthropic".to_string(),
        other => crate::auth::canonical_provider(other),
    }
}

/// The built-in slash commands surfaced in the input autocomplete.
fn builtin_commands() -> Vec<CommandHint> {
    vec![
        CommandHint {
            name: "help".to_string(),
            description: "show help".to_string(),
        },
        CommandHint {
            name: "models".to_string(),
            description: "choose a model".to_string(),
        },
        CommandHint {
            name: "connect".to_string(),
            description: "connect a provider".to_string(),
        },
        CommandHint {
            name: "compact".to_string(),
            description: "summarize the conversation".to_string(),
        },
        CommandHint {
            name: "undo".to_string(),
            description: "revert file changes".to_string(),
        },
        CommandHint {
            name: "redo".to_string(),
            description: "reapply file changes".to_string(),
        },
    ]
}

/// Recomputes the slash-command suggestions for the current input.
fn refresh_suggestions(app: &mut App, config: &Config) {
    app.suggestions.clear();
    app.suggestion_index = 0;
    if app.connect.is_some() || app.models.is_some() || app.busy {
        return;
    }
    let Some(query) = app.input.strip_prefix('/') else {
        return;
    };
    if query.contains(char::is_whitespace) {
        return;
    }
    let query = query.to_ascii_lowercase();
    let mut hints = builtin_commands();
    for command in &config.ecosystem.commands {
        hints.push(CommandHint {
            name: command.name.clone(),
            description: command.description.clone().unwrap_or_default(),
        });
    }
    app.suggestions = hints
        .into_iter()
        .filter(|hint| hint.name.to_ascii_lowercase().starts_with(&query))
        .collect();
}

fn handle_models_key(key: KeyEvent, app: &mut App, config: &mut Config) {
    let Some(mut state) = app.models.take() else {
        return;
    };
    let mut keep = true;
    match key.code {
        KeyCode::Esc => {
            keep = false;
            app.status = "model unchanged".to_string();
        }
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
        }
        KeyCode::Down => {
            let last = state.filtered().len().saturating_sub(1);
            state.selected = (state.selected + 1).min(last);
        }
        KeyCode::Backspace => {
            state.filter.pop();
            state.selected = 0;
        }
        KeyCode::Enter => {
            if let Some(model) = state.selected_model().map(str::to_string) {
                match Config::set_active_model_at(&Config::config_path(), &model) {
                    Ok(()) => {
                        config.model = model.clone();
                        app.model = model.clone();
                        app.items
                            .push(ChatItem::Info(format!("model set to {model}")));
                        app.status = "ready".to_string();
                    }
                    Err(err) => state.error = Some(format!("{err:#}")),
                }
                keep = false;
            }
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            state.filter.push(c);
            state.selected = 0;
        }
        _ => {}
    }
    if keep {
        app.models = Some(state);
    }
}

fn handle_model_result(result: Result<Vec<String>, String>, app: &mut App) {
    if app.models.is_none() {
        return;
    }
    match result {
        Ok(models) if models.is_empty() => {
            app.models = None;
            app.items
                .push(ChatItem::Error("provider returned no models".to_string()));
            app.status = "ready".to_string();
        }
        Ok(models) => {
            app.status = format!("{} model(s) — pick one", models.len());
            app.models = Some(ModelsState::ready(models));
        }
        Err(err) => {
            app.models = None;
            app.items.push(ChatItem::Error(format!("models: {err}")));
            app.status = "ready".to_string();
        }
    }
}

fn handle_paste(text: String, app: &mut App) {
    let mut text = text;
    text.retain(|ch| ch != '\r' && ch != '\n');
    if let Some(state) = app.connect.as_mut() {
        state.input.push_str(&text);
    } else if let Some(state) = app.models.as_mut() {
        state.filter.push_str(&text);
        state.selected = 0;
    } else {
        app.input.push_str(&text);
        app.auto_scroll = true;
    }
}

fn handle_connect_key(key: KeyEvent, app: &mut App, config: &mut Config) {
    let Some(mut state) = app.connect.take() else {
        return;
    };
    let mut keep = true;
    match key.code {
        KeyCode::Esc => {
            keep = false;
            app.status = "connect cancelled".to_string();
        }
        KeyCode::Enter => {
            let value = state.input.trim().to_string();
            match state.step.clone() {
                ConnectStep::Provider => {
                    if value.is_empty() {
                        state.error = Some("enter a provider name or number".to_string());
                    } else {
                        state.step = ConnectStep::Key {
                            provider: resolve_provider_choice(&value),
                        };
                        state.input.clear();
                        state.error = None;
                    }
                }
                ConnectStep::Key { provider } => {
                    if value.is_empty() {
                        state.error = Some("enter an API key".to_string());
                    } else {
                        match crate::auth::connect(&provider, &value) {
                            Ok(name) => {
                                config.apply_provider(&name, &value);
                                app.model = config.model.clone();
                                app.items.push(ChatItem::Info(format!(
                                    "connected to {name} ({})",
                                    config.model
                                )));
                                app.status = "ready".to_string();
                                keep = false;
                            }
                            Err(err) => state.error = Some(format!("{err:#}")),
                        }
                    }
                }
            }
        }
        KeyCode::Backspace => {
            state.input.pop();
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            state.input.push(c);
        }
        _ => {}
    }
    if keep {
        app.connect = Some(state);
    }
}

fn ensure_session<'a>(session: &'a mut Option<SessionLog>, cwd: &Path) -> Result<&'a SessionLog> {
    if session.is_none() {
        *session = Some(SessionLog::create(cwd)?);
    }
    Ok(session.as_ref().expect("session was just created"))
}

fn handle_agent_event(event: AgentEvent, app: &mut App) {
    match event {
        AgentEvent::Text(delta) => {
            app.auto_scroll = true;
            app.push_assistant_delta(delta);
        }
        AgentEvent::ToolCall { name, args } => {
            app.assistant_open = false;
            app.auto_scroll = true;
            app.items.push(ChatItem::Tool { name, args });
            app.status = "running tool...".to_string();
        }
        AgentEvent::ToolProgress { name, chunk } => {
            app.auto_scroll = true;
            let append = matches!(
                app.items.last(),
                Some(ChatItem::ToolProgress { name: last, .. }) if last == &name
            );
            if append {
                if let Some(ChatItem::ToolProgress { output, .. }) = app.items.last_mut() {
                    if !output.is_empty() {
                        output.push('\n');
                    }
                    output.push_str(&chunk);
                }
            } else {
                app.items.push(ChatItem::ToolProgress {
                    name,
                    output: chunk,
                });
            }
        }
        AgentEvent::ToolResult { name, output } => {
            app.auto_scroll = true;
            app.items.push(ChatItem::ToolResult { name, output });
            app.status = "thinking...".to_string();
        }
        AgentEvent::Error(message) => {
            app.items.push(ChatItem::Error(message));
        }
        AgentEvent::Finished(history) => {
            app.history = history;
            app.busy = false;
            app.busy_since = None;
            app.assistant_open = false;
            app.auto_scroll = true;
            app.status = "ready".to_string();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Mode, Reasoning};

    fn test_app() -> App {
        App::new(
            "test-model".to_string(),
            "/tmp".to_string(),
            Mode::Build,
            Reasoning::Auto,
        )
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn provider_choice_maps_numbers_and_aliases() {
        assert_eq!(resolve_provider_choice("1"), "openai");
        assert_eq!(resolve_provider_choice("2"), "deepseek");
        assert_eq!(resolve_provider_choice("3"), "anthropic");
        assert_eq!(resolve_provider_choice("DeepSeek"), "deepseek");
        assert_eq!(resolve_provider_choice("gpt-4o"), "openai");
        assert_eq!(resolve_provider_choice("my-endpoint"), "my-endpoint");
    }

    #[test]
    fn paste_appends_to_input_and_connect_prompt() {
        let mut app = test_app();
        handle_paste("sk-abc\ndef".to_string(), &mut app);
        assert_eq!(app.input, "sk-abcdef");

        app.connect = Some(ConnectState::new());
        handle_paste("deepseek".to_string(), &mut app);
        assert_eq!(app.connect.as_ref().unwrap().input, "deepseek");
    }

    #[test]
    fn connect_selects_provider_then_waits_for_key() {
        let mut app = test_app();
        let mut config = Config::default();
        app.connect = Some(ConnectState::new());

        handle_connect_key(key(KeyCode::Char('2')), &mut app, &mut config);
        handle_connect_key(key(KeyCode::Enter), &mut app, &mut config);

        let state = app.connect.as_ref().unwrap();
        assert!(matches!(
            &state.step,
            ConnectStep::Key { provider } if provider == "deepseek"
        ));
        assert!(state.input.is_empty());
        assert!(state.error.is_none());
    }

    #[test]
    fn connect_empty_provider_reports_error() {
        let mut app = test_app();
        let mut config = Config::default();
        app.connect = Some(ConnectState::new());

        handle_connect_key(key(KeyCode::Enter), &mut app, &mut config);

        assert!(app.connect.as_ref().unwrap().error.is_some());
    }

    #[test]
    fn connect_escape_cancels() {
        let mut app = test_app();
        let mut config = Config::default();
        app.connect = Some(ConnectState::new());

        handle_connect_key(key(KeyCode::Esc), &mut app, &mut config);

        assert!(app.connect.is_none());
    }

    #[test]
    fn escape_clears_input_then_quits() {
        let mut app = test_app();
        app.input = "draft".to_string();

        escape_action(&mut app);
        assert!(app.input.is_empty());
        assert!(!app.should_quit);

        escape_action(&mut app);
        assert!(app.should_quit);
    }

    #[test]
    fn help_lists_builtins_and_discovered_commands() {
        let mut config = Config::default();
        config
            .ecosystem
            .commands
            .push(crate::ecosystem::CommandDef {
                name: "review".to_string(),
                description: None,
                template: String::new(),
                agent: None,
                subtask: false,
            });

        let help = help_text(&config);
        assert!(help.contains("built-in commands"));
        assert!(help.contains("/connect"));
        assert!(help.contains("/review"));
    }

    #[test]
    fn suggestions_show_for_slash_and_filter() {
        let config = Config::default();
        let mut app = test_app();

        app.input = "/".to_string();
        refresh_suggestions(&mut app, &config);
        assert!(app.suggestions.iter().any(|hint| hint.name == "models"));

        app.input = "/mo".to_string();
        refresh_suggestions(&mut app, &config);
        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0].name, "models");

        app.input = "hello".to_string();
        refresh_suggestions(&mut app, &config);
        assert!(app.suggestions.is_empty());

        app.input = "/models foo".to_string();
        refresh_suggestions(&mut app, &config);
        assert!(app.suggestions.is_empty());
    }

    #[test]
    fn suggestions_include_ecosystem_commands() {
        let mut config = Config::default();
        config
            .ecosystem
            .commands
            .push(crate::ecosystem::CommandDef {
                name: "review".to_string(),
                description: Some("review the diff".to_string()),
                template: String::new(),
                agent: None,
                subtask: false,
            });
        let mut app = test_app();
        app.input = "/rev".to_string();
        refresh_suggestions(&mut app, &config);
        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0].name, "review");
    }

    #[test]
    fn models_state_filters_and_selects() {
        let mut state = ModelsState::ready(vec![
            "deepseek-chat".to_string(),
            "deepseek-reasoner".to_string(),
        ]);
        assert_eq!(state.filtered().len(), 2);
        assert_eq!(state.selected_model(), Some("deepseek-chat"));

        state.filter = "reason".to_string();
        assert_eq!(state.filtered(), vec!["deepseek-reasoner"]);
        assert_eq!(state.selected_model(), Some("deepseek-reasoner"));

        state.filter = "missing".to_string();
        assert!(state.selected_model().is_none());
    }
}
