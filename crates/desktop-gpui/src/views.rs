//! The window's panels: the rail, the sidebar, the transcript, the composer and
//! the changes pane.
//!
//! They draw the same furniture the Electron window does — one topbar with the
//! chat tabs, a navigation rail, the project/session sidebar, the conversation
//! and its composer, and the changes a turn left behind — from the same state
//! `crate::app::Root` keeps.

use crate::app::{Button, ConfirmAction, CustomizeTab, Overlay, Rail, Root};
use crate::icons::{self, Icon};
use crate::markdown::{Block, Inline};
use crate::model::{Entry, Status};
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{
    canvas, div, point, px, ClickEvent, Context, Focusable, Font, FontStyle, FontWeight,
    MouseButton, PathBuilder, Pixels, Render, SharedString, Stateful, StrikethroughStyle,
    StyledText, TextRun, UnderlineStyle, Window, WindowControlArea,
};

// ---------- shared pieces ----------

const RAIL_WIDTH: f32 = 56.0;
const TOPBAR_HEIGHT: f32 = 46.0;
const SIDEBAR_RESIZER_WIDTH: f32 = 7.0;
pub(crate) const SETTINGS_NAVIGATION_WIDTH: f32 = 232.0;

/// A square icon control: the window's own buttons in the topbar, the rail, a
/// card's header and a row's trailing actions. `box_size` is the target the
/// control fills; the glyph inside it is three fifths of that, the 18-in-30 the
/// page's `.icon` buttons use.
pub(crate) fn icon_button(
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
        .rounded(px(if box_size >= 38.0 { 10.0 } else { 8.0 }))
        .cursor_pointer()
        .text_color(color)
        .bg(if active {
            theme.panel_2()
        } else {
            gpui::transparent_black()
        })
        .hover(move |style| style.bg(theme.panel_2()))
        .child(icons::icon(mark, box_size * 0.6, color))
}

fn navigation_button(
    id: &'static str,
    mark: Icon,
    enabled: bool,
    theme: &Theme,
) -> Stateful<gpui::Div> {
    let color = if enabled { theme.dim() } else { theme.faint() };
    let button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .w(px(30.0))
        .h(px(30.0))
        .rounded(px(8.0))
        .cursor(if enabled {
            gpui::CursorStyle::PointingHand
        } else {
            gpui::CursorStyle::Arrow
        })
        .text_color(color)
        .bg(gpui::transparent_black())
        .child(icons::icon(mark, 18.0, color));
    if enabled {
        button.hover(move |style| style.bg(theme.panel_2()))
    } else {
        button
    }
}

pub(crate) fn pill(text: impl Into<String>, theme: &Theme, accent: bool) -> gpui::Div {
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
    text: impl Into<SharedString>,
    theme: &Theme,
) -> impl Fn(&mut Window, &mut gpui::App) -> gpui::AnyView + 'static {
    let text = text.into();
    let theme = theme.clone();
    move |_window, cx| {
        let tip = cx.new(|_| Tip {
            text: text.clone(),
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
        .children(root.current_worktree().map(|worktree| {
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .flex_none()
                .text_size(px(11.0))
                .text_color(theme.accent())
                .child(icons::icon(Icon::Worktree, 12.0, theme.accent()))
                .child(worktree.name.clone())
        }))
        .child(div().flex_1())
}

pub fn topbar(
    root: &mut Root,
    theme: &Theme,
    scale: f32,
    _window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let destination = matches!(root.overlay, Some(Overlay::Settings))
        || matches!(root.rail, Rail::Projects | Rail::Customize);
    let back_enabled = root.can_navigate_back();
    let forward_enabled = root.can_navigate_forward();
    let mut row = div()
        .id("topbar")
        .window_control_area(WindowControlArea::Drag)
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .h(px(TOPBAR_HEIGHT * scale))
        .bg(theme.sidebar())
        .border_b_1()
        .border_color(theme.border())
        .on_click(cx.listener(|_, event: &ClickEvent, window, _| {
            if event.click_count() == 2 {
                window.titlebar_double_click();
            }
        }));

    let back = navigation_button("nav-back", Icon::Back, back_enabled, theme)
        .tooltip(tip("Back \u{00b7} \u{2318}[", theme))
        .on_click(cx.listener(|root, _, _, cx| root.navigate_back(cx)));
    let forward = navigation_button("nav-forward", Icon::Forward, forward_enabled, theme)
        .tooltip(tip("Forward \u{00b7} \u{2318}]", theme))
        .on_click(cx.listener(|root, _, _, cx| root.navigate_forward(cx)));
    let toggle = icon_button("nav-sidebar", Icon::Sidebar, 30.0, theme, false)
        .tooltip(tip("Show or hide the sidebar \u{00b7} \u{2318}B", theme))
        .on_click(cx.listener(|root, _, _, cx| {
            root.sidebar_visible = !root.sidebar_visible;
            let visible = root.sidebar_visible;
            root.save_prefs(serde_json::json!({ "sidebar_visible": visible }), cx);
            cx.notify();
        }));
    let leading_width = if destination {
        destination_leading_width(root)
    } else if root.sidebar_visible {
        RAIL_WIDTH + root.sidebar_width + SIDEBAR_RESIZER_WIDTH
    } else {
        220.0
    };
    row = row.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_1()
            .flex_none()
            .w(px(leading_width))
            .h_full()
            .pl(px(76.0))
            .pr_2()
            .child(back)
            .child(forward)
            .child(toggle)
            .child(div().flex_none().w(px(6.0))),
    );

    if destination {
        let border_color = if root.overlay.is_some() || root.rail == Rail::Customize {
            theme.border()
        } else {
            gpui::transparent_black()
        };
        return row.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .flex_1()
                .h_full()
                .pl(px(14.0))
                .pr_2()
                .border_l_1()
                .border_color(border_color)
                .child(div().flex_1())
                .child(pill("BETA", theme, true)),
        );
    }

    let mut content = div()
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .flex_1()
        .min_w(px(0.0))
        .h_full()
        .pl(px(14.0))
        .pr_2()
        .border_l_1()
        .border_color(theme.border());

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
        content = content.child(tabs);
    } else {
        content = content.child(thread_label(root, theme));
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
    content = content
        .child(new_chat)
        .child(changes)
        .child(details)
        .child(div().flex_none().w(px(4.0)))
        .child(beta);
    row.child(content)
}

fn destination_leading_width(root: &Root) -> f32 {
    if matches!(root.overlay, Some(Overlay::Settings)) {
        RAIL_WIDTH + SETTINGS_NAVIGATION_WIDTH
    } else if root.rail == Rail::Customize {
        RAIL_WIDTH + root.sidebar_width + SIDEBAR_RESIZER_WIDTH
    } else {
        220.0
    }
}

// ---------- rail ----------

