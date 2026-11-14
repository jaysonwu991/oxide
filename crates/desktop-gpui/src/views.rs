//! The window's panels: the rail, the sidebar, the transcript, the composer and
//! the changes pane.
//!
//! They draw the same furniture the Electron window does — one topbar with the
//! chat tabs, a navigation rail, the project/session sidebar, the conversation
//! and its composer, and the changes a turn left behind — from the same state
//! `crate::app::Root` keeps.

use crate::app::{Button, Overlay, Rail, Root};
use crate::icons::{self, Icon};
use crate::markdown::{Block, Inline};
use crate::model::{Entry, Status};
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{
    div, px, Context, Font, FontStyle, FontWeight, MouseButton, Pixels, Render, SharedString,
    Stateful, StrikethroughStyle, StyledText, TextRun, UnderlineStyle, Window, WindowControlArea,
};

// ---------- shared pieces ----------

/// A square icon control: the window's own buttons in the topbar, the rail, a
/// card's header and a row's trailing actions. `box_size` is the target the
/// control fills; the glyph inside it is three fifths of that, the 18-in-30 the
/// page's `.icon` buttons use.
fn icon_button(
    id: impl Into<gpui::ElementId>,
    mark: Icon,
    box_size: f32,
    theme: &Theme,
    active: bool,
) -> Stateful<gpui::Div> {
    let color = if active { theme.text() } else { theme.dim() };
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .w(px(box_size))
        .h(px(box_size))
        .rounded(px(8.0))
        .cursor_pointer()
        .text_color(color)
        .hover(move |style| style.bg(theme.panel_2()))
        .child(icons::icon(mark, box_size * 0.6, color))
}

fn pill(text: impl Into<String>, theme: &Theme, accent: bool) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .flex_none()
        .h(px(19.0))
        .px(px(7.0))
        .rounded(px(6.0))
        .bg(if accent {
            theme.wash("accent", 0.2)
        } else {
            theme.panel_3()
        })
        .text_color(if accent { theme.accent() } else { theme.dim() })
        .text_size(px(10.0))
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

/// The runs a markdown span paints with.
fn runs(theme: &Theme, spans: &[Inline]) -> (String, Vec<TextRun>) {
    let mut text = String::new();
    let mut runs = Vec::new();
    for span in spans {
        if span.text.is_empty() {
            continue;
        }
        let family: SharedString = if span.code {
            "Menlo".into()
        } else {
            ".SystemUIFont".into()
        };
        let color = if span.link.is_some() {
            theme.accent()
        } else if span.code {
            theme.tool()
        } else {
            theme.text()
        };
        runs.push(TextRun {
            len: span.text.len(),
            font: Font {
                family,
                features: Default::default(),
                fallbacks: None,
                weight: if span.bold {
                    FontWeight::SEMIBOLD
                } else {
                    FontWeight::NORMAL
                },
                style: if span.italic {
                    FontStyle::Italic
                } else {
                    FontStyle::Normal
                },
            },
            color,
            background_color: span.code.then(|| theme.panel_3()),
            underline: span.link.as_ref().map(|_| UnderlineStyle {
                thickness: px(1.0),
                color: Some(color),
                wavy: false,
            }),
            strikethrough: span.strike.then(|| StrikethroughStyle {
                thickness: px(1.0),
                color: Some(color),
            }),
        });
        text.push_str(&span.text);
    }
    (text, runs)
}

fn styled(theme: &Theme, spans: &[Inline]) -> StyledText {
    let (text, runs) = runs(theme, spans);
    StyledText::new(text).with_runs(runs)
}

/// A single line of the window's text, in the window's own size.
fn line(text: impl Into<SharedString>, theme: &Theme) -> gpui::Div {
    div()
        .text_size(px(13.5))
        .text_color(theme.text())
        .line_height(px(20.0))
        .child(text.into())
}

fn session_when(session: &crate::model::SessionRow) -> String {
    let time = relative_time(session.modified_at);
    match &session.worktree {
        Some(worktree) => {
            if time.is_empty() {
                worktree.clone()
            } else {
                format!("{time} · {worktree}")
            }
        }
        None => time,
    }
}

fn relative_time(seconds: u64) -> String {
    let now = Root::unix_now();
    if seconds == 0 || seconds > now {
        return String::new();
    }
    let delta = now - seconds;
    match delta {
        0..=59 => "now".to_string(),
        60..=3599 => format!("{}m", delta / 60),
        3600..=86_399 => format!("{}h", delta / 3600),
        86_400..=604_799 => format!("{}d", delta / 86_400),
        _ => format!("{}w", delta / 604_800),
    }
}

// ---------- tooltips ----------

/// The hover label a control carries: GPUI draws no accessible name of its own,
/// so the words a glyph would have said live here.
pub struct Tip {
    text: SharedString,
    theme: Theme,
}

impl Render for Tip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .px(px(8.0))
            .py(px(4.0))
            .rounded(px(6.0))
            .bg(self.theme.panel_3())
            .border_1()
            .border_color(self.theme.border())
            .text_size(px(11.0))
            .text_color(self.theme.text())
            .child(self.text.clone())
    }
}

pub fn tip(
    text: &'static str,
    theme: &Theme,
) -> impl Fn(&mut Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let theme = theme.clone();
    move |_window, cx| {
        let tip = cx.new(|_| Tip {
            text: SharedString::from(text),
            theme: theme.clone(),
        });
        gpui::AnyView::from(tip)
    }
}

// ---------- topbar ----------

/// The thread the window is on: the folder it runs in and its title. Drawn in
/// place of the tabs while one chat is open.
fn thread_label(root: &Root, theme: &Theme) -> impl IntoElement {
    let title = match root.chats.get(root.active) {
        Some(chat) => chat.title.clone(),
        None => "No open chats".to_string(),
    };
    div()
        .flex()
        .flex_row()
        .items_center()
        .flex_1()
        .min_w(px(0.0))
        .gap_2()
        .child(icons::icon(Icon::Folder, 16.0, theme.faint()))
        .child(
            div()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text())
                .child(title),
        )
        .child(div().flex_1())
}

