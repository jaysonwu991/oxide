//! The window itself: the state the front-end holds, the engine's commands it
//! calls, and the packets it paints.
//!
//! This is the GPUI front-end of the desktop app. It is a beta: the same engine,
//! the same configuration, the same session store and the same approvals as the
//! Electron window in `crates/desktop`, drawn natively by GPUI instead of a
//! webview. Everything the engine answers — projects, threads, turns, cards,
//! diffs, models, themes, prefs — is read from the commands in
//! `oxide_desktop::commands`, which is why both windows behave the same way.

use crate::editor::{self, Editor};
use crate::engine::Engine;
use crate::model::{Attachment, Chat, Entry, ProjectRow, QueuedMessage, SessionRow};
use crate::overlays;
use crate::theme::Theme;
use crate::views;
use gpui::prelude::*;
use gpui::{
    actions, div, px, size, App, Context, Entity, Focusable, KeyBinding, MouseButton,
    MouseDownEvent, MouseMoveEvent, Render, SharedString, Subscription, Window, WindowBounds,
    WindowOptions,
};
use oxide_core::worktree::{Worktree, Worktrees};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

actions!(
    shortcuts,
    [
        NewChat,
        CloseChat,
        ToggleSidebar,
        ToggleDetails,
        OpenSettings,
        ToggleChanges,
        OpenModels,
        OpenWorktrees,
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
#[derive(Clone, Copy, PartialEq, Eq)]
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

/// What is drawn over the window.
#[derive(Clone)]
pub enum Overlay {
    Settings,
    About,
    Updates,
    Models,
    Shortcuts,
    Trust {
        project: String,
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
    Worktrees,
    WorktreeBranch {
        project: String,
        name: String,
    },
    ComposerTools,
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
    pub sessions: Vec<SessionRow>,
    pub chats: Vec<Chat>,
    pub active: usize,
    pub history: Vec<usize>,
    pub forward: Vec<usize>,
    pub next_chat: u64,
    pub editors: HashMap<u64, Entity<Editor>>,
    pub search: Entity<Editor>,
    pub worktree_name: Entity<Editor>,
    pub worktree_base: Entity<Editor>,
    pub worktree_branch: Entity<Editor>,
    pub session_name: Entity<Editor>,
    pub query: String,
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
    pub approvals: Vec<String>,
    pub commands: Vec<Value>,
    pub at_answer: Value,
    pub at_chat: Option<u64>,
    pub at_generation: u64,
    /// The branch the open thread's folder is on, when it is a repository. The
    /// composer's top row names it, and leaves the fact out when there is none.
    pub branch: Option<String>,
    /// The MCP servers this project loads, as the details pane lists them.
    pub sources: Vec<Value>,
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
    /// The pointer position and sidebar width when a resize drag began.
    pub resizing: Option<(f32, f32)>,
    pub show_changes: bool,
    pub update_check: Option<Value>,
    pub update_error: Option<String>,
    pub checking_updates: bool,
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
                theme.faint(),
                theme.accent(),
                theme.wash("accent", 0.25),
                cx,
            )
        });
        let worktree_name = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "Worktree name",
                theme.text(),
                theme.faint(),
                theme.accent(),
                theme.wash("accent", 0.25),
                cx,
            )
        });
        let worktree_base = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "HEAD (current checkout)",
                theme.text(),
                theme.faint(),
                theme.accent(),
                theme.wash("accent", 0.25),
                cx,
            )
        });
        let worktree_branch = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "Branch name",
                theme.text(),
                theme.faint(),
                theme.accent(),
                theme.wash("accent", 0.25),
                cx,
            )
        });
        let session_name = cx.new(|cx| {
            let theme = Theme::default();
            Editor::new(
                "Chat title",
                theme.text(),
                theme.faint(),
                theme.accent(),
                theme.wash("accent", 0.25),
                cx,
            )
        });

        let mut subscriptions = Vec::new();
        subscriptions.push(cx.subscribe(&search, |root, _editor, _event, cx| {
            let query = root.search_text(cx);
            if query != root.query {
                root.query = query;
                root.search_sessions(cx);
            }
            cx.notify();
        }));

        let mut root = Self {
            engine,
            theme: Theme::default(),
            strings: Strings(HashMap::new()),
            projects: Vec::new(),
            sessions: Vec::new(),
            chats: Vec::new(),
            active: 0,
            history: Vec::new(),
            forward: Vec::new(),
            next_chat: 1,
            editors: HashMap::new(),
            search,
            worktree_name,
            worktree_base,
            worktree_branch,
            session_name,
            query: String::new(),
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
            approvals: Vec::new(),
            commands: Vec::new(),
            at_answer: Value::Null,
            at_chat: None,
            at_generation: 0,
            branch: None,
            sources: Vec::new(),
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
            resizing: None,
            show_changes: false,
            update_check: None,
            update_error: None,
            checking_updates: false,
            subscriptions,
        };

        window.set_window_title("Oxide Beta");
        root.load_prefs(cx);
        root.load_language(cx);
        root.load_themes(cx);
        root.refresh_projects(cx);

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
        bindings.push(KeyBinding::new("cmd-j", ToggleDetails, Some("Root")));
        bindings.push(KeyBinding::new("cmd-,", OpenSettings, Some("Root")));
        bindings.push(KeyBinding::new("cmd-enter", ToggleChanges, Some("Root")));
        bindings.push(KeyBinding::new("cmd-shift-m", OpenModels, Some("Root")));
        bindings.push(KeyBinding::new("cmd-shift-g", OpenWorktrees, Some("Root")));
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

    /// The window's menu bar, the way the stable app's Electron menu is built.
    pub fn install_menus(cx: &mut App) {
        use gpui::{Menu, MenuItem, OsAction};
        cx.set_menus(vec![
            Menu {
                name: "Oxide Beta".into(),
                items: vec![
                    MenuItem::action("About Oxide Beta", AboutApp),
                    MenuItem::separator(),
                    MenuItem::action("Check for Updates…", CheckForUpdates),
                    MenuItem::action("Settings…", OpenSettings),
                    MenuItem::separator(),
                    MenuItem::action("Hide Oxide Beta", HideApp),
                    MenuItem::separator(),
                    MenuItem::action("Quit Oxide Beta", QuitApp),
                ],
            },
            Menu {
                name: "File".into(),
                items: vec![
                    MenuItem::action("New Chat", NewChat),
                    MenuItem::action("Close Chat", CloseChat),
                ],
            },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::os_action("Cut", editor::Cut, OsAction::Cut),
                    MenuItem::os_action("Copy", editor::Copy, OsAction::Copy),
                    MenuItem::os_action("Paste", editor::Paste, OsAction::Paste),
                    MenuItem::os_action("Select All", editor::SelectAll, OsAction::SelectAll),
                ],
            },
            Menu {
                name: "View".into(),
                items: vec![
                    MenuItem::action("Show or Hide Details", ToggleDetails),
                    MenuItem::action("Show or Hide the Sidebar", ToggleSidebar),
                    MenuItem::action("Changes", ToggleChanges),
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
                items: vec![
                    MenuItem::action("Minimize", MinimizeWindow),
                    MenuItem::action("Close Window", CloseWindow),
                ],
            },
            Menu {
                name: "Help".into(),
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
            |root, result, cx| match result {
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

    pub fn check_for_updates(&mut self, cx: &mut Context<Self>) {
        self.overlay = Some(Overlay::Updates);
        self.update_check = None;
        self.update_error = None;
        self.checking_updates = true;
        self.call(
            "check_gpui_updates",
            json!({ "current": env!("CARGO_PKG_VERSION") }),
            cx,
            |root, result, cx| {
                root.checking_updates = false;
                match result {
                    Ok(value) => root.update_check = Some(value),
                    Err(error) => root.update_error = Some(error),
                }
                cx.notify();
            },
        );
        cx.notify();
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
        self.call(
            "list_projects",
            json!({}),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    root.projects = crate::model::projects(&value);
                    root.refresh_sessions(cx);
                }
                Err(error) => root.toast(format!("Could not list projects: {error}"), true, cx),
            },
        );
    }

    pub fn refresh_sessions(&mut self, cx: &mut Context<Self>) {
        self.call("all_sessions", json!({}), cx, |root, result, _| {
            if let Ok(value) = result {
                root.sessions = value
                    .as_array()
                    .map(|sessions| sessions.iter().filter_map(SessionRow::from_value).collect())
                    .unwrap_or_default();
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
        self.searching = true;
        let editor = self.search.clone();
        window.focus(&editor.read(cx).focus_handle(cx));
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
        self.call(
            "project_info",
            json!({ "project": project }),
            cx,
            |root, result, _| {
                if let Ok(value) = result {
                    root.info = value;
                }
            },
        );
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
            |root, result, _cx| {
                if let Ok(value) = result {
                    root.commands = value.as_array().cloned().unwrap_or_default();
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
        self.push_chat(window, project, None, "New chat".to_string(), cx);
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
        self.show_details = !self.show_details;
        if self.show_details {
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
            window.focus(&editor.read(cx).focus_handle(cx));
        }
        cx.notify();
    }

    // ---------- chats ----------

    /// The project a new chat in the active column belongs to.
    pub fn new_chat_project(&self) -> Option<String> {
        self.current_project()
    }

    pub fn new_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(project) = self.new_chat_project() else {
            self.overlay = Some(Overlay::NewProject);
            cx.notify();
            return;
        };
        self.push_chat(window, project, None, "New chat".to_string(), cx);
    }

    pub fn push_chat(
        &mut self,
        window: &mut Window,
        project: String,
        session: Option<String>,
        title: String,
        cx: &mut Context<Self>,
    ) {
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
                theme.faint(),
                theme.accent(),
                theme.wash("accent", 0.28),
                cx,
            )
        });
        self.subscriptions
            .push(cx.subscribe(&editor, move |root, editor, _event, cx| {
                let (text, caret) = {
                    let editor = editor.read(cx);
                    (editor.text().to_string(), editor.cursor_utf16())
                };
                root.refresh_at_suggestions(id, text, caret, cx);
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
        if let Some(editor) = self.editors.get(&id) {
            window.focus(&editor.read(cx).focus_handle(cx));
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

    pub fn activate(&mut self, window: &mut Window, index: usize, cx: &mut Context<Self>) {
        if index == self.active || index >= self.chats.len() {
            return;
        }
        self.history.push(self.active);
        self.forward.clear();
        self.active = index;
        self.changes = None;
        self.show_changes = false;
        self.load_project_commands(cx);
        self.refresh_repository(index, cx);
        if let Some(editor) = self
            .chats
            .get(index)
            .and_then(|chat| self.editors.get(&chat.id))
        {
            window.focus(&editor.read(cx).focus_handle(cx));
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

    pub fn focus_composer(&self, window: &mut Window, cx: &App) {
        if let Some(chat) = self.chats.get(self.active) {
            if let Some(editor) = self.editors.get(&chat.id) {
                window.focus(&editor.read(cx).focus_handle(cx));
            }
        }
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

    pub fn refresh_repository(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(project) = self.chats.get(index).map(|chat| chat.project.clone()) else {
            return;
        };
        self.call(
            "repository_changes",
            json!({ "project": project }),
            cx,
            move |root, result, _| {
                if let Ok(value) = result {
                    if let Some(chat) = root.chats.get_mut(index) {
                        chat.changes = Some(value.clone());
                    }
                    if index == root.active {
                        root.changes = Some(value);
                    }
                }
            },
        );
    }

    pub fn send(&mut self, cx: &mut Context<Self>) {
        let index = self.active;
        let Some(chat) = self.chats.get_mut(index) else {
            return;
        };
        let Some(editor) = self.editors.get(&chat.id).cloned() else {
            return;
        };
        let prompt = editor.read(cx).text().trim().to_string();
        if prompt.is_empty() {
            return;
        }
        if let Some(run) = chat.run {
            let follow_up = self.prefs["follow_up"].as_str() != Some("steer");
            let queue = chat.attachments.clone();
            let names = queue
                .iter()
                .map(|item| item.name.clone())
                .collect::<Vec<_>>();
            chat.queued.push(QueuedMessage {
                text: prompt.clone(),
                attachments: names,
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
        let attachments: Vec<Value> = chat
            .attachments
            .iter()
            .map(|attachment| json!({ "dataUrl": attachment.data_url, "name": attachment.name }))
            .collect();
        let names: Vec<String> = chat
            .attachments
            .iter()
            .map(|attachment| attachment.name.clone())
            .collect();

        chat.entries.push(Entry::User {
            text: prompt.clone(),
            attachments: names,
        });
        chat.attachments.clear();
        chat.scroll_to_end = true;
        chat.awaiting_run = true;

        let mut args = json!({
            "project": project,
            "prompt": prompt,
            "session": session,
            "attachments": if attachments.is_empty() { Value::Null } else { Value::Array(attachments) },
        });
        if let Some(reasoning) = reasoning {
            args["reasoning"] = json!(reasoning);
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
                        chat.awaiting_run = false;
                        chat.entries.push(Entry::Error(error));
                    }
                }
            }
            cx.notify();
        });

        editor.update(cx, |editor, cx| editor.clear(cx));
    }

    pub fn stop(&mut self, cx: &mut Context<Self>) {
        let index = self.active;
        let Some(run) = self.chats.get(index).and_then(|chat| chat.run) else {
            return;
        };
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
        self.call(
            "set_model",
            json!({ "provider": provider, "model": model }),
            cx,
            |root, result, cx| match result {
                Ok(_) => {
                    root.overlay = None;
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
            prompt: Some(SharedString::from("Add a project")),
        });
        cx.spawn(async move |this, cx| {
            let result = paths.await.unwrap_or_else(|_| Ok(None));
            let _ = this.update(cx, |root, cx| match result {
                Ok(Some(paths)) => {
                    if let Some(path) = paths.into_iter().next() {
                        let path = path.to_string_lossy().to_string();
                        root.call(
                            "add_project",
                            json!({ "path": path }),
                            cx,
                            |root, result, cx| match result {
                                Ok(_) => root.refresh_projects(cx),
                                Err(error) => root.toast(error, true, cx),
                            },
                        );
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
    /// the stable window's `pick_files` does — the same data URL the core reads.
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
                                chat.attachments.push(attachment);
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

    // ---------- packets ----------

    fn event(&mut self, name: &str, payload: Value, cx: &mut Context<Self>) {
        match name {
            "agent-event" => {
                let run = payload["runId"].as_u64();
                let kind = payload["type"].as_str().unwrap_or_default().to_string();
                if let Some(index) = self.claim_run(run) {
                    self.chats[index].apply(&payload);
                }
                if kind == "session_title" {
                    self.refresh_sessions(cx);
                }
            }
            "agent-start" => {
                let run = payload["runId"].as_u64().unwrap_or_default();
                let session = payload["sessionId"].as_str().map(str::to_string);
                let title = payload["title"].as_str().map(str::to_string);
                if let Some(index) = self.claim_run(Some(run)) {
                    let chat = &mut self.chats[index];
                    chat.session = session;
                    if let Some(title) = title {
                        if !title.trim().is_empty() {
                            chat.title = title;
                        }
                    }
                }
                self.refresh_sessions(cx);
            }
            "agent-end" => {
                let run = payload["runId"].as_u64();
                if let Some(index) = self.claim_run(run) {
                    let chat = &mut self.chats[index];
                    chat.run = None;
                    chat.awaiting_run = false;
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
                self.refresh_repository(index, cx);
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
                if matches!(self.overlay, Some(Overlay::Models)) {
                    self.load_models(cx);
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
        match &overlay {
            Overlay::Models => self.load_models(cx),
            Overlay::Settings => self.load_project_commands(cx),
            Overlay::Trust { project } => {
                let project = project.clone();
                self.call(
                    "project_info",
                    json!({ "project": project }),
                    cx,
                    |root, result, _| {
                        if let Ok(value) = result {
                            root.info = value;
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
    Ok(Some(Attachment {
        name,
        data_url: format!(
            "data:{mime};base64,{}",
            oxide_core::media::base64_encode(&bytes)
        ),
    }))
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

        let theme = self.theme.clone();
        let scale = self.prefs["font_scale"]
            .as_f64()
            .unwrap_or(1.0)
            .clamp(0.7, 1.6) as f32;

        let mut shell = div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.background())
            .text_color(theme.text())
            .text_size(px(13.5 * scale))
            .font_family(".SystemUIFont")
            .cursor(if self.resizing.is_some() {
                gpui::CursorStyle::ResizeLeftRight
            } else {
                gpui::CursorStyle::Arrow
            })
            .key_context("Root")
            .on_mouse_move(cx.listener(|this, event, window, cx| {
                if this.resizing.is_some() {
                    this.handle_sidebar_drag(event, window, cx);
                }
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.end_sidebar_drag(cx)),
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
            .on_action(cx.listener(|this, _: &ToggleDetails, _window, cx| {
                this.show_details = !this.show_details;
                cx.notify();
            }))
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
                this.show_changes = !this.show_changes;
                if this.show_changes {
                    let index = this.active;
                    this.refresh_repository(index, cx);
                }
                cx.notify();
            }))
            .on_action(cx.listener(|this, _: &OpenModels, _window, cx| {
                this.open_overlay(Overlay::Models, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenWorktrees, _window, cx| this.open_worktrees(cx)))
            .on_action(cx.listener(|this, _: &Back, _window, cx| this.navigate_back(cx)))
            .on_action(cx.listener(|this, _: &Forward, _window, cx| this.navigate_forward(cx)))
            .on_action(cx.listener(|this, _: &ShowShortcuts, _window, cx| {
                this.open_overlay(Overlay::Shortcuts, cx)
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
                if this.overlay.take().is_some() {
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
        if !matches!(self.overlay, Some(Overlay::Settings)) {
            shell = shell.child(overlays::layer(self, &theme, scale, window, cx));
        }
        if self.resizing.is_some() {
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
}

fn clamp_sidebar_width(viewport: f32, width: f32) -> f32 {
    const MIN_SIDEBAR_WIDTH: f32 = 160.0;
    const MIN_READING_WIDTH: f32 = 475.0;
    let available = (viewport - MIN_READING_WIDTH).max(MIN_SIDEBAR_WIDTH);
    width.clamp(MIN_SIDEBAR_WIDTH, available)
}

#[cfg(test)]
mod sidebar_resize_tests {
    use super::clamp_sidebar_width;

    #[test]
    fn sidebar_resize_preserves_the_reading_column() {
        assert_eq!(clamp_sidebar_width(1280.0, 320.0), 320.0);
        assert_eq!(clamp_sidebar_width(1280.0, 120.0), 160.0);
        assert_eq!(clamp_sidebar_width(1280.0, 900.0), 805.0);
        assert_eq!(clamp_sidebar_width(320.0, 280.0), 160.0);
    }
}

actions!(composer, [Send, Stop]);

/// A one-pixel rule.
pub fn rule(theme: &Theme) -> gpui::Div {
    div().h(px(1.0)).w_full().bg(theme.border())
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
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .h(px(32.0))
            .px_3()
            .rounded(px(8.0))
            .bg(background)
            .text_color(color)
            .text_size(px(12.5))
            .hover(move |style| style.bg(hover))
            .child(self.label);
        if let Some(width) = self.width {
            row = row.w(px(width));
        }
        row.on_click(cx.listener(move |root, _event, window, cx| on_click(root, window, cx)))
    }
}
