//! The window: a sidebar of projects and threads, the transcript of the open
//! thread, and the composer under it.
//!
//! Everything it paints comes from `state`, `transcript` and `runner`; this
//! module is the drawing and the keyboard. A turn's events arrive on the channel
//! the runner writes to and are applied here, so a run that streams for minutes
//! never blocks the window.

use crate::editor::{caret_color, placeholder_color, selection_color, Editor, EditorElement};
use crate::runner::{RunControls, Runner, TurnRequest, UiEvent};
use crate::state::{self, Completion, Phase, ProjectRow, QuestionForm, Thread, ThreadRow};
use crate::theme::Theme;
use crate::transcript::{self, Item};
use gpui::{
    div, prelude::*, px, AnyElement, App, Context, CursorStyle, Entity, FontWeight, KeyDownEvent,
    MouseButton, PathPromptOptions, Render, Rgba, ScrollHandle, SharedString, Window,
};
use oxide_core::approval::Decision;

use oxide_core::at;
use oxide_core::commands::CommandEntry;
use oxide_core::llm::{Message, MessageContent};
use oxide_core::session::SessionLog;
use oxide_core::updates::Check;
use oxide_desktop::at::PathCache;
use oxide_desktop::manager::{self, DesktopManager, ProjectTrust};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedReceiver;

/// How many rows the `@` and `/` lists offer, and how many lines a tool's card
/// previews before it is unfolded.
const MAX_ROWS: usize = 8;
const PREVIEW_LINES: usize = 10;

/// The project the window has open, as the composer and the footer read it.
struct OpenProject {
    path: PathBuf,
    name: String,
    trusted: bool,
    awaiting_trust: bool,
    resources: Vec<String>,
    provider: String,
    model: String,
    window: u64,
    subscription: bool,
    key_problem: Option<String>,
}

/// The app's update dialog: what a check found, and how an install is going.
struct UpdateState {
    check: Option<Check>,
    status: String,
}

struct PendingApproval {
    id: u64,
    tool: String,
    detail: String,
}

/// A question being asked: the request it answers, and the form on screen.
struct PendingQuestion {
    id: u64,
    form: QuestionForm,
}

pub struct Root {
    manager: DesktopManager,
    projects: Vec<ProjectRow>,
    threads: Vec<ThreadRow>,
    project: Option<OpenProject>,
    thread: Thread,
    editor: Entity<Editor>,
    answer_editor: Entity<Editor>,
    runner: Runner,
    scroll: ScrollHandle,
    theme: Theme,
    paths: Arc<PathCache>,
    commands: Vec<CommandEntry>,
    completion: Completion,
    completion_key: (String, usize),
    /// The transcript item whose output is unfolded, by index.
    expanded: Option<usize>,
    approval: Option<PendingApproval>,
    question: Option<PendingQuestion>,
    update: Option<UpdateState>,
    controls: Option<RunControls>,
    run: u64,
    started: Option<Instant>,
    status: String,
}