pub fn topbar(
    root: &mut Root,
    theme: &Theme,
    scale: f32,
    _window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let mut row = div()
        .window_control_area(WindowControlArea::Drag)
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .h(px(46.0 * scale))
        .pl(px(76.0))
        .pr_2()
        .gap_1()
        .bg(theme.background())
        .border_b_1()
        .border_color(theme.border());

    let back = icon_button("nav-back", Icon::Back, 30.0, theme, false)
        .tooltip(tip("Back \u{00b7} \u{2318}[", theme))
        .on_click(cx.listener(|root, _, _, cx| {
            if let Some(previous) = root.history.pop() {
                root.forward.push(root.active);
                root.active = previous.min(root.chats.len().saturating_sub(1));
                cx.notify();
            }
        }));
    let forward = icon_button("nav-forward", Icon::Forward, 30.0, theme, false)
        .tooltip(tip("Forward \u{00b7} \u{2318}]", theme))
        .on_click(cx.listener(|root, _, _, cx| {
            if let Some(next) = root.forward.pop() {
                root.history.push(root.active);
                root.active = next.min(root.chats.len().saturating_sub(1));
                cx.notify();
            }
        }));
    let toggle = icon_button("nav-sidebar", Icon::Sidebar, 30.0, theme, false)
        .tooltip(tip("Show or hide the sidebar \u{00b7} \u{2318}B", theme))
        .on_click(cx.listener(|root, _, _, cx| {
            root.sidebar_visible = !root.sidebar_visible;
            let visible = root.sidebar_visible;
            root.save_prefs(serde_json::json!({ "sidebar_visible": visible }), cx);
            cx.notify();
        }));
    row = row
        .child(back)
        .child(forward)
        .child(toggle)
        .child(div().flex_none().w(px(6.0)));

    let mut tabs = div()
        .flex()
        .flex_row()
        .items_center()
        .flex_1()
        .min_w(px(0.0))
        .gap_1()
        .overflow_hidden();
    if root.chats.is_empty() {
        tabs = tabs.child(
            div()
                .text_size(px(12.5))
                .text_color(theme.faint())
                .child("No open chats"),
        );
    }
    let mut tab_elements = Vec::new();
    for (index, chat) in root.chats.iter().enumerate() {
        let active = index == root.active;
        let title = chat.title.clone();
        let mut tab = div()
            .id(("chat-tab", index))
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .w(px(190.0))
            .h(px(32.0))
            .px_2()
            .gap_1()
            .rounded(px(8.0))
            .cursor_pointer()
            .bg(if active {
                theme.panel_2()
            } else {
                theme.sidebar()
            })
            .text_color(if active { theme.text() } else { theme.dim() })
            .hover(move |style| style.bg(theme.panel_2()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_size(px(12.5))
                    .child(title),
            );
        if chat.running() {
            tab = tab.child(
                div()
                    .flex_none()
                    .w(px(6.0))
                    .h(px(6.0))
                    .rounded(px(3.0))
                    .bg(theme.accent()),
            );
        }
        tab = tab.child(
            icon_button(("close-tab", index), Icon::Close, 22.0, theme, false)
                .tooltip(tip("Close this chat", theme))
                .on_click(cx.listener(move |root, _, _, cx| root.close_chat(index, cx))),
        );
        tab_elements.push(tab.on_click(cx.listener(move |root, _, _window, cx| {
            if index == root.active {
                return;
            }
            root.history.push(root.active);
            root.forward.clear();
            root.active = index;
            cx.notify();
        })));
    }
    tabs = tabs.children(tab_elements);
    // One chat open has nothing to switch between, so the topbar names the
    // thread that is on screen — the folder, its title — the way the page swaps
    // its tabs for the thread's own label.
    if root.chats.len() > 1 {
        row = row.child(tabs);
    } else {
        row = row.child(thread_label(root, theme));
    }

    let new_chat = icon_button("new-chat", Icon::Plus, 30.0, theme, false)
        .tooltip(tip("New chat \u{00b7} \u{2318}N", theme))
        .on_click(cx.listener(|root, _, window, cx| root.new_chat(window, cx)));
    let changes = icon_button("changes", Icon::Changes, 30.0, theme, root.show_changes)
        .tooltip(tip(
            "Changes this turn left \u{00b7} \u{2318}\u{23ce}",
            theme,
        ))
        .on_click(cx.listener(|root, _, _, cx| {
            root.show_changes = !root.show_changes;
            if root.show_changes {
                let index = root.active;
                root.refresh_repository(index, cx);
            }
            cx.notify();
        }));
    let details = icon_button(
        "details-toggle",
        Icon::Details,
        30.0,
        theme,
        root.show_details,
    )
    .tooltip(tip("Project details", theme))
    .on_click(cx.listener(|root, _, _, cx| root.toggle_details(cx)));
    let beta = pill("BETA", theme, true);
    row.child(new_chat)
        .child(changes)
        .child(details)
        .child(div().flex_none().w(px(4.0)))
        .child(beta)
}

// ---------- rail ----------

pub fn rail(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let chats = root.rail == Rail::Chats;
    let customize = root.rail == Rail::Customize;
    let sidebar = root.sidebar_visible;
    div()
        .flex()
        .flex_col()
        .items_center()
        .flex_none()
        .w(px(56.0))
        .h_full()
        .py(px(12.0))
        .px(px(8.0))
        .gap(px(5.0))
        .bg(theme.sidebar())
        .border_r_1()
        .border_color(theme.border())
        .child(
            icon_button("rail-home", Icon::Home, 40.0, theme, chats)
                .tooltip(tip("Chats", theme))
                .on_click(cx.listener(|root, _, _, cx| {
                    root.rail = Rail::Chats;
                    root.overlay = None;
                    cx.notify();
                })),
        )
        .child(
            icon_button("rail-sidebar", Icon::Sidebar, 40.0, theme, sidebar)
                .tooltip(tip("Show or hide the sidebar", theme))
                .on_click(cx.listener(|root, _, _, cx| {
                    root.sidebar_visible = !root.sidebar_visible;
                    let visible = root.sidebar_visible;
                    root.save_prefs(serde_json::json!({ "sidebar_visible": visible }), cx);
                    cx.notify();
                })),
        )
        .child(
            icon_button("rail-archived", Icon::Archive, 40.0, theme, false)
                .tooltip(tip("Archived threads", theme))
                .on_click(cx.listener(|root, _, _, cx| root.open_overlay(Overlay::Archived, cx))),
        )
        .child(div().flex_1())
        .child(
            icon_button("rail-customize", Icon::Customize, 40.0, theme, customize)
                .tooltip(tip("Customize", theme))
                .on_click(cx.listener(|root, _, _, cx| {
                    root.rail = Rail::Customize;
                    cx.notify();
                })),
        )
        .child(
            icon_button("rail-settings", Icon::Settings, 40.0, theme, false)
                .tooltip(tip("Settings \u{00b7} \u{2318},", theme))
                .on_click(cx.listener(|root, _, _, cx| root.open_overlay(Overlay::Settings, cx))),
        )
}

// ---------- sidebar ----------

pub fn sidebar(
    root: &mut Root,
    theme: &Theme,
    scale: f32,
    _window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let width = root.sidebar_width;
    let search = root.search.clone();
    let height = px(scale * 20.0).max(px(20.0));
    let searching = root.searching;

    let mut column = div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(width))
        .h_full()
        .min_h(px(0.0))
        .bg(theme.sidebar());

    let mut top = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .h(px(44.0))
        .px(px(12.0));
    if searching {
        top = top.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .min_w(px(0.0))
                .h(px(30.0))
                .px_2()
                .rounded(px(8.0))
                .bg(theme.panel())
                .border_1()
                .border_color(theme.border())
                .child(icons::icon(Icon::Search, 14.0, theme.faint()))
                .child(div().flex_1().min_w(px(0.0)).h(height).child(search))
                .child(
                    icon_button("sidebar-search-close", Icon::Close, 22.0, theme, false)
                        .on_click(cx.listener(|root, _, _, cx| root.close_search(cx))),
                ),
        );
    } else {
        top = top
            .child(
                div()
                    .flex_1()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text())
                    .child("Oxide"),
            )
            .child(
                icon_button("sidebar-search", Icon::Search, 28.0, theme, false)
                    .tooltip(tip("Search chats", theme))
                    .on_click(cx.listener(|root, _, window, cx| root.open_search(window, cx))),
            )
            .child(
                icon_button("sidebar-add-project-top", Icon::Plus, 28.0, theme, false)
                    .tooltip(tip("Add a project", theme))
                    .on_click(cx.listener(|root, _, _, cx| root.add_project(cx))),
            );
    }
    column = column.child(top);

    column = column.child(
        div().px(px(8.0)).pb(px(6.0)).child(
            div()
                .id("sidebar-new-chat")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(7.0))
                .h(px(34.0))
                .px_2()
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel()))
                .child(icons::icon(Icon::Pencil, 15.0, theme.accent()))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text())
                        .child("New Chat"),
                )
                .on_click(cx.listener(|root, _, window, cx| root.new_chat(window, cx))),
        ),
    );

    let mut list = div()
        .id("sidebar-list")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .px(px(4.0))
        .pb(px(14.0))
        .gap(px(2.0));

    if !root.query.trim().is_empty() {
        let results = root.results.clone();
        if results.is_empty() {
            list = list.child(
                div()
                    .px_2()
                    .py_2()
                    .text_size(px(12.5))
                    .text_color(theme.faint())
                    .child("No chat matches that"),
            );
        }
        for session in results {
            let id = session.id.clone();
            let when = if session.preview.is_empty() {
                relative_time(session.modified_at)
            } else {
                session
                    .preview
                    .lines()
                    .next()
                    .unwrap_or_default()
                    .chars()
                    .take(24)
                    .collect()
            };
            list = list.child(session_row(
                &session.title,
                &when,
                session.pinned,
                session.worktree.is_some(),
                theme,
                cx,
                move |root, window, cx| {
                    if let Some(found) = root.sessions.iter().find(|row| row.id == id).cloned() {
                        root.open_session(window, found, cx);
                    }
                },
            ));
        }
    } else {
        let pinned: Vec<_> = root
            .sessions
            .iter()
            .filter(|session| session.pinned)
            .cloned()
            .collect();
        if !pinned.is_empty() {
            list = list.child(section("Pinned", theme));
            for session in pinned {
                let id = session.id.clone();
                let title = session.title.clone();
                let when = session_when(&session);
                list = list.child(session_row(
                    &title,
                    &when,
                    true,
                    session.worktree.is_some(),
                    theme,
                    cx,
                    move |root, window, cx| {
                        if let Some(found) = root.sessions.iter().find(|row| row.id == id).cloned()
                        {
                            root.open_session(window, found, cx);
                        }
                    },
                ));
            }
        }

        list = list.child(projects_header(theme, cx));
        let projects = root.projects.clone();
        let collapsed = root.collapsed.clone();
        for project in projects {
            let path = project.path.clone();
            let name = root.project_name(&path);
            let sessions = root
                .sessions_of(&path)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            let count = if project.session_count > 0 {
                project.session_count
            } else {
                sessions.len()
            };
            let expanded = !collapsed.contains(&path);
            let missing = !project.exists;
            let pinned = project.pinned;
            let when = relative_time(project.last_session_at);
            let path_for_new = path.clone();
            let path_for_remove = path.clone();
            let path_for_toggle = path.clone();
            let project_id = project.id.clone();
            let project_name = name.clone();
            let mut header = div()
                .id(SharedString::from(format!("project-{path}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(px(30.0))
                .px_2()
                .rounded(px(8.0))
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel()))
                .child(div().flex_none().w(px(4.0)))
                .child(icons::icon(
                    if expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    12.0,
                    theme.faint(),
                ))
                .child(icons::icon(Icon::Folder, 15.0, theme.dim()))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(13.0))
                        .text_color(theme.dim())
                        .child(name),
                )
                .children(pinned.then(|| icons::icon(Icon::Pin, 12.0, theme.accent())))
                .children(missing.then(|| {
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(theme.error())
                        .child("missing")
                }))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(theme.faint())
                        .child(if when.is_empty() {
                            count.to_string()
                        } else {
                            format!("{count} · {when}")
                        }),
                )
                .child(
                    icon_button(
                        SharedString::from(format!("project-new-{path}")),
                        Icon::Plus,
                        20.0,
                        theme,
                        false,
                    )
                    .tooltip(tip("New chat in this project", theme))
                    .on_click(cx.listener(move |root, _, window, cx| {
                        cx.stop_propagation();
                        root.push_chat(
                            window,
                            path_for_new.clone(),
                            None,
                            "New chat".to_string(),
                            cx,
                        );
                        root.focus_composer(window, cx);
                    })),
                )
                .child(
                    icon_button(
                        SharedString::from(format!("project-remove-{path}")),
                        Icon::More,
                        20.0,
                        theme,
                        false,
                    )
                    .tooltip(tip("Pin or remove this project", theme))
                    .on_click(cx.listener(move |root, _, _window, cx| {
                        cx.stop_propagation();
                        let pinned = root
                            .projects
                            .iter()
                            .find(|project| project.id == project_id)
                            .map(|project| project.pinned)
                            .unwrap_or(false);
                        root.overlay = Some(Overlay::ProjectActions {
                            id: project_id.clone(),
                            path: path_for_remove.clone(),
                            name: project_name.clone(),
                            pinned,
                        });
                        cx.notify();
                    })),
                );
            header = header.on_click(cx.listener(move |root, _, _window, cx| {
                if root.collapsed.contains(&path_for_toggle) {
                    root.collapsed.remove(&path_for_toggle);
                } else {
                    root.collapsed.insert(path_for_toggle.clone());
                }
                cx.notify();
            }));
            list = list.child(header);
            if expanded {
                if sessions.is_empty() {
                    list = list.child(
                        div()
                            .pl(px(46.0))
                            .pr_2()
                            .pb_1()
                            .text_size(px(12.0))
                            .text_color(theme.faint())
                            .child("No chats"),
                    );
                }
                for session in sessions {
                    let id = session.id.clone();
                    let title = session.title.clone();
                    let when = session_when(&session);
                    let pinned = session.pinned;
                    list = list.child(session_row(
                        &title,
                        &when,
                        pinned,
                        session.worktree.is_some(),
                        theme,
                        cx,
                        move |root, window, cx| {
                            if let Some(found) =
                                root.sessions.iter().find(|row| row.id == id).cloned()
                            {
                                root.open_session(window, found, cx);
                            }
                        },
                    ));
                }
            }
        }

        // The threads the reader was in most recently, across every folder: the
        // page ends its sidebar with the same list, so a thread is one click
        // away even when its project is folded away.
        let mut recents = root.sessions.clone();
        recents.retain(|session| !session.pinned);
        recents.sort_by_key(|session| std::cmp::Reverse(session.modified_at));
        recents.truncate(5);
        if !recents.is_empty() {
            list = list.child(section("Recents", theme));
            for session in recents {
                let id = session.id.clone();
                let title = session.title.clone();
                let when = session_when(&session);
                let pinned = session.pinned;
                list = list.child(session_row(
                    &title,
                    &when,
                    pinned,
                    session.worktree.is_some(),
                    theme,
                    cx,
                    move |root, window, cx| {
                        if let Some(found) = root.sessions.iter().find(|row| row.id == id).cloned()
                        {
                            root.open_session(window, found, cx);
                        }
                    },
                ));
            }
        }
    }

    column = column.child(list);
    column
}

