pub mod app;
pub mod ui;

use crate::agent::{self, AgentEvent, ApprovalRequest, Approver, Runtime};
use crate::config::Config;
use crate::ecosystem::AgentMode;
use crate::llm::Message;
use crate::lsp::LspManager;
use crate::mcp::McpRegistry;
use crate::media;
use crate::plugin::PluginHost;
use crate::session::SessionLog;
use crate::snapshots::Snapshots;
use crate::tui::app::{App, ChatItem, ConnectState, ConnectStep};
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
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

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
        "Ask me to build, refactor, debug or explain code. Ctrl+C to quit.".to_string(),
    ));
    app.items.push(ChatItem::Info(format!(
        "mode: {} (Shift+Tab cycles build → auto-edit → plan)",
        app.mode.label()
    )));
    app.items.push(ChatItem::Info(format!(
        "reasoning: {} (Ctrl+R cycles auto → off → low → medium → high; auto resolves to {} for {})",
        app.reasoning.label(),
        config.effective_reasoning().label(),
        config.model
    )));
    app.items.push(ChatItem::Info(format!(
        "ecosystem: {}",
        config.ecosystem.summary()
    )));
    app.items.push(ChatItem::Info(format!(
        "mcp: {} server(s), {} tool(s) connected",
        mcp.server_count(),
        mcp.tool_count()
    )));
    app.items.push(ChatItem::Info(format!(
        "plugins: {} loaded{}",
        plugins.plugin_count(),
        if plugins.is_active() {
            ""
        } else {
            " (runtime unavailable)"
        }
    )));
    app.items.push(ChatItem::Info(format!(
        "memory: {} stored entr{}",
        config.memory.len(),
        if config.memory.len() == 1 { "y" } else { "ies" }
    )));
    app.items.push(ChatItem::Info(
        "multimodal: attach images/PDFs with @path or Ctrl+V; the agent can read image/PDF files."
            .to_string(),
    ));
    app.items.push(ChatItem::Info(
        "snapshots: /undo and /redo revert the agent's file changes".to_string(),
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
                        snapshots.as_ref(), &lsp, &mut session, &approve,
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
            approval = approval_rx.recv() => {
                if let Some(request) = approval {
                    app.status = format!("approve `{}`? y/n — {}", request.tool, request.detail);
                    app.pending_approval = Some(request);
                }
            }
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
) {
    if app.connect.is_some() {
        handle_connect_key(key, app, config);
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
        KeyCode::Esc => app.should_quit = true,
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
            if raw == "/undo" || raw == "/redo" {
                app.input.clear();
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
                if app.busy {
                    return;
                }
                let config = config.clone();
                let history = app.history.clone();
                let fallback = history.clone();
                let (tx, new_rx) = unbounded_channel();
                *rx = Some(new_rx);
                app.busy = true;
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
            app.input.clear();
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
                    app.status =
                        "no clipboard image (install pngpaste, wl-paste or xclip)".to_string();
                }
            }
        }
        KeyCode::Char(ch) => {
            app.input.push(ch);
            app.auto_scroll = true;
        }
        KeyCode::Backspace => {
            app.input.pop();
        }
        KeyCode::Up => {
            app.scroll = app.scroll.saturating_sub(1);
            app.auto_scroll = false;
        }
        KeyCode::Down => {
            app.scroll = app.scroll.saturating_add(1);
            app.auto_scroll = false;
        }
        _ => {}
    }
}

fn resolve_provider_choice(value: &str) -> String {
    match value.trim().to_ascii_lowercase().as_str() {
        "1" | "openai" | "gpt" | "gpt-4" | "gpt-4o" => "openai".to_string(),
        "2" | "deepseek" => "deepseek".to_string(),
        "3" | "anthropic" => "anthropic".to_string(),
        other => other.to_string(),
    }
}

fn handle_paste(text: String, app: &mut App) {
    let text: String = text
        .chars()
        .filter(|ch| *ch != '\r' && *ch != '\n')
        .collect();
    if let Some(state) = app.connect.as_mut() {
        state.input.push_str(&text);
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
}
