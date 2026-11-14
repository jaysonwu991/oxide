//! The window's panels: the rail, the sidebar, the transcript, the composer and
//! the changes pane.
//!
//! They draw the same furniture the Electron window does — one topbar with the
//! chat tabs, a navigation rail, the project/session sidebar, the conversation
//! and its composer, and the changes a turn left behind — from the same state
//! `crate::app::Root` keeps.

use crate::app::{Button, Overlay, Rail, Root};
use crate::markdown::{Block, Inline};
use crate::model::{Entry, Status};
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{
    div, px, Context, Font, FontStyle, FontWeight, MouseButton, Pixels, SharedString, Stateful,
    StrikethroughStyle, StyledText, TextRun, UnderlineStyle, Window,
};

// ---------- shared pieces ----------

fn glyph_button(
    id: impl Into<gpui::ElementId>,

    glyph: &'static str,
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
        .w(px(30.0))
        .h(px(30.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .text_color(color)
        .hover(move |style| style.bg(theme.panel_2()))
        .child(div().text_size(px(15.0)).child(glyph))
}

fn pill(text: impl Into<String>, theme: &Theme, accent: bool) -> gpui::Div {
    div()
        .flex()
        .items_center()
        .h(px(20.0))
        .px_2()
        .rounded(px(10.0))
        .bg(if accent {
            theme.wash("accent", 0.3)
        } else {
            theme.panel_3()
        })
        .text_color(if accent { theme.accent() } else { theme.dim() })
        .text_size(px(11.0))
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

// ---------- topbar ----------

pub fn topbar(
    root: &mut Root,
    theme: &Theme,
    scale: f32,
    _window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .h(px(46.0 * scale))
        .px_2()
        .gap_1()
        .bg(theme.sidebar())
        .border_b_1()
        .border_color(theme.border());

    let back =
        glyph_button("nav-back", "←", theme, false).on_click(cx.listener(|root, _, _, cx| {
            if let Some(previous) = root.history.pop() {
                root.forward.push(root.active);
                root.active = previous.min(root.chats.len().saturating_sub(1));
                cx.notify();
            }
        }));
    let forward =
        glyph_button("nav-forward", "→", theme, false).on_click(cx.listener(|root, _, _, cx| {
            if let Some(next) = root.forward.pop() {
                root.history.push(root.active);
                root.active = next.min(root.chats.len().saturating_sub(1));
                cx.notify();
            }
        }));
    let toggle =
        glyph_button("nav-sidebar", "☰", theme, false).on_click(cx.listener(|root, _, _, cx| {
            root.sidebar_visible = !root.sidebar_visible;
            let visible = root.sidebar_visible;
            root.save_prefs(serde_json::json!({ "sidebar_visible": visible }), cx);
            cx.notify();
        }));
    row = row.child(back).child(forward).child(toggle);

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
            glyph_button(("close-tab", index), "×", theme, false)
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
    row = row.child(tabs);

    let new_chat = glyph_button("new-chat", "＋", theme, false)
        .on_click(cx.listener(|root, _, window, cx| root.new_chat(window, cx)));
    let changes = glyph_button("changes", "±", theme, root.show_changes).on_click(cx.listener(
        |root, _, _, cx| {
            root.show_changes = !root.show_changes;
            if root.show_changes {
                let index = root.active;
                root.refresh_repository(index, cx);
            }
            cx.notify();
        },
    ));
    let beta = pill("BETA", theme, true);
    row.child(new_chat)
        .child(changes)
        .child(div().flex_none().w(px(6.0)))
        .child(beta)
}

// ---------- rail ----------

pub fn rail(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let chats = root.rail == Rail::Chats;
    let customize = root.rail == Rail::Customize;
    div()
        .flex()
        .flex_col()
        .items_center()
        .flex_none()
        .w(px(48.0))
        .h_full()
        .py_2()
        .gap_1()
        .bg(theme.sidebar())
        .border_r_1()
        .border_color(theme.border())
        .child(
            glyph_button("rail-new", "＋", theme, false)
                .on_click(cx.listener(|root, _, window, cx| root.new_chat(window, cx))),
        )
        .child(
            glyph_button("rail-chats", "☰", theme, chats).on_click(cx.listener(
                |root, _, _, cx| {
                    root.rail = Rail::Chats;
                    root.overlay = None;
                    cx.notify();
                },
            )),
        )
        .child(
            glyph_button("rail-customize", "◇", theme, customize).on_click(cx.listener(
                |root, _, _, cx| {
                    root.rail = Rail::Customize;
                    cx.notify();
                },
            )),
        )
        .child(div().flex_1())
        .child(
            glyph_button("rail-settings", "⚙", theme, false)
                .on_click(cx.listener(|root, _, _, cx| root.open_overlay(Overlay::Settings, cx))),
        )
}

// ---------- sidebar ----------

pub fn sidebar(
    root: &mut Root,
    theme: &Theme,
    scale: f32,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let width = root.sidebar_width;
    let search = root.search.clone();
    let height = px(scale * 20.0).max(px(20.0));

    let mut column = div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(width))
        .h_full()
        .min_h(px(0.0))
        .bg(theme.sidebar());

    column = column.child(
        div().flex().flex_col().px_3().pt_3().pb_2().child(
            div()
                .flex()
                .items_center()
                .h(px(30.0))
                .px_2()
                .rounded(px(8.0))
                .bg(theme.panel())
                .border_1()
                .border_color(theme.border())
                .child(div().h(height).w_full().child(search)),
        ),
    );

    let mut list = div()
        .id("sidebar-list")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll()
        .px_2()
        .pb_2()
        .gap_1();

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
                .h(px(28.0))
                .px_2()
                .rounded(px(7.0))
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel()))
                .child(
                    div()
                        .flex_none()
                        .w(px(10.0))
                        .text_size(px(10.0))
                        .text_color(theme.faint())
                        .child(if expanded { "▾" } else { "▸" }),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.5))
                        .text_color(theme.dim())
                        .child(name),
                )
                .children(pinned.then(|| pill("★", theme, false)))
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
                    glyph_button(
                        SharedString::from(format!("project-new-{path}")),
                        "＋",
                        theme,
                        false,
                    )
                    .on_click(cx.listener(move |root, _, window, cx| {
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
                    glyph_button(
                        SharedString::from(format!("project-remove-{path}")),
                        "⋯",
                        theme,
                        false,
                    )
                    .on_click(cx.listener(move |root, _, _window, cx| {
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
                            .pl(px(24.0))
                            .pr_2()
                            .py_1()
                            .text_size(px(12.0))
                            .text_color(theme.faint())
                            .child("No chats yet"),
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
    }

    column = column.child(list);
    column = column.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .px_2()
            .py_2()
            .border_t_1()
            .border_color(theme.border())
            .child(
                Button::new("Add project")
                    .render("add-project", theme, cx, |root, _, cx| root.add_project(cx)),
            )
            .child(
                Button::new("Archived").render("open-archived", theme, cx, |root, _, cx| {
                    root.open_overlay(Overlay::Archived, cx)
                }),
            )
            .child(div().flex_1())
            .child(pill("Beta", theme, false)),
    );
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

fn session_row(
    title: &str,
    when: &str,
    pinned: bool,
    theme: &Theme,
    cx: &mut Context<Root>,
    open: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> impl IntoElement {
    let title = title.to_string();
    let when = when.to_string();
    div()
        .id(SharedString::from(format!("session-{}", title.clone())))
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .h(px(28.0))
        .pl(px(24.0))
        .pr_2()
        .rounded(px(7.0))
        .cursor_pointer()
        .hover(move |style| style.bg(theme.panel()))
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
        .children(pinned.then(|| pill("★", theme, false)))
        .child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(theme.faint())
                .child(when),
        )
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
    let chat = &root.chats[index];
    let title = chat.title.clone();
    let project = root.project_name(&chat.project);
    let trust = root.info["trust"].clone();
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
        .px_4()
        .pt_4()
        .pb_2()
        .gap_4();

    transcript = transcript.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .pb_1()
            .child(
                div()
                    .text_size(px(17.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text())
                    .child(title),
            )
            .child(pill(project, theme, false)),
    );

    if !trust.as_bool().unwrap_or(true) {
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
        transcript = transcript.child(
            div()
                .py_8()
                .text_size(px(13.5))
                .text_color(theme.faint())
                .child("Ask Oxide to read, change or run something in this project."),
        );
    }

    for (position, entry) in entries.iter().enumerate() {
        transcript = transcript.child(entry_view(entry, index, position, theme, cx));
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

    if scroll_to_end {
        scroll.scroll_to_bottom();
        root.chats[index].scroll_to_end = false;
    }

    pane_body = pane_body.child(composer(root, theme, scale, editor_height, window, cx));
    pane = pane.child(pane_body);
    root.chats[index].scroll = scroll;
    pane
}

fn empty_state(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let project = root.selected_project().cloned();
    div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_3()
        .child(
            div()
                .text_size(px(22.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text())
                .child("Oxide Beta"),
        )
        .child(
            div()
                .text_size(px(13.0))
                .text_color(theme.dim())
                .child("A native GPUI window onto the same projects, chats and approvals."),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(Button::new("New chat").primary().render(
                    "empty-new",
                    theme,
                    cx,
                    |root, window, cx| root.new_chat(window, cx),
                ))
                .child(
                    Button::new("Add project")
                        .render("empty-add", theme, cx, |root, _, cx| root.add_project(cx)),
                ),
        )
        .children(project.map(|project| {
            div()
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child(format!("New chats open in {}", project.display_path))
        }))
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
                .px_3()
                .py_2()
                .rounded(px(12.0))
                .bg(theme.panel_2())
                .border_1()
                .border_color(theme.border())
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.user())
                        .child("You"),
                )
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
            card
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
            let (dot, color) = match card.status {
                Status::Pending => ("○", theme.faint()),
                Status::Running => ("◌", theme.accent()),
                Status::Done => ("●", theme.success()),
                Status::Failed => ("●", theme.error()),
            };
            let mut block = div()
                .flex()
                .flex_col()
                .gap_1()
                .px_3()
                .py_2()
                .rounded(px(10.0))
                .bg(theme.color("tool_pending_bg"))
                .border_1()
                .border_color(theme.border())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(div().text_size(px(10.0)).text_color(color).child(dot))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.tool())
                                .child(card.name.clone()),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(12.0))
                                .text_color(theme.dim())
                                .child(card.detail.clone()),
                        ),
                );
            if !card.output.trim().is_empty() {
                block = block.child(
                    div()
                        .id(SharedString::from(format!("tool-output-{}", card.id)))
                        .max_h(px(240.0))
                        .overflow_hidden()
                        .rounded(px(8.0))
                        .bg(theme.color("panel"))
                        .p_2()
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
            .line_height(px(21.0))
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
    let provider = root.info["provider"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    let reasoning = root.info["reasoning"].as_str().unwrap_or("off").to_string();

    let mut box_row = div()
        .key_context("Composer")
        .on_action(cx.listener(|root, _: &crate::app::Send, _window, cx| root.send(cx)))
        .on_action(cx.listener(|root, _: &crate::app::Stop, _window, cx| root.stop(cx)))
        .flex()
        .flex_col()
        .gap_2()
        .w_full()
        .p_3()
        .bg(theme.panel_2())
        .border_1()
        .border_color(theme.border())
        .rounded(px(18.0));

    if !attachments.is_empty() {
        box_row = box_row.child(
            div().flex().flex_row().flex_wrap().gap_1().children(
                attachments
                    .iter()
                    .enumerate()
                    .map(|(position, attachment)| {
                        let name = attachment.name.clone();
                        div()
                            .id(("attachment", position))
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_1()
                            .px_2()
                            .h(px(22.0))
                            .rounded(px(11.0))
                            .bg(theme.panel_3())
                            .text_size(px(11.5))
                            .text_color(theme.dim())
                            .child(name)
                            .child("×")
                            .cursor_pointer()
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

    let mut controls = div().flex().flex_row().items_center().gap_2().w_full();
    controls = controls
        .child(chip(
            ("chip-project", 0usize),
            project,
            theme,
            cx,
            |root, _, cx| {
                if let Some(project) = root.selected_project().cloned() {
                    root.overlay = Some(Overlay::Confirm {
                        title: "Project".to_string(),
                        body: format!("Chats in this window run in {}", project.display_path),
                        action: crate::app::ConfirmAction::RemoveProject(project.id.clone()),
                    });
                    cx.notify();
                }
            },
        ))
        .child(chip(
            ("chip-model", 1usize),
            model.clone(),
            theme,
            cx,
            |root, _, cx| root.open_overlay(Overlay::Models, cx),
        ));
    if !provider.is_empty() {
        controls = controls.child(
            div()
                .text_size(px(11.0))
                .text_color(theme.faint())
                .child(provider),
        );
    }
    controls = controls.child(chip(
        ("chip-attach", 3usize),
        "+ attach",
        theme,
        cx,
        |root, _, cx| root.attach_files(cx),
    ));
    controls = controls.child(chip(
        ("chip-reasoning", 2usize),
        format!("thinking: {reasoning}"),
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
    controls = controls.child(div().flex_1());

    if running {
        controls = controls.child(Button::new("Stop").danger().render(
            "composer-stop",
            theme,
            cx,
            |root, _, cx| root.stop(cx),
        ));
    } else {
        controls = controls.child(Button::new("Send").primary().render(
            "composer-send",
            theme,
            cx,
            |root, window, cx| {
                root.send(cx);
                root.focus_composer(window, cx);
            },
        ));
    }

    box_row = box_row.child(controls);

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
        .px_4()
        .pb_4()
        .pt_2()
        .child(
            div()
                .flex()
                .flex_row()
                .justify_center()
                .w_full()
                .child(div().w(px(820.0)).max_w_full().child(box_row)),
        )
}

fn reasoning_level(root: &Root) -> String {
    root.info["reasoning"].as_str().unwrap_or("off").to_string()
}

fn chip(
    id: impl Into<gpui::ElementId>,
    label: impl Into<String>,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .h(px(24.0))
        .px_2()
        .rounded(px(12.0))
        .bg(theme.panel_3())
        .text_size(px(11.5))
        .text_color(theme.dim())
        .cursor_pointer()
        .hover(move |style| style.bg(theme.panel()))
        .child(label.into())
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
        glyph_button("changes-close", "×", theme, false).on_click(cx.listener(|root, _, _, cx| {
            root.show_changes = false;
            cx.notify();
        })),
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