fn section(title: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .px_2()
        .pt_2()
        .pb_1()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.faint())
        .child(title)
}

/// The sidebar's `Projects` heading: the name on the left, the folder-picker
/// behind the plus on the right, the way the page pairs them.
fn projects_header(theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(28.0))
        .px_2()
        .pt_1()
        .child(
            div()
                .flex_1()
                .text_size(px(11.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.faint())
                .child("Projects"),
        )
        .child(
            icon_button("sidebar-add-project", Icon::Plus, 22.0, theme, false)
                .on_click(cx.listener(|root, _, _, cx| root.add_project(cx))),
        )
}

fn session_row(
    title: &str,
    when: &str,
    pinned: bool,
    worktree: bool,
    theme: &Theme,
    cx: &mut Context<Root>,
    open: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> impl IntoElement {
    let title = title.to_string();
    let when = when.to_string();
    div()
        .id(SharedString::from(format!("session-{title}")))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(30.0))
        .pl(px(30.0))
        .pr_2()
        .rounded(px(8.0))
        .cursor_pointer()
        .hover(move |style| style.bg(theme.panel()))
        .child(icons::icon(
            if pinned { Icon::Pin } else { Icon::Chat },
            13.0,
            if pinned {
                theme.accent()
            } else {
                theme.faint()
            },
        ))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(12.5))
                .text_color(theme.text())
                .child(title.clone()),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(10.5))
                .text_color(theme.faint())
                .child(when),
        )
        .children(worktree.then(|| icons::icon(Icon::Branch, 12.0, theme.accent())))
        .on_click(cx.listener(move |root, _, window, cx| open(root, window, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |root, _, _, cx| {
                let Some(session) = root.sessions.iter().find(|row| row.title == title).cloned()
                else {
                    return;
                };
                root.overlay = Some(Overlay::SessionActions {
                    id: session.id,
                    title: session.title,
                    pinned: session.pinned,
                });
                cx.notify();
            }),
        )
}

pub fn sidebar_resizer(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let _ = root;
    div()
        .id("sidebar-resizer")
        .flex_none()
        .w(px(6.0))
        .h_full()
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .bg(theme.sidebar())
        .border_r_1()
        .border_color(theme.border())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|root, event, _, cx| root.begin_sidebar_drag(event, cx)),
        )
        .on_mouse_move(cx.listener(|root, event, _, cx| root.handle_sidebar_drag(event, cx)))
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|root, _, _, cx| root.end_sidebar_drag(cx)),
        )
}

