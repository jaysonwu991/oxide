pub mod app;
pub mod ui;

use crate::agent::{self, AgentEvent, ApprovalRequest, Approver, Runtime};
use crate::config::{Config, Reasoning};
use crate::ecosystem::AgentMode;
use crate::llm::{LlmClient, Message};
use crate::lsp::LspManager;
use crate::mcp::McpRegistry;
use crate::media;
use crate::plugin::PluginHost;
use crate::session::SessionLog;
use crate::snapshots::Snapshots;
use crate::tui::app::{
    App, ChatItem, CommandHint, ConnectState, ConnectStep, ModelsState, TrustState,
};
use anyhow::{Context, Result};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind,
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

const MAX_TOOL_PROGRESS_BYTES: usize = 6_000;

pub async fn run(config: Config, cwd: PathBuf, session: Option<SessionLog>) -> Result<()> {
    let mcp = Arc::new(McpRegistry::new(&config.ecosystem.mcp));
    let plugins = Arc::new(PluginHost::spawn(&config.ecosystem.plugins, &cwd).await);
    let snapshots = Snapshots::open(&cwd).ok().map(Arc::new);
    let lsp = Arc::new(LspManager::new());

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableMouseCapture
    )?;
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
        DisableMouseCapture,
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
    app.context_limit = context_limit(&config);
    app.session_name = session.as_ref().and_then(|log| log.name());
    app.theme = config.theme.clone();

    // Project trust: prompt once for projects with resources that can execute
    // or reshape the agent, unless a decision was saved or the default applies.
    let trust_store = crate::trust::TrustStore::load().unwrap_or_default();
    if let Some(saved) = trust_store.decision(&cwd) {
        config.trusted = saved;
    } else if crate::trust::requires_trust(&cwd)
        && config.default_project_trust == crate::trust::DefaultTrust::Ask
    {
        config.trusted = false;
        app.trust = Some(TrustState::new(
            cwd.display().to_string(),
            crate::trust::project_resources(&cwd),
        ));
    } else {
        config.trusted =
            crate::trust::resolve(&trust_store, &cwd, None, config.default_project_trust)
                .is_trusted();
    }
    if !config.trusted {
        config.reload_ecosystem(&cwd);
    }

    app.items.push(ChatItem::Banner);
    app.items.push(ChatItem::Info(
        "Build, refactor, debug, and understand your code.".to_string(),
    ));
    app.items.push(ChatItem::Info(format!(
        "ecosystem: {} · mcp: {} configured, {} loaded, {} tool(s) · plugins: {}{} · memory: {} entr{}",
        config.ecosystem.summary(),
        mcp.configured_count(),
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
        "tips: Enter send or guide · Alt+Enter follow-up · Shift+Enter newline · / commands · Ctrl+O tool details · /models switch model · /init create AGENTS.md · @path or Ctrl+V attach images · ↑/↓ history · Ctrl+C quit"
            .to_string(),
    ));
    if !config.ecosystem.context_files.is_empty() {
        let files: Vec<String> = config
            .ecosystem
            .context_files
            .iter()
            .map(|path| {
                path.file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.display().to_string())
            })
            .collect();
        app.items.push(ChatItem::Info(format!(
            "context files: {}",
            files.join(", ")
        )));
    }
    if config.api_key.trim().is_empty() {
        app.items.push(ChatItem::Info(
            "Welcome! Connect a model provider to send your first message.".to_string(),
        ));
        app.connect = Some(ConnectState::new());
        app.status = "setup required".to_string();
    }

    if let Some(log) = &session {
        match log.messages() {
            Ok(messages) => {
                for message in &messages {
                    match message.role.as_str() {
                        "user" => {
                            if let Some(content) = message.display() {
                                if !content.trim().is_empty() {
                                    app.input_history.push(content.clone());
                                }
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
    if !config.api_key.trim().is_empty() && config.model_catalog.is_empty() {
        let warm_config = config.clone();
        tokio::spawn(async move {
            let _ = LlmClient::new(warm_config).list_models().await;
        });
    }
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
                    Some(Ok(Event::Mouse(mouse))) => handle_mouse(mouse, &mut app),
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
/// Ctrl+C is a global quit shortcut, honored even while a dialog is open.
fn is_quit_shortcut(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn is_newline_shortcut(key: &KeyEvent) -> bool {
    key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT)
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
    // Ctrl+C always quits, even while a dialog or the trust prompt is open.
    if is_quit_shortcut(&key) {
        app.should_quit = true;
        return;
    }

    if app.connect.is_some() {
        handle_connect_key(key, app, config);
        return;
    }

    if app.trust.is_some() {
        handle_trust_key(key, app, config, cwd);
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
        KeyCode::BackTab => {
            config.mode = config.mode.next();
            app.mode = config.mode;
            app.status = format!("mode: {}", app.mode.label());
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            config.reasoning = config.reasoning.next();
            app.reasoning = config.reasoning;
            app.status = if app.reasoning == Reasoning::Auto {
                "reasoning: auto (provider native)".to_string()
            } else {
                format!("reasoning: {}", app.reasoning.label())
            };
        }
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_tool_output();
            app.status = if app.expand_tools {
                "tool output expanded".to_string()
            } else {
                "tool output collapsed".to_string()
            };
        }
        KeyCode::Enter if is_newline_shortcut(&key) => {
            app.input.push('\n');
            app.auto_scroll = true;
            refresh_suggestions(app, config);
        }
        KeyCode::Enter => {
            if app.busy {
                let raw = app.input.trim().to_string();
                if raw.is_empty() {
                    return;
                }
                app.remember_input(&raw);
                app.input.clear();
                app.items.push(ChatItem::User(raw.clone()));
                app.auto_scroll = true;
                if key.modifiers.contains(KeyModifiers::ALT) {
                    app.follow_ups.push(Message::user(raw));
                    app.status = "queued follow-up...".to_string();
                } else {
                    app.steering.push(Message::user(raw));
                    app.status = "queued guidance...".to_string();
                }
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
            app.remember_input(&raw);
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
            if raw == "/connect"
                || raw.starts_with("/connect ")
                || raw == "/login"
                || raw.starts_with("/login ")
            {
                app.input.clear();
                refresh_suggestions(app, config);
                let rest = raw
                    .strip_prefix("/connect")
                    .or_else(|| raw.strip_prefix("/login"))
                    .unwrap_or_default();
                let provider = rest.trim().to_string();
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
            if raw == "/logout" || raw.starts_with("/logout ") {
                app.input.clear();
                refresh_suggestions(app, config);
                let requested = raw.strip_prefix("/logout").unwrap_or_default().trim();
                let provider = if requested.is_empty() {
                    config.provider.clone()
                } else {
                    crate::auth::canonical_provider(requested)
                };
                match logout_provider(&provider) {
                    Ok(true) => {
                        config.api_key.clear();
                        app.items.push(ChatItem::Info(format!(
                            "logged out of {provider} — run /login to reconnect"
                        )));
                    }
                    Ok(false) => app.items.push(ChatItem::Info(format!(
                        "no stored credentials for {provider}"
                    ))),
                    Err(err) => app.items.push(ChatItem::Error(format!("{err:#}"))),
                }
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
            if raw == "/clone" {
                app.input.clear();
                refresh_suggestions(app, config);
                if app.busy {
                    return;
                }
                match SessionLog::fork(cwd, &app.history) {
                    Ok(log) => {
                        app.items.push(ChatItem::Info(format!(
                            "cloned session into {} ({} messages)",
                            log.id(),
                            app.history.len()
                        )));
                        *session = Some(log);
                    }
                    Err(err) => app.items.push(ChatItem::Error(format!("{err:#}"))),
                }
                return;
            }
            if raw == "/fork" || raw.starts_with("/fork ") {
                app.input.clear();
                refresh_suggestions(app, config);
                if app.busy {
                    return;
                }
                let requested = raw
                    .strip_prefix("/fork")
                    .unwrap_or_default()
                    .trim()
                    .parse::<usize>()
                    .ok();
                match requested {
                    Some(index) => match fork_point(&app.history, index) {
                        Some((cut, prompt)) => match SessionLog::fork(cwd, &app.history[..cut]) {
                            Ok(log) => {
                                app.history.truncate(cut);
                                *session = Some(log);
                                app.input = prompt;
                                app.items.push(ChatItem::Info(format!(
                                    "forked at message {index} — edit and resend"
                                )));
                            }
                            Err(err) => app.items.push(ChatItem::Error(format!("{err:#}"))),
                        },
                        None => app.items.push(ChatItem::Error(format!(
                            "no user message at index {index} (1-based)"
                        ))),
                    },
                    None => {
                        for (position, message) in user_messages(&app.history) {
                            let preview: String = message.chars().take(72).collect();
                            app.items
                                .push(ChatItem::Info(format!("{position}. {preview}")));
                        }
                        app.items.push(ChatItem::Info(
                            "use /fork <n> to branch from a user message".to_string(),
                        ));
                    }
                }
                return;
            }
            if raw == "/tree" {
                app.input.clear();
                refresh_suggestions(app, config);
                for (position, message) in user_messages(&app.history) {
                    let preview: String = message.chars().take(72).collect();
                    app.items
                        .push(ChatItem::Info(format!("{position}. {preview}")));
                }
                app.items.push(ChatItem::Info(
                    "branch with /fork <n> or duplicate with /clone".to_string(),
                ));
                return;
            }
            if raw == "/new" {
                app.input.clear();
                refresh_suggestions(app, config);
                if app.busy {
                    return;
                }
                app.history.clear();
                app.items.clear();
                app.invalidate_render_cache();
                app.steering = crate::agent::Steering::new();
                app.follow_ups = crate::agent::Steering::new();
                match SessionLog::create(cwd) {
                    Ok(log) => {
                        app.items
                            .push(ChatItem::Info(format!("new session {}", log.id())));
                        *session = Some(log);
                    }
                    Err(err) => app.items.push(ChatItem::Error(format!("session: {err:#}"))),
                }
                return;
            }
            if raw == "/session" {
                app.input.clear();
                refresh_suggestions(app, config);
                app.items
                    .push(ChatItem::Info(session_info(session, &app.history)));
                return;
            }
            if raw == "/name" || raw.starts_with("/name ") {
                app.input.clear();
                refresh_suggestions(app, config);
                let name = raw.strip_prefix("/name").unwrap_or_default().trim();
                if name.is_empty() {
                    app.items
                        .push(ChatItem::Error("usage: /name <name>".into()));
                    return;
                }
                let log = match ensure_session(session, cwd) {
                    Ok(log) => log,
                    Err(err) => {
                        app.items.push(ChatItem::Error(format!("session: {err:#}")));
                        return;
                    }
                };
                match log.set_name(name) {
                    Ok(()) => app
                        .items
                        .push(ChatItem::Info(format!("session named `{name}`"))),
                    Err(err) => app.items.push(ChatItem::Error(format!("{err:#}"))),
                }
                return;
            }
            if raw == "/model" || raw.starts_with("/model ") {
                app.input.clear();
                refresh_suggestions(app, config);
                let requested = raw.strip_prefix("/model").unwrap_or_default().trim();
                if requested.is_empty() {
                    app.items.push(ChatItem::Info(format!(
                        "model: {} ({}); usage: /model <id>",
                        config.model, config.provider
                    )));
                    return;
                }
                config.model = requested.to_string();
                if let Err(err) = Config::set_active_model_at(&Config::config_path(), requested) {
                    app.items.push(ChatItem::Error(format!("{err:#}")));
                }
                app.items
                    .push(ChatItem::Info(format!("model set to {requested}")));
                return;
            }
            if raw == "/thinking" || raw.starts_with("/thinking ") {
                app.input.clear();
                refresh_suggestions(app, config);
                let requested = raw.strip_prefix("/thinking").unwrap_or_default().trim();
                if requested.is_empty() {
                    app.items.push(ChatItem::Info(format!(
                        "thinking: {}; usage: /thinking <off|low|medium|high|auto>",
                        config.reasoning.label()
                    )));
                    return;
                }
                match crate::config::Reasoning::parse(requested) {
                    Some(level) => {
                        config.reasoning = level;
                        app.items
                            .push(ChatItem::Info(format!("thinking set to {}", level.label())));
                    }
                    None => app.items.push(ChatItem::Error(format!(
                        "unknown thinking level `{requested}`",
                    ))),
                }
                return;
            }
            if raw == "/theme" || raw.starts_with("/theme ") {
                app.input.clear();
                refresh_suggestions(app, config);
                let requested = raw.strip_prefix("/theme").unwrap_or_default().trim();
                if requested.is_empty() {
                    let names = crate::theme::names(cwd);
                    app.items.push(ChatItem::Info(format!(
                        "theme: {}; available: {}",
                        app.theme.name,
                        names.join(", ")
                    )));
                    return;
                }
                let theme = crate::theme::load(cwd, requested);
                if !crate::theme::names(cwd).iter().any(|n| n == requested) {
                    app.items
                        .push(ChatItem::Error(format!("unknown theme `{requested}`")));
                    return;
                }
                config.theme = theme.clone();
                app.theme = theme;
                app.invalidate_render_cache();
                app.items
                    .push(ChatItem::Info(format!("theme set to {requested}")));
                return;
            }
            if raw == "/trust" || raw.starts_with("/trust ") {
                app.input.clear();
                refresh_suggestions(app, config);
                let requested = raw.strip_prefix("/trust").unwrap_or_default().trim();
                let mut store = crate::trust::TrustStore::load().unwrap_or_default();
                if requested == "show" {
                    let decision = store.decision(cwd);
                    app.items.push(ChatItem::Info(format!(
                        "project trust: {}\ndefault: {}",
                        match decision {
                            Some(true) => "trusted".to_string(),
                            Some(false) => "declined".to_string(),
                            None => "no saved decision".to_string(),
                        },
                        config.default_project_trust.label()
                    )));
                    return;
                }
                let trusted = !matches!(requested, "off" | "no" | "deny" | "never");
                store.set(cwd, trusted);
                match store.save() {
                    Ok(()) => {
                        config.trusted = trusted;
                        config.reload_ecosystem(cwd);
                        app.items.push(ChatItem::Info(if trusted {
                            "saved trust decision: trusted".to_string()
                        } else {
                            "saved trust decision: declined".to_string()
                        }));
                    }
                    Err(err) => app.items.push(ChatItem::Error(format!("{err:#}"))),
                }
                return;
            }
            if raw == "/reload" {
                app.input.clear();
                match Config::load(
                    cwd,
                    None,
                    None,
                    None,
                    Some(config.mode.label().to_string()),
                    Some(config.reasoning.label().to_string()),
                ) {
                    Ok(mut reloaded) => {
                        reloaded.load_context_files = config.load_context_files;
                        if !config.load_context_files {
                            reloaded.ecosystem = crate::ecosystem::load_with(cwd, false);
                        }
                        *config = reloaded;
                        refresh_suggestions(app, config);
                        app.items.push(ChatItem::Info(
                            "reloaded config, commands and skills".into(),
                        ));
                    }
                    Err(err) => app.items.push(ChatItem::Error(format!("{err:#}"))),
                }
                return;
            }
            if raw == "/hotkeys" {
                app.input.clear();
                refresh_suggestions(app, config);
                app.items.push(ChatItem::Info(hotkeys_text()));
                return;
            }
            if raw == "/export" || raw.starts_with("/export ") {
                app.input.clear();
                refresh_suggestions(app, config);
                let target = raw.strip_prefix("/export").unwrap_or_default().trim();
                match export_session(session, &app.history, cwd, target) {
                    Ok(path) => app
                        .items
                        .push(ChatItem::Info(format!("exported to {}", path.display()))),
                    Err(err) => app.items.push(ChatItem::Error(format!("{err:#}"))),
                }
                return;
            }
            let init = raw == "/init";
            app.input.clear();
            refresh_suggestions(app, config);
            if config.api_key.trim().is_empty() {
                app.items.push(ChatItem::Error(
                    "no provider connected — run /connect to add an API key".to_string(),
                ));
                return;
            }
            let resolved = if init {
                None
            } else {
                config.resolve_command(&raw)
            };
            let prompt = if init {
                init_prompt()
            } else {
                resolved
                    .as_ref()
                    .map(|command| command.prompt.clone())
                    .unwrap_or_else(|| raw.clone())
            };
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
                follow_ups: app.follow_ups.clone(),
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
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let half = (app.view_height / 2).max(1);
            app.scroll_up(half);
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let half = (app.view_height / 2).max(1);
            app.scroll_down(half);
        }
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_up(1),
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_down(1),
        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_to_top(),
        KeyCode::Char(ch) => {
            app.input.push(ch);
            app.history_index = None;
            app.auto_scroll = true;
            refresh_suggestions(app, config);
        }
        KeyCode::Backspace => {
            app.input.pop();
            app.history_index = None;
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
            } else if !app.input_history.is_empty() {
                app.history_prev();
                refresh_suggestions(app, config);
            }
        }
        KeyCode::Down => {
            if !app.suggestions.is_empty() {
                let last = app.suggestions.len().saturating_sub(1);
                app.suggestion_index = (app.suggestion_index + 1).min(last);
            } else if app.history_index.is_some() {
                app.history_next();
                refresh_suggestions(app, config);
            }
        }
        KeyCode::PageUp => app.scroll_up(app.page_step()),
        KeyCode::PageDown => app.scroll_down(app.page_step()),
        KeyCode::Home => app.scroll_to_top(),
        KeyCode::End => app.scroll_to_bottom(),
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

/// The instruction sent to the agent by the `/init` command.
fn init_prompt() -> String {
    "Initialize this project's AGENTS.md file.\n\n\
     Analyze the repository to understand its structure, build/lint/test commands and \
     conventions. Then create AGENTS.md at the project root, or update it in place if it \
     already exists — never blindly replace existing content.\n\n\
     Read key files first (README, manifests, CI config, and any existing AGENTS.md or \
     CLAUDE.md), then cover the things future agent sessions need most:\n\
     - build, lint and test commands\n\
     - command order and focused verification steps when they matter\n\
     - architecture and repo structure that is not obvious from filenames alone\n\
     - project-specific conventions, setup quirks and operational gotchas\n\
     - references to existing instruction sources such as Cursor or Copilot rules\n\n\
     Keep it concise and specific to this project. When done, report what you wrote."
        .to_string()
}

/// The model's context window, used to show a Pi-style context percentage.
/// Configurable with `OXIDE_CONTEXT_LIMIT`; falls back to a 128k default.
fn context_limit(config: &Config) -> u64 {
    std::env::var("OXIDE_CONTEXT_LIMIT")
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .filter(|value| *value > 0)
        .unwrap_or_else(|| (config.max_tokens as u64).max(128_000))
}

fn help_text(config: &Config) -> String {
    let mut lines = vec![
        "built-in commands:".to_string(),
        "  /help                 show this help".to_string(),
        "  /hotkeys              show the keyboard shortcuts".to_string(),
        "  /new                  start a new session".to_string(),
        "  /session              show session file, id, name, and stats".to_string(),
        "  /name <name>          name the current session".to_string(),
        "  /model [id]           show or switch the active model".to_string(),
        "  /thinking [level]     show or set the thinking level".to_string(),
        "  /export [file]        export the session to HTML".to_string(),
        "  /tree                 list user messages for branching".to_string(),
        "  /fork [n]             branch a new session from message n".to_string(),
        "  /clone                duplicate the current session".to_string(),
        "  /reload               reload config, commands, and skills".to_string(),
        "  /trust [show|off]     save or show the project trust decision".to_string(),
        "  /theme [name]         show or switch the color theme".to_string(),
        "  /init                 create or update AGENTS.md for this project".to_string(),
        "  /connect [provider]   connect a provider and save its API key".to_string(),
        "  /login                 alias of /connect (Pi-style)".to_string(),
        "  /logout [provider]     remove stored credentials".to_string(),
        "  /models [filter]      list and switch the active model".to_string(),
        "  /undo, /redo          revert or reapply the agent's file changes".to_string(),
        "  /compact              summarize the conversation to free context".to_string(),
        "keys: Enter send/guide · Shift+Enter newline · Alt+Enter follow-up while busy · Shift+Tab mode · Ctrl+R reasoning · Ctrl+O tool details · Ctrl+V image · ↑/↓ history · PgUp/PgDn/wheel scroll · Ctrl+U/D half page · Ctrl+C quit"
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

/// Removes a stored credential, returning whether one existed.
fn logout_provider(provider: &str) -> Result<bool> {
    let mut store = crate::auth::AuthStore::load()?;
    if store.remove(provider) {
        store.save()?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Keyboard shortcuts shown by `/hotkeys`.
fn hotkeys_text() -> String {
    [
        "keyboard shortcuts:",
        "  Enter                 send (queues steering while busy)",
        "  Shift+Enter           insert a newline",
        "  Alt+Enter             queue a follow-up while busy",
        "  Esc                   clear input; with empty input, quit",
        "  Shift+Tab             cycle permission mode",
        "  Ctrl+R                cycle reasoning/thinking level",
        "  Ctrl+O                toggle tool output",
        "  Ctrl+V                attach a clipboard image",
        "  Tab                   complete the selected slash command",
        "  Ctrl+Y / Ctrl+E       scroll one line",
        "  Ctrl+U / Ctrl+D       scroll half a page",
        "  PgUp / PgDn / wheel   scroll the transcript",
        "  Ctrl+G / Home         scroll to the top",
        "  End                   return to the latest message",
        "  Up / Down             input history",
        "  Ctrl+C                quit",
    ]
    .join("\n")
}

/// 1-based positions and text of user messages, for `/tree` and `/fork`.
fn user_messages(history: &[Message]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut position = 0;
    for message in history {
        if message.role == "user" {
            position += 1;
            out.push((position, message.display().unwrap_or_default()));
        }
    }
    out
}

/// Finds the history index just before the `index`-th user message and the
/// message text, so `/fork` can branch there.
fn fork_point(history: &[Message], index: usize) -> Option<(usize, String)> {
    if index == 0 {
        return None;
    }
    let mut seen = 0;
    for (position, message) in history.iter().enumerate() {
        if message.role == "user" {
            seen += 1;
            if seen == index {
                return Some((position, message.display().unwrap_or_default()));
            }
        }
    }
    None
}

/// Human-readable session summary shown by `/session`.
fn session_info(session: &Option<SessionLog>, history: &[Message]) -> String {
    match session {
        Some(log) => {
            let name = log.name().unwrap_or_else(|| "(unnamed)".to_string());
            let user = history.iter().filter(|m| m.role == "user").count();
            let assistant = history.iter().filter(|m| m.role == "assistant").count();
            let tools = history.iter().filter(|m| m.role == "tool").count();
            format!(
                "session {}\nname: {}\nfile: {}\nmessages: {} user · {} assistant · {} tool",
                log.id(),
                name,
                log.cwd(),
                user,
                assistant,
                tools
            )
        }
        None => "no session yet (send a message to create one)".to_string(),
    }
}

/// Writes the session transcript to an HTML file. Returns the written path.
fn export_session(
    session: &Option<SessionLog>,
    history: &[Message],
    cwd: &Path,
    target: &str,
) -> Result<PathBuf> {
    let path = if target.is_empty() {
        let id = session
            .as_ref()
            .map(|log| log.id().to_string())
            .unwrap_or_else(|| "session".to_string());
        cwd.join(format!("oxide-{id}.html"))
    } else {
        let path = PathBuf::from(target);
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    };
    let mut body = String::from(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<title>oxide session</title>\n\
         <style>body{font-family:ui-monospace,monospace;max-width:48rem;margin:2rem auto;padding:0 1rem}\
         .user{color:#0a7}.assistant{color:#333}.tool{color:#888}pre{white-space:pre-wrap}</style>\n",
    );
    for message in history {
        let class = match message.role.as_str() {
            "user" => "user",
            "assistant" => "assistant",
            _ => "tool",
        };
        body.push_str(&format!(
            "<pre class=\"{class}\">{}: {}</pre>\n",
            html_escape(&message.role),
            html_escape(&message.display().unwrap_or_default())
        ));
    }
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

fn html_escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn resolve_provider_choice(value: &str) -> String {
    match value.trim() {
        "1" => "openai".to_string(),
        "2" => "deepseek".to_string(),
        "3" => "anthropic".to_string(),
        "4" => "portkey".to_string(),
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
            name: "hotkeys".to_string(),
            description: "show keyboard shortcuts".to_string(),
        },
        CommandHint {
            name: "new".to_string(),
            description: "start a new session".to_string(),
        },
        CommandHint {
            name: "session".to_string(),
            description: "show session info".to_string(),
        },
        CommandHint {
            name: "tree".to_string(),
            description: "list user messages for branching".to_string(),
        },
        CommandHint {
            name: "fork".to_string(),
            description: "branch a new session from a message".to_string(),
        },
        CommandHint {
            name: "clone".to_string(),
            description: "duplicate the session".to_string(),
        },
        CommandHint {
            name: "name".to_string(),
            description: "name the session".to_string(),
        },
        CommandHint {
            name: "model".to_string(),
            description: "switch the model".to_string(),
        },
        CommandHint {
            name: "thinking".to_string(),
            description: "set the thinking level".to_string(),
        },
        CommandHint {
            name: "export".to_string(),
            description: "export the session to HTML".to_string(),
        },
        CommandHint {
            name: "theme".to_string(),
            description: "switch the color theme".to_string(),
        },
        CommandHint {
            name: "trust".to_string(),
            description: "save project trust decision".to_string(),
        },
        CommandHint {
            name: "reload".to_string(),
            description: "reload config and skills".to_string(),
        },
        CommandHint {
            name: "init".to_string(),
            description: "create or update AGENTS.md".to_string(),
        },
        CommandHint {
            name: "login".to_string(),
            description: "connect a provider (alias of /connect)".to_string(),
        },
        CommandHint {
            name: "logout".to_string(),
            description: "remove stored credentials".to_string(),
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
    if app.connect.is_some() || app.models.is_some() || app.trust.is_some() || app.busy {
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
    for template in &config.ecosystem.prompt_templates {
        if config.ecosystem.command(&template.name).is_some() {
            continue;
        }
        let mut description = template.description.clone().unwrap_or_default();
        if let Some(hint) = &template.argument_hint {
            if !description.is_empty() {
                description = format!("{hint} — {description}");
            } else {
                description = hint.clone();
            }
        }
        hints.push(CommandHint {
            name: template.name.clone(),
            description,
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

/// Scrolls the conversation with the mouse wheel without stealing keys from
/// the input box.
fn handle_mouse(mouse: MouseEvent, app: &mut App) {
    match mouse.kind {
        MouseEventKind::ScrollUp => app.scroll_up(3),
        MouseEventKind::ScrollDown => app.scroll_down(3),
        _ => {}
    }
}

fn handle_trust_key(key: KeyEvent, app: &mut App, config: &mut Config, cwd: &Path) {
    let Some(mut state) = app.trust.take() else {
        return;
    };
    let mut keep = true;
    match key.code {
        KeyCode::Left | KeyCode::Char('h') => state.selected = 0,
        KeyCode::Right | KeyCode::Char('l') | KeyCode::Tab => state.selected = 1,
        KeyCode::Char('t') | KeyCode::Char('T') | KeyCode::Char('y') => state.selected = 0,
        KeyCode::Char('n') | KeyCode::Char('N') => state.selected = 1,
        KeyCode::Esc => {
            // Escape declines for this session without saving a decision.
            config.trusted = false;
            config.reload_ecosystem(cwd);
            app.items
                .push(ChatItem::Info("project resources not trusted".to_string()));
            return;
        }
        KeyCode::Enter => {
            let trusted = state.selected == 0;
            let path = crate::trust::TrustStore::path();
            let mut store = crate::trust::TrustStore::load().unwrap_or_default();
            store.set(cwd, trusted);
            if let Err(err) = store.save_to(&path) {
                app.items.push(ChatItem::Error(format!("{err:#}")));
            }
            config.trusted = trusted;
            config.reload_ecosystem(cwd);
            app.items.push(ChatItem::Info(if trusted {
                format!("trusted {} — project resources loaded", cwd.display())
            } else {
                format!("declined trust for {}", cwd.display())
            }));
            keep = false;
        }
        _ => {}
    }
    if keep && app.trust.is_none() {
        app.trust = Some(state);
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
                    let provider = if value.is_empty() {
                        crate::auth::KNOWN_PROVIDERS[state.selected]
                            .name
                            .to_string()
                    } else {
                        resolve_provider_choice(&value)
                    };
                    state.step = ConnectStep::Key { provider };
                    state.input.clear();
                    state.error = None;
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
                                    "logged in to {} · model {}",
                                    crate::auth::provider_label(&name),
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
            if state.input.is_empty() && matches!(state.step, ConnectStep::Key { .. }) {
                state.step = ConnectStep::Provider;
                state.error = None;
            } else {
                state.input.pop();
                state.error = None;
            }
        }
        KeyCode::Up if matches!(state.step, ConnectStep::Provider) && state.input.is_empty() => {
            state.selected = state.selected.saturating_sub(1);
        }
        KeyCode::Down if matches!(state.step, ConnectStep::Provider) && state.input.is_empty() => {
            state.selected = (state.selected + 1).min(crate::auth::KNOWN_PROVIDERS.len() - 1);
        }
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            state.input.push(c);
            state.error = None;
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
        AgentEvent::Thought { millis } => {
            app.auto_scroll = true;
            app.items.push(ChatItem::Thought(millis));
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
                let index = app.items.len() - 1;
                if let Some(ChatItem::ToolProgress { output, .. }) = app.items.last_mut() {
                    append_tool_progress(output, &chunk);
                }
                app.mark_render_dirty(index);
            } else {
                app.items.push(ChatItem::ToolProgress {
                    name,
                    output: trailing_text(&chunk, MAX_TOOL_PROGRESS_BYTES),
                });
            }
        }
        AgentEvent::ToolResult {
            name,
            args,
            output,
            diff,
        } => {
            app.auto_scroll = true;
            app.resolve_tool(name, args, output, diff);
            app.status = "thinking...".to_string();
        }
        AgentEvent::Usage { input, output } => {
            app.tokens_in = app.tokens_in.saturating_add(input);
            app.tokens_out = app.tokens_out.saturating_add(output);
            app.context_used = input;
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

fn append_tool_progress(output: &mut String, chunk: &str) {
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str(chunk);
    if output.len() > MAX_TOOL_PROGRESS_BYTES {
        const MARKER: &str = "…\n";
        *output = format!(
            "{MARKER}{}",
            trailing_text(output, MAX_TOOL_PROGRESS_BYTES - MARKER.len())
        );
    }
}

fn trailing_text(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut start = text.len() - max_bytes;
    while !text.is_char_boundary(start) {
        start += 1;
    }
    text[start..].to_string()
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

    #[test]
    fn tool_progress_keeps_a_bounded_utf8_tail() {
        let mut output = "old".repeat(MAX_TOOL_PROGRESS_BYTES);
        append_tool_progress(&mut output, "latest 🚀");
        assert!(output.len() <= MAX_TOOL_PROGRESS_BYTES);
        assert!(output.starts_with("…\n"));
        assert!(output.ends_with("latest 🚀"));
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn user_messages_and_fork_points_are_one_based() {
        let history = vec![
            Message::user("first"),
            Message::assistant("reply", vec![]),
            Message::user("second"),
        ];
        let messages = user_messages(&history);
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].0, 1);
        assert_eq!(messages[1].0, 2);
        assert_eq!(messages[1].1, "second");

        assert_eq!(fork_point(&history, 1).unwrap().0, 0);
        assert_eq!(fork_point(&history, 2).unwrap().0, 2);
        assert!(fork_point(&history, 3).is_none());
        assert!(fork_point(&history, 0).is_none());
    }

    #[test]
    fn provider_choice_maps_numbers_and_aliases() {
        assert_eq!(resolve_provider_choice("1"), "openai");
        assert_eq!(resolve_provider_choice("2"), "deepseek");
        assert_eq!(resolve_provider_choice("3"), "anthropic");
        assert_eq!(resolve_provider_choice("4"), "portkey");
        assert_eq!(resolve_provider_choice("DeepSeek"), "deepseek");
        assert_eq!(resolve_provider_choice("Port-Key"), "portkey");
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
    fn connect_enter_accepts_highlighted_provider() {
        let mut app = test_app();
        let mut config = Config::default();
        app.connect = Some(ConnectState::new());

        handle_connect_key(key(KeyCode::Enter), &mut app, &mut config);

        assert!(matches!(
            &app.connect.as_ref().unwrap().step,
            ConnectStep::Key { provider } if provider == "openai"
        ));
    }

    #[test]
    fn connect_arrows_select_provider_and_backspace_returns() {
        let mut app = test_app();
        let mut config = Config::default();
        app.connect = Some(ConnectState::new());

        handle_connect_key(key(KeyCode::Down), &mut app, &mut config);
        handle_connect_key(key(KeyCode::Enter), &mut app, &mut config);
        assert!(matches!(
            &app.connect.as_ref().unwrap().step,
            ConnectStep::Key { provider } if provider == "deepseek"
        ));

        handle_connect_key(key(KeyCode::Backspace), &mut app, &mut config);
        assert!(matches!(
            app.connect.as_ref().unwrap().step,
            ConnectStep::Provider
        ));
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
    fn ctrl_c_is_a_global_quit_shortcut() {
        assert!(is_quit_shortcut(&KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        )));
        assert!(!is_quit_shortcut(&key(KeyCode::Char('c'))));
        assert!(!is_quit_shortcut(&key(KeyCode::Char('d'))));
    }

    #[test]
    fn shift_enter_is_a_newline_shortcut() {
        assert!(is_newline_shortcut(&KeyEvent::new(
            KeyCode::Enter,
            KeyModifiers::SHIFT
        )));
        assert!(!is_newline_shortcut(&key(KeyCode::Enter)));
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

        app.input = "/models".to_string();
        refresh_suggestions(&mut app, &config);
        assert!(app.suggestions.iter().any(|hint| hint.name == "models"));

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

        state = ModelsState::ready(vec!["claude-sonnet-5".to_string()]);
        state.filter = "Claude Sonnet 5".to_string();
        assert_eq!(state.selected_model(), Some("claude-sonnet-5"));

        state.filter = "missing".to_string();
        assert!(state.selected_model().is_none());
    }

    #[test]
    fn up_recalls_submitted_input() {
        let mut app = test_app();
        app.remember_input("first");
        app.remember_input("second");

        app.history_prev();
        assert_eq!(app.input, "second");
        app.history_prev();
        assert_eq!(app.input, "first");
        app.history_prev();
        assert_eq!(app.input, "first");

        app.history_next();
        assert_eq!(app.input, "second");
        app.history_next();
        assert!(app.input.is_empty());
    }

    #[test]
    fn scroll_helpers_adjust_offset_and_follow_state() {
        let mut app = test_app();
        app.scroll = 30;

        app.scroll_up(5);
        assert_eq!(app.scroll, 25);
        assert!(!app.auto_scroll);

        app.scroll_down(10);
        assert_eq!(app.scroll, 35);
        assert!(!app.auto_scroll);

        app.scroll_to_top();
        assert_eq!(app.scroll, 0);
        assert!(!app.auto_scroll);

        app.scroll_to_bottom();
        assert!(app.auto_scroll);
    }

    #[test]
    fn page_step_uses_viewport_height() {
        let mut app = test_app();
        assert_eq!(app.page_step(), 1);
        app.view_height = 24;
        assert_eq!(app.page_step(), 24);
    }

    #[test]
    fn mouse_wheel_scrolls_without_touching_history() {
        let mut app = test_app();
        app.remember_input("prompt");
        app.scroll = 30;

        handle_mouse(
            MouseEvent {
                kind: MouseEventKind::ScrollUp,
                column: 0,
                row: 0,
                modifiers: KeyModifiers::NONE,
            },
            &mut app,
        );
        assert_eq!(app.scroll, 27);
        assert_eq!(app.input_history, vec!["prompt".to_string()]);
        assert!(app.input.is_empty());
    }

    #[test]
    fn remember_input_dedupes_consecutive_and_ignores_blank() {
        let mut app = test_app();
        app.remember_input("same");
        app.remember_input("same");
        app.remember_input("   ");
        assert_eq!(app.input_history, vec!["same".to_string()]);
    }

    #[test]
    fn init_is_a_builtin_command() {
        let config = Config::default();
        let mut app = test_app();
        app.input = "/ini".to_string();
        refresh_suggestions(&mut app, &config);
        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0].name, "init");
        assert!(help_text(&config).contains("/init"));
        assert!(init_prompt().contains("AGENTS.md"));
    }
}
