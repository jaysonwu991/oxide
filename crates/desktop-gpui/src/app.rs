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
use crate::model::{Attachment, Chat, Entry, ProjectRow, SessionRow};
use crate::overlays;
use crate::theme::Theme;
use crate::views;
use gpui::prelude::*;
use gpui::{
    actions, div, px, size, App, Context, Entity, Focusable, KeyBinding, MouseDownEvent,
    MouseMoveEvent, Render, SharedString, Subscription, Window, WindowBounds, WindowOptions,
};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

actions!(
    shortcuts,
    [
        NewChat,
        CloseChat,
        ToggleSidebar,
        OpenSettings,
        ToggleChanges,
        OpenModels,
        Back,
        Forward,
        DismissOverlay,
        QuitApp,
        ShowShortcuts,
    ]
);

/// What the navigation rail shows.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Rail {
    Chats,
    Customize,
}

/// What is drawn over the window.
#[derive(Clone)]
pub enum Overlay {
    Settings,
    Models,
    Themes,
    Language,
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
        pinned: bool,
    },
    ProjectActions {
        id: String,
        path: String,
        name: String,
        pinned: bool,
    },
    Archived,
    Connect {
        provider: String,
    },
    NewProject,
}

#[derive(Clone)]
pub enum ConfirmAction {
    RemoveProject(String),
    ArchiveSession(String),
    CloseChat(usize),
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
    pub query: String,
    pub results: Vec<SessionRow>,
    pub rail: Rail,
    pub overlay: Option<Overlay>,
    pub toast: Option<Toast>,
    pub prefs: Value,
    pub info: Value,
    pub models: Value,
    pub approvals: Vec<String>,
    pub theme_names: Vec<String>,
    pub current_theme: String,
    pub languages: Vec<Value>,
    pub language: String,
    pub archived: Vec<SessionRow>,
    pub changes: Option<Value>,
    pub sidebar_visible: bool,
    pub sidebar_width: f32,
    pub resizing: Option<f32>,
    pub collapsed: HashSet<String>,
    pub show_changes: bool,
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
            query: String::new(),
            results: Vec::new(),
            rail: Rail::Chats,
            overlay: None,
            toast: None,
            prefs: Value::Null,
            info: Value::Null,
            models: Value::Null,
            approvals: Vec::new(),
            theme_names: Vec::new(),
            current_theme: "dark".to_string(),
            languages: Vec::new(),
            language: String::new(),
            archived: Vec::new(),
            changes: None,
            sidebar_visible: true,
            sidebar_width: 268.0,
            resizing: None,
            collapsed: HashSet::new(),
            show_changes: false,
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
        bindings.push(KeyBinding::new("cmd-n", NewChat, Some("Root")));
        bindings.push(KeyBinding::new("cmd-w", CloseChat, Some("Root")));
        bindings.push(KeyBinding::new("cmd-b", ToggleSidebar, Some("Root")));
        bindings.push(KeyBinding::new("cmd-,", OpenSettings, Some("Root")));
        bindings.push(KeyBinding::new("cmd-enter", ToggleChanges, Some("Root")));
        bindings.push(KeyBinding::new("cmd-shift-m", OpenModels, Some("Root")));
        bindings.push(KeyBinding::new("cmd-[", Back, Some("Root")));
        bindings.push(KeyBinding::new("cmd-]", Forward, Some("Root")));
        bindings.push(KeyBinding::new("escape", DismissOverlay, Some("Root")));
        bindings.push(KeyBinding::new("cmd-q", QuitApp, Some("Root")));
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
        use gpui::{Menu, MenuItem};
        cx.set_menus(vec![
            Menu {
                name: "Oxide Beta".into(),
                items: vec![
                    MenuItem::action("About Oxide Beta", ShowShortcuts),
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
                name: "View".into(),
                items: vec![
                    MenuItem::action("Show or Hide the Sidebar", ToggleSidebar),
                    MenuItem::action("Changes", ToggleChanges),
                    MenuItem::action("Model Picker", OpenModels),
                ],
            },
            Menu {
                name: "Window".into(),
                items: vec![
                    MenuItem::action("Settings", OpenSettings),
                    MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
                ],
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

    /// The language tag the window is drawn in.
    pub fn strings_language(&self) -> String {
        self.language.clone()
    }

    pub fn load_themes(&mut self, cx: &mut Context<Self>) {
        let project = self.current_project().unwrap_or_default();
        self.call(
            "list_themes",
            json!({ "project": project }),
            cx,
            |root, result, _| {
                if let Ok(value) = result {
                    root.theme_names = value["names"]
                        .as_array()
                        .map(|names| {
                            names
                                .iter()
                                .filter_map(|name| name.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default();
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
                    root.overlay = None;
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
                    root.projects = value["projects"]
                        .as_array()
                        .map(|projects| {
                            projects.iter().filter_map(ProjectRow::from_value).collect()
                        })
                        .unwrap_or_default();
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

    pub fn load_project_commands(&mut self, cx: &mut Context<Self>) {
        self.load_project_info(cx);
        self.load_models(cx);
        self.load_approvals(cx);
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
            .push(cx.subscribe(&editor, |_root, editor, _event, cx| {
                let _ = editor.read(cx).height();
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
            chat.entries.push(Entry::User {
                text: prompt.clone(),
                attachments: queue.iter().map(|item| item.name.clone()).collect(),
            });
            chat.attachments.clear();
            chat.scroll_to_end = true;
            let attachments: Vec<Value> = queue
                .iter()
                .map(
                    |attachment| json!({ "dataUrl": attachment.data_url, "name": attachment.name }),
                )
                .collect();
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
                |root, result, cx| {
                    if let Err(error) = result {
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

    pub fn archive_session(&mut self, id: String, cx: &mut Context<Self>) {
        self.overlay = None;
        self.call(
            "archive_session",
            json!({ "id": id }),
            cx,
            |root, result, cx| match result {
                Ok(_) => root.refresh_sessions(cx),
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

    pub fn load_archived(&mut self, cx: &mut Context<Self>) {
        self.call("archived_sessions", json!({}), cx, |root, result, _| {
            if let Ok(value) = result {
                root.archived = value
                    .as_array()
                    .map(|sessions| sessions.iter().filter_map(SessionRow::from_value).collect())
                    .unwrap_or_default();
            }
        });
    }

    pub fn unarchive_session(&mut self, id: String, cx: &mut Context<Self>) {
        self.call(
            "unarchive_session",
            json!({ "id": id, "pinned": false }),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    root.sessions = value
                        .as_array()
                        .map(|sessions| {
                            sessions.iter().filter_map(SessionRow::from_value).collect()
                        })
                        .unwrap_or_default();
                    root.load_archived(cx);
                    cx.notify();
                }
                Err(error) => root.toast(error, true, cx),
            },
        );
    }

    pub fn set_project_pinned(&mut self, id: String, pinned: bool, cx: &mut Context<Self>) {
        self.overlay = None;
        self.call(
            "set_project_pinned",
            json!({ "id": id, "pinned": pinned }),
            cx,
            |root, result, cx| match result {
                Ok(value) => {
                    root.projects = value["projects"]
                        .as_array()
                        .map(|projects| {
                            projects.iter().filter_map(ProjectRow::from_value).collect()
                        })
                        .unwrap_or_default();
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
            .filter(|session| session.project == project)
            .collect();
        sessions.sort_by(|left, right| {
            right
                .pinned
                .cmp(&left.pinned)
                .then(right.modified_at.cmp(&left.modified_at))
        });
        sessions
    }

    pub fn open_overlay(&mut self, overlay: Overlay, cx: &mut Context<Self>) {
        match &overlay {
            Overlay::Models => self.load_models(cx),
            Overlay::Settings => self.load_project_commands(cx),
            Overlay::Archived => self.load_archived(cx),
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
        let project = self.current_project()?;
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
            .key_context("Root")
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
            .on_action(cx.listener(|this, _: &OpenSettings, _window, cx| {
                this.open_overlay(Overlay::Settings, cx)
            }))
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
            .on_action(cx.listener(|this, _: &Back, _window, cx| {
                if let Some(previous) = this.history.pop() {
                    this.forward.push(this.active);
                    this.active = previous.min(this.chats.len().saturating_sub(1));
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &Forward, _window, cx| {
                if let Some(next) = this.forward.pop() {
                    this.history.push(this.active);
                    this.active = next.min(this.chats.len().saturating_sub(1));
                    cx.notify();
                }
            }))
            .on_action(cx.listener(|this, _: &ShowShortcuts, _window, cx| {
                this.open_overlay(Overlay::Shortcuts, cx)
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
        if self.sidebar_visible {
            body = body.child(views::sidebar(self, &theme, scale, cx));
            body = body.child(views::sidebar_resizer(self, &theme, cx));
        }
        body = body.child(views::main(self, &theme, scale, window, cx));
        shell = shell.child(body);
        shell = shell.child(overlays::layer(self, &theme, scale, window, cx));
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
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(gpui::TitlebarOptions {
            title: Some(SharedString::from("Oxide Beta")),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        ..Default::default()
    };
    let _ = cx.open_window(options, move |window, cx| {
        cx.new(|cx| Root::new(engine, packets, window, cx))
    });
}

impl Root {
    pub fn handle_sidebar_drag(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some(origin) = self.resizing {
            let width = (event.position.x - px(origin)).max(px(180.0));
            self.sidebar_width = f32::from(width);
            cx.notify();
        }
    }

    pub fn begin_sidebar_drag(&mut self, event: &MouseDownEvent, cx: &mut Context<Self>) {
        self.resizing = Some(f32::from(event.position.x));
        cx.notify();
    }

    pub fn end_sidebar_drag(&mut self, cx: &mut Context<Self>) {
        self.resizing = None;
        cx.notify();
    }
}

actions!(composer, [Send, Stop]);

/// The four sides of a card, kept in one place so every panel in the window
/// draws the same border and radius.
pub fn card(theme: &Theme) -> gpui::Div {
    div()
        .bg(theme.panel())
        .border_1()
        .border_color(theme.border())
        .rounded(px(10.0))
}

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
            theme.accent()
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
            theme.accent()
        } else {
            theme.panel_3()
        };
        let mut row = div()
            .id(id)
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .h(px(30.0))
            .px_3()
            .rounded(px(8.0))
            .bg(background)
            .text_color(color)
            .hover(move |style| style.bg(hover))
            .child(self.label);
        if let Some(width) = self.width {
            row = row.w(px(width));
        }
        row.on_click(cx.listener(move |root, _event, window, cx| on_click(root, window, cx)))
    }
}
