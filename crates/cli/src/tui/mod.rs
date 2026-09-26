pub mod app;
mod markdown;
pub mod ui;

use crate::agent::{self, AgentEvent, Runtime};
use crate::approval::{ApprovalBroker, Decision};
use crate::config::{Config, Reasoning};
use crate::ecosystem::AgentMode;
use crate::llm::{LlmClient, Message};
use crate::lsp::LspManager;
use crate::mcp::{McpRegistry, McpStatus};
use crate::media;
use crate::plugin::PluginHost;
use crate::session::SessionLog;
use crate::snapshots::Snapshots;
use crate::tui::app::{
    App, ChatItem, CommandHint, ConnectField, ConnectState, ConnectStep, ListRow, MarketplacePane,
    MarketplacesState, ModelChoice, ModelsState, PendingApproval, Selection, SessionsState,
    SubagentState, Tone, TrustState, UsageField, UsageState,
};
use anyhow::{Context, Result};
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream, KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use futures::StreamExt;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

const MAX_TOOL_PROGRESS_BYTES: usize = 6_000;

/// The result of an async marketplace action (add or install), delivered back
/// to the event loop so the overlay can refresh and show the outcome.
enum MarketplaceOutcome {
    Message(String),
    Error(String),
}

pub async fn run(
    config: Config,
    cwd: PathBuf,
    session: Option<SessionLog>,
    open_sessions_picker: bool,
    theme_name: String,
) -> Result<()> {
    // Refuse before touching the terminal: with stdin or stdout redirected (a
    // test, a pipe, a CI runner) raw mode has no console to drive and the event
    // loop would block on input that never arrives.
    if !std::io::stdin().is_terminal() || !std::io::stdout().is_terminal() {
        anyhow::bail!(
            "the interactive TUI requires a terminal; use -p or --mode rpc when input or output is redirected"
        );
    }
    let mcp = Arc::new(McpRegistry::new(&config.ecosystem.mcp));
    let plugins = Arc::new(PluginHost::spawn(&config.ecosystem.hooks, &cwd).await);
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
        open_sessions_picker,
        theme_name,
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
    open_sessions_picker: bool,
    theme_name: String,
) -> Result<()> {
    let mut app = App::new(
        config.model.clone(),
        cwd.display().to_string(),
        config.reasoning,
    );
    app.context_limit = context_limit(&config);
    app.provider = config.provider.clone();
    app.auto_compact = config.compaction.enabled;
    app.available_providers = available_providers(&config);
    app.session_name = session.as_ref().and_then(|log| log.name());
    if let Some(log) = &session {
        let totals = log.usage_totals();
        app.tokens_in = totals.input;
        app.tokens_out = totals.output;
        app.tokens_cache_read = totals.cache_read;
        app.tokens_cache_write = totals.cache_write;
        app.cost = totals.cost;
        app.cache_hit_rate = totals.cache_hit_rate;
    }
    app.show_thinking = config.supports_reasoning();
    app.show_thinking_blocks = !crate::config::load_hide_thinking_block();
    app.theme = crate::theme::load(&cwd, &theme_name);
    app.usage_settings = crate::portkey_usage::UsageSettings::load().unwrap_or_default();
    if app.usage_settings.enabled && app.usage_settings.available(&config) {
        app.usage = Some(crate::portkey_usage::UsageBar::new(&app.usage_settings));
    }

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

    let mut banner = vec![
        "Code, research, automate, and more.".to_string(),
        config.ecosystem.summary(),
        format!(
            "mcp: {} configured · {} loaded · {} tools · hooks: {}{} · memory: {} entries",
            mcp.configured_count(),
            mcp.server_count(),
            mcp.tool_count(),
            plugins.hook_count(),
            if plugins.is_active() {
                ""
            } else {
                " (unavailable)"
            },
            config.memory.len(),
        ),
    ];
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
        banner.push(format!("context files: {}", files.join(", ")));
    }
    app.items.push(ChatItem::Banner { info: banner });
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
                let count = messages.len();
                restore_history(
                    &mut app,
                    messages,
                    format!(
                        "resumed session {} ({} message{})",
                        log.id(),
                        count,
                        if count == 1 { "" } else { "s" }
                    ),
                );
            }
            Err(err) => app.items.push(ChatItem::Error(format!("session: {err:#}"))),
        }
    }

    if open_sessions_picker {
        match SessionLog::list(&cwd) {
            Ok(sessions) => {
                let count = sessions.len();
                app.sessions = Some(SessionsState::ready(sessions));
                app.status = format!("{count} session(s) — pick one");
            }
            Err(err) => app.items.push(ChatItem::Error(format!("session: {err:#}"))),
        }
    }

    let mut reader = EventStream::new();
    let mut rx: Option<UnboundedReceiver<AgentEvent>> = None;
    let (models_tx, mut models_rx) = unbounded_channel::<ModelCatalogs>();
    let (mcps_tx, mut mcps_rx) = unbounded_channel::<Vec<(String, String, McpStatus)>>();
    let (plugins_tx, mut plugins_rx) = unbounded_channel::<String>();
    let (listings_tx, mut listings_rx) = unbounded_channel::<Result<ChatItem, String>>();
    let (marketplaces_tx, mut marketplaces_rx) = unbounded_channel::<MarketplaceOutcome>();
    let (usage_tx, mut usage_rx) =
        unbounded_channel::<Result<crate::portkey_usage::Snapshot, String>>();
    if config.model_catalog.is_empty() {
        let warmups: Vec<Config> = model_providers(&config)
            .into_iter()
            .map(|(_, provider_config)| provider_config)
            .collect();
        if !warmups.is_empty() {
            tokio::spawn(async move {
                for provider_config in warmups {
                    let _ = LlmClient::new(provider_config).list_models().await;
                }
            });
        }
    }
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(500));
    let mut progress_tick = tokio::time::interval(std::time::Duration::from_millis(120));
    let mut branch_tick = tokio::time::interval(std::time::Duration::from_secs(2));
    let mut usage_tick = tokio::time::interval(std::time::Duration::from_secs(60));
    let mut usage_inflight = false;

    // Every front-end asks through the same broker, so a rule saved with
    // "always allow" is remembered in the shared `approvals.json` and a denial's
    // text reaches the agent as guidance. The question arrives on the run's own
    // event stream, ordered after the tool call it belongs to.
    let approvals = ApprovalBroker::new();

    loop {
        let terminal_area = terminal.draw(|frame| ui::draw(frame, &mut app))?.area;

        let mut got_agent_event = false;
        tokio::select! {
            maybe_event = reader.next() => {
                match maybe_event {
                    Some(Ok(Event::Key(key))) => handle_key(
                        key, &mut app, &mut config, &cwd, &mut rx, &mcp, &plugins,
                        snapshots.as_ref(), &lsp, &mut session, &approvals, &models_tx, &mcps_tx,
                        &plugins_tx, &listings_tx, &marketplaces_tx, &usage_tx,
                    ),
                    Some(Ok(Event::Paste(text))) => handle_paste(text, &mut app),
                    Some(Ok(Event::Mouse(mouse))) => handle_mouse(mouse, &mut app, terminal_area),
                    _ => {}
                }
            }
            agent_event = recv_opt(&mut rx) => {
                if let Some(event) = agent_event {
                    let finished = matches!(event, AgentEvent::Finished(_));
                    let notify = finished && app.notify_on_finish && config.notify.on_complete;
                    handle_agent_event(event, &mut app);
                    if finished {
                        app.notify_on_finish = false;
                    }
                    if notify {
                        crate::notify::send("Oxide", &app.completion_summary(), config.notify.sound);
                    }
                    if finished && plugins.is_active() {
                        app.extension_statuses = plugins.statuses().await;
                    }
                    if finished && app.usage.is_some() && !usage_inflight {
                        usage_inflight = spawn_usage_refresh(&config, &app.usage_settings, &usage_tx);
                    }
                    got_agent_event = true;
                }
            }
            result = models_rx.recv() => {
                if let Some(result) = result {
                    handle_model_result(result, &mut app);
                }
            }
            statuses = mcps_rx.recv() => {
                if let Some(statuses) = statuses {
                    app.clear_progress();
                    app.items.push(mcp_listing(&statuses));
                    app.auto_scroll = true;
                    app.status = "ready".to_string();
                }
            }
            listing = listings_rx.recv() => {
                if let Some(result) = listing {
                    app.clear_progress();
                    match result {
                        Ok(item) => {
                            app.items.push(item);
                            app.auto_scroll = true;
                        }
                        Err(err) => app.items.push(ChatItem::Error(format!("plugins: {err}"))),
                    }
                    app.status = "ready".to_string();
                }
            }
            plugin_result = plugins_rx.recv() => {
                if let Some(text) = plugin_result {
                    app.items.push(ChatItem::Info(text));
                    app.auto_scroll = true;
                    app.status = "ready".to_string();
                }
            }
            marketplace_result = marketplaces_rx.recv() => {
                if let Some(outcome) = marketplace_result {
                    handle_marketplace_outcome(outcome, &mut app);
                }
            }
            usage_result = usage_rx.recv() => {
                if let Some(result) = usage_result {
                    usage_inflight = false;
                    if let Some(bar) = app.usage.as_mut() {
                        bar.apply(result);
                    }
                }
            }
            _ = tick.tick(), if app.busy => {
                app.mark_running_tool_dirty();
            }
            _ = progress_tick.tick(), if app.has_progress() => {
                app.mark_progress_dirty();
            }
            _ = branch_tick.tick(), if !app.busy => {
                app.refresh_git_branch();
            }
            _ = usage_tick.tick(), if app.usage.is_some() && !usage_inflight => {
                usage_inflight = spawn_usage_refresh(&config, &app.usage_settings, &usage_tx);
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
/// Ctrl+C is a global quit shortcut, honored even while a dialog is open.
fn is_quit_shortcut(key: &KeyEvent) -> bool {
    key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL)
}

fn is_newline_shortcut(key: &KeyEvent) -> bool {
    key.code == KeyCode::Enter && key.modifiers.contains(KeyModifiers::SHIFT)
}

/// Pi's `useWindowsKeybindings`: Windows itself, or Linux under WSL, where the
/// terminal claims `Alt+Up` for its own scrollback.
fn use_windows_keybindings() -> bool {
    use_windows_keybindings_for(
        cfg!(windows),
        cfg!(target_os = "linux"),
        std::env::var_os("WSL_DISTRO_NAME").is_some() || std::env::var_os("WSL_INTEROP").is_some(),
    )
}

fn use_windows_keybindings_for(windows: bool, linux: bool, wsl: bool) -> bool {
    windows || (linux && wsl)
}

/// The dequeue shortcut as Pi spells it: `Alt+Up`, `Alt+Q` where the terminal
/// owns `Alt+Up`, and `Option+Up` on macOS, where `Alt` is the Option key.
fn dequeue_key_label() -> &'static str {
    dequeue_key_label_for(use_windows_keybindings(), cfg!(target_os = "macos"))
}

fn dequeue_key_label_for(windows: bool, macos: bool) -> &'static str {
    match (windows, macos) {
        (true, _) => "Alt+Q",
        (false, true) => "Option+Up",
        (false, false) => "Alt+Up",
    }
}

/// `Alt+Up`, plus `Alt+Q` where the terminal owns `Alt+Up`.
fn is_dequeue_shortcut(key: &KeyEvent) -> bool {
    if !key.modifiers.contains(KeyModifiers::ALT) {
        return false;
    }
    match key.code {
        KeyCode::Up => true,
        KeyCode::Char('q') | KeyCode::Char('Q') => use_windows_keybindings(),
        _ => false,
    }
}