// ---------- main ----------

// ---------- details ----------

/// The pane beside the conversation: the project, the work tree's own totals,
/// the branch, and the sources (MCP servers) the thread can reach — the page's
/// right-hand column, at the same 280px.
pub fn details(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let project = root.selected_project().cloned();
    let branch = root.branch.clone();
    let sources = root.sources.clone();
    let totals = root
        .chats
        .get(root.active)
        .and_then(|chat| chat.changes.clone())
        .unwrap_or(serde_json::Value::Null);
    let added = totals["added"].as_u64().unwrap_or(0);
    let removed = totals["removed"].as_u64().unwrap_or(0);
    let files = totals["files"].as_array().map(Vec::len).unwrap_or(0);

    let mut column = div()
        .id("details-pane")
        .flex()
        .flex_col()
        .flex_none()
        .w(px(280.0))
        .h_full()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .gap_1()
        .px(px(14.0))
        .py(px(12.0))
        .bg(theme.background())
        .border_l_1()
        .border_color(theme.border());

    let name = project
        .as_ref()
        .map(|project| root.project_name(&project.path))
        .unwrap_or_else(|| "No project".to_string());
    column = column.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(30.0))
            .px_2()
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.5))
                    .text_color(theme.dim())
                    .child(name),
            )
            .child({
                let project = project.clone();
                icon_button("details-more", Icon::More, 24.0, theme, false)
                    .tooltip(tip("Project actions", theme))
                    .on_click(cx.listener(move |root, _, _, cx| {
                        let Some(project) = project.clone() else {
                            return;
                        };
                        root.overlay = Some(Overlay::ProjectActions {
                            id: project.id,
                            path: project.path,
                            name: project.name,
                            pinned: project.pinned,
                        });
                        cx.notify();
                    }))
            }),
    );

    column = column.child(detail_row(
        "details-changes",
        Icon::Changes,
        "View changes",
        if files > 0 {
            Some(vec![
                (format!("+{added}"), theme.success()),
                (format!("\u{2212}{removed}"), theme.error()),
            ])
        } else {
            None
        },
        theme,
        cx,
        |root, _, cx| {
            root.show_changes = !root.show_changes;
            let index = root.active;
            if root.show_changes {
                root.refresh_repository(index, cx);
            }
            cx.notify();
        },
    ));

    column = column.child(detail_row(
        "details-repository",
        Icon::Folder,
        "Repository",
        Some(vec![(
            branch.unwrap_or_else(|| "Local checkout".to_string()),
            theme.faint(),
        )]),
        theme,
        cx,
        |root, _, cx| {
            root.rail = Rail::Customize;
            root.overlay = None;
            cx.notify();
        },
    ));

    column = column.child(div().h(px(12.0)));

    column = column.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(26.0))
            .px_2()
            .child(
                div()
                    .flex_1()
                    .text_size(px(11.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.faint())
                    .child("Sources"),
            )
            .child(
                icon_button("details-add-source", Icon::Plus, 22.0, theme, false)
                    .tooltip(tip("Sources come from the project's mcp.json", theme))
                    .on_click(cx.listener(|root, _, _, cx| {
                        root.rail = Rail::Customize;
                        root.overlay = None;
                        cx.notify();
                    })),
            ),
    );

    if sources.is_empty() {
        column = column.child(
            div()
                .px_2()
                .py_1()
                .text_size(px(11.5))
                .text_color(theme.faint())
                .child("No MCP servers in this project"),
        );
    } else {
        for server in &sources {
            let name = server["name"].as_str().unwrap_or_default().to_string();
            let state = if !server["enabled"].as_bool().unwrap_or(true) {
                "Off".to_string()
            } else {
                match server["state"].as_str().unwrap_or_default() {
                    "connected" => "Connected".to_string(),
                    "needs-auth" => "Needs auth".to_string(),
                    "needs-trust" => "Needs trust".to_string(),
                    "" => "Ready".to_string(),
                    other => other.to_string(),
                }
            };
            let color = if state == "Connected" {
                theme.success()
            } else if state == "Off" {
                theme.faint()
            } else {
                theme.tool()
            };
            column = column.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(28.0))
                    .px_2()
                    .rounded(px(7.0))
                    .child(icons::icon(Icon::Globe, 14.0, theme.faint()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(12.0))
                            .text_color(theme.text())
                            .child(name),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(color)
                            .child(state),
                    ),
            );
        }
    }

    column = column.child(
        div()
            .id("details-view-all")
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(28.0))
            .px_2()
            .rounded(px(7.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(theme.dim())
            .hover(move |style| style.bg(theme.panel()).text_color(theme.text()))
            .child("View all")
            .on_click(cx.listener(|root, _, _, cx| {
                root.rail = Rail::Customize;
                root.overlay = None;
                cx.notify();
            })),
    );

    column
}

fn detail_row(
    id: &'static str,
    mark: Icon,
    label: &str,
    trailing: Option<Vec<(String, gpui::Hsla)>>,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> Stateful<gpui::Div> {
    let mut row = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .h(px(30.0))
        .px_2()
        .rounded(px(7.0))
        .cursor_pointer()
        .text_size(px(12.5))
        .text_color(theme.dim())
        .hover(move |style| style.bg(theme.panel()).text_color(theme.text()))
        .child(icons::icon(mark, 15.0, theme.faint()))
        .child(div().flex_1().child(label.to_string()));
    if let Some(values) = trailing {
        for (value, color) in values {
            row = row.child(
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(color)
                    .child(value),
            );
        }
    }
    row.on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

fn changes_pill(
    files: usize,
    added: u64,
    removed: u64,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    div()
        .id("changes-pill")
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .flex_none()
        .h(px(28.0))
        .px(px(12.0))
        .rounded(px(14.0))
        .bg(theme.panel())
        .border_1()
        .border_color(theme.border())
        .cursor_pointer()
        .text_size(px(11.5))
        .text_color(theme.dim())
        .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
        .tooltip(tip(
            "Open the changes pane \u{00b7} \u{2318}\u{23ce}",
            theme,
        ))
        .child(if files == 1 {
            "1 file changed".to_string()
        } else {
            format!("{files} files changed")
        })
        .child(
            div()
                .flex_none()
                .text_color(theme.success())
                .child(format!("+{added}")),
        )
        .child(
            div()
                .flex_none()
                .text_color(theme.error())
                .child(format!("\u{2212}{removed}")),
        )
        .on_click(cx.listener(|root, _, _, cx| {
            root.show_changes = !root.show_changes;
            let index = root.active;
            if root.show_changes {
                root.refresh_repository(index, cx);
            }
            cx.notify();
        }))
}

pub fn main(
    root: &mut Root,
    theme: &Theme,
    scale: f32,
    window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let mut column = div()
        .flex()
        .flex_row()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0));

    if root.rail == Rail::Customize {
        column = column.child(customize_page(root, theme, cx));
        return column;
    }

    column = column.child(conversation(root, theme, scale, window, cx));
    if root.show_details {
        column = column.child(details(root, theme, cx));
    }
    if root.show_changes {
        column = column.child(changes_panel(root, theme, cx));
    }
    column
}