pub fn rail(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let chats = root.rail == Rail::Chats && !matches!(root.overlay, Some(Overlay::Settings));
    let projects = root.rail == Rail::Projects && !matches!(root.overlay, Some(Overlay::Settings));
    let settings = matches!(root.overlay, Some(Overlay::Settings));
    let customize = root.rail == Rail::Customize && !settings;
    div()
        .flex()
        .flex_col()
        .items_center()
        .flex_none()
        .w(px(RAIL_WIDTH))
        .h_full()
        .py(px(9.0))
        .px(px(4.0))
        .gap(px(7.0))
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
            icon_button("rail-projects", Icon::Folder, 40.0, theme, projects)
                .tooltip(tip("Projects", theme))
                .on_click(cx.listener(|root, _, _, cx| {
                    root.rail = Rail::Projects;
                    root.overlay = None;
                    cx.notify();
                })),
        )
        .child(
            icon_button("rail-worktrees", Icon::Worktree, 40.0, theme, false)
                .tooltip(tip("Git worktrees", theme))
                .on_click(cx.listener(|root, _, _, cx| root.open_worktrees(cx))),
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
            icon_button("rail-settings", Icon::Settings, 40.0, theme, settings)
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
        .gap(px(2.0))
        .h(px(46.0))
        .px(px(10.0))
        .border_b_1()
        .border_color(theme.border());
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
                    .id("sidebar-brand")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_1()
                    .px(px(6.0))
                    .cursor_pointer()
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text())
                    .child("Oxide")
                    .child(icons::icon(Icon::ChevronDown, 12.0, theme.faint()))
                    .on_click(cx.listener(|root, _, _, cx| {
                        root.rail = Rail::Projects;
                        root.overlay = None;
                        cx.notify();
                    })),
            )
            .child(
                icon_button("sidebar-notifications", Icon::Bell, 28.0, theme, false)
                    .tooltip(tip("Notification settings", theme))
                    .on_click(cx.listener(|root, _, _, cx| {
                        root.settings_tab = crate::app::SettingsTab::Notifications;
                        root.open_overlay(Overlay::Settings, cx);
                    })),
            )
            .child(
                icon_button("sidebar-search", Icon::Search, 28.0, theme, false)
                    .tooltip(tip("Search chats", theme))
                    .on_click(cx.listener(|root, _, window, cx| root.open_search(window, cx))),
            );
    }
    column = column.child(top);

    column = column.child(
        div().px(px(10.0)).pt(px(4.0)).pb(px(10.0)).child(
            div()
                .id("sidebar-new-chat")
                .flex()
                .flex_row()
                .items_center()
                .gap(px(7.0))
                .h(px(34.0))
                .px(px(8.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel()))
                .child(icons::icon(Icon::Pencil, 16.0, theme.dim()))
                .child(
                    div()
                        .text_size(px(13.5))
                        .font_weight(FontWeight::MEDIUM)
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
        .gap(px(0.0));

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
            let active = root
                .chats
                .iter()
                .any(|chat| chat.session.as_deref() == Some(id.as_str()));
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
                &session,
                &when,
                None,
                SessionFlags {
                    pinned: session.pinned,
                    worktree: session.worktree.is_some(),
                    active,
                    indent: 12.0,
                    show_actions: true,
                },
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
        list = list.child(projects_header(theme, cx));
        let projects = root.projects.clone();
        let current_project = root.current_project_owner();
        for project in projects {
            let path = project.path.clone();
            let name = root.project_name(&path);
            let mut sessions = root
                .sessions_of(&path)
                .into_iter()
                .cloned()
                .collect::<Vec<_>>();
            let expanded = root.expanded_projects.contains(&path);
            let collapsed = root.collapsed_projects.contains(&path);
            let hidden_sessions = sessions.len().saturating_sub(5);
            if collapsed {
                sessions.clear();
            } else if !expanded {
                sessions.truncate(5);
            }
            let missing = !project.exists;
            let pinned = project.pinned;
            let selected = current_project.as_deref() == Some(path.as_str());
            let path_for_new = path.clone();
            let path_for_select = path.clone();
            let path_for_remove = path.clone();
            let path_for_toggle = path.clone();
            let project_id = project.id.clone();
            let project_id_for_menu = project_id.clone();
            let project_name = name.clone();
            let inline_actions =
                root.inline_project_actions.as_deref() == Some(project.id.as_str());
            let group = SharedString::from(format!("project-row-{}", project.id));
            let new_tooltip = format!("New chat in {name}");
            let group_for_new = group.clone();
            let mut project_group = div().flex().flex_col().gap(px(2.0)).mb(px(10.0));
            let header = div()
                .id(SharedString::from(format!("project-{path}")))
                .group(group)
                .relative()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .h(px(32.0))
                .px(px(8.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel()))
                .tooltip(tip(path.clone(), theme))
                .child(
                    div()
                        .id(SharedString::from(format!("project-toggle-{path}")))
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .w(px(16.0))
                        .h(px(24.0))
                        .rounded(px(5.0))
                        .cursor_pointer()
                        .hover(move |style| style.bg(theme.panel_2()))
                        .child(icons::icon(
                            if collapsed {
                                Icon::ChevronRight
                            } else {
                                Icon::ChevronDown
                            },
                            12.0,
                            theme.faint(),
                        ))
                        .on_click(cx.listener(move |root, _, _, cx| {
                            cx.stop_propagation();
                            if root.collapsed_projects.contains(&path_for_toggle) {
                                root.collapsed_projects.remove(&path_for_toggle);
                            } else {
                                root.collapsed_projects.insert(path_for_toggle.clone());
                            }
                            cx.notify();
                        })),
                )
                .child(icons::icon(
                    Icon::Folder,
                    15.0,
                    if missing { theme.error() } else { theme.dim() },
                ))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(13.5))
                        .font_weight(if selected {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if selected { theme.text() } else { theme.dim() })
                        .child(name),
                )
                .children(pinned.then(|| icons::icon(Icon::Pin, 12.0, theme.accent())))
                .children(missing.then(|| {
                    div()
                        .flex_none()
                        .w(px(5.0))
                        .h(px(5.0))
                        .rounded_full()
                        .bg(theme.error())
                }))
                .child(
                    div()
                        .absolute()
                        .top(px(0.0))
                        .right(px(2.0))
                        .bottom(px(0.0))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(0.0))
                        .px(px(1.0))
                        .rounded(px(7.0))
                        .bg(theme.panel())
                        .opacity(0.0)
                        .group_hover(group_for_new, move |style| style.opacity(1.0))
                        .child(
                            icon_button(
                                SharedString::from(format!("project-new-{path}")),
                                Icon::Pencil,
                                20.0,
                                theme,
                                inline_actions,
                            )
                            .tooltip(tip(new_tooltip, theme))
                            .on_click(cx.listener(
                                move |root, _, window, cx| {
                                    cx.stop_propagation();
                                    root.inline_project_actions = None;
                                    root.collapsed_projects.remove(&path_for_new);
                                    root.push_chat(
                                        window,
                                        path_for_new.clone(),
                                        None,
                                        "New chat".to_string(),
                                        cx,
                                    );
                                    root.focus_composer(window, cx);
                                },
                            )),
                        )
                        .child(
                            icon_button(
                                SharedString::from(format!("project-remove-{path}")),
                                Icon::More,
                                20.0,
                                theme,
                                false,
                            )
                            .tooltip(tip("Project actions", theme))
                            .on_click(cx.listener(
                                move |root, _, _window, cx| {
                                    cx.stop_propagation();
                                    root.inline_project_actions =
                                        if root.inline_project_actions.as_deref()
                                            == Some(project_id_for_menu.as_str())
                                        {
                                            None
                                        } else {
                                            Some(project_id_for_menu.clone())
                                        };
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .on_click(cx.listener(move |root, _, window, cx| {
                    root.select_project(window, path_for_select.clone(), cx);
                }));
            project_group = project_group.child(header);
            if inline_actions {
                project_group = project_group.child(project_actions_inline(
                    InlineProjectActions {
                        id: project_id.clone(),
                        path: path_for_remove.clone(),
                        name: project_name.clone(),
                        pinned,
                        session_count: root.sessions_of(&path).len(),
                        active: selected,
                    },
                    theme,
                    cx,
                ));
            }
            if !collapsed && sessions.is_empty() {
                project_group = project_group.child(
                    div()
                        .flex()
                        .items_center()
                        .h(px(30.0))
                        .pl(px(44.0))
                        .pr(px(8.0))
                        .text_size(px(12.0))
                        .text_color(theme.faint())
                        .child("No chats"),
                );
            }
            if !collapsed {
                for session in sessions {
                    let id = session.id.clone();
                    let pinned = session.pinned;
                    let active = root
                        .chats
                        .iter()
                        .any(|chat| chat.session.as_ref() == Some(&id));
                    project_group = project_group.child(session_row(
                        &session,
                        "",
                        None,
                        SessionFlags {
                            pinned,
                            worktree: session.worktree.is_some(),
                            active,
                            indent: 24.0,
                            show_actions: true,
                        },
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
            if !collapsed && hidden_sessions > 0 && !root.expanded_projects.contains(&path) {
                let path_for_expand = path.clone();
                project_group = project_group.child(
                    div()
                        .id(SharedString::from(format!("project-more-{path}")))
                        .flex()
                        .items_center()
                        .h(px(30.0))
                        .pl(px(24.0))
                        .pr(px(8.0))
                        .rounded(px(7.0))
                        .cursor_pointer()
                        .text_size(px(12.5))
                        .text_color(theme.faint())
                        .hover(move |style| style.bg(theme.panel()).text_color(theme.text()))
                        .child("Show more")
                        .on_click(cx.listener(move |root, _, _, cx| {
                            root.expanded_projects.insert(path_for_expand.clone());
                            cx.notify();
                        })),
                );
            }
            list = list.child(project_group);
        }

        let mut recents = root.sessions.clone();
        recents.sort_by_key(|session| std::cmp::Reverse(session.modified_at));
        recents.truncate(5);
        if !recents.is_empty() {
            list = list.child(section("Recents", theme));
            let mut recent_rows = div().flex().flex_col().gap(px(4.0));
            for session in recents {
                let id = session.id.clone();
                let active = root
                    .chats
                    .iter()
                    .any(|chat| chat.session.as_ref() == Some(&id));
                recent_rows = recent_rows.child(session_row(
                    &session,
                    "",
                    None,
                    SessionFlags {
                        pinned: session.pinned,
                        worktree: session.worktree.is_some(),
                        active,
                        indent: 8.0,
                        show_actions: true,
                    },
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
            list = list.child(recent_rows);
        }
    }

    column = column.child(list);
    column
}

fn section(title: &'static str, theme: &Theme) -> gpui::Div {
    div()
        .px(px(10.0))
        .pt(px(14.0))
        .pb(px(6.0))
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.dim())
        .child(title)
}

/// The sidebar's `Projects` heading: the name on the left, the folder-picker
/// behind the plus on the right, the way the page pairs them.
fn projects_header(theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    div()
        .flex()
        .flex_row()
        .items_center()
        .h(px(32.0))
        .px(px(10.0))
        .pt(px(4.0))
        .child(
            div()
                .flex_1()
                .text_size(px(12.5))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.dim())
                .child("Projects"),
        )
        .child(
            icon_button("sidebar-add-project", Icon::Plus, 22.0, theme, false)
                .tooltip(tip("New project", theme))
                .on_click(cx.listener(|root, _, _, cx| root.add_project(cx))),
        )
}

struct InlineProjectActions {
    id: String,
    path: String,
    name: String,
    pinned: bool,
    session_count: usize,
    active: bool,
}

fn project_actions_inline(
    project: InlineProjectActions,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let InlineProjectActions {
        id,
        path,
        name,
        pinned,
        session_count,
        active,
    } = project;
    let path_for_new = path.clone();
    let path_for_remove = path.clone();
    let id_for_pin = id.clone();
    let id_for_remove = id.clone();
    let chats = match session_count {
        0 => "No chats".to_string(),
        1 => "1 chat".to_string(),
        count => format!("{count} chats"),
    };
    let summary = format!("{chats} · {} active", usize::from(active));
    div()
        .mx(px(10.0))
        .mb(px(6.0))
        .p(px(10.0))
        .rounded(px(10.0))
        .border_1()
        .border_color(theme.border())
        .bg(theme.panel())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(8.0))
                .child(icons::icon(Icon::Folder, 15.0, theme.dim()))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text())
                        .child(name),
                )
                .child(
                    icon_button(
                        SharedString::from(format!("project-inline-pin-{id}")),
                        Icon::Pin,
                        24.0,
                        theme,
                        pinned,
                    )
                    .tooltip(tip(
                        if pinned {
                            "Unpin project"
                        } else {
                            "Pin project"
                        },
                        theme,
                    ))
                    .on_click(cx.listener(move |root, _, _, cx| {
                        root.set_project_pinned(id_for_pin.clone(), !pinned, cx);
                    })),
                ),
        )
        .child(
            div()
                .mt(px(4.0))
                .text_size(px(11.5))
                .text_color(theme.dim())
                .child(summary),
        )
        .child(
            div()
                .mt(px(5.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(11.5))
                .text_color(theme.faint())
                .child(path),
        )
        .child(
            div()
                .mt(px(9.0))
                .flex()
                .flex_row()
                .gap(px(6.0))
                .child(
                    Button::new("New chat")
                        .render(
                            SharedString::from(format!("project-inline-new-{id}")),
                            theme,
                            cx,
                            move |root, window, cx| {
                                root.inline_project_actions = None;
                                root.collapsed_projects.remove(&path_for_new);
                                root.push_chat(
                                    window,
                                    path_for_new.clone(),
                                    None,
                                    "New chat".to_string(),
                                    cx,
                                );
                                root.focus_composer(window, cx);
                            },
                        )
                        .flex_1(),
                )
                .child(Button::new("Remove").danger().render(
                    SharedString::from(format!("project-inline-remove-{id}")),
                    theme,
                    cx,
                    move |root, _, cx| {
                        root.inline_project_actions = None;
                        root.overlay = Some(Overlay::Confirm {
                            title: "Remove project".to_string(),
                            body: format!(
                                "Remove {path_for_remove} from the sidebar? The folder and its chats stay on disk."
                            ),
                            action: ConfirmAction::RemoveProject(id_for_remove.clone()),
                        });
                        cx.notify();
                    },
                )),
        )
}

#[derive(Clone, Copy)]
struct SessionFlags {
    pinned: bool,
    worktree: bool,
    active: bool,
    indent: f32,
    show_actions: bool,
}

fn session_row(
    session: &crate::model::SessionRow,
    when: &str,
    shortcut: Option<usize>,
    flags: SessionFlags,
    theme: &Theme,
    cx: &mut Context<Root>,
    open: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> impl IntoElement {
    let SessionFlags {
        pinned,
        worktree,
        active,
        indent,
        show_actions,
    } = flags;
    let id = session.id.clone();
    let title = session.title.clone();
    let project = session.project.clone();
    let pinned_value = session.pinned;
    let when = when.to_string();
    let group = SharedString::from(format!("session-row-{id}"));
    let group_for_actions = group.clone();
    let pin_id = id.clone();
    let archive_id = id.clone();
    let menu_id = id.clone();
    let menu_title = title.clone();
    let menu_project = project.clone();
    let meta = shortcut.map(shortcut_label).unwrap_or(when.clone());
    div()
        .id(SharedString::from(format!("session-{id}")))
        .group(group)
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .h(px(32.0))
        .pl(px(indent))
        .pr(px(8.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .bg(if active {
            theme.panel_3()
        } else {
            theme.sidebar()
        })
        .hover(move |style| style.bg(theme.panel()))
        .children(pinned.then(|| icons::icon(Icon::Pin, 12.0, theme.accent())))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(12.5))
                .font_weight(if active {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .text_color(if active { theme.text() } else { theme.dim() })
                .child(title.clone()),
        )
        .children((!meta.is_empty()).then(|| {
            div()
                .flex_none()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_size(px(10.5))
                .text_color(theme.faint())
                .child(meta)
        }))
        .children(worktree.then(|| icons::icon(Icon::Branch, 12.0, theme.accent())))
        .children(show_actions.then(|| {
            div()
                .absolute()
                .top(px(0.0))
                .right(px(2.0))
                .bottom(px(0.0))
                .flex()
                .flex_row()
                .items_center()
                .justify_end()
                .gap(px(0.0))
                .w(px(48.0))
                .px(px(1.0))
                .rounded(px(7.0))
                .bg(theme.panel())
                .opacity(0.0)
                .group_hover(group_for_actions, move |style| style.opacity(1.0))
                .child(
                    icon_button(
                        SharedString::from(format!("session-pin-{id}")),
                        Icon::Pin,
                        20.0,
                        theme,
                        pinned_value,
                    )
                    .tooltip(tip(
                        if pinned_value {
                            "Unpin thread"
                        } else {
                            "Pin thread"
                        },
                        theme,
                    ))
                    .on_click(cx.listener(move |root, _, _, cx| {
                        cx.stop_propagation();
                        root.set_session_pinned(pin_id.clone(), !pinned_value, cx);
                    })),
                )
                .child(
                    icon_button(
                        SharedString::from(format!("session-archive-{id}")),
                        Icon::Archive,
                        20.0,
                        theme,
                        false,
                    )
                    .tooltip(tip("Archive thread", theme))
                    .on_click(cx.listener(move |root, _, _, cx| {
                        cx.stop_propagation();
                        root.archive_session(archive_id.clone(), cx);
                    })),
                )
        }))
        .on_click(cx.listener(move |root, _, window, cx| open(root, window, cx)))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |root, _, _, cx| {
                root.overlay = Some(Overlay::SessionActions {
                    id: menu_id.clone(),
                    title: menu_title.clone(),
                    project: menu_project.clone(),
                    pinned: pinned_value,
                });
                cx.notify();
            }),
        )
}

fn shortcut_label(index: usize) -> String {
    if cfg!(target_os = "macos") {
        format!("\u{2318}{index}")
    } else {
        format!("Ctrl+{index}")
    }
}

pub fn sidebar_resizer(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let _ = root;
    div()
        .id("sidebar-resizer")
        .flex_none()
        .w(px(SIDEBAR_RESIZER_WIDTH))
        .h_full()
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .bg(theme.sidebar())
        .border_r_1()
        .border_color(theme.border())
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|root, event, _, cx| root.begin_sidebar_drag(event, cx)),
        )
        .on_mouse_move(
            cx.listener(|root, event, window, cx| root.handle_sidebar_drag(event, window, cx)),
        )
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

    let mut panel = div()
        .id("details-pane")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .gap_1()
        .px(px(14.0))
        .py(px(12.0))
        .rounded(px(12.0))
        .bg(theme.panel())
        .border_1()
        .border_color(theme.border());

    let name = project
        .as_ref()
        .map(|project| root.project_name(&project.path))
        .unwrap_or_else(|| "No project".to_string());
    panel = panel.child(
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

    panel = panel.child(detail_row(
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

    panel = panel.child(detail_row(
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

    panel = panel.child(div().h(px(12.0)));

    panel = panel.child(
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
        panel = panel.child(
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
            panel = panel.child(
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

    panel = panel.child(
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

    div()
        .flex()
        .flex_none()
        .w(px(296.0))
        .min_h(px(0.0))
        .p(px(8.0))
        .bg(theme.background())
        .child(panel)
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
        .items_start()
        .flex_1()
        .h_full()
        .min_w(px(0.0))
        .min_h(px(0.0));

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
        .h_full()
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
        .px(px(22.0))
        .pt(px(34.0))
        .pb(px(10.0))
        .gap(px(22.0));

    if untrusted {
        let path = root.chats[index].project.clone();
        transcript = transcript.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .flex_none()
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
                .flex_none()
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
        .max_w(px(560.0))
        .mx_auto()
        .pb(px(32.0))
        .child(
            div()
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .w(px(56.0))
                .h(px(56.0))
                .rounded(px(11.0))
                .bg(theme.well())
                .child(icons::mark(40.0)),
        )
        .child(
            div()
                .pt(px(16.0))
                .text_size(px(24.0))
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
        .w_full()
        .max_w(px(552.0))
        .mx_auto()
        .gap_3()
        .pt_1();
    for text in SUGGESTIONS {
        grid = grid.child(
            div()
                .id(SharedString::from(format!("suggestion-{text}")))
                .flex()
                .items_start()
                .w(px(270.0))
                .min_h(px(68.0))
                .px(px(16.0))
                .py(px(14.0))
                .rounded(px(12.0))
                .border_1()
                .border_color(theme.border())
                .bg(theme.panel())
                .text_size(px(12.5))
                .line_height(px(18.5))
                .text_color(theme.dim())
                .cursor_pointer()
                .hover(move |style| {
                    style
                        .border_color(theme.dim())
                        .bg(theme.panel_2())
                        .text_color(theme.text())
                })
                .child(
                    div()
                        .w_full()
                        .min_w(px(0.0))
                        .whitespace_normal()
                        .child(text),
                )
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

fn tool_label(name: &str) -> &str {
    match name {
        "tool" | "" => "Tool result",
        "bash" => "Command",
        "read" => "Read",
        "edit" => "Edit",
        "find" => "Find",
        "grep" => "Search",
        "web_search" => "Web search",
        "web_fetch" => "Web fetch",
        other => other,
    }
}

fn compact_tool_output(output: &str) -> (String, bool) {
    let pretty = serde_json::from_str::<serde_json::Value>(output.trim())
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| output.to_string());
    let lines = pretty.lines().collect::<Vec<_>>();
    let shown = lines
        .iter()
        .take(10)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    (shown, lines.len() > 10)
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
            streaming: _,
            text,
        } => {
            let mut column = div()
                .flex()
                .flex_col()
                .w_full()
                .min_w(px(0.0))
                .flex_none()
                .gap_2()
                .pl(px(18.0))
                .pr(px(4.0));
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
            let mut block = div()
                .flex()
                .flex_col()
                .w_full()
                .min_w(px(0.0))
                .flex_none()
                .gap_2()
                .px(px(12.0))
                .py(px(10.0))
                .rounded(px(10.0))
                .border_1()
                .border_color(if card.status == Status::Failed {
                    theme.error()
                } else {
                    theme.border()
                })
                .bg(if card.status == Status::Failed {
                    theme.color("tool_error_bg")
                } else if card.status == Status::Running {
                    theme.color("tool_pending_bg")
                } else {
                    theme.panel()
                });
            block = block.child(
                div()
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
                            .w(px(22.0))
                            .h(px(22.0))
                            .rounded(px(6.0))
                            .bg(theme.panel_2())
                            .child(icons::icon(Icon::More, 13.0, theme.faint())),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(12.5))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text())
                            .child(tool_label(&card.name).to_string()),
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
                let (output, hidden) = compact_tool_output(&card.output);
                block = block.child(
                    div()
                        .id(SharedString::from(format!("tool-output-{}", card.id)))
                        .flex()
                        .flex_col()
                        .gap_1()
                        .w_full()
                        .px(px(10.0))
                        .py(px(8.0))
                        .rounded(px(7.0))
                        .bg(theme.background())
                        .border_1()
                        .border_color(theme.border())
                        .text_size(px(11.5))
                        .line_height(px(17.0))
                        .font_family("Menlo")
                        .whitespace_normal()
                        .overflow_hidden()
                        .text_color(theme.dim())
                        .child(output)
                        .children(hidden.then(|| {
                            div()
                                .pt_1()
                                .text_size(px(10.5))
                                .text_color(theme.faint())
                                .child("Output truncated")
                        })),
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
        Block::Paragraph(spans) => {
            let lines = inline_lines(spans);
            let rows = lines
                .iter()
                .map(|line| estimated_rows(line, 92))
                .sum::<usize>();
            div()
                .flex()
                .flex_col()
                .w_full()
                .min_w(px(0.0))
                .min_h(px(rows as f32 * 22.0))
                .flex_none()
                .whitespace_normal()
                .text_size(px(13.5))
                .line_height(px(22.0))
                .children(lines.into_iter().map(|line| {
                    div()
                        .w_full()
                        .min_w(px(0.0))
                        .flex_none()
                        .whitespace_normal()
                        .child(styled(theme, &line))
                }))
        }
        Block::Heading { level, spans } => div()
            .w_full()
            .min_w(px(0.0))
            .flex_none()
            .whitespace_normal()
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
                .w_full()
                .min_w(px(0.0))
                .flex_none()
                .overflow_hidden()
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
                        .whitespace_normal()
                        .overflow_hidden()
                        .text_color(theme.text())
                        .child(text.clone()),
                )
        }
        Block::Quote(spans) => div()
            .w_full()
            .min_w(px(0.0))
            .flex_none()
            .min_h(px(estimated_rows(spans, 92) as f32 * 20.0))
            .whitespace_normal()
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
            .w_full()
            .min_w(px(0.0))
            .flex_none()
            .min_h(px(estimated_rows(spans, 88) as f32 * 21.0))
            .gap_2()
            .pl(px(*depth as f32 * 14.0))
            .text_size(px(13.5))
            .line_height(px(21.0))
            .child(div().text_color(theme.faint()).child("•"))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .whitespace_normal()
                    .child(styled(theme, spans)),
            ),
        Block::Numbered {
            depth,
            number,
            spans,
        } => div()
            .flex()
            .flex_row()
            .w_full()
            .min_w(px(0.0))
            .flex_none()
            .min_h(px(estimated_rows(spans, 86) as f32 * 21.0))
            .gap_2()
            .pl(px(*depth as f32 * 14.0))
            .text_size(px(13.5))
            .line_height(px(21.0))
            .child(div().text_color(theme.faint()).child(number.clone()))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .whitespace_normal()
                    .child(styled(theme, spans)),
            ),
        Block::Rule => crate::app::rule(theme),
    }
}

fn inline_lines(spans: &[Inline]) -> Vec<Vec<Inline>> {
    let mut lines = vec![Vec::new()];
    for span in spans {
        for (index, part) in span.text.split('\n').enumerate() {
            if index > 0 {
                lines.push(Vec::new());
            }
            if !part.is_empty() {
                let mut next = span.clone();
                next.text = part.to_string();
                lines
                    .last_mut()
                    .expect("a line is always present")
                    .push(next);
            }
        }
    }
    lines
}

fn estimated_rows(spans: &[Inline], columns: usize) -> usize {
    let columns = columns.max(1);
    let mut rows = 0;
    let mut current = 0;
    for character in spans.iter().flat_map(|span| span.text.chars()) {
        if character == '\n' {
            rows += 1;
            current = 0;
        } else {
            current += 1;
            if current == columns {
                rows += 1;
                current = 0;
            }
        }
    }
    rows + usize::from(current > 0)
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
    let editor_text = editor
        .as_ref()
        .map(|editor| editor.read(cx).text().to_string())
        .unwrap_or_default();
    let running = chat.running();
    let attachments = chat.attachments.clone();
    let queued = chat.queued.clone();
    let project = root
        .selected_project()
        .map(|project| root.project_name(&project.path))
        .unwrap_or_else(|| root.project_name(&chat.project));
    let worktree = root.current_worktree().cloned();
    let model = root.info["model"].as_str().unwrap_or("model").to_string();
    let reasoning = root.info["reasoning"].as_str().unwrap_or("off").to_string();
    let context_tokens = chat
        .context_tokens
        .or_else(|| chat.usage.as_ref().map(|usage| usage.context_tokens))
        .unwrap_or(0);
    let context_window = root.info["contextWindow"].as_u64().unwrap_or(0);
    let usage_detail = chat.usage.as_ref().map(|usage| {
        format!(
            "↑{} ↓{} · cache read {} · write {} · ${:.2} · {} calls",
            format_tokens(usage.input),
            format_tokens(usage.output),
            format_tokens(usage.cache_read),
            format_tokens(usage.cache_write),
            usage.cost,
            usage.model_calls
        )
    });

    let branch = root.branch.clone();

    let mut box_row = div()
        .key_context("Composer")
        .on_action(cx.listener(|root, _: &crate::app::Send, _window, cx| root.send(cx)))
        .on_action(cx.listener(|root, _: &crate::app::Stop, _window, cx| root.stop(cx)))
        .flex()
        .flex_col()
        .gap(px(4.0))
        .w_full()
        .px(px(12.0))
        .py(px(10.0))
        .bg(theme.panel_2())
        .border_1()
        .border_color(theme.border())
        .rounded(px(20.0));

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
            .child(
                div()
                    .id("composer-project")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .cursor_pointer()
                    .rounded(px(6.0))
                    .hover(move |style| style.bg(theme.panel_3()))
                    .child(icons::icon(Icon::Folder, 13.0, theme.faint()))
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(11.5))
                            .text_color(theme.dim())
                            .child(project.clone()),
                    )
                    .on_click(
                        cx.listener(|root, _, _, cx| root.open_overlay(Overlay::Projects, cx)),
                    ),
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
            .child(
                div()
                    .id("composer-worktree")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .h(px(22.0))
                    .px_1()
                    .rounded(px(6.0))
                    .cursor_pointer()
                    .text_size(px(11.0))
                    .text_color(if worktree.is_some() {
                        theme.accent()
                    } else {
                        theme.faint()
                    })
                    .hover(move |style| style.bg(theme.panel_3()))
                    .tooltip(tip("Choose a Git worktree", theme))
                    .child(icons::icon(Icon::Worktree, 13.0, theme.faint()))
                    .child(
                        worktree
                            .as_ref()
                            .map(|worktree| worktree.name.clone())
                            .unwrap_or_else(|| "Worktree".to_string()),
                    )
                    .on_click(cx.listener(|root, _, _, cx| root.open_worktrees(cx))),
            )
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

    if let Some(query) = editor_text
        .strip_prefix('/')
        .filter(|query| !query.chars().any(char::is_whitespace))
    {
        let commands = root
            .commands
            .iter()
            .filter(|command| {
                command["name"]
                    .as_str()
                    .is_some_and(|name| name.starts_with(&query.to_ascii_lowercase()))
            })
            .take(7)
            .cloned()
            .collect::<Vec<_>>();
        if !commands.is_empty() {
            let mut menu = div()
                .id("composer-command-menu")
                .flex()
                .flex_col()
                .gap(px(2.0))
                .mb_1()
                .p(px(5.0))
                .rounded(px(10.0))
                .bg(theme.background())
                .border_1()
                .border_color(theme.border());
            for command in commands {
                let name = command["name"].as_str().unwrap_or_default().to_string();
                let description = command["description"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let arguments = command["arguments"].as_str().map(str::to_string);
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("composer-command-{name}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .h(px(34.0))
                        .px_2()
                        .rounded(px(7.0))
                        .cursor_pointer()
                        .hover(move |style| style.bg(theme.panel_3()))
                        .child(
                            div()
                                .flex_none()
                                .w(px(112.0))
                                .text_size(px(12.0))
                                .font_family("Menlo")
                                .text_color(theme.text())
                                .child(format!("/{name}")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(11.5))
                                .text_color(theme.dim())
                                .child(description),
                        )
                        .children(arguments.map(|arguments| {
                            div()
                                .flex_none()
                                .text_size(px(10.5))
                                .text_color(theme.faint())
                                .child(arguments)
                        }))
                        .on_click(cx.listener(move |root, _, window, cx| {
                            root.fill_composer(format!("/{name} "), window, cx)
                        })),
                );
            }
            box_row = box_row.child(menu);
        }
    }

    if root.at_chat == Some(chat.id) {
        let start = root.at_answer["start"].as_u64().unwrap_or(0) as usize;
        let end = root.at_answer["end"].as_u64().unwrap_or(start as u64) as usize;
        let rows = root.at_answer["rows"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if !rows.is_empty() {
            let mut menu = div()
                .id("composer-at-menu")
                .flex()
                .flex_col()
                .gap(px(2.0))
                .mb_1()
                .p(px(5.0))
                .rounded(px(10.0))
                .bg(theme.background())
                .border_1()
                .border_color(theme.border());
            for row in rows {
                let label = row["label"].as_str().unwrap_or_default().to_string();
                let insert = row["insert"].as_str().unwrap_or_default().to_string();
                let kind = row["kind"].as_str().unwrap_or_default().to_string();
                let editor = editor.clone();
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("composer-at-{label}")))
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .h(px(32.0))
                        .px_2()
                        .rounded(px(7.0))
                        .cursor_pointer()
                        .hover(move |style| style.bg(theme.panel_3()))
                        .child(icons::icon(
                            if kind == "folder" {
                                Icon::Folder
                            } else {
                                Icon::File
                            },
                            14.0,
                            theme.faint(),
                        ))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .font_family("Menlo")
                                .text_size(px(11.5))
                                .text_color(theme.text())
                                .child(label),
                        )
                        .on_click(cx.listener(move |root, _, window, cx| {
                            if let Some(editor) = &editor {
                                editor.update(cx, |editor, cx| {
                                    editor.replace_utf16_range(start..end, &insert, cx)
                                });
                                window.focus(&editor.read(cx).focus_handle(cx));
                            }
                            root.at_answer = serde_json::Value::Null;
                            root.at_chat = None;
                            cx.notify();
                        })),
                );
            }
            box_row = box_row.child(menu);
        }
    }

    if !queued.is_empty() {
        let mut rows = div().flex().flex_col().gap_1().px_1();
        for message in queued {
            rows = rows.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_h(px(34.0))
                    .px_2()
                    .rounded(px(9.0))
                    .bg(theme.background())
                    .border_1()
                    .border_color(theme.border())
                    .child(
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if message.follow_up {
                                theme.faint()
                            } else {
                                theme.accent()
                            })
                            .child(if message.follow_up { "Queued" } else { "Steer" }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.5))
                            .text_color(theme.dim())
                            .child(message.text),
                    )
                    .children((!message.attachments.is_empty()).then(|| {
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(theme.faint())
                            .child(format!("{} attached", message.attachments.len()))
                    })),
            );
        }
        box_row = box_row.child(rows);
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
        .h(px(30.0));
    footer = footer
        .child(
            icon_button(
                ("composer-attach", 0usize),
                Icon::Paperclip,
                30.0,
                theme,
                false,
            )
            .tooltip(tip("Attach a file", theme))
            .on_click(cx.listener(|root, _, _, cx| root.attach_files(cx))),
        )
        .child(
            icon_button(("composer-tools", 1usize), Icon::More, 30.0, theme, false)
                .tooltip(tip("More tools", theme))
                .on_click(
                    cx.listener(|root, _, _, cx| root.open_overlay(Overlay::ComposerTools, cx)),
                ),
        )
        .child(
            div()
                .id("composer-approvals")
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .h(px(30.0))
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
    footer = footer.child(context_ring(
        context_tokens,
        context_window,
        usage_detail.as_deref(),
        theme,
    ));

    if running {
        footer = footer.child(
            div()
                .id("composer-stop")
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .w(px(30.0))
                .h(px(30.0))
                .rounded(px(15.0))
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
                .w(px(30.0))
                .h(px(30.0))
                .rounded(px(15.0))
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

    div()
        .flex()
        .flex_col()
        .flex_none()
        .w_full()
        .items_center()
        .px(px(22.0))
        .pb(px(10.0))
        .pt(px(14.0))
        .child(div().w_full().max_w(px(780.0)).child(box_row))
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

fn context_ring(
    tokens: u64,
    window: u64,
    usage_detail: Option<&str>,
    theme: &Theme,
) -> impl IntoElement {
    let percent = if window == 0 {
        None
    } else {
        Some((tokens as f64 / window as f64) * 100.0)
    };
    let shown = percent
        .map(|value| value.clamp(0.0, 100.0).round() as u32)
        .unwrap_or(0);
    let color = match shown {
        91.. => theme.error(),
        71..=90 => theme.tool(),
        _ => theme.dim(),
    };
    let mut label = percent.map_or_else(
        || "Context window · no request yet".to_string(),
        |value| {
            format!(
                "Context window · {} / {} tokens ({:.0}%)",
                format_tokens(tokens),
                format_tokens(window),
                value
            )
        },
    );
    if let Some(detail) = usage_detail {
        label.push('\n');
        label.push_str(detail);
    }
    div()
        .id("context-ring")
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .w(px(30.0))
        .h(px(30.0))
        .rounded(px(15.0))
        .hover(move |style| style.bg(theme.panel_3()))
        .tooltip(tip(label, theme))
        .child(
            canvas(
                |bounds, _, _| bounds,
                move |bounds, _, window, _| {
                    let extent = f32::from(bounds.size.width).min(f32::from(bounds.size.height));
                    let center = point(
                        bounds.origin.x + px(extent / 2.0),
                        bounds.origin.y + px(extent / 2.0),
                    );
                    let radius = extent * 0.47;
                    let stroke = px(1.5);
                    let mut base = PathBuilder::stroke(stroke);
                    for step in 0..=64 {
                        let angle = -std::f32::consts::FRAC_PI_2
                            + step as f32 * std::f32::consts::TAU / 64.0;
                        let at = point(
                            center.x + px(radius * angle.cos()),
                            center.y + px(radius * angle.sin()),
                        );
                        if step == 0 {
                            base.move_to(at);
                        } else {
                            base.line_to(at);
                        }
                    }
                    if let Ok(path) = base.build() {
                        window.paint_path(path, gpui::Hsla { a: 0.28, ..color });
                    }

                    let arc = (shown as f32 / 100.0 * 64.0).round() as usize;
                    if arc > 0 {
                        let mut filled = PathBuilder::stroke(stroke);
                        for step in 0..=arc {
                            let angle = -std::f32::consts::FRAC_PI_2
                                + step as f32 * std::f32::consts::TAU / 64.0;
                            let at = point(
                                center.x + px(radius * angle.cos()),
                                center.y + px(radius * angle.sin()),
                            );
                            if step == 0 {
                                filled.move_to(at);
                            } else {
                                filled.line_to(at);
                            }
                        }
                        if let Ok(path) = filled.build() {
                            window.paint_path(path, color);
                        }
                    }
                },
            )
            .absolute()
            .size_full(),
        )
        .child(
            div()
                .text_size(px(8.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(color)
                .child(if percent.is_some() {
                    shown.to_string()
                } else {
                    String::new()
                }),
        )
}

fn format_tokens(count: u64) -> String {
    match count {
        0..=999 => count.to_string(),
        1_000..=9_999 => format!("{:.1}k", count as f64 / 1_000.0),
        10_000..=999_999 => format!("{}k", (count as f64 / 1_000.0).round() as u64),
        1_000_000..=9_999_999 => format!("{:.1}M", count as f64 / 1_000_000.0),
        _ => format!("{}M", (count as f64 / 1_000_000.0).round() as u64),
    }
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
        .h(px(30.0))
        .px(px(8.0))
        .rounded(px(8.0))
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

// ---------- projects ----------

pub fn projects_page(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let projects = root.projects.clone();
    let mut rows = div()
        .id("projects-directory")
        .flex()
        .flex_col()
        .w_full()
        .border_t_1()
        .border_color(theme.border());

    if projects.is_empty() {
        rows = rows.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .gap_3()
                .py(px(72.0))
                .child(icons::icon(Icon::Folder, 28.0, theme.faint()))
                .child(
                    div()
                        .text_size(px(14.0))
                        .text_color(theme.dim())
                        .child("No projects yet."),
                )
                .child(Button::new("Add a project").primary().render(
                    "projects-empty-add",
                    theme,
                    cx,
                    |root, _, cx| root.add_project(cx),
                )),
        );
    }

    for project in projects {
        let path = project.path.clone();
        let name = root.project_name(&path);
        let session_count = root.sessions_of(&path).len();
        let chats = project.session_count.max(session_count);
        let updated = relative_time(project.last_session_at);
        let open_path = path.clone();
        let new_path = path.clone();
        let action_path = path.clone();
        let action_id = project.id.clone();
        let action_name = name.clone();
        let new_control = SharedString::from(format!("project-directory-new-{path}"));
        let more_control = SharedString::from(format!("project-directory-more-{path}"));
        let pinned = project.pinned;
        rows = rows.child(
            div()
                .id(SharedString::from(format!("project-directory-{path}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_4()
                .h(px(72.0))
                .px_2()
                .border_b_1()
                .border_color(theme.border())
                .rounded(px(10.0))
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel()))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(16.0))
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .flex_none()
                                .w(px(36.0))
                                .h(px(36.0))
                                .rounded(px(10.0))
                                .bg(theme.panel())
                                .border_1()
                                .border_color(theme.border())
                                .child(icons::icon(Icon::Folder, 18.0, theme.dim())),
                        )
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(3.0))
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(14.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.text())
                                        .child(name),
                                )
                                .child(
                                    div()
                                        .overflow_hidden()
                                        .whitespace_nowrap()
                                        .text_size(px(12.0))
                                        .text_color(theme.faint())
                                        .child(project.display_path.clone()),
                                ),
                        ),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(90.0))
                        .text_size(px(12.5))
                        .text_color(theme.dim())
                        .child(format!("{chats} chats")),
                )
                .child(
                    div()
                        .flex_none()
                        .w(px(92.0))
                        .text_size(px(12.5))
                        .text_color(theme.faint())
                        .child(if updated.is_empty() {
                            "Never".to_string()
                        } else {
                            updated
                        }),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_1()
                        .child(
                            icon_button(new_control, Icon::Plus, 30.0, theme, false)
                                .tooltip(tip("New chat in this project", theme))
                                .on_click(cx.listener(move |root, _, window, cx| {
                                    cx.stop_propagation();
                                    root.push_chat(
                                        window,
                                        new_path.clone(),
                                        None,
                                        "New chat".to_string(),
                                        cx,
                                    );
                                    root.focus_composer(window, cx);
                                })),
                        )
                        .child(
                            icon_button(more_control, Icon::More, 30.0, theme, false)
                                .tooltip(tip("Project actions", theme))
                                .on_click(cx.listener(move |root, _, _, cx| {
                                    cx.stop_propagation();
                                    let pinned = root
                                        .projects
                                        .iter()
                                        .find(|project| project.id == action_id)
                                        .map(|project| project.pinned)
                                        .unwrap_or(pinned);
                                    root.overlay = Some(Overlay::ProjectActions {
                                        id: action_id.clone(),
                                        path: action_path.clone(),
                                        name: action_name.clone(),
                                        pinned,
                                    });
                                    cx.notify();
                                })),
                        ),
                )
                .on_click(cx.listener(move |root, _, window, cx| {
                    root.rail = Rail::Chats;
                    root.sidebar_visible = true;
                    root.select_project(window, open_path.clone(), cx);
                })),
        );
    }

    div()
        .id("projects-page")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .bg(theme.background())
        .child(
            div()
                .flex()
                .flex_col()
                .w_full()
                .max_w(px(1160.0))
                .mx_auto()
                .px(px(48.0))
                .pt(px(30.0))
                .pb(px(48.0))
                .gap_4()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_4()
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(25.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text())
                                .child("Projects"),
                        )
                        .child(Button::new("+  Create").primary().render(
                            "projects-create",
                            theme,
                            cx,
                            |root, _, cx| root.add_project(cx),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(38.0))
                        .px_2()
                        .text_size(px(12.0))
                        .text_color(theme.dim())
                        .child(div().flex_1().child("Name"))
                        .child(div().w(px(90.0)).child("Chats"))
                        .child(div().w(px(92.0)).child("Updated"))
                        .child(div().w(px(64.0))),
                )
                .child(rows),
        )
}

// ---------- customize ----------

pub fn customize_sidebar(
    root: &mut Root,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let sources = root.sources.clone();
    let width = root.sidebar_width;
    let selected = root.customize_tab;
    let mut nav = div().flex().flex_col().gap(px(3.0));
    for (tab, mark, label) in [
        (CustomizeTab::Plugins, Icon::Customize, "Plugins"),
        (CustomizeTab::Skills, Icon::Chat, "Skills"),
        (CustomizeTab::McpServers, Icon::Globe, "MCP servers"),
        (CustomizeTab::Approvals, Icon::Lock, "Approvals"),
    ] {
        nav = nav.child(
            div()
                .id(SharedString::from(format!("customize-{label}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(38.0))
                .px(px(12.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .bg(if selected == tab {
                    theme.panel_3()
                } else {
                    theme.sidebar()
                })
                .text_color(if selected == tab {
                    theme.text()
                } else {
                    theme.dim()
                })
                .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
                .child(icons::icon(mark, 17.0, theme.dim()))
                .child(
                    div()
                        .text_size(px(13.0))
                        .font_weight(FontWeight::MEDIUM)
                        .child(label),
                )
                .on_click(cx.listener(move |root, _, _, cx| {
                    root.customize_tab = tab;
                    root.overlay = None;
                    cx.notify();
                })),
        );
    }

    let mut installed = div()
        .id("customize-installed")
        .flex()
        .flex_col()
        .gap(px(1.0))
        .flex_1()
        .min_h(px(0.0))
        .overflow_y_scroll();
    if sources.is_empty() {
        installed = installed.child(
            div()
                .px(px(12.0))
                .py_1()
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child("No project integrations"),
        );
    } else {
        for source in sources {
            let name = source["name"].as_str().unwrap_or("MCP server").to_string();
            let online = source["state"].as_str() == Some("connected");
            installed = installed.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(32.0))
                    .px(px(12.0))
                    .rounded(px(6.0))
                    .text_size(px(12.5))
                    .text_color(theme.dim())
                    .child(icons::icon(Icon::Globe, 14.0, theme.faint()))
                    .child(div().flex_1().child(name))
                    .child(div().w(px(6.0)).h(px(6.0)).rounded(px(3.0)).bg(if online {
                        theme.success()
                    } else {
                        theme.faint()
                    })),
            );
        }
    }

    div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(width))
        .h_full()
        .min_h(px(0.0))
        .px(px(12.0))
        .pt(px(16.0))
        .pb(px(14.0))
        .gap_3()
        .bg(theme.sidebar())
        .child(
            div()
                .px(px(8.0))
                .pb_1()
                .text_size(px(18.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text())
                .child("Customize"),
        )
        .child(nav)
        .child(
            div()
                .px(px(8.0))
                .pt_2()
                .text_size(px(12.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.dim())
                .child("Installed"),
        )
        .child(installed)
        .child(
            div()
                .id("customize-back")
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(38.0))
                .px(px(10.0))
                .rounded(px(8.0))
                .cursor_pointer()
                .text_size(px(13.0))
                .text_color(theme.dim())
                .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
                .child(icons::icon(Icon::Back, 16.0, theme.faint()))
                .child("Back to chat")
                .on_click(cx.listener(|root, _, _, cx| {
                    root.rail = Rail::Chats;
                    root.overlay = None;
                    cx.notify();
                })),
        )
}

pub fn customize_page(
    root: &mut Root,
    theme: &Theme,
    _window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let project = root
        .selected_project()
        .map(|project| project.display_path.clone())
        .unwrap_or_else(|| "No project selected".to_string());
    let tab = root.customize_tab;
    let (title, subtitle) = match tab {
        CustomizeTab::Plugins => (
            "Plugins",
            "Extend Oxide with tools, skills, and commands from your project and marketplaces.",
        ),
        CustomizeTab::Skills => (
            "Skills",
            "Skills available to chats in the selected project.",
        ),
        CustomizeTab::McpServers => (
            "MCP servers",
            "The servers this project loads, and whether Oxide can reach them.",
        ),
        CustomizeTab::Approvals => (
            "Approvals",
            "Tools this project can run without asking first.",
        ),
    };

    let mut content = div()
        .flex()
        .flex_col()
        .w_full()
        .max_w(px(1100.0))
        .mx_auto()
        .gap_5()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(26.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text())
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(15.0))
                        .line_height(px(23.0))
                        .text_color(theme.dim())
                        .child(subtitle),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.faint())
                        .child(format!("Loading from {project}")),
                ),
        );

    match tab {
        CustomizeTab::Plugins => {
            content = content
                .child(
                    customize_card(theme)
                        .child(
                            div()
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_3()
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .w(px(40.0))
                                        .h(px(40.0))
                                        .rounded(px(11.0))
                                        .bg(theme.panel_2())
                                        .child(icons::icon(Icon::Customize, 21.0, theme.accent())),
                                )
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .gap_1()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .child(
                                            div()
                                                .text_size(px(15.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .text_color(theme.text())
                                                .child("Project plugins"),
                                        )
                                        .child(
                                            div()
                                                .text_size(px(12.5))
                                                .line_height(px(19.0))
                                                .text_color(theme.dim())
                                                .whitespace_normal()
                                                .child("Plugins come from the project's own trusted configuration and the shared plugin store."),
                                        ),
                                )
                                .child(Button::new("Refresh").render(
                                    "customize-plugins-refresh",
                                    theme,
                                    cx,
                                    |root, _, cx| root.load_project_commands(cx),
                                )),
                        ),
                )
                .child(
                    customize_empty(
                        theme,
                        Icon::Customize,
                        "No marketplace plugins are exposed by this engine build.",
                        "The installed project integrations remain available under MCP servers and Approvals.",
                    ),
                );
        }
        CustomizeTab::Skills => {
            let agents = root.info["agents"].as_array().cloned().unwrap_or_default();
            let mut list = div().flex().flex_col().gap_2();
            if agents.is_empty() {
                list = list.child(customize_empty(
                    theme,
                    Icon::Chat,
                    "No project skills loaded",
                    "Trust the project and refresh to load its skills.",
                ));
            } else {
                for agent in agents {
                    let name = agent["name"].as_str().unwrap_or("Agent").to_string();
                    let description = agent["description"].as_str().unwrap_or("").to_string();
                    list = list.child(
                        customize_card(theme)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(icons::icon(Icon::Chat, 20.0, theme.accent()))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_1()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .child(
                                        div()
                                            .text_size(px(14.0))
                                            .font_weight(FontWeight::MEDIUM)
                                            .whitespace_normal()
                                            .child(name),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(12.5))
                                            .text_color(theme.dim())
                                            .whitespace_normal()
                                            .child(description),
                                    ),
                            )
                            .child(pill("Loaded", theme, false)),
                    );
                }
            }
            content = content.child(list);
        }
        CustomizeTab::McpServers => {
            let sources = root.sources.clone();
            let mut list = div().flex().flex_col().gap_2();
            if sources.is_empty() {
                list = list.child(customize_empty(
                    theme,
                    Icon::Globe,
                    "No MCP servers in this project",
                    "Servers configured in mcp.json appear here.",
                ));
            } else {
                for source in sources {
                    let name = source["name"].as_str().unwrap_or("MCP server").to_string();
                    let state = source["state"].as_str().unwrap_or("ready").to_string();
                    let enabled = source["enabled"].as_bool().unwrap_or(true);
                    let label = if !enabled {
                        "Off"
                    } else if state == "connected" {
                        "Connected"
                    } else if state == "needs-auth" {
                        "Needs auth"
                    } else {
                        "Ready"
                    };
                    list = list.child(
                        customize_card(theme)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(icons::icon(Icon::Globe, 19.0, theme.dim()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(14.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .whitespace_normal()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .text_size(px(12.0))
                                    .text_color(if label == "Connected" {
                                        theme.success()
                                    } else {
                                        theme.dim()
                                    })
                                    .child(label),
                            ),
                    );
                }
            }
            content = content.child(list);
        }
        CustomizeTab::Approvals => {
            let approvals = root.approvals.clone();
            let mut list = div().flex().flex_col().gap_2();
            if approvals.is_empty() {
                list = list.child(customize_empty(
                    theme,
                    Icon::Lock,
                    "No pre-approved tools",
                    "Every tool call asks before it runs.",
                ));
            } else {
                for tool in approvals {
                    list = list.child(
                        customize_card(theme)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(icons::icon(Icon::Check, 17.0, theme.success()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(13.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .whitespace_normal()
                                    .child(tool),
                            ),
                    );
                }
                list = list.child(Button::new("Clear approvals").danger().render(
                    "customize-clear-approvals",
                    theme,
                    cx,
                    |root, _, cx| root.clear_approvals(cx),
                ));
            }
            content = content.child(list);
        }
    }

    div()
        .id("customize-page")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .px(px(48.0))
        .pt(px(36.0))
        .pb(px(48.0))
        .bg(theme.background())
        .child(content)
}

fn customize_card(theme: &Theme) -> gpui::Div {
    div()
        .w_full()
        .min_w(px(0.0))
        .flex_none()
        .px(px(18.0))
        .py(px(16.0))
        .rounded(px(12.0))
        .bg(theme.panel())
        .border_1()
        .border_color(theme.border())
}

fn customize_empty(theme: &Theme, mark: Icon, title: &str, detail: &str) -> gpui::Div {
    customize_card(theme)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_2()
        .py(px(44.0))
        .child(icons::icon(mark, 24.0, theme.faint()))
        .child(
            div()
                .text_size(px(14.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text())
                .child(title.to_string()),
        )
        .child(
            div()
                .text_size(px(12.5))
                .text_color(theme.dim())
                .child(detail.to_string()),
        )
}