impl Root {
    pub fn new(
        runner: Runner,
        mut events: UnboundedReceiver<UiEvent>,
        manager: DesktopManager,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = cx.new(|cx| Editor::new("Message Oxide…", cx));
        let answer_editor = cx.new(|cx| Editor::new("Your answer", cx));
        let theme = Theme::load(None, &Theme::configured_name());
        let projects = manager
            .overview()
            .map(|projects| state::project_rows(&projects))
            .unwrap_or_default();
        let mut root = Self {
            projects,
            threads: Vec::new(),
            project: None,
            thread: Thread::default(),
            editor,
            answer_editor,
            runner,
            scroll: ScrollHandle::new(),
            theme,
            paths: Arc::new(PathCache::default()),
            commands: Vec::new(),
            completion: Completion::None,
            completion_key: (String::new(), 0),
            expanded: None,
            approval: None,
            question: None,
            update: None,
            controls: None,
            run: 0,
            started: None,
            status: String::new(),
            manager,
        };
        let first = root.projects.first().map(|project| project.id.clone());
        if let Some(id) = first {
            root.open_project(&id);
        }
        let composer = root.editor.read(cx).focus_handle();
        window.focus(&composer);
        // Everything the runner says arrives here, on the window's own task, so
        // a turn is painted as it streams and the window never waits on one.
        cx.spawn(async move |this, cx| {
            while let Some(event) = events.recv().await {
                if this
                    .update(cx, |root, cx| {
                        root.handle(event, cx);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        // A running turn's elapsed time and spinner are painted from the clock,
        // so the window repaints once a second while one is going.
        cx.spawn(async move |this, cx| loop {
            let Ok(executor) = cx.update(|cx| cx.background_executor().clone()) else {
                break;
            };
            executor.timer(Duration::from_millis(500)).await;
            if this
                .update(cx, |root, cx| {
                    if root.thread.running {
                        cx.notify();
                    }
                })
                .is_err()
            {
                break;
            }
        })
        .detach();
        root
    }

    // ---------- the window's lists ----------

    fn reload_projects(&mut self) {
        if let Ok(projects) = self.manager.overview() {
            self.projects = state::project_rows(&projects);
        }
    }

    fn reload_threads(&mut self) {
        let Some(project) = self.project.as_ref() else {
            self.threads.clear();
            return;
        };
        let path = project.path.clone();
        let sessions = self.manager.sessions_for(&path).unwrap_or_default();
        let views = self.manager.session_views(sessions).unwrap_or_default();
        self.threads = state::thread_rows(&views);
    }

    fn open_project(&mut self, id: &str) {
        let Some(row) = self.projects.iter().find(|row| row.id == id).cloned() else {
            return;
        };
        let _ = self.manager.touch(id);
        let config = manager::load_project_config(&row.path);
        let (trust, key_problem, provider, model, context, subscription) = match &config {
            Ok(config) => {
                let trust = manager::project_trust(config, &row.path);
                let key_problem = config
                    .require_api_key()
                    .err()
                    .map(|error| format!("{error:#}"));
                (
                    trust,
                    key_problem,
                    config.provider.clone(),
                    config.model.clone(),
                    config.context_window(),
                    config.is_subscription(),
                )
            }
            Err(error) => (
                ProjectTrust {
                    required: false,
                    trusted: false,
                    awaiting: false,
                    default: "ask".to_string(),
                    resources: Vec::new(),
                },
                Some(format!("{error:#}")),
                String::new(),
                String::new(),
                128_000,
                false,
            ),
        };
        let trusted = trust.trusted;
        self.commands = oxide_core::commands::palette_with(&row.path, trusted)
            .into_iter()
            .filter(|entry| entry.front_ends.contains(&"desktop"))
            .collect();
        self.project = Some(OpenProject {
            path: row.path.clone(),
            name: row.name.clone(),
            trusted,
            awaiting_trust: trust.awaiting,
            resources: trust.resources.clone(),
            provider,
            model,
            window: context,
            subscription,
            key_problem,
        });
        self.thread = Thread::new(row.path.clone());
        self.controls = None;
        self.approval = None;
        self.question = None;
        self.completion = Completion::None;
        self.expanded = None;
        self.status.clear();
        self.paths.clear();
        self.theme = Theme::load(Some(&row.path), &Theme::configured_name());
        self.reload_threads();
        self.scroll.set_offset(gpui::point(px(0.), px(0.)));
    }

    fn new_chat(&mut self, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            self.status = "Choose a project first.".to_string();
            return;
        };
        self.thread = Thread::new(project.path.clone());
        self.controls = None;
        self.approval = None;
        self.question = None;
        self.completion = Completion::None;
        self.expanded = None;
        self.status.clear();
        self.editor.update(cx, |editor, cx| editor.clear(cx));
        self.scroll.set_offset(gpui::point(px(0.), px(0.)));
    }

    fn open_thread(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(project) = self.project.as_ref() else {
            return;
        };
        let cwd = project.path.clone();
        match SessionLog::open_ref(&cwd, id) {
            Ok(log) => {
                let messages = log.messages().unwrap_or_default();
                let totals = log.usage_totals();
                let title = log
                    .summary()
                    .ok()
                    .map(|summary| {
                        summary
                            .name
                            .filter(|name| !name.trim().is_empty())
                            .unwrap_or(summary.preview)
                    })
                    .unwrap_or_default();
                let mut thread = Thread::new(cwd);
                thread.session_id = Some(id.to_string());
                thread.title = title;
                thread.items = state::replay(&messages);
                thread.usage = state::Usage {
                    input: totals.input,
                    output: totals.output,
                    cache_read: totals.cache_read,
                    cache_write: totals.cache_write,
                    cost: totals.cost,
                    cache_hit: totals.cache_hit_rate,
                    context: Some(log.context_tokens()).filter(|tokens| *tokens > 0),
                };
                self.thread = thread;
                self.controls = None;
                self.approval = None;
                self.question = None;
                self.expanded = None;
                self.status.clear();
            }
            Err(error) => self.status = format!("{error:#}"),
        }
        self.scroll.scroll_to_bottom();
        let _ = cx;
    }

    fn add_project(&mut self, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some(SharedString::from("Open a project")),
        });
        let this = cx.entity();
        cx.spawn(async move |_, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                if let Some(path) = paths.into_iter().next() {
                    let _ = this.update(cx, |root, cx| {
                        match root.manager.add_project(&path) {
                            Ok(project) => {
                                root.reload_projects();
                                root.open_project(&project.id);
                            }
                            Err(error) => root.status = format!("{error:#}"),
                        }
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }

    // ---------- the run ----------

    fn send(&mut self, cx: &mut Context<Self>) {
        let project = if self.thread.project.as_os_str().is_empty() {
            self.project.as_ref().map(|project| project.path.clone())
        } else {
            Some(self.thread.project.clone())
        };
        let Some(project) = project else {
            self.status = "Choose a project first.".to_string();
            return;
        };
        let prompt = self.editor.read(cx).content().trim_end().to_string();
        if prompt.trim().is_empty() {
            return;
        }
        let command = prompt.trim();
        if command == "/new" {
            self.new_chat(cx);
            return;
        }
        if command == "/permissions" || command == "/permissions clear" {
            self.runner.permissions(project.clone());
            if command.ends_with("clear") {
                self.runner.clear_permissions(project.clone());
            }
            self.editor.update(cx, |editor, cx| editor.clear(cx));
            self.completion = Completion::None;
            return;
        }
        if let Some(name) = command.strip_prefix("/theme ") {
            let name = name.trim().to_string();
            let cwd = Some(project.as_path());
            self.theme = Theme::load(cwd, &name);
            let _ = oxide_core::config::Config::set_theme_at(
                &oxide_core::config::Config::config_path(),
                &self.theme.name.clone(),
            );
            self.editor.update(cx, |editor, cx| editor.clear(cx));
            self.completion = Completion::None;
            cx.notify();
            return;
        }
        if self.thread.running {
            self.queue(prompt, cx);
            return;
        }
        self.thread.push_user(prompt.clone());
        self.thread.running = true;
        self.thread.phase = Phase::Starting;
        self.thread.error = None;
        self.thread.changes = None;
        self.thread.queued.clear();
        self.status.clear();
        self.expanded = None;
        self.editor.update(cx, |editor, cx| editor.clear(cx));
        self.completion = Completion::None;
        self.run += 1;
        self.started = Some(Instant::now());
        let request = TurnRequest {
            run: self.run,
            project,
            session: self.thread.session_id.clone(),
            prompt,
            inline: Vec::new(),
        };
        self.controls = Some(self.runner.start(request));
        self.scroll.scroll_to_bottom();
    }

    /// A message typed while a turn runs waits on the follow-up queue, the way
    /// the Electron window's default delivery does.
    fn queue(&mut self, prompt: String, cx: &mut Context<Self>) {
        let Some(controls) = self.controls.clone() else {
            return;
        };
        let message = Message {
            role: "user".to_string(),
            content: Some(MessageContent::Text(prompt.clone())),
            tool_calls: None,
            tool_call_id: None,
            thinking: None,
        };
        if controls.follow_ups.push_labeled(message, prompt.clone()) {
            self.thread.queued.push(prompt);
            self.editor.update(cx, |editor, cx| editor.clear(cx));
            self.completion = Completion::None;
        } else {
            self.status = "The turn is finishing; send that again.".to_string();
        }
    }

    fn stop(&mut self) {
        if let Some(controls) = self.controls.as_ref() {
            controls.cancel.cancel();
            self.thread.phase = Phase::Stopped;
            self.thread.running = false;
        }
    }

    fn handle(&mut self, event: UiEvent, cx: &mut Context<Self>) {
        match event {
            UiEvent::Started {
                run,
                session_id,
                title,
            } => {
                if run == self.run {
                    self.thread.session_id = session_id;
                    if !title.trim().is_empty() {
                        self.thread.title = title;
                    }
                    self.thread.phase = Phase::Thinking;
                }
            }
            UiEvent::Agent { run, event } => {
                if run == self.run {
                    self.thread.apply(&event);
                }
            }
            UiEvent::Ended {
                run,
                session_id,
                error,
                stopped,
                changes,
            } => {
                if run != self.run {
                    return;
                }
                if session_id.is_some() {
                    self.thread.session_id = session_id;
                }
                if error.is_some() {
                    self.thread.error = error;
                }
                if stopped {
                    self.thread.phase = Phase::Stopped;
                }
                self.thread.finish();
                self.thread.changes = changes;
                self.controls = None;
                self.started = None;
                self.paths.clear();
                self.reload_projects();
                self.reload_threads();
                self.scroll.scroll_to_bottom();
            }
            UiEvent::Approval { id, tool, detail } => {
                self.approval = Some(PendingApproval { id, tool, detail });
            }
            UiEvent::Question { id, questions } => {
                self.question = Some(PendingQuestion {
                    id,
                    form: QuestionForm::new(questions),
                });
                self.answer_editor.update(cx, |editor, cx| editor.clear(cx));
            }
            UiEvent::QuestionClosed { id } => {
                if self
                    .question
                    .as_ref()
                    .is_some_and(|question| question.id == id)
                {
                    self.question = None;
                }
            }
            UiEvent::Permissions { rules } => {
                let text = if rules.is_empty() {
                    "No tools are allowed without asking in this project.".to_string()
                } else {
                    format!(
                        "Allowed without asking in this project: {}\n(`/permissions clear` forgets them)",
                        rules.join(", ")
                    )
                };
                self.thread.items.push(Item::Info(text));
            }
            UiEvent::Update { result } => match result {
                Ok(check) => {
                    let status = if check.update_available {
                        String::new()
                    } else {
                        format!("Oxide {} is the newest release.", check.current)
                    };
                    self.update = Some(UpdateState {
                        check: Some(*check),
                        status,
                    });
                }
                Err(error) => {
                    self.thread.items.push(Item::Error(error));
                }
            },
            UiEvent::UpdateInstalled { result } => {
                let status = match result {
                    Ok(path) => format!(
                        "Installed. Quit and open the app again to run it ({}).",
                        path.display()
                    ),
                    Err(error) => error,
                };
                if let Some(update) = self.update.as_mut() {
                    update.status = status;
                } else {
                    self.thread.items.push(Item::Info(status));
                }
            }
            UiEvent::PermissionsCleared { error } => match error {
                Some(error) => self.thread.items.push(Item::Error(error)),
                None => self.thread.items.push(Item::Info(
                    "Cleared this project's approval rules.".to_string(),
                )),
            },
        }
    }

    // ---------- the composer's completion ----------

    /// Recomputes the rows above the composer when the text or the caret moved.
    /// The line the caret sits in decides which list is offered: an `@`
    /// reference completes the project's paths, a `/` the thread starts with
    /// completes the catalog.
    fn refresh_completion(&mut self, cx: &mut App) {
        let (text, cursor) = {
            let editor = self.editor.read(cx);
            (editor.content(), editor.cursor())
        };
        let key = (text.clone(), cursor);
        if self.completion_key == key {
            return;
        }
        self.completion_key = key;
        let Some(project) = self.project.as_ref().map(|project| project.path.clone()) else {
            self.completion = Completion::None;
            return;
        };
        let at = state::at_completion(&self.paths.paths(&project), &text, cursor, MAX_ROWS);
        self.completion = if at.is_open() {
            at
        } else {
            state::slash_completion(&text, &self.commands, MAX_ROWS)
        };
    }

    fn take_completion(&mut self, cx: &mut Context<Self>) -> bool {
        let Some((range, insert)) = self.completion.take() else {
            return false;
        };
        self.editor
            .update(cx, |editor, cx| editor.replace_range(range, &insert, cx));
        self.completion = Completion::None;
        self.completion_key = (String::new(), 0);
        true
    }

    // ---------- the keyboard ----------

    fn on_composer_key(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let key = keystroke.key.as_str();
        let shift = keystroke.modifiers.shift;
        let command = keystroke.modifiers.platform || keystroke.modifiers.control;

        if self.completion.is_open() {
            match key {
                "up" => {
                    self.completion.move_by(-1);
                    cx.notify();
                    return;
                }
                "down" => {
                    self.completion.move_by(1);
                    cx.notify();
                    return;
                }
                "tab" => {
                    self.take_completion(cx);
                    return;
                }
                "enter" if !shift => {
                    if self.take_completion(cx) {
                        return;
                    }
                }
                "escape" => {
                    self.completion = Completion::None;
                    self.completion_key = (String::new(), 0);
                    cx.notify();
                    return;
                }
                _ => {}
            }
        }

        if command {
            match key {
                "c" => self.editor.update(cx, |editor, cx| editor.copy(cx)),
                "x" => self.editor.update(cx, |editor, cx| editor.cut(cx)),
                "v" => self.editor.update(cx, |editor, cx| editor.paste(cx)),
                "a" => self.editor.update(cx, |editor, cx| editor.select_all(cx)),
                "n" => self.new_chat(cx),
                "enter" => self.send(cx),
                _ => {}
            }
            return;
        }

        match key {
            "enter" if shift => self.editor.update(cx, |editor, cx| editor.insert("\n", cx)),
            "enter" => self.send(cx),
            "backspace" => self
                .editor
                .update(cx, |editor, cx| editor.delete_backward(cx)),
            "delete" => self
                .editor
                .update(cx, |editor, cx| editor.delete_forward(cx)),
            "left" => self
                .editor
                .update(cx, |editor, cx| editor.move_horizontal(false, shift, cx)),
            "right" => self
                .editor
                .update(cx, |editor, cx| editor.move_horizontal(true, shift, cx)),
            "up" => self
                .editor
                .update(cx, |editor, cx| editor.move_vertical(false, shift, cx)),
            "down" => self
                .editor
                .update(cx, |editor, cx| editor.move_vertical(true, shift, cx)),
            "home" => self
                .editor
                .update(cx, |editor, cx| editor.move_line_edge(false, shift, cx)),
            "end" => self
                .editor
                .update(cx, |editor, cx| editor.move_line_edge(true, shift, cx)),
            "escape" => self.stop(),
            "tab" => self.editor.update(cx, |editor, cx| editor.insert("  ", cx)),
            _ => {}
        }
        let _ = window;
    }

    // ---------- the pieces of the window ----------

    fn sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let sidebar = self.theme.sidebar();
        let border = self.theme.border();
        let text = self.theme.text();
        let dim = self.theme.dim();
        let accent = self.theme.accent();
        let panel_2 = self.theme.panel_2();
        let hover = self.theme.panel_3();

        let mut list: Vec<AnyElement> = Vec::new();
        for (index, project) in self.projects.clone().into_iter().enumerate() {
            let selected = self
                .project
                .as_ref()
                .is_some_and(|open| open.path == project.path);
            let id = project.id.clone();
            list.push(
                div()
                    .id(("project", index))
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id(("project-row", index))
                            .flex()
                            .items_center()
                            .justify_between()
                            .px_3()
                            .py_1p5()
                            .rounded_md()
                            .cursor_pointer()
                            .when(selected, |style| style.bg(panel_2))
                            .hover(move |style| style.bg(hover))
                            .text_color(if selected { text } else { dim })
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.open_project(&id);
                                let composer = this.editor.read(cx).focus_handle();
                                window.focus(&composer);
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(project.name.clone()),
                            )
                            .child(
                                div()
                                    .text_color(dim)
                                    .text_size(px(11.))
                                    .child(project.sessions.to_string()),
                            ),
                    )
                    .children(if selected {
                        self.thread_rows(index, cx)
                    } else {
                        Vec::new()
                    })
                    .into_any_element(),
            );
        }

        div()
            .flex()
            .flex_col()
            .w(px(232.))
            .h_full()
            .flex_none()
            .bg(sidebar)
            .border_r_1()
            .border_color(border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .py_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(text)
                                    .font_weight(FontWeight::BOLD)
                                    .text_size(px(14.))
                                    .child("Oxide"),
                            )
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded(px(4.))
                                    .bg(panel_2)
                                    .text_color(accent)
                                    .text_size(px(10.))
                                    .child("GPUI"),
                            ),
                    ),
            )
            .child(
                div()
                    .id("new-chat")
                    .mx_3()
                    .mb_2()
                    .px_3()
                    .py_1p5()
                    .rounded_md()
                    .bg(panel_2)
                    .text_color(text)
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.new_chat(cx);
                        cx.notify();
                    }))
                    .child("+  New chat"),
            )
            .child(
                div()
                    .id("projects")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h(px(0.))
                    .gap_0p5()
                    .px_2()
                    .overflow_y_scroll()
                    .children(list),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        div()
                            .id("add-project")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_color(dim)
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.add_project(cx);
                            }))
                            .child("+  Add project"),
                    )
                    .child(
                        div()
                            .id("check-updates")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_color(dim)
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.runner.check_updates();
                                this.update = Some(UpdateState {
                                    check: None,
                                    status: "Checking for updates…".to_string(),
                                });
                                cx.notify();
                            }))
                            .child("Check for updates"),
                    )
                    .child(
                        div()
                            .id("theme")
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .text_color(dim)
                            .cursor_pointer()
                            .hover(move |style| style.bg(hover))
                            .on_click(cx.listener(|this, _, _, cx| {
                                let name = this.theme.toggled_name().to_string();
                                let project =
                                    this.project.as_ref().map(|project| project.path.clone());
                                this.theme = Theme::load(project.as_deref(), &name);
                                cx.notify();
                            }))
                            .child(format!("Theme: {}", self.theme.name)),
                    )
                    .child(
                        div()
                            .px_2()
                            .text_size(px(11.))
                            .text_color(self.theme.faint())
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(match self.project.as_ref() {
                                Some(project) => {
                                    format!("{} · {}", project.provider, project.model)
                                }
                                None => "No project open".to_string(),
                            }),
                    ),
            )
            .into_any_element()
    }

    fn thread_rows(&mut self, project_index: usize, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let dim = self.theme.dim();
        let text = self.theme.text();
        let hover = self.theme.panel_3();
        let faint = self.theme.faint();
        let selected = self.thread.session_id.clone();
        let mut rows = Vec::new();
        for (index, thread) in self.threads.clone().into_iter().enumerate() {
            let active = selected.as_deref() == Some(thread.id.as_str());
            let id = thread.id.clone();
            rows.push(
                div()
                    .id(SharedString::from(format!(
                        "thread-{project_index}-{index}"
                    )))
                    .pl(px(18.))
                    .pr_3()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(move |style| style.bg(hover))
                    .text_color(if active { text } else { dim })
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .when(active, |style| style.bg(hover))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.open_thread(&id, cx);
                        cx.notify();
                    }))
                    .child(if thread.title.trim().is_empty() {
                        "Untitled".to_string()
                    } else {
                        thread.title.clone()
                    })
                    .into_any_element(),
            );
        }
        if rows.is_empty() {
            rows.push(
                div()
                    .pl(px(18.))
                    .py_1()
                    .text_size(px(11.))
                    .text_color(faint)
                    .child("No chats")
                    .into_any_element(),
            );
        }
        rows
    }

    fn item_view(
        &mut self,
        index: usize,
        item_colors: &ItemColors,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.expanded == Some(index);
        let colors = *item_colors;
        match self.thread.items.get(index) {
            None => div().into_any_element(),
            Some(Item::User(text)) => div()
                .flex()
                .gap_2()
                .child(div().flex_none().text_color(colors.user).child("❯"))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(colors.user)
                        .child(text.clone()),
                )
                .into_any_element(),
            Some(Item::Assistant { text, .. }) => div()
                .flex()
                .gap_2()
                .child(div().flex_none().text_color(colors.accent).child("◆"))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.))
                        .text_color(colors.assistant)
                        .child(text.clone()),
                )
                .into_any_element(),
            Some(Item::Thinking { text, millis }) => div()
                .flex()
                .flex_col()
                .gap_1()
                .pl(px(18.))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(colors.dim)
                        .child(match millis {
                            Some(millis) => {
                                format!("✦ Thought for {}", transcript::format_duration(*millis))
                            }
                            None => "✦ Thinking".to_string(),
                        }),
                )
                .child(
                    div()
                        .italic()
                        .text_color(colors.thinking)
                        .child(text.clone()),
                )
                .into_any_element(),
            Some(Item::Tool(tool)) => {
                let title = transcript::tool_label(&tool.name);
                let summary = transcript::tool_summary(&tool.name, &tool.args);
                let body = transcript::preview(
                    &tool.name,
                    &tool.output,
                    if expanded { usize::MAX } else { PREVIEW_LINES },
                );
                let status = if tool.running {
                    colors.dim
                } else if tool.is_error {
                    colors.error
                } else {
                    colors.success
                };
                let background = if tool.running {
                    colors.pending
                } else if tool.is_error {
                    colors.error_bg
                } else {
                    colors.success_bg
                };
                let mut card = div()
                    .id(("tool", index))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded_md()
                    .bg(background)
                    .border_l_2()
                    .border_color(status)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.expanded = if this.expanded == Some(index) {
                            None
                        } else {
                            Some(index)
                        };
                        cx.notify();
                    }))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_color(if tool.running || tool.is_error {
                                        status
                                    } else {
                                        colors.tool
                                    })
                                    .font_weight(FontWeight::BOLD)
                                    .child(title),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_color(colors.dim)
                                    .child(summary),
                            )
                            .child(div().text_size(px(11.)).text_color(colors.dim).child(
                                if tool.running {
                                    tool.verb().to_string()
                                } else {
                                    format!(
                                        "{} · {}",
                                        tool.verb(),
                                        transcript::format_duration(tool.millis)
                                    )
                                },
                            )),
                    );
                if !body.is_empty() {
                    card = card.child(
                        div()
                            .font_family("Menlo")
                            .text_size(px(11.))
                            .text_color(if tool.is_error {
                                colors.error
                            } else {
                                colors.dim
                            })
                            .child(body),
                    );
                }
                card.into_any_element()
            }
            Some(Item::Info(text)) => div()
                .pl(px(18.))
                .text_size(px(11.))
                .text_color(colors.dim)
                .child(text.clone())
                .into_any_element(),
            Some(Item::Error(text)) => div()
                .pl(px(18.))
                .text_color(colors.error)
                .child(text.clone())
                .into_any_element(),
        }
    }

    fn changes_card(&self, colors: &ItemColors) -> Option<AnyElement> {
        let changes = self.thread.changes.as_ref()?;
        if changes.files.is_empty() {
            return None;
        }
        let rows: Vec<AnyElement> =
            changes
                .files
                .iter()
                .map(|file| {
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.success)
                                .child(state::change_letter(file.status).to_string()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_color(colors.text)
                                .child(file.path.clone()),
                        )
                        .child(div().text_size(px(11.)).text_color(colors.dim).child(
                            if file.binary {
                                "binary".to_string()
                            } else {
                                format!("+{} −{}", file.added, file.removed)
                            },
                        ))
                        .into_any_element()
                })
                .collect();
        Some(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded_md()
                .bg(colors.pending)
                .border_1()
                .border_color(colors.border)
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(div().text_color(colors.text).child(format!(
                            "Changed {} file{}",
                            changes.files.len(),
                            if changes.files.len() == 1 { "" } else { "s" }
                        )))
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(colors.dim)
                                .child(format!("+{} −{}", changes.added, changes.removed)),
                        ),
                )
                .children(rows)
                .into_any_element(),
        )
    }

    fn transcript(&mut self, colors: &ItemColors, cx: &mut Context<Self>) -> AnyElement {
        let mut children: Vec<AnyElement> = Vec::new();
        if self
            .project
            .as_ref()
            .is_some_and(|project| project.awaiting_trust)
        {
            children.push(self.trust_card(colors, cx));
        }
        if self.thread.items.is_empty() {
            children.push(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .pt_8()
                    .child(div().text_color(colors.text).text_size(px(18.)).child(
                        match self.project.as_ref() {
                            Some(project) => format!("What should Oxide do in {}?", project.name),
                            None => "Open a project to start.".to_string(),
                        },
                    ))
                    .child(div().text_color(colors.dim).child(
                        "Ask for a change, a fix or an explanation. `@path` attaches a file; \
                                 `/` lists the commands.",
                    ))
                    .into_any_element(),
            );
        }
        for index in 0..self.thread.items.len() {
            children.push(self.item_view(index, colors, cx));
        }
        if let Some(card) = self.changes_card(colors) {
            children.push(card);
        }

        div()
            .id("transcript")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&self.scroll)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .max_w(px(780.))
                    .mx_auto()
                    .w_full()
                    .px_6()
                    .py_4()
                    .children(children),
            )
            .into_any_element()
    }

    fn trust_card(&mut self, colors: &ItemColors, cx: &mut Context<Self>) -> AnyElement {
        let resources = self
            .project
            .as_ref()
            .map(|project| project.resources.clone())
            .unwrap_or_default();
        let path = self.project.as_ref().map(|project| project.path.clone());
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded_md()
            .border_1()
            .border_color(colors.border)
            .bg(colors.pending)
            .child(
                div()
                    .text_color(colors.text)
                    .font_weight(FontWeight::BOLD)
                    .child("Trust this project?"),
            )
            .child(div().text_color(colors.dim).child(format!(
                "It configures {} for the agent. Untrusted, they are not loaded.",
                if resources.is_empty() {
                    "project resources".to_string()
                } else {
                    resources.join(", ")
                }
            )))
            .child(
                div()
                    .flex()
                    .gap_2()
                    .child(
                        div()
                            .id("trust-yes")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(colors.accent)
                            .text_color(colors.background)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, window, cx| {
                                if let Some(project) = this.project.as_ref().map(|p| p.path.clone())
                                {
                                    let _ = manager::set_project_trust(&project, true);
                                    let id = this
                                        .projects
                                        .iter()
                                        .find(|row| row.path == project)
                                        .map(|row| row.id.clone());
                                    if let Some(id) = id {
                                        this.open_project(&id);
                                    }
                                    let composer = this.editor.read(cx).focus_handle();
                                    window.focus(&composer);
                                    cx.notify();
                                }
                            }))
                            .child("Trust"),
                    )
                    .child(
                        div()
                            .id("trust-no")
                            .px_3()
                            .py_1()
                            .rounded_md()
                            .bg(colors.pending)
                            .text_color(colors.dim)
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(project) = this.project.as_mut() {
                                    project.awaiting_trust = false;
                                }
                                cx.notify();
                            }))
                            .child("Not now"),
                    ),
            )
            .child(
                div().text_size(px(11.)).text_color(colors.faint).child(
                    path.map(|path| path.display().to_string())
                        .unwrap_or_default(),
                ),
            )
            .into_any_element()
    }

    fn completion_rows(
        &mut self,
        colors: &ItemColors,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let rows = match &self.completion {
            Completion::None => return None,
            _ => self.completion.len(),
        };
        if rows == 0 {
            return None;
        }
        let mut children: Vec<AnyElement> = Vec::new();
        match &self.completion {
            Completion::At { rows, selected, .. } => {
                let rows = rows.clone();
                let selected = *selected;
                for (index, row) in rows.into_iter().enumerate() {
                    let label = row.label.clone();
                    let insert = row.insert.clone();
                    children.push(
                        div()
                            .id(("at-row", index))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .when(index == selected, |style| style.bg(colors.pending))
                            .hover(move |style| style.bg(colors.pending))
                            .cursor_pointer()
                            .text_color(if index == selected {
                                colors.text
                            } else {
                                colors.dim
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.editor.update(cx, |editor, cx| {
                                    let range = this.completion.range().unwrap_or(0..0);
                                    editor.replace_range(range, &insert, cx);
                                });
                                this.completion = Completion::None;
                                cx.notify();
                            }))
                            .child(if row.kind == at::AtKind::Folder {
                                "▸"
                            } else {
                                "·"
                            })
                            .child(label)
                            .into_any_element(),
                    );
                }
            }
            Completion::Slash { rows, selected, .. } => {
                let rows = rows.clone();
                let selected = *selected;
                for (index, name) in rows.into_iter().enumerate() {
                    let query = name.clone();
                    let description = self
                        .commands
                        .iter()
                        .find(|entry| entry.name == name)
                        .map(|entry| entry.description.clone())
                        .unwrap_or_default();
                    children.push(
                        div()
                            .id(("slash-row", index))
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded_md()
                            .when(index == selected, |style| style.bg(colors.pending))
                            .hover(move |style| style.bg(colors.pending))
                            .cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let range = this.completion.range().unwrap_or(0..0);
                                this.editor.update(cx, |editor, cx| {
                                    editor.replace_range(range, &format!("/{query} "), cx)
                                });
                                this.completion = Completion::None;
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .text_color(if index == selected {
                                        colors.accent
                                    } else {
                                        colors.text
                                    })
                                    .child(format!("/{name}")),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(11.))
                                    .text_color(colors.dim)
                                    .child(description),
                            )
                            .into_any_element(),
                    );
                }
            }
            Completion::None => {}
        }
        Some(
            div()
                .flex()
                .flex_col()
                .gap_0p5()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(colors.border)
                .bg(colors.panel)
                .children(children)
                .into_any_element(),
        )
    }

    fn composer(&mut self, colors: &ItemColors, cx: &mut Context<Self>) -> AnyElement {
        let focus = self.editor.read(cx).focus_handle();
        let rows = self.completion_rows(colors, cx);
        let mut queued: Vec<AnyElement> = Vec::new();
        for (index, message) in self.thread.queued.clone().into_iter().enumerate() {
            queued.push(
                div()
                    .id(("queued", index))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(colors.pending)
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.dim)
                            .child("Queued"),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_color(colors.dim)
                            .child(message),
                    )
                    .into_any_element(),
            );
        }

        let running = self.thread.running;
        let has_text = !self.editor.read(cx).is_empty();
        let action = if running && !has_text {
            div()
                .id("stop")
                .px_3()
                .py_1()
                .rounded_md()
                .bg(colors.error)
                .text_color(colors.background)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.stop();
                    cx.notify();
                }))
                .child("Stop")
        } else {
            div()
                .id("send")
                .px_3()
                .py_1()
                .rounded_md()
                .bg(colors.accent)
                .text_color(colors.background)
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.send(cx);
                    cx.notify();
                }))
                .child("Send")
        };

        let editor = div()
            .id("composer-editor")
            .w_full()
            .min_h(px(24.))
            .cursor(CursorStyle::IBeam)
            .track_focus(&focus)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                this.on_composer_key(event, window, cx);
                cx.notify();
            }))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    let handle = this.editor.read(cx).focus_handle();
                    window.focus(&handle);
                    let position = event.position;
                    this.editor
                        .update(cx, |editor, cx| editor.begin_selection(position, cx));
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                let position = event.position;
                this.editor
                    .update(cx, |editor, cx| editor.extend_selection(position, cx));
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.editor
                        .update(cx, |editor, cx| editor.end_selection(cx));
                }),
            )
            .child(EditorElement {
                editor: self.editor.clone(),
                selection: selection_color(colors.accent),
                caret: caret_color(colors.accent),
                placeholder: placeholder_color(colors.dim),
            });

        let context = self.project.as_ref().and_then(|project| {
            self.thread
                .usage
                .context_percent(project.window)
                .map(|percent| {
                    format!(
                        "ctx {percent:.0}% · {}",
                        transcript::format_tokens(project.window)
                    )
                })
        });
        let spend = self.project.as_ref().and_then(|project| {
            (self.thread.usage.cost > 0.0).then(|| {
                format!(
                    "{}{}",
                    transcript::format_cost(self.thread.usage.cost),
                    if project.subscription { " (sub)" } else { "" }
                )
            })
        });
        let tokens = if self.thread.usage.input + self.thread.usage.output > 0 {
            format!(
                "↑{} ↓{}",
                transcript::format_tokens(self.thread.usage.input),
                transcript::format_tokens(self.thread.usage.output)
            )
        } else {
            String::new()
        };
        let status = match self
            .project
            .as_ref()
            .and_then(|project| project.key_problem.clone())
        {
            Some(problem) if !running && self.thread.items.is_empty() => problem,
            _ if running => match self.started {
                Some(started) => format!(
                    "{} · {}",
                    self.thread.phase.label(),
                    transcript::format_duration(started.elapsed().as_millis() as u64)
                ),
                None => self.thread.phase.label().to_string(),
            },
            _ => self.thread.status(),
        };

        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_6()
            .pb_3()
            .children(rows)
            .children(queued)
            .child(
                div()
                    .max_w(px(780.))
                    .mx_auto()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .p_3()
                    .rounded_lg()
                    .bg(colors.panel)
                    .border_1()
                    .border_color(if has_text {
                        colors.accent
                    } else {
                        colors.border
                    })
                    .child(editor)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(11.))
                                    .text_color(if self.thread.error.is_some() {
                                        colors.error
                                    } else {
                                        colors.dim
                                    })
                                    .child(status),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap_2()
                                            .text_size(px(11.))
                                            .text_color(colors.dim)
                                            .child(tokens)
                                            .child(spend.unwrap_or_default())
                                            .child(context.unwrap_or_default()),
                                    )
                                    .child(action),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn header(&mut self, colors: &ItemColors) -> AnyElement {
        let title = self.thread.label().to_string();
        let project = self.project.as_ref();
        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .px_6()
            .py_2()
            .border_b_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .min_w(px(0.))
                    .child(
                        div()
                            .text_color(colors.text)
                            .font_weight(FontWeight::BOLD)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.faint)
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(match project {
                                Some(project) => project.path.display().to_string(),
                                None => "No project open".to_string(),
                            }),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(if project.is_some_and(|project| project.trusted) {
                                colors.success
                            } else {
                                colors.dim
                            })
                            .child(match project {
                                Some(project) if project.trusted => "trusted".to_string(),
                                Some(_) => "untrusted".to_string(),
                                None => String::new(),
                            }),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(colors.dim)
                            .child(match project {
                                Some(project) => project.model.clone(),
                                None => String::new(),
                            }),
                    ),
            )
            .into_any_element()
    }

    fn dialogs(&mut self, colors: &ItemColors, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.approval.is_none() && self.question.is_none() {
            if let Some(update) = self.update.as_ref() {
                let check = update.check.clone();
                let status = update.status.clone();
                let mut card =
                    div()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .p_4()
                        .w(px(560.))
                        .rounded_lg()
                        .bg(colors.panel)
                        .border_1()
                        .border_color(colors.border)
                        .child(
                            div()
                                .text_color(colors.text)
                                .font_weight(FontWeight::BOLD)
                                .child(match &check {
                                    Some(check) if check.update_available => {
                                        format!("Update available (v{})", check.latest)
                                    }
                                    Some(check) => format!("Oxide {}", check.current),
                                    None => "Check for updates".to_string(),
                                }),
                        )
                        .child(div().text_size(px(11.)).text_color(colors.dim).child(
                            match &check {
                                Some(check) if check.update_available => match &check.notes {
                                    Some(notes) => {
                                        notes.lines().take(8).collect::<Vec<_>>().join("\n")
                                    }
                                    None => format!("Release {}", check.tag),
                                },
                                Some(check) => check.latest.clone(),
                                None => status.clone(),
                            },
                        ));
                if !status.is_empty() {
                    card = card.child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.dim)
                            .child(status),
                    );
                }
                let installable = check
                    .as_ref()
                    .is_some_and(|check| check.update_available && check.installable);
                let requested = check.clone();
                card = card.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1())
                        .child(self.dialog_button(
                            "update-later",
                            "Later",
                            colors.pending,
                            colors.text,
                            cx,
                            |this, _| this.update = None,
                        ))
                        .when(installable, |card| {
                            card.child(self.dialog_button(
                                "update-install",
                                "Install",
                                colors.accent,
                                colors.background,
                                cx,
                                move |this, cx| {
                                    if let Some(check) = requested.clone() {
                                        this.runner.install_update(check);
                                    }
                                    if let Some(update) = this.update.as_mut() {
                                        update.status = "Downloading update…".to_string();
                                    }
                                    cx.notify();
                                },
                            ))
                        }),
                );
                return Some(self.overlay(card.into_any_element()));
            }
        }
        if let Some(approval) = self.approval.as_ref() {
            let id = approval.id;
            let tool = approval.tool.clone();
            let detail = approval.detail.clone();
            let card = div()
                .flex()
                .flex_col()
                .gap_3()
                .p_4()
                .w(px(520.))
                .rounded_lg()
                .bg(colors.panel)
                .border_1()
                .border_color(colors.border)
                .child(
                    div()
                        .text_color(colors.text)
                        .font_weight(FontWeight::BOLD)
                        .child(format!("Allow `{tool}`?")),
                )
                .child(
                    div()
                        .font_family("Menlo")
                        .text_size(px(11.))
                        .text_color(colors.dim)
                        .child(detail),
                )
                .child(
                    div()
                        .flex()
                        .gap_2()
                        .child(self.dialog_button(
                            "allow-once",
                            "Allow once",
                            colors.accent,
                            colors.background,
                            cx,
                            move |this, cx| {
                                this.runner.answer_approval(id, Decision::Once);
                                this.approval = None;
                                let _ = cx;
                            },
                        ))
                        .child(self.dialog_button(
                            "allow-always",
                            "Always allow",
                            colors.pending,
                            colors.text,
                            cx,
                            move |this, _| {
                                this.runner.answer_approval(id, Decision::Always);
                                this.approval = None;
                            },
                        ))
                        .child(self.dialog_button(
                            "deny",
                            "Deny",
                            colors.pending,
                            colors.error,
                            cx,
                            move |this, _| {
                                this.runner
                                    .answer_approval(id, Decision::Deny { message: None });
                                this.approval = None;
                            },
                        )),
                );
            return Some(self.overlay(card.into_any_element()));
        }
        let question = self.question.as_ref()?;
        let form = &question.form;
        let id = question.id;
        let step = form.step;
        let total = form.questions.len();
        let current = form.questions.get(step)?.clone();
        let last = form.last_step();
        let heading = current
            .header
            .clone()
            .filter(|header| !header.trim().is_empty())
            .unwrap_or_else(|| current.question.clone());

        let mut options: Vec<AnyElement> = Vec::new();
        for (index, choice) in current.options.iter().enumerate() {
            let chosen = form.is_picked(index);
            let multi = current.multi_select;
            options.push(
                div()
                    .id(("question-choice", index))
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .cursor_pointer()
                    .bg(if chosen { colors.pending } else { colors.panel })
                    .border_1()
                    .border_color(if chosen { colors.accent } else { colors.border })
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(question) = this.question.as_mut() {
                            question.form.step = step;
                            question.form.choose(index);
                        }
                        cx.notify();
                    }))
                    .child(div().text_color(colors.text).child(format!(
                        "{} {}",
                        if chosen {
                            if multi {
                                "☑"
                            } else {
                                "◉"
                            }
                        } else if multi {
                            "☐"
                        } else {
                            "○"
                        },
                        choice.label
                    )))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.dim)
                            .child(choice.description.clone().unwrap_or_default()),
                    )
                    .into_any_element(),
            );
        }

        let typed_focus = self.answer_editor.read(cx).focus_handle();
        let typed = div()
            .id("question-typed")
            .w_full()
            .min_h(px(20.))
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(colors.border)
            .cursor(CursorStyle::IBeam)
            .track_focus(&typed_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &gpui::MouseDownEvent, window, cx| {
                    let handle = this.answer_editor.read(cx).focus_handle();
                    window.focus(&handle);
                    let position = event.position;
                    this.answer_editor
                        .update(cx, |editor, cx| editor.begin_selection(position, cx));
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &gpui::MouseMoveEvent, _, cx| {
                let position = event.position;
                this.answer_editor
                    .update(cx, |editor, cx| editor.extend_selection(position, cx));
            }))
            .child(EditorElement {
                editor: self.answer_editor.clone(),
                selection: selection_color(colors.accent),
                caret: caret_color(colors.accent),
                placeholder: placeholder_color(colors.dim),
            });

        let card = div()
            .flex()
            .flex_col()
            .gap_3()
            .p_4()
            .w(px(560.))
            .rounded_lg()
            .bg(colors.panel)
            .border_1()
            .border_color(colors.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_color(colors.text)
                            .font_weight(FontWeight::BOLD)
                            .child(heading),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(colors.faint)
                            .child(format!("{} of {}", step + 1, total)),
                    ),
            )
            .child(div().text_color(colors.dim).child(current.question.clone()))
            .children(options)
            .child(typed)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(self.dialog_button(
                        "question-back",
                        "Back",
                        colors.pending,
                        if step > 0 { colors.text } else { colors.faint },
                        cx,
                        move |this, cx| {
                            if let Some(question) = this.question.as_mut() {
                                if question.form.step > 0 {
                                    question.form.step -= 1;
                                    question.form.typed[step] =
                                        this.answer_editor.read(cx).content();
                                    question.form.typed[question.form.step] =
                                        this.answer_editor.read(cx).content();
                                }
                            }
                        },
                    ))
                    .child(div().flex_1())
                    .child(self.dialog_button(
                        "question-dismiss",
                        "Dismiss",
                        colors.pending,
                        colors.dim,
                        cx,
                        move |this, cx| {
                            this.runner.answer_question(id, Vec::new());
                            this.question = None;
                            this.answer_editor.update(cx, |editor, cx| editor.clear(cx));
                        },
                    ))
                    .child(self.dialog_button(
                        "question-next",
                        if last { "Submit" } else { "Next" },
                        colors.accent,
                        colors.background,
                        cx,
                        move |this, cx| {
                            let typed = this.answer_editor.read(cx).content();
                            let done = if let Some(question) = this.question.as_mut() {
                                question.form.typed[step] = typed;
                                if question.form.step + 1 < question.form.questions.len() {
                                    question.form.step += 1;
                                    let next = question.form.step;
                                    question.form.typed[next].clear();
                                    false
                                } else {
                                    true
                                }
                            } else {
                                false
                            };
                            if done {
                                let answers = this
                                    .question
                                    .as_ref()
                                    .map(|question| question.form.answers())
                                    .unwrap_or_default();
                                this.runner.answer_question(id, answers);
                                this.question = None;
                            }
                            let restored = this
                                .question
                                .as_ref()
                                .map(|question| question.form.typed[question.form.step].clone());
                            this.answer_editor.update(cx, |editor, cx| {
                                editor.clear(cx);
                                if let Some(text) = restored {
                                    editor.set_content(text, cx);
                                }
                            });
                        },
                    )),
            );
        Some(self.overlay(card.into_any_element()))
    }

    fn overlay(&self, card: AnyElement) -> AnyElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(gpui::rgba(0x00000060))
            .child(card)
            .into_any_element()
    }

    fn dialog_button(
        &self,
        id: &'static str,
        label: &'static str,
        background: Rgba,
        foreground: Rgba,
        cx: &mut Context<Self>,
        handler: impl Fn(&mut Self, &mut Context<Self>) + 'static,
    ) -> AnyElement {
        div()
            .id(id)
            .px_3()
            .py_1()
            .rounded_md()
            .bg(background)
            .text_color(foreground)
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                handler(this, cx);
                cx.notify();
            }))
            .child(label)
            .into_any_element()
    }
}