fn conversation(
    root: &mut Root,
    theme: &Theme,
    scale: f32,
    window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let mut pane = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .bg(theme.background());

    if root.chats.is_empty() {
        pane = pane.child(empty_state(root, theme, cx));
        return pane;
    }

    let index = root.active;
    let trust = root.info["trust"].clone();
    // Trust is the object `project_info` answers with, not a flag: the prompt to
    // grant it belongs to a project whose own resources are still unloaded.
    let untrusted =
        trust["required"].as_bool().unwrap_or(false) && !trust["trusted"].as_bool().unwrap_or(true);
    let mut scroll = gpui::ScrollHandle::new();
    std::mem::swap(&mut scroll, &mut root.chats[index].scroll);
    let scroll_to_end = root.chats[index].scroll_to_end;
    let entries = root.chats[index].entries.clone();

    let mut transcript = div()
        .id("transcript")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .track_scroll(&scroll)
        .px(px(28.0))
        .pt(px(38.0))
        .pb(px(16.0))
        .gap(px(24.0));

    if untrusted {
        let path = root.chats[index].project.clone();
        transcript = transcript.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .p_3()
                .rounded(px(10.0))
                .bg(theme.wash("accent", 0.18))
                .border_1()
                .border_color(theme.border())
                .child(line(
                    "This project is not trusted, so its own skills, plugins and MCP servers are not loaded.",
                    theme,
                ))
                .child(Button::new("Trust project").render(
                    "trust-project",
                    theme,
                    cx,
                    move |root, _, cx| {
                        root.open_overlay(
                            Overlay::Trust {
                                project: path.clone(),
                            },
                            cx,
                        )
                    },
                )),
        );
    }

    if entries.is_empty() {
        transcript = transcript.child(empty_state(root, theme, cx));
    }

    for (position, entry) in entries.iter().enumerate() {
        transcript = transcript.child(
            div()
                .w_full()
                .max_w(px(780.0))
                .mx_auto()
                .child(entry_view(entry, index, position, theme, cx)),
        );
    }

    let composer_editor = root.editors.get(&root.chats[index].id).cloned();
    let editor_height = composer_editor
        .as_ref()
        .map(|editor| editor.read(cx).height())
        .unwrap_or(px(20.0));

    let mut pane_body = div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .child(transcript);

    // What the work tree holds right now, on the composer's own top edge: the
    // page's chip, and the way into the changes pane.
    let totals = root.chats[index]
        .changes
        .clone()
        .unwrap_or(serde_json::Value::Null);
    let files = totals["files"].as_array().map(Vec::len).unwrap_or(0);
    if files > 0 {
        pane_body = pane_body.child(
            div()
                .flex()
                .flex_row()
                .justify_center()
                .w_full()
                .pb(px(10.0))
                .child(changes_pill(
                    files,
                    totals["added"].as_u64().unwrap_or(0),
                    totals["removed"].as_u64().unwrap_or(0),
                    theme,
                    cx,
                )),
        );
    }

    if scroll_to_end {
        scroll.scroll_to_bottom();
        root.chats[index].scroll_to_end = false;
    }

    pane_body = pane_body.child(composer(root, theme, scale, editor_height, window, cx));
    pane = pane.child(pane_body);
    root.chats[index].scroll = scroll;
    pane
}

/// The page the window opens on before a thread is in the transcript: the mark,
/// the invitation and — once a project is open — the four suggestions, which put
/// their text in the composer to edit rather than sending it.
const SUGGESTIONS: [&str; 4] = [
    "Explain this codebase and its architecture.",
    "Find and fix the highest-priority bug in this repository.",
    "Add tests for the most important untested code path.",
    "Review the working tree changes and summarize the risks.",
];

fn empty_state(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let project = root.selected_project().cloned();
    let mut page = div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .w_full()
        .max_w(px(640.0))
        .mx_auto()
        .pb(px(32.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .w(px(64.0))
                .h(px(64.0))
                .rounded(px(18.0))
                .bg(theme.well())
                .child(icons::mark(40.0)),
        )
        .child(
            div()
                .pt(px(16.0))
                .text_size(px(28.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text())
                .child("What should we build?"),
        );

    match project {
        Some(project) => {
            page = page
                .child(
                    div()
                        .pt(px(8.0))
                        .text_size(px(12.5))
                        .text_color(theme.faint())
                        .child(project.display_path),
                )
                .child(div().pt(px(22.0)).w_full().child(suggestions(theme, cx)));
        }
        None => {
            page =
                page.child(
                    div()
                        .pt(px(8.0))
                        .text_size(px(12.5))
                        .text_color(theme.dim())
                        .child("Open a folder to start a chat in it."),
                )
                .child(div().pt(px(22.0)).child(
                    Button::new("Add project").primary().render(
                        "empty-add",
                        theme,
                        cx,
                        |root, _, cx| root.add_project(cx),
                    ),
                ));
        }
    }
    page
}

fn suggestions(theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let mut grid = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .justify_center()
        .gap_3()
        .pt_1();
    for text in SUGGESTIONS {
        grid = grid.child(
            div()
                .id(SharedString::from(format!("suggestion-{text}")))
                .flex()
                .items_center()
                .w(px(314.0))
                .min_h(px(76.0))
                .p(px(16.0))
                .rounded(px(14.0))
                .border_1()
                .border_color(theme.border())
                .bg(theme.panel())
                .text_size(px(13.0))
                .line_height(px(18.0))
                .text_color(theme.dim())
                .cursor_pointer()
                .hover(move |style| {
                    style
                        .border_color(theme.dim())
                        .bg(theme.panel_2())
                        .text_color(theme.text())
                })
                .child(text)
                .on_click(cx.listener(move |root, _, window, cx| {
                    root.fill_composer(text.to_string(), window, cx)
                })),
        );
    }
    grid
}

/// The card a turn leaves behind when it changed files: the count, the totals,
/// the first few files it touched, and the two things a reader wants next —
/// putting them back, or reading them in the changes pane.
fn edited_card(
    card: &crate::model::EditedCard,
    chat: usize,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let count = card.files.len();
    let mut head = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .w(px(26.0))
                .h(px(26.0))
                .rounded(px(7.0))
                .bg(theme.panel_3())
                .child(icons::icon(Icon::File, 14.0, theme.dim())),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text())
                .child(if count == 1 {
                    "Edited 1 file".to_string()
                } else {
                    format!("Edited {count} files")
                }),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(11.5))
                .text_color(theme.success())
                .child(format!("+{}", card.added)),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(11.5))
                .text_color(theme.error())
                .child(format!("\u{2212}{}", card.removed)),
        );

    if card.undone {
        head = head.child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(theme.faint())
                .child("Undone"),
        );
    } else if card.latest {
        head = head.child(
            div()
                .id(("edited-undo", chat))
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_none()
                .h(px(26.0))
                .px_2()
                .rounded(px(6.0))
                .cursor_pointer()
                .border_1()
                .border_color(theme.border())
                .text_size(px(11.5))
                .text_color(theme.text())
                .hover(move |style| style.bg(theme.panel_3()))
                .tooltip(tip("Put this turn's files back", theme))
                .child("Undo")
                .child(icons::icon(Icon::Back, 12.0, theme.dim()))
                .on_click(cx.listener(|root, _, _, cx| root.undo_turn(cx))),
        );
    }
    head = head.child(
        div()
            .id(("edited-view", chat))
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(px(26.0))
            .px_2()
            .rounded(px(6.0))
            .cursor_pointer()
            .border_1()
            .border_color(theme.border())
            .text_size(px(11.5))
            .text_color(theme.text())
            .hover(move |style| style.bg(theme.panel_3()))
            .child("View changes")
            .on_click(cx.listener(|root, _, _, cx| {
                root.show_changes = true;
                let index = root.active;
                root.refresh_repository(index, cx);
                cx.notify();
            })),
    );

    let mut block = div()
        .flex()
        .flex_col()
        .gap_2()
        .p_3()
        .rounded(px(12.0))
        .border_1()
        .border_color(theme.border())
        .bg(theme.panel())
        .child(head);

    let shown = card.files.len().min(6);
    for file in card.files.iter().take(shown) {
        block = block.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .w(px(12.0))
                        .text_size(px(11.0))
                        .text_color(theme.faint())
                        .child(status_letter(&file.status)),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_family("Menlo")
                        .text_size(px(11.5))
                        .text_color(theme.dim())
                        .child(file.path.clone()),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(theme.success())
                        .child(format!("+{}", file.added)),
                )
                .child(
                    div()
                        .flex_none()
                        .text_size(px(11.0))
                        .text_color(theme.error())
                        .child(format!("\u{2212}{}", file.removed)),
                ),
        );
    }
    if card.files.len() > shown {
        block = block.child(
            div()
                .text_size(px(11.5))
                .text_color(theme.faint())
                .child(format!("and {} more", card.files.len() - shown)),
        );
    }
    block
}