/// Cycles the thinking level and records it on the session. Shared by the
/// `Shift+Tab` (Pi's binding) and `Ctrl+R` (the pre-desktop binding) shortcuts.
fn cycle_reasoning(app: &mut App, config: &mut Config, session: &mut Option<SessionLog>) {
    config.reasoning = config.reasoning.next();
    app.reasoning = config.reasoning;
    if let Some(log) = session.as_ref() {
        let _ = log.append_thinking_level(config.reasoning.label());
    }
    app.show_status(if app.reasoning == Reasoning::Auto {
        "reasoning: auto (provider native)".to_string()
    } else {
        format!("reasoning: {}", app.reasoning.label())
    });
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
    approvals: &Arc<ApprovalBroker>,
    models_tx: &UnboundedSender<ModelCatalogs>,
    mcps_tx: &UnboundedSender<Vec<(String, String, McpStatus)>>,
    plugins_tx: &UnboundedSender<String>,
    listings_tx: &UnboundedSender<Result<ChatItem, String>>,
    marketplaces_tx: &UnboundedSender<MarketplaceOutcome>,
    usage_tx: &UnboundedSender<Result<crate::portkey_usage::Snapshot, String>>,
) {
    // Ctrl+C copies an active mouse selection, otherwise quits. It still quits
    // while a dialog or the trust prompt is open (those never hold a selection).
    if is_quit_shortcut(&key) {
        if !copy_selection(app) {
            app.should_quit = true;
        }
        return;
    }

    if is_dequeue_shortcut(&key) {
        dequeue_messages(app);
        refresh_suggestions(app, config);
        return;
    }

    if app.connect.is_some() {
        handle_connect_key(key, app, config);
        sync_usage_bar(app, config, usage_tx);
        return;
    }

    if app.usage_modal.is_some() {
        handle_usage_key(key, app, config, usage_tx);
        return;
    }

    if app.trust.is_some() {
        handle_trust_key(key, app, config, cwd);
        return;
    }

    if app.models.is_some() {
        handle_models_key(key, app, config);
        sync_usage_bar(app, config, usage_tx);
        return;
    }

    if app.sessions.is_some() {
        handle_sessions_key(key, app, cwd, session);
        return;
    }

    if app.marketplaces.is_some() {
        handle_marketplaces_key(key, app, marketplaces_tx);
        return;
    }

    match key.code {
        KeyCode::Esc => {
            // Esc refuses the tool while one is waiting: the composer is the
            // answer field then, so clearing it would leave the request open.
            if app.pending_approval.is_some() {
                answer_approval(app, approvals, Decision::Deny { message: None });
                refresh_suggestions(app, config);
                return;
            }
            escape_action(app);
            refresh_suggestions(app, config);
        }
        KeyCode::BackTab => {
            cycle_reasoning(app, config, session);
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            cycle_reasoning(app, config, session);
        }
        KeyCode::Char('o') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_tool_output();
            app.show_status(if app.expand_tools {
                "tool output expanded".to_string()
            } else {
                "tool output collapsed".to_string()
            });
        }
        KeyCode::Char('t') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.toggle_thinking_blocks();
            app.show_status(if app.show_thinking_blocks {
                "thinking blocks: visible".to_string()
            } else {
                "thinking blocks: hidden".to_string()
            });
        }
        KeyCode::Enter if is_newline_shortcut(&key) => {
            app.insert_input("\n");
            app.auto_scroll = true;
            refresh_suggestions(app, config);
        }
        KeyCode::Enter => {
            if app.input.trim() == "/exit" {
                app.clear_input();
                refresh_suggestions(app, config);
                app.should_quit = true;
                return;
            }
            let command = app.input.trim().to_string();
            if handle_attach_command(app, &command) {
                app.clear_input();
                refresh_suggestions(app, config);
                return;
            }
            // While a tool waits for an answer, what is typed is the answer:
            // `y`/`a` allow it, `n` refuses it, and any other text refuses it
            // with that text as guidance for the agent. A leading `/` is left
            // to the commands above so `/exit` still quits.
            if app.pending_approval.is_some() && !command.starts_with('/') {
                answer_approval_input(app, approvals, &command);
                refresh_suggestions(app, config);
                return;
            }
            if app.busy {
                let raw = app.input.trim().to_string();
                queue_while_busy(app, &raw, cwd, key.modifiers.contains(KeyModifiers::ALT));
                return;
            }
            let raw = app.input.trim().to_string();
            if raw.is_empty() && app.attachments.is_empty() {
                return;
            }
            if complete_suggestion(app) {
                refresh_suggestions(app, config);
                return;
            }
            app.remember_input(&raw);
            if raw == "/undo" || raw == "/redo" {
                app.clear_input();
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
            if raw == "/copy" || raw == "/copy all" {
                app.clear_input();
                refresh_suggestions(app, config);
                copy_command(app, raw.ends_with(" all"));
                return;
            }
            if raw == "/compact" || raw.starts_with("/compact ") {
                app.clear_input();
                refresh_suggestions(app, config);
                if app.busy {
                    return;
                }
                let instructions = raw
                    .strip_prefix("/compact")
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let config = config.clone();
                let history = app.history.clone();
                let log = session.clone();
                let (tx, new_rx) = unbounded_channel();
                *rx = Some(new_rx);
                app.busy = true;
                app.busy_since = Some(std::time::Instant::now());
                app.status = "compacting...".to_string();
                tokio::spawn(async move {
                    let Some(log) = log else {
                        let fallback = history.clone();
                        match crate::compact::compact_messages(
                            &config,
                            history,
                            Some(&instructions),
                        )
                        .await
                        {
                            Ok(messages) => {
                                let _ = tx.send(AgentEvent::Finished(messages));
                            }
                            Err(err) => {
                                let _ = tx.send(AgentEvent::Error(format!("compact: {err:#}")));
                                let _ = tx.send(AgentEvent::Finished(fallback));
                            }
                        }
                        return;
                    };
                    let budget = config.compaction.resolve(&config.provider, &config.model);
                    let tokens = crate::compact::estimate_tokens(&history) as u64;
                    match crate::compact::generate(
                        &config,
                        &history,
                        &[],
                        budget,
                        tokens,
                        Some(&instructions),
                    )
                    .await
                    {
                        Ok(Some(compaction)) => {
                            let first_kept = log
                                .context_ids()
                                .get(compaction.first_kept)
                                .cloned()
                                .unwrap_or_else(|| compaction.first_kept.to_string());
                            let _ = log.append_compaction(
                                compaction.summary.clone(),
                                first_kept,
                                compaction.tokens_before,
                                Some(compaction.details.clone()),
                                compaction.usage,
                            );
                            let refreshed = log.messages().unwrap_or(history);
                            if let Some(usage) = compaction.usage {
                                let _ = tx.send(AgentEvent::Usage {
                                    input: usage.input,
                                    output: usage.output,
                                    cache_read: usage.cache_read,
                                    cache_write: usage.cache_write,
                                    cost: usage.cost,
                                });
                            }
                            let _ = tx.send(AgentEvent::Compaction {
                                summary: compaction.summary.clone(),
                                summarized: compaction.summarized,
                                tokens_before: compaction.tokens_before,
                                read_files: compaction.details.read_files.clone(),
                                modified_files: compaction.details.modified_files.clone(),
                            });
                            let _ = tx.send(AgentEvent::Finished(refreshed));
                        }
                        Ok(None) => {
                            let _ = tx.send(AgentEvent::Error(
                                "compact: nothing to compact yet".to_string(),
                            ));
                            let _ = tx.send(AgentEvent::Finished(history));
                        }
                        Err(err) => {
                            let _ = tx.send(AgentEvent::Error(format!("compact: {err:#}")));
                            let _ = tx.send(AgentEvent::Finished(history));
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
                app.clear_input();
                refresh_suggestions(app, config);
                let rest = raw
                    .strip_prefix("/connect")
                    .or_else(|| raw.strip_prefix("/login"))
                    .unwrap_or_default();
                let provider = rest.trim().to_string();
                let mut state = ConnectState::new();
                if !provider.is_empty() {
                    let name = resolve_provider_choice(&provider);
                    // A stored credential turns a login into a switch, so
                    // moving between logged-in providers never asks for the
                    // key again.
                    if state.is_connected(&name) {
                        match switch_provider(app, config, &name) {
                            Ok(name) => {
                                sync_usage_bar(app, config, usage_tx);
                                app.items.push(ChatItem::Info(format!(
                                    "logged in to {} · model {}",
                                    crate::auth::provider_label(&name),
                                    config.model
                                )));
                                app.status = "ready".to_string();
                            }
                            Err(err) => app.items.push(ChatItem::Error(format!("{err:#}"))),
                        }
                        return;
                    }
                    state.step = ConnectStep::Key { provider: name };
                }
                app.connect = Some(state);
                app.status = "connecting...".to_string();
                return;
            }
            if raw == "/logout" || raw.starts_with("/logout ") {
                app.clear_input();
                refresh_suggestions(app, config);
                let requested = raw.strip_prefix("/logout").unwrap_or_default().trim();
                let provider = if requested.is_empty() {
                    config.provider.clone()
                } else {
                    crate::auth::canonical_provider(requested)
                };
                if provider.is_empty() {
                    app.items.push(ChatItem::Info(
                        "no provider connected — run /login to add one".to_string(),
                    ));
                    return;
                }
                let active = crate::auth::canonical_provider(&config.provider) == provider;
                match logout_provider(&provider) {
                    Ok(true) => {
                        if active {
                            let message = logout_active_provider(app, config, &provider);
                            app.items.push(ChatItem::Info(message));
                        } else {
                            app.items.push(ChatItem::Info(format!(
                                "logged out of {provider} — still using {}",
                                config.provider
                            )));
                        }
                        app.available_providers = available_providers(config);
                        app.status = "ready".to_string();
                        sync_usage_bar(app, config, usage_tx);
                    }
                    Ok(false) => app.items.push(ChatItem::Info(format!(
                        "no stored credentials for {provider}"
                    ))),
                    Err(err) => app.items.push(ChatItem::Error(format!("{err:#}"))),
                }
                return;
            }
            if raw == "/mcps" {
                app.clear_input();
                refresh_suggestions(app, config);
                app.status = "checking MCP servers...".to_string();
                app.show_progress("Checking MCP servers");
                let mcp = Arc::clone(mcp);
                let tx = mcps_tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(mcp.statuses().await);
                });
                return;
            }
            if raw == "/marketplaces" || raw == "/marketplace" {
                app.clear_input();
                refresh_suggestions(app, config);
                match crate::plugin_registry::marketplace_overview() {
                    Ok(all) => {
                        app.marketplaces = Some(MarketplacesState::ready(all));
                        app.status = "marketplaces".to_string();
                    }
                    Err(err) => app
                        .items
                        .push(ChatItem::Error(format!("marketplaces: {err:#}"))),
                }
                return;
            }
            if raw == "/plugins"
                || raw.starts_with("/plugins ")
                || raw == "/plugin"
                || raw.starts_with("/plugin ")
            {
                app.clear_input();
                refresh_suggestions(app, config);
                let args = raw
                    .strip_prefix("/plugins")
                    .or_else(|| raw.strip_prefix("/plugin"))
                    .unwrap_or_default()
                    .trim();
                let (verb, rest) = match args.split_once(char::is_whitespace) {
                    Some((verb, rest)) => (verb, rest.trim()),
                    None => (args, ""),
                };
                match verb {
                    "" | "list" => {
                        app.status = "loading plugins...".to_string();
                        app.show_progress("Loading plugins");
                        let tx = listings_tx.clone();
                        tokio::task::spawn_blocking(move || {
                            let result = plugins_listing().map_err(|err| format!("{err:#}"));
                            let _ = tx.send(result);
                        });
                    }
                    "install" => {
                        if rest.is_empty() {
                            app.items.push(ChatItem::Error(
                                "usage: /plugins install <name>[@marketplace]".to_string(),
                            ));
                            return;
                        }
                        let (name, marketplace) = crate::plugin_registry::split_ref(rest);
                        app.status = format!("installing plugin `{name}`...");
                        let tx = plugins_tx.clone();
                        tokio::spawn(async move {
                            let result = crate::plugin_registry::install(
                                &name,
                                marketplace.as_deref(),
                            )
                            .await
                            .map_err(|err| format!("{err:#}"));
                            let _ = tx.send(match result {
                                Ok(text) => text,
                                Err(err) => format!("plugin install failed: {err}"),
                            });
                        });
                    }
                    "uninstall" => {
                        if rest.is_empty() {
                            app.items.push(ChatItem::Error(
                                "usage: /plugins uninstall <name>".to_string(),
                            ));
                            return;
                        }
                        match crate::plugin_registry::uninstall(rest) {
                            Ok(text) => app.items.push(ChatItem::Info(text)),
                            Err(err) => app.items.push(ChatItem::Error(format!("plugin: {err:#}"))),
                        }
                    }
                    "enable" | "disable" => {
                        if rest.is_empty() {
                            app.items
                                .push(ChatItem::Error(format!("usage: /plugins {verb} <name>")));
                            return;
                        }
                        match crate::plugin_registry::set_enabled(rest, verb == "enable") {
                            Ok(text) => app.items.push(ChatItem::Info(text)),
                            Err(err) => app.items.push(ChatItem::Error(format!("plugin: {err:#}"))),
                        }
                    }
                    "marketplace" => {
                        let (sub, sub_rest) = match rest.split_once(char::is_whitespace) {
                            Some((sub, rest)) => (sub, rest.trim()),
                            None => (rest, ""),
                        };
                        match sub {
                            "list" => match crate::plugin_registry::list_marketplaces() {
                                Ok(text) => app.items.push(ChatItem::Info(text)),
                                Err(err) => {
                                    app.items.push(ChatItem::Error(format!("plugin: {err:#}")))
                                }
                            },
                            "add" => {
                                if sub_rest.is_empty() {
                                    app.items.push(ChatItem::Error(
                                        "usage: /plugins marketplace add <url|path>".to_string(),
                                    ));
                                    return;
                                }
                                app.status = format!("adding marketplace `{sub_rest}`...");
                                let source = sub_rest.to_string();
                                let tx = plugins_tx.clone();
                                tokio::spawn(async move {
                                    let result = crate::plugin_registry::add_marketplace(&source)
                                        .await
                                        .map_err(|err| format!("{err:#}"));
                                    let _ = tx.send(match result {
                                        Ok(text) => text,
                                        Err(err) => format!("marketplace add failed: {err}"),
                                    });
                                });
                            }
                            "update" => {
                                if sub_rest.is_empty() {
                                    app.items.push(ChatItem::Error(
                                        "usage: /plugins marketplace update <name>".to_string(),
                                    ));
                                    return;
                                }
                                app.status = format!("updating marketplace `{sub_rest}`...");
                                let name = sub_rest.to_string();
                                let tx = plugins_tx.clone();
                                tokio::spawn(async move {
                                    let result = crate::plugin_registry::update_marketplace(&name)
                                        .await
                                        .map_err(|err| format!("{err:#}"));
                                    let _ = tx.send(match result {
                                        Ok(text) => text,
                                        Err(err) => format!("marketplace update failed: {err}"),
                                    });
                                });
                            }
                            "remove" => {
                                if sub_rest.is_empty() {
                                    app.items.push(ChatItem::Error(
                                        "usage: /plugins marketplace remove <name>".to_string(),
                                    ));
                                    return;
                                }
                                match crate::plugin_registry::remove_marketplace(sub_rest) {
                                    Ok(text) => app.items.push(ChatItem::Info(text)),
                                    Err(err) => {
                                        app.items.push(ChatItem::Error(format!("plugin: {err:#}")))
                                    }
                                }
                            }
                            _ => app.items.push(ChatItem::Error(
                                "usage: /plugins marketplace <list|add <url|path>|update <name>|remove <name>>"
                                    .to_string(),
                            )),
                        }
                    }
                    _ => app.items.push(ChatItem::Error(
                        "usage: /plugins [list] · install <name>[@mp] · uninstall <name> · enable|disable <name> · marketplace <list|add|update|remove>"
                            .to_string(),
                    )),
                }
                return;
            }
            if raw == "/notify" || raw.starts_with("/notify ") {
                app.clear_input();
                refresh_suggestions(app, config);
                handle_notify_command(app, config, &raw);
                return;
            }
            if raw == "/approvals" || raw.starts_with("/approvals ") {
                app.clear_input();
                refresh_suggestions(app, config);
                handle_approvals_command(app, config, approvals, cwd, &raw, &Config::config_path());
                return;
            }
            if raw == "/usage" {
                app.clear_input();
                refresh_suggestions(app, config);
                app.usage_modal = Some(UsageState::new(app.usage_settings.clone()));
                return;
            }
            if raw.starts_with("/usage ") {
                app.clear_input();
                refresh_suggestions(app, config);
                handle_usage_command(app, config, &raw, usage_tx);
                return;
            }
            if raw == "/models" || raw.starts_with("/models ") {
                app.clear_input();
                refresh_suggestions(app, config);
                let providers = model_providers(config);
                if providers.is_empty() {
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
                let provider = crate::auth::canonical_provider(&config.provider);
                let mut state = ModelsState::loading(
                    provider,
                    config.model.clone(),
                    config.default_model.clone(),
                );
                state.filter = filter;
                app.models = Some(state);
                app.status = "loading models...".to_string();
                let tx = models_tx.clone();
                tokio::spawn(async move {
                    let fetched = futures::future::join_all(providers.into_iter().map(
                        |(name, provider_config)| async move {
                            let models = LlmClient::new(provider_config)
                                .list_models()
                                .await
                                .map_err(|err| format!("{err:#}"));
                            (name, models)
                        },
                    ))
                    .await;
                    let mut catalogs = ModelCatalogs::default();
                    for (name, models) in fetched {
                        match models {
                            Ok(models) => catalogs.choices.extend(
                                models
                                    .into_iter()
                                    .map(|model| ModelChoice::new(name.clone(), model)),
                            ),
                            Err(err) => catalogs.errors.push(format!("{name}: {err}")),
                        }
                    }
                    let _ = tx.send(catalogs);
                });
                return;
            }
            if raw == "/help" || raw == "/?" {
                app.clear_input();
                refresh_suggestions(app, config);
                app.items.push(ChatItem::Info(help_text(config)));
                return;
            }
            if raw == "/clone" {
                app.clear_input();
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
                app.clear_input();
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
                    Some(index) => fork_session(app, config, cwd, session, rx, index),
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
            if raw == "/tree" || raw.starts_with("/tree ") {
                app.clear_input();
                refresh_suggestions(app, config);
                if app.busy {
                    return;
                }
                let requested = raw
                    .strip_prefix("/tree")
                    .unwrap_or_default()
                    .trim()
                    .parse::<usize>()
                    .ok();
                match requested {
                    Some(index) => branch_in_place(app, config, session, rx, index),
                    None => {
                        for (position, message) in user_messages(&app.history) {
                            let preview: String = message.chars().take(72).collect();
                            app.items
                                .push(ChatItem::Info(format!("{position}. {preview}")));
                        }
                        app.items.push(ChatItem::Info(
                            "use /tree <n> to branch there (the abandoned path is summarized) \
                             or /clone to duplicate"
                                .to_string(),
                        ));
                    }
                }
                return;
            }
            if raw == "/new" {
                app.clear_input();
                refresh_suggestions(app, config);
                if app.busy {
                    return;
                }
                app.history.clear();
                app.items.clear();
                app.tokens_in = 0;
                app.tokens_out = 0;
                app.tokens_cache_read = 0;
                app.tokens_cache_write = 0;
                app.cost = 0.0;
                app.cache_hit_rate = None;
                app.context_used = 0;
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
                app.clear_input();
                refresh_suggestions(app, config);
                app.items
                    .push(ChatItem::Info(session_info(session, &app.history)));
                return;
            }
            if raw == "/resume" {
                app.clear_input();
                refresh_suggestions(app, config);
                if app.busy {
                    return;
                }
                match SessionLog::list(cwd) {
                    Ok(sessions) if sessions.is_empty() => {
                        app.items.push(ChatItem::Info(
                            "no sessions for this project yet".to_string(),
                        ));
                    }
                    Ok(sessions) => {
                        let count = sessions.len();
                        app.sessions = Some(SessionsState::ready(sessions));
                        app.status = format!("{count} session(s) — pick one");
                    }
                    Err(err) => app.items.push(ChatItem::Error(format!("session: {err:#}"))),
                }
                return;
            }
            if raw == "/name" || raw.starts_with("/name ") {
                app.clear_input();
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
                app.clear_input();
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
                if let Some(log) = session.as_ref() {
                    let _ = log.append_model_change(&config.provider, &config.model);
                }
                app.items
                    .push(ChatItem::Info(format!("model set to {requested}")));
                return;
            }
            if raw == "/thinking" || raw.starts_with("/thinking ") {
                app.clear_input();
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
                        if let Some(log) = session.as_ref() {
                            let _ = log.append_thinking_level(level.label());
                        }
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
                app.clear_input();
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
                app.theme = theme;
                app.invalidate_render_cache();
                app.items
                    .push(ChatItem::Info(format!("theme set to {requested}")));
                return;
            }
            if raw == "/trust" || raw.starts_with("/trust ") {
                app.clear_input();
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
                app.clear_input();
                match Config::load(
                    cwd,
                    None,
                    None,
                    None,
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
                app.clear_input();
                refresh_suggestions(app, config);
                app.items.push(ChatItem::Info(hotkeys_text()));
                return;
            }
            if raw == "/export" || raw.starts_with("/export ") {
                app.clear_input();
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
            app.clear_input();
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

            let mut parts = app.take_attachment_parts();
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
            app.notify_on_finish = true;
            app.status = "thinking...".to_string();

            let (tx, new_rx) = unbounded_channel();
            *rx = Some(new_rx);
            let config = config.clone();
            let cwd = cwd.to_path_buf();
            let history = app.history.clone();
            let approve = approvals.approver(&cwd, tx.clone(), app.steering.clone());
            let runtime = Runtime {
                mcp: Arc::clone(mcp),
                plugins: Arc::clone(plugins),
                session: session.clone().map(Arc::new),
                snapshots: snapshots.cloned(),
                lsp: Arc::clone(lsp),
                approve,
                steering: app.steering.clone(),
                follow_ups: app.follow_ups.clone(),
                cancel: crate::agent::Cancel::new(),
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
                    if app.add_attachment(part) {
                        let count = app.attachments.len();
                        app.show_status(format!("{count} attachment(s) pending"));
                    } else {
                        app.show_status("image already attached");
                    }
                }
                None => {
                    app.show_status("no image found on clipboard");
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
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if !app.input.is_empty() {
                app.input_home();
                refresh_suggestions(app, config);
            }
        }
        KeyCode::Char('y') if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_up(1),
        KeyCode::Char('e') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if app.input.is_empty() {
                app.scroll_down(1);
            } else {
                app.input_end();
                refresh_suggestions(app, config);
            }
        }
        KeyCode::Char('g') if key.modifiers.contains(KeyModifiers::CONTROL) => app.scroll_to_top(),
        KeyCode::Char(ch) => {
            app.insert_input(&ch.to_string());
            app.history_index = None;
            app.auto_scroll = true;
            refresh_suggestions(app, config);
        }
        KeyCode::Backspace => {
            app.input_backspace();
            app.history_index = None;
            refresh_suggestions(app, config);
        }
        KeyCode::Left => {
            app.input_cursor_left();
            refresh_suggestions(app, config);
        }
        KeyCode::Right => {
            app.input_cursor_right();
            refresh_suggestions(app, config);
        }
        KeyCode::Tab if !app.suggestions.is_empty() => {
            if complete_suggestion(app) {
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
    app.clear_input();
}

/// What the typed answer to a tool approval accepts.
pub(crate) const APPROVAL_HINT: &str = "y = once · a = always (this project) · n [reason] = deny";

/// Maps what the user typed into the answer field to a decision: `y`/`a`/`n`
/// (or the words) answer it, and any other text refuses the tool with that text
/// as guidance for the agent. `None` means nothing to answer with — an empty
/// line keeps the request open.
fn parse_approval_answer(input: &str) -> Option<Decision> {
    let text = input.trim();
    if text.is_empty() {
        return None;
    }
    let lowered = text.to_ascii_lowercase();
    let word = match lowered.as_str() {
        "y" => "yes",
        "a" => "always",
        "n" => "no",
        other => other,
    };
    Some(
        Decision::parse(word, None).unwrap_or_else(|| Decision::Deny {
            message: Some(text.to_string()),
        }),
    )
}

/// Answers the waiting tool with what was typed. An empty line is not an
/// answer, so the request stays open with the hint.
fn answer_approval_input(app: &mut App, approvals: &ApprovalBroker, typed: &str) {
    match parse_approval_answer(typed) {
        Some(decision) => answer_approval(app, approvals, decision),
        None => app.show_status(format!("approve: {APPROVAL_HINT}")),
    }
}

/// Sends the answer for the tool that is waiting and records it in the
/// transcript. The broker routes the id back to the request that emitted it, so
/// an answer for a request that already timed out is dropped rather than applied
/// to the next one.
fn answer_approval(app: &mut App, approvals: &ApprovalBroker, decision: Decision) {
    let Some(pending) = app.pending_approval.take() else {
        return;
    };
    app.clear_input();
    app.auto_scroll = true;
    let outcome = match &decision {
        Decision::Once => format!("allowed `{}` once", pending.tool),
        Decision::Always => format!("always allowed `{}` in this project", pending.tool),
        Decision::Deny {
            message: Some(reason),
        } => format!("denied `{}` — told the agent: {reason}", pending.tool),
        Decision::Deny { message: None } => format!("denied `{}`", pending.tool),
    };
    app.status = match &decision {
        Decision::Deny { .. } => "denied".to_string(),
        _ => "thinking...".to_string(),
    };
    if approvals.answer(pending.id, decision) {
        app.items.push(ChatItem::Info(outcome));
    } else {
        app.items.push(ChatItem::Error(format!(
            "`{}` was no longer waiting for an answer; it was denied when the prompt timed out",
            pending.tool
        )));
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
fn context_limit(config: &Config) -> u64 {
    config.context_window()
}

/// Providers with a stored credential plus the active one. Pi shows the provider
/// in the footer only when more than one is available.
fn available_providers(config: &Config) -> usize {
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Ok(store) = crate::auth::AuthStore::load() {
        names.extend(store.entries.keys().cloned());
    }
    if !config.api_key.trim().is_empty() {
        names.insert(config.provider.clone());
    }
    names.len().max(1)
}

fn help_text(config: &Config) -> String {
    let mut lines = vec![
        "built-in commands:".to_string(),
        "  /help                 show this help".to_string(),
        "  /hotkeys              show the keyboard shortcuts".to_string(),
        "  /exit                 quit Oxide".to_string(),
        "  /skill:<name>         load a skill by name".to_string(),
        "  /new                  start a new session".to_string(),
        "  /session              show session file, id, name, and stats".to_string(),
        "  /resume               browse and resume a past session".to_string(),
        "  /name <name>          name the current session".to_string(),
        "  /model [id]           show or switch the active model".to_string(),
        "  /thinking [level]     show or set the thinking level".to_string(),
        "  /export [file]        export the session to HTML".to_string(),
        "  /tree [n]             list branch points, or branch at message n".to_string(),
        "  /fork [n]             branch a new session from message n".to_string(),
        "  /clone                duplicate the current session".to_string(),
        "  /reload               reload config, commands, and skills".to_string(),
        "  /trust [show|off]     save or show the project trust decision".to_string(),
        "  /theme [name]         show or switch the color theme".to_string(),
        "  /init                 create or update AGENTS.md for this project".to_string(),
        "  /connect [provider]   connect a provider and save its API key".to_string(),
        "  /login [provider]     alias of /connect; switches when already stored".to_string(),
        "  /logout [provider]    remove stored credentials (switches providers)".to_string(),
        "  /models [filter]      list models from every logged-in provider".to_string(),
        "  /mcps                 list MCP servers and connection status".to_string(),
        "  /plugins              manage plugins and marketplaces".to_string(),
        "  /marketplaces         browse, add, and remove plugin marketplaces".to_string(),
        "  /notify [on|off]      show or set completion notifications (sound too)".to_string(),
        "  /approvals [on|off]   ask before a gated tool runs; list or clear the rules"
            .to_string(),
        "  /usage                 configure the Portkey spend bar (dialog)".to_string(),
        "  /undo, /redo          revert or reapply the agent's file changes".to_string(),
        "  /compact [focus]      summarize older context, optionally with a focus".to_string(),
        "  /copy                 copy the last assistant message".to_string(),
        "  /copy all             copy the whole transcript".to_string(),
        format!(
            "keys: Enter send/guide · Shift+Enter newline · Alt+Enter follow-up while busy · {} edit queued · Shift+Tab/Ctrl+R reasoning · Ctrl+O tool details · Ctrl+T thinking · Ctrl+V image · Ctrl+A/E message start/end · ↑/↓ history · PgUp/PgDn/wheel scroll · Ctrl+U/D half page · drag to select and copy · Ctrl+C copy selection/quit",
            dequeue_key_label()
        ),
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

/// Starts a background fetch of today's and the month's Portkey spend for the
/// configured user. Returns whether a request was started.
fn spawn_usage_refresh(
    config: &Config,
    settings: &crate::portkey_usage::UsageSettings,
    usage_tx: &UnboundedSender<Result<crate::portkey_usage::Snapshot, String>>,
) -> bool {
    let Some(key) = settings.effective_key(config) else {
        return false;
    };
    let settings = settings.clone();
    let key = key.to_string();
    let tx = usage_tx.clone();
    tokio::spawn(async move {
        let result = crate::portkey_usage::spend(&settings, &key)
            .await
            .map_err(|err| format!("{err:#}"));
        let _ = tx.send(result);
    });
    true
}

/// Shows or hides the bar to match the settings and the logged-in provider, and
/// starts a fetch when it first appears. A login (or a provider change) can make
/// an already-enabled bar usable, so this runs after the connect dialog too.
fn sync_usage_bar(
    app: &mut App,
    config: &Config,
    usage_tx: &UnboundedSender<Result<crate::portkey_usage::Snapshot, String>>,
) {
    let wanted = app.usage_settings.enabled && app.usage_settings.available(config);
    match (wanted, app.usage.is_some()) {
        (true, false) => {
            app.usage = Some(crate::portkey_usage::UsageBar::new(&app.usage_settings));
            spawn_usage_refresh(config, &app.usage_settings, usage_tx);
        }
        (false, true) => app.usage = None,
        _ => {}
    }
}

/// `/usage`: configure and toggle the Portkey spend bar.
/// Handles `/notify [on|off]`, `/notify sound [on|off]`, and `/notify test`,
/// persisting the two completion-notification flags to `settings.json`.
fn handle_notify_command(app: &mut App, config: &mut Config, raw: &str) {
    const HINT: &str = "usage: /notify [on|off] · /notify sound [on|off] · /notify test";
    let args = raw.strip_prefix("/notify").unwrap_or_default().trim();
    let mut settings = config.notify;

    if args.is_empty() || args == "status" {
        app.items.push(ChatItem::Info(format!(
            "notifications: {}; sound: {}\n{HINT}",
            on_off(settings.on_complete),
            on_off(settings.sound),
        )));
        return;
    }

    let (verb, rest) = match args.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb, rest.trim()),
        None => (args, ""),
    };

    let (key, enabled) = match verb {
        "test" => {
            crate::notify::send(
                "Oxide",
                "Test notification — all tasks completed.",
                settings.sound,
            );
            app.items
                .push(ChatItem::Info("sent a test notification".to_string()));
            return;
        }
        "sound" => {
            if rest.is_empty() {
                app.items.push(ChatItem::Info(format!(
                    "notification sound: {}",
                    on_off(settings.sound)
                )));
                return;
            }
            let Some(enabled) = parse_toggle(rest) else {
                app.items.push(ChatItem::Error(format!(
                    "usage: /notify sound <on|off>\n{HINT}"
                )));
                return;
            };
            settings.sound = enabled;
            (crate::notify::NotifyKey::Sound, enabled)
        }
        _ => {
            let Some(enabled) = parse_toggle(verb) else {
                app.items.push(ChatItem::Error(format!(
                    "unknown /notify option `{verb}`\n{HINT}"
                )));
                return;
            };
            settings.on_complete = enabled;
            (crate::notify::NotifyKey::OnComplete, enabled)
        }
    };

    config.notify = settings;
    match crate::notify::save(key, enabled) {
        Ok(path) => app.items.push(ChatItem::Info(format!(
            "notifications: {}; sound: {} ({})",
            on_off(settings.on_complete),
            on_off(settings.sound),
            path.display()
        ))),
        Err(err) => app.items.push(ChatItem::Error(format!("notify: {err:#}"))),
    }
}

fn on_off(value: bool) -> &'static str {
    if value {
        "on"
    } else {
        "off"
    }
}

/// `/approvals`: whether a permission-gated tool is asked about, plus the rules
/// this project has already answered with "always allow".
fn handle_approvals_command(
    app: &mut App,
    config: &mut Config,
    approvals: &ApprovalBroker,
    cwd: &Path,
    raw: &str,
    path: &Path,
) {
    const HINT: &str = "usage: /approvals [on|off] · /approvals list · /approvals clear";
    let args = raw.strip_prefix("/approvals").unwrap_or_default().trim();
    let verb = args.split_whitespace().next().unwrap_or_default();

    match verb {
        "list" | "" => {
            let allowed = approvals.list(cwd);
            let rules = if allowed.is_empty() {
                "none".to_string()
            } else {
                allowed.join(", ")
            };
            app.items.push(ChatItem::Info(format!(
                "tool approvals: {} ({}); always allowed here: {rules}\n{HINT}",
                if config.auto_approve { "off" } else { "on" },
                if config.auto_approve {
                    "gated tools run without asking"
                } else {
                    "asked before a gated tool runs"
                },
            )));
        }
        "clear" => match approvals.clear(cwd) {
            Ok(()) => app.items.push(ChatItem::Info(
                "cleared this project's approvals".to_string(),
            )),
            Err(err) => app
                .items
                .push(ChatItem::Error(format!("approvals: {err:#}"))),
        },
        other => {
            let Some(asking) = parse_toggle(other) else {
                app.items.push(ChatItem::Error(format!(
                    "unknown /approvals option `{other}`\n{HINT}"
                )));
                return;
            };
            // Asking is the inverse of the stored auto-approval, and it is a
            // `config.json` key, so `/approvals` writes the same field the
            // `--ask-approvals` flag overrides for a single run.
            config.auto_approve = !asking;
            match Config::set_auto_approve_at(path, config.auto_approve) {
                Ok(()) => app.items.push(ChatItem::Info(format!(
                    "tool approvals: {} ({})",
                    on_off(asking),
                    path.display()
                ))),
                Err(err) => app
                    .items
                    .push(ChatItem::Error(format!("approvals: {err:#}"))),
            }
        }
    }
}

fn parse_toggle(value: &str) -> Option<bool> {
    match value.to_ascii_lowercase().as_str() {
        "on" | "true" | "yes" | "enable" | "enabled" | "1" => Some(true),
        "off" | "false" | "no" | "disable" | "disabled" | "0" => Some(false),
        _ => None,
    }
}

fn handle_usage_command(
    app: &mut App,
    config: &Config,
    raw: &str,
    usage_tx: &UnboundedSender<Result<crate::portkey_usage::Snapshot, String>>,
) {
    const HINT: &str = "usage: /usage on|off · /usage user <firstname.lastname> · /usage budget <amount|off> · /usage currency <usd|cny> · /usage key <pk-...> · /usage metadata <key>";
    let args = raw.strip_prefix("/usage").unwrap_or_default().trim();
    let (verb, rest) = match args.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb, rest.trim()),
        None => (args, ""),
    };
    let mut settings = app.usage_settings.clone();

    let (changed, note, refresh) = match verb {
        "" | "status" => {
            app.items
                .push(ChatItem::Info(crate::portkey_usage::status_text(
                    &settings, config,
                )));
            if settings.available(config) {
                spawn_usage_refresh(config, &settings, usage_tx);
            }
            return;
        }
        "on" => {
            if !config.is_portkey() {
                app.items.push(ChatItem::Error(
                    "the Portkey spend bar needs a Portkey login — run /login portkey".to_string(),
                ));
                return;
            }
            if settings.user.trim().is_empty() {
                app.items.push(ChatItem::Error(
                    "set the Portkey username first: /usage user <firstname.lastname>".to_string(),
                ));
                return;
            }
            if settings.effective_key(config).is_none() {
                app.items.push(ChatItem::Error(
                    "no Portkey API key — run /login portkey or set one with /usage key <pk-...>"
                        .to_string(),
                ));
                return;
            }
            settings.enabled = true;
            (
                true,
                Some(format!("portkey usage bar on ({})", settings.user.trim())),
                true,
            )
        }
        "off" => {
            settings.enabled = false;
            (true, Some("portkey usage bar off".to_string()), false)
        }
        "user" => {
            if rest.is_empty() || rest.split_whitespace().count() > 1 {
                app.items.push(ChatItem::Error(
                    "usage: /usage user <firstname.lastname> (one word)".to_string(),
                ));
                return;
            }
            settings.user = rest.to_string();
            let enabled = settings.enabled && settings.available(config);
            if settings.enabled && !enabled {
                app.items.push(ChatItem::Error(
                    "the Portkey spend bar needs a Portkey login — run /login portkey".to_string(),
                ));
            }
            (true, Some(format!("portkey user set to {rest}")), enabled)
        }
        "key" => {
            if rest.is_empty() {
                app.items.push(ChatItem::Error(
                    "usage: /usage key <pk-...> (or `off` to use the provider key)".to_string(),
                ));
                return;
            }
            settings.api_key = if matches!(rest, "off" | "none" | "default") {
                String::new()
            } else {
                rest.to_string()
            };
            let note = if settings.api_key.is_empty() {
                "portkey usage key cleared; using the provider credential".to_string()
            } else {
                format!("portkey usage key set ({})", mask(&settings.api_key))
            };
            (true, Some(note), settings.enabled)
        }
        "budget" => {
            let amount = rest.trim();
            match amount.chars().next() {
                Some('$') => settings.currency = crate::portkey_usage::Currency::Usd,
                Some('¥' | '￥') => settings.currency = crate::portkey_usage::Currency::Cny,
                _ => {}
            }
            settings.budget = amount
                .trim_start_matches(['$', '¥', '￥'])
                .parse::<f64>()
                .ok()
                .filter(|value| *value > 0.0 && value.is_finite());
            let note = match settings.budget {
                Some(budget) => format!(
                    "portkey month budget set to {}",
                    settings.currency.format(budget)
                ),
                None => "portkey month budget cleared".to_string(),
            };
            (true, Some(note), false)
        }
        "currency" => {
            let Some(currency) = crate::portkey_usage::Currency::parse(rest) else {
                app.items.push(ChatItem::Error(
                    "usage: /usage currency <usd|cny> (also `$` or `¥`)".to_string(),
                ));
                return;
            };
            settings.currency = currency;
            let note = match settings.budget {
                Some(budget) => format!(
                    "portkey budget currency set to {} ({})",
                    currency.name(),
                    currency.format(budget)
                ),
                None => format!("portkey budget currency set to {}", currency.name()),
            };
            (true, Some(note), false)
        }
        "metadata" => {
            if rest.is_empty() {
                app.items.push(ChatItem::Error(
                    "usage: /usage metadata <key> (e.g. `_user` or `email`)".to_string(),
                ));
                return;
            }
            settings.metadata_key = rest.to_string();
            (
                true,
                Some(format!("portkey user metadata key set to {rest}")),
                settings.enabled,
            )
        }
        other => {
            app.items.push(ChatItem::Error(format!(
                "unknown /usage option `{other}`\n{HINT}"
            )));
            return;
        }
    };

    if changed {
        if let Err(err) = settings.save() {
            app.items.push(ChatItem::Error(format!("usage: {err:#}")));
            return;
        }
    }
    apply_usage_settings(app, config, settings, refresh, usage_tx);
    if let Some(note) = note {
        app.items.push(ChatItem::Info(note));
    }
}

/// Applies Portkey usage settings to the running app: stores them, shows or
/// hides the bar, and starts a fetch when it first appears.
fn apply_usage_settings(
    app: &mut App,
    config: &Config,
    settings: crate::portkey_usage::UsageSettings,
    refresh: bool,
    usage_tx: &UnboundedSender<Result<crate::portkey_usage::Snapshot, String>>,
) {
    app.usage_settings = settings.clone();
    if !settings.enabled || !settings.available(config) {
        app.usage = None;
    } else if let Some(bar) = app.usage.as_mut() {
        bar.update(&settings);
    } else {
        app.usage = Some(crate::portkey_usage::UsageBar::new(&settings));
    }
    if refresh {
        spawn_usage_refresh(config, &settings, usage_tx);
    }
}

/// Routes keys while the `/usage` settings dialog is open. Text fields are
/// edited in place; `Esc` commits and closes the dialog.
fn handle_usage_key(
    key: KeyEvent,
    app: &mut App,
    config: &Config,
    usage_tx: &UnboundedSender<Result<crate::portkey_usage::Snapshot, String>>,
) {
    let Some(mut state) = app.usage_modal.take() else {
        return;
    };
    if state.editing {
        match key.code {
            KeyCode::Esc => {
                state.editing = false;
                state.input.clear();
                state.error = None;
            }
            KeyCode::Enter => {
                if commit_usage_field(&mut state) {
                    state.editing = false;
                    state.input.clear();
                    state.error = None;
                }
            }
            KeyCode::Backspace => {
                state.input.pop();
                state.error = None;
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
        app.usage_modal = Some(state);
        return;
    }

    match key.code {
        KeyCode::Esc => {
            let settings = state.settings.clone();
            if let Err(err) = settings.save() {
                app.items.push(ChatItem::Error(format!("usage: {err:#}")));
            } else {
                app.show_status("portkey usage settings saved");
            }
            if settings.enabled && !settings.available(config) {
                app.items.push(ChatItem::Error(
                    "the Portkey spend bar needs a Portkey login — run /login portkey".to_string(),
                ));
            }
            apply_usage_settings(app, config, settings, true, usage_tx);
        }
        KeyCode::Up => {
            state.selected = state.selected.saturating_sub(1);
            state.error = None;
            app.usage_modal = Some(state);
        }
        KeyCode::Down => {
            state.selected = (state.selected + 1).min(UsageField::ALL.len() - 1);
            state.error = None;
            app.usage_modal = Some(state);
        }
        KeyCode::Enter | KeyCode::Char(' ') => {
            match state.field() {
                UsageField::Enabled => {
                    state.settings.enabled = !state.settings.enabled;
                    state.error = if state.settings.enabled && !state.settings.available(config) {
                        Some("needs a Portkey login — run /login portkey".to_string())
                    } else {
                        None
                    };
                }
                UsageField::Currency => {
                    state.settings.currency = match state.settings.currency {
                        crate::portkey_usage::Currency::Usd => crate::portkey_usage::Currency::Cny,
                        crate::portkey_usage::Currency::Cny => crate::portkey_usage::Currency::Usd,
                    };
                    state.error = None;
                }
                field => {
                    state.editing = true;
                    // A stored key is masked, so editing starts from a blank
                    // field instead of an unreadable prefilled value.
                    state.input = if field == UsageField::ApiKey {
                        String::new()
                    } else {
                        usage_field_value(&state.settings, field)
                    };
                    state.error = None;
                }
            }
            app.usage_modal = Some(state);
        }
        _ => {
            app.usage_modal = Some(state);
        }
    }
}

/// The current text of an editable usage field, used to seed the editor.
fn usage_field_value(settings: &crate::portkey_usage::UsageSettings, field: UsageField) -> String {
    match field {
        UsageField::User => settings.user.clone(),
        UsageField::Metadata => settings.metadata_key.clone(),
        UsageField::Budget => settings
            .budget
            .map(|budget| format!("{budget:.2}"))
            .unwrap_or_default(),
        UsageField::ApiKey => settings.api_key.clone(),
        UsageField::Endpoint => settings.base_url.clone(),
        UsageField::Enabled | UsageField::Currency => String::new(),
    }
}

/// Applies the dialog's edited text to the selected field. Returns `false` and
/// records an error when the value does not parse, so the editor stays open.
fn commit_usage_field(state: &mut UsageState) -> bool {
    let raw = state.input.trim().to_string();
    match state.field() {
        UsageField::User => {
            if raw.contains(char::is_whitespace) {
                state.error = Some("enter one word, e.g. firstname.lastname".to_string());
                return false;
            }
            state.settings.user = raw;
        }
        UsageField::Metadata => {
            state.settings.metadata_key = if raw.is_empty() {
                "_user".to_string()
            } else {
                raw
            };
        }
        UsageField::Budget => {
            match raw.chars().next() {
                Some('$') => state.settings.currency = crate::portkey_usage::Currency::Usd,
                Some('¥' | '￥') => state.settings.currency = crate::portkey_usage::Currency::Cny,
                _ => {}
            }
            let amount = raw.trim_start_matches(['$', '¥', '￥']).trim();
            if amount.is_empty() || matches!(amount, "off" | "none") {
                state.settings.budget = None;
            } else {
                match amount.parse::<f64>() {
                    Ok(value) if value > 0.0 && value.is_finite() => {
                        state.settings.budget = Some(value);
                    }
                    _ => {
                        state.error = Some("budget must be a positive number".to_string());
                        return false;
                    }
                }
            }
        }
        UsageField::ApiKey => {
            state.settings.api_key = if matches!(raw.as_str(), "off" | "none" | "default") {
                String::new()
            } else {
                raw
            };
        }
        UsageField::Endpoint => {
            state.settings.base_url = if raw.is_empty() {
                "https://api.portkey.ai/v1".to_string()
            } else {
                raw
            };
        }
        UsageField::Enabled | UsageField::Currency => {}
    }
    true
}

/// Masks a credential for display (`pk-test-1234` -> `pk-te...1234`).
fn mask(key: &str) -> String {
    let chars: Vec<char> = key.chars().collect();
    if chars.len() <= 8 {
        return "…".to_string();
    }
    format!(
        "{}...{}",
        chars[..4].iter().collect::<String>(),
        chars[chars.len() - 4..].iter().collect::<String>()
    )
}

/// Keyboard shortcuts shown by `/hotkeys`.
fn hotkeys_text() -> String {
    [
        "keyboard shortcuts:".to_string(),
        "  Enter                 send (queues steering while busy)".to_string(),
        "  Shift+Enter           insert a newline".to_string(),
        "  Alt+Enter             queue a follow-up while busy".to_string(),
        format!(
            "  {:<22}pull queued messages back into the editor",
            dequeue_key_label()
        ),
        "  Esc                   clear the input, or deny a waiting tool".to_string(),
        "  Shift+Tab / Ctrl+R    cycle reasoning/thinking level".to_string(),
        "  Ctrl+O                toggle tool output".to_string(),
        "  Ctrl+T                show or hide thinking blocks".to_string(),
        "  Ctrl+V                attach a clipboard image".to_string(),
        "  Tab                   complete the selected command or @path".to_string(),
        "  Ctrl+A / Ctrl+E       jump to the start/end of the message (when it is not empty)"
            .to_string(),
        "  Ctrl+Y                scroll up one line".to_string(),
        "  Ctrl+E                scroll down one line (when the message is empty)".to_string(),
        "  Ctrl+U / Ctrl+D       scroll half a page".to_string(),
        "  PgUp / PgDn / wheel   scroll the transcript".to_string(),
        "  Ctrl+G / Home         scroll to the top".to_string(),
        "  End                   return to the latest message".to_string(),
        "  Up / Down             input history".to_string(),
        "  drag (mouse)          select text; copies on release".to_string(),
        "  Ctrl+C                copy the selection, or quit".to_string(),
        "  /exit                 quit Oxide".to_string(),
        "  /copy                 copy the last assistant message".to_string(),
        "  /copy all             copy the whole transcript".to_string(),
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

/// Branches the conversation at the `index`-th user message (1-based). When the
/// abandoned tail is non-empty it is summarized into the new branch (Pi-style
/// branch summarization), which runs asynchronously while the TUI stays busy.
fn fork_session(
    app: &mut App,
    config: &Config,
    cwd: &Path,
    session: &mut Option<SessionLog>,
    rx: &mut Option<UnboundedReceiver<AgentEvent>>,
    index: usize,
) {
    let Some((cut, prompt)) = fork_point(&app.history, index) else {
        app.items.push(ChatItem::Error(format!(
            "no user message at index {index} (1-based)"
        )));
        return;
    };
    let abandoned = app.history[cut..].to_vec();
    match SessionLog::fork(cwd, &app.history[..cut]) {
        Ok(log) => {
            app.history.truncate(cut);
            *session = Some(log);
            app.set_input(prompt);
            app.items.push(ChatItem::Info(format!(
                "forked at message {index} — edit and resend"
            )));
            if abandoned.is_empty() {
                return;
            }
            let config = config.clone();
            let log = session.clone();
            let base = app.history.clone();
            let (tx, new_rx) = unbounded_channel();
            *rx = Some(new_rx);
            app.busy = true;
            app.busy_since = Some(std::time::Instant::now());
            app.status = "summarizing branch...".to_string();
            tokio::spawn(async move {
                match crate::compact::summarize_branch(&config, &abandoned, None).await {
                    Ok((summary, usage)) => {
                        let history = if let Some(log) = &log {
                            let leaf = log.leaf_id();
                            let _ = log.branch_with_summary(
                                leaf.as_deref(),
                                summary,
                                None,
                                Some(usage.into()),
                            );
                            log.messages().unwrap_or(base)
                        } else {
                            let mut history = base;
                            history.push(Message::user(format!("[branch summary]\n{summary}")));
                            history
                        };
                        let _ = tx.send(AgentEvent::Usage {
                            input: usage.input,
                            output: usage.output,
                            cache_read: usage.cache_read,
                            cache_write: usage.cache_write,
                            cost: usage.cost,
                        });
                        let _ = tx.send(AgentEvent::Finished(history));
                    }
                    Err(err) => {
                        let _ = tx.send(AgentEvent::Error(format!("branch summary: {err:#}")));
                        let _ = tx.send(AgentEvent::Finished(base));
                    }
                }
            });
        }
        Err(err) => app.items.push(ChatItem::Error(format!("{err:#}"))),
    }
}

/// Navigates the current session tree to the `index`-th user message and
/// continues there, summarizing the abandoned path as a Pi-style
/// `branch_summary` entry.
fn branch_in_place(
    app: &mut App,
    config: &Config,
    session: &mut Option<SessionLog>,
    rx: &mut Option<UnboundedReceiver<AgentEvent>>,
    index: usize,
) {
    let Some(log) = session.as_ref() else {
        app.items.push(ChatItem::Error(
            "no session to branch — send a message first".to_string(),
        ));
        return;
    };
    let entries = log.context();
    let mut seen = 0usize;
    let mut found = false;
    let mut branch_from: Option<String> = None;
    let mut prompt = String::new();
    let mut abandoned: Vec<Message> = Vec::new();
    for entry in &entries {
        if let crate::session::Entry::Message(message) = entry {
            if message.message.role == "user" {
                seen += 1;
                if seen == index {
                    found = true;
                    branch_from = entry.parent_id().map(str::to_string);
                    prompt = message.message.display().unwrap_or_default();
                }
            }
        }
        if found {
            abandoned.extend(entry.context_messages());
        }
    }
    if !found {
        app.items.push(ChatItem::Error(format!(
            "no user message at index {index} (1-based)"
        )));
        return;
    }
    let log = log.clone();
    let config = config.clone();
    let (tx, new_rx) = unbounded_channel();
    *rx = Some(new_rx);
    app.busy = true;
    app.busy_since = Some(std::time::Instant::now());
    app.status = "summarizing branch...".to_string();
    tokio::spawn(async move {
        match crate::compact::summarize_branch(&config, &abandoned, None).await {
            Ok((summary, usage)) => {
                let _ = log.branch_with_summary(
                    branch_from.as_deref(),
                    summary,
                    None,
                    Some(usage.into()),
                );
                let history = log.messages().unwrap_or_default();
                let _ = tx.send(AgentEvent::Usage {
                    input: usage.input,
                    output: usage.output,
                    cache_read: usage.cache_read,
                    cache_write: usage.cache_write,
                    cost: usage.cost,
                });
                let _ = tx.send(AgentEvent::Branch {
                    history,
                    prompt,
                    message: format!("branched at message {index} — edit and resend"),
                });
            }
            Err(err) => {
                let _ = tx.send(AgentEvent::Error(format!("branch summary: {err:#}")));
                let history = log.messages().unwrap_or_default();
                let _ = tx.send(AgentEvent::Branch {
                    history,
                    prompt,
                    message: "branch summary failed".to_string(),
                });
            }
        }
    });
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
                "session {}\nname: {}\nfile: {}\nentries: {}\nmessages: {} user · {} assistant · {} tool",
                log.id(),
                name,
                log.cwd(),
                log.entries().len(),
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
        cwd.join(format!("Oxide-{id}.html"))
    } else {
        let path = PathBuf::from(target);
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    };
    let mut body = String::from(
        "<!doctype html>\n<meta charset=\"utf-8\">\n<title>Oxide session</title>\n\
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
    let value = value.trim();
    if let Ok(index) = value.parse::<usize>() {
        if let Some(option) = index
            .checked_sub(1)
            .and_then(|index| crate::auth::KNOWN_PROVIDERS.get(index))
        {
            return option.name.to_string();
        }
    }
    crate::auth::canonical_provider(value)
}

/// The built-in slash commands surfaced in the input autocomplete.
/// Handles `/attach [list|remove <id|n>|clear]`, editing the pending composer
/// attachments. Returns whether the input was an attach command.
fn handle_attach_command(app: &mut App, raw: &str) -> bool {
    let Some(rest) = raw
        .strip_prefix("/attachments")
        .or_else(|| raw.strip_prefix("/attach"))
    else {
        return false;
    };
    match rest.trim() {
        "" | "list" => app.items.push(ChatItem::Info(app.attachment_listing())),
        "clear" => {
            let removed = app.attachments.len();
            app.attachments.clear();
            app.items.push(ChatItem::Info(if removed == 0 {
                "no attachments".to_string()
            } else {
                format!("cleared {removed} attachment(s)")
            }));
        }
        other => {
            let key = other
                .strip_prefix("remove")
                .or_else(|| other.strip_prefix("rm"))
                .map(str::trim)
                .filter(|key| !key.is_empty());
            match key {
                Some(key) => match app.remove_attachment(key) {
                    Some(label) => app
                        .items
                        .push(ChatItem::Info(format!("removed attachment {label}"))),
                    None => app
                        .items
                        .push(ChatItem::Error(format!("no attachment matching `{key}`"))),
                },
                None => app.items.push(ChatItem::Info(
                    "usage: /attach · /attach remove <id|n> · /attach clear".to_string(),
                )),
            }
        }
    }
    true
}

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
            name: "exit".to_string(),
            description: "quit Oxide".to_string(),
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
            name: "resume".to_string(),
            description: "browse and resume a past session".to_string(),
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
            name: "mcps".to_string(),
            description: "check MCP server status".to_string(),
        },
        CommandHint {
            name: "plugins".to_string(),
            description: "manage plugins and marketplaces".to_string(),
        },
        CommandHint {
            name: "marketplaces".to_string(),
            description: "browse and manage plugin marketplaces".to_string(),
        },
        CommandHint {
            name: "notify".to_string(),
            description: "toggle completion notifications and sound".to_string(),
        },
        CommandHint {
            name: "approvals".to_string(),
            description: "ask before a gated tool runs".to_string(),
        },
        CommandHint {
            name: "usage".to_string(),
            description: "Portkey spend bar".to_string(),
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
        CommandHint {
            name: "attach".to_string(),
            description: "list, remove, or clear pending attachments".to_string(),
        },
    ]
}

/// One node in a built-in command's argument grammar, used for completion.
struct ArgSpec {
    value: &'static str,
    description: &'static str,
    children: &'static [ArgSpec],
}

const ON_OFF_ARGS: &[ArgSpec] = &[
    ArgSpec {
        value: "on",
        description: "enable",
        children: &[],
    },
    ArgSpec {
        value: "off",
        description: "disable",
        children: &[],
    },
];

const MARKETPLACE_ARGS: &[ArgSpec] = &[
    ArgSpec {
        value: "list",
        description: "list marketplaces",
        children: &[],
    },
    ArgSpec {
        value: "add",
        description: "add a marketplace",
        children: &[],
    },
    ArgSpec {
        value: "update",
        description: "fetch a marketplace's latest manifest",
        children: &[],
    },
    ArgSpec {
        value: "remove",
        description: "remove a marketplace",
        children: &[],
    },
];

const PLUGINS_ARGS: &[ArgSpec] = &[
    ArgSpec {
        value: "list",
        description: "list installed plugins",
        children: &[],
    },
    ArgSpec {
        value: "install",
        description: "install a plugin",
        children: &[],
    },
    ArgSpec {
        value: "uninstall",
        description: "remove a plugin",
        children: &[],
    },
    ArgSpec {
        value: "enable",
        description: "enable a plugin",
        children: &[],
    },
    ArgSpec {
        value: "disable",
        description: "disable a plugin",
        children: &[],
    },
    ArgSpec {
        value: "marketplace",
        description: "manage marketplaces",
        children: MARKETPLACE_ARGS,
    },
];

/// Argument completions for the built-in commands with a fixed grammar.
const COMMAND_ARGS: &[(&str, &[ArgSpec])] = &[
    (
        "attach",
        &[
            ArgSpec {
                value: "list",
                description: "list pending attachments",
                children: &[],
            },
            ArgSpec {
                value: "remove",
                description: "remove one by id or number",
                children: &[],
            },
            ArgSpec {
                value: "clear",
                description: "remove every attachment",
                children: &[],
            },
        ],
    ),
    (
        "copy",
        &[ArgSpec {
            value: "all",
            description: "copy the whole transcript",
            children: &[],
        }],
    ),
    (
        "notify",
        &[
            ArgSpec {
                value: "on",
                description: "enable the completion toast",
                children: &[],
            },
            ArgSpec {
                value: "off",
                description: "disable the completion toast",
                children: &[],
            },
            ArgSpec {
                value: "sound",
                description: "toggle the alert sound",
                children: ON_OFF_ARGS,
            },
            ArgSpec {
                value: "test",
                description: "send a sample notification",
                children: &[],
            },
        ],
    ),
    (
        "approvals",
        &[
            ArgSpec {
                value: "on",
                description: "ask before a gated tool runs",
                children: &[],
            },
            ArgSpec {
                value: "off",
                description: "run gated tools without asking",
                children: &[],
            },
            ArgSpec {
                value: "list",
                description: "show the state and this project's rules",
                children: &[],
            },
            ArgSpec {
                value: "clear",
                description: "forget this project's always-allow rules",
                children: &[],
            },
        ],
    ),
    (
        "usage",
        &[
            ArgSpec {
                value: "status",
                description: "show the status and syntax",
                children: &[],
            },
            ArgSpec {
                value: "on",
                description: "show the bar",
                children: &[],
            },
            ArgSpec {
                value: "off",
                description: "hide the bar",
                children: &[],
            },
            ArgSpec {
                value: "user",
                description: "user whose spend to show",
                children: &[],
            },
            ArgSpec {
                value: "key",
                description: "usage API key",
                children: &[],
            },
            ArgSpec {
                value: "budget",
                description: "monthly budget",
                children: &[],
            },
            ArgSpec {
                value: "currency",
                description: "budget currency",
                children: &[
                    ArgSpec {
                        value: "usd",
                        description: "US dollars",
                        children: &[],
                    },
                    ArgSpec {
                        value: "cny",
                        description: "Chinese yuan",
                        children: &[],
                    },
                ],
            },
            ArgSpec {
                value: "metadata",
                description: "metadata key holding the user",
                children: &[],
            },
        ],
    ),
    ("plugins", PLUGINS_ARGS),
    ("plugin", PLUGINS_ARGS),
    (
        "thinking",
        &[
            ArgSpec {
                value: "auto",
                description: "let the model decide",
                children: &[],
            },
            ArgSpec {
                value: "off",
                description: "disable reasoning",
                children: &[],
            },
            ArgSpec {
                value: "low",
                description: "low effort",
                children: &[],
            },
            ArgSpec {
                value: "medium",
                description: "medium effort",
                children: &[],
            },
            ArgSpec {
                value: "high",
                description: "high effort",
                children: &[],
            },
        ],
    ),
    (
        "trust",
        &[
            ArgSpec {
                value: "show",
                description: "show the saved decision",
                children: &[],
            },
            ArgSpec {
                value: "on",
                description: "trust this project",
                children: &[],
            },
            ArgSpec {
                value: "off",
                description: "decline this project",
                children: &[],
            },
        ],
    ),
];

fn command_args(name: &str) -> &'static [ArgSpec] {
    COMMAND_ARGS
        .iter()
        .find(|(command, _)| *command == name)
        .map(|(_, args)| *args)
        .unwrap_or(&[])
}

/// Candidates for the argument being typed after a slash command, walking the
/// fixed grammar for builtins and offering provider names for the login
/// commands. Empty when the command takes free text or has no completion.
fn argument_suggestions(command: &str, rest: &str) -> Vec<CommandHint> {
    let mut tokens: Vec<&str> = rest.split_whitespace().collect();
    let starts_new_token = rest.chars().last().map(char::is_whitespace).unwrap_or(true);
    let prefix = if starts_new_token {
        ""
    } else {
        tokens.pop().unwrap_or("")
    };
    let prefix = prefix.to_ascii_lowercase();

    if tokens.is_empty() && matches!(command, "connect" | "login" | "logout") {
        return crate::auth::KNOWN_PROVIDERS
            .iter()
            .filter(|provider| provider.name.starts_with(&prefix))
            .map(|provider| CommandHint {
                name: provider.name.to_string(),
                description: provider.label.to_string(),
            })
            .collect();
    }

    let mut node = command_args(command);
    for token in &tokens {
        let Some(child) = node
            .iter()
            .find(|spec| spec.value.eq_ignore_ascii_case(token))
        else {
            return Vec::new();
        };
        node = child.children;
    }
    node.iter()
        .filter(|spec| spec.value.starts_with(&prefix))
        .map(|spec| CommandHint {
            name: spec.value.to_string(),
            description: spec.description.to_string(),
        })
        .collect()
}

/// The `start..end` byte range of the whitespace-delimited token at the cursor,
/// used to replace just that token when completing a command argument.
fn slash_arg_token_bounds(input: &str, cursor: usize) -> (usize, usize) {
    let before = &input[..cursor];
    let start = before
        .char_indices()
        .rev()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index + ch.len_utf8()))
        .unwrap_or(0);
    let tail = &input[cursor..];
    let end = tail
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(cursor + index))
        .unwrap_or(input.len());
    (start, end)
}

/// Recomputes slash-command or `@path` suggestions for the current input.
fn refresh_suggestions(app: &mut App, config: &Config) {
    app.suggestions.clear();
    app.suggestion_index = 0;
    if app.connect.is_some()
        || app.models.is_some()
        || app.sessions.is_some()
        || app.marketplaces.is_some()
        || app.trust.is_some()
        || app.busy
    {
        return;
    }
    if let Some((_, _, query)) = active_file_query(&app.input, app.input_cursor) {
        let query = query.to_ascii_lowercase();
        let paths = app
            .workspace_paths
            .get_or_insert_with(|| crate::tools::workspace_paths(Path::new(&app.cwd)));
        app.suggestions = paths
            .iter()
            .filter(|path| path.to_ascii_lowercase().contains(&query))
            .take(200)
            .map(|path| CommandHint {
                name: path.clone(),
                description: String::new(),
            })
            .collect();
        return;
    }
    let Some(query) = app.input[..app.input_cursor].strip_prefix('/') else {
        return;
    };
    if let Some((command, rest)) = query.split_once(char::is_whitespace) {
        app.suggestions = argument_suggestions(&command.to_ascii_lowercase(), rest);
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
    for skill in &config.ecosystem.skills {
        hints.push(CommandHint {
            name: format!("skill:{}", skill.name),
            description: skill.description.clone().unwrap_or_default(),
        });
    }
    app.suggestions = hints
        .into_iter()
        .filter(|hint| hint.name.to_ascii_lowercase().starts_with(&query))
        .collect();
}

/// Returns the byte range and query for the `@path` token at the cursor.
fn active_file_query(input: &str, cursor: usize) -> Option<(usize, usize, &str)> {
    let before = input.get(..cursor)?;
    let start = before
        .char_indices()
        .rev()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(index + ch.len_utf8()))
        .unwrap_or(0);
    let query = before.get(start..)?.strip_prefix('@')?;
    let tail = input.get(cursor..)?;
    let end = tail
        .char_indices()
        .find_map(|(index, ch)| ch.is_whitespace().then_some(cursor + index))
        .unwrap_or(input.len());
    Some((start, end, query))
}

/// Applies the selected command or file suggestion to the input.
fn complete_suggestion(app: &mut App) -> bool {
    let Some(name) = app
        .suggestions
        .get(app.suggestion_index)
        .map(|hint| hint.name.clone())
    else {
        return false;
    };
    if app.input.starts_with('/') {
        if app.input[..app.input_cursor].contains(char::is_whitespace) {
            let (start, end) = slash_arg_token_bounds(&app.input, app.input_cursor);
            let completed = format!("{name} ");
            let replace_end = if app.input[end..].starts_with(' ') {
                end + 1
            } else {
                end
            };
            if app.input.get(start..replace_end) == Some(completed.as_str()) {
                return false;
            }
            app.input.replace_range(start..replace_end, &completed);
            app.input_cursor = start + completed.len();
            return true;
        }
        let completed = format!("/{name}");
        let end = app
            .input
            .find(char::is_whitespace)
            .unwrap_or(app.input.len());
        if app.input.get(..end) == Some(completed.as_str()) {
            return false;
        }
        app.input.replace_range(..end, &completed);
        app.input_cursor = completed.len();
    } else if let Some((start, end, _)) = active_file_query(&app.input, app.input_cursor) {
        let replace_end = if app.input[end..].starts_with(' ') {
            end + 1
        } else {
            end
        };
        let completed = format!("@{name} ");
        app.input.replace_range(start..replace_end, &completed);
        app.input_cursor = start + completed.len();
    } else {
        return false;
    }
    true
}

fn handle_models_key(key: KeyEvent, app: &mut App, config: &mut Config) {
    let Some(mut state) = app.models.take() else {
        return;
    };
    let mut keep = true;
    match key.code {
        KeyCode::Esc => {
            keep = false;
            app.show_status("model unchanged");
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            keep = false;
            app.show_status("model unchanged");
        }
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(choice) = state.selected_model().cloned() {
                match select_model(app, config, &choice) {
                    Ok(()) => {
                        if let Err(err) =
                            Config::set_default_model_at(&Config::config_path(), &choice.model)
                        {
                            state.error = Some(format!("{err:#}"));
                        } else {
                            config.default_model = Some(choice.model.clone());
                            app.items
                                .push(ChatItem::Info(format!("default model: {}", choice.model)));
                            app.status = "ready".to_string();
                            keep = false;
                        }
                    }
                    Err(err) => state.error = Some(format!("{err:#}")),
                }
            }
        }
        KeyCode::Up => {
            let len = state.filtered().len();
            if len > 0 {
                state.selected = if state.selected == 0 {
                    len - 1
                } else {
                    state.selected - 1
                };
            }
        }
        KeyCode::Down => {
            let len = state.filtered().len();
            if len > 0 {
                state.selected = if state.selected + 1 >= len {
                    0
                } else {
                    state.selected + 1
                };
            }
        }
        KeyCode::Backspace => {
            state.filter.pop();
            state.selected = 0;
        }
        KeyCode::Enter => {
            if let Some(choice) = state.selected_model().cloned() {
                match select_model(app, config, &choice) {
                    Ok(()) => {
                        app.items.push(ChatItem::Info(format!(
                            "model set to {} [{}]",
                            choice.model, choice.provider
                        )));
                        app.status = "ready".to_string();
                        keep = false;
                    }
                    Err(err) => state.error = Some(format!("{err:#}")),
                }
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

fn handle_sessions_key(key: KeyEvent, app: &mut App, cwd: &Path, session: &mut Option<SessionLog>) {
    let Some(mut state) = app.sessions.take() else {
        return;
    };
    let mut keep = true;

    if state.confirm_delete {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Some(summary) = state.selected_session() {
                    match SessionLog::delete(cwd, &summary.id) {
                        Ok(()) => {
                            app.items
                                .push(ChatItem::Info(format!("deleted session {}", summary.id)));
                            match SessionLog::list(cwd) {
                                Ok(sessions) => state = SessionsState::ready(sessions),
                                Err(err) => state.error = Some(format!("{err:#}")),
                            }
                            state.selected = 0;
                        }
                        Err(err) => state.error = Some(format!("{err:#}")),
                    }
                }
                state.confirm_delete = false;
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                state.confirm_delete = false;
            }
            _ => {}
        }
        if keep {
            app.sessions = Some(state);
        }
        return;
    }

    if state.renaming {
        match key.code {
            KeyCode::Esc => {
                state.renaming = false;
                state.rename_input.clear();
            }
            KeyCode::Enter => {
                let name = state.rename_input.trim().to_string();
                if !name.is_empty() {
                    if let Some(summary) = state.selected_session() {
                        match SessionLog::rename(cwd, &summary.id, &name) {
                            Ok(()) => {
                                app.items.push(ChatItem::Info(format!(
                                    "renamed session {} to `{name}`",
                                    summary.id
                                )));
                                match SessionLog::list(cwd) {
                                    Ok(sessions) => state = SessionsState::ready(sessions),
                                    Err(err) => state.error = Some(format!("{err:#}")),
                                }
                            }
                            Err(err) => state.error = Some(format!("{err:#}")),
                        }
                    }
                }
                state.renaming = false;
                state.rename_input.clear();
            }
            KeyCode::Backspace => {
                state.rename_input.pop();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                state.rename_input.push(c);
            }
            _ => {}
        }
        if keep {
            app.sessions = Some(state);
        }
        return;
    }

    match key.code {
        KeyCode::Esc => {
            keep = false;
            app.show_status("session unchanged");
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
        KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.show_paths = !state.show_paths;
        }
        KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.newest_first = !state.newest_first;
        }
        KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.named_only = !state.named_only;
            state.selected = 0;
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(summary) = state.selected_session() {
                state.renaming = true;
                state.rename_input = summary.name.clone().unwrap_or_default();
            }
        }
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.selected_session().is_some() {
                state.confirm_delete = true;
            }
        }
        KeyCode::Enter => {
            if let Some(summary) = state.selected_session() {
                match SessionLog::open(summary.path.clone()) {
                    Ok(log) => switch_session(app, session, log),
                    Err(err) => {
                        app.items.push(ChatItem::Error(format!("session: {err:#}")));
                        app.status = "ready".to_string();
                    }
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
        app.sessions = Some(state);
    }
}

/// Reloads the marketplace list in place after an action changes the state on
/// disk, keeping the current selection where possible.
fn refresh_marketplace_state(state: &mut MarketplacesState) {
    match crate::plugin_registry::marketplace_overview() {
        Ok(all) => {
            state.all = all;
            state.clamp_selection();
        }
        Err(err) => state.error = Some(format!("{err:#}")),
    }
}

fn handle_marketplace_outcome(outcome: MarketplaceOutcome, app: &mut App) {
    let (text, is_error) = match outcome {
        MarketplaceOutcome::Message(text) => (text, false),
        MarketplaceOutcome::Error(text) => (text, true),
    };
    if let Some(state) = app.marketplaces.as_mut() {
        state.busy = false;
        if is_error {
            state.error = Some(text);
        } else {
            state.message = Some(text);
            state.error = None;
        }
        refresh_marketplace_state(state);
    } else if is_error {
        app.items.push(ChatItem::Error(text));
    } else {
        app.items.push(ChatItem::Info(text));
        app.auto_scroll = true;
    }
    app.status = "ready".to_string();
}

type MarketplacesTx = UnboundedSender<MarketplaceOutcome>;

fn handle_marketplaces_key(key: KeyEvent, app: &mut App, marketplaces_tx: &MarketplacesTx) {
    let Some(mut state) = app.marketplaces.take() else {
        return;
    };
    let mut keep = true;

    if state.adding {
        match key.code {
            KeyCode::Esc => {
                state.adding = false;
                state.add_input.clear();
            }
            KeyCode::Enter => {
                let source = state.add_input.trim().to_string();
                state.adding = false;
                state.add_input.clear();
                if !source.is_empty() {
                    state.busy = true;
                    state.message = None;
                    state.error = None;
                    let tx = marketplaces_tx.clone();
                    tokio::spawn(async move {
                        let result = crate::plugin_registry::add_marketplace(&source).await;
                        let _ = tx.send(match result {
                            Ok(text) => MarketplaceOutcome::Message(text),
                            Err(err) => MarketplaceOutcome::Error(format!("{err:#}")),
                        });
                    });
                }
            }
            KeyCode::Backspace => {
                state.add_input.pop();
            }
            KeyCode::Char(c)
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                state.add_input.push(c);
            }
            _ => {}
        }
        app.marketplaces = Some(state);
        return;
    }

    if state.confirm_remove {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') | KeyCode::Enter => {
                if let Some(marketplace) = state.selected_marketplace().cloned() {
                    match crate::plugin_registry::remove_marketplace(&marketplace.name) {
                        Ok(text) => {
                            state.message = Some(text);
                            state.error = None;
                            state.busy = false;
                            refresh_marketplace_state(&mut state);
                        }
                        Err(err) => state.error = Some(format!("{err:#}")),
                    }
                }
                state.confirm_remove = false;
            }
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                state.confirm_remove = false;
            }
            _ => {}
        }
        app.marketplaces = Some(state);
        return;
    }

    match key.code {
        KeyCode::Esc => {
            keep = false;
            app.show_status("marketplaces closed");
        }
        KeyCode::Up => match state.pane {
            MarketplacePane::Marketplaces => {
                let len = state.marketplaces().len();
                if len > 0 {
                    state.selected = if state.selected == 0 {
                        len - 1
                    } else {
                        state.selected - 1
                    };
                }
            }
            MarketplacePane::Plugins => {
                let len = state.plugins().len();
                if len > 0 {
                    state.plugin_selected = if state.plugin_selected == 0 {
                        len - 1
                    } else {
                        state.plugin_selected - 1
                    };
                }
            }
        },
        KeyCode::Down => match state.pane {
            MarketplacePane::Marketplaces => {
                let len = state.marketplaces().len();
                if len > 0 {
                    state.selected = (state.selected + 1) % len;
                }
            }
            MarketplacePane::Plugins => {
                let len = state.plugins().len();
                if len > 0 {
                    state.plugin_selected = (state.plugin_selected + 1) % len;
                }
            }
        },
        KeyCode::Tab | KeyCode::BackTab => {
            // The filter targets the active pane, so switching panes drops it.
            // Keep the selected marketplace stable across that reset.
            let selected = state.selected_marketplace().map(|mp| mp.name.clone());
            state.pane = match state.pane {
                MarketplacePane::Marketplaces => MarketplacePane::Plugins,
                MarketplacePane::Plugins => MarketplacePane::Marketplaces,
            };
            state.filter.clear();
            if let Some(name) = selected {
                if let Some(index) = state.all.iter().position(|mp| mp.name == name) {
                    state.selected = index;
                }
            }
            state.clamp_selection();
        }
        KeyCode::Backspace => {
            state.filter.pop();
            state.clamp_selection();
        }
        KeyCode::Char('a') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.adding = true;
            state.add_input.clear();
        }
        KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if state.selected_marketplace().is_some() {
                state.confirm_remove = true;
            }
        }
        KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            state.busy = false;
            state.message = None;
            state.error = None;
            refresh_marketplace_state(&mut state);
        }
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            if let Some(marketplace) = state.selected_marketplace() {
                let name = marketplace.name.clone();
                state.busy = true;
                state.message = None;
                state.error = None;
                let tx = marketplaces_tx.clone();
                tokio::spawn(async move {
                    let result = crate::plugin_registry::update_marketplace(&name).await;
                    let _ = tx.send(match result {
                        Ok(text) => MarketplaceOutcome::Message(text),
                        Err(err) => MarketplaceOutcome::Error(format!("{err:#}")),
                    });
                });
            }
        }
        KeyCode::Enter => match state.pane {
            MarketplacePane::Marketplaces => {
                if !state.plugins().is_empty() {
                    state.pane = MarketplacePane::Plugins;
                    state.plugin_selected = 0;
                    state.filter.clear();
                }
            }
            MarketplacePane::Plugins => {
                if let Some(plugin) = state.selected_plugin() {
                    let marketplace = state.selected_marketplace().map(|mp| mp.name.clone());
                    if plugin.installed {
                        match crate::plugin_registry::set_enabled(&plugin.name, !plugin.enabled) {
                            Ok(text) => {
                                state.message = Some(text);
                                state.error = None;
                                state.busy = false;
                                refresh_marketplace_state(&mut state);
                            }
                            Err(err) => state.error = Some(format!("{err:#}")),
                        }
                    } else if let Some(marketplace) = marketplace {
                        state.busy = true;
                        state.message = None;
                        state.error = None;
                        let tx = marketplaces_tx.clone();
                        let name = plugin.name.clone();
                        tokio::spawn(async move {
                            let result =
                                crate::plugin_registry::install(&name, Some(&marketplace)).await;
                            let _ = tx.send(match result {
                                Ok(text) => MarketplaceOutcome::Message(text),
                                Err(err) => MarketplaceOutcome::Error(format!("{err:#}")),
                            });
                        });
                    }
                }
            }
        },
        KeyCode::Char(c)
            if !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
        {
            state.filter.push(c);
            state.clamp_selection();
        }
        _ => {}
    }
    if keep {
        app.marketplaces = Some(state);
    }
}

