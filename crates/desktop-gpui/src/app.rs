//! The window itself: the state the front-end holds, the engine's commands it
//! calls, and the packets it paints.
//!
//! This is the primary GPUI front-end of the desktop app. It uses the same
//! configuration, session store and approvals as the Electron app in
//! `crates/desktop`, drawn natively instead of by a webview. Everything the
//! engine answers — projects, threads, turns, cards, diffs, models, themes,
//! prefs — is read from the commands in `oxide_desktop::commands`, which is
//! why both windows behave the same way.

use crate::editor::{self, Editor, EditorEvent};
use crate::engine::Engine;
use crate::model::{
    Attachment, Chat, EditedCard, EditedFile, Entry, ProjectRow, QueuedMessage, SessionRow,
};
use crate::overlays;
use crate::terminal::TerminalPane;
use crate::theme::Theme;
use crate::views;
use gpui::prelude::*;
use gpui::{
    actions, div, px, size, App, Context, Entity, FocusHandle, Focusable, KeyBinding, KeyDownEvent,
    MouseButton, MouseDownEvent, MouseMoveEvent, Render, SharedString, Subscription, Window,
    WindowBounds, WindowOptions,
};
use oxide_core::worktree::{Worktree, Worktrees};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const MAX_ATTACHMENTS_PER_MESSAGE: usize = 8;

actions!(
    shortcuts,
    [
        NewChat,
        CloseChat,
        ToggleSidebar,
        ToggleActivities,
        ToggleDetails,
        OpenSettings,
        ToggleChanges,
        OpenModels,
        OpenWorktrees,
        OpenTerminal,
        OpenPulls,
        AboutApp,
        CheckForUpdates,
        HideApp,
        IncreaseTextSize,
        DecreaseTextSize,
        ResetTextSize,
        ToggleFullscreen,
        MinimizeWindow,
        CloseWindow,
        Back,
        Forward,
        DismissOverlay,
        QuitApp,
        ShowShortcuts,
        AcceptPrediction,
        TerminalTab,
        TerminalBackTab,
        SwitchToSession1,
        SwitchToSession2,
        SwitchToSession3,
        SwitchToSession4,
        SwitchToSession5,
        SwitchToSession6,
        SwitchToSession7,
        SwitchToSession8,
        SwitchToSession9,
    ]
);

/// What the navigation rail shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rail {
    Chats,
    Projects,
    Customize,
}

/// The section the Customize destination is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum CustomizeTab {
    Plugins,
    Skills,
    McpServers,
    Approvals,
}