/// `status_letter` names a change the way git does: added, modified, deleted.
fn status_letter(status: &str) -> &'static str {
    match status {
        "added" => "A",
        "deleted" => "D",
        "renamed" => "R",
        _ => "M",
    }
}

fn entry_view(
    entry: &Entry,
    chat: usize,
    _position: usize,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> gpui::Div {
    match entry {
        Entry::User { text, attachments } => {
            let mut card = div()
                .flex()
                .flex_col()
                .gap_2()
                .px(px(15.0))
                .py(px(11.0))
                .rounded(px(18.0))
                .bg(theme.panel_2())
                .max_w(px(676.0))
                .child(
                    div()
                        .text_size(px(13.5))
                        .line_height(px(21.0))
                        .text_color(theme.text())
                        .child(text.clone()),
                );
            if !attachments.is_empty() {
                card = card.child(
                    div().flex().flex_row().gap_1().children(
                        attachments
                            .iter()
                            .map(|name| pill(name.clone(), theme, false)),
                    ),
                );
            }
            div().flex().flex_row().w_full().justify_end().child(card)
        }
        Entry::Assistant {
            blocks,
            streaming,
            text,
        } => {
            let mut column = div().flex().flex_col().gap_2().child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(11.0))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme.assistant())
                            .child("Oxide"),
                    )
                    .children(streaming.then(|| pill("streaming", theme, true))),
            );
            if blocks.is_empty() && text.is_empty() {
                column = column.child(
                    div()
                        .text_size(px(13.0))
                        .text_color(theme.faint())
                        .child("Thinking…"),
                );
            }
            for block in blocks {
                column = column.child(block_view(block, theme));
            }
            column
        }
        Entry::Thinking {
            text,
            seconds,
            done,
        } => {
            let label = match (done, seconds) {
                (true, Some(seconds)) => format!("Thought for {seconds}s"),
                (true, None) => "Thought".to_string(),
                (false, _) => "Thinking…".to_string(),
            };
            div()
                .flex()
                .flex_col()
                .gap_1()
                .pl_2()
                .border_l_1()
                .border_color(theme.border())
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.thinking_text())
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(19.0))
                        .text_color(theme.thinking_text())
                        .child(text.clone()),
                )
        }
        Entry::Tool(card) => {
            let (color, state) = match card.status {
                Status::Pending => (theme.faint(), "Queued"),
                Status::Running => (theme.accent(), "Running"),
                Status::Done => (theme.success(), ""),
                Status::Failed => (theme.error(), "Failed"),
            };
            // A finished call is a line of text, the way the page draws it: no
            // panel, no border, just the name, what it acted on and its output.
            // A running or failed one is the only card that gets a surface.
            let mut block = div().flex().flex_col().py(px(4.0));
            if matches!(card.status, Status::Running | Status::Failed) {
                block = block
                    .px(px(12.0))
                    .py(px(9.0))
                    .rounded(px(12.0))
                    .border_l_2()
                    .border_color(color)
                    .bg(if card.status == Status::Failed {
                        theme.color("tool_error_bg")
                    } else {
                        theme.color("tool_pending_bg")
                    });
            }
            block = block.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text())
                            .child(card.name.clone()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .font_family("Menlo")
                            .text_size(px(11.5))
                            .text_color(theme.faint())
                            .child(card.detail.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(color)
                            .child(state.to_string()),
                    ),
            );
            if !card.output.trim().is_empty() {
                block = block.child(
                    div()
                        .id(SharedString::from(format!("tool-output-{}", card.id)))
                        .mt(px(6.0))
                        .max_h(px(110.0))
                        .overflow_hidden()
                        .text_size(px(11.5))
                        .line_height(px(17.0))
                        .font_family("Menlo")
                        .text_color(theme.dim())
                        .child(card.output.chars().take(6000).collect::<String>()),
                );
            }
            if let Some(diff) = &card.diff {
                if let Some(text) = diff["text"].as_str().or_else(|| diff.as_str()) {
                    block = block.child(diff_block(text, theme));
                }
            }
            block
        }
        Entry::Edited(card) => edited_card(card, chat, theme, cx),
        Entry::Error(message) => div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(10.0))
            .bg(theme.color("tool_error_bg"))
            .border_1()
            .border_color(theme.error())
            .child(
                div()
                    .text_size(px(11.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.error())
                    .child("Error"),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme.text())
                    .child(message.clone()),
            ),
        Entry::Info(message) => div()
            .text_size(px(12.0))
            .text_color(theme.faint())
            .child(message.clone()),
        Entry::Compaction {
            summary,
            summarized,
            tokens,
        } => div()
            .flex()
            .flex_col()
            .gap_1()
            .px_3()
            .py_2()
            .rounded(px(10.0))
            .bg(theme.panel())
            .border_1()
            .border_color(theme.border())
            .child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme.info())
                    .child(format!("Compacted {summarized} messages ({tokens} tokens)")),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme.dim())
                    .child(summary.clone()),
            ),
        Entry::Approval(card) => {
            let mut block = div()
                .flex()
                .flex_col()
                .gap_2()
                .px_3()
                .py_2()
                .rounded(px(10.0))
                .bg(theme.wash("accent", 0.2))
                .border_1()
                .border_color(theme.border())
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.accent())
                        .child(format!("Approval needed: {}", card.tool)),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.dim())
                        .child(card.detail.clone()),
                );
            match card.decision.as_deref() {
                Some(decision) => {
                    block = block.child(div().text_size(px(12.0)).text_color(theme.faint()).child(
                        match decision {
                            "once" => "Allowed once".to_string(),
                            "always" => "Always allowed for this project".to_string(),
                            "deny" => "Denied".to_string(),
                            other => format!("Closed ({other})"),
                        },
                    ));
                }
                None => {
                    let id = card.id;
                    block = block.child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .child(Button::new("Allow once").primary().render(
                                ("approve-once", id),
                                theme,
                                cx,
                                move |root, _, cx| root.resolve_approval(id, "once", cx),
                            ))
                            .child(Button::new("Always").render(
                                ("approve-always", id),
                                theme,
                                cx,
                                move |root, _, cx| root.resolve_approval(id, "always", cx),
                            ))
                            .child(Button::new("Deny").danger().render(
                                ("approve-deny", id),
                                theme,
                                cx,
                                move |root, _, cx| root.resolve_approval(id, "deny", cx),
                            )),
                    );
                }
            }
            block
        }
        Entry::Question(card) => {
            let count = card.questions.as_array().map(Vec::len).unwrap_or(0);
            let mut block = div()
                .flex()
                .flex_col()
                .gap_2()
                .px_3()
                .py_2()
                .rounded(px(10.0))
                .bg(theme.panel())
                .border_1()
                .border_color(theme.border())
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.accent())
                        .child(if count == 1 {
                            "The model asked a question".to_string()
                        } else {
                            format!("The model asked {count} questions")
                        }),
                );
            if card.answered {
                block = block.child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.faint())
                        .child("Answered"),
                );
            } else {
                let id = card.id;
                block = block.child(Button::new("Answer").primary().render(
                    ("answer-question", id),
                    theme,
                    cx,
                    move |root, _, cx| root.open_overlay(Overlay::Question { chat, id }, cx),
                ));
            }
            block
        }
    }
}