fn switch_session(app: &mut App, session: &mut Option<SessionLog>, log: SessionLog) {
    match log.messages() {
        Ok(messages) => {
            let id = log.id().to_string();
            app.items.clear();
            app.input_history.clear();
            app.attachments.clear();
            app.steering = crate::agent::Steering::new();
            app.follow_ups = crate::agent::Steering::new();
            app.assistant_open = false;
            let count = messages.len();
            restore_history(
                app,
                messages,
                format!(
                    "resumed session {id} ({count} message{})",
                    if count == 1 { "" } else { "s" }
                ),
            );
            app.session_name = log.name();
            let totals = log.usage_totals();
            app.tokens_in = totals.input;
            app.tokens_out = totals.output;
            app.tokens_cache_read = totals.cache_read;
            app.tokens_cache_write = totals.cache_write;
            app.cost = totals.cost;
            app.cache_hit_rate = totals.cache_hit_rate;
            app.context_used = 0;
            app.invalidate_render_cache();
            app.auto_scroll = true;
            *session = Some(log);
            app.show_status(format!("resumed {id}"));
        }
        Err(err) => {
            app.items.push(ChatItem::Error(format!("session: {err:#}")));
            app.status = "ready".to_string();
        }
    }
}

fn restore_history(app: &mut App, messages: Vec<Message>, resumed: String) {
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
                if let Some(text) = thinking_text(message) {
                    app.items.push(ChatItem::Thinking { text, millis: None });
                }
                if let Some(content) = message.display() {
                    app.items.push(ChatItem::Assistant(content));
                }
            }
            _ => {}
        }
    }
    app.history = messages;
    app.items.push(ChatItem::Info(resumed));
}