/// Which section the settings sheet is showing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SettingsTab {
    General,
    Appearance,
    Notifications,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeScope {
    Turn,
    Repository,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PanelTab {
    Changes,
    Terminal,
    Pulls,
}

#[derive(Clone)]
pub struct FileViewer {
    pub path: String,
    pub kind: String,
    pub text: String,
    pub image: Option<std::sync::Arc<gpui::Image>>,
    pub truncated: bool,
    pub source: bool,
    pub loading: bool,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ComputerUsePolicy {
    pub loading: bool,
    pub error: Option<String>,
    pub configured: bool,
    pub server: String,
    pub project: String,
    pub value: Value,
}

#[derive(Clone)]
pub struct ChangeReview {
    pub project: String,
    pub scope: ChangeScope,
    pub unified: bool,
    pub files: Vec<EditedFile>,
    pub turn_files: Vec<EditedFile>,
    pub turn_baseline: Option<String>,
    pub added: usize,
    pub removed: usize,
    pub open_files: HashSet<String>,
    pub expanded: HashSet<String>,
    pub sides: HashMap<String, Value>,
    pub loading: HashSet<String>,
    pub errors: HashMap<String, String>,
}

impl ChangeReview {
    fn new(project: String, scope: ChangeScope, card: &EditedCard) -> Self {
        let first = card.files.first().map(|file| file.path.clone());
        Self {
            project,
            scope,
            unified: false,
            files: card.files.clone(),
            turn_files: card.files.clone(),
            turn_baseline: card.baseline.clone(),
            added: card.added,
            removed: card.removed,
            open_files: first.into_iter().collect(),
            expanded: HashSet::new(),
            sides: HashMap::new(),
            loading: HashSet::new(),
            errors: HashMap::new(),
        }
    }
}

/// What is drawn over the window.
#[derive(Clone)]
pub enum Overlay {
    Settings,
    About,
    ImagePreview {
        name: String,
        image: std::sync::Arc<gpui::Image>,
    },
    Updates,
    Help,
    CommandMenu,
    Mcp,
    Agents,
    Shortcuts,
    Trust {
        project: String,
        onboarding: bool,
    },
    Approval {
        chat: usize,
        id: u64,
    },
    Question {
        chat: usize,
        id: u64,
    },
    Confirm {
        title: String,
        body: String,
        action: ConfirmAction,
    },
    SessionActions {
        id: String,
        title: String,
        project: String,
        pinned: bool,
    },
    ProjectActions {
        id: String,
        path: String,
        name: String,
        pinned: bool,
    },
    Approvals,
    ComputerUse,
    Worktrees,
    WorktreeBranch {
        project: String,
        name: String,
    },
    Projects,
    RenameSession {
        project: String,
        id: String,
        title: String,
    },
    Connect {
        provider: String,
    },
    NewProject,
    FileViewer,
}

#[derive(Clone)]
pub enum ConfirmAction {
    RemoveProject(String),
    CloseChat(usize),
    RemoveWorktree { project: String, name: String },
}

pub struct Toast {
    pub text: String,
    pub error: bool,
}

fn field_editor(placeholder: &'static str, cx: &mut App) -> Entity<Editor> {
    cx.new(|cx| {
        let theme = Theme::default();
        Editor::new(
            placeholder,
            theme.text(),
            theme.dim(),
            theme.faint(),
            theme.accent(),
            theme.selection(),
            cx,
        )
        .single_line()
    })
}

/// The strings the window draws, from the shared catalog the CLI, the desktop
/// app and the extension all read. A key missing from it falls back to the
/// English the window is authored in.
pub struct Strings(HashMap<String, String>);

impl Strings {
    pub fn load(value: &Value) -> Self {
        let mut map = HashMap::new();
        if let Some(strings) = value["strings"].as_object() {
            for (key, text) in strings {
                if let Some(text) = text.as_str() {
                    map.insert(key.clone(), text.to_string());
                }
            }
        }
        Self(map)
    }

    pub fn t(&self, key: &str, fallback: &str) -> String {
        self.0
            .get(key)
            .cloned()
            .unwrap_or_else(|| fallback.to_string())
    }
}

pub struct Root {
    pub engine: Engine,
    pub theme: Theme,
    pub strings: Strings,
    pub projects: Vec<ProjectRow>,
    /// Whether the engine has answered with the project list. The welcome page
    /// waits for it, so the window cannot offer to add a folder, or say that
    /// none is open, before the app knows which folders there are.
    pub projects_loaded: bool,
    pub sessions: Vec<SessionRow>,
    pub chats: Vec<Chat>,
    pub active: usize,
    pub history: Vec<usize>,
    pub forward: Vec<usize>,
    pub next_chat: u64,
    pub editors: HashMap<u64, Entity<Editor>>,
    pending_focus: Option<u64>,
    pub search: Entity<Editor>,
    pub command_query: Entity<Editor>,
    pub worktree_name: Entity<Editor>,
    pub worktree_base: Entity<Editor>,
    pub worktree_branch: Entity<Editor>,
    pub session_name: Entity<Editor>,
    pub project_name: Entity<Editor>,
    pub plugin_query: Entity<Editor>,
    pub marketplace_source: Entity<Editor>,
    pub provider_filter: Entity<Editor>,
    pub provider_key: Entity<Editor>,
    pub provider_model: Entity<Editor>,
    pub provider_url: Entity<Editor>,
    pub new_project_folders: Vec<String>,
    pub query: String,
    pub activities_open: bool,
    /// Whether the sidebar is showing the search field in place of its brand row,
    /// the page's own swap between the name and the box.
    pub searching: bool,
    pub results: Vec<SessionRow>,
    pub rail: Rail,
    pub customize_tab: CustomizeTab,
    pub settings_tab: SettingsTab,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    pub prefs: Value,
    pub info: Value,
    pub models: Value,
    pub model_menu_open: bool,
    pub model_menu_show_models: bool,
    pub effort_drag_level: Option<String>,
    pub composer_tools_open: bool,
    pub approvals: Vec<String>,
    pub computer_use_policy: ComputerUsePolicy,
    pub commands: Vec<Value>,
    pub at_answer: Value,
    pub at_chat: Option<u64>,
    pub at_generation: u64,
    /// The branch the open thread's folder is on, when it is a repository. The
    /// composer's top row names it, and leaves the fact out when there is none.
    pub branch: Option<String>,
    /// The MCP servers this project loads, as the details pane lists them.
    pub sources: Vec<Value>,
    pub pulls: Vec<Value>,
    pub pulls_error: Option<String>,
    pub pull_detail: Option<Value>,
    pub pull_open: Option<u64>,
    pub pull_loading: bool,
    pub worktrees: Vec<Worktree>,
    pub worktrees_error: Option<String>,
    pub worktree_detached: bool,
    pub worktree_copy_changes: bool,
    pub show_details: bool,
    pub theme_names: Vec<String>,
    pub theme_previews: HashMap<String, Theme>,
    pub current_theme: String,
    pub languages: Vec<Value>,
    pub language: String,
    pub changes: Option<Value>,
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub expanded_projects: HashSet<String>,
    pub collapsed_projects: HashSet<String>,
    pub inline_project_actions: Option<String>,
    pub project_actions_anchor_y: Option<f32>,
    pub(crate) project_actions_dismiss_armed: bool,
    pub assistant_actions: Option<(usize, usize)>,
    pub edited_actions: Option<(usize, usize)>,
    pub(crate) composer_tools_dismiss_armed: bool,
    /// The pointer position and sidebar width when a resize drag began.
    pub resizing: Option<(f32, f32)>,
    pub show_changes: bool,
    pub live_changes: Option<(String, usize, usize, usize)>,
    pub live_changes_busy: bool,
    pub changes_width: f32,
    pub changes_resizing: Option<(f32, f32)>,
    pub change_review: Option<ChangeReview>,
    pub panel_tab: PanelTab,
    pub repository_status: Vec<Value>,
    pub terminal: TerminalPane,
    pub terminal_focus: FocusHandle,
    pub file_viewer: Option<FileViewer>,
    pub plugin_catalog: Value,
    pub plugin_busy: bool,
    pub plugin_message: Option<String>,
    pub providers: Vec<Value>,
    pub prediction: String,
    pub prediction_chat: Option<u64>,
    pub prediction_generation: u64,
    pub run_clock_ticking: bool,
    /// Bumped for every command typed into the dock's terminal, so the branch
    /// read that follows a command only answers for the newest one.
    pub terminal_generation: u64,
    pub update_check: Option<Value>,
    pub update_error: Option<String>,
    pub checking_updates: bool,
    pub update_progress: Option<Value>,
    pub update_result: Option<Value>,
    pub update_failure: Option<String>,
    pub update_restart_failed: bool,
    pub updating_update: bool,
    pub update_token: Option<String>,
    pub update_flag: bool,
    pub subscriptions: Vec<Subscription>,
}

impl Root {
    pub fn new(
        engine: Engine,
        mut packets: tokio::sync::mpsc::UnboundedReceiver<Value>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "Search chats",
                theme.text(),
                theme.dim(),
                theme.faint(),
                theme.accent(),
                theme.selection(),
                cx,
            )
            .single_line()
        });
        let command_query = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "Search commands",
                theme.text(),
                theme.dim(),
                theme.faint(),
                theme.accent(),
                theme.selection(),
                cx,
            )
            .single_line()
        });
        let worktree_name = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "Worktree name",
                theme.text(),
                theme.dim(),
                theme.faint(),
                theme.accent(),
                theme.selection(),
                cx,
            )
            .single_line()
        });
        let worktree_base = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "HEAD (current checkout)",
                theme.text(),
                theme.dim(),
                theme.faint(),
                theme.accent(),
                theme.selection(),
                cx,
            )
            .single_line()
        });
        let worktree_branch = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "Branch name",
                theme.text(),
                theme.dim(),
                theme.faint(),
                theme.accent(),
                theme.selection(),
                cx,
            )
            .single_line()
        });
        let session_name = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "Chat title",
                theme.text(),
                theme.dim(),
                theme.faint(),
                theme.accent(),
                theme.selection(),
                cx,
            )
            .single_line()
        });
        let project_name = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "Project name",
                theme.text(),
                theme.dim(),
                theme.faint(),
                theme.accent(),
                theme.selection(),
                cx,
            )
            .single_line()
        });
        let plugin_query = field_editor("Search plugins", cx);
        let marketplace_source = field_editor("Marketplace URL or owner/repo", cx);
        let provider_filter = field_editor("Search providers", cx);
        let provider_key = field_editor("Paste your API key", cx);
        let provider_model = field_editor("Model (optional)", cx);
        let provider_url = field_editor("Base URL (optional)", cx);

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(&search, |root, _editor, _event, cx| {
            let query = root.search_text(cx);
            if query != root.query {
                root.query = query;
                root.search_sessions(cx);
            }
            cx.notify();
        }));
        subscriptions.push(cx.subscribe(&command_query, |_root, _editor, _event, cx| {
            cx.notify();
        }));
        for editor in [
            &plugin_query,
            &marketplace_source,
            &provider_filter,
            &provider_key,
            &provider_model,
            &provider_url,
        ] {
            subscriptions.push(cx.subscribe(editor, |_root, _editor, _event, cx| {
                cx.notify();
            }));
        }

        let mut root = Self {
            engine,
            theme: Theme::default(),
            strings: Strings(HashMap::new()),
            projects: Vec::new(),
            projects_loaded: false,
            sessions: Vec::new(),
            chats: Vec::new(),
            active: 0,
            history: Vec::new(),
            forward: Vec::new(),
            next_chat: 1,
            editors: HashMap::new(),
            pending_focus: None,
            search,
            command_query,
            worktree_name,
            worktree_base,
            worktree_branch,
            session_name,
            project_name,
            plugin_query,
            marketplace_source,
            provider_filter,
            provider_key,
            provider_model,
            provider_url,
            new_project_folders: Vec::new(),
            query: String::new(),
            activities_open: false,
            searching: false,
            results: Vec::new(),
            rail: Rail::Chats,
            customize_tab: CustomizeTab::Plugins,
            settings_tab: SettingsTab::General,
            overlay: None,
            toast: None,
            prefs: Value::Null,
            info: Value::Null,
            models: Value::Null,
            model_menu_open: false,
            model_menu_show_models: false,
            effort_drag_level: None,
            composer_tools_open: false,
            approvals: Vec::new(),
            computer_use_policy: ComputerUsePolicy::default(),
            commands: Vec::new(),
            at_answer: Value::Null,
            at_chat: None,
            at_generation: 0,
            branch: None,
            sources: Vec::new(),
            pulls: Vec::new(),
            pulls_error: None,
            pull_detail: None,
            pull_open: None,
            pull_loading: false,
            worktrees: Vec::new(),
            worktrees_error: None,
            worktree_detached: true,
            worktree_copy_changes: false,
            show_details: true,
            theme_names: Vec::new(),
            theme_previews: HashMap::from([("dark".to_string(), Theme::default())]),
            current_theme: "dark".to_string(),
            languages: Vec::new(),
            language: String::new(),
            changes: None,
            sidebar_visible: true,
            sidebar_width: 300.0,
            expanded_projects: HashSet::new(),
            collapsed_projects: HashSet::new(),
            inline_project_actions: None,
            project_actions_anchor_y: None,
            project_actions_dismiss_armed: false,
            assistant_actions: None,
            edited_actions: None,
            composer_tools_dismiss_armed: false,
            resizing: None,
            show_changes: false,
            live_changes: None,
            live_changes_busy: false,
            changes_width: 430.0,
            changes_resizing: None,
            change_review: None,
            panel_tab: PanelTab::Changes,
            repository_status: Vec::new(),
            terminal: TerminalPane::default(),
            terminal_focus: cx.focus_handle(),
            file_viewer: None,
            plugin_catalog: Value::Null,
            plugin_busy: false,
            plugin_message: None,
            providers: Vec::new(),
            prediction: String::new(),
            prediction_chat: None,
            prediction_generation: 0,
            run_clock_ticking: false,
            terminal_generation: 0,
            update_check: None,
            update_error: None,
            checking_updates: false,
            update_progress: None,
            update_result: None,
            update_failure: None,
            update_restart_failed: false,
            updating_update: false,
            update_token: None,
            update_flag: false,
            subscriptions,
        };

        window.set_window_title("Oxide");
        root.load_prefs(cx);
        root.load_language(cx);
        root.load_themes(cx);
        root.refresh_projects(cx);
        root.catch_up_on_launch_update(cx);

        // A branch switched in a terminal is the reader's to see here, and
        // nothing in this process was told it changed: coming back to the
        // window reads the folder's facts again, the way the page does. The
        // observer runs once as it is registered, which is the launch read.
        let mut registered = false;
        let activation = cx.observe_window_activation(window, move |root, window, cx| {
            let first = !std::mem::replace(&mut registered, true);
            if first || !window.is_window_active() {
                return;
            }
            root.load_git(cx);
            if root.show_changes || root.show_details {
                let index = root.active;
                root.refresh_repository(index, cx);
            }
        });
        root.subscriptions.push(activation);

        cx.spawn(async move |this, cx| {
            while let Some(packet) = packets.recv().await {
                let name = packet["id"].as_str().unwrap_or_default().to_string();
                let payload = packet["payload"].clone();
                if this
                    .update(cx, |root, cx| root.event(&name, payload, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();

        root
    }

    pub fn bind_keys(cx: &mut App) {
        let mut bindings = editor::key_bindings();
        bindings.push(KeyBinding::new("cmd-t", NewChat, Some("Root")));
        bindings.push(KeyBinding::new("cmd-n", NewChat, Some("Root")));
        bindings.push(KeyBinding::new("cmd-w", CloseChat, Some("Root")));
        bindings.push(KeyBinding::new("cmd-b", ToggleSidebar, Some("Root")));
        bindings.push(KeyBinding::new(
            "cmd-shift-a",
            ToggleActivities,
            Some("Root"),
        ));
        bindings.push(KeyBinding::new("cmd-j", ToggleDetails, Some("Root")));
        bindings.push(KeyBinding::new("cmd-,", OpenSettings, Some("Root")));
        bindings.push(KeyBinding::new("cmd-enter", ToggleChanges, Some("Root")));
        bindings.push(KeyBinding::new("cmd-shift-m", OpenModels, Some("Root")));
        bindings.push(KeyBinding::new("cmd-shift-g", OpenWorktrees, Some("Root")));
        bindings.push(KeyBinding::new("cmd-shift-t", OpenTerminal, Some("Root")));
        bindings.push(KeyBinding::new("cmd-shift-p", OpenPulls, Some("Root")));
        bindings.push(KeyBinding::new("cmd-[", Back, Some("Root")));
        bindings.push(KeyBinding::new("cmd-]", Forward, Some("Root")));
        bindings.push(KeyBinding::new("cmd-h", HideApp, Some("Root")));
        bindings.push(KeyBinding::new("cmd-+", IncreaseTextSize, Some("Root")));
        bindings.push(KeyBinding::new("cmd-=", IncreaseTextSize, Some("Root")));
        bindings.push(KeyBinding::new("cmd--", DecreaseTextSize, Some("Root")));
        bindings.push(KeyBinding::new("cmd-0", ResetTextSize, Some("Root")));
        bindings.push(KeyBinding::new(
            "ctrl-cmd-f",
            ToggleFullscreen,
            Some("Root"),
        ));
        bindings.push(KeyBinding::new("cmd-shift-w", CloseWindow, Some("Root")));
        bindings.push(KeyBinding::new("escape", DismissOverlay, Some("Root")));
        bindings.push(KeyBinding::new("cmd-q", QuitApp, Some("Root")));
        bindings.push(KeyBinding::new(
            "tab",
            AcceptPrediction,
            Some("Composer && prediction"),
        ));
        bindings.push(KeyBinding::new("tab", TerminalTab, Some("Terminal")));
        bindings.push(KeyBinding::new(
            "shift-tab",
            TerminalBackTab,
            Some("Terminal"),
        ));
        bindings.push(KeyBinding::new("cmd-1", SwitchToSession1, Some("Root")));
        bindings.push(KeyBinding::new("cmd-2", SwitchToSession2, Some("Root")));
        bindings.push(KeyBinding::new("cmd-3", SwitchToSession3, Some("Root")));
        bindings.push(KeyBinding::new("cmd-4", SwitchToSession4, Some("Root")));
        bindings.push(KeyBinding::new("cmd-5", SwitchToSession5, Some("Root")));
        bindings.push(KeyBinding::new("cmd-6", SwitchToSession6, Some("Root")));
        bindings.push(KeyBinding::new("cmd-7", SwitchToSession7, Some("Root")));
        bindings.push(KeyBinding::new("cmd-8", SwitchToSession8, Some("Root")));
        bindings.push(KeyBinding::new("cmd-9", SwitchToSession9, Some("Root")));
        bindings.push(KeyBinding::new("enter", crate::app::Send, Some("Composer")));
        bindings.push(KeyBinding::new(
            "cmd-enter",
            crate::app::Send,
            Some("Composer"),
        ));
        bindings.push(KeyBinding::new("cmd-.", Stop, Some("Composer")));
        cx.bind_keys(bindings);
    }

    /// The window's menu bar, the way Electron's menu is built.
    pub fn install_menus(cx: &mut App) {
        use gpui::{Menu, MenuItem, OsAction};
        cx.set_menus(vec![
            Menu {
                name: "Oxide".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("About Oxide", AboutApp),
                    MenuItem::separator(),
                    MenuItem::action("Check for Updates…", CheckForUpdates),
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Hide Oxide", HideApp),
                    MenuItem::separator(),
                    MenuItem::action("Quit Oxide", QuitApp),
                ],
            },
            Menu {
                name: "File".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("New Chat", NewChat),
                    MenuItem::action("Close Chat", CloseChat),
                ],
            },
            Menu {
                name: "Edit".into(),
                disabled: false,
                items: vec![
                    MenuItem::os_action("Cut", editor::Cut, OsAction::Cut),
                    MenuItem::os_action("Copy", editor::Copy, OsAction::Copy),
                    MenuItem::os_action("Paste", editor::Paste, OsAction::Paste),
                    MenuItem::os_action("Select All", editor::SelectAll, OsAction::SelectAll),
                ],
            },
            Menu {
                name: "View".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("Show or Hide Details", ToggleDetails),
                    MenuItem::action("Show or Hide the Sidebar", ToggleSidebar),
                    MenuItem::action("Activities", ToggleActivities),
                    MenuItem::action("Changes", ToggleChanges),
                    MenuItem::action("Terminal", OpenTerminal),
                    MenuItem::action("Pull Requests", OpenPulls),
                    MenuItem::action("Model Picker", OpenModels),
                    MenuItem::action("Git Worktrees", OpenWorktrees),
                    MenuItem::separator(),
                    MenuItem::action("Larger Text", IncreaseTextSize),
                    MenuItem::action("Smaller Text", DecreaseTextSize),
                    MenuItem::action("Actual Size", ResetTextSize),
                    MenuItem::separator(),
                    MenuItem::action("Back", Back),
                    MenuItem::action("Forward", Forward),
                    MenuItem::separator(),
                    MenuItem::action("Toggle Full Screen", ToggleFullscreen),
                ],
            },
            Menu {
                name: "Window".into(),
                disabled: false,
                items: vec![
                    MenuItem::action("Minimize", MinimizeWindow),
                    MenuItem::action("Close Window", CloseWindow),
                ],
            },
            Menu {
                name: "Help".into(),
                disabled: false,
                items: vec![MenuItem::action("Keyboard Shortcuts", ShowShortcuts)],
            },
        ]);
    }

    // ---------- commands ----------

    /// Dispatches one engine command and hands its answer back on the UI task.
    pub fn call<F>(&mut self, command: &'static str, args: Value, cx: &mut Context<Self>, then: F)
    where
        F: FnOnce(&mut Self, Result<Value, String>, &mut Context<Self>) + 'static,
    {
        let reply = self.engine.call(command, args);
        cx.spawn(async move |this, cx| {
            let result = reply
                .await
                .unwrap_or_else(|_| Err("the engine stopped answering".to_string()));
            let _ = this.update(cx, |root, cx| {
                then(root, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub fn toast(&mut self, text: impl Into<String>, error: bool, cx: &mut Context<Self>) {
        self.toast = Some(Toast {
            text: text.into(),
            error,
        });
        let deadline = Instant::now() + Duration::from_secs(4);
        cx.spawn(async move |this, cx| loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            if Instant::now() >= deadline {
                let _ = this.update(cx, |root, cx| {
                    root.toast = None;
                    cx.notify();
                });
                break;
            }
            if this.update(cx, |_, _| false).is_err() {
                break;
            }
        })
        .detach();
    }

    pub fn load_prefs(&mut self, cx: &mut Context<Self>) {
        self.call("read_prefs", json!({}), cx, |root, result, _| {
            if let Ok(prefs) = result {
                root.prefs = prefs;
                if let Some(visible) = root.prefs["sidebar_visible"].as_bool() {
                    root.sidebar_visible = visible;
                }
            }
        });
    }

    pub fn save_prefs(&mut self, patch: Value, cx: &mut Context<Self>) {
        self.call(
            "write_prefs",
            json!({ "patch": patch }),
            cx,
            move |root, result, cx| match result {
                Ok(prefs) => {
                    root.prefs = prefs;
                    if let Some(visible) = root.prefs["sidebar_visible"].as_bool() {
                        root.sidebar_visible = visible;
                    }
                    cx.notify();
                }
                Err(error) => root.toast(format!("Could not save that setting: {error}"), true, cx),
            },
        );
    }

    pub fn load_language(&mut self, cx: &mut Context<Self>) {
        let project = self.current_project();
        self.call(
            "language",
            json!({ "project": project }),
            cx,
            |root, result, _| {
                if let Ok(value) = result {
                    root.strings = Strings::load(&value);
                    root.language = value["language"].as_str().unwrap_or_default().to_string();
                    root.languages = value["languages"].as_array().cloned().unwrap_or_default();
                }
            },
        );
    }

    pub fn load_themes(&mut self, cx: &mut Context<Self>) {
        let project = self.current_project().unwrap_or_default();
        self.call(
            "list_themes",
            json!({ "project": project }),
            cx,
            move |root, result, cx| {
                if let Ok(value) = result {
                    let names = value["names"]
                        .as_array()
                        .map(|names| {
                            names
                                .iter()
                                .filter_map(|name| name.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                    root.theme_names = names;
                    for name in root.theme_names.clone() {
                        let project = project.clone();
                        root.call(
                            "theme_colors",
                            json!({ "project": project, "name": name }),
                            cx,
                            move |root, result, cx| {
                                if let Ok(value) = result {
                                    root.theme_previews
                                        .insert(name.clone(), Theme::from_value(&value));
                                    cx.notify();
                                }
                            },
                        );
                    }
                }
            },
        );
        self.apply_theme(cx);
    }

    pub fn apply_theme(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        let name = self.current_theme.clone();
        self.call(
            "theme_colors",
            json!({ "project": project, "name": name }),
            cx,
            |root, result, _| {
                if let Ok(value) = result {
                    root.theme = Theme::from_value(&value);
                    root.theme_previews
                        .insert(root.current_theme.clone(), root.theme.clone());
                }
            },
        );
    }

    pub fn set_theme(&mut self, name: String, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        self.call(
            "set_theme",
            json!({ "project": project, "name": name }),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    root.theme = Theme::from_value(&value);
                    root.current_theme = root.theme.name.clone();
                    root.theme_previews
                        .insert(root.current_theme.clone(), root.theme.clone());
                    cx.notify();
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    /// The launch's own install starts before this window listens, so its first
    /// steps were emitted to nobody; the engine keeps the newest one, and it is
    /// heard here through the same handler an event would reach.
    fn catch_up_on_launch_update(&mut self, cx: &mut Context<Self>) {
        self.call("launch_update", json!({}), cx, |root, result, cx| {
            let Ok(heard) = result else {
                return;
            };
            let Some(name) = heard["event"].as_str() else {
                return;
            };
            let payload = heard["payload"].clone();
            root.event(name, payload, cx);
            if name == "update-progress" {
                root.overlay = Some(Overlay::Updates);
            }
        });
    }

    /// What the rail's update button says, or nothing when there is no update
    /// to speak of.
    pub fn update_notice(&self) -> Option<&'static str> {
        if self.updating_update {
            let installing = self
                .update_progress
                .as_ref()
                .and_then(|progress| progress["stage"].as_str())
                == Some("installing");
            return Some(if installing {
                "Installing update"
            } else {
                "Downloading update"
            });
        }
        if let Some(result) = self.update_result.as_ref() {
            return Some(match result["pending"].as_bool() {
                Some(true) => "Installing update",
                Some(false) => "Restart and install update",
                None => "Update Available",
            });
        }
        let found = self
            .update_check
            .as_ref()
            .is_some_and(|check| check["updateAvailable"].as_bool() == Some(true));
        (found || self.update_flag).then_some("Update Available")
    }

    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        let installed = self
            .update_result
            .as_ref()
            .is_some_and(|result| result.get("pending").is_some());
        if self.updating_update || installed {
            self.overlay = Some(Overlay::Updates);
            cx.notify();
            return;
        }
        self.overlay = Some(Overlay::Updates);
        self.update_check = None;
        self.update_error = None;
        self.update_progress = None;
        self.update_result = None;
        self.update_failure = None;
        self.update_restart_failed = false;
        self.updating_update = false;
        self.update_token = None;
        self.checking_updates = true;
        self.call(
            "check_gpui_updates",
            json!({ "current": env!("CARGO_PKG_VERSION") }),
            cx,
            |root, result, cx| {
                root.checking_updates = false;
                match result {
                    Ok(value) => {
                        root.update_flag = value["updateAvailable"].as_bool() == Some(true);
                        root.update_check = Some(value);
                    }
                    Err(error) => root.update_error = Some(error),
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub fn install_update(&mut self, cx: &mut Context<Self>) {
        self.begin_update(false, cx);
    }

    pub fn download_update(&mut self, cx: &mut Context<Self>) {
        self.begin_update(true, cx);
    }

    fn begin_update(&mut self, download: bool, cx: &mut Context<Self>) {
        if self.updating_update {
            return;
        }
        let token = oxide_desktop::update::new_token();
        self.overlay = Some(Overlay::Updates);
        self.update_error = None;
        self.update_result = None;
        self.update_failure = None;
        self.update_restart_failed = false;
        self.update_progress = Some(json!({
            "stage": "checking",
            "version": self
                .update_check
                .as_ref()
                .and_then(|check| check["latest"].as_str())
                .unwrap_or_default(),
            "received": 0,
            "total": null,
            "token": token,
        }));
        self.update_token = Some(token.clone());
        self.updating_update = true;
        let expected = token.clone();
        let command = if download {
            "download_gpui_update"
        } else {
            "install_gpui_update"
        };
        self.call(
            command,
            json!({
                "current": env!("CARGO_PKG_VERSION"),
                "token": token,
            }),
            cx,
            move |root, result, cx| {
                if root.update_token.as_deref() != Some(expected.as_str()) {
                    return;
                }
                root.updating_update = false;
                root.update_progress = None;
                match result {
                    Ok(value) if value["cancelled"].as_bool().unwrap_or(false) => {
                        root.update_token = None;
                    }
                    Ok(value) => root.update_result = Some(value),
                    Err(error) => {
                        root.update_token = None;
                        root.update_failure = Some(error);
                    }
                }
                cx.notify();
            },
        );
        cx.notify();
    }

    pub fn cancel_update(&mut self, cx: &mut Context<Self>) {
        let Some(token) = self.update_token.take() else {
            return;
        };
        self.updating_update = false;
        self.update_progress = None;
        self.overlay = None;
        let expected = token.clone();
        self.call(
            "cancel_update",
            json!({ "token": token }),
            cx,
            move |root, result, cx| {
                if root.update_token.as_deref() == Some(expected.as_str()) {
                    if let Err(error) = result {
                        root.update_failure = Some(error);
                    }
                    cx.notify();
                }
            },
        );
        cx.notify();
    }

    pub fn restart_for_update(&mut self, cx: &mut Context<Self>) {
        self.update_failure = None;
        self.update_restart_failed = false;
        self.call("restart_app", json!({}), cx, |root, result, cx| {
            if let Err(error) = result {
                root.update_failure = Some(error);
                root.update_restart_failed = true;
                cx.notify();
            }
        });
    }

    fn set_text_scale(&mut self, scale: f64, cx: &mut Context<Self>) {
        self.save_prefs(json!({ "font_scale": scale.clamp(0.7, 1.6) }), cx);
        cx.notify();
    }

    fn adjust_text_scale(&mut self, delta: f64, cx: &mut Context<Self>) {
        let current = self.prefs["font_scale"].as_f64().unwrap_or(1.0);
        let stepped = ((current * 10.0).round() / 10.0 + delta).clamp(0.7, 1.6);
        self.set_text_scale((stepped * 10.0).round() / 10.0, cx);
    }

    pub fn set_language(&mut self, tag: String, cx: &mut Context<Self>) {
        let project = self.current_project().unwrap_or_default();
        self.call(
            "set_language",
            json!({ "project": project, "tag": tag }),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    root.strings = Strings::load(&value);
                    root.languages = value["languages"].as_array().cloned().unwrap_or_default();
                    root.language = value["language"].as_str().unwrap_or_default().to_string();
                    cx.notify();
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn refresh_projects(&mut self, cx: &mut Context<Self>) {
        self.call("list_projects", json!({}), cx, |root, result, cx| {
            root.projects_loaded = true;
            match result {
                Ok(value) => {
                    root.projects = crate::model::projects(&value);
                    root.refresh_sessions(cx);
                }
                Err(error) => root.toast(format!("Could not list projects: {error}"), true, cx),
            }
        });
    }

    pub fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        self.call("all_sessions", json!({}), cx, |root, result, _| {
            if let Ok(value) = result {
                root.sessions = value
                    .as_array()
                    .map(|sessions| sessions.iter().filter_map(SessionRow::from_value).collect())
                    .unwrap_or_default();
                synchronize_session_titles(&mut root.chats, &root.sessions);
            }
        });
    }

    pub fn search_sessions(&mut self, cx: &mut Context<Self>) {
        let query = self.query.trim().to_string();
        if query.is_empty() {
            self.results.clear();
            return;
        }
        self.call(
            "search_sessions",
            json!({ "query": query }),
            cx,
            move |root, result, _| {
                if let Ok(value) = result {
                    root.results = value
                        .as_array()
                        .map(|matches| matches.iter().filter_map(SessionRow::from_value).collect())
                        .unwrap_or_default();
                }
            },
        );
    }

    /// The sidebar's search: the field takes the brand row's place and the caret
    /// goes into it, so looking for a thread is one click rather than an always
    /// present box above the list.
    pub fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.activities_open = false;
        self.searching = true;
        let editor = self.search.clone();
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    pub fn close_search(&mut self, cx: &mut Context<Self>) {
        self.searching = false;
        self.query.clear();
        self.results.clear();
        let editor = self.search.clone();
        editor.update(cx, |editor, cx| editor.clear(cx));
        cx.notify();
    }

    pub fn toggle_activities(&mut self, cx: &mut Context<Self>) {
        self.activities_open = !self.activities_open;
        if self.activities_open {
            self.searching = false;
            self.query.clear();
            self.results.clear();
            self.overlay = None;
        }
        cx.notify();
    }

    pub fn search_text(&self, cx: &App) -> String {
        self.search.read(cx).text().to_string()
    }

    pub fn current_project(&self) -> Option<String> {
        if let Some(chat) = self.chats.get(self.active) {
            return Some(chat.project.clone());
        }
        self.projects
            .iter()
            .find(|project| project.registered)
            .map(|project| project.path.clone())
            .or_else(|| self.projects.first().map(|project| project.path.clone()))
    }

    pub fn load_project_info(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        let expected = project.clone();
        self.call(
            "project_info",
            json!({ "project": project }),
            cx,
            move |root, result, cx| {
                if let Ok(value) = result {
                    if root.current_project().as_deref() != Some(expected.as_str()) {
                        return;
                    }
                    let awaiting = project_trust_awaiting(&value);
                    root.info = value;
                    if awaiting {
                        root.model_menu_open = false;
                        root.overlay = Some(Overlay::Trust {
                            project: expected.clone(),
                            onboarding: false,
                        });
                    }
                    cx.notify();
                }
            },
        );
        self.load_pulls(cx);
    }

    pub fn load_models(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        self.call(
            "list_models",
            json!({ "project": project, "refresh": false }),
            cx,
            |root, result, _cx| match result {
                Ok(value) => root.models = value,
                Err(error) => {
                    root.models = json!({ "error": error });
                }
            },
        );
    }

    pub fn toggle_model_menu(&mut self, cx: &mut Context<Self>) {
        if self.model_menu_open {
            self.model_menu_open = false;
            self.model_menu_show_models = false;
            cx.notify();
        } else {
            self.open_model_menu(cx);
        }
    }

    pub fn open_model_menu(&mut self, cx: &mut Context<Self>) {
        self.overlay = None;
        self.rail = Rail::Chats;
        self.composer_tools_open = false;
        self.model_menu_open = true;
        self.model_menu_show_models = false;
        self.load_models(cx);
        cx.notify();
    }

    pub fn load_approvals(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        self.call(
            "list_approvals",
            json!({ "project": project }),
            cx,
            |root, result, _| {
                if let Ok(value) = result {
                    root.approvals = value
                        .as_array()
                        .map(|tools| {
                            tools
                                .iter()
                                .filter_map(|tool| tool.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
                }
            },
        );
    }

    pub fn open_computer_use(&mut self, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::ComputerUse);
        self.load_computer_use_policy(cx);
    }

    pub fn load_computer_use_policy(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            self.computer_use_policy = ComputerUsePolicy {
                error: Some("Select a project to configure app access.".to_string()),
                ..ComputerUsePolicy::default()
            };
            cx.notify();
            return;
        };
        self.computer_use_policy.loading = true;
        self.computer_use_policy.error = None;
        self.computer_use_policy.project = project.clone();
        self.call(
            "computer_use_policy",
            json!({ "project": project, "action": "list" }),
            cx,
            |root, result, cx| {
                let expected = root.computer_use_policy.project.clone();
                if root.current_project().as_deref() != Some(expected.as_str()) {
                    return;
                }
                root.computer_use_policy.loading = false;
                match result {
                    Ok(value) => {
                        root.computer_use_policy.configured =
                            value["configured"].as_bool().unwrap_or(false);
                        root.computer_use_policy.server =
                            value["server"].as_str().unwrap_or_default().to_string();
                        root.computer_use_policy.value = value["policy"].clone();
                        root.computer_use_policy.error = (!root.computer_use_policy.configured)
                            .then(|| {
                                value["error"]
                                    .as_str()
                                    .unwrap_or(
                                        "oxide-computer-use is not configured for this project",
                                    )
                                    .to_string()
                            });
                    }
                    Err(error) => {
                        root.computer_use_policy.configured = false;
                        root.computer_use_policy.error = Some(error);
                    }
                }
                cx.notify();
            },
        );
    }

    pub fn change_computer_use_policy(
        &mut self,
        action: String,
        app: Option<String>,
        mode: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.current_project() else {
            return;
        };
        let expected = project.clone();
        self.computer_use_policy.loading = true;
        self.computer_use_policy.error = None;
        self.call(
            "computer_use_policy",
            json!({
                "project": project,
                "action": action,
                "app": app,
                "mode": mode,
            }),
            cx,
            move |root, result, cx| match result {
                Ok(value) => {
                    if root.current_project().as_deref() != Some(expected.as_str()) {
                        return;
                    }
                    root.computer_use_policy.loading = false;
                    root.computer_use_policy.configured =
                        value["configured"].as_bool().unwrap_or(false);
                    root.computer_use_policy.server =
                        value["server"].as_str().unwrap_or_default().to_string();
                    root.computer_use_policy.value = value["policy"].clone();
                    if !root.computer_use_policy.configured {
                        root.computer_use_policy.error = Some(
                            value["error"]
                                .as_str()
                                .unwrap_or("oxide-computer-use is not configured for this project")
                                .to_string(),
                        );
                    }
                    cx.notify();
                }
                Err(error) => {
                    if root.current_project().as_deref() != Some(expected.as_str()) {
                        return;
                    }
                    root.computer_use_policy.loading = false;
                    root.computer_use_policy.error = Some(error);
                    cx.notify();
                }
            },
        );
    }

    pub fn load_git(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            self.branch = None;
            return;
        };
        self.call(
            "git_info",
            json!({ "project": project }),
            cx,
            |root, result, cx| {
                root.branch = result.ok().and_then(|value| {
                    if !value["repo"].as_bool().unwrap_or(false) {
                        return None;
                    }
                    let branch = value["branch"].as_str().unwrap_or_default().trim();
                    if !branch.is_empty() {
                        return Some(branch.to_string());
                    }
                    let detached = value["detached"].as_str().unwrap_or_default().trim();
                    (!detached.is_empty()).then(|| detached.to_string())
                });
                cx.notify();
            },
        );
    }

    pub fn load_sources(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            self.sources.clear();
            return;
        };
        self.call(
            "mcp_servers",
            json!({ "project": project }),
            cx,
            |root, result, cx| {
                if let Ok(value) = result {
                    root.sources = value.as_array().cloned().unwrap_or_default();
                    cx.notify();
                }
            },
        );
    }

    pub fn load_pulls(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            self.pulls.clear();
            self.pulls_error = None;
            return;
        };
        self.call(
            "pull_requests",
            json!({ "project": project }),
            cx,
            |root, result, _cx| match result {
                Ok(value) => {
                    root.pulls = value.as_array().cloned().unwrap_or_default();
                    root.pulls_error = None;
                }
                Err(error) => {
                    root.pulls.clear();
                    root.pulls_error = Some(error);
                    root.pull_detail = None;
                    root.pull_open = None;
                }
            },
        );
    }

    pub fn open_pulls(&mut self, cx: &mut Context<Self>) {
        if self.show_changes && self.panel_tab == PanelTab::Pulls {
            self.close_workspace_panel(cx);
            return;
        }
        self.stop_terminal(cx);
        self.show_changes = true;
        self.panel_tab = PanelTab::Pulls;
        self.changes_width = self.changes_width.max(500.0);
        self.load_pulls(cx);
        cx.notify();
    }

    pub fn load_pull(&mut self, number: u64, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        self.pull_open = Some(number);
        self.pull_loading = true;
        self.pull_detail = None;
        self.call(
            "pull_request",
            json!({ "project": project, "number": number }),
            cx,
            move |root, result, cx| {
                if root.pull_open != Some(number) {
                    return;
                }
                root.pull_loading = false;
                match result {
                    Ok(value) => root.pull_detail = Some(value),
                    Err(error) => root.toast(error, true, cx),
                }
            },
        );
    }

    pub fn open_terminal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.show_changes && self.panel_tab == PanelTab::Terminal {
            self.close_workspace_panel(cx);
            return;
        }
        self.show_changes = true;
        self.panel_tab = PanelTab::Terminal;
        self.changes_width = self.changes_width.max(620.0);
        window.focus(&self.terminal_focus, cx);
        if self.terminal.id.is_none() {
            self.start_terminal(cx);
        }
        cx.notify();
    }

    fn start_terminal(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            self.toast("Select a project first.", true, cx);
            return;
        };
        let fallback_cwd = project.clone();
        self.terminal.error = None;
        self.call(
            "terminal_open",
            json!({
                "project": project,
                "cols": self.terminal.cols,
                "rows": self.terminal.rows,
            }),
            cx,
            move |root, result, _cx| match result {
                Ok(value) => {
                    let id = value["id"].as_u64().unwrap_or_default();
                    let cwd = value["cwd"].as_str().unwrap_or(&fallback_cwd).to_string();
                    root.terminal.start(id, cwd);
                }
                Err(error) => root.terminal.error = Some(error.clone()),
            },
        );
    }

    pub fn restart_terminal(&mut self, cx: &mut Context<Self>) {
        let id = self.terminal.id;
        self.terminal.stop();
        if let Some(id) = id {
            self.call("terminal_close", json!({ "id": id }), cx, |_, _, _| {});
        }
        self.start_terminal(cx);
    }

    pub fn terminal_key(&mut self, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let Some(id) = self.terminal.id else {
            return;
        };
        let Some(data) =
            crate::workspace::terminal_key_bytes(event, self.terminal.application_cursor())
        else {
            return;
        };
        let submitted = data == "\r";
        self.call(
            "terminal_write",
            json!({ "id": id, "data": data }),
            cx,
            |root, result, cx| {
                if let Err(error) = result {
                    root.terminal.error = Some(error.clone());
                    cx.notify();
                }
            },
        );
        if submitted {
            self.read_branch_after_terminal(cx);
        }
    }

    /// A command typed in the dock's own terminal may have switched the folder's
    /// branch, and this window never left its own focus to hear about it: the
    /// branch is read again once the command has had time to run.
    fn read_branch_after_terminal(&mut self, cx: &mut Context<Self>) {
        self.terminal_generation = self.terminal_generation.wrapping_add(1);
        let generation = self.terminal_generation;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(700))
                .await;
            let _ = this.update(cx, |root, cx| {
                if root.terminal_generation == generation {
                    root.load_git(cx);
                }
            });
        })
        .detach();
    }

    pub fn terminal_write_text(&mut self, text: &'static str, cx: &mut Context<Self>) {
        let Some(id) = self.terminal.id else {
            return;
        };
        self.call(
            "terminal_write",
            json!({ "id": id, "data": text }),
            cx,
            |root, result, cx| {
                if let Err(error) = result {
                    root.terminal.error = Some(error);
                    cx.notify();
                }
            },
        );
    }

    pub fn close_workspace_panel(&mut self, cx: &mut Context<Self>) {
        self.show_changes = false;
        self.stop_terminal(cx);
        cx.notify();
    }

    fn stop_terminal(&mut self, cx: &mut Context<Self>) {
        let id = self.terminal.id;
        self.terminal.stop();
        if let Some(id) = id {
            self.call("terminal_close", json!({ "id": id }), cx, |_, _, _| {});
        }
    }

    pub fn select_panel(&mut self, tab: PanelTab, window: &mut Window, cx: &mut Context<Self>) {
        match tab {
            PanelTab::Changes => {
                self.stop_terminal(cx);
                self.open_repository_changes(cx);
            }
            PanelTab::Terminal => self.open_terminal(window, cx),
            PanelTab::Pulls => self.open_pulls(cx),
        }
    }

    pub fn open_file(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        self.file_viewer = Some(FileViewer {
            path: path.clone(),
            kind: String::new(),
            text: String::new(),
            image: None,
            truncated: false,
            source: false,
            loading: true,
            error: None,
        });
        self.overlay = Some(Overlay::FileViewer);
        self.call(
            "file_view",
            json!({ "project": project, "path": path }),
            cx,
            move |root, result, _| {
                let Some(viewer) = root.file_viewer.as_mut() else {
                    return;
                };
                if viewer.path != path {
                    return;
                }
                viewer.loading = false;
                match result {
                    Ok(value) => {
                        viewer.kind = value["kind"].as_str().unwrap_or("text").to_string();
                        viewer.text = value["text"].as_str().unwrap_or_default().to_string();
                        viewer.truncated = value["truncated"].as_bool().unwrap_or(false);
                        viewer.image = value["data_url"]
                            .as_str()
                            .and_then(crate::model::image_from_data_url);
                    }
                    Err(error) => viewer.error = Some(error),
                }
            },
        );
    }

    pub fn toggle_file_source(&mut self, cx: &mut Context<Self>) {
        if let Some(viewer) = self.file_viewer.as_mut() {
            viewer.source = !viewer.source;
            cx.notify();
        }
    }

    pub fn shape_change(
        &mut self,
        action: String,
        path: Option<String>,
        hunk: Option<Value>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self
            .change_review
            .as_ref()
            .map(|review| review.project.clone())
            .or_else(|| self.current_project())
        else {
            return;
        };
        let mut args = json!({ "project": project, "action": action });
        if let Some(path) = path.clone() {
            args["path"] = json!(path);
        }
        if let Some(hunk) = hunk {
            args["hunk"] = hunk;
        }
        self.call("shape_change", args, cx, |root, result, cx| match result {
            Ok(_) => root.refresh_change_review(cx),
            Err(error) => root.toast(error, true, cx),
        });
    }

    pub fn load_plugins(&mut self, cx: &mut Context<Self>) {
        self.plugin_catalog = Value::Null;
        self.plugin_message = None;
        self.call(
            "plugin_catalog",
            json!({}),
            cx,
            |root, result, _| match result {
                Ok(value) => root.plugin_catalog = value,
                Err(error) => {
                    root.plugin_message = Some(format!("Could not load plugins: {error}"))
                }
            },
        );
    }

    pub fn change_plugin(
        &mut self,
        action: String,
        name: String,
        marketplace: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.plugin_busy {
            return;
        }
        self.plugin_busy = true;
        self.plugin_message = Some(format!("Working on {name}…"));
        let mut args = json!({ "action": action, "name": name });
        if let Some(marketplace) = marketplace {
            args["marketplace"] = json!(marketplace);
        }
        self.call("change_plugin", args, cx, |root, result, cx| {
            root.plugin_busy = false;
            match result {
                Ok(message) => {
                    root.plugin_message = message.as_str().map(str::to_string);
                    root.load_plugins(cx);
                    root.load_project_commands(cx);
                }
                Err(error) => root.plugin_message = Some(format!("Plugin change failed: {error}")),
            }
        });
    }

    pub fn toggle_mcp(&mut self, name: String, enabled: bool, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        self.call(
            "set_mcp_server",
            json!({ "project": project, "name": name, "enabled": enabled }),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    root.sources = value.as_array().cloned().unwrap_or_default();
                    root.load_project_info(cx);
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn cycle_mcp_exposure(&mut self, name: String, current: String, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        let exposure = match current.as_str() {
            "deferred" => "direct",
            "direct" => "hidden",
            _ => "deferred",
        };
        self.call(
            "set_mcp_exposure",
            json!({ "project": project, "name": name, "exposure": exposure }),
            cx,
            |root, result, cx| match result {
                Ok(value) => root.sources = value.as_array().cloned().unwrap_or_default(),
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn authorize_mcp(&mut self, name: String, scope: String, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        self.call(
            "authorize_mcp_server",
            json!({ "project": project, "name": name, "scope": scope }),
            cx,
            |root, result, cx| match result {
                Ok(value) => root.sources = value.as_array().cloned().unwrap_or_default(),
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn open_connect(&mut self, provider: String, cx: &mut Context<Self>) {
        self.provider_filter
            .update(cx, |editor, cx| editor.clear(cx));
        self.provider_key.update(cx, |editor, cx| editor.clear(cx));
        self.provider_model
            .update(cx, |editor, cx| editor.clear(cx));
        self.provider_url.update(cx, |editor, cx| editor.clear(cx));
        self.overlay = Some(Overlay::Connect { provider });
        self.call(
            "list_providers",
            json!({}),
            cx,
            |root, result, cx| match result {
                Ok(value) => root.providers = value.as_array().cloned().unwrap_or_default(),
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn login_provider(&mut self, provider: String, cx: &mut Context<Self>) {
        let key = self.provider_key.read(cx).text().trim().to_string();
        let model = self.provider_model.read(cx).text().trim().to_string();
        let base_url = self.provider_url.read(cx).text().trim().to_string();
        let mut args = json!({ "provider": provider });
        if !key.is_empty() {
            args["key"] = json!(key);
        }
        if !model.is_empty() {
            args["model"] = json!(model);
        }
        if !base_url.is_empty() {
            args["baseUrl"] = json!(base_url);
        }
        self.call("login", args, cx, |root, result, cx| match result {
            Ok(_) => {
                root.overlay = None;
                root.provider_key.update(cx, |editor, cx| editor.clear(cx));
                root.load_project_info(cx);
                root.load_models(cx);
                root.toast("Provider connected.", false, cx);
            }
            Err(error) => root.toast(format!("Login failed: {error}"), true, cx),
        });
    }

    pub fn accept_prediction(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.prediction.is_empty() {
            return;
        }
        let Some(id) = self.chats.get(self.active).map(|chat| chat.id) else {
            return;
        };
        let Some(editor) = self.editors.get(&id).cloned() else {
            return;
        };
        let current = editor.read(cx).text().to_string();
        let combined = if current.trim().is_empty() {
            self.prediction.clone()
        } else {
            format!("{} {}", current.trim_end(), self.prediction)
        };
        self.prediction.clear();
        self.prediction_chat = None;
        self.prediction_generation = self.prediction_generation.wrapping_add(1);
        editor.update(cx, |editor, cx| editor.set_text(combined, cx));
        window.focus(&editor.read(cx).focus_handle(cx), cx);
        cx.notify();
    }

    fn maybe_predict(&mut self, cx: &mut Context<Self>) {
        self.prediction.clear();
        self.prediction_chat = None;
        if !self.prefs["predictions"].as_bool().unwrap_or(false) {
            return;
        }
        let Some(chat) = self.chats.get(self.active) else {
            return;
        };
        let (Some(project), Some(id)) = (Some(chat.project.clone()), chat.session.clone()) else {
            return;
        };
        if self
            .editors
            .get(&chat.id)
            .is_some_and(|editor| !editor.read(cx).text().trim().is_empty())
        {
            return;
        }
        let chat_id = chat.id;
        self.prediction_generation = self.prediction_generation.wrapping_add(1);
        let generation = self.prediction_generation;
        self.call(
            "predict",
            json!({ "project": project, "id": id }),
            cx,
            move |root, result, cx| {
                if generation != root.prediction_generation {
                    return;
                }
                let Ok(value) = result else {
                    return;
                };
                let suggestion = value["suggestion"].as_str().unwrap_or_default().trim();
                if suggestion.is_empty()
                    || root.chats.get(root.active).map(|chat| chat.id) != Some(chat_id)
                    || root
                        .editors
                        .get(&chat_id)
                        .is_some_and(|editor| !editor.read(cx).text().trim().is_empty())
                {
                    return;
                }
                root.prediction = suggestion.to_string();
                root.prediction_chat = Some(chat_id);
                cx.notify();
            },
        );
    }

    pub fn current_project_owner(&self) -> Option<String> {
        let current = self.current_project()?;
        if let Some(worktree) = self
            .worktrees
            .iter()
            .find(|worktree| worktree.path == current)
        {
            return Some(worktree.project.clone());
        }
        Some(
            Worktrees::open()
                .project_of(Path::new(&current))
                .map(|path| path.to_string_lossy().to_string())
                .unwrap_or(current),
        )
    }

    pub fn current_worktree(&self) -> Option<&Worktree> {
        let current = self.current_project()?;
        self.worktrees
            .iter()
            .find(|worktree| worktree.path == current)
    }

    pub fn load_worktrees(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project_owner() else {
            self.worktrees.clear();
            self.worktrees_error = None;
            return;
        };
        self.call(
            "worktrees",
            json!({ "project": project }),
            cx,
            |root, result, _cx| match result {
                Ok(value) => {
                    root.worktrees = value
                        .as_array()
                        .map(|entries| {
                            entries
                                .iter()
                                .filter_map(|entry| serde_json::from_value(entry.clone()).ok())
                                .collect()
                        })
                        .unwrap_or_default();
                    root.worktrees_error = None;
                }
                Err(error) => {
                    root.worktrees.clear();
                    root.worktrees_error = Some(error);
                }
            },
        );
    }

    pub fn load_commands(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            self.commands.clear();
            return;
        };
        self.call(
            "list_commands",
            json!({ "project": project }),
            cx,
            |root, result, cx| {
                if let Ok(value) = result {
                    root.commands = value.as_array().cloned().unwrap_or_default();
                    cx.notify();
                }
            },
        );
    }

    fn refresh_at_suggestions(
        &mut self,
        chat_id: u64,
        text: String,
        caret: usize,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.current_project() else {
            self.at_answer = Value::Null;
            self.at_chat = None;
            return;
        };
        if !text.contains('@') {
            self.at_answer = Value::Null;
            self.at_chat = None;
            return;
        }
        self.at_generation = self.at_generation.wrapping_add(1);
        let generation = self.at_generation;
        self.call(
            "at_suggestions",
            json!({ "project": project, "text": text, "caret": caret }),
            cx,
            move |root, result, _cx| {
                if root.at_generation != generation {
                    return;
                }
                match result {
                    Ok(answer) => {
                        let has_rows = answer["rows"]
                            .as_array()
                            .is_some_and(|rows| !rows.is_empty());
                        root.at_answer = if has_rows { answer } else { Value::Null };
                        root.at_chat = has_rows.then_some(chat_id);
                    }
                    Err(_) => {
                        root.at_answer = Value::Null;
                        root.at_chat = None;
                    }
                }
            },
        );
    }

    pub fn open_worktrees(&mut self, cx: &mut Context<Self>) {
        self.load_worktrees(cx);
        self.overlay = Some(Overlay::Worktrees);
        cx.notify();
    }

    pub fn select_worktree(
        &mut self,
        window: &mut Window,
        path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let Some(project) = self.current_project_owner() else {
            self.overlay = Some(Overlay::NewProject);
            cx.notify();
            return;
        };
        let Some(folder) = path else {
            self.overlay = None;
            self.push_chat(window, project, None, "New chat".to_string(), cx);
            return;
        };
        let name = self
            .worktrees
            .iter()
            .find(|worktree| worktree.path == folder)
            .map(|worktree| worktree.name.clone())
            .unwrap_or_else(|| "New chat".to_string());
        self.overlay = None;
        self.push_chat(window, folder, None, name, cx);
    }

    pub fn select_project(&mut self, window: &mut Window, project: String, cx: &mut Context<Self>) {
        self.rail = Rail::Chats;
        self.overlay = None;
        self.inline_project_actions = None;
        if self.current_project_owner().as_deref() == Some(project.as_str()) {
            self.focus_composer(window, cx);
            cx.notify();
            return;
        }
        // Picking another folder is a move to that folder, not another thread:
        // the draft nothing has been written in yet follows the picker, so
        // clicking through the sidebar leaves one tab to type in. A tab that
        // already holds work keeps it and the picker opens its own.
        if self.retarget_draft(project.clone(), cx) {
            self.focus_composer(window, cx);
            cx.notify();
            return;
        }
        self.push_chat(window, project, None, "New chat".to_string(), cx);
    }

    /// Points the active tab's untouched draft at `project`, answering whether
    /// there was one to move. Everything the folder decides for a thread — its
    /// commands, its repository — is read again, since the draft now stands in
    /// a different one.
    fn retarget_draft(&mut self, project: String, cx: &mut Context<Self>) -> bool {
        let index = self.active;
        let Some(chat) = self.chats.get(index) else {
            return false;
        };
        let Some(editor) = self.editors.get(&chat.id).cloned() else {
            return false;
        };
        if !draft_follows_project(chat, editor.read(cx).text(), &project) {
            return false;
        }
        self.changes = None;
        self.show_changes = false;
        self.stop_terminal(cx);
        self.prediction.clear();
        self.prediction_chat = None;
        self.prediction_generation = self.prediction_generation.wrapping_add(1);
        self.at_answer = Value::Null;
        self.at_chat = None;
        self.chats[index].project = project;
        self.load_project_commands(cx);
        self.refresh_repository(index, cx);
        true
    }

    pub fn create_worktree(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project_owner() else {
            return;
        };
        let name = self.worktree_name.read(cx).text().trim().to_string();
        let base = self.worktree_base.read(cx).text().trim().to_string();
        let detached = self.worktree_detached;
        let copy_changes = self.worktree_copy_changes;
        self.call(
            "create_worktree",
            json!({
                "project": project,
                "name": if name.is_empty() { Value::Null } else { json!(name) },
                "base": if base.is_empty() { Value::Null } else { json!(base) },
                "detached": detached,
                "copyChanges": copy_changes,
                "source": Value::Null,
            }),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    if let Ok(worktree) = serde_json::from_value::<Worktree>(value) {
                        root.toast(
                            format!("Created worktree {} on {}.", worktree.name, worktree.branch),
                            false,
                            cx,
                        );
                    }
                    root.worktree_name.update(cx, |editor, cx| editor.clear(cx));
                    root.worktree_base.update(cx, |editor, cx| editor.clear(cx));
                    root.load_worktrees(cx);
                }
                Err(error) => root.toast(format!("Could not create worktree: {error}"), true, cx),
            },
        );
    }

    pub fn create_worktree_branch(
        &mut self,
        project: String,
        name: String,
        cx: &mut Context<Self>,
    ) {
        let branch = self.worktree_branch.read(cx).text().trim().to_string();
        if branch.is_empty() {
            return;
        }
        self.call(
            "create_worktree_branch",
            json!({ "project": project, "name": name, "branch": branch }),
            cx,
            |root, result, cx| match result {
                Ok(_) => {
                    root.overlay = Some(Overlay::Worktrees);
                    root.worktree_branch
                        .update(cx, |editor, cx| editor.clear(cx));
                    root.load_worktrees(cx);
                }
                Err(error) => root.toast(format!("Could not create branch: {error}"), true, cx),
            },
        );
    }

    pub fn remove_worktree(&mut self, project: String, name: String, cx: &mut Context<Self>) {
        let removed_path = self
            .worktrees
            .iter()
            .find(|worktree| worktree.name == name)
            .map(|worktree| worktree.path.clone());
        let local_path = project.clone();
        self.call(
            "remove_worktree",
            json!({ "project": project, "name": name }),
            cx,
            move |root, result, cx| match result {
                Ok(_) => {
                    if let Some(removed_path) = &removed_path {
                        for chat in &mut root.chats {
                            if &chat.project == removed_path {
                                chat.project = local_path.clone();
                            }
                        }
                    }
                    root.overlay = Some(Overlay::Worktrees);
                    root.load_worktrees(cx);
                    let index = root.active;
                    root.refresh_repository(index, cx);
                }
                Err(error) => root.toast(format!("Could not remove worktree: {error}"), true, cx),
            },
        );
    }

    pub fn handoff_current_session(&mut self, cx: &mut Context<Self>) {
        let index = self.active;
        let Some(chat) = self.chats.get(index) else {
            return;
        };
        let Some(id) = chat.session.clone() else {
            return;
        };
        let source = chat.project.clone();
        let Some(project) = self.current_project_owner() else {
            return;
        };
        let to_worktree = source == project;
        self.call(
            "handoff_session",
            json!({
                "project": project,
                "source": source,
                "id": id,
                "toWorktree": to_worktree,
            }),
            cx,
            move |root, result, cx| match result {
                Ok(value) => {
                    if let Some(cwd) = value["cwd"].as_str() {
                        if let Some(chat) = root.chats.get_mut(index) {
                            chat.project = cwd.to_string();
                        }
                    }
                    root.overlay = None;
                    root.load_project_commands(cx);
                    root.refresh_repository(index, cx);
                    root.refresh_sessions(cx);
                    root.toast(
                        if to_worktree {
                            "Chat moved to its worktree."
                        } else {
                            "Chat moved to Local."
                        },
                        false,
                        cx,
                    );
                }
                Err(error) => root.toast(format!("Could not hand off chat: {error}"), true, cx),
            },
        );
    }

    pub fn load_project_commands(&mut self, cx: &mut Context<Self>) {
        self.load_project_info(cx);
        self.load_models(cx);
        self.load_approvals(cx);
        self.load_git(cx);
        self.load_sources(cx);
        self.load_worktrees(cx);
        self.load_commands(cx);
    }

    pub fn toggle_details(&mut self, cx: &mut Context<Self>) {
        if self.show_changes {
            self.close_workspace_panel(cx);
            self.show_details = true;
            self.change_review = None;
            let index = self.active;
            self.refresh_repository(index, cx);
            self.load_sources(cx);
            cx.notify();
            return;
        }
        let next = toggled_details(self.show_details, !self.chats.is_empty());
        if next == self.show_details {
            return;
        }
        self.show_details = next;
        if self.show_details {
            self.show_changes = false;
            self.change_review = None;
            self.stop_terminal(cx);
            let index = self.active;
            self.refresh_repository(index, cx);
            self.load_sources(cx);
        }
        cx.notify();
    }

    /// Puts a draft in the composer and puts the caret in it: what a welcome
    /// suggestion does, since a suggestion is text to edit rather than a prompt
    /// that was sent.
    pub fn fill_composer(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.chats.is_empty() {
            self.new_chat(window, cx);
        }
        let Some(id) = self.chats.get(self.active).map(|chat| chat.id) else {
            return;
        };
        if let Some(editor) = self.editors.get(&id).cloned() {
            editor.update(cx, |editor, cx| {
                editor.set_text(text, cx);
            });
            window.focus(&editor.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }

    // ---------- chats ----------

    /// The project a new chat in the active column belongs to.
    pub fn new_chat_project(&self) -> Option<String> {
        self.current_project()
    }

    pub fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let reusable = self.chats.get(self.active).and_then(|chat| {
            let editor = self.editors.get(&chat.id)?;
            can_reuse_new_chat(chat, editor.read(cx).text()).then_some(editor.clone())
        });
        if let Some(editor) = reusable {
            window.focus(&editor.read(cx).focus_handle(cx), cx);
            cx.notify();
            return;
        }
        let Some(project) = self.new_chat_project() else {
            self.open_new_project(window, cx);
            return;
        };
        self.push_chat(window, project, None, "New chat".to_string(), cx);
    }

    pub fn open_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.project_name.read(cx).focus_handle(cx);
        self.open_overlay(Overlay::NewProject, cx);
        window.focus(&focus, cx);
    }

    pub fn open_command_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let focus = self.command_query.read(cx).focus_handle(cx);
        self.open_overlay(Overlay::CommandMenu, cx);
        window.focus(&focus, cx);
    }

    pub fn push_chat(
        &mut self,
        window: &mut Window,
        project: String,
        session: Option<String>,
        title: String,
        cx: &mut Context<Self>,
    ) {
        self.push_chat_with_focus(Some(window), project, session, title, cx);
    }

    fn push_chat_in_memory(
        &mut self,
        project: String,
        session: Option<String>,
        title: String,
        cx: &mut Context<Self>,
    ) {
        self.push_chat_with_focus(None, project, session, title, cx);
    }

    fn push_chat_with_focus(
        &mut self,
        window: Option<&mut Window>,
        project: String,
        session: Option<String>,
        title: String,
        cx: &mut Context<Self>,
    ) {
        self.model_menu_open = false;
        self.composer_tools_open = false;
        self.prediction.clear();
        self.prediction_chat = None;
        self.prediction_generation = self.prediction_generation.wrapping_add(1);
        self.stop_terminal(cx);
        let id = self.next_chat;
        self.next_chat += 1;
        let mut chat = Chat::new(project.clone(), title);
        chat.id = id;
        chat.session = session.clone();
        chat.loaded = session.is_none();

        let theme = self.theme.clone();
        let placeholder = self.strings.t("composer.placeholder", "Ask Oxide…");
        let editor = cx.new(move |cx| {
            Editor::new(
                placeholder,
                theme.text(),
                theme.dim(),
                theme.faint(),
                theme.accent(),
                theme.selection(),
                cx,
            )
        });
        self.subscriptions
            .push(cx.subscribe(&editor, move |root, editor, event, cx| {
                if let EditorEvent::AttachmentPasted { name, data_url } = event {
                    let result = root
                        .chats
                        .iter_mut()
                        .find(|chat| chat.id == id)
                        .map(|chat| {
                            add_attachment(chat, Attachment::new(name.clone(), data_url.clone()))
                        })
                        .unwrap_or(Ok(()));
                    if let Err(error) = result {
                        root.toast(error, true, cx);
                    }
                } else {
                    let (text, caret) = {
                        let editor = editor.read(cx);
                        (editor.text().to_string(), editor.cursor_utf16())
                    };
                    if !text.is_empty() && root.prediction_chat == Some(id) {
                        root.prediction.clear();
                        root.prediction_chat = None;
                        root.prediction_generation = root.prediction_generation.wrapping_add(1);
                    }
                    root.refresh_at_suggestions(id, text, caret, cx);
                }
                cx.notify();
            }));

        self.editors.insert(id, editor);
        self.chats.push(chat);
        self.active = self.chats.len() - 1;
        self.changes = None;
        self.show_changes = false;

        if let Some(session) = session {
            self.open_session_messages(project, session, cx);
        }
        self.load_project_commands(cx);
        // The work tree's own totals are what the composer's chip and the
        // details pane show, so they are read with the thread rather than only
        // when a turn ends.
        self.refresh_repository(self.active, cx);
        if let Some(window) = window {
            if let Some(editor) = self.editors.get(&id) {
                window.focus(&editor.read(cx).focus_handle(cx), cx);
            }
        } else {
            self.pending_focus = Some(id);
        }
        cx.notify();
    }

    fn open_session_messages(&mut self, project: String, session: String, cx: &mut Context<Self>) {
        let index = self.active;
        self.call(
            "session_messages",
            json!({ "project": project, "id": session }),
            cx,
            move |root, result, cx| {
                let Some(chat) = root.chats.get_mut(index) else {
                    return;
                };
                match result {
                    Ok(value) => chat.load(&value),
                    Err(error) => {
                        chat.loading = false;
                        chat.entries.push(Entry::Error(error));
                    }
                }
                root.refresh_repository(index, cx);
                cx.notify();
            },
        );
    }

    pub fn open_session(
        &mut self,
        window: &mut Window,
        session: SessionRow,
        cx: &mut Context<Self>,
    ) {
        // Picking a result is the end of a search: the field goes back to being
        // the brand row, the way the page closes its palette.
        if self.searching {
            self.close_search(cx);
        }
        if let Some(project) = self.projects.iter().find(|project| {
            self.sessions_of(&project.path)
                .iter()
                .any(|row| row.id == session.id)
        }) {
            self.collapsed_projects.remove(&project.path);
            if self
                .sessions_of(&project.path)
                .iter()
                .position(|row| row.id == session.id)
                .is_some_and(|position| position >= 5)
            {
                self.expanded_projects.insert(project.path.clone());
            }
        }
        if let Some(index) = self
            .chats
            .iter()
            .position(|chat| chat.session.as_deref() == Some(session.id.as_str()))
        {
            self.activate(window, index, cx);
            return;
        }
        let project = session.project.clone();
        self.push_chat(
            window,
            project,
            Some(session.id.clone()),
            session.title.clone(),
            cx,
        );
    }

    pub fn branch_active_chat(&mut self, cx: &mut Context<Self>) {
        let Some(chat) = self.chats.get(self.active) else {
            return;
        };
        let (Some(project), Some(session)) = (Some(chat.project.clone()), chat.session.clone())
        else {
            return;
        };
        self.assistant_actions = None;
        self.call(
            "fork_session",
            json!({ "project": project, "id": session, "name": "Side chat" }),
            cx,
            move |root, result, cx| match result {
                Ok(value) => {
                    let Some(id) = value["id"].as_str().map(str::to_string) else {
                        root.toast("The side chat did not return a session id.", true, cx);
                        return;
                    };
                    let title = value["name"]
                        .as_str()
                        .filter(|name| !name.trim().is_empty())
                        .unwrap_or("Side chat")
                        .to_string();
                    let project = value["cwd"]
                        .as_str()
                        .filter(|cwd| !cwd.is_empty())
                        .unwrap_or(&project)
                        .to_string();
                    root.push_chat_in_memory(project, Some(id), title, cx);
                    root.toast("Started a side chat.", false, cx);
                }
                Err(error) => root.toast(format!("Could not start a side chat: {error}"), true, cx),
            },
        );
    }

    pub fn activate(&mut self, window: &mut Window, index: usize, cx: &mut Context<Self>) {
        if index == self.active || index >= self.chats.len() {
            return;
        }
        self.history.push(self.active);
        self.forward.clear();
        self.active = index;
        self.changes = None;
        self.show_changes = false;
        self.stop_terminal(cx);
        self.prediction.clear();
        self.prediction_chat = None;
        self.prediction_generation = self.prediction_generation.wrapping_add(1);
        self.load_project_commands(cx);
        self.refresh_repository(index, cx);
        if let Some(editor) = self
            .chats
            .get(index)
            .and_then(|chat| self.editors.get(&chat.id))
        {
            window.focus(&editor.read(cx).focus_handle(cx), cx);
        }
        cx.notify();
    }

    pub fn close_chat(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.chats.len() {
            return;
        }
        if self.chats[index].running() {
            self.overlay = Some(Overlay::Confirm {
                title: "Close this chat?".to_string(),
                body: "A turn is running in it. Closing the tab leaves the turn running; reopen the chat to see it again.".to_string(),
                action: ConfirmAction::CloseChat(index),
            });
            cx.notify();
            return;
        }
        let chat = self.chats.remove(index);
        self.editors.remove(&chat.id);
        if self.active >= self.chats.len() {
            self.active = self.chats.len().saturating_sub(1);
        }
        cx.notify();
    }

    pub fn focus_composer(&self, window: &mut Window, cx: &mut App) {
        if let Some(chat) = self.chats.get(self.active) {
            if let Some(editor) = self.editors.get(&chat.id) {
                window.focus(&editor.read(cx).focus_handle(cx), cx);
            }
        }
    }

    fn text_input_focused(&self, window: &Window, cx: &App) -> bool {
        let fields = [
            &self.search,
            &self.command_query,
            &self.worktree_name,
            &self.worktree_base,
            &self.worktree_branch,
            &self.session_name,
            &self.project_name,
            &self.plugin_query,
            &self.marketplace_source,
            &self.provider_filter,
            &self.provider_key,
            &self.provider_model,
            &self.provider_url,
        ];
        fields
            .into_iter()
            .any(|editor| editor.read(cx).focus_handle(cx).is_focused(window))
            || self
                .editors
                .values()
                .any(|editor| editor.read(cx).focus_handle(cx).is_focused(window))
    }

    pub fn open_sidebar_session(
        &mut self,
        shortcut: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(session) = self
            .sidebar_sessions()
            .get(shortcut.saturating_sub(1))
            .cloned()
        else {
            return;
        };
        self.open_session(window, session, cx);
    }

    pub fn can_navigate_back(&self) -> bool {
        matches!(self.overlay, Some(Overlay::Settings))
            || self.rail != Rail::Chats
            || !self.history.is_empty()
    }

    pub fn can_navigate_forward(&self) -> bool {
        !matches!(self.overlay, Some(Overlay::Settings))
            && self.rail == Rail::Chats
            && !self.forward.is_empty()
    }

    pub fn navigate_back(&mut self, cx: &mut Context<Self>) {
        if matches!(self.overlay, Some(Overlay::Settings)) || self.rail != Rail::Chats {
            self.overlay = None;
            self.rail = Rail::Chats;
            cx.notify();
            return;
        }
        let Some(previous) = self.history.pop() else {
            return;
        };
        self.forward.push(self.active);
        self.active = previous.min(self.chats.len().saturating_sub(1));
        cx.notify();
    }

    pub fn navigate_forward(&mut self, cx: &mut Context<Self>) {
        if self.rail != Rail::Chats || matches!(self.overlay, Some(Overlay::Settings)) {
            return;
        }
        let Some(next) = self.forward.pop() else {
            return;
        };
        self.history.push(self.active);
        self.active = next.min(self.chats.len().saturating_sub(1));
        cx.notify();
    }

    pub fn refresh_live_changes(&mut self, cx: &mut Context<Self>) {
        if self.live_changes_busy {
            return;
        }
        let Some(project) = self.chats.get(self.active).map(|chat| chat.project.clone()) else {
            return;
        };
        if project.is_empty() {
            return;
        }
        self.live_changes_busy = true;
        let asked = project.clone();
        self.call(
            "repository_changes",
            json!({ "project": project }),
            cx,
            move |root, result, cx| {
                root.live_changes_busy = false;
                root.live_changes = result.ok().map(|value| {
                    (
                        asked,
                        value["files"].as_array().map(Vec::len).unwrap_or(0),
                        value["added"].as_u64().unwrap_or(0) as usize,
                        value["removed"].as_u64().unwrap_or(0) as usize,
                    )
                });
                cx.notify();
            },
        );
    }

    pub fn refresh_repository(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(project) = self.chats.get(index).map(|chat| chat.project.clone()) else {
            return;
        };
        let status_project = project.clone();
        self.call(
            "repository_status",
            json!({ "project": status_project }),
            cx,
            |root, result, _| {
                if let Ok(value) = result {
                    root.repository_status = value.as_array().cloned().unwrap_or_default();
                }
            },
        );
        self.call(
            "repository_changes",
            json!({ "project": project }),
            cx,
            move |root, result, cx| {
                if let Ok(value) = result {
                    if let Some(chat) = root.chats.get_mut(index) {
                        chat.changes = Some(value.clone());
                    }
                    if index == root.active {
                        let mut first = None;
                        if let Some(card) = EditedCard::from_value(&value) {
                            if let Some(review) = root.change_review.as_mut() {
                                if review.scope == ChangeScope::Repository {
                                    review.files = card.files;
                                    review.added = card.added;
                                    review.removed = card.removed;
                                    if review.open_files.is_empty() {
                                        first = review.files.first().map(|file| file.path.clone());
                                        if let Some(path) = first.as_ref() {
                                            review.open_files.insert(path.clone());
                                        }
                                    }
                                }
                            }
                        }
                        root.changes = Some(value);
                        if let Some(path) = first {
                            root.request_change_sides(path, cx);
                        }
                    }
                }
            },
        );
    }

    pub fn send(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let index = self.active;
        let Some(chat_id) = self.chats.get(index).map(|chat| chat.id) else {
            return;
        };
        let Some(editor) = self.editors.get(&chat_id).cloned() else {
            return;
        };
        let prompt = editor.read(cx).text().trim().to_string();
        let has_attachments = self
            .chats
            .get(index)
            .is_some_and(|chat| !chat.attachments.is_empty());
        if !composer_has_content(&prompt, has_attachments) {
            return;
        }
        self.prediction.clear();
        self.prediction_chat = None;
        self.prediction_generation = self.prediction_generation.wrapping_add(1);
        if !prompt.is_empty() && self.run_slash_command(&prompt, window, cx) {
            return;
        }
        self.model_menu_open = false;
        let Some(chat) = self.chats.get_mut(index) else {
            return;
        };
        if let Some(run) = chat.run {
            let follow_up = self.prefs["follow_up"].as_str() != Some("steer");
            let queue = chat.attachments.clone();
            chat.queued.push(QueuedMessage {
                text: prompt.clone(),
                attachments: queue.clone(),
                follow_up,
            });
            chat.attachments.clear();
            chat.scroll_to_end = true;
            let attachments: Vec<Value> = queue
                .iter()
                .map(
                    |attachment| json!({ "dataUrl": attachment.data_url, "name": attachment.name }),
                )
                .collect();
            let prompt_for_error = prompt.clone();
            self.call(
                "steer_run",
                json!({
                    "runId": run,
                    "message": prompt,
                    "followUp": follow_up,
                    "attachments": if attachments.is_empty() {
                        Value::Null
                    } else {
                        Value::Array(attachments)
                    },
                }),
                cx,
                move |root, result, cx| {
                    if let Err(error) = result {
                        if let Some(chat) = root.chats.get_mut(index) {
                            chat.queued
                                .retain(|message| message.text != prompt_for_error);
                        }
                        root.toast(error, true, cx);
                    }
                },
            );
            editor.update(cx, |editor, cx| editor.clear(cx));
            return;
        }
        if chat.awaiting_run {
            return;
        }
        let project = chat.project.clone();
        let session = chat.session.clone();
        let reasoning = chat.reasoning.clone();
        let agent = chat.agent.clone();
        let attachments: Vec<Value> = chat
            .attachments
            .iter()
            .map(|attachment| json!({ "dataUrl": attachment.data_url, "name": attachment.name }))
            .collect();
        let history_attachments = chat.attachments.clone();

        chat.entries.push(Entry::User {
            text: prompt.clone(),
            attachments: history_attachments,
        });
        chat.attachments.clear();
        chat.scroll_to_end = true;
        chat.stop_requested = false;
        chat.awaiting_run = true;
        chat.run_started_at = Some(Instant::now());

        let mut args = json!({
            "project": project,
            "prompt": prompt,
            "session": session,
            "attachments": if attachments.is_empty() { Value::Null } else { Value::Array(attachments) },
        });
        if let Some(reasoning) = reasoning {
            args["reasoning"] = json!(reasoning);
        }
        if let Some(agent) = agent {
            args["agent"] = json!(agent);
        }

        self.call("send_prompt", args, cx, move |root, result, cx| {
            match result {
                Ok(value) => {
                    let run = value.as_u64().unwrap_or_default();
                    if let Some(chat) = root.chats.get_mut(index) {
                        if chat.run.is_none() {
                            chat.run = Some(run);
                        }
                        chat.awaiting_run = false;
                    }
                }
                Err(error) => {
                    if let Some(chat) = root.chats.get_mut(index) {
                        chat.finish_run();
                        chat.awaiting_run = false;
                        chat.entries.push(Entry::Error(error));
                    }
                }
            }
            cx.notify();
        });
        self.start_run_clock_ticker(cx);

        editor.update(cx, |editor, cx| editor.clear(cx));
    }

    fn start_run_clock_ticker(&mut self, cx: &mut Context<Self>) {
        if self.run_clock_ticking {
            return;
        }
        self.run_clock_ticking = true;
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            let running = this
                .update(cx, |root, cx| {
                    let running = root.chats.iter().any(Chat::running);
                    if running {
                        cx.notify();
                    } else {
                        root.run_clock_ticking = false;
                    }
                    running
                })
                .unwrap_or(false);
            if !running {
                break;
            }
        })
        .detach();
    }

    pub fn run_slash_command(
        &mut self,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some((name, args)) = slash_command(text) else {
            return false;
        };
        let name = name.to_ascii_lowercase();
        if !performs_slash_command(&name, args) {
            return false;
        }
        match name.as_str() {
            "help" => self.open_overlay(Overlay::Help, cx),
            "mcp" => self.open_overlay(Overlay::Mcp, cx),
            "computer-use" => self.open_computer_use(cx),
            "model" => self.open_model_menu(cx),
            "agent" => self.open_overlay(Overlay::Agents, cx),
            "permissions" => self.open_overlay(Overlay::Approvals, cx),
            "connect" => self.open_connect(
                if args.is_empty() {
                    "a provider".to_string()
                } else {
                    args.to_string()
                },
                cx,
            ),
            "changes" => self.open_repository_changes(cx),
            "terminal" => self.open_terminal(window, cx),
            "pulls" => self.open_pulls(cx),
            "customize" => {
                self.rail = Rail::Customize;
                self.overlay = None;
                self.load_plugins(cx);
            }
            "trust" => {
                let Some(project) = self.current_project() else {
                    self.toast("Select a project first.", true, cx);
                    self.clear_active_editor(cx);
                    return true;
                };
                self.open_overlay(
                    Overlay::Trust {
                        project,
                        onboarding: false,
                    },
                    cx,
                );
            }
            "reasoning" if args.is_empty() => {
                self.open_model_menu(cx);
            }
            "reasoning"
                if self.info["reasoningLevels"]
                    .as_array()
                    .is_some_and(|levels| {
                        levels
                            .iter()
                            .filter_map(Value::as_str)
                            .any(|level| level.eq_ignore_ascii_case(args))
                    }) =>
            {
                self.set_reasoning(args.to_ascii_lowercase(), cx);
            }
            "reasoning" => {
                let levels = self.info["reasoningLevels"]
                    .as_array()
                    .map(|levels| {
                        levels
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .unwrap_or_default();
                self.toast(format!("Reasoning must be one of {levels}."), true, cx);
            }
            _ => return false,
        }
        self.clear_active_editor(cx);
        true
    }

    fn clear_active_editor(&mut self, cx: &mut Context<Self>) {
        let Some(editor) = self
            .chats
            .get(self.active)
            .and_then(|chat| self.editors.get(&chat.id))
            .cloned()
        else {
            return;
        };
        editor.update(cx, |editor, cx| editor.clear(cx));
        cx.notify();
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        let index = self.active;
        let Some(run) = self.chats.get(index).and_then(|chat| chat.run) else {
            return;
        };
        if let Some(chat) = self.chats.get_mut(index) {
            chat.stop_requested = true;
        }
        self.call(
            "cancel_run",
            json!({ "runId": run }),
            cx,
            move |root, _, cx| {
                if let Some(chat) = root.chats.get_mut(index) {
                    chat.run = None;
                    chat.queued.clear();
                }
                cx.notify();
            },
        );
    }

    pub fn set_model(&mut self, provider: String, model: String, cx: &mut Context<Self>) {
        self.model_menu_open = false;
        self.model_menu_show_models = false;
        self.call(
            "set_model",
            json!({ "provider": provider, "model": model }),
            cx,
            |root, result, cx| match result {
                Ok(_) => {
                    root.model_menu_open = false;
                    root.model_menu_show_models = false;
                    root.load_project_info(cx);
                    root.toast("Model updated", false, cx);
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn set_reasoning(&mut self, level: String, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.get_mut(self.active) {
            chat.reasoning = Some(level.clone());
        }
        self.overlay = None;
        self.call(
            "set_reasoning",
            json!({ "reasoning": level }),
            cx,
            |root, result, cx| match result {
                Ok(_) => root.load_project_info(cx),
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn set_agent(&mut self, agent: Option<String>, cx: &mut Context<Self>) {
        if let Some(chat) = self.chats.get_mut(self.active) {
            chat.agent = agent;
        }
        self.overlay = None;
        cx.notify();
    }

    pub fn resolve_approval(&mut self, id: u64, decision: &str, cx: &mut Context<Self>) {
        let decision = decision.to_string();
        let index = self.active;
        if let Some(chat) = self.chats.get_mut(index) {
            for entry in chat.entries.iter_mut() {
                if let Entry::Approval(card) = entry {
                    if card.id == id {
                        card.decision = Some(decision.clone());
                    }
                }
            }
        }
        self.overlay = None;
        self.call(
            "resolve_approval",
            json!({ "id": id, "decision": decision }),
            cx,
            |root, result, cx| {
                if let Err(error) = result {
                    root.toast(error, true, cx);
                }
            },
        );
    }

    pub fn resolve_question(&mut self, id: u64, answers: Value, cx: &mut Context<Self>) {
        let index = self.active;
        if let Some(chat) = self.chats.get_mut(index) {
            for entry in chat.entries.iter_mut() {
                if let Entry::Question(card) = entry {
                    if card.id == id {
                        card.answered = true;
                        card.answers = answers.clone();
                    }
                }
            }
        }
        self.overlay = None;
        self.call(
            "resolve_question",
            json!({ "id": id, "answers": answers }),
            cx,
            |root, result, cx| {
                if let Err(error) = result {
                    root.toast(error, true, cx);
                }
            },
        );
    }

    pub fn add_project(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from("Add a source folder")),
        });
        cx.spawn(async move |this, cx| {
            let result = paths.await.unwrap_or_else(|_| Ok(None));
            let _ = this.update(cx, |root, cx| match result {
                Ok(Some(paths)) => {
                    if let Some(path) = paths.into_iter().next() {
                        let path = path.to_string_lossy().to_string();
                        if !root.new_project_folders.contains(&path) {
                            root.new_project_folders.push(path.clone());
                        }
                        if root.project_name.read(cx).text().trim().is_empty() {
                            let fallback = std::path::Path::new(&path)
                                .file_name()
                                .map(|name| name.to_string_lossy().to_string())
                                .unwrap_or_default();
                            root.project_name
                                .update(cx, |editor, cx| editor.set_text(fallback, cx));
                        }
                        cx.notify();
                    }
                }
                Ok(None) => {}
                Err(error) => root.toast(
                    format!("Could not open the folder picker: {error}"),
                    true,
                    cx,
                ),
            });
        })
        .detach();
    }

    pub fn create_project(&mut self, cx: &mut Context<Self>) {
        let name = self.project_name.read(cx).text().trim().to_string();
        if self.new_project_folders.is_empty() {
            self.toast("Add at least one source folder", true, cx);
            return;
        }
        if name.is_empty() {
            self.toast("Project name cannot be empty", true, cx);
            return;
        }
        let folders = self.new_project_folders.clone();
        self.call(
            "create_project",
            json!({ "name": name, "folders": folders }),
            cx,
            move |root, result, cx| match result {
                Ok(value) => {
                    root.projects = crate::model::projects(&value["projects"]);
                    let first_folder = root
                        .new_project_folders
                        .first()
                        .cloned()
                        .unwrap_or_default();
                    let project = added_project_path(&value, &root.projects, first_folder);
                    root.new_project_folders.clear();
                    root.project_name.update(cx, |editor, cx| editor.clear(cx));
                    root.overlay = None;
                    root.refresh_sessions(cx);
                    root.push_chat_in_memory(project.clone(), None, "New chat".to_string(), cx);
                    root.prompt_for_added_project(project, cx);
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn open_project_actions(
        &mut self,
        window: &mut Window,
        id: String,
        path: String,
        name: String,
        pinned: bool,
        cx: &mut Context<Self>,
    ) {
        self.inline_project_actions = None;
        self.project_name
            .update(cx, |editor, cx| editor.set_text(name.clone(), cx));
        self.overlay = Some(Overlay::ProjectActions {
            id,
            path,
            name,
            pinned,
        });
        let focus = self.project_name.read(cx).focus_handle(cx);
        window.focus(&focus, cx);
        cx.notify();
    }

    pub fn save_project_name(&mut self, id: String, cx: &mut Context<Self>) {
        let name = self.project_name.read(cx).text().trim().to_string();
        if name.is_empty() {
            self.toast("Project name cannot be empty", true, cx);
            return;
        }
        self.call(
            "update_project",
            json!({ "id": id, "name": name }),
            cx,
            |root, result, cx| match result {
                Ok(_) => {
                    root.overlay = None;
                    root.refresh_projects(cx);
                }
                Err(error) => root.toast(format!("Could not rename project: {error}"), true, cx),
            },
        );
    }

    pub fn remove_project(&mut self, id: String, cx: &mut Context<Self>) {
        self.overlay = None;
        self.inline_project_actions = None;
        self.call(
            "remove_project",
            json!({ "id": id }),
            cx,
            |root, result, cx| match result {
                Ok(_) => root.refresh_projects(cx),
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn set_session_pinned(&mut self, id: String, pinned: bool, cx: &mut Context<Self>) {
        self.overlay = None;
        self.call(
            "set_session_pinned",
            json!({ "id": id, "pinned": pinned }),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    root.sessions = value
                        .as_array()
                        .map(|sessions| {
                            sessions.iter().filter_map(SessionRow::from_value).collect()
                        })
                        .unwrap_or_default();
                    cx.notify();
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn archive_session(&mut self, id: String, cx: &mut Context<Self>) {
        self.overlay = None;
        self.call(
            "archive_session",
            json!({ "id": id }),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    root.sessions = value
                        .as_array()
                        .map(|sessions| {
                            sessions.iter().filter_map(SessionRow::from_value).collect()
                        })
                        .unwrap_or_default();
                    cx.notify();
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn rename_session(&mut self, project: String, id: String, cx: &mut Context<Self>) {
        let name = self.session_name.read(cx).text().trim().to_string();
        if name.is_empty() {
            return;
        }
        let updated_name = name.clone();
        self.call(
            "rename_session",
            json!({ "project": project, "id": id, "name": name }),
            cx,
            move |root, result, cx| match result {
                Ok(_) => {
                    root.overlay = None;
                    for session in &mut root.sessions {
                        if session.id == id {
                            session.title = updated_name.clone();
                        }
                    }
                    root.refresh_sessions(cx);
                    cx.notify();
                }
                Err(error) => root.toast(format!("Could not rename chat: {error}"), true, cx),
            },
        );
    }

    pub fn set_project_pinned(&mut self, id: String, pinned: bool, cx: &mut Context<Self>) {
        self.overlay = None;
        self.inline_project_actions = None;
        self.call(
            "set_project_pinned",
            json!({ "id": id, "pinned": pinned }),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    root.projects = crate::model::projects(&value);
                    cx.notify();
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn reorder_project(&mut self, dragged: String, target: String, cx: &mut Context<Self>) {
        if dragged == target {
            return;
        }
        let mut ids = self
            .projects
            .iter()
            .map(|project| project.id.clone())
            .collect::<Vec<_>>();
        let Some(position) = ids.iter().position(|id| id == &dragged) else {
            return;
        };
        ids.remove(position);
        let Some(target_position) = ids.iter().position(|id| id == &target) else {
            return;
        };
        ids.insert(target_position + 1, dragged);
        self.call(
            "reorder_projects",
            json!({ "ids": ids }),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    root.projects = crate::model::projects(&value);
                    cx.notify();
                }
                Err(error) => root.toast(format!("Could not reorder projects: {error}"), true, cx),
            },
        );
    }

    pub fn arm_project_actions_dismissal(&mut self, cx: &mut Context<Self>) {
        self.project_actions_dismiss_armed = false;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(40))
                .await;
            let _ = this.update(cx, |root, _| root.project_actions_dismiss_armed = true);
        })
        .detach();
    }

    pub fn arm_composer_tools_dismissal(&mut self, cx: &mut Context<Self>) {
        self.composer_tools_dismiss_armed = false;
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(40))
                .await;
            let _ = this.update(cx, |root, _| root.composer_tools_dismiss_armed = true);
        })
        .detach();
    }

    pub fn open_repository_changes(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.chats.get(self.active).map(|chat| chat.project.clone()) else {
            return;
        };
        self.stop_terminal(cx);
        self.rail = Rail::Chats;
        let card = self
            .changes
            .as_ref()
            .and_then(EditedCard::from_value)
            .unwrap_or_else(|| EditedCard {
                project: project.clone(),
                baseline: None,
                after: None,
                files: Vec::new(),
                added: 0,
                removed: 0,
                latest: false,
                undone: false,
            });
        self.open_change_review(project, ChangeScope::Repository, card, None, cx);
        self.panel_tab = PanelTab::Changes;
        self.refresh_repository(self.active, cx);
    }

    pub fn toggle_repository_changes(&mut self, cx: &mut Context<Self>) {
        if self.show_changes && self.panel_tab == PanelTab::Changes {
            self.close_workspace_panel(cx);
        } else {
            self.open_repository_changes(cx);
        }
    }

    pub fn open_turn_changes(
        &mut self,
        card: &EditedCard,
        path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let project = if card.project.is_empty() {
            self.chats
                .get(self.active)
                .map(|chat| chat.project.clone())
                .unwrap_or_default()
        } else {
            card.project.clone()
        };
        self.open_change_review(project, ChangeScope::Turn, card.clone(), path, cx);
        self.panel_tab = PanelTab::Changes;
    }

    fn open_change_review(
        &mut self,
        project: String,
        scope: ChangeScope,
        card: EditedCard,
        path: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let mut review = ChangeReview::new(project, scope, &card);
        if let Some(path) = path {
            review.open_files.clear();
            review.open_files.insert(path);
        }
        let first = review.open_files.iter().next().cloned();
        self.change_review = Some(review);
        self.show_changes = true;
        if let Some(path) = first {
            self.request_change_sides(path, cx);
        }
        cx.notify();
    }

    pub fn set_change_scope(&mut self, scope: ChangeScope, cx: &mut Context<Self>) {
        if self
            .change_review
            .as_ref()
            .is_none_or(|review| review.scope == scope)
        {
            return;
        }
        match scope {
            ChangeScope::Turn => {
                let Some(review) = self.change_review.as_mut() else {
                    return;
                };
                review.files = review.turn_files.clone();
                review.sides.clear();
                review.errors.clear();
                review.loading.clear();
                review.scope = ChangeScope::Turn;
            }
            ChangeScope::Repository => {
                let Some(card) = self.changes.as_ref().and_then(EditedCard::from_value) else {
                    return;
                };
                let Some(review) = self.change_review.as_mut() else {
                    return;
                };
                review.files = card.files.clone();
                review.added = card.added;
                review.removed = card.removed;
                review.sides.clear();
                review.errors.clear();
                review.loading.clear();
                review.scope = ChangeScope::Repository;
            }
        }
        if !self.change_review.as_ref().is_some_and(|review| {
            review
                .files
                .iter()
                .any(|file| review.open_files.contains(&file.path))
        }) {
            if let Some(review) = self.change_review.as_mut() {
                review.open_files.clear();
                if let Some(first) = review.files.first() {
                    review.open_files.insert(first.path.clone());
                }
            }
        }
        if let Some(path) = self
            .change_review
            .as_ref()
            .and_then(|review| review.open_files.iter().next().cloned())
        {
            self.request_change_sides(path, cx);
        }
        cx.notify();
    }

    pub fn toggle_change_file(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(review) = self.change_review.as_mut() else {
            return;
        };
        if !review.open_files.remove(&path) {
            review.open_files.insert(path.clone());
            self.request_change_sides(path, cx);
        }
        cx.notify();
    }

    pub fn toggle_change_layout(&mut self, cx: &mut Context<Self>) {
        if let Some(review) = self.change_review.as_mut() {
            review.unified = !review.unified;
            cx.notify();
        }
    }

    pub fn expand_change_context(&mut self, path: String, start: usize, cx: &mut Context<Self>) {
        if let Some(review) = self.change_review.as_mut() {
            review.expanded.insert(format!("{path}:{start}"));
            cx.notify();
        }
    }

    pub fn refresh_change_review(&mut self, cx: &mut Context<Self>) {
        if let Some(review) = self.change_review.as_mut() {
            review.sides.clear();
            review.errors.clear();
            review.loading.clear();
            if review.scope == ChangeScope::Repository {
                if let Some(card) = self.changes.as_ref().and_then(EditedCard::from_value) {
                    review.files = card.files;
                    review.added = card.added;
                    review.removed = card.removed;
                }
            }
        }
        let paths = self
            .change_review
            .as_ref()
            .map(|review| review.open_files.iter().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        if let Some(review) = self.change_review.as_ref() {
            if review.scope == ChangeScope::Repository {
                self.refresh_repository(self.active, cx);
            }
        }
        for path in paths {
            self.request_change_sides(path, cx);
        }
        cx.notify();
    }

    fn request_change_sides(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(review) = self.change_review.as_mut() else {
            return;
        };
        if review.sides.contains_key(&path) || review.loading.contains(&path) {
            return;
        }
        review.loading.insert(path.clone());
        let project = review.project.clone();
        let baseline = (review.scope == ChangeScope::Turn)
            .then(|| review.turn_baseline.clone())
            .flatten();
        let (command, args) = if let Some(baseline) = baseline {
            (
                "change_sides",
                json!({ "project": project, "baseline": baseline, "path": path }),
            )
        } else {
            (
                "repository_sides",
                json!({ "project": project, "path": path }),
            )
        };
        self.call(command, args, cx, move |root, result, _| {
            let Some(review) = root.change_review.as_mut() else {
                return;
            };
            review.loading.remove(&path);
            match result {
                Ok(value) => {
                    review.sides.insert(path.clone(), value);
                    review.errors.remove(&path);
                }
                Err(error) => {
                    review.errors.insert(path.clone(), error);
                }
            }
        });
    }

    pub fn undo_turn(&mut self, cx: &mut Context<Self>) {
        let index = self.active;
        let Some(chat) = self.chats.get(index) else {
            return;
        };
        let (project, baseline, after) = (
            chat.project.clone(),
            chat.baseline.clone(),
            chat.after.clone(),
        );
        let Some(baseline) = baseline else {
            return;
        };
        self.call(
            "undo_turn",
            json!({ "project": project, "baseline": baseline, "after": after }),
            cx,
            move |root, result, cx| match result {
                Ok(_) => {
                    if let Some(chat) = root.chats.get_mut(index) {
                        chat.mark_undone();
                    }
                    root.refresh_repository(index, cx);
                    root.toast("The turn's changes were undone", false, cx);
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn clear_approvals(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.current_project() else {
            return;
        };
        self.call(
            "clear_approvals",
            json!({ "project": project }),
            cx,
            |root, result, cx| match result {
                Ok(_) => {
                    root.approvals.clear();
                    root.toast("Allowed tools cleared for this project", false, cx);
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    /// Picks files from disk and holds them as the turn's attachments, the way
    /// Electron's `pick_files` does — the same data URL the core reads.
    pub fn attach_files(&mut self, cx: &mut Context<Self>) {
        let index = self.active;
        let paths = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some(SharedString::from("Attach files")),
        });
        cx.spawn(async move |this, cx| {
            let chosen = paths.await.unwrap_or_else(|_| Ok(None));
            let _ = this.update(cx, |root, cx| {
                let Ok(Some(paths)) = chosen else {
                    return;
                };
                let mut refused = Vec::new();
                for path in paths {
                    match attachment_from_path(&path) {
                        Ok(Some(attachment)) => {
                            if let Some(chat) = root.chats.get_mut(index) {
                                if let Err(error) = add_attachment(chat, attachment) {
                                    if refused.is_empty() {
                                        refused.push(error);
                                    }
                                }
                            }
                        }
                        Ok(None) => {}
                        Err(error) => refused.push(error),
                    }
                }
                if let Some(error) = refused.first() {
                    root.toast(error.clone(), true, cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub fn set_trust(&mut self, project: String, trusted: bool, cx: &mut Context<Self>) {
        self.call(
            "set_project_trust",
            json!({ "project": project, "trusted": trusted }),
            cx,
            |root, result, cx| match result {
                Ok(info) => {
                    root.info = info;
                    root.overlay = None;
                    root.toast("Project access updated", false, cx);
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    fn prompt_for_added_project(&mut self, project: String, cx: &mut Context<Self>) {
        let expected = project.clone();
        self.call(
            "project_info",
            json!({ "project": project }),
            cx,
            move |root, result, cx| match result {
                Ok(value) => {
                    if root.current_project().as_deref() != Some(expected.as_str()) {
                        return;
                    }
                    root.info = value;
                    root.model_menu_open = false;
                    root.overlay = Some(Overlay::Trust {
                        project: expected.clone(),
                        onboarding: true,
                    });
                    cx.notify();
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    // ---------- packets ----------

    fn event(&mut self, name: &str, payload: Value, cx: &mut Context<Self>) {
        match name {
            "agent-event" => {
                let run = payload["runId"].as_u64();
                let kind = payload["type"].as_str().unwrap_or_default().to_string();
                if let Some(index) = self.claim_run(run) {
                    self.chats[index].apply(&payload);
                    if kind == "tool_execution_end" && index == self.active {
                        self.refresh_live_changes(cx);
                    }
                }
                if kind == "session_title" {
                    if let Some(index) = self.chats.iter().position(|chat| chat.run == run) {
                        if let Some(title) = payload["title"].as_str() {
                            if !title.trim().is_empty() {
                                self.chats[index].title = title.to_string();
                                self.chats[index].provisional_title = false;
                            }
                        }
                    }
                    self.refresh_sessions(cx);
                }
            }
            "agent-start" => {
                let run = payload["runId"].as_u64().unwrap_or_default();
                let session = payload["sessionId"].as_str().map(str::to_string);
                let title = payload["title"].as_str().map(str::to_string);
                if let Some(index) = self.claim_run(Some(run)) {
                    let chat = &mut self.chats[index];
                    let was_new = chat.session.is_none();
                    chat.session = session;
                    if let Some(title) = agent_start_title(&chat.title, title.as_deref(), was_new) {
                        chat.title = title;
                    }
                }
                self.refresh_sessions(cx);
            }
            "agent-end" => {
                let run = payload["runId"].as_u64();
                let ended = self.claim_run(run);
                let ended_active = ended == Some(self.active);
                if let Some(index) = ended {
                    let chat = &mut self.chats[index];
                    chat.run = None;
                    chat.awaiting_run = false;
                    chat.finish_run();
                    chat.changes = payload.get("changes").cloned();
                    chat.baseline = payload["baseline"].as_str().map(str::to_string);
                    chat.after = payload["after"].as_str().map(str::to_string);
                    for queued in std::mem::take(&mut chat.queued) {
                        chat.entries.push(Entry::User {
                            text: queued.text,
                            attachments: queued.attachments,
                        });
                    }
                    chat.record_turn(&payload);
                    for entry in chat.entries.iter_mut() {
                        if let Entry::Approval(card) = entry {
                            if card.decision.is_none() {
                                card.decision = Some("expired".to_string());
                            }
                        }
                    }
                }
                self.refresh_sessions(cx);
                let index = self.active;
                // A run may have switched branch, so the composer's own row is
                // read again along with the changes the turn left behind.
                self.load_git(cx);
                self.refresh_repository(index, cx);
                self.refresh_live_changes(cx);
                if ended_active {
                    self.maybe_predict(cx);
                }
            }
            "approval-request" => {
                let id = payload["id"].as_u64().unwrap_or_default();
                let tool = payload["toolName"].as_str().unwrap_or_default().to_string();
                let detail = payload["detail"].as_str().unwrap_or_default().to_string();
                if let Some(index) = self.running_chat() {
                    if let Some(chat) = self.chats.get_mut(index) {
                        chat.entries
                            .push(Entry::Approval(crate::model::ApprovalCard {
                                id,
                                tool: tool.clone(),
                                detail: detail.clone(),
                                decision: None,
                            }));
                    }
                    self.overlay = Some(Overlay::Approval { chat: index, id });
                }
            }
            "question-request" => {
                let id = payload["id"].as_u64().unwrap_or_default();
                if let Some(index) = self.running_chat() {
                    if let Some(chat) = self.chats.get_mut(index) {
                        chat.entries
                            .push(Entry::Question(crate::model::QuestionCard {
                                id,
                                questions: payload["questions"].clone(),
                                answered: false,
                                answers: Value::Null,
                            }));
                    }
                    self.overlay = Some(Overlay::Question { chat: index, id });
                }
            }
            "question-closed" => {
                let id = payload["id"].as_u64().unwrap_or_default();
                for chat in self.chats.iter_mut() {
                    for entry in chat.entries.iter_mut() {
                        if let Entry::Question(card) = entry {
                            if card.id == id && !card.answered {
                                card.answered = true;
                            }
                        }
                    }
                }
                if let Some(Overlay::Question { id: open, .. }) = self.overlay {
                    if open == id {
                        self.overlay = None;
                    }
                }
            }
            "model-catalog" => {
                if self.model_menu_open {
                    self.load_models(cx);
                }
            }
            "terminal-output" => {
                let id = payload["terminal"].as_u64().unwrap_or_default();
                let data = payload["data"].as_str().unwrap_or_default();
                self.terminal.feed(id, data.as_bytes());
            }
            "terminal-exit" => {
                let id = payload["terminal"].as_u64().unwrap_or_default();
                self.terminal.exit(id);
            }
            "update-available" => {
                self.update_flag = true;
                self.update_check = None;
                self.update_error = None;
                self.update_result = None;
                self.update_failure = None;
                self.updating_update = true;
                self.update_progress = Some(json!({ "stage": "checking" }));
                self.overlay = Some(Overlay::Updates);
            }
            "update-progress" => {
                if self.update_token.is_none() {
                    self.update_token = payload["token"].as_str().map(str::to_string);
                }
                if payload["token"].as_str().is_none()
                    || payload["token"].as_str() == self.update_token.as_deref()
                {
                    self.updating_update = true;
                    self.update_result = None;
                    self.update_failure = None;
                    self.update_restart_failed = false;
                    self.update_progress = Some(payload);
                }
            }
            "update-ready" => {
                self.updating_update = false;
                self.update_progress = None;
                self.update_result = Some(payload);
                self.update_token = None;
                self.overlay = Some(Overlay::Updates);
            }
            "update-failed" => {
                self.updating_update = false;
                self.update_progress = None;
                if payload["cancelled"].as_bool().unwrap_or(false) {
                    self.update_token = None;
                    if matches!(self.overlay, Some(Overlay::Updates)) {
                        self.overlay = None;
                    }
                } else {
                    self.update_token = None;
                    self.update_restart_failed = false;
                    let message = payload["message"]
                        .as_str()
                        .unwrap_or("The update did not finish.")
                        .to_string();
                    if matches!(self.overlay, Some(Overlay::Updates)) {
                        self.update_failure = Some(message);
                    } else {
                        self.toast(format!("Could not install the update: {message}"), true, cx);
                    }
                }
            }
            "restart" => {
                self.relaunch(cx);
            }
            _ => {}
        }
        cx.notify();
    }

    fn relaunch(&mut self, cx: &mut Context<Self>) {
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).spawn();
        }
        cx.quit();
    }

    /// The chat a run belongs to: the one already carrying its id, else the one
    /// that asked for a turn and is waiting for its id to come back.
    pub fn claim_run(&mut self, run: Option<u64>) -> Option<usize> {
        let Some(run) = run else {
            return Some(self.active);
        };
        if let Some(index) = self.chats.iter().position(|chat| chat.run == Some(run)) {
            return Some(index);
        }
        let index = self.chats.iter().position(|chat| chat.awaiting_run)?;
        let chat = &mut self.chats[index];
        chat.run = Some(run);
        chat.awaiting_run = false;
        Some(index)
    }

    fn running_chat(&self) -> Option<usize> {
        if self
            .chats
            .get(self.active)
            .map(Chat::running)
            .unwrap_or(false)
        {
            return Some(self.active);
        }
        self.chats.iter().position(Chat::running)
    }

    // ---------- helpers for the views ----------

    pub fn unix_now() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|elapsed| elapsed.as_secs())
            .unwrap_or(0)
    }

    pub fn project_name(&self, path: &str) -> String {
        self.projects
            .iter()
            .find(|project| project.path == path)
            .map(|project| {
                if project.name.is_empty() {
                    project.display_path.clone()
                } else {
                    project.name.clone()
                }
            })
            .unwrap_or_else(|| {
                std::path::Path::new(path)
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string())
            })
    }

    pub fn sessions_of<'a>(&'a self, project: &'a str) -> Vec<&'a SessionRow> {
        let mut sessions: Vec<&SessionRow> = self
            .sessions
            .iter()
            .filter(|session| {
                session.project == project
                    || Worktrees::open()
                        .project_of(Path::new(&session.project))
                        .is_some_and(|owner| owner == Path::new(project))
            })
            .collect();
        sessions.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then(right.modified_at.cmp(&left.modified_at))
        });
        sessions
    }

    pub fn sidebar_sessions(&self) -> Vec<SessionRow> {
        let mut ordered = Vec::new();
        let mut seen = HashSet::new();
        for project in &self.projects {
            for session in self.sessions_of(&project.path) {
                if seen.insert(session.id.clone()) {
                    ordered.push(session.clone());
                    if ordered.len() == 9 {
                        return ordered;
                    }
                }
            }
        }
        ordered
    }

    pub fn open_overlay(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        self.model_menu_open = false;
        self.composer_tools_open = false;
        match &overlay {
            Overlay::Help => self.load_commands(cx),
            Overlay::CommandMenu => {
                self.command_query.update(cx, |editor, cx| editor.clear(cx));
            }
            Overlay::Mcp => self.load_sources(cx),
            Overlay::Agents => self.load_project_info(cx),
            Overlay::Settings => self.load_project_commands(cx),
            Overlay::NewProject => {
                self.new_project_folders.clear();
                self.project_name.update(cx, |editor, cx| editor.clear(cx));
            }
            Overlay::Trust {
                project,
                onboarding,
            } => {
                let project = project.clone();
                let onboarding = *onboarding;
                self.call(
                    "project_info",
                    json!({ "project": project }),
                    cx,
                    move |root, result, _| {
                        if let Ok(value) = result {
                            root.info = value;
                            if onboarding {
                                root.overlay = Some(Overlay::Trust {
                                    project,
                                    onboarding: true,
                                });
                            }
                        }
                    },
                );
            }
            _ => {}
        }
        self.overlay = Some(overlay);
        cx.notify();
    }

    pub fn selected_project(&self) -> Option<&ProjectRow> {
        let project = self.current_project_owner()?;
        self.projects.iter().find(|row| row.path == project)
    }
}

fn added_project_path(value: &Value, projects: &[ProjectRow], fallback: String) -> String {
    value["added"]
        .as_str()
        .and_then(|id| projects.iter().find(|project| project.id == id))
        .map(|project| project.path.clone())
        .unwrap_or(fallback)
}

fn project_trust_awaiting(value: &Value) -> bool {
    value["trust"]["awaiting"].as_bool().unwrap_or(false)
}

/// The attachment a path makes: the core's own limit and MIME rules, in the
/// data URL shape `send_prompt` takes. `Ok(None)` for a path the core would not
/// accept as an attachment.
fn attachment_from_path(path: &std::path::Path) -> Result<Option<Attachment>, String> {
    if !oxide_core::media::is_attachment_path(path) {
        return Ok(None);
    }
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if bytes.len() > oxide_core::media::MAX_ATTACHMENT_BYTES {
        return Err(format!(
            "{} is larger than {} MB",
            path.display(),
            oxide_core::media::MAX_ATTACHMENT_BYTES / (1024 * 1024)
        ));
    }
    let mime = if let Some(mime) = oxide_core::media::image_mime_of(path) {
        mime.to_string()
    } else if oxide_core::media::is_pdf_path(path) {
        "application/pdf".to_string()
    } else {
        "text/plain".to_string()
    };
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.display().to_string());
    Ok(Some(Attachment::new(
        name,
        format!(
            "data:{mime};base64,{}",
            oxide_core::media::base64_encode(&bytes)
        ),
    )))
}

fn add_attachment(chat: &mut Chat, attachment: Attachment) -> Result<(), String> {
    if chat
        .attachments
        .iter()
        .any(|current| current.data_url == attachment.data_url)
    {
        return Ok(());
    }
    if chat.attachments.len() >= MAX_ATTACHMENTS_PER_MESSAGE {
        return Err(format!(
            "At most {MAX_ATTACHMENTS_PER_MESSAGE} attachments per message"
        ));
    }
    chat.attachments.push(attachment);
    Ok(())
}

fn composer_has_content(prompt: &str, has_attachments: bool) -> bool {
    !prompt.trim().is_empty() || has_attachments
}

pub(crate) fn details_visible(show_details: bool, has_chat: bool) -> bool {
    show_details && has_chat
}

fn toggled_details(show_details: bool, has_chat: bool) -> bool {
    if !has_chat {
        return show_details;
    }
    !show_details
}

fn agent_start_title(current: &str, incoming: Option<&str>, was_new: bool) -> Option<String> {
    let incoming = incoming.filter(|title| !title.trim().is_empty())?;
    (was_new || current.trim().is_empty() || current == "New chat").then(|| incoming.to_string())
}

pub(crate) fn change_hunk(lines: &[Value], first: usize, last: usize) -> Value {
    let start = first.saturating_sub(3);
    let end = (last + 4).min(lines.len());
    let old_start = lines[start..end]
        .iter()
        .find_map(|line| line["old"].as_u64())
        .or_else(|| {
            lines[..start]
                .iter()
                .rev()
                .find_map(|line| line["old"].as_u64().map(|number| number + 1))
        })
        .unwrap_or(1);
    let new_start = lines[start..end]
        .iter()
        .find_map(|line| line["new"].as_u64())
        .or_else(|| {
            lines[..start]
                .iter()
                .rev()
                .find_map(|line| line["new"].as_u64().map(|number| number + 1))
        })
        .unwrap_or(1);
    json!({
        "old_start": old_start,
        "new_start": new_start,
        "lines": lines[start..end]
            .iter()
            .map(|line| json!({
                "kind": line["kind"].as_str().unwrap_or("context"),
                "text": line["text"].as_str().unwrap_or_default(),
            }))
            .collect::<Vec<_>>(),
    })
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A window with no thread in it opens on the invite and the composer under
        // it, the way the page does, rather than on a page with nothing to type
        // into: the first project's draft is made in memory here and becomes a
        // session only when something is sent.
        if self.chats.is_empty() {
            if let Some(project) = self.current_project() {
                self.push_chat(window, project, None, "New chat".to_string(), cx);
            }
        }
        if let Some(id) = self.pending_focus.take() {
            if let Some(editor) = self.editors.get(&id) {
                window.focus(&editor.read(cx).focus_handle(cx), cx);
            }
        }

        let theme = self.theme.clone();
        let scale = self.prefs["font_scale"]
            .as_f64()
            .unwrap_or(1.0)
            .clamp(0.7, 1.6) as f32;

        let mut shell = div()
            .id("oxide-app")
            .role(gpui::Role::Application)
            .aria_label("Oxide")
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background())
            .text_color(theme.text())
            .text_size(px(13.5 * scale))
            .font_family(".SystemUIFont")
            .cursor(
                if self.resizing.is_some() || self.changes_resizing.is_some() {
                    gpui::CursorStyle::ResizeLeftRight
                } else {
                    gpui::CursorStyle::Arrow
                },
            )
            .key_context("Root")
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "tab"
                    && !this.text_input_focused(window, cx)
                    && !this.terminal_focus.is_focused(window)
                {
                    if event.keystroke.modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                }
            }))
            .on_mouse_move(cx.listener(|this, event, window, cx| {
                if this.resizing.is_some() {
                    this.handle_sidebar_drag(event, window, cx);
                } else if this.changes_resizing.is_some() {
                    this.handle_changes_drag(event, window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.effort_drag_level = None;
                    this.end_sidebar_drag(cx);
                    this.end_changes_drag(cx);
                }),
            )
            .on_action(cx.listener(|this, _: &NewChat, window, cx| this.new_chat(window, cx)))
            .on_action(cx.listener(|this, _: &CloseChat, _window, cx| {
                let index = this.active;
                this.close_chat(index, cx)
            }))
            .on_action(cx.listener(|this, _: &ToggleSidebar, _window, cx| {
                this.sidebar_visible = !this.sidebar_visible;
                let visible = this.sidebar_visible;
                this.save_prefs(json!({ "sidebar_visible": visible }), cx);
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &ToggleDetails, _window, cx| this.toggle_details(cx)))
            .on_action(
                cx.listener(|this, _: &ToggleActivities, _window, cx| this.toggle_activities(cx)),
            )
            .on_action(cx.listener(|this, _: &OpenSettings, _window, cx| {
                this.open_overlay(Overlay::Settings, cx)
            }))
            .on_action(cx.listener(|this, _: &AboutApp, _window, cx| {
                this.overlay = Some(Overlay::About);
                cx.notify();
            }))
            .on_action(
                cx.listener(|this, _: &CheckForUpdates, _window, cx| this.check_for_updates(cx)),
            )
            .on_action(cx.listener(|_, _: &HideApp, _window, cx| cx.hide()))
            .on_action(cx.listener(|this, _: &IncreaseTextSize, _window, cx| {
                this.adjust_text_scale(0.1, cx)
            }))
            .on_action(cx.listener(|this, _: &DecreaseTextSize, _window, cx| {
                this.adjust_text_scale(-0.1, cx)
            }))
            .on_action(
                cx.listener(|this, _: &ResetTextSize, _window, cx| this.set_text_scale(1.0, cx)),
            )
            .on_action(
                cx.listener(|_, _: &ToggleFullscreen, window, _cx| window.toggle_fullscreen()),
            )
            .on_action(cx.listener(|_, _: &MinimizeWindow, window, _cx| window.minimize_window()))
            .on_action(cx.listener(|_, _: &CloseWindow, window, _cx| window.remove_window()))
            .on_action(cx.listener(|this, _: &ToggleChanges, _window, cx| {
                this.toggle_repository_changes(cx);
            }))
            .on_action(cx.listener(|this, _: &OpenModels, _window, cx| this.toggle_model_menu(cx)))
            .on_action(cx.listener(|this, _: &OpenWorktrees, _window, cx| this.open_worktrees(cx)))
            .on_action(
                cx.listener(|this, _: &OpenTerminal, window, cx| this.open_terminal(window, cx)),
            )
            .on_action(cx.listener(|this, _: &OpenPulls, _window, cx| this.open_pulls(cx)))
            .on_action(cx.listener(|this, _: &Back, _window, cx| this.navigate_back(cx)))
            .on_action(cx.listener(|this, _: &Forward, _window, cx| this.navigate_forward(cx)))
            .on_action(cx.listener(|this, _: &ShowShortcuts, _window, cx| {
                this.open_overlay(Overlay::Shortcuts, cx)
            }))
            .on_action(cx.listener(|this, _: &AcceptPrediction, window, cx| {
                this.accept_prediction(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &TerminalTab, _window, cx| {
                    this.terminal_write_text("\t", cx)
                }),
            )
            .on_action(cx.listener(|this, _: &TerminalBackTab, _window, cx| {
                this.terminal_write_text("\u{1b}[Z", cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchToSession1, window, cx| {
                this.open_sidebar_session(1, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchToSession2, window, cx| {
                this.open_sidebar_session(2, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchToSession3, window, cx| {
                this.open_sidebar_session(3, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchToSession4, window, cx| {
                this.open_sidebar_session(4, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchToSession5, window, cx| {
                this.open_sidebar_session(5, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchToSession6, window, cx| {
                this.open_sidebar_session(6, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchToSession7, window, cx| {
                this.open_sidebar_session(7, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchToSession8, window, cx| {
                this.open_sidebar_session(8, window, cx)
            }))
            .on_action(cx.listener(|this, _: &SwitchToSession9, window, cx| {
                this.open_sidebar_session(9, window, cx)
            }))
            .on_action(cx.listener(|_, _: &QuitApp, _window, cx| cx.quit()))
            .on_action(cx.listener(|this, _: &DismissOverlay, window, cx| {
                if matches!(
                    this.overlay,
                    Some(Overlay::Trust {
                        onboarding: true,
                        ..
                    })
                ) {
                } else if this.activities_open {
                    this.activities_open = false;
                    cx.notify();
                } else if this.assistant_actions.take().is_some()
                    || this.edited_actions.take().is_some()
                {
                    cx.notify();
                } else if this.composer_tools_open {
                    this.composer_tools_open = false;
                    cx.notify();
                } else if this.model_menu_open {
                    if this.model_menu_show_models {
                        this.model_menu_show_models = false;
                    } else {
                        this.model_menu_open = false;
                    }
                    cx.notify();
                } else if this.overlay.take().is_some() {
                    if this.file_viewer.is_some() {
                        this.file_viewer = None;
                    }
                    cx.notify();
                } else {
                    this.focus_composer(window, cx);
                }
            }))
            .child(views::topbar(self, &theme, scale, window, cx));

        let mut body = div().flex().flex_row().flex_1().min_h(px(0.0));
        body = body.child(views::rail(self, &theme, cx));

        if matches!(self.overlay, Some(Overlay::Settings)) {
            body = body.child(overlays::settings_page(self, &theme, cx));
        } else if self.rail == Rail::Customize {
            body = body.child(views::customize_sidebar(self, &theme, cx));
            body = body.child(views::sidebar_resizer(self, &theme, cx));
            body = body.child(views::customize_page(self, &theme, window, cx));
        } else if self.rail == Rail::Projects {
            body = body.child(views::projects_page(self, &theme, cx));
        } else {
            if self.sidebar_visible {
                body = body.child(views::sidebar(self, &theme, scale, window, cx));
                body = body.child(views::sidebar_resizer(self, &theme, cx));
            }
            body = body.child(views::main(self, &theme, scale, window, cx));
        }
        shell = shell.child(body);
        if self.sidebar_visible
            && self.rail == Rail::Chats
            && self.overlay.is_none()
            && self.inline_project_actions.is_some()
        {
            shell = shell.child(views::project_actions_floating(self, &theme, cx));
        }
        if !matches!(self.overlay, Some(Overlay::Settings)) {
            shell = shell.child(overlays::layer(self, &theme, scale, window, cx));
        }
        if self.resizing.is_some() || self.changes_resizing.is_some() {
            shell = shell.child(
                div()
                    .absolute()
                    .size_full()
                    .cursor(gpui::CursorStyle::ResizeLeftRight)
                    .on_mouse_move(cx.listener(|this, event, window, cx| {
                        this.handle_sidebar_drag(event, window, cx)
                    }))
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| this.end_sidebar_drag(cx)),
                    ),
            );
        }
        if let Some(toast) = &self.toast {
            let color = if toast.error {
                theme.error()
            } else {
                theme.text()
            };
            let background = if toast.error {
                theme.color("tool_error_bg")
            } else {
                theme.panel_2()
            };
            shell = shell.child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .flex()
                    .flex_row()
                    .justify_center()
                    .pb_4()
                    .child(
                        div()
                            .px_3()
                            .py_2()
                            .rounded(px(10.0))
                            .bg(background)
                            .border_1()
                            .border_color(theme.border())
                            .text_size(px(12.0))
                            .text_color(color)
                            .child(toast.text.clone()),
                    ),
            );
        }
        shell
    }
}

/// The window this front-end opens.
pub fn open(engine: Engine, packets: tokio::sync::mpsc::UnboundedReceiver<Value>, cx: &mut App) {
    let bounds = gpui::Bounds::centered(None, size(px(1280.0), px(840.0)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Maximized(bounds)),
        // The window draws its own topbar, so the platform's is hidden and the
        // traffic lights are placed inside it, the 76 points the topbar leaves
        // for them.
        titlebar: Some(gpui::TitlebarOptions {
            title: Some(SharedString::from("Oxide")),
            appears_transparent: true,
            traffic_light_position: Some(gpui::point(px(14.0), px(16.0))),
        }),
        ..Default::default()
    };
    let _ = cx.open_window(options, move |window, cx| {
        cx.new(|cx| Root::new(engine, packets, window, cx))
    });
}

impl Root {
    pub fn handle_sidebar_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((origin, start_width)) = self.resizing {
            let delta = f32::from(event.position.x) - origin;
            let viewport = f32::from(window.viewport_size().width);
            self.sidebar_width = clamp_sidebar_width(viewport, start_width + delta);
            cx.notify();
        }
    }

    pub fn begin_sidebar_drag(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.resizing = Some((f32::from(event.position.x), self.sidebar_width));
        cx.notify();
    }

    pub fn end_sidebar_drag(&mut self, cx: &mut Context<Self>) {
        self.resizing = None;
        cx.notify();
    }

    pub fn begin_changes_drag(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.changes_resizing = Some((f32::from(event.position.x), self.changes_width));
        cx.notify();
    }

    pub fn handle_changes_drag(
        &mut self,
        event: &MouseMoveEvent,
        window: &Window,
        cx: &mut Context<Self>,
    ) {
        if let Some((origin, start_width)) = self.changes_resizing {
            let viewport = f32::from(window.viewport_size().width);
            self.changes_width = resized_changes_width(
                viewport,
                self.sidebar_visible,
                self.sidebar_width,
                origin,
                f32::from(event.position.x),
                start_width,
            );
            cx.notify();
        }
    }

    pub fn end_changes_drag(&mut self, cx: &mut Context<Self>) {
        self.changes_resizing = None;
        cx.notify();
    }
}

fn clamp_sidebar_width(viewport: f32, width: f32) -> f32 {
    const MIN_SIDEBAR_WIDTH: f32 = 160.0;
    const MIN_READING_WIDTH: f32 = 475.0;
    let available = (viewport - MIN_READING_WIDTH).max(MIN_SIDEBAR_WIDTH);
    width.clamp(MIN_SIDEBAR_WIDTH, available)
}

fn resized_changes_width(
    viewport: f32,
    sidebar_visible: bool,
    sidebar_width: f32,
    origin: f32,
    current: f32,
    start_width: f32,
) -> f32 {
    let sidebar = if sidebar_visible {
        sidebar_width + 7.0
    } else {
        0.0
    };
    let max_width = (viewport - 56.0 - sidebar - 420.0).max(340.0);
    (start_width + origin - current).clamp(320.0, max_width)
}

fn slash_command(text: &str) -> Option<(&str, &str)> {
    let command = text.trim().strip_prefix('/')?;
    let end = command.find(char::is_whitespace).unwrap_or(command.len());
    let (name, args) = command.split_at(end);
    (!name.is_empty()).then(|| (name, args.trim()))
}

fn performs_slash_command(name: &str, args: &str) -> bool {
    match name {
        "mcp" => args.is_empty(),
        "help" | "model" | "agent" | "permissions" | "computer-use" | "trust" | "reasoning"
        | "connect" => true,
        "changes" | "terminal" | "pulls" | "customize" => args.is_empty(),
        _ => false,
    }
}

fn can_reuse_new_chat(chat: &Chat, composer_text: &str) -> bool {
    chat.session.is_none()
        && chat.entries.is_empty()
        && !chat.running()
        && chat.attachments.is_empty()
        && chat.queued.is_empty()
        && composer_text.trim().is_empty()
}

fn synchronize_session_titles(chats: &mut [Chat], sessions: &[SessionRow]) {
    let titles: HashMap<&str, &SessionRow> = sessions
        .iter()
        .filter(|session| !session.title.trim().is_empty())
        .map(|session| (session.id.as_str(), session))
        .collect();
    for chat in chats {
        let Some(session) = chat.session.as_deref() else {
            continue;
        };
        if let Some(row) = titles.get(session) {
            if row.named || chat.provisional_title {
                chat.title = row.title.clone();
                if row.named {
                    chat.provisional_title = false;
                }
            }
        }
    }
}

/// Whether picking another project moves this tab with it: a draft nothing has
/// been written in yet stands in one folder at a time and follows the picker,
/// while a tab that holds work keeps the folder that work was started in.
fn draft_follows_project(chat: &Chat, composer_text: &str, project: &str) -> bool {
    chat.project != project && can_reuse_new_chat(chat, composer_text)
}

#[cfg(test)]
mod sidebar_resize_tests {
    use super::{
        add_attachment, can_reuse_new_chat, clamp_sidebar_width, composer_has_content,
        details_visible, draft_follows_project, performs_slash_command, resized_changes_width,
        slash_command, synchronize_session_titles, toggled_details, Rail,
    };
    use crate::model::{Attachment, Chat, SessionRow};
    use crate::views::{customize_destination, effort_position, model_display_name};

    #[test]
    fn sidebar_resize_preserves_the_reading_column() {
        assert_eq!(clamp_sidebar_width(1280.0, 320.0), 320.0);
        assert_eq!(clamp_sidebar_width(1280.0, 120.0), 160.0);
        assert_eq!(clamp_sidebar_width(1280.0, 900.0), 805.0);
        assert_eq!(clamp_sidebar_width(320.0, 280.0), 160.0);
    }

    #[test]
    fn changes_resize_tracks_the_divider_and_preserves_the_reading_column() {
        assert_eq!(
            resized_changes_width(1728.0, true, 300.0, 1280.0, 1180.0, 430.0),
            530.0
        );
        assert_eq!(
            resized_changes_width(1728.0, true, 300.0, 1280.0, 1500.0, 430.0),
            320.0
        );
        assert_eq!(
            resized_changes_width(1100.0, true, 300.0, 900.0, 700.0, 430.0),
            340.0
        );
    }

    #[test]
    fn project_details_are_available_only_with_an_open_chat() {
        assert!(details_visible(true, true));
        assert!(!details_visible(true, false));
        assert!(!details_visible(false, true));
        assert!(!toggled_details(true, true));
        assert!(toggled_details(false, true));
        assert!(toggled_details(true, false));
    }

    #[test]
    fn slash_commands_are_split_and_routed() {
        assert_eq!(slash_command("/model"), Some(("model", "")));
        assert_eq!(
            slash_command("  /Reasoning   high  "),
            Some(("Reasoning", "high"))
        );
        assert_eq!(slash_command("what is /model?"), None);
        assert_eq!(slash_command("/"), None);

        for command in [
            "help",
            "mcp",
            "model",
            "agent",
            "permissions",
            "computer-use",
            "trust",
            "reasoning",
        ] {
            assert!(performs_slash_command(command, ""), "{command}");
        }
        assert!(performs_slash_command("reasoning", "high"));
        assert!(!performs_slash_command("mcp", "list"));
        assert!(!performs_slash_command("tdd", ""));
    }

    #[test]
    fn customize_leaves_settings_in_one_click_and_still_toggles_its_rail() {
        assert_eq!(
            customize_destination(Rail::Customize, true),
            Rail::Customize
        );
        assert_eq!(customize_destination(Rail::Chats, true), Rail::Customize);
        assert_eq!(customize_destination(Rail::Customize, false), Rail::Chats);
        assert_eq!(customize_destination(Rail::Chats, false), Rail::Customize);
    }

    #[test]
    fn effort_slider_tracks_the_selected_level_and_models_read_as_names() {
        let levels = ["auto", "low", "medium", "high", "max"]
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        assert_eq!(effort_position(&levels, "high"), Some(3));
        assert_eq!(effort_position(&levels, "HIGH"), Some(3));
        assert_eq!(effort_position(&levels, "unknown"), None);
        assert_eq!(
            model_display_name("deepseek-v4.1-flash"),
            "DeepSeek V4.1 Flash"
        );
        assert_eq!(model_display_name("gpt-5"), "GPT 5");
    }

    #[test]
    fn a_blank_new_chat_is_reused_instead_of_opening_a_duplicate_tab() {
        let chat = Chat::new("/tmp/project".to_string(), "New chat".to_string());
        assert!(can_reuse_new_chat(&chat, "   "));

        let mut composed = Chat::new("/tmp/project".to_string(), "New chat".to_string());
        assert!(!can_reuse_new_chat(&composed, "Explain this"));

        composed.session = Some("saved-thread".to_string());
        assert!(!can_reuse_new_chat(&composed, ""));
    }

    #[test]
    fn a_refreshed_session_title_reaches_the_open_chat() {
        let mut chat = Chat::new("/tmp/project".to_string(), "GPUI Desktop".to_string());
        chat.session = Some("session".to_string());
        let sessions = vec![SessionRow {
            id: "session".to_string(),
            title: "GPUI Desktop: Fix the composer title".to_string(),
            ..SessionRow::default()
        }];
        synchronize_session_titles(std::slice::from_mut(&mut chat), &sessions);
        assert_eq!(chat.title, "GPUI Desktop: Fix the composer title");

        chat.provisional_title = false;
        chat.title = "Generated title".to_string();
        synchronize_session_titles(std::slice::from_mut(&mut chat), &sessions);
        assert_eq!(chat.title, "Generated title");
    }

    #[test]
    fn picking_a_project_moves_a_blank_draft_instead_of_opening_a_tab() {
        let draft = Chat::new("/tmp/project".to_string(), "New chat".to_string());
        assert!(draft_follows_project(&draft, "", "/tmp/other"));
        assert!(!draft_follows_project(&draft, "", "/tmp/project"));
        assert!(!draft_follows_project(
            &draft,
            "half a thought",
            "/tmp/other"
        ));

        let mut held = Chat::new("/tmp/project".to_string(), "New chat".to_string());
        held.attachments.push(Attachment::new(
            "shot.png".to_string(),
            "data:image/png;base64,00".to_string(),
        ));
        assert!(!draft_follows_project(&held, "", "/tmp/other"));

        let mut saved = Chat::new("/tmp/project".to_string(), "Older thread".to_string());
        saved.session = Some("saved-thread".to_string());
        assert!(!draft_follows_project(&saved, "", "/tmp/other"));
    }

    #[test]
    fn attachments_deduplicate_and_keep_the_composer_limit() {
        let mut chat = Chat::new("/tmp/project".to_string(), "New chat".to_string());
        for index in 0..8 {
            add_attachment(
                &mut chat,
                Attachment::new(
                    format!("image-{index}.png"),
                    format!("data:image/png;base64,{index}"),
                ),
            )
            .unwrap();
        }
        add_attachment(
            &mut chat,
            Attachment::new(
                "again.png".to_string(),
                "data:image/png;base64,0".to_string(),
            ),
        )
        .unwrap();
        assert_eq!(chat.attachments.len(), 8);
        assert!(add_attachment(
            &mut chat,
            Attachment::new(
                "ninth.png".to_string(),
                "data:image/png;base64,ninth".to_string(),
            ),
        )
        .unwrap_err()
        .contains("At most 8"));
    }

    #[test]
    fn an_attachment_can_start_a_turn_without_text() {
        assert!(composer_has_content("", true));
        assert!(!composer_has_content("  \n ", false));
        assert!(composer_has_content("Describe this", false));
    }
}

#[cfg(test)]
mod project_flow_tests {
    use super::{added_project_path, agent_start_title, project_trust_awaiting};
    use crate::model;
    use serde_json::json;

    #[test]
    fn a_new_project_is_the_one_the_window_opens() {
        let value = json!({
            "added": "new-id",
            "projects": [
                { "id": "old-id", "path": "/tmp/old", "name": "Old" },
                { "id": "new-id", "path": "/tmp/new", "name": "New" }
            ]
        });
        let projects = model::projects(&value["projects"]);
        assert_eq!(
            added_project_path(&value, &projects, "/tmp/typed".to_string()),
            "/tmp/new"
        );
    }

    #[test]
    fn trust_prompts_read_the_trust_object() {
        assert!(project_trust_awaiting(
            &json!({ "trust": { "awaiting": true } })
        ));
        assert!(!project_trust_awaiting(
            &json!({ "trust": { "awaiting": false } })
        ));
    }

    #[test]
    fn an_existing_chat_keeps_the_title_its_sidebar_names() {
        assert_eq!(
            agent_start_title("Old title", Some("New topic"), false),
            None
        );
        assert_eq!(
            agent_start_title("Old title", Some("New topic"), true),
            Some("New topic".to_string())
        );
        assert_eq!(
            agent_start_title("New chat", Some("First topic"), false),
            Some("First topic".to_string())
        );
    }
}

actions!(composer, [Send, Stop]);

/// A one-pixel rule.
pub fn rule(theme: &Theme) -> gpui::Div {
    div().h(px(1.0)).w_full().bg(theme.border())
}

#[cfg(test)]
mod workspace_parity_tests {
    use super::change_hunk;
    use serde_json::json;

    #[test]
    fn a_hunk_carries_context_and_the_line_number_each_side_uses() {
        let lines = vec![
            json!({ "kind": "context", "old": 3, "new": 3, "text": "before" }),
            json!({ "kind": "remove", "old": 4, "new": null, "text": "old" }),
            json!({ "kind": "add", "old": null, "new": 4, "text": "new" }),
            json!({ "kind": "context", "old": 5, "new": 5, "text": "after" }),
        ];
        let hunk = change_hunk(&lines, 1, 2);
        assert_eq!(hunk["old_start"], 3);
        assert_eq!(hunk["new_start"], 3);
        assert_eq!(hunk["lines"].as_array().unwrap().len(), 4);
        assert_eq!(hunk["lines"][1]["kind"], "remove");
        assert_eq!(hunk["lines"][2]["kind"], "add");
    }
}

/// A button in the window's own style: the page's `ghost` and `primary` rows.
pub struct Button {
    pub label: String,
    pub primary: bool,
    pub danger: bool,
    pub width: Option<f32>,
}

impl Button {
    pub fn new(label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            primary: false,
            danger: false,
            width: None,
        }
    }

    pub fn primary(mut self) -> Self {
        self.primary = true;
        self
    }

    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    pub fn render(
        self,
        id: impl Into<gpui::ElementId>,
        theme: &Theme,
        cx: &mut Context<Root>,
        on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
    ) -> gpui::Stateful<gpui::Div> {
        let background = if self.primary {
            theme.text()
        } else {
            theme.panel_2()
        };
        let color = if self.primary {
            theme.background()
        } else if self.danger {
            theme.error()
        } else {
            theme.text()
        };
        let hover = if self.primary {
            theme.wash("text", 0.88)
        } else {
            theme.panel_3()
        };
        let mut row = div()
            .id(id)
            .role(gpui::Role::Button)
            .aria_label(self.label.clone())
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .tab_index(0)
            .h(px(32.0))
            .px_3()
            .rounded(px(8.0))
            .bg(background)
            .border_1()
            .border_color(background)
            .text_color(color)
            .text_size(px(12.5))
            .hover(move |style| style.bg(hover))
            .focus(move |style| style.border_color(theme.accent()))
            .child(self.label);
        if let Some(width) = self.width {
            row = row.w(px(width));
        }
        row.on_click(cx.listener(move |root, _event, window, cx| on_click(root, window, cx)))
    }
}