fn block_view(block: &Block, theme: &Theme) -> gpui::Div {
    match block {
        Block::Paragraph(spans) => div()
            .text_size(px(13.5))
            .line_height(px(22.0))
            .child(styled(theme, spans)),
        Block::Heading { level, spans } => div()
            .pt_1()
            .text_size(px(match level {
                1 => 18.0,
                2 => 16.0,
                _ => 14.5,
            }))
            .line_height(px(24.0))
            .font_weight(FontWeight::SEMIBOLD)
            .child(styled(theme, spans)),
        Block::Code { language, text } => {
            let label = if language.is_empty() {
                "code".to_string()
            } else {
                language.clone()
            };
            div()
                .flex()
                .flex_col()
                .rounded(px(9.0))
                .bg(theme.panel())
                .border_1()
                .border_color(theme.border())
                .child(
                    div()
                        .px_2()
                        .py_1()
                        .text_size(px(10.5))
                        .text_color(theme.faint())
                        .child(label),
                )
                .child(
                    div()
                        .px_3()
                        .py_2()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .font_family("Menlo")
                        .text_color(theme.text())
                        .child(text.clone()),
                )
        }
        Block::Quote(spans) => div()
            .pl_3()
            .border_l_2()
            .border_color(theme.border())
            .text_size(px(13.0))
            .line_height(px(20.0))
            .text_color(theme.dim())
            .child(styled(theme, spans)),
        Block::Bullet { depth, spans } => div()
            .flex()
            .flex_row()
            .gap_2()
            .pl(px(*depth as f32 * 14.0))
            .text_size(px(13.5))
            .line_height(px(21.0))
            .child(div().text_color(theme.faint()).child("•"))
            .child(div().flex_1().child(styled(theme, spans))),
        Block::Numbered {
            depth,
            number,
            spans,
        } => div()
            .flex()
            .flex_row()
            .gap_2()
            .pl(px(*depth as f32 * 14.0))
            .text_size(px(13.5))
            .line_height(px(21.0))
            .child(div().text_color(theme.faint()).child(number.clone()))
            .child(div().flex_1().child(styled(theme, spans))),
        Block::Rule => crate::app::rule(theme),
    }
}

fn diff_block(text: &str, theme: &Theme) -> gpui::Div {
    let mut column = div()
        .flex()
        .flex_col()
        .rounded(px(8.0))
        .bg(theme.panel())
        .border_1()
        .border_color(theme.border())
        .p_2()
        .text_size(px(11.0))
        .line_height(px(16.0))
        .font_family("Menlo");
    for line in text.lines().take(200) {
        let trimmed = line.trim_start();
        let color = if trimmed.starts_with('+') {
            theme.success()
        } else if trimmed.starts_with('-') {
            theme.error()
        } else {
            theme.dim()
        };
        column = column.child(div().text_color(color).child(line.to_string()));
    }
    column
}

// ---------- composer ----------