/// Reasoning text a stored assistant message carries, if any. Redacted
/// thinking blocks hold no text and are skipped.
fn thinking_text(message: &Message) -> Option<String> {
    let text = message
        .thinking
        .as_ref()?
        .iter()
        .filter_map(|block| block["thinking"].as_str())
        .collect::<Vec<_>>()
        .join("\n");
    if text.trim().is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Tone for an MCP connection state, so the listing colors healthy servers,
/// caution states, and failures differently.
fn mcp_tone(status: &McpStatus) -> Tone {
    match status {
        McpStatus::Connected => Tone::Success,
        McpStatus::NeedsAuth | McpStatus::NeedsTrust => Tone::Warning,
        McpStatus::Disabled => Tone::Dim,
        McpStatus::Error(_) => Tone::Error,
    }
}

fn mcp_listing(statuses: &[(String, String, McpStatus)]) -> ChatItem {
    if statuses.is_empty() {
        return ChatItem::Info("no MCP servers configured".to_string());
    }
    let rows = statuses
        .iter()
        .map(|(name, source, status)| {
            ListRow::new(name.clone())
                .status(status.to_string(), mcp_tone(status))
                .detail(source.clone())
        })
        .collect();
    ChatItem::Listing {
        title: format!("MCP servers ({})", statuses.len()),
        rows,
    }
}

/// Installed plugins as a structured listing for `/plugins`, including the
/// marketplace, version, description, and on-disk path of each entry.
fn plugins_listing() -> Result<ChatItem> {
    let state = crate::plugin_registry::load_state()?;
    if state.plugins.is_empty() {
        return Ok(ChatItem::Info(
            "no plugins installed — use /plugins install <name>@<marketplace>".to_string(),
        ));
    }
    let rows = state
        .plugins
        .values()
        .map(|plugin| {
            let marketplace = plugin
                .marketplace
                .as_deref()
                .map(|marketplace| format!("@{marketplace}"))
                .unwrap_or_default();
            let version = plugin
                .version
                .as_deref()
                .map(|version| format!(" v{version}"))
                .unwrap_or_default();
            let (label, tone) = if plugin.enabled {
                (format!("enabled{version}"), Tone::Success)
            } else {
                (format!("disabled{version}"), Tone::Dim)
            };
            let mut row = ListRow::new(format!("{}{marketplace}", plugin.name)).status(label, tone);
            if let Some(description) = &plugin.description {
                row = row.note(description.clone());
            }
            row.note(format!("path: {}", plugin.path.display()))
        })
        .collect();
    Ok(ChatItem::Listing {
        title: format!("Plugins ({})", state.plugins.len()),
        rows,
    })
}

fn handle_model_result(catalogs: ModelCatalogs, app: &mut App) {
    let Some(state) = app.models.as_ref() else {
        return;
    };
    let provider = state.provider.clone();
    let current = state.current.clone();
    let default = state.default.clone();
    if catalogs.choices.is_empty() {
        app.models = None;
        let message = if catalogs.errors.is_empty() {
            "provider returned no models".to_string()
        } else {
            format!("models: {}", catalogs.errors.join("; "))
        };
        app.items.push(ChatItem::Error(message));
        app.status = "ready".to_string();
        return;
    }
    if !catalogs.errors.is_empty() {
        app.items.push(ChatItem::Info(format!(
            "some providers returned no models — {}",
            catalogs.errors.join("; ")
        )));
        app.auto_scroll = true;
    }
    let count = catalogs.choices.len();
    app.status = format!("{count} model(s) — pick one");
    app.models = Some(ModelsState::ready(
        catalogs.choices,
        provider,
        current,
        default,
    ));
}

fn handle_paste(text: String, app: &mut App) {
    let mut text = text;
    text.retain(|ch| ch != '\r' && ch != '\n');
    if let Some(state) = app.connect.as_mut() {
        state.input.push_str(&text);
    } else if let Some(state) = app.models.as_mut() {
        state.filter.push_str(&text);
        state.selected = 0;
    } else if let Some(state) = app.sessions.as_mut() {
        if state.renaming {
            state.rename_input.push_str(&text);
        } else {
            state.filter.push_str(&text);
            state.selected = 0;
        }
    } else {
        app.insert_input(&text);
        app.auto_scroll = true;
    }
}

/// Routes pointer interaction to suggestions, otherwise scrolling the chat or
/// starting a text selection.
fn handle_mouse(mouse: MouseEvent, app: &mut App, terminal_area: Rect) {
    match mouse.kind {
        MouseEventKind::ScrollUp => {
            if ui::suggestion_index_at(app, terminal_area, mouse.column, mouse.row).is_some() {
                app.suggestion_index = app.suggestion_index.saturating_sub(1);
            } else {
                app.scroll_up(3);
            }
        }
        MouseEventKind::ScrollDown => {
            if ui::suggestion_index_at(app, terminal_area, mouse.column, mouse.row).is_some() {
                app.suggestion_index =
                    (app.suggestion_index + 1).min(app.suggestions.len().saturating_sub(1));
            } else {
                app.scroll_down(3);
            }
        }
        MouseEventKind::Moved => {
            if let Some(index) =
                ui::suggestion_index_at(app, terminal_area, mouse.column, mouse.row)
            {
                app.suggestion_index = index;
            }
        }
        MouseEventKind::Down(MouseButton::Left) => {
            app.selection = None;
            if let Some(index) =
                ui::suggestion_index_at(app, terminal_area, mouse.column, mouse.row)
            {
                app.suggestion_index = index;
                if complete_suggestion(app) {
                    app.suggestions.clear();
                    app.suggestion_index = 0;
                }
            } else if let Some((line, column)) =
                ui::message_position_at(app, terminal_area, mouse.column, mouse.row)
            {
                app.selection = Some(Selection::new(line, column));
            }
        }
        MouseEventKind::Drag(MouseButton::Left) => {
            if app.selection.is_some() {
                if let Some((line, column)) =
                    ui::message_position_at(app, terminal_area, mouse.column, mouse.row)
                {
                    if let Some(selection) = app.selection.as_mut() {
                        selection.cursor = (line, column);
                    }
                }
            }
        }
        MouseEventKind::Up(MouseButton::Left) => {
            // Copy-on-select: a drag copies the text and clears the highlight; a
            // plain click just drops the (empty) selection.
            if let Some(selection) = app.selection {
                if selection.anchor == selection.cursor {
                    app.selection = None;
                } else {
                    copy_selection(app);
                }
            }
        }
        _ => {}
    }
}

/// Queues a message typed while the agent is busy. Attachments gathered with
/// `Ctrl+V` and `@path` image/pdf references travel with it, so a steer sent
/// mid-run contributes the same context the idle send path would.
fn queue_while_busy(app: &mut App, raw: &str, cwd: &Path, follow_up: bool) {
    let mut parts = app.take_attachment_parts();
    for path in media::referenced_attachments(raw, cwd) {
        match media::load_attachment(&path) {
            Ok(part) => parts.push(part),
            Err(err) => app
                .items
                .push(ChatItem::Error(format!("attachment: {err:#}"))),
        }
    }
    if raw.is_empty() && parts.is_empty() {
        return;
    }
    let media_count = parts.len();
    app.remember_input(raw);
    app.clear_input();
    let shown = if media_count > 0 {
        format!("{raw}\n[{media_count} attachment(s)]")
    } else {
        raw.to_string()
    };
    app.items.push(ChatItem::User(shown));
    app.auto_scroll = true;
    let message = if parts.is_empty() {
        Message::user(raw)
    } else {
        Message::user_parts(raw, parts)
    };
    if follow_up {
        app.follow_ups.push(message);
        app.status = "queued follow-up...".to_string();
    } else {
        app.steering.push(message);
        app.status = "queued guidance...".to_string();
    }
}

/// Pulls every queued message back into the editor so it can be edited or
/// extended before it is sent, like Pi's `app.message.dequeue`. The queued text
/// leads and whatever is already in the editor follows, so typing while busy
/// then dequeuing reads as one message with the new text appended.
fn dequeue_messages(app: &mut App) {
    let mut queued = app.steering.drain();
    queued.extend(app.follow_ups.drain());
    if queued.is_empty() {
        app.show_status("no queued messages to restore");
        return;
    }

    // A queued message's images/PDFs have no text form, so put them back with
    // the pending attachments rather than dropping them when the text is
    // edited.
    let mut restored_media = 0usize;
    for message in &queued {
        if let Some(crate::llm::MessageContent::Parts(parts)) = &message.content {
            for part in parts {
                if !matches!(part, crate::llm::ContentPart::Text { .. })
                    && app.add_attachment(part.clone())
                {
                    restored_media += 1;
                }
            }
        }
    }

    let texts: Vec<String> = queued.iter().map(queued_text).collect();
    let current = app.input.trim_matches('\n');
    let combined = if current.trim().is_empty() {
        texts.join("\n\n")
    } else {
        format!("{}\n\n{current}", texts.join("\n\n"))
    };
    app.input = combined;
    app.input_cursor = app.input.len();

    // Queued messages are shown in the transcript as they are typed, so drop
    // those entries: they were never sent and are editable again. The entry may
    // carry a `[N attachment(s)]` suffix the message text does not.
    for text in texts.iter().rev() {
        let with_media = format!("{text}\n[");
        if let Some(index) = app.items.iter().rposition(|item| {
            matches!(item, ChatItem::User(shown) if shown == text || shown.starts_with(&with_media))
        }) {
            app.items.remove(index);
            app.mark_render_dirty(index);
        }
    }

    let count = texts.len();
    let media = if restored_media > 0 {
        format!(" and {restored_media} attachment(s)")
    } else {
        String::new()
    };
    app.show_status(format!(
        "restored {count} queued message{}{media} to the editor",
        if count == 1 { "" } else { "s" }
    ));
}

/// The user-visible text of a message, ignoring `[image]`/`[file]` markers that
/// [`Message::display`] adds for media parts.
fn queued_text(message: &Message) -> String {
    match &message.content {
        Some(crate::llm::MessageContent::Text(text)) => text.clone(),
        Some(crate::llm::MessageContent::Parts(parts)) => parts
            .iter()
            .filter_map(|part| match part {
                crate::llm::ContentPart::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
        None => String::new(),
    }
}

/// Copies the active mouse selection, if any, reporting the result. Returns
/// whether a selection was present.
fn copy_selection(app: &mut App) -> bool {
    let Some(selection) = app.selection.take() else {
        return false;
    };
    let text = selection.text(&app.lines);
    if text.trim().is_empty() {
        app.show_status("nothing to copy");
        return true;
    }
    match crate::clipboard::copy(&text) {
        Ok(()) => app.show_status(format!("copied {} chars", text.chars().count())),
        Err(err) => app.items.push(ChatItem::Error(format!("copy: {err:#}"))),
    }
    true
}

/// `/copy` copies the last assistant message; `/copy all` copies the whole
/// visible transcript.
fn copy_command(app: &mut App, all: bool) {
    let text = if all {
        app.lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
            .trim_matches('\n')
            .to_string()
    } else {
        match app.items.iter().rev().find_map(|item| match item {
            ChatItem::Assistant(text) if !text.trim().is_empty() => Some(text.clone()),
            _ => None,
        }) {
            Some(text) => text,
            None => {
                app.show_status("nothing to copy");
                return;
            }
        }
    };
    if text.trim().is_empty() {
        app.show_status("nothing to copy");
        return;
    }
    match crate::clipboard::copy(&text) {
        Ok(()) => app.show_status(format!("copied {} chars", text.chars().count())),
        Err(err) => app.items.push(ChatItem::Error(format!("copy: {err:#}"))),
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

/// The model catalogs fetched from every logged-in provider, plus the providers
/// that failed so one stale credential cannot hide the others.
#[derive(Debug, Default)]
struct ModelCatalogs {
    choices: Vec<ModelChoice>,
    errors: Vec<String>,
}

/// The providers to list models for: the active one first, then every other
/// provider with a stored credential, each with a config pointed at it. A
/// custom provider is only included when a remembered endpoint gives it a base
/// URL of its own, since otherwise its catalog would be queried against the
/// active provider's endpoint.
fn model_providers(config: &Config) -> Vec<(String, Config)> {
    crate::config::provider_configs(config)
}

/// Switches the running session to a provider that already has a stored
/// credential: its key, model, and endpoint are applied, the choice is saved
/// for the next launch, and the footer follows.
fn switch_provider(app: &mut App, config: &mut Config, provider: &str) -> Result<String> {
    let (name, key) = crate::auth::select_stored(provider)?;
    config.apply_provider(&name, &key);
    config.persist_selection_at(&Config::config_path())?;
    apply_model_state(app, config);
    Ok(name)
}

/// Refreshes the footer and picker state after the active provider or model
/// changed.
fn apply_model_state(app: &mut App, config: &Config) {
    app.model = config.model.clone();
    app.provider = config.provider.clone();
    app.show_thinking = config.supports_reasoning();
    app.available_providers = available_providers(config);
}

/// Selects a model from the picker, switching providers first when the chosen
/// model belongs to another logged-in one, and remembers it for that provider.
fn select_model(app: &mut App, config: &mut Config, choice: &ModelChoice) -> Result<()> {
    if crate::auth::canonical_provider(&config.provider)
        != crate::auth::canonical_provider(&choice.provider)
    {
        switch_provider(app, config, &choice.provider)?;
    }
    config.model = choice.model.clone();
    config.persist_selection_at(&Config::config_path())?;
    apply_model_state(app, config);
    Ok(())
}

/// Adopts another logged-in provider after the active one was logged out, or
/// clears the credential when none is left.
fn logout_active_provider(app: &mut App, config: &mut Config, provider: &str) -> String {
    let Some(next) = crate::auth::stored_providers().into_iter().next() else {
        config.api_key.clear();
        config.provider.clear();
        apply_model_state(app, config);
        return format!("logged out of {provider} — run /login to reconnect");
    };
    match switch_provider(app, config, &next) {
        Ok(name) => format!(
            "logged out of {provider} — switched to {} · model {}",
            crate::auth::provider_label(&name),
            config.model
        ),
        Err(err) => format!("logged out of {provider}; {err:#}"),
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
            app.show_status("connect cancelled");
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
                    if value.is_empty() && !state.is_connected(&provider) {
                        state.error = Some("enter an API key".to_string());
                    } else {
                        let canonical = crate::auth::canonical_provider(&provider);
                        state.key = value;
                        // The active provider's live values win over its preset,
                        // which matters for an endpoint set by hand in the file.
                        if canonical == crate::auth::canonical_provider(&config.provider) {
                            state.model = config.model.clone();
                            state.base_url = config.base_url.clone();
                        } else {
                            state.model = config.model_for_provider(&canonical);
                            state.base_url = config.base_url_for_provider(&canonical);
                        }
                        state.portkey_config = if canonical == "portkey" {
                            config.portkey_config.clone()
                        } else {
                            String::new()
                        };
                        state.step = ConnectStep::Options {
                            provider: provider.clone(),
                        };
                        state.focus = ConnectField::Model;
                        state.input = state.model.clone();
                        state.error = None;
                    }
                }
                ConnectStep::Options { provider } => {
                    state.commit_focus();
                    let fields = ConnectState::option_fields(&provider);
                    let current = fields
                        .iter()
                        .position(|field| *field == state.focus)
                        .unwrap_or(0);
                    if current + 1 < fields.len() {
                        state.focus_field(fields[current + 1]);
                    } else {
                        match save_connect(app, config, &state, &provider) {
                            Ok(name) => {
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
        KeyCode::Backspace => match state.step.clone() {
            ConnectStep::Provider => {
                state.input.pop();
                state.error = None;
            }
            ConnectStep::Key { .. } => {
                if state.input.is_empty() {
                    state.step = ConnectStep::Provider;
                    state.error = None;
                } else {
                    state.input.pop();
                    state.error = None;
                }
            }
            ConnectStep::Options { provider } => {
                if state.input.is_empty() {
                    let fields = ConnectState::option_fields(&provider);
                    let current = fields
                        .iter()
                        .position(|field| *field == state.focus)
                        .unwrap_or(0);
                    if current == 0 {
                        state.step = ConnectStep::Key { provider };
                        state.input = state.key.clone();
                        state.error = None;
                    } else {
                        state.focus_field(fields[current - 1]);
                    }
                } else {
                    state.input.pop();
                    state.error = None;
                }
            }
        },
        KeyCode::Up => match state.step.clone() {
            ConnectStep::Provider if state.input.is_empty() => {
                state.selected = state.selected.saturating_sub(1);
            }
            ConnectStep::Options { provider } => {
                let fields = ConnectState::option_fields(&provider);
                let current = fields
                    .iter()
                    .position(|field| *field == state.focus)
                    .unwrap_or(0);
                if current > 0 {
                    state.focus_field(fields[current - 1]);
                }
            }
            _ => {}
        },
        KeyCode::Down => match state.step.clone() {
            ConnectStep::Provider if state.input.is_empty() => {
                state.selected = (state.selected + 1).min(crate::auth::KNOWN_PROVIDERS.len() - 1);
            }
            ConnectStep::Options { provider } => {
                let fields = ConnectState::option_fields(&provider);
                let current = fields
                    .iter()
                    .position(|field| *field == state.focus)
                    .unwrap_or(0);
                if current + 1 < fields.len() {
                    state.focus_field(fields[current + 1]);
                }
            }
            _ => {}
        },
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

/// Saves a login from the dialog: stores a new key (or reuses the stored one),
/// applies the optional model/endpoint/Config ID, and persists the selection.
fn save_connect(
    app: &mut App,
    config: &mut Config,
    state: &ConnectState,
    provider: &str,
) -> Result<String> {
    let (name, key) = if state.key.trim().is_empty() {
        crate::auth::select_stored(provider)?
    } else {
        let name = crate::auth::connect(provider, &state.key)?;
        (name, state.key.clone())
    };
    config.apply_provider(&name, &key);
    config.apply_login_options(&state.model, &state.base_url, &state.portkey_config);
    config.persist_selection_at(&Config::config_path())?;
    apply_model_state(app, config);
    Ok(name)
}

fn ensure_session<'a>(session: &'a mut Option<SessionLog>, cwd: &Path) -> Result<&'a SessionLog> {
    if session.is_none() {
        *session = Some(SessionLog::create(cwd)?);
    }
    Ok(session.as_ref().expect("session was just created"))
}

fn handle_agent_event(event: AgentEvent, app: &mut App) {
    match event {
        AgentEvent::ApprovalRequest { id, tool, detail } => {
            app.status = "waiting for approval".to_string();
            app.items.push(ChatItem::Info(format!(
                "approve `{tool}`? {APPROVAL_HINT}\n  {detail}"
            )));
            app.auto_scroll = true;
            app.pending_approval = Some(PendingApproval { id, tool });
        }
        AgentEvent::Text(delta) => {
            app.auto_scroll = true;
            // Clear any `retrying...` notice now that output is flowing again.
            app.status = "thinking...".to_string();
            app.push_assistant_delta(delta);
        }
        AgentEvent::ThinkingDelta(delta) => {
            app.auto_scroll = true;
            app.assistant_open = false;
            app.push_thinking_delta(delta);
            app.status = "thinking...".to_string();
        }
        AgentEvent::Thought { millis } => app.finish_thinking(millis),
        AgentEvent::ThoughtDone { .. } => {
            // The step's text is committed: a later step (or a retry's discard)
            // must not merge into or remove this step's bubble.
            app.assistant_open = false;
        }
        AgentEvent::Retrying {
            attempt,
            max,
            delay_ms,
        } => {
            app.discard_thinking();
            app.discard_assistant();
            app.status = format!(
                "retrying ({attempt}/{max}) in {}s...",
                delay_ms.div_ceil(1000).max(1)
            );
        }
        AgentEvent::SubagentActivity { agent, tool, args } => {
            // A `task` call is opaque until it returns, so the subagent's latest
            // activity stands in for the result it has not produced yet.
            let tools = app.subagent.as_ref().map_or(0, |state| state.tools) + 1;
            app.status = format!("{agent} · {tool}");
            app.subagent = Some(SubagentState {
                agent,
                tool,
                args,
                tools,
            });
        }
        AgentEvent::ToolCall { name, args } => {
            app.assistant_open = false;
            app.auto_scroll = true;
            app.running_tool = Some((name.clone(), std::time::Instant::now()));
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
            millis,
        } => {
            app.auto_scroll = true;
            app.running_tool = None;
            app.subagent = None;
            // The tool this question was about has resolved (answered, or denied
            // when the prompt timed out), so the composer stops asking for it.
            if app
                .pending_approval
                .as_ref()
                .is_some_and(|pending| pending.tool == name)
            {
                app.pending_approval = None;
            }
            app.resolve_tool(name, args, output, diff, millis);
            app.status = "thinking...".to_string();
        }
        AgentEvent::Usage {
            input,
            output,
            cache_read,
            cache_write,
            cost,
        } => {
            app.tokens_in = app.tokens_in.saturating_add(input);
            app.tokens_out = app.tokens_out.saturating_add(output);
            app.tokens_cache_read = app.tokens_cache_read.saturating_add(cache_read);
            app.tokens_cache_write = app.tokens_cache_write.saturating_add(cache_write);
            app.cost += cost;
            // `input` is the uncached prompt only; the cached prefix is still
            // part of the context that occupies the window.
            app.context_used = input.saturating_add(cache_read).saturating_add(cache_write);
            if cache_read + cache_write > 0 {
                let prompt = input + cache_read + cache_write;
                if prompt > 0 {
                    app.cache_hit_rate = Some(cache_read as f64 / prompt as f64 * 100.0);
                }
            }
        }
        AgentEvent::Compaction {
            summary,
            summarized,
            tokens_before,
            read_files,
            modified_files,
        } => {
            app.auto_scroll = true;
            app.items.push(ChatItem::Compaction {
                summary,
                summarized,
                tokens_before,
                read_files,
                modified_files,
            });
        }
        AgentEvent::Branch {
            history,
            prompt,
            message,
        } => {
            app.running_tool = None;
            app.busy = false;
            app.busy_since = None;
            app.pending_approval = None;
            app.assistant_open = false;
            app.status = "ready".to_string();
            app.reset_history(history);
            app.set_input(prompt);
            app.items.push(ChatItem::Info(message));
            app.auto_scroll = true;
        }
        AgentEvent::Error(message) => {
            // The turn produced nothing, so its partial reasoning goes with it.
            app.discard_thinking();
            app.items.push(ChatItem::Error(message));
        }
        AgentEvent::Finished(history) => {
            app.history = history;
            app.workspace_paths = None;
            app.running_tool = None;
            app.subagent = None;
            app.busy = false;
            app.busy_since = None;
            app.pending_approval = None;
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
    use crate::approvals::ApprovalStore;
    use crate::config::{Config, Reasoning};
    use std::path::Path;

    fn test_app() -> App {
        App::new(
            "test-model".to_string(),
            "/tmp".to_string(),
            Reasoning::Auto,
        )
    }

    #[test]
    fn approval_answers_parse_from_the_composer() {
        assert_eq!(parse_approval_answer("y"), Some(Decision::Once));
        assert_eq!(parse_approval_answer(" Yes "), Some(Decision::Once));
        assert_eq!(parse_approval_answer("a"), Some(Decision::Always));
        assert_eq!(parse_approval_answer("ALWAYS"), Some(Decision::Always));
        assert_eq!(
            parse_approval_answer("n"),
            Some(Decision::Deny { message: None })
        );
        assert_eq!(
            parse_approval_answer("deny"),
            Some(Decision::Deny { message: None })
        );
        // Anything else refuses the tool and reaches the agent as guidance.
        assert_eq!(
            parse_approval_answer("use a narrower command"),
            Some(Decision::Deny {
                message: Some("use a narrower command".to_string())
            })
        );
        assert_eq!(parse_approval_answer("   "), None);
    }

    fn approval_broker(dir: &Path) -> ApprovalBroker {
        ApprovalBroker::from_store(
            ApprovalStore::load_from(dir.join("approvals.json")),
            crate::approval::APPROVAL_TIMEOUT,
        )
    }

    fn approval_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_{name}_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn a_typed_always_is_remembered_for_the_project() {
        let dir = approval_dir("tui_approval_always");
        let broker = Arc::new(approval_broker(&dir));
        let (tx, mut rx) = unbounded_channel();
        let approve = broker.approver(&dir, tx, crate::agent::Steering::new());
        let answered = tokio::spawn(approve("bash".into(), "rm -rf /tmp/x".into()));

        let mut app = test_app();
        handle_agent_event(rx.recv().await.unwrap(), &mut app);
        let pending = app.pending_approval.as_ref().expect("a waiting approval");
        assert_eq!(pending.tool, "bash");
        assert!(app.items.iter().any(|item| matches!(
            item,
            ChatItem::Info(text) if text.contains("approve `bash`?") && text.contains("rm -rf /tmp/x")
        )));

        app.set_input("a".to_string());
        let decision = parse_approval_answer(&app.input).unwrap();
        answer_approval(&mut app, &broker, decision);

        assert!(answered.await.unwrap(), "always allow runs the tool");
        assert!(app.pending_approval.is_none());
        assert!(app.input.is_empty(), "the answer clears the composer");
        assert_eq!(broker.list(&dir), vec!["bash".to_string()]);
        assert!(app.items.iter().any(|item| matches!(
            item,
            ChatItem::Info(text) if text.contains("always allowed `bash`")
        )));

        // The stored rule answers the next request for the same tool without a
        // prompt, which is what keeps the three front-ends in step.
        let (tx, mut rx) = unbounded_channel();
        let approve = broker.approver(&dir, tx, crate::agent::Steering::new());
        assert!(approve("bash".into(), "ls".into()).await);
        assert!(rx.try_recv().is_err(), "no question the second time");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_typed_denial_reaches_the_agent_as_guidance() {
        let dir = approval_dir("tui_approval_deny");
        let broker = Arc::new(approval_broker(&dir));
        let steering = crate::agent::Steering::new();
        let (tx, mut rx) = unbounded_channel();
        let approve = broker.approver(&dir, tx, steering.clone());
        let answered = tokio::spawn(approve("bash".into(), "rm -rf /".into()));

        let mut app = test_app();
        handle_agent_event(rx.recv().await.unwrap(), &mut app);
        app.set_input("target just that directory".to_string());
        answer_approval_input(&mut app, &broker, "target just that directory");

        assert!(!answered.await.unwrap(), "a denial refuses the tool");
        let messages = steering.drain();
        assert_eq!(messages.len(), 1);
        assert_eq!(
            messages[0].display().as_deref(),
            Some("target just that directory")
        );

        // Esc denies without a message, and an empty line keeps the request
        // open with the hint.
        let broker = Arc::new(approval_broker(&dir));
        let (tx, mut rx) = unbounded_channel();
        let approve = broker.approver(&dir, tx, crate::agent::Steering::new());
        let answered = tokio::spawn(approve("bash".into(), "ls".into()));
        let mut app = test_app();
        handle_agent_event(rx.recv().await.unwrap(), &mut app);
        answer_approval_input(&mut app, &broker, "   ");
        assert!(
            app.pending_approval.is_some(),
            "an empty line answers nothing"
        );
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Status(text)) if text.contains("a = always")
        ));
        answer_approval(&mut app, &broker, Decision::Deny { message: None });
        assert!(!answered.await.unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn approvals_command_toggles_lists_and_clears() {
        let dir = approval_dir("tui_approvals_cmd");
        let path = dir.join("config.json");
        let store_path = dir.join("approvals.json");
        let mut store = ApprovalStore::load_from(store_path);
        store.allow(&dir, "bash").unwrap();
        let broker = ApprovalBroker::from_store(store, crate::approval::APPROVAL_TIMEOUT);

        let mut app = test_app();
        let mut config = Config::default();
        assert!(
            config.auto_approve,
            "gated tools run without asking by default"
        );

        handle_approvals_command(&mut app, &mut config, &broker, &dir, "/approvals on", &path);
        assert!(!config.auto_approve);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["auto_approve"], serde_json::json!(false));

        handle_approvals_command(&mut app, &mut config, &broker, &dir, "/approvals", &path);
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text))
                if text.contains("tool approvals: on") && text.contains("always allowed here: bash")
        ));

        handle_approvals_command(
            &mut app,
            &mut config,
            &broker,
            &dir,
            "/approvals off",
            &path,
        );
        assert!(config.auto_approve);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["auto_approve"], serde_json::json!(true));

        handle_approvals_command(
            &mut app,
            &mut config,
            &broker,
            &dir,
            "/approvals clear",
            &path,
        );
        assert!(broker.list(&dir).is_empty());
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text)) if text.contains("cleared this project's approvals")
        ));

        handle_approvals_command(
            &mut app,
            &mut config,
            &broker,
            &dir,
            "/approvals maybe",
            &path,
        );
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Error(text)) if text.contains("unknown /approvals option `maybe`")
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_question_stops_when_its_tool_resolves_or_the_turn_ends() {
        let dir = approval_dir("tui_approval_stale");
        let broker = Arc::new(approval_broker(&dir));
        let (tx, mut rx) = unbounded_channel();
        let approve = broker.approver(&dir, tx, crate::agent::Steering::new());
        let answered = tokio::spawn(approve("bash".into(), "ls".into()));

        let mut app = test_app();
        handle_agent_event(rx.recv().await.unwrap(), &mut app);
        assert!(app.pending_approval.is_some());

        // The tool resolving without an answer (the prompt timed out) ends the
        // question, so the composer is a prompt again rather than a stale one.
        handle_agent_event(
            AgentEvent::ToolResult {
                name: "bash".into(),
                args: "{}".into(),
                output: "denied".into(),
                diff: None,
                millis: 1,
            },
            &mut app,
        );
        assert!(app.pending_approval.is_none());

        // A turn that ends while the question is up (an abort) drops it too.
        let (tx, mut rx) = unbounded_channel();
        let approve = broker.approver(&dir, tx, crate::agent::Steering::new());
        let _unanswered = tokio::spawn(approve("write".into(), "file.txt".into()));
        handle_agent_event(rx.recv().await.unwrap(), &mut app);
        assert!(app.pending_approval.is_some());
        handle_agent_event(AgentEvent::Finished(Vec::new()), &mut app);
        assert!(app.pending_approval.is_none());

        answered.abort();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_finished_step_survives_a_later_retry() {
        let mut app = test_app();
        handle_agent_event(AgentEvent::Text("summary".into()), &mut app);
        // The step commits (`ThoughtDone`), so a later step that fails before
        // emitting anything and retries must not discard this reply.
        handle_agent_event(AgentEvent::ThoughtDone { millis: 5 }, &mut app);
        assert!(!app.assistant_open);
        handle_agent_event(
            AgentEvent::Retrying {
                attempt: 1,
                max: 3,
                delay_ms: 500,
            },
            &mut app,
        );
        assert_eq!(app.items.len(), 1);
        assert!(matches!(&app.items[0], ChatItem::Assistant(text) if text == "summary"));
    }

    #[test]
    fn a_retry_discards_the_step_in_progress() {
        let mut app = test_app();
        handle_agent_event(AgentEvent::Text("partial".into()), &mut app);
        handle_agent_event(
            AgentEvent::Retrying {
                attempt: 1,
                max: 3,
                delay_ms: 500,
            },
            &mut app,
        );
        assert!(app.items.is_empty());
    }

    #[test]
    fn subagent_activity_reports_progress_and_clears_on_result() {
        let mut app = test_app();
        handle_agent_event(
            AgentEvent::ToolCall {
                name: "task".into(),
                args: r#"{"prompt":"review","subagent_type":"rust-reviewer"}"#.into(),
            },
            &mut app,
        );
        assert_eq!(app.status, "running tool...");
        assert!(app.subagent.is_none());

        for (tool, args) in [
            ("grep", r#"{"pattern":"resolve_tool"}"#),
            ("read", r#"{"path":"src/tui/ui.rs"}"#),
        ] {
            handle_agent_event(
                AgentEvent::SubagentActivity {
                    agent: "rust-reviewer".into(),
                    tool: tool.into(),
                    args: args.into(),
                },
                &mut app,
            );
        }
        let state = app.subagent.as_ref().expect("activity tracked");
        assert_eq!(state.agent, "rust-reviewer");
        assert_eq!(state.tool, "read");
        assert_eq!(state.tools, 2);
        assert_eq!(app.status, "rust-reviewer · read");

        handle_agent_event(
            AgentEvent::ToolResult {
                name: "task".into(),
                args: "{}".into(),
                output: "the report".into(),
                diff: None,
                millis: 500,
            },
            &mut app,
        );
        assert!(app.subagent.is_none());
        assert!(app.running_tool.is_none());

        handle_agent_event(
            AgentEvent::SubagentActivity {
                agent: "rust-reviewer".into(),
                tool: "grep".into(),
                args: "{}".into(),
            },
            &mut app,
        );
        handle_agent_event(AgentEvent::Finished(vec![]), &mut app);
        assert!(app.subagent.is_none());
    }

    #[test]
    fn dequeue_returns_queued_messages_to_the_editor() {
        let mut app = test_app();
        app.busy = true;
        app.items.push(ChatItem::User("first".into()));
        app.items.push(ChatItem::User("second".into()));
        app.steering.push(Message::user("first"));
        app.follow_ups.push(Message::user("second"));
        app.input = "my extra context".into();
        assert_eq!(app.queued_count(), 2);

        dequeue_messages(&mut app);

        assert_eq!(app.queued_count(), 0);
        // Queued text leads so the typed text reads as appended context.
        assert_eq!(
            app.input, "first\n\nsecond\n\nmy extra context",
            "queued text should precede what was already typed"
        );
        assert_eq!(app.input_cursor, app.input.len());
        // The transcript entries were never sent, so they are gone.
        assert!(
            !app.items
                .iter()
                .any(|item| matches!(item, ChatItem::User(_))),
            "queued turns should be removed from the transcript"
        );
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Status(text)) if text == "restored 2 queued messages to the editor"
        ));
    }

    #[test]
    fn queued_steering_while_busy_keeps_attachments() {
        let mut app = test_app();
        app.busy = true;
        app.add_attachment(crate::llm::ContentPart::ImageUrl {
            image_url: crate::llm::ImageUrl {
                url: "data:image/png;base64,AAAA".into(),
                detail: None,
            },
        });

        queue_while_busy(&mut app, "look at this", Path::new("."), false);

        assert_eq!(app.queued_count(), 1);
        assert!(app.attachments.is_empty(), "attachments are consumed");
        let queued = app.steering.drain();
        let has_image = matches!(
            queued[0].content,
            Some(crate::llm::MessageContent::Parts(ref parts))
                if parts.iter().any(|part| matches!(part, crate::llm::ContentPart::ImageUrl { .. }))
        );
        assert!(has_image, "the queued message carries the image");
        assert!(app.items.iter().any(|item| matches!(
            item,
            ChatItem::User(text) if text.contains("[1 attachment(s)]")
        )));
    }

    #[test]
    fn attach_command_lists_removes_and_clears() {
        let mut app = test_app();
        let image = |data: &str| crate::llm::ContentPart::ImageUrl {
            image_url: crate::llm::ImageUrl {
                url: format!("data:image/png;base64,{data}"),
                detail: None,
            },
        };
        app.add_attachment(image("AAAA"));
        app.add_attachment(image("BBBB"));

        assert!(!handle_attach_command(&mut app, "hello"));
        assert!(handle_attach_command(&mut app, "/attach"));
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text)) if text.contains("pending attachments") && text.contains("2.")
        ));

        assert!(handle_attach_command(&mut app, "/attachments remove 1"));
        assert_eq!(app.attachments.len(), 1);

        assert!(handle_attach_command(&mut app, "/attach remove zzzz"));
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Error(text)) if text.contains("no attachment")
        ));

        assert!(handle_attach_command(&mut app, "/attach clear"));
        assert!(app.attachments.is_empty());
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text)) if text.contains("cleared 1")
        ));
    }

    #[test]
    fn dequeuing_a_queued_attachment_restores_it() {
        let mut app = test_app();
        app.busy = true;
        app.add_attachment(crate::llm::ContentPart::ImageUrl {
            image_url: crate::llm::ImageUrl {
                url: "data:image/png;base64,AAAA".into(),
                detail: None,
            },
        });
        queue_while_busy(&mut app, "look", Path::new("."), false);
        assert!(app.attachments.is_empty());
        assert!(app
            .items
            .iter()
            .any(|item| matches!(item, ChatItem::User(_))));

        dequeue_messages(&mut app);

        assert_eq!(app.input, "look");
        assert_eq!(
            app.attachments.len(),
            1,
            "the image returns as a pending attachment"
        );
        assert!(
            !app.items
                .iter()
                .any(|item| matches!(item, ChatItem::User(_))),
            "the queued transcript entry is removed"
        );
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Status(text)) if text.contains("1 attachment(s)")
        ));
    }

    #[test]
    fn dequeue_with_nothing_queued_keeps_the_editor_and_says_so() {
        let mut app = test_app();
        app.input = "half a thought".into();
        dequeue_messages(&mut app);
        assert_eq!(app.input, "half a thought");
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Status(text)) if text == "no queued messages to restore"
        ));
    }

    #[test]
    fn dequeue_leaves_already_consumed_messages_alone() {
        let mut app = test_app();
        app.items.push(ChatItem::User("sent already".into()));
        app.follow_ups.push(Message::user("still waiting"));
        dequeue_messages(&mut app);
        assert_eq!(app.input, "still waiting");
        let users: Vec<&String> = app
            .items
            .iter()
            .filter_map(|item| match item {
                ChatItem::User(text) => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(
            users,
            ["sent already"],
            "the sent turn stays in the transcript"
        );
    }

    #[test]
    fn dequeue_shortcut_matches_pi() {
        let alt_up = KeyEvent::new(KeyCode::Up, KeyModifiers::ALT);
        assert!(is_dequeue_shortcut(&alt_up));
        assert!(!is_dequeue_shortcut(&KeyEvent::new(
            KeyCode::Up,
            KeyModifiers::NONE
        )));
        assert!(!use_windows_keybindings_for(false, false, false));
        assert!(use_windows_keybindings_for(true, false, false));
        assert!(use_windows_keybindings_for(false, true, true));
        assert!(!use_windows_keybindings_for(false, true, false));
        assert_eq!(dequeue_key_label_for(false, true), "Option+Up");
        assert_eq!(dequeue_key_label_for(false, false), "Alt+Up");
        assert_eq!(dequeue_key_label_for(true, false), "Alt+Q");
    }

    #[test]
    fn idle_tips_are_visible_in_the_transcript() {
        let mut app = test_app();
        // The busy-phase label is invisible when idle, so a tip must become a
        // transcript item for the user to see it.
        app.show_status("copied 12 chars");
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Status(text)) if text == "copied 12 chars"
        ));
        app.show_status("copied 34 chars");
        assert_eq!(
            app.items
                .iter()
                .filter(|item| matches!(item, ChatItem::Status(_)))
                .count(),
            1,
            "a repeated tip replaces the previous line"
        );
        app.items.push(ChatItem::Assistant("an answer".into()));
        app.show_status("copied 56 chars");
        assert_eq!(
            app.items
                .iter()
                .filter(|item| matches!(item, ChatItem::Status(_)))
                .count(),
            2,
            "a tip after other output starts a new line"
        );
    }

    #[test]
    fn restored_sessions_show_their_reasoning() {
        let mut app = test_app();
        let message = Message::assistant("the fix is in the loader", vec![]).with_thinking(vec![
            serde_json::json!({"type": "thinking", "thinking": "the test moved"}),
            serde_json::json!({"type": "redacted_thinking", "data": "x"}),
        ]);
        restore_history(&mut app, vec![message], "resumed".to_string());
        assert!(matches!(
            &app.items[0],
            ChatItem::Thinking { text, .. } if text == "the test moved"
        ));
        assert!(matches!(&app.items[1], ChatItem::Assistant(_)));
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
        assert_eq!(resolve_provider_choice("5"), "zai");
        assert_eq!(resolve_provider_choice("DeepSeek"), "deepseek");
        assert_eq!(resolve_provider_choice("Port-Key"), "portkey");
        assert_eq!(resolve_provider_choice("glm"), "zai");
        assert_eq!(resolve_provider_choice("gpt-4o"), "openai");
        assert_eq!(resolve_provider_choice("my-endpoint"), "my-endpoint");
        assert_eq!(resolve_provider_choice("9"), "9");
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
    fn connect_state_tracks_providers_that_are_already_logged_in() {
        let state = ConnectState {
            connected: vec!["openai".to_string(), "anthropic".to_string()],
            ..ConnectState::new()
        };

        assert!(state.is_connected("openai"));
        assert!(state.is_connected("OpenAI"));
        assert!(
            state.is_connected("gpt-4o"),
            "aliases resolve to their provider"
        );
        assert!(!state.is_connected("portkey"));
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
    fn connect_key_step_opens_optional_settings() {
        let mut app = test_app();
        let mut config = Config::default();
        app.connect = Some(ConnectState::new());

        // Pick Portkey so the Config ID row is offered.
        handle_connect_key(key(KeyCode::Char('4')), &mut app, &mut config);
        handle_connect_key(key(KeyCode::Enter), &mut app, &mut config);
        handle_connect_key(key(KeyCode::Char('k')), &mut app, &mut config);
        handle_connect_key(key(KeyCode::Enter), &mut app, &mut config);

        let state = app.connect.as_ref().unwrap();
        assert!(matches!(&state.step, ConnectStep::Options { provider } if provider == "portkey"));
        assert_eq!(state.focus, ConnectField::Model);
        assert_eq!(state.key, "k");
        assert_eq!(state.model, config.model_for_provider("portkey"));
        assert_eq!(state.base_url, "https://api.portkey.ai/v1");
        assert!(ConnectState::option_fields("portkey").contains(&ConnectField::PortkeyConfig));
        assert!(!ConnectState::option_fields("openai").contains(&ConnectField::PortkeyConfig));

        handle_connect_key(key(KeyCode::Down), &mut app, &mut config);
        assert_eq!(app.connect.as_ref().unwrap().focus, ConnectField::BaseUrl);
        handle_connect_key(key(KeyCode::Down), &mut app, &mut config);
        assert_eq!(
            app.connect.as_ref().unwrap().focus,
            ConnectField::PortkeyConfig
        );
        handle_connect_key(key(KeyCode::Down), &mut app, &mut config);
        assert_eq!(
            app.connect.as_ref().unwrap().focus,
            ConnectField::PortkeyConfig,
            "the last row is a boundary"
        );
        handle_connect_key(key(KeyCode::Up), &mut app, &mut config);
        assert_eq!(app.connect.as_ref().unwrap().focus, ConnectField::BaseUrl);
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
    fn escape_clears_input_without_quitting() {
        let mut app = test_app();
        app.set_input("draft".to_string());

        escape_action(&mut app);
        assert!(app.input.is_empty());
        assert!(!app.should_quit);

        escape_action(&mut app);
        assert!(!app.should_quit);
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
        assert!(help.contains("/mcps"));
        assert!(help.contains("/notify"));
        assert!(help.contains("/plugins"));
        assert!(help.contains("/review"));
    }

    #[test]
    fn suggestions_show_for_slash_and_filter() {
        let config = Config::default();
        let mut app = test_app();

        app.set_input("/".to_string());
        refresh_suggestions(&mut app, &config);
        assert!(app.suggestions.iter().any(|hint| hint.name == "models"));
        assert!(app.suggestions.iter().any(|hint| hint.name == "mcps"));
        assert!(app.suggestions.iter().any(|hint| hint.name == "plugins"));

        app.set_input("/models".to_string());
        refresh_suggestions(&mut app, &config);
        assert!(app.suggestions.iter().any(|hint| hint.name == "models"));

        app.set_input("hello".to_string());
        refresh_suggestions(&mut app, &config);
        assert!(app.suggestions.is_empty());

        app.set_input("/models foo".to_string());
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
        app.set_input("/rev".to_string());
        refresh_suggestions(&mut app, &config);
        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0].name, "review");
    }

    #[test]
    fn suggestions_include_skills() {
        let mut config = Config::default();
        config.ecosystem.skills.push(crate::ecosystem::Skill {
            name: "audit".to_string(),
            description: Some("audit dependencies".to_string()),
            content: String::new(),
        });
        let mut app = test_app();
        app.set_input("/ski".to_string());
        refresh_suggestions(&mut app, &config);
        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0].name, "skill:audit");

        app.set_input("/skill:".to_string());
        refresh_suggestions(&mut app, &config);
        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0].name, "skill:audit");
    }

    #[test]
    fn suggestions_complete_command_arguments() {
        let config = Config::default();
        let mut app = test_app();

        app.set_input("/notify ".to_string());
        refresh_suggestions(&mut app, &config);
        let names: Vec<&str> = app.suggestions.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["on", "off", "sound", "test"]);

        app.set_input("/notify s".to_string());
        refresh_suggestions(&mut app, &config);
        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0].name, "sound");

        app.set_input("/notify sound ".to_string());
        refresh_suggestions(&mut app, &config);
        let names: Vec<&str> = app.suggestions.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["on", "off"]);

        app.set_input("/usage currency ".to_string());
        refresh_suggestions(&mut app, &config);
        let names: Vec<&str> = app.suggestions.iter().map(|h| h.name.as_str()).collect();
        assert_eq!(names, vec!["usd", "cny"]);

        // Provider names for the login commands.
        app.set_input("/login d".to_string());
        refresh_suggestions(&mut app, &config);
        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0].name, "deepseek");

        // Free-text commands and unknown arguments offer nothing.
        app.set_input("/models foo".to_string());
        refresh_suggestions(&mut app, &config);
        assert!(app.suggestions.is_empty());
        app.set_input("/notify bogus ".to_string());
        refresh_suggestions(&mut app, &config);
        assert!(app.suggestions.is_empty());
    }

    #[test]
    fn completing_an_argument_replaces_only_that_token() {
        let config = Config::default();
        let mut app = test_app();
        app.set_input("/notify s".to_string());
        app.input_cursor = app.input.len();
        refresh_suggestions(&mut app, &config);
        assert!(complete_suggestion(&mut app));
        assert_eq!(app.input, "/notify sound ");

        refresh_suggestions(&mut app, &config);
        assert!(complete_suggestion(&mut app));
        assert_eq!(app.input, "/notify sound on ");
        assert_eq!(app.input_cursor, app.input.len());
    }

    #[test]
    fn completing_an_argument_reuses_the_existing_separator() {
        let config = Config::default();
        let mut app = test_app();
        app.set_input("/notify s foo".to_string());
        app.input_cursor = "/notify s".len();
        refresh_suggestions(&mut app, &config);
        assert!(complete_suggestion(&mut app));
        assert_eq!(app.input, "/notify sound foo");
        assert_eq!(app.input_cursor, "/notify sound ".len());
    }

    #[test]
    fn suggestions_show_project_files_for_at_path() {
        let dir =
            std::env::temp_dir().join(format!("oxide_file_suggestions_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("README.md"), "oxide").unwrap();

        let config = Config::default();
        let mut app = App::new(
            "test-model".to_string(),
            dir.display().to_string(),
            crate::config::Reasoning::Auto,
        );
        app.set_input("review @main".to_string());
        refresh_suggestions(&mut app, &config);

        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0].name, "src/main.rs");
        assert!(complete_suggestion(&mut app));
        assert_eq!(app.input, "review @src/main.rs ");
        assert_eq!(active_file_query(&app.input, app.input_cursor), None);

        app.set_input("review @sr".to_string());
        refresh_suggestions(&mut app, &config);
        assert_eq!(app.suggestions[0].name, "src/");
        assert!(complete_suggestion(&mut app));
        assert_eq!(app.input, "review @src/ ");
        assert_eq!(app.input_cursor, app.input.len());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn models_state_filters_and_selects() {
        let mut state = ModelsState::ready(
            vec![
                ModelChoice::new("deepseek", "deepseek-chat"),
                ModelChoice::new("deepseek", "deepseek-reasoner"),
            ],
            "deepseek".to_string(),
            "deepseek-chat".to_string(),
            Some("deepseek-reasoner".to_string()),
        );
        assert_eq!(state.filtered().len(), 2);
        assert_eq!(
            state.filtered(),
            vec![
                &ModelChoice::new("deepseek", "deepseek-chat"),
                &ModelChoice::new("deepseek", "deepseek-reasoner"),
            ]
        );
        assert_eq!(
            state.selected_model(),
            Some(&ModelChoice::new("deepseek", "deepseek-chat"))
        );

        state.filter = "reason".to_string();
        assert_eq!(
            state.filtered(),
            vec![&ModelChoice::new("deepseek", "deepseek-reasoner")]
        );

        state = ModelsState::ready(
            vec![ModelChoice::new("anthropic", "claude-sonnet-5")],
            "anthropic".to_string(),
            "claude-sonnet-5".to_string(),
            None,
        );
        state.filter = "Claude Sonnet 5".to_string();
        assert_eq!(
            state.selected_model(),
            Some(&ModelChoice::new("anthropic", "claude-sonnet-5"))
        );

        state.filter = "missing".to_string();
        assert!(state.selected_model().is_none());
    }

    #[test]
    fn models_state_lists_the_same_model_from_every_provider() {
        let mut state = ModelsState::ready(
            vec![
                ModelChoice::new("openai", "gpt-5.4"),
                ModelChoice::new("portkey", "gpt-5.4"),
                ModelChoice::new("openai", "gpt-5.6-sol"),
            ],
            "portkey".to_string(),
            "gpt-5.4".to_string(),
            None,
        );

        // The active provider's copy of the current model sorts first, and both
        // providers stay visible.
        assert_eq!(
            state.filtered()[0],
            &ModelChoice::new("portkey", "gpt-5.4"),
            "the active provider's model comes first"
        );
        state.filter = "gpt-5.4".to_string();
        assert_eq!(state.filtered().len(), 2);
    }

    #[test]
    fn input_cursor_edits_unicode_text() {
        let mut app = test_app();
        app.set_input("a界c".to_string());

        app.input_cursor_left();
        app.input_cursor_left();
        app.insert_input("b");
        assert_eq!(app.input, "ab界c");

        app.input_cursor_right();
        app.input_backspace();
        assert_eq!(app.input, "abc");
        assert!(app.input.is_char_boundary(app.input_cursor));
    }

    #[test]
    fn input_home_and_end_jump_to_the_edges() {
        let mut app = test_app();
        app.set_input("hello world".to_string());
        app.input_cursor = 5;

        app.input_home();
        assert_eq!(app.input_cursor, 0);

        app.input_end();
        assert_eq!(app.input_cursor, app.input.len());
    }

    #[test]
    fn formats_mcp_statuses() {
        let ChatItem::Listing { title, rows } = mcp_listing(&[
            (
                "context7".to_string(),
                "remote: https://mcp.context7.com/mcp/oauth".to_string(),
                McpStatus::Connected,
            ),
            (
                "newrelic".to_string(),
                "remote: https://mcp.newrelic.com/mcp/".to_string(),
                McpStatus::NeedsAuth,
            ),
        ]) else {
            panic!("expected a listing");
        };
        assert_eq!(title, "MCP servers (2)");
        assert_eq!(rows[0].name, "context7");
        assert_eq!(rows[0].status.as_deref(), Some("Connected"));
        assert_eq!(rows[0].tone, Tone::Success);
        assert_eq!(rows[1].status.as_deref(), Some("Needs Auth"));
        assert_eq!(rows[1].tone, Tone::Warning);
        assert!(matches!(
            mcp_listing(&[]),
            ChatItem::Info(ref text) if text == "no MCP servers configured"
        ));
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
            Rect::new(0, 0, 80, 24),
        );
        assert_eq!(app.scroll, 27);
        assert_eq!(app.input_history, vec!["prompt".to_string()]);
        assert!(app.input.is_empty());
    }

    #[test]
    fn mouse_hover_and_click_complete_a_suggestion() {
        let mut app = test_app();
        app.set_input("/s".to_string());
        app.suggestions = vec![
            CommandHint {
                name: "session".to_string(),
                description: "show session info".to_string(),
            },
            CommandHint {
                name: "share".to_string(),
                description: "share this session".to_string(),
            },
        ];
        let area = Rect::new(0, 0, 80, 24);

        handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Moved,
                column: 4,
                row: 16,
                modifiers: KeyModifiers::NONE,
            },
            &mut app,
            area,
        );
        assert_eq!(app.suggestion_index, 1);

        handle_mouse(
            MouseEvent {
                kind: MouseEventKind::Down(MouseButton::Left),
                column: 4,
                row: 16,
                modifiers: KeyModifiers::NONE,
            },
            &mut app,
            area,
        );
        assert_eq!(app.input, "/share");
        assert_eq!(app.input_cursor, app.input.len());
        assert!(app.suggestions.is_empty());
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
        app.set_input("/ini".to_string());
        refresh_suggestions(&mut app, &config);
        assert_eq!(app.suggestions.len(), 1);
        assert_eq!(app.suggestions[0].name, "init");
        assert!(help_text(&config).contains("/init"));
        assert!(init_prompt().contains("AGENTS.md"));
    }

    #[test]
    fn sessions_state_filters_sorts_and_selects() {
        use crate::session::SessionSummary;

        let summary = |id: &str, name: Option<&str>, modified: u64| SessionSummary {
            id: id.to_string(),
            name: name.map(str::to_string),
            cwd: "/tmp/proj".to_string(),
            created_at: 0,
            modified_at: modified,
            message_count: 1,
            preview: format!("preview {id}"),
            path: std::path::PathBuf::from(id),
        };
        let mut state = SessionsState::ready(vec![
            summary("aaa", Some("alpha"), 100),
            summary("bbb", None, 200),
            summary("ccc", Some("gamma"), 300),
        ]);

        assert_eq!(state.filtered()[0].id, "ccc");

        state.named_only = true;
        assert_eq!(state.filtered().len(), 2);

        state.filter = "alpha".to_string();
        assert_eq!(state.selected_session().unwrap().id, "aaa");
    }

    #[tokio::test]
    async fn usage_command_configures_and_toggles_the_bar() {
        let dir = std::env::temp_dir().join(format!("oxide_usage_cmd_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("portkey-usage.json");
        std::env::set_var("OXIDE_USAGE_FILE", &path);

        let mut app = test_app();
        // Point the bar at a closed port so a refresh cannot reach the network.
        app.usage_settings.base_url = "http://127.0.0.1:9".to_string();
        let mut config = Config::default();
        let (tx, _rx) = unbounded_channel();

        handle_usage_command(&mut app, &config, "/usage", &tx);
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text)) if text.contains("bar: off")
        ));

        handle_usage_command(&mut app, &config, "/usage on", &tx);
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Error(text)) if text.contains("needs a Portkey login")
        ));
        assert!(app.usage.is_none());

        handle_usage_command(&mut app, &config, "/usage user firstname.lastname", &tx);
        handle_usage_command(&mut app, &config, "/usage budget 600", &tx);
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text)) if text.contains("budget set to $600.00")
        ));
        handle_usage_command(&mut app, &config, "/usage currency ¥", &tx);
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text)) if text.contains("currency set to cny (¥600.00)")
        ));
        handle_usage_command(&mut app, &config, "/usage budget ¥601", &tx);
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text)) if text.contains("budget set to ¥601.00")
        ));
        handle_usage_command(&mut app, &config, "/usage budget 600", &tx);
        handle_usage_command(&mut app, &config, "/usage currency eur", &tx);
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Error(text)) if text.contains("currency <usd|cny>")
        ));

        handle_usage_command(&mut app, &config, "/usage on", &tx);
        assert!(app.usage.is_none(), "a Portkey login is still missing");

        config.apply_provider("portkey", "pk-test");
        handle_usage_command(&mut app, &config, "/usage on", &tx);
        assert!(app.usage.is_some());
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text)) if text.contains("bar on")
        ));
        assert_eq!(
            app.usage.as_ref().unwrap().columns(0.0),
            "Session: $0.00 | Today: … | Month: … / ¥600.00"
        );

        let saved = crate::portkey_usage::UsageSettings::load_from(&path).unwrap();
        assert!(saved.enabled);
        assert_eq!(saved.user, "firstname.lastname");
        assert_eq!(saved.budget, Some(600.0));
        assert_eq!(saved.currency, crate::portkey_usage::Currency::Cny);
        handle_usage_command(&mut app, &config, "/usage budget $600", &tx);
        assert_eq!(
            crate::portkey_usage::UsageSettings::load_from(&path)
                .unwrap()
                .currency,
            crate::portkey_usage::Currency::Usd,
            "a `$` amount switches the currency back"
        );

        handle_usage_command(&mut app, &config, "/usage key pk-usage-key", &tx);
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text)) if text.contains("pk-u...-key")
        ));

        handle_usage_command(&mut app, &config, "/usage off", &tx);
        assert!(app.usage.is_none());
        sync_usage_bar(&mut app, &config, &tx);
        assert!(app.usage.is_none(), "off stays off");
        assert!(
            !crate::portkey_usage::UsageSettings::load_from(&path)
                .unwrap()
                .enabled
        );

        handle_usage_command(&mut app, &config, "/usage bogus", &tx);
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Error(text)) if text.contains("unknown /usage option")
        ));

        std::env::remove_var("OXIDE_USAGE_FILE");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn notify_command_toggles_and_persists() {
        let dir = std::env::temp_dir().join(format!("oxide_notify_cmd_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::env::set_var("OXIDE_SETTINGS_FILE", &path);

        let mut app = test_app();
        let mut config = Config::default();

        handle_notify_command(&mut app, &mut config, "/notify");
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Info(text))
                if text.contains("notifications: on; sound: on")
        ));

        // The toast is on through a project/env override while the global file
        // is empty, so `/notify sound off` must persist only the sound key.
        config.notify.on_complete = true;
        handle_notify_command(&mut app, &mut config, "/notify sound off");
        assert!(!config.notify.sound);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            saved.get("notifyOnComplete").is_none(),
            "the untouched key is not persisted"
        );
        assert_eq!(saved["notifySound"], serde_json::json!(false));

        handle_notify_command(&mut app, &mut config, "/notify off");
        assert!(!config.notify.on_complete);
        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(saved["notifyOnComplete"], serde_json::json!(false));
        assert_eq!(saved["notifySound"], serde_json::json!(false));

        handle_notify_command(&mut app, &mut config, "/notify bogus");
        assert!(matches!(
            app.items.last(),
            Some(ChatItem::Error(text)) if text.contains("unknown /notify option")
        ));

        std::env::remove_var("OXIDE_SETTINGS_FILE");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn usage_dialog_edits_fields_in_place() {
        let mut app = test_app();
        let config = Config::default();
        let (tx, _rx) = unbounded_channel();
        app.usage_modal = Some(UsageState::new(app.usage_settings.clone()));

        // Move to User, open the editor, type, and apply.
        handle_usage_key(key(KeyCode::Down), &mut app, &config, &tx);
        assert_eq!(app.usage_modal.as_ref().unwrap().field(), UsageField::User);
        handle_usage_key(key(KeyCode::Enter), &mut app, &config, &tx);
        assert!(app.usage_modal.as_ref().unwrap().editing);
        for ch in "firstname.lastname".chars() {
            handle_usage_key(key(KeyCode::Char(ch)), &mut app, &config, &tx);
        }
        handle_usage_key(key(KeyCode::Enter), &mut app, &config, &tx);
        let state = app.usage_modal.as_ref().unwrap();
        assert!(!state.editing);
        assert_eq!(state.settings.user, "firstname.lastname");

        // Budget accepts a currency prefix and parses it.
        handle_usage_key(key(KeyCode::Down), &mut app, &config, &tx);
        handle_usage_key(key(KeyCode::Down), &mut app, &config, &tx);
        assert_eq!(
            app.usage_modal.as_ref().unwrap().field(),
            UsageField::Budget
        );
        handle_usage_key(key(KeyCode::Enter), &mut app, &config, &tx);
        for ch in "¥600".chars() {
            handle_usage_key(key(KeyCode::Char(ch)), &mut app, &config, &tx);
        }
        handle_usage_key(key(KeyCode::Enter), &mut app, &config, &tx);
        let state = app.usage_modal.as_ref().unwrap();
        assert_eq!(state.settings.budget, Some(600.0));
        assert_eq!(state.settings.currency, crate::portkey_usage::Currency::Cny);

        // A bad value keeps the editor open with an error.
        handle_usage_key(key(KeyCode::Enter), &mut app, &config, &tx);
        handle_usage_key(key(KeyCode::Char('x')), &mut app, &config, &tx);
        handle_usage_key(key(KeyCode::Enter), &mut app, &config, &tx);
        let state = app.usage_modal.as_ref().unwrap();
        assert!(state.editing);
        assert!(state.error.is_some());
    }

    #[tokio::test]
    async fn a_portkey_login_shows_an_enabled_bar() {
        let mut app = test_app();
        app.usage_settings = crate::portkey_usage::UsageSettings {
            enabled: true,
            user: "firstname.lastname".to_string(),
            base_url: "http://127.0.0.1:9".to_string(),
            ..Default::default()
        };
        let mut config = Config::default();
        let (tx, _rx) = unbounded_channel();

        sync_usage_bar(&mut app, &config, &tx);
        assert!(app.usage.is_none(), "no Portkey login yet");

        config.apply_provider("portkey", "pk-test");
        sync_usage_bar(&mut app, &config, &tx);
        assert!(app.usage.is_some(), "the login makes the bar usable");

        config.apply_provider("openai", "sk-test");
        sync_usage_bar(&mut app, &config, &tx);
        assert!(app.usage.is_none(), "another provider hides it again");
    }
}