/// The colors a transcript item is painted with, copied out of the theme so the
/// closures that paint them are not borrowing it.
#[derive(Clone, Copy)]
pub struct ItemColors {
    background: Rgba,
    panel: Rgba,
    border: Rgba,
    text: Rgba,
    dim: Rgba,
    faint: Rgba,
    accent: Rgba,
    user: Rgba,
    assistant: Rgba,
    tool: Rgba,
    success: Rgba,
    error: Rgba,
    pending: Rgba,
    success_bg: Rgba,
    error_bg: Rgba,
    thinking: Rgba,
}

impl ItemColors {
    fn of(theme: &Theme) -> Self {
        Self {
            background: theme.background(),
            panel: theme.panel(),
            border: theme.border(),
            text: theme.text(),
            dim: theme.dim(),
            faint: theme.faint(),
            accent: theme.accent(),
            user: theme.user(),
            assistant: theme.assistant(),
            tool: theme.tool(),
            success: theme.success(),
            error: theme.error(),
            pending: theme.tool_pending_bg(),
            success_bg: theme.tool_success_bg(),
            error_bg: theme.tool_error_bg(),
            thinking: theme.thinking_text(),
        }
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh_completion(cx);
        let colors = ItemColors::of(&self.theme);
        let dialogs = self.dialogs(&colors, cx);
        let sidebar = self.sidebar(cx);
        let header = self.header(&colors);
        let transcript = self.transcript(&colors, cx);
        let composer = self.composer(&colors, cx);
        div()
            .relative()
            .flex()
            .flex_row()
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .text_size(px(13.))
            .line_height(px(19.))
            .child(sidebar)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w(px(0.))
                    .h_full()
                    .child(header)
                    .child(transcript)
                    .child(composer),
            )
            .children(dialogs)
    }
}