fn composer(
    root: &mut Root,
    theme: &Theme,
    scale: f32,
    editor_height: Pixels,
    _window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let index = root.active;
    let chat = &root.chats[index];
    let editor = root.editors.get(&chat.id).cloned();
    let running = chat.running();
    let attachments = chat.attachments.clone();
    let project = root.project_name(&chat.project);
    let model = root.info["model"].as_str().unwrap_or("model").to_string();
    let reasoning = root.info["reasoning"].as_str().unwrap_or("off").to_string();

    let branch = root.branch.clone();

    let mut box_row = div()
        .key_context("Composer")
        .on_action(cx.listener(|root, _: &crate::app::Send, _window, cx| root.send(cx)))
        .on_action(cx.listener(|root, _: &crate::app::Stop, _window, cx| root.stop(cx)))
        .flex()
        .flex_col()
        .gap_1()
        .w_full()
        .px(px(13.0))
        .py(px(11.0))
        .bg(theme.panel_2())
        .border_1()
        .border_color(theme.border())
        .rounded(px(22.0));

    // The row on the box's top edge, the way the page's composer carries its
    // own: the folder the thread runs in and the branch it writes to. It sits
    // inside the box rather than above it, so the two edges read as one.
    box_row = box_row.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(24.0))
            .px_1()
            .child(icons::icon(Icon::Folder, 13.0, theme.faint()))
            .child(
                div()
                    .flex_none()
                    .text_size(px(11.5))
                    .text_color(theme.dim())
                    .child(project.clone()),
            )
            .children(branch.map(|branch| {
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(icons::icon(Icon::Branch, 13.0, theme.faint()))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.0))
                            .text_color(theme.faint())
                            .child(branch),
                    )
            }))
            .child(div().flex_1())
            .children(running.then(|| {
                div()
                    .flex_none()
                    .text_size(px(11.0))
                    .text_color(theme.accent())
                    .child("Working…")
            })),
    );

    if !attachments.is_empty() {
        box_row = box_row.child(
            div().flex().flex_row().flex_wrap().gap_2().px_1().children(
                attachments
                    .iter()
                    .enumerate()
                    .map(|(position, attachment)| {
                        let name = attachment.name.clone();
                        let image = attachment.data_url.starts_with("data:image");
                        div()
                            .id(("attachment", position))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .h(px(26.0))
                            .px(px(6.0))
                            .rounded(px(7.0))
                            .bg(theme.panel_3())
                            .text_size(px(11.5))
                            .text_color(theme.dim())
                            .cursor_pointer()
                            .child(icons::icon(
                                if image { Icon::Image } else { Icon::File },
                                14.0,
                                theme.faint(),
                            ))
                            .child(
                                div()
                                    .max_w(px(220.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(name),
                            )
                            .child(icons::icon(Icon::Close, 11.0, theme.faint()))
                            .on_click(cx.listener(move |root, _, _window, cx| {
                                if let Some(chat) = root.chats.get_mut(index) {
                                    chat.attachments.remove(position);
                                }
                                cx.notify();
                            }))
                    }),
            ),
        );
    }

    let input = match editor {
        Some(editor) => {
            let height = (f32::from(editor_height) + 6.0).clamp(26.0, 240.0);
            div().w_full().h(px(height * scale)).child(editor.clone())
        }
        None => div().h(px(28.0)),
    };

    box_row = box_row.child(input);

    let mut footer = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .w_full()
        .min_h(px(34.0));
    footer = footer
        .child(
            icon_button(
                ("composer-attach", 0usize),
                Icon::Paperclip,
                28.0,
                theme,
                false,
            )
            .tooltip(tip("Attach a file", theme))
            .on_click(cx.listener(|root, _, _, cx| root.attach_files(cx))),
        )
        .child(
            icon_button(("composer-tools", 1usize), Icon::More, 28.0, theme, false)
                .tooltip(tip("More tools", theme))
                .on_click(cx.listener(|root, _, _, cx| {
                    root.rail = Rail::Customize;
                    root.overlay = None;
                    cx.notify();
                })),
        )
        .child(
            div()
                .id("composer-approvals")
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(px(28.0))
                .px_2()
                .rounded(px(8.0))
                .cursor_pointer()
                .text_size(px(11.5))
                .text_color(theme.dim())
                .hover(move |style| style.bg(theme.panel_3()).text_color(theme.text()))
                .tooltip(tip("Tool approvals", theme))
                .child(icons::icon(Icon::Lock, 13.0, theme.faint()))
                .child("Tool approvals")
                .on_click(cx.listener(|root, _, _, cx| root.open_overlay(Overlay::Approvals, cx))),
        )
        .child(div().flex_1());

    footer = footer.child(setting_chip(
        ("chip-model", 1usize),
        model.clone(),
        theme,
        cx,
        |root, _, cx| root.open_overlay(Overlay::Models, cx),
    ));
    footer = footer.child(setting_chip(
        ("chip-reasoning", 2usize),
        capitalised(&reasoning),
        theme,
        cx,
        |root, _, cx| {
            let levels = root.info["reasoningLevels"]
                .as_array()
                .map(|levels| {
                    levels
                        .iter()
                        .filter_map(|level| level.as_str().map(str::to_string))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let next = levels
                .iter()
                .position(|level| level == &reasoning_level(root))
                .map(|position| levels[(position + 1) % levels.len()].clone())
                .unwrap_or_else(|| "off".to_string());
            root.set_reasoning(next, cx);
        },
    ));

    if running {
        footer = footer.child(
            div()
                .id("composer-stop")
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .w(px(28.0))
                .h(px(28.0))
                .rounded(px(14.0))
                .cursor_pointer()
                .bg(theme.panel_3())
                .hover(move |style| style.bg(theme.panel()))
                .tooltip(tip("Stop the turn", theme))
                .child(icons::icon(Icon::Square, 14.0, theme.text()))
                .on_click(cx.listener(|root, _, _, cx| root.stop(cx))),
        );
    } else {
        footer = footer.child(
            div()
                .id("composer-send")
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .w(px(28.0))
                .h(px(28.0))
                .rounded(px(14.0))
                .cursor_pointer()
                .bg(theme.text())
                .hover(move |style| style.bg(theme.accent()))
                .tooltip(tip("Send \u{00b7} \u{23ce}", theme))
                .child(icons::icon(Icon::ArrowUp, 15.0, theme.background()))
                .on_click(cx.listener(|root, _, window, cx| {
                    root.send(cx);
                    root.focus_composer(window, cx);
                })),
        );
    }

    box_row = box_row.child(footer);

    if let Some(usage) = &chat.usage {
        box_row = box_row.child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_2()
                .text_size(px(10.5))
                .text_color(theme.faint())
                .child(format!(
                    "{} in · {} out · {} cache read · {} cache write · ${:.2} · {} calls · {} context",
                    usage.input,
                    usage.output,
                    usage.cache_read,
                    usage.cache_write,
                    usage.cost,
                    usage.model_calls,
                    usage.context_tokens
                )),
        );
    }
    if let Some(tokens) = chat.context_tokens {
        box_row = box_row.child(div().text_size(px(10.5)).text_color(theme.faint()).child(
            format!(
                "context {tokens} tokens{}",
                if chat.context_model.is_empty() {
                    String::new()
                } else {
                    format!(" · {} / {}", chat.context_provider, chat.context_model)
                }
            ),
        ));
    }

    div()
        .flex()
        .flex_col()
        .flex_none()
        .w_full()
        .items_center()
        .px(px(28.0))
        .pb(px(14.0))
        .pt(px(16.0))
        .child(div().w_full().max_w(px(820.0)).child(box_row))
}

/// The label a reasoning level wears: the stored name is `high`, the chip says
/// `High`, the way the page prints it.
fn capitalised(label: &str) -> String {
    let mut characters = label.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
}

fn reasoning_level(root: &Root) -> String {
    root.info["reasoning"].as_str().unwrap_or("off").to_string()
}

fn setting_chip(
    id: impl Into<gpui::ElementId>,
    label: impl Into<String>,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_none()
        .h(px(26.0))
        .px(px(7.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(px(11.5))
        .text_color(theme.dim())
        .hover(move |style| style.bg(theme.panel_3()).text_color(theme.text()))
        .child(label.into())
        .child(icons::icon(Icon::ChevronDown, 12.0, theme.faint()))
        .on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

// ---------- changes ----------

fn changes_panel(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let changes = root.changes.clone();
    let message = root.chats.get(root.active).and_then(|chat| {
        chat.changes.as_ref().map(|changes| {
            (
                changes["files"].as_array().map(Vec::len).unwrap_or(0),
                changes["added"].as_u64().unwrap_or(0),
                changes["removed"].as_u64().unwrap_or(0),
            )
        })
    });

    let mut column = div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(380.0))
        .h_full()
        .bg(theme.panel())
        .border_l_1()
        .border_color(theme.border());

    let mut header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .border_b_1()
        .border_color(theme.border())
        .child(
            div()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Changes"),
        );
    if let Some((files, added, removed)) = message {
        header = header
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.dim())
                    .child(format!("{files} files")),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.success())
                    .child(format!("+{added}")),
            )
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.error())
                    .child(format!("-{removed}")),
            );
    }
    if root
        .chats
        .get(root.active)
        .and_then(|chat| chat.baseline.clone())
        .is_some()
    {
        header =
            header.child(
                Button::new("Undo")
                    .render("changes-undo", theme, cx, |root, _, cx| root.undo_turn(cx)),
            );
    }
    header = header.child(div().flex_1()).child(
        icon_button("changes-close", Icon::Close, 26.0, theme, false).on_click(cx.listener(
            |root, _, _, cx| {
                root.show_changes = false;
                cx.notify();
            },
        )),
    );
    column = column.child(header);

    let mut body = div()
        .id("changes-body")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .p_2()
        .gap_2();

    match changes {
        Some(value) => {
            let files = value["files"].as_array().cloned().unwrap_or_default();
            if files.is_empty() {
                body = body.child(
                    div()
                        .p_2()
                        .text_size(px(12.0))
                        .text_color(theme.faint())
                        .child("No uncommitted changes"),
                );
            }
            for file in files {
                let path = file["path"].as_str().unwrap_or_default().to_string();
                let added = file["added"].as_u64().unwrap_or(0);
                let removed = file["removed"].as_u64().unwrap_or(0);
                let diff = file["diff"].as_str().unwrap_or_default().to_string();
                let binary = file["binary"].as_bool().unwrap_or(false);
                let mut card = div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(px(8.0))
                    .bg(theme.background())
                    .border_1()
                    .border_color(theme.border())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(11.5))
                                    .child(path),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme.success())
                                    .child(format!("+{added}")),
                            )
                            .child(
                                div()
                                    .text_size(px(11.0))
                                    .text_color(theme.error())
                                    .child(format!("-{removed}")),
                            ),
                    );
                if binary {
                    card = card.child(
                        div()
                            .text_size(px(11.0))
                            .text_color(theme.faint())
                            .child("binary file"),
                    );
                } else if !diff.is_empty() {
                    card = card.child(diff_block(&diff, theme));
                }
                body = body.child(card);
            }
        }
        None => {
            body = body.child(
                div()
                    .p_2()
                    .text_size(px(12.0))
                    .text_color(theme.faint())
                    .child("Reading the work tree…"),
            );
        }
    }

    column.child(body)
}

// ---------- customize ----------

fn customize_page(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let project = root
        .selected_project()
        .map(|project| project.display_path.clone())
        .unwrap_or_else(|| "No project selected".to_string());
    let mut rows: Vec<(String, String)> = vec![
        (
            "Plugins".to_string(),
            "Installed plugins extend the agent: skills, tools and MCP servers.".to_string(),
        ),
        (
            "Skills".to_string(),
            "Each plugin's skills load from the project's own trust, as in the terminal."
                .to_string(),
        ),
        (
            "MCP servers".to_string(),
            "Configured servers are shared through the same config the CLI reads.".to_string(),
        ),
        (
            "Approvals".to_string(),
            "Tools allowed for this project, kept in the shared approval store.".to_string(),
        ),
    ];
    rows.push((
        "Appearance".to_string(),
        format!(
            "Themes are the CLI's: {} available.",
            root.theme_names.len()
        ),
    ));

    div()
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .id("customize-page")
        .overflow_y_scroll()
        .p_6()
        .gap_3()
        .child(
            div()
                .text_size(px(20.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Customize"),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(theme.dim())
                .child(format!("Loading from {project}")),
        )
        .children(rows.into_iter().map(|(title, body)| {
            crate::app::card(theme)
                .flex()
                .flex_col()
                .gap_1()
                .p_3()
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.dim())
                        .child(body),
                )
        }))
        .children(
            root.approvals
                .iter()
                .map(|tool| pill(tool.clone(), theme, false)),
        )
        .children((!root.approvals.is_empty()).then(|| {
            Button::new("Clear allowed tools").danger().render(
                "clear-approvals",
                theme,
                cx,
                |root, _, cx| root.clear_approvals(cx),
            )
        }))
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(Button::new("Themes").render(
                    "customize-themes",
                    theme,
                    cx,
                    |root, _, cx| root.open_overlay(Overlay::Themes, cx),
                ))
                .child(Button::new("Language").render(
                    "customize-language",
                    theme,
                    cx,
                    |root, _, cx| root.open_overlay(Overlay::Language, cx),
                ))
                .child(Button::new("Settings").render(
                    "customize-settings",
                    theme,
                    cx,
                    |root, _, cx| root.open_overlay(Overlay::Settings, cx),
                )),
        )
}
