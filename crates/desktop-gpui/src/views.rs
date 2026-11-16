//! The window's panels: the rail, the sidebar, the transcript, the composer and
//! the changes pane.
//!
//! They draw the same furniture the Electron window does — one topbar with the
//! chat tabs, a navigation rail, the project/session sidebar, the conversation
//! and its composer, and the changes a turn left behind — from the same state
//! `crate::app::Root` keeps.

use crate::app::{Button, ChangeScope, ConfirmAction, CustomizeTab, Overlay, Rail, Root};
use crate::icons::{self, Icon};
use crate::markdown::{Block, Inline};
use crate::model::{Attachment, EditedFile, Entry, Status};
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{
    canvas, div, percentage, point, px, Animation, AnimationExt as _, ClickEvent, Context,
    Focusable, Font, FontStyle, FontWeight, KeyDownEvent, MouseButton, PathBuilder, Pixels, Point,
    Render, SharedString, Stateful, StrikethroughStyle, StyledText, TextRun, Transformation,
    UnderlineStyle, Window, WindowControlArea,
};
use serde_json::Value;
use std::collections::HashSet;
use std::time::Duration;

// ---------- shared pieces ----------

const RAIL_WIDTH: f32 = 56.0;
const TOPBAR_HEIGHT: f32 = 50.0;
const SIDEBAR_RESIZER_WIDTH: f32 = 7.0;
pub(crate) const SETTINGS_NAVIGATION_WIDTH: f32 = 232.0;

#[derive(Clone)]
struct ProjectDrag {
    id: String,
    name: String,
    position: Point<Pixels>,
}

impl ProjectDrag {
    fn new(id: String, name: String) -> Self {
        Self {
            id,
            name,
            position: Point::default(),
        }
    }

    fn at(mut self, position: Point<Pixels>) -> Self {
        self.position = position;
        self
    }
}

impl Render for ProjectDrag {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div()
            .pl(px(f32::from(self.position.x) - 70.0))
            .pt(px(f32::from(self.position.y) - 16.0))
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .w(px(180.0))
                    .h(px(32.0))
                    .px_2()
                    .rounded(px(8.0))
                    .bg(gpui::black().opacity(0.82))
                    .shadow_lg()
                    .child(icons::icon(Icon::Folder, 14.0, gpui::white()))
                    .child(
                        div()
                            .text_size(px(12.5))
                            .text_color(gpui::white())
                            .child(self.name.clone()),
                    ),
            )
    }
}

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
        .tab_index(0)
        .role(gpui::Role::Button)
        .aria_label(mark.label())
        .text_color(color)
        .bg(if active {
            theme.panel_2()
        } else {
            gpui::transparent_black()
        })
        .hover(move |style| style.bg(theme.panel_2()))
        .focus(move |style| {
            style
                .bg(theme.panel_3())
                .border_1()
                .border_color(theme.accent())
        })
        .child(icons::icon(
            mark,
            if box_size >= 38.0 {
                20.0
            } else {
                box_size * 0.6
            },
            color,
        ))
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
        .role(gpui::Role::Button)
        .aria_label(mark.label())
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

fn attachment_thumbnail(
    id: impl Into<gpui::ElementId>,
    attachment: &Attachment,
    size: f32,
    radius: f32,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> Stateful<gpui::Div> {
    let image = attachment.image.clone();
    let clickable = image.is_some();
    let preview_image = image.clone();
    let preview_name = attachment.name.clone();
    div()
        .id(id)
        .relative()
        .w(px(size))
        .h(px(size))
        .flex_none()
        .items_center()
        .justify_center()
        .overflow_hidden()
        .rounded(px(radius))
        .bg(theme.panel_3())
        .cursor(if clickable {
            gpui::CursorStyle::PointingHand
        } else {
            gpui::CursorStyle::Arrow
        })
        .children(image.map(|image| {
            gpui::img(image)
                .size_full()
                .object_fit(gpui::ObjectFit::Cover)
                .cursor(gpui::CursorStyle::PointingHand)
                .on_click(cx.listener(move |root, _, _, cx| {
                    let Some(image) = preview_image.clone() else {
                        return;
                    };
                    root.overlay = Some(Overlay::ImagePreview {
                        name: preview_name.clone(),
                        image,
                    });
                    cx.notify();
                }))
        }))
        .children((!clickable).then(|| {
            icons::icon(
                if attachment.data_url.starts_with("data:image") {
                    Icon::Image
                } else {
                    Icon::File
                },
                18.0,
                theme.faint(),
            )
        }))
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

fn styled_height(
    window: &mut Window,
    theme: &Theme,
    spans: &[Inline],
    width: f32,
    font_size: f32,
    line_height: f32,
) -> f32 {
    let (text, runs) = runs(theme, spans);
    window
        .text_system()
        .shape_text(
            SharedString::from(text),
            px(font_size),
            &runs,
            Some(px(width.max(1.0))),
            None,
        )
        .ok()
        .map(|lines| {
            lines
                .iter()
                .map(|line| f32::from(line.size(px(line_height)).height))
                .sum::<f32>()
                .max(line_height)
        })
        .unwrap_or(line_height)
}

fn plain_text_height(
    window: &mut Window,
    text: &str,
    width: f32,
    font_size: f32,
    line_height: f32,
    family: &'static str,
) -> f32 {
    let run = TextRun {
        len: text.len(),
        font: Font {
            family: family.into(),
            features: Default::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        },
        color: gpui::transparent_black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    window
        .text_system()
        .shape_text(
            SharedString::from(text.to_string()),
            px(font_size),
            std::slice::from_ref(&run),
            Some(px(width.max(1.0))),
            None,
        )
        .ok()
        .map(|lines| {
            lines
                .iter()
                .map(|line| f32::from(line.size(px(line_height)).height))
                .sum::<f32>()
                .max(line_height)
        })
        .unwrap_or(line_height)
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
        .role(gpui::Role::Toolbar)
        .aria_label("Window toolbar")
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
                .child(div().flex_1()),
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
        .id("chat-tabs")
        .role(gpui::Role::TabList)
        .aria_label("Open chats")
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
            .role(gpui::Role::Tab)
            .aria_label(title.clone())
            .aria_selected(active)
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
            if root.show_changes {
                root.show_changes = false;
                root.change_review = None;
                cx.notify();
            } else {
                root.open_repository_changes(cx);
            }
        }));
    let computer_use_active = root
        .chats
        .get(root.active)
        .and_then(|chat| chat.computer_use())
        .is_some_and(|activity| activity.running);
    let computer_use = icon_button(
        "topbar-computer-use",
        Icon::Pointer,
        30.0,
        theme,
        computer_use_active,
    )
    .tooltip(tip("Computer use", theme))
    .on_click(cx.listener(|root, _, _, cx| root.open_computer_use(cx)));
    let details = icon_button(
        "details-toggle",
        Icon::Details,
        30.0,
        theme,
        root.show_details,
    )
    .tooltip(tip("Project details", theme))
    .on_click(cx.listener(|root, _, _, cx| root.toggle_details(cx)));
    content = content
        .child(new_chat)
        .child(changes)
        .child(computer_use)
        .child(details)
        .child(div().flex_none().w(px(4.0)));
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

pub(crate) fn customize_destination(current: Rail, leaving_settings: bool) -> Rail {
    if current == Rail::Customize && !leaving_settings {
        Rail::Chats
    } else {
        Rail::Customize
    }
}

pub fn rail(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let chats = root.rail == Rail::Chats && !matches!(root.overlay, Some(Overlay::Settings));
    let projects = root.rail == Rail::Projects && !matches!(root.overlay, Some(Overlay::Settings));
    let settings = matches!(root.overlay, Some(Overlay::Settings));
    let customize = root.rail == Rail::Customize && !settings;
    div()
        .id("primary-navigation")
        .role(gpui::Role::Navigation)
        .aria_label("Primary navigation")
        .flex()
        .flex_col()
        .items_center()
        .flex_none()
        .w(px(RAIL_WIDTH))
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
                .on_click(cx.listener(|root, _, window, cx| {
                    if root.show_changes {
                        root.close_workspace_panel(cx);
                    }
                    root.rail = Rail::Chats;
                    root.overlay = None;
                    root.new_chat(window, cx);
                })),
        )
        .child(
            icon_button("rail-projects", Icon::Folder, 40.0, theme, projects)
                .tooltip(tip("Projects", theme))
                .on_click(cx.listener(|root, _, _, cx| {
                    if root.show_changes {
                        root.close_workspace_panel(cx);
                    }
                    root.rail = if root.rail == Rail::Projects {
                        Rail::Chats
                    } else {
                        Rail::Projects
                    };
                    root.overlay = None;
                    cx.notify();
                })),
        )
        .child(
            icon_button("rail-more", Icon::More, 40.0, theme, false)
                .tooltip(tip("Command menu", theme))
                .on_click(cx.listener(|root, _, window, cx| root.open_command_menu(window, cx))),
        )
        .child(
            div()
                .flex_none()
                .w(px(20.0))
                .h(px(1.0))
                .my(px(5.0))
                .bg(theme.border()),
        )
        .child(
            icon_button("rail-worktrees", Icon::Worktree, 40.0, theme, false)
                .tooltip(tip("Git worktrees", theme))
                .on_click(cx.listener(|root, _, _, cx| {
                    if matches!(root.overlay, Some(Overlay::Worktrees)) {
                        root.overlay = None;
                        cx.notify();
                    } else {
                        root.open_worktrees(cx);
                    }
                })),
        )
        .child(
            icon_button("rail-customize", Icon::Customize, 40.0, theme, customize)
                .tooltip(tip("Customize", theme))
                .on_click(cx.listener(|root, _, _, cx| {
                    if root.show_changes {
                        root.close_workspace_panel(cx);
                    }
                    let settings = matches!(root.overlay, Some(Overlay::Settings));
                    root.overlay = None;
                    root.rail = customize_destination(root.rail, settings);
                    if root.rail == Rail::Customize {
                        root.load_plugins(cx);
                    }
                    cx.notify();
                })),
        )
        .child(div().flex_1())
        .child(
            icon_button("rail-settings", Icon::Settings, 40.0, theme, settings)
                .tooltip(tip("Settings \u{00b7} \u{2318},", theme))
                .on_click(cx.listener(|root, _, _, cx| {
                    if root.show_changes {
                        root.close_workspace_panel(cx);
                    }
                    if matches!(root.overlay, Some(Overlay::Settings)) {
                        root.overlay = None;
                        cx.notify();
                    } else {
                        root.open_overlay(Overlay::Settings, cx);
                    }
                })),
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
    let activities_open = root.activities_open;
    let activity_busy = root.chats.iter().any(|chat| chat.running());
    let mut column = div()
        .id("thread-sidebar")
        .role(gpui::Role::Complementary)
        .aria_label("Projects and chats")
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
        .flex_none()
        .min_h(px(32.0));
    if searching {
        let search_focus = search.clone();
        top = top.child(
            div()
                .id("chat-search")
                .role(gpui::Role::SearchInput)
                .aria_label("Search chats")
                .aria_value(root.query.clone())
                .on_a11y_action(gpui::AccessibleAction::Focus, move |_, window, cx| {
                    window.focus(&search_focus.read(cx).focus_handle(cx), cx);
                })
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
                    .flex_1()
                    .px(px(6.0))
                    .text_size(px(16.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text())
                    .child("Oxide"),
            )
            .child({
                let control = div()
                    .relative()
                    .flex_none()
                    .child(
                        icon_button(
                            "sidebar-activities",
                            Icon::Activity,
                            28.0,
                            theme,
                            activities_open || activity_busy,
                        )
                        .tooltip(tip("Activities", theme))
                        .on_click(cx.listener(|root, _, _, cx| root.toggle_activities(cx))),
                    )
                    .children(activity_busy.then(|| {
                        div()
                            .absolute()
                            .top(px(4.0))
                            .right(px(4.0))
                            .w(px(5.0))
                            .h(px(5.0))
                            .rounded_full()
                            .bg(theme.tool())
                    }));
                control
            })
            .child(
                icon_button("sidebar-search", Icon::Search, 28.0, theme, false)
                    .tooltip(tip("Search chats", theme))
                    .on_click(cx.listener(|root, _, window, cx| root.open_search(window, cx))),
            );
    }
    column = column.child(
        div()
            .flex()
            .flex_col()
            .gap(px(7.0))
            .px(px(14.0))
            .pt(px(17.0))
            .pb(px(11.0))
            .border_b_1()
            .border_color(theme.border())
            .child(top)
            .child(
                div()
                    .id("sidebar-new-chat")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(7.0))
                    .h(px(34.0))
                    .px(px(9.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .hover(move |style| style.bg(theme.panel()))
                    .child(icons::icon(Icon::Pencil, 16.0, theme.dim()))
                    .child(
                        div()
                            .text_size(px(14.0))
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

    if activities_open {
        list = list.child(activity_feed(root, theme, cx));
    } else if !root.query.trim().is_empty() {
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
            let running = root
                .chats
                .iter()
                .any(|chat| chat.session.as_deref() == Some(id.as_str()) && chat.running());
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
                    running,
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
            let path_for_toggle = path.clone();
            let project_id_for_menu = project.id.clone();
            let project_name = name.clone();
            let inline_actions =
                root.inline_project_actions.as_deref() == Some(project.id.as_str());
            let group = SharedString::from(format!("project-row-{}", project.id));
            let new_tooltip = format!("New chat in {name}");
            let group_for_new = group.clone();
            let mut project_group = div().flex().flex_col().mb(px(6.0));
            let header = div()
                .id(SharedString::from(format!("project-{path}")))
                .role(gpui::Role::TreeItem)
                .aria_label(format!("Project {name}"))
                .aria_selected(selected)
                .aria_expanded(!collapsed)
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
                        .role(gpui::Role::Button)
                        .aria_label(if collapsed {
                            format!("Expand project {name}")
                        } else {
                            format!("Collapse project {name}")
                        })
                        .aria_expanded(!collapsed)
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
                                move |root, event: &ClickEvent, _window, cx| {
                                    cx.stop_propagation();
                                    let position = event.position();
                                    root.project_actions_anchor_y = Some(f32::from(position.y));
                                    if root.inline_project_actions.as_deref()
                                        == Some(project_id_for_menu.as_str())
                                    {
                                        root.inline_project_actions = None;
                                    } else {
                                        root.inline_project_actions =
                                            Some(project_id_for_menu.clone());
                                        root.arm_project_actions_dismissal(cx);
                                    }
                                    cx.notify();
                                },
                            )),
                        ),
                )
                .on_click(cx.listener(move |root, _, window, cx| {
                    root.select_project(window, path_for_select.clone(), cx);
                }));
            let drag_value = ProjectDrag::new(project.id.clone(), project_name.clone());
            let drop_target = project.id.clone();
            let project_surface = div()
                .id(SharedString::from(format!(
                    "project-surface-{}",
                    project.id
                )))
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(header)
                .on_drag(drag_value, |drag: &ProjectDrag, position, _, cx| {
                    cx.new(|_| drag.clone().at(position))
                })
                .on_drop(cx.listener(move |root, drag: &ProjectDrag, _, cx| {
                    root.reorder_project(drag.id.clone(), drop_target.clone(), cx)
                }));
            project_group = project_group.child(project_surface);
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
                    let running = root
                        .chats
                        .iter()
                        .any(|chat| chat.session.as_ref() == Some(&id) && chat.running());
                    project_group = project_group.child(session_row(
                        &session,
                        "",
                        None,
                        SessionFlags {
                            pinned,
                            worktree: session.worktree.is_some(),
                            active,
                            running,
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
            if !collapsed && (hidden_sessions > 0 || expanded) {
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
                        .child(if expanded { "Show less" } else { "Show more" })
                        .on_click(cx.listener(move |root, _, _, cx| {
                            if !root.expanded_projects.remove(&path_for_expand) {
                                root.expanded_projects.insert(path_for_expand.clone());
                            }
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
                let running = root
                    .chats
                    .iter()
                    .any(|chat| chat.session.as_ref() == Some(&id) && chat.running());
                recent_rows = recent_rows.child(session_row(
                    &session,
                    "",
                    None,
                    SessionFlags {
                        pinned: session.pinned,
                        worktree: session.worktree.is_some(),
                        active,
                        running,
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

fn activity_feed(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> Stateful<gpui::Div> {
    let mut priority = Vec::new();
    let mut represented = std::collections::HashSet::new();
    for (index, chat) in root.chats.iter().enumerate() {
        let approval = chat.entries.iter().any(|entry| {
            matches!(
                entry,
                Entry::Approval(card) if card.decision.is_none()
            )
        });
        let question = chat.entries.iter().any(|entry| {
            matches!(
                entry,
                Entry::Question(card) if !card.answered
            )
        });
        if chat.running() || approval || question {
            if let Some(session) = &chat.session {
                represented.insert(session.clone());
            }
            let detail = if approval {
                "A tool is waiting for your approval".to_string()
            } else if question {
                "A question is waiting for your answer".to_string()
            } else {
                "A turn is running".to_string()
            };
            priority.push((
                index,
                chat.title.clone(),
                root.project_name(&chat.project),
                detail,
                chat.running(),
            ));
        }
    }

    let mut recent = root.sessions.clone();
    recent.sort_by_key(|session| std::cmp::Reverse(session.modified_at));
    recent.retain(|session| !represented.contains(&session.id));
    recent.truncate(12);

    let mut feed = div()
        .id("sidebar-activities-panel")
        .flex()
        .flex_col()
        .w_full()
        .gap(px(4.0));
    feed = feed.child(section("Priority", theme));
    if priority.is_empty() {
        feed = feed.child(
            div()
                .px(px(10.0))
                .py(px(8.0))
                .text_size(px(11.5))
                .line_height(px(17.0))
                .text_color(theme.faint())
                .child("Nothing is waiting on you."),
        );
    } else {
        for (index, title, project, detail, running) in priority {
            let active = index == root.active;
            feed = feed.child(activity_row(
                ActivityItem {
                    id: SharedString::from(format!("activity-chat-{index}")),
                    title,
                    project,
                    detail,
                    running,
                    active,
                },
                theme,
                cx,
                move |root, window, cx| {
                    root.activate(window, index, cx);
                    cx.notify();
                },
            ));
        }
    }

    if !recent.is_empty() {
        feed = feed.child(section("Recent", theme));
        for session in recent {
            let id = session.id.clone();
            let title = session.title.clone();
            let project = root.project_name(&session.project);
            let detail = session
                .preview
                .lines()
                .next()
                .filter(|line| !line.trim().is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| relative_time(session.modified_at));
            let active = root
                .chats
                .iter()
                .any(|chat| chat.session.as_deref() == Some(id.as_str()));
            feed = feed.child(activity_row(
                ActivityItem {
                    id: SharedString::from(format!("activity-session-{id}")),
                    title,
                    project,
                    detail,
                    running: false,
                    active,
                },
                theme,
                cx,
                move |root, window, cx| {
                    if let Some(session) = root.sessions.iter().find(|row| row.id == id).cloned() {
                        root.open_session(window, session, cx);
                    }
                },
            ));
        }
    }
    feed
}

struct ActivityItem {
    id: SharedString,
    title: String,
    project: String,
    detail: String,
    running: bool,
    active: bool,
}

fn activity_row(
    item: ActivityItem,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> Stateful<gpui::Div> {
    let ActivityItem {
        id,
        title,
        project,
        detail,
        running,
        active,
    } = item;
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(title.clone())
        .aria_description(format!("{project} · {detail}"))
        .aria_selected(active)
        .flex()
        .flex_col()
        .gap(px(3.0))
        .mx(px(6.0))
        .px(px(8.0))
        .py(px(7.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .bg(if active {
            theme.panel()
        } else {
            gpui::transparent_black()
        })
        .hover(move |style| style.bg(theme.panel()))
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
                        .text_size(px(12.5))
                        .font_weight(if active {
                            FontWeight::MEDIUM
                        } else {
                            FontWeight::NORMAL
                        })
                        .text_color(if active { theme.text() } else { theme.dim() })
                        .child(title),
                )
                .children(running.then(|| {
                    div()
                        .flex_none()
                        .w(px(6.0))
                        .h(px(6.0))
                        .rounded_full()
                        .bg(theme.tool())
                })),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_none()
                        .max_w(px(115.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(10.5))
                        .text_color(theme.faint())
                        .child(project),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(10.5))
                        .text_color(theme.faint())
                        .child(detail),
                ),
        )
        .on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
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
        .px(px(16.0))
        .pt(px(14.0))
        .pb(px(8.0))
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
                .on_click(cx.listener(|root, _, window, cx| root.open_new_project(window, cx))),
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

fn project_actions_panel(
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
        .w_full()
        .p(px(10.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(theme.border())
        .bg(theme.panel())
        .shadow_xl()
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

pub(crate) fn project_actions_floating(
    root: &mut Root,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let id = root.inline_project_actions.clone().unwrap_or_default();
    let project = root
        .projects
        .iter()
        .find(|project| project.id == id)
        .cloned();
    let fallback = root.project_actions_anchor_y.unwrap_or(70.0);
    let left = RAIL_WIDTH + root.sidebar_width + SIDEBAR_RESIZER_WIDTH + 8.0;
    let top = (fallback - 16.0).max(TOPBAR_HEIGHT + 8.0);
    let mut card = div()
        .id("project-actions-floating")
        .absolute()
        .left(px(left))
        .top(px(top))
        .w(px(320.0))
        .on_mouse_down_out(cx.listener(|root, _, _, cx| {
            if root.project_actions_dismiss_armed {
                root.inline_project_actions = None;
                cx.notify();
            }
        }));
    if let Some(project) = project {
        let active = root.current_project_owner().as_deref() == Some(project.path.as_str());
        card = card.child(project_actions_panel(
            InlineProjectActions {
                id: project.id,
                path: project.path.clone(),
                name: root.project_name(&project.path),
                pinned: project.pinned,
                session_count: root.sessions_of(&project.path).len(),
                active,
            },
            theme,
            cx,
        ));
    }
    card
}

#[derive(Clone, Copy)]
struct SessionFlags {
    pinned: bool,
    worktree: bool,
    active: bool,
    running: bool,
    indent: f32,
    show_actions: bool,
}

fn session_spinner(id: &str, theme: &Theme) -> impl IntoElement {
    icons::icon(Icon::Spinner, 12.0, theme.accent()).with_animation(
        SharedString::from(format!("session-spinner-{id}")),
        Animation::new(Duration::from_millis(760)).repeat(),
        |icon, delta| icon.with_transformation(Transformation::rotate(percentage(delta))),
    )
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
        running,
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
        .role(gpui::Role::TreeItem)
        .aria_label(title.clone())
        .aria_description(format!("{project} · {meta}"))
        .aria_selected(active)
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
        .children(running.then(|| session_spinner(&id, theme)))
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
    let pull = root
        .pulls
        .iter()
        .find(|pull| {
            pull["head"].as_str() == branch.as_deref()
                && pull["state"]
                    .as_str()
                    .is_some_and(|state| state.eq_ignore_ascii_case("open"))
        })
        .or_else(|| {
            root.pulls.iter().find(|pull| {
                pull["state"]
                    .as_str()
                    .is_some_and(|state| state.eq_ignore_ascii_case("open"))
            })
        })
        .or_else(|| root.pulls.first())
        .cloned();
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
        .role(gpui::Role::Complementary)
        .aria_label("Project details")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_y_scroll()
        .gap(px(2.0))
        .px(px(16.0))
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
            .h(px(34.0))
            .px(px(4.0))
            .border_b_1()
            .border_color(theme.border())
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
        "Changes",
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
            if root.show_changes {
                root.show_changes = false;
                root.change_review = None;
                cx.notify();
            } else {
                root.open_repository_changes(cx);
            }
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

    if let Some(pull) = pull {
        let number = pull["number"].as_u64().unwrap_or(0);
        let state = pull["state"].as_str().unwrap_or_default();
        let url = pull["url"].as_str().unwrap_or_default().to_string();
        if number > 0 {
            let color = if state.eq_ignore_ascii_case("open") {
                theme.success()
            } else if state.eq_ignore_ascii_case("merged") {
                theme.color("pull_merged")
            } else {
                theme.error()
            };
            panel = panel.child(detail_row(
                "details-pull",
                Icon::PullRequest,
                "Pull request",
                Some(vec![(format!("PR #{number}"), color)]),
                theme,
                cx,
                move |root, _, cx| {
                    if url.is_empty() {
                        root.open_pulls(cx);
                    } else {
                        root.open_pulls(cx);
                        root.load_pull(number, cx);
                    }
                },
            ));
        }
    }

    if let Some(activity) = root
        .chats
        .get(root.active)
        .and_then(|chat| chat.computer_use())
    {
        let status = if activity.stopped {
            "Stopped"
        } else if activity.failed {
            "Action failed"
        } else if activity.running {
            "Using computer"
        } else {
            "Finished"
        };
        let action = [activity.app.clone(), Some(activity.action.clone())]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
        let mut card = div()
            .id("details-computer-use")
            .flex()
            .flex_col()
            .gap_2()
            .mt_2()
            .p_2()
            .rounded(px(10.0))
            .border_1()
            .border_color(if activity.failed {
                theme.error()
            } else if activity.running {
                theme.accent()
            } else {
                theme.border()
            })
            .bg(theme.panel_2())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(icons::icon(
                        Icon::Pointer,
                        15.0,
                        if activity.running {
                            theme.accent()
                        } else if activity.failed {
                            theme.error()
                        } else {
                            theme.faint()
                        },
                    ))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text())
                            .child("Computer Use"),
                    )
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(if activity.failed {
                                theme.error()
                            } else if activity.running {
                                theme.accent()
                            } else {
                                theme.faint()
                            })
                            .child(status),
                    ),
            )
            .children((!action.is_empty()).then(|| {
                div()
                    .text_size(px(10.5))
                    .text_color(theme.dim())
                    .child(action)
            }))
            .children(activity.image.as_ref().and_then(|image| {
                image.image.clone().map(|preview| {
                    let data_url = image.data_url.clone();
                    let name = activity
                        .image_app
                        .clone()
                        .unwrap_or_else(|| image.name.clone());
                    div()
                        .id("details-computer-use-preview")
                        .flex()
                        .items_center()
                        .justify_center()
                        .w_full()
                        .max_h(px(190.0))
                        .overflow_hidden()
                        .rounded(px(7.0))
                        .bg(theme.background())
                        .cursor_pointer()
                        .child(gpui::img(preview).w_full().max_h(px(190.0)))
                        .on_click(cx.listener(move |root, _, _, cx| {
                            if let Some(image) = crate::model::image_from_data_url(&data_url) {
                                root.overlay = Some(Overlay::ImagePreview {
                                    name: name.clone(),
                                    image,
                                });
                                cx.notify();
                            }
                        }))
                })
            }))
            .child(div().text_size(px(10.5)).text_color(theme.faint()).child(
                if activity.image.is_some() {
                    "Last captured screenshot"
                } else {
                    "No screenshot captured yet"
                },
            ));
        if activity.running {
            card = card.child(Button::new("Stop task").danger().render(
                "details-computer-use-stop",
                theme,
                cx,
                |root, _, cx| root.stop(cx),
            ));
        }
        panel = panel.child(card);
    }

    panel = panel.child(div().h(px(8.0)));

    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .h(px(30.0))
            .px(px(4.0))
            .border_t_1()
            .border_color(theme.border())
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
                    .h(px(32.0))
                    .px(px(4.0))
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
            .h(px(32.0))
            .px(px(4.0))
            .rounded(px(7.0))
            .cursor_pointer()
            .text_size(px(12.0))
            .text_color(theme.dim())
            .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
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
        .w(px(312.0))
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
        .h(px(36.0))
        .px(px(4.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .text_size(px(12.5))
        .text_color(theme.dim())
        .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
        .child(icons::icon(mark, 16.0, theme.faint()))
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
        .id("main-workspace")
        .role(gpui::Role::Main)
        .aria_label("Conversation workspace")
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
        column = column.child(changes_resizer(root, theme, cx));
        column = column.child(if root.panel_tab == crate::app::PanelTab::Changes {
            changes_panel(root, theme, cx).into_any_element()
        } else {
            crate::workspace::panel(root, theme, window, cx).into_any_element()
        });
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
    let leading = RAIL_WIDTH
        + if root.sidebar_visible {
            root.sidebar_width + SIDEBAR_RESIZER_WIDTH
        } else {
            0.0
        };
    let trailing = (if root.show_details { 312.0 } else { 0.0 })
        + (if root.show_changes {
            root.changes_width + 5.0
        } else {
            0.0
        });
    let reading_width =
        (f32::from(window.viewport_size().width) - leading - trailing - 44.0).clamp(280.0, 740.0);

    let mut transcript = div()
        .id("transcript")
        .role(gpui::Role::Log)
        .aria_label("Conversation transcript")
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
                                onboarding: false,
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
                .id(SharedString::from(format!(
                    "transcript-entry-{index}-{position}"
                )))
                .role(gpui::Role::Article)
                .aria_label(entry_accessibility_label(entry))
                .w_full()
                .max_w(px(740.0))
                .mx_auto()
                .flex_none()
                .child(entry_view(
                    entry,
                    index,
                    position,
                    theme,
                    reading_width,
                    window,
                    cx,
                )),
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
                .id("welcome-heading")
                .role(gpui::Role::Heading)
                .aria_level(1)
                .aria_label("What should we build?")
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
                        |root, window, cx| root.open_new_project(window, cx),
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
                .role(gpui::Role::Button)
                .aria_label(text.to_string())
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
    let review_card = card.clone();
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
            .on_click(cx.listener(move |root, _, _, cx| {
                root.open_turn_changes(&review_card, None, cx);
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

    let shown = card.files.len().min(3);
    for file in card.files.iter().take(shown) {
        let file_path = file.path.clone();
        let review_card = card.clone();
        block = block.child(
            div()
                .id(SharedString::from(format!(
                    "edited-file-{chat}-{file_path}"
                )))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(28.0))
                .px_1()
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel_2()))
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
                )
                .child(icons::icon(Icon::ChevronRight, 11.0, theme.faint()))
                .on_click(cx.listener(move |root, _, _, cx| {
                    root.open_turn_changes(&review_card, Some(file_path.clone()), cx);
                })),
        );
    }
    if card.files.len() > shown {
        let review_card = card.clone();
        block = block.child(
            div()
                .id(("edited-more", chat))
                .flex()
                .items_center()
                .h(px(26.0))
                .px_1()
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel_2()))
                .text_size(px(11.5))
                .text_color(theme.faint())
                .child(format!("Show {} more files", card.files.len() - shown))
                .on_click(cx.listener(move |root, _, _, cx| {
                    root.open_turn_changes(&review_card, None, cx);
                })),
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
    if crate::model::is_computer_tool(name) {
        return "Computer Use";
    }
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

fn entry_accessibility_label(entry: &Entry) -> String {
    match entry {
        Entry::User { text, attachments } => {
            let attachment_text = match attachments.len() {
                0 => String::new(),
                1 => " with 1 attachment".to_string(),
                count => format!(" with {count} attachments"),
            };
            format!("You said: {text}{attachment_text}")
        }
        Entry::Assistant { text, .. } => format!("Oxide said: {text}"),
        Entry::Thinking { text, done, .. } => {
            if *done {
                format!("Oxide thinking: {text}")
            } else {
                format!("Oxide is thinking: {text}")
            }
        }
        Entry::Tool(card) => format!(
            "{} {}: {}",
            tool_label(&card.name),
            card.detail,
            compact_tool_output(&card.output).0
        ),
        Entry::Edited(card) => format!(
            "Edited {} file{}: {}",
            card.files.len(),
            if card.files.len() == 1 { "" } else { "s" },
            card.files
                .iter()
                .map(|file| file.path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Entry::Error(error) => format!("Error: {error}"),
        Entry::Info(info) => info.clone(),
        Entry::Compaction { summary, .. } => format!("Conversation compacted: {summary}"),
        Entry::Approval(card) => format!("Approval needed for {}: {}", card.tool, card.detail),
        Entry::Question(_) => "Oxide has a question".to_string(),
    }
}

fn entry_view(
    entry: &Entry,
    chat: usize,
    position: usize,
    theme: &Theme,
    width: f32,
    window: &mut Window,
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
                .max_w(px(676.0));
            if !text.is_empty() {
                card = card.child(
                    div()
                        .text_size(px(13.5))
                        .line_height(px(21.0))
                        .text_color(theme.text())
                        .child(text.clone()),
                );
            }
            if !attachments.is_empty() {
                card = card.child(
                    div().flex().flex_row().flex_wrap().gap(px(6.0)).children(
                        attachments
                            .iter()
                            .enumerate()
                            .map(|(attachment_position, attachment)| {
                                attachment_thumbnail(
                                    SharedString::from(format!(
                                        "message-attachment-{chat}-{position}-{attachment_position}"
                                    )),
                                    attachment,
                                    96.0,
                                    8.0,
                                    theme,
                                    cx,
                                )
                            }),
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
                column = column.child(block_view(block, theme, width - 22.0, window));
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
                        .min_h(px(plain_text_height(
                            window,
                            text,
                            width - 16.0,
                            12.5,
                            19.0,
                            ".SystemUIFont",
                        )))
                        .text_color(theme.thinking_text())
                        .child(text.clone()),
                )
        }
        Entry::Tool(card) => {
            let computer_use = crate::model::is_computer_tool(&card.name);
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
                } else if computer_use && card.status == Status::Running {
                    theme.accent()
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
                            .child(icons::icon(
                                if computer_use {
                                    Icon::Pointer
                                } else {
                                    Icon::More
                                },
                                13.0,
                                if computer_use {
                                    theme.accent()
                                } else {
                                    theme.faint()
                                },
                            )),
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
                let output_height =
                    plain_text_height(window, &output, width - 46.0, 11.5, 17.0, "Menlo")
                        + if hidden { 17.0 } else { 0.0 };
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
                        .min_h(px(output_height + 16.0))
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
            if !card.media.is_empty() {
                block = block.child(div().flex().flex_row().flex_wrap().gap_2().children(
                    card.media.iter().enumerate().map(|(position, attachment)| {
                        attachment_thumbnail(
                            SharedString::from(format!("tool-attachment-{}-{position}", card.id)),
                            attachment,
                            180.0,
                            9.0,
                            theme,
                            cx,
                        )
                    }),
                ));
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

fn block_view(block: &Block, theme: &Theme, width: f32, window: &mut Window) -> gpui::Div {
    match block {
        Block::Paragraph(spans) => {
            let lines = inline_lines(spans);
            let height = lines
                .iter()
                .map(|line| styled_height(window, theme, line, width, 13.5, 22.0))
                .sum::<f32>()
                .max(22.0);
            div()
                .flex()
                .flex_col()
                .w_full()
                .min_w(px(0.0))
                .min_h(px(height))
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
            .min_h(px(styled_height(
                window,
                theme,
                spans,
                width - 12.0,
                13.0,
                20.0,
            )))
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
            .min_h(px(styled_height(
                window,
                theme,
                spans,
                width - 24.0 - *depth as f32 * 14.0,
                13.5,
                21.0,
            )))
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
            .min_h(px(styled_height(
                window,
                theme,
                spans,
                width - 32.0 - *depth as f32 * 14.0,
                13.5,
                21.0,
            )))
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

pub(crate) fn markdown_document(
    text: &str,
    theme: &Theme,
    width: f32,
    window: &mut Window,
) -> gpui::Div {
    let blocks = crate::markdown::parse(text);
    let mut document = div()
        .flex()
        .flex_col()
        .w_full()
        .gap_3()
        .text_color(theme.text());
    for block in blocks {
        document = document.child(block_view(&block, theme, width, window));
    }
    document
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

fn model_effort_menu(
    root: &mut Root,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> Stateful<gpui::Div> {
    let panel = div()
        .id("composer-model-menu")
        .absolute()
        .right_0()
        .bottom(px(38.0))
        .w(px(310.0))
        .flex()
        .flex_col()
        .overflow_hidden()
        .rounded(px(12.0))
        .border_1()
        .border_color(theme.border())
        .bg(theme.panel())
        .shadow_xl()
        .occlude()
        .on_mouse_down_out(cx.listener(|root, _, _, cx| {
            root.model_menu_open = false;
            root.model_menu_show_models = false;
            cx.notify();
        }));

    if root.model_menu_show_models {
        model_picker_page(panel, root, theme, cx)
    } else {
        model_summary_page(panel, root, theme, cx)
    }
}

fn model_summary_page(
    panel: Stateful<gpui::Div>,
    root: &Root,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> Stateful<gpui::Div> {
    let current_model = root.info["model"].as_str().unwrap_or("Model");
    let model_label = model_display_name(current_model);
    let current_effort = root.info["reasoning"]
        .as_str()
        .unwrap_or("auto")
        .to_string();
    let levels = reasoning_levels(root);
    let show_effort = root.info["supportsReasoning"].as_bool().unwrap_or(false) && levels.len() > 1;

    let model_row = div()
        .id("composer-model-page")
        .flex()
        .flex_row()
        .items_center()
        .justify_center()
        .gap(px(6.0))
        .h(px(36.0))
        .px(px(12.0))
        .rounded(px(8.0))
        .bg(theme.panel_2())
        .cursor_pointer()
        .text_size(px(12.5))
        .font_weight(FontWeight::MEDIUM)
        .text_color(theme.text())
        .hover(move |style| style.bg(theme.panel_3()))
        .child(div().cursor_pointer().child(model_label))
        .child(
            div()
                .cursor_pointer()
                .child(icons::icon(Icon::ChevronRight, 13.0, theme.faint())),
        )
        .on_click(cx.listener(|root, _, _, cx| {
            root.model_menu_show_models = true;
            cx.notify();
        }));

    let mut panel = panel.p(px(10.0));
    if show_effort {
        panel = panel
            .child(
                div()
                    .flex()
                    .justify_center()
                    .pt(px(2.0))
                    .text_size(px(13.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.accent())
                    .child(capitalised(&current_effort)),
            )
            .child(model_row)
            .child(effort_slider(theme, cx, &levels, &current_effort));
    } else {
        panel = panel.child(model_row);
    }
    panel
}

fn model_picker_page(
    panel: Stateful<gpui::Div>,
    root: &Root,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> Stateful<gpui::Div> {
    let header = div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .h(px(40.0))
        .px(px(8.0))
        .border_b_1()
        .border_color(theme.border())
        .child(
            div()
                .id("composer-model-back")
                .flex()
                .items_center()
                .justify_center()
                .w(px(28.0))
                .h(px(28.0))
                .rounded(px(7.0))
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel_2()))
                .child(icons::icon(Icon::Back, 15.0, theme.dim()))
                .on_click(cx.listener(|root, _, _, cx| {
                    root.model_menu_show_models = false;
                    cx.notify();
                })),
        )
        .child(
            div()
                .flex_1()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text())
                .child("Models"),
        )
        .child(
            div()
                .id("composer-model-refresh")
                .flex()
                .items_center()
                .justify_center()
                .h(px(26.0))
                .px(px(8.0))
                .rounded(px(7.0))
                .cursor_pointer()
                .text_size(px(10.5))
                .text_color(theme.dim())
                .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
                .child("Refresh")
                .on_click(cx.listener(|root, _, _, cx| {
                    let project = root.current_project().unwrap_or_default();
                    root.call(
                        "list_models",
                        serde_json::json!({ "project": project, "refresh": true }),
                        cx,
                        |root, result, _| {
                            if let Ok(value) = result {
                                root.models = value;
                            }
                        },
                    );
                })),
        );
    panel
        .child(header)
        .child(model_picker_body(root, theme, cx))
}

fn model_picker_body(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> Stateful<gpui::Div> {
    let data = root.models.clone();
    let current_model = root.info["model"].as_str().unwrap_or_default().to_string();

    let mut body = div()
        .id("composer-model-list")
        .role(gpui::Role::ListBox)
        .aria_label("Models")
        .flex()
        .flex_col()
        .gap(px(2.0))
        .max_h(px(270.0))
        .overflow_y_scroll()
        .p(px(6.0));
    if let Some(error) = data["error"].as_str() {
        body = body
            .child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(11.5))
                    .line_height(px(17.0))
                    .text_color(theme.error())
                    .child(error.to_string()),
            )
            .child(
                div()
                    .id("composer-model-connect")
                    .role(gpui::Role::Button)
                    .aria_label("Connect a provider")
                    .mx_2()
                    .mt_1()
                    .px_2()
                    .py_2()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .text_size(px(11.5))
                    .text_color(theme.accent())
                    .hover(move |style| style.bg(theme.panel_2()))
                    .child("Connect a provider")
                    .on_click(cx.listener(|root, _, _, cx| {
                        root.open_connect("a provider".to_string(), cx)
                    })),
            );
    }
    let providers = data["providers"].as_array().cloned().unwrap_or_default();
    if providers.is_empty() && data["error"].as_str().is_none() {
        body = body.child(
            div()
                .px_2()
                .py_3()
                .text_size(px(11.5))
                .text_color(theme.faint())
                .child("Loading models…"),
        );
    }
    for provider in providers {
        let provider_name = provider["provider"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let active = provider["active"].as_bool().unwrap_or(false);
        let current = provider["current"].as_str().unwrap_or_default().to_string();
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_2()
                .pt(px(7.0))
                .pb(px(3.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(10.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if active {
                            theme.accent()
                        } else {
                            theme.faint()
                        })
                        .child(provider_name.clone()),
                )
                .children((!current.is_empty()).then(|| {
                    div()
                        .text_size(px(10.0))
                        .text_color(theme.faint())
                        .child(current.clone())
                })),
        );
        if let Some(error) = provider["error"].as_str() {
            body = body.child(
                div()
                    .px_2()
                    .py_1()
                    .text_size(px(10.5))
                    .text_color(theme.error())
                    .child(error.to_string()),
            );
        }
        for model in provider["models"].as_array().cloned().unwrap_or_default() {
            let (id, label, detail) = match &model {
                serde_json::Value::String(name) => (name.clone(), model_display_name(name), None),
                other => (
                    other["id"]
                        .as_str()
                        .or_else(|| other["name"].as_str())
                        .unwrap_or_default()
                        .to_string(),
                    other["name"]
                        .as_str()
                        .map(model_display_name)
                        .unwrap_or_else(|| {
                            model_display_name(
                                other["id"]
                                    .as_str()
                                    .or_else(|| other["name"].as_str())
                                    .unwrap_or_default(),
                            )
                        }),
                    other["id"].as_str().and_then(|id| {
                        other["name"]
                            .as_str()
                            .filter(|name| *name != id)
                            .map(|_| id.to_string())
                    }),
                ),
            };
            let selected = active && id == current_model;
            let provider_choice = provider_name.clone();
            let model_choice = id.clone();
            body = body.child(
                div()
                    .id(SharedString::from(format!(
                        "composer-model-{provider_name}-{id}"
                    )))
                    .role(gpui::Role::RadioButton)
                    .aria_label(label.clone())
                    .aria_description(detail.clone().unwrap_or_default())
                    .aria_selected(selected)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_h(px(32.0))
                    .px_2()
                    .rounded(px(7.0))
                    .cursor_pointer()
                    .bg(if selected {
                        theme.panel_2()
                    } else {
                        gpui::transparent_black()
                    })
                    .hover(move |style| style.bg(theme.panel_2()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(11.5))
                                    .text_color(if selected {
                                        theme.accent()
                                    } else {
                                        theme.text()
                                    })
                                    .child(label),
                            )
                            .children(detail.map(|detail| {
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_size(px(10.0))
                                    .text_color(theme.faint())
                                    .child(detail)
                            })),
                    )
                    .children(selected.then(|| icons::icon(Icon::Check, 13.0, theme.accent())))
                    .on_click(cx.listener(move |root, _, _, cx| {
                        root.set_model(provider_choice.clone(), model_choice.clone(), cx)
                    })),
            );
        }
    }
    body
}

fn effort_slider(
    theme: &Theme,
    cx: &mut Context<Root>,
    levels: &[String],
    current: &str,
) -> Stateful<gpui::Div> {
    const TRACK_WIDTH: f32 = 290.0;
    const TRACK_HEIGHT: f32 = 10.0;
    const SLIDER_HEIGHT: f32 = 32.0;
    const KNOB_SIZE: f32 = 26.0;
    let selected = effort_position(levels, current).unwrap_or_default();
    let progress = if levels.len() > 1 {
        selected as f32 / (levels.len() - 1) as f32
    } else {
        0.0
    };
    let knob_left = progress * (TRACK_WIDTH - KNOB_SIZE);
    let mut hits = div()
        .absolute()
        .top(px(0.0))
        .left_0()
        .right_0()
        .bottom_0()
        .flex()
        .flex_row();
    for level in levels {
        let choice = level.clone();
        let label = capitalised(level);
        let down_choice = level.clone();
        let move_choice = level.clone();
        hits = hits.child(
            div()
                .id(SharedString::from(format!("composer-effort-{level}")))
                .flex_1()
                .h_full()
                .cursor_pointer()
                .tooltip(tip(label, theme))
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |root, _, _, cx| {
                        root.effort_drag_level = Some(down_choice.clone());
                        root.set_reasoning(choice.clone(), cx);
                    }),
                )
                .on_mouse_move(
                    cx.listener(move |root, event: &gpui::MouseMoveEvent, _, cx| {
                        if event.dragging()
                            && root.effort_drag_level.as_deref() != Some(move_choice.as_str())
                        {
                            root.effort_drag_level = Some(move_choice.clone());
                            root.set_reasoning(move_choice.clone(), cx);
                        }
                    }),
                ),
        );
    }

    div()
        .id("composer-effort-slider")
        .relative()
        .w(px(TRACK_WIDTH))
        .h(px(SLIDER_HEIGHT))
        .child(
            div()
                .absolute()
                .left_0()
                .right_0()
                .top(px((SLIDER_HEIGHT - TRACK_HEIGHT) / 2.0))
                .h(px(TRACK_HEIGHT))
                .rounded_full()
                .bg(theme.panel_3()),
        )
        .child(
            div()
                .absolute()
                .left_0()
                .top(px((SLIDER_HEIGHT - TRACK_HEIGHT) / 2.0))
                .h(px(TRACK_HEIGHT))
                .w(px(progress * TRACK_WIDTH))
                .rounded_full()
                .bg(theme.accent()),
        )
        .child(
            div()
                .absolute()
                .left(px(knob_left))
                .top(px((SLIDER_HEIGHT - KNOB_SIZE) / 2.0))
                .w(px(KNOB_SIZE))
                .h(px(KNOB_SIZE))
                .rounded_full()
                .bg(theme.text())
                .border_2()
                .border_color(theme.panel()),
        )
        .child(hits)
}

fn reasoning_levels(root: &Root) -> Vec<String> {
    root.info["reasoningLevels"]
        .as_array()
        .map(|levels| {
            levels
                .iter()
                .filter_map(|level| level.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .filter(|levels| !levels.is_empty())
        .unwrap_or_else(|| vec!["auto".to_string()])
}

pub(crate) fn effort_position(levels: &[String], current: &str) -> Option<usize> {
    levels
        .iter()
        .position(|level| level.eq_ignore_ascii_case(current))
}

pub(crate) fn model_display_name(model: &str) -> String {
    model
        .split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| match part.to_ascii_lowercase().as_str() {
            "gpt" => "GPT".to_string(),
            "deepseek" => "DeepSeek".to_string(),
            "claude" => "Claude".to_string(),
            "gemini" => "Gemini".to_string(),
            "qwen" => "Qwen".to_string(),
            "codex" => "Codex".to_string(),
            _ => capitalised(part),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn composer_tools_menu(
    root: &mut Root,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> Stateful<gpui::Div> {
    let project = root.current_project().unwrap_or_default();
    div()
        .id("composer-tools-menu")
        .role(gpui::Role::Menu)
        .aria_label("Composer tools")
        .absolute()
        .left_0()
        .bottom(px(36.0))
        .w(px(220.0))
        .flex()
        .flex_col()
        .p(px(6.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(theme.border())
        .bg(theme.panel())
        .shadow_xl()
        .on_mouse_down_out(cx.listener(|root, _, _, cx| {
            if root.composer_tools_dismiss_armed {
                root.composer_tools_open = false;
                cx.notify();
            }
        }))
        .child(composer_tool_item(
            "composer-tool-connect",
            Icon::Globe,
            "Connect a provider",
            theme,
            cx,
            |root, _, cx| {
                root.composer_tools_open = false;
                root.open_connect("a provider".to_string(), cx);
            },
        ))
        .child(composer_tool_item(
            "composer-tool-project-access",
            Icon::Lock,
            "Project access",
            theme,
            cx,
            move |root, _, cx| {
                root.composer_tools_open = false;
                if !project.is_empty() {
                    root.open_overlay(
                        Overlay::Trust {
                            project: project.clone(),
                            onboarding: false,
                        },
                        cx,
                    );
                }
            },
        ))
        .child(composer_tool_item(
            "composer-tool-agents",
            Icon::Chat,
            "Agents",
            theme,
            cx,
            |root, _, cx| {
                root.composer_tools_open = false;
                root.open_overlay(Overlay::Agents, cx);
            },
        ))
        .child(composer_tool_item(
            "composer-tool-shortcuts",
            Icon::Keyboard,
            "Keyboard shortcuts",
            theme,
            cx,
            |root, _, cx| {
                root.composer_tools_open = false;
                root.open_overlay(Overlay::Shortcuts, cx);
            },
        ))
}

fn composer_tool_item(
    id: &'static str,
    mark: Icon,
    label: &'static str,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> Stateful<gpui::Div> {
    div()
        .id(id)
        .role(gpui::Role::MenuItem)
        .aria_label(label)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(9.0))
        .min_h(px(34.0))
        .px(px(10.0))
        .py(px(6.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(move |style| style.bg(theme.panel_2()))
        .child(icons::icon(mark, 14.0, theme.dim()))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(theme.text())
                .child(label),
        )
        .on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

fn composer(
    root: &mut Root,
    theme: &Theme,
    _scale: f32,
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
    let prediction = (root.prediction_chat == Some(chat.id))
        .then(|| root.prediction.clone())
        .filter(|text| !text.is_empty());
    let attachments = chat.attachments.clone();
    let queued = chat.queued.clone();
    let show_strip = chat.entries.is_empty() && !running;
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
        .id(("composer", chat.id))
        .role(gpui::Role::Form)
        .aria_label("Message composer")
        .key_context(if prediction.is_some() {
            "Composer prediction"
        } else {
            "Composer"
        })
        .on_key_down(cx.listener(|root, event: &KeyDownEvent, _window, cx| {
            let stroke = &event.keystroke;
            if stroke.key == "enter"
                && stroke.modifiers.platform
                && !stroke.modifiers.shift
                && root.prefs["send_key"].as_str() != Some("mod-enter")
            {
                cx.stop_propagation();
                root.open_repository_changes(cx);
            }
        }))
        .on_action(cx.listener(|root, _: &crate::app::Send, window, cx| root.send(window, cx)))
        .on_action(cx.listener(|root, _: &crate::app::Stop, _window, cx| root.stop(cx)))
        .on_action(
            cx.listener(|root, _: &crate::app::AcceptPrediction, window, cx| {
                root.accept_prediction(window, cx)
            }),
        )
        .flex()
        .flex_col()
        .gap(px(2.0))
        .w_full()
        .px(px(12.0))
        .py(px(10.0))
        .bg(theme.panel_2())
        .border_1()
        .border_color(theme.border())
        .rounded(px(20.0));

    // Codex keeps the folder facts on their own narrow strip before a thread
    // starts. Once the thread has a transcript, that room belongs to the
    // message and the normal rounded composer takes the full column.
    let strip = show_strip.then(|| {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_3()
            .w_full()
            .max_w(px(716.0))
            .min_h(px(32.0))
            .px(px(10.0))
            .py(px(4.0))
            .bg(theme.panel_2())
            .border_1()
            .border_color(theme.border())
            .border_b(px(0.0))
            .rounded_t(px(10.0))
            .child(
                div()
                    .id("composer-project")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_1()
                    .flex_none()
                    .cursor_pointer()
                    .rounded(px(6.0))
                    .hover(move |style| style.bg(theme.panel_3()))
                    .child(icons::icon(Icon::Folder, 13.0, theme.faint()))
                    .child(
                        div()
                            .max_w(px(240.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
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
                    .flex_none()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .child(icons::icon(Icon::Branch, 13.0, theme.faint()))
                    .child(
                        div()
                            .min_w(px(0.0))
                            .max_w(px(200.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .text_size(px(11.0))
                            .text_color(theme.faint())
                            .child(branch),
                    )
            }))
            .child(div().flex_1())
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
                    .children(worktree.as_ref().map(|worktree| {
                        div()
                            .max_w(px(160.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .child(worktree.name.clone())
                    }))
                    .on_click(cx.listener(|root, _, _, cx| root.open_worktrees(cx))),
            )
    });

    if !attachments.is_empty() {
        box_row = box_row.child(
            div().flex().flex_row().flex_wrap().gap_2().px_1().children(
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
                            .h(px(54.0))
                            .max_w(px(220.0))
                            .p(px(4.0))
                            .pr(px(5.0))
                            .rounded(px(10.0))
                            .border_1()
                            .border_color(theme.border())
                            .bg(theme.panel_2())
                            .text_size(px(11.5))
                            .text_color(theme.dim())
                            .child(attachment_thumbnail(
                                SharedString::from(format!(
                                    "composer-attachment-preview-{position}"
                                )),
                                attachment,
                                44.0,
                                7.0,
                                theme,
                                cx,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .max_w(px(130.0))
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .child(name),
                            )
                            .child(
                                div()
                                    .id(SharedString::from(format!("attachment-remove-{position}")))
                                    .flex_none()
                                    .p(px(2.0))
                                    .rounded(px(5.0))
                                    .cursor_pointer()
                                    .hover(move |style| style.bg(theme.panel_3()))
                                    .child(icons::icon(Icon::Close, 11.0, theme.faint()))
                                    .on_click(cx.listener(move |root, _, _window, cx| {
                                        if let Some(chat) = root.chats.get_mut(index) {
                                            chat.attachments.remove(position);
                                        }
                                        cx.notify();
                                    })),
                            )
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
                            if !root.run_slash_command(&format!("/{name}"), window, cx) {
                                root.fill_composer(format!("/{name} "), window, cx);
                            }
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
                                window.focus(&editor.read(cx).focus_handle(cx), cx);
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

    if let Some(prediction) = prediction {
        box_row = box_row.child(
            div()
                .id("composer-prediction")
                .role(gpui::Role::Suggestion)
                .aria_label(format!("Suggested next message: {prediction}"))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .min_h(px(30.0))
                .px_2()
                .rounded(px(8.0))
                .bg(theme.background())
                .border_1()
                .border_color(theme.border())
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel_3()))
                .child(
                    div()
                        .flex_none()
                        .text_size(px(10.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.faint())
                        .child("TAB"),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(11.5))
                        .text_color(theme.dim())
                        .child(prediction),
                )
                .child(icons::icon(Icon::ArrowUp, 12.0, theme.faint()))
                .on_click(cx.listener(|root, _, window, cx| root.accept_prediction(window, cx))),
        );
    }

    let input = match editor {
        Some(editor) => {
            let height = (f32::from(editor_height) + 12.0).clamp(50.0, 220.0);
            let focus_editor = editor.clone();
            let value_editor = editor.clone();
            div()
                .id(("composer-input", chat.id))
                .role(gpui::Role::MultilineTextInput)
                .aria_label("Message")
                .aria_value(editor_text.clone())
                .aria_placeholder("Ask Oxide…")
                .on_a11y_action(gpui::AccessibleAction::Focus, move |_, window, cx| {
                    window.focus(&focus_editor.read(cx).focus_handle(cx), cx);
                })
                .on_a11y_action(gpui::AccessibleAction::SetValue, move |data, _, cx| {
                    if let Some(gpui::accesskit::ActionData::Value(value)) = data {
                        value_editor
                            .update(cx, |editor, cx| editor.set_text(value.to_string(), cx));
                    }
                })
                .w_full()
                .h(px(height))
                .px_1()
                .py(px(4.0))
                .overflow_hidden()
                .child(editor.clone())
        }
        None => div().id(("composer-input", chat.id)).h(px(50.0)),
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
        .child({
            let tools_open = root.composer_tools_open;
            let mut tools = div().relative().child(
                icon_button(
                    ("composer-tools", 1usize),
                    Icon::More,
                    30.0,
                    theme,
                    tools_open,
                )
                .tooltip(tip("More tools", theme))
                .on_click(cx.listener(|root, _, _, cx| {
                    root.composer_tools_open = !root.composer_tools_open;
                    root.model_menu_open = false;
                    if root.composer_tools_open {
                        root.arm_composer_tools_dismissal(cx);
                    }
                    cx.notify();
                })),
            );
            if tools_open {
                tools = tools.child(composer_tools_menu(root, theme, cx));
            }
            tools
        })
        .child(
            icon_button(
                ("composer-approvals", 2usize),
                Icon::Lock,
                30.0,
                theme,
                false,
            )
            .tooltip(tip("Tool approvals", theme))
            .on_click(cx.listener(|root, _, _, cx| root.open_overlay(Overlay::Approvals, cx))),
        )
        .child(div().flex_1());

    let mut model_controls = div().relative().flex().flex_row().gap_1().flex_none();
    model_controls = model_controls.child(setting_chip(
        ("chip-model", 1usize),
        format!(
            "{} · {}",
            model_display_name(&model),
            capitalised(&reasoning)
        ),
        theme,
        cx,
        |root, _, cx| root.open_model_menu(cx),
    ));
    if root.model_menu_open {
        model_controls = model_controls.child(model_effort_menu(root, theme, cx));
    }
    footer = footer.child(model_controls);
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
                    root.send(window, cx);
                    if root.overlay.is_none() {
                        root.focus_composer(window, cx);
                    }
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
        .children(strip)
        .child(div().w_full().max_w(px(740.0)).child(box_row))
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
    let access_label = label.into();
    div()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(access_label.clone())
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
        .child(access_label)
        .child(icons::icon(Icon::ChevronDown, 12.0, theme.faint()))
        .on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

// ---------- changes ----------

fn changes_resizer(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let _ = root;
    div()
        .id("changes-resizer")
        .flex_none()
        .w(px(5.0))
        .h_full()
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .bg(theme.panel())
        .border_l_1()
        .border_color(theme.border())
        .hover(move |style| style.bg(theme.panel_3()))
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(|root, event, _, cx| root.begin_changes_drag(event, cx)),
        )
        .on_mouse_move(
            cx.listener(|root, event, window, cx| root.handle_changes_drag(event, window, cx)),
        )
        .on_mouse_up(
            MouseButton::Left,
            cx.listener(|root, _, _, cx| root.end_changes_drag(cx)),
        )
}

fn changes_panel(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let review = root.change_review.clone();
    let scope = review
        .as_ref()
        .map(|review| review.scope)
        .unwrap_or(ChangeScope::Repository);
    let files = review
        .as_ref()
        .map(|review| review.files.clone())
        .unwrap_or_default();
    let added = review.as_ref().map(|review| review.added).unwrap_or(0);
    let removed = review.as_ref().map(|review| review.removed).unwrap_or(0);
    let unified = review.as_ref().is_some_and(|review| review.unified);
    let open_files = review
        .as_ref()
        .map(|review| review.open_files.clone())
        .unwrap_or_default();
    let expanded = review
        .as_ref()
        .map(|review| review.expanded.clone())
        .unwrap_or_default();
    let sides = review
        .as_ref()
        .map(|review| review.sides.clone())
        .unwrap_or_default();
    let loading = review
        .as_ref()
        .map(|review| review.loading.clone())
        .unwrap_or_default();
    let errors = review
        .as_ref()
        .map(|review| review.errors.clone())
        .unwrap_or_default();
    let repository_status = root.repository_status.clone();
    let has_turn = review
        .as_ref()
        .is_some_and(|review| !review.turn_files.is_empty());

    let mut column = div()
        .id("changes-panel")
        .role(gpui::Role::Region)
        .aria_label("Code review")
        .flex()
        .flex_col()
        .flex_none()
        .w(px(root.changes_width))
        .h_full()
        .bg(theme.panel())
        .border_l_1()
        .border_color(theme.border());

    let mut header = div()
        .flex()
        .flex_col()
        .border_b_1()
        .border_color(theme.border())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .pt_2()
                .child(crate::workspace::panel_tabs(root, theme, cx))
                .child(div().flex_1())
                .child(
                    icon_button("changes-close", Icon::Close, 26.0, theme, false)
                        .tooltip(tip("Close panel", theme))
                        .on_click(cx.listener(|root, _, _, cx| {
                            root.close_workspace_panel(cx);
                        })),
                ),
        );
    let mut tools = div()
        .flex()
        .flex_row()
        .flex_wrap()
        .items_center()
        .gap_2()
        .px_3()
        .py_2()
        .child(
            div()
                .flex_none()
                .text_size(px(12.5))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Changes"),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(10.5))
                .text_color(theme.faint())
                .child(format!("{} files", files.len())),
        )
        .child(change_scope_button(
            "changes-scope-turn",
            "Last turn",
            scope == ChangeScope::Turn,
            has_turn,
            theme,
            cx,
            |root, cx| root.set_change_scope(ChangeScope::Turn, cx),
        ))
        .child(change_scope_button(
            "changes-scope-repository",
            "Uncommitted",
            scope == ChangeScope::Repository,
            true,
            theme,
            cx,
            |root, cx| root.set_change_scope(ChangeScope::Repository, cx),
        ))
        .child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(theme.success())
                .child(format!("+{added}")),
        )
        .child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(theme.error())
                .child(format!("\u{2212}{removed}")),
        );

    if root
        .chats
        .get(root.active)
        .and_then(|chat| chat.baseline.clone())
        .is_some()
    {
        tools =
            tools.child(
                Button::new("Undo")
                    .render("changes-undo", theme, cx, |root, _, cx| root.undo_turn(cx)),
            );
    }
    tools = tools
        .child(
            div()
                .id("changes-layout")
                .flex()
                .items_center()
                .justify_center()
                .flex_none()
                .h(px(26.0))
                .px_2()
                .rounded(px(6.0))
                .cursor_pointer()
                .text_size(px(10.5))
                .text_color(theme.dim())
                .hover(move |style| style.bg(theme.panel_3()).text_color(theme.text()))
                .child(if unified {
                    "Split diff"
                } else {
                    "Unified diff"
                })
                .on_click(cx.listener(|root, _, _, cx| root.toggle_change_layout(cx))),
        )
        .child(
            icon_button("changes-refresh", Icon::Activity, 26.0, theme, false)
                .tooltip(tip("Refresh changes", theme))
                .on_click(cx.listener(|root, _, _, cx| root.refresh_change_review(cx))),
        );
    header = header.child(tools);
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

    if files.is_empty() {
        body = body.child(
            div()
                .p_2()
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child("No changes."),
        );
    }
    for file in files {
        let path = file.path.clone();
        let toggle_path = path.clone();
        let open = open_files.contains(&path);
        let mut card = div()
            .flex()
            .flex_col()
            .rounded(px(8.0))
            .bg(theme.background())
            .border_1()
            .border_color(theme.border())
            .overflow_hidden()
            .child(
                div()
                    .id(SharedString::from(format!("change-file-{path}")))
                    .role(gpui::Role::Button)
                    .aria_label(format!("{} change {}", file.status, path))
                    .aria_expanded(open)
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_h(px(38.0))
                    .px_2()
                    .cursor_pointer()
                    .bg(if open {
                        theme.panel_2()
                    } else {
                        theme.background()
                    })
                    .hover(move |style| style.bg(theme.panel_2()))
                    .child(change_status_badge(&file.status, theme))
                    .child(change_path_label(&path, theme))
                    .children((!file.binary).then(|| {
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(theme.success())
                            .child(format!("+{}", file.added))
                    }))
                    .children((!file.binary).then(|| {
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(theme.error())
                            .child(format!("\u{2212}{}", file.removed))
                    }))
                    .child(icons::icon(
                        if open {
                            Icon::ChevronDown
                        } else {
                            Icon::ChevronRight
                        },
                        12.0,
                        theme.faint(),
                    ))
                    .on_click(cx.listener(move |root, _, _, cx| {
                        root.toggle_change_file(toggle_path.clone(), cx)
                    })),
            );
        if open {
            let state = repository_status
                .iter()
                .find(|state| state["path"].as_str() == Some(path.as_str()));
            let staged = state
                .and_then(|state| state["staged"].as_bool())
                .unwrap_or(false);
            let worktree = state
                .and_then(|state| state["worktree"].as_bool())
                .unwrap_or(false);
            if loading.contains(&file.path) {
                card = card.child(change_note("Reading the change…", theme));
            } else if let Some(error) = errors.get(&file.path) {
                card = card.child(change_note(error, theme));
            } else if let Some(lines) = sides.get(&file.path) {
                let mut controls = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_t_1()
                    .border_color(theme.border());
                if worktree {
                    controls = controls.child(Button::new("Stage file").render(
                        SharedString::from(format!("change-stage-{path}")),
                        theme,
                        cx,
                        {
                            let path = path.clone();
                            move |root, _, cx| {
                                root.shape_change("stage".to_string(), Some(path.clone()), None, cx)
                            }
                        },
                    ));
                }
                if staged {
                    controls = controls.child(Button::new("Unstage file").render(
                        SharedString::from(format!("change-unstage-{path}")),
                        theme,
                        cx,
                        {
                            let path = path.clone();
                            move |root, _, cx| {
                                root.shape_change(
                                    "unstage".to_string(),
                                    Some(path.clone()),
                                    None,
                                    cx,
                                )
                            }
                        },
                    ));
                }
                controls = controls
                    .child(div().flex_1())
                    .child(Button::new("Open file").render(
                        SharedString::from(format!("change-open-{path}")),
                        theme,
                        cx,
                        move |root, _, cx| root.open_file(path.clone(), cx),
                    ));
                card = card.child(controls).child(change_review_diff(
                    &file,
                    lines,
                    unified,
                    &expanded,
                    (staged, worktree),
                    theme,
                    cx,
                ));
            } else if !file.diff.is_empty() {
                card = card.child(diff_block(&file.diff, theme));
            } else {
                card = card.child(change_note("Reading the change…", theme));
            }
        }
        body = body.child(card);
    }
    if review.is_none() {
        body = body.child(
            div()
                .p_2()
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child("Open Changes from the composer."),
        );
    }

    column.child(body)
}

fn change_scope_button(
    id: &'static str,
    label: &'static str,
    active: bool,
    enabled: bool,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Context<Root>) + 'static,
) -> Stateful<gpui::Div> {
    let mut button = div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .h(px(26.0))
        .px_2()
        .rounded(px(6.0))
        .text_size(px(10.5))
        .text_color(if active { theme.text() } else { theme.dim() })
        .bg(if active {
            theme.panel_3()
        } else {
            gpui::transparent_black()
        });
    if enabled {
        button = button
            .cursor_pointer()
            .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
            .on_click(cx.listener(move |root, _, _, cx| on_click(root, cx)));
    } else {
        button = button.opacity(0.45);
    }
    button.child(label)
}

fn change_status_badge(status: &str, theme: &Theme) -> gpui::Div {
    let color = match status {
        "added" => theme.success(),
        "deleted" => theme.error(),
        _ => theme.accent(),
    };
    div()
        .flex_none()
        .text_size(px(10.5))
        .font_family("Menlo")
        .text_color(color)
        .child(status_letter(status))
}

fn change_path_label(path: &str, theme: &Theme) -> gpui::Div {
    let slash = path
        .rfind(['/', '\\'])
        .map(|position| position + 1)
        .unwrap_or(0);
    div()
        .flex()
        .flex_row()
        .flex_1()
        .min_w(px(0.0))
        .overflow_hidden()
        .whitespace_nowrap()
        .text_size(px(11.5))
        .child(
            div()
                .flex_none()
                .text_color(theme.faint())
                .child(path[..slash].to_string()),
        )
        .child(
            div()
                .min_w(px(0.0))
                .overflow_hidden()
                .text_color(theme.text())
                .child(path[slash..].to_string()),
        )
}

fn change_note(text: &str, theme: &Theme) -> gpui::Div {
    div()
        .w_full()
        .px_2()
        .py(px(10.0))
        .text_size(px(11.0))
        .text_color(theme.faint())
        .child(text.to_string())
}

fn change_review_diff(
    file: &EditedFile,
    sides: &Value,
    unified: bool,
    expanded: &HashSet<String>,
    state: (bool, bool),
    theme: &Theme,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let (staged, worktree) = state;
    if sides["binary"].as_bool().unwrap_or(false) {
        return div()
            .id(SharedString::from(format!("change-diff-{}", file.path)))
            .w_full()
            .child(change_note("Binary file — no text diff.", theme));
    }
    if sides["omitted"].as_bool().unwrap_or(false) {
        return div()
            .id(SharedString::from(format!("change-diff-{}", file.path)))
            .w_full()
            .child(change_note("File too large to diff line by line.", theme));
    }
    let lines = sides["lines"].as_array().cloned().unwrap_or_default();
    if lines.is_empty() {
        return div()
            .id(SharedString::from(format!("change-diff-{}", file.path)))
            .w_full()
            .child(change_note("No textual changes.", theme));
    }
    let longest = lines
        .iter()
        .filter_map(|line| line["text"].as_str())
        .map(|text| text.chars().count())
        .max()
        .unwrap_or(0) as f32;
    let content_width = if unified {
        (longest * 6.8 + 100.0).max(720.0)
    } else {
        ((longest * 6.8 + 100.0) * 2.0).max(920.0)
    };

    let mut column = div()
        .flex()
        .flex_col()
        .w(px(content_width))
        .font_family("Menlo")
        .text_size(px(10.5))
        .line_height(px(17.0))
        .border_t_1()
        .border_color(theme.border());
    let mut index = 0;
    while index < lines.len() {
        if lines[index]["kind"].as_str() != Some("context") {
            if index == 0 || lines[index - 1]["kind"].as_str() == Some("context") {
                let mut end = index + 1;
                while end < lines.len() && lines[end]["kind"].as_str() != Some("context") {
                    end += 1;
                }
                let hunk = crate::app::change_hunk(&lines, index, end - 1);
                let old = hunk["old_start"].as_u64().unwrap_or(1);
                let new = hunk["new_start"].as_u64().unwrap_or(1);
                let path = file.path.clone();
                let mut actions = div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(28.0))
                    .px_2()
                    .bg(theme.panel());
                actions = actions.child(
                    div()
                        .flex_1()
                        .font_family(".SystemUIFont")
                        .text_size(px(10.5))
                        .text_color(theme.faint())
                        .child(format!("Hunk at {old}/{new}")),
                );
                if worktree {
                    let path_for_stage = path.clone();
                    let hunk_for_stage = hunk.clone();
                    actions = actions.child(Button::new("Stage hunk").render(
                        SharedString::from(format!("stage-hunk-{path}-{index}")),
                        theme,
                        cx,
                        move |root, _, cx| {
                            root.shape_change(
                                "stage-hunk".to_string(),
                                Some(path_for_stage.clone()),
                                Some(hunk_for_stage.clone()),
                                cx,
                            )
                        },
                    ));
                }
                if staged {
                    actions = actions.child(Button::new("Unstage hunk").render(
                        SharedString::from(format!("unstage-hunk-{path}-{index}")),
                        theme,
                        cx,
                        move |root, _, cx| {
                            root.shape_change(
                                "unstage-hunk".to_string(),
                                Some(path.clone()),
                                Some(hunk.clone()),
                                cx,
                            )
                        },
                    ));
                }
                column = column.child(actions);
            }
            column = column.child(change_diff_row(&lines[index], unified, theme));
            index += 1;
            continue;
        }
        let mut end = index;
        while end < lines.len() && lines[end]["kind"].as_str() == Some("context") {
            end += 1;
        }
        let key = format!("{}:{index}", file.path);
        if expanded.contains(&key) || end - index <= 7 {
            for line in &lines[index..end] {
                column = column.child(change_diff_row(line, unified, theme));
            }
        } else {
            for line in &lines[index..index + 3] {
                column = column.child(change_diff_row(line, unified, theme));
            }
            let hidden = end - index - 6;
            let path = file.path.clone();
            let start = index;
            column = column.child(
                div()
                    .id(SharedString::from(format!("change-context-{key}")))
                    .flex()
                    .items_center()
                    .h(px(24.0))
                    .px_2()
                    .cursor_pointer()
                    .bg(theme.panel())
                    .text_size(px(10.5))
                    .text_color(theme.faint())
                    .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
                    .child(format!("{hidden} unmodified lines"))
                    .on_click(cx.listener(move |root, _, _, cx| {
                        root.expand_change_context(path.clone(), start, cx)
                    })),
            );
            for line in &lines[end - 3..end] {
                column = column.child(change_diff_row(line, unified, theme));
            }
        }
        index = end;
    }
    div()
        .id(SharedString::from(format!("change-diff-{}", file.path)))
        .w_full()
        .overflow_scroll()
        .child(column)
}

fn change_diff_row(line: &Value, unified: bool, theme: &Theme) -> gpui::Div {
    let kind = line["kind"].as_str().unwrap_or("context");
    let old = line["old"].as_u64().map(|number| number.to_string());
    let new = line["new"].as_u64().map(|number| number.to_string());
    let text = line["text"].as_str().unwrap_or_default();
    let (background, color) = match kind {
        "add" => (theme.wash("success", 0.12), theme.success()),
        "remove" => (theme.wash("error", 0.12), theme.error()),
        _ => (gpui::transparent_black(), theme.dim()),
    };
    let mut row = div()
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_h(px(17.0))
        .bg(background);
    if unified {
        row = row
            .child(change_line_number(old.as_deref(), theme))
            .child(change_line_number(new.as_deref(), theme))
            .child(change_line_text(text, color));
    } else {
        row = row
            .child(change_line_number(old.as_deref(), theme))
            .child(change_line_text(
                if kind == "add" { "" } else { text },
                color,
            ))
            .child(change_line_number(new.as_deref(), theme))
            .child(change_line_text(
                if kind == "remove" { "" } else { text },
                color,
            ));
    }
    row
}

fn change_line_number(number: Option<&str>, theme: &Theme) -> gpui::Div {
    div()
        .flex_none()
        .w(px(32.0))
        .pr_1()
        .text_size(px(10.0))
        .text_color(theme.faint())
        .child(number.unwrap_or_default().to_string())
}

fn change_line_text(text: &str, color: gpui::Hsla) -> gpui::Div {
    div()
        .flex_1()
        .min_w(px(0.0))
        .whitespace_nowrap()
        .pr(px(6.0))
        .text_color(color)
        .child(text.to_string())
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
                    |root, window, cx| root.open_new_project(window, cx),
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
                .role(gpui::Role::Button)
                .aria_label(format!("Open project {name}"))
                .aria_description(format!("{chats} chats · updated {updated}"))
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
        .role(gpui::Role::Main)
        .aria_label("Projects")
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
                            |root, window, cx| root.open_new_project(window, cx),
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
                .role(gpui::Role::Tab)
                .aria_label(label.to_string())
                .aria_selected(selected == tab)
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
    let catalog_loading = tab == CustomizeTab::Plugins && root.plugin_catalog.is_null();
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
                        .id("customize-title")
                        .role(gpui::Role::Heading)
                        .aria_level(1)
                        .aria_label(title.to_string())
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
                .children(catalog_loading.then(|| {
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.faint())
                        .child(format!("Loading plugins from {project}"))
                })),
        );

    match tab {
        CustomizeTab::Plugins => {
            let catalog = root.plugin_catalog.clone();
            let query = root.plugin_query.read(cx).text().to_ascii_lowercase();
            let plugin_query_focus = root.plugin_query.clone();
            let plugin_query_value = root.plugin_query.clone();
            let marketplace_focus = root.marketplace_source.clone();
            let marketplace_value = root.marketplace_source.clone();
            let installed = catalog["installed"].as_array().cloned().unwrap_or_default();
            let marketplaces = catalog["marketplaces"]
                .as_array()
                .cloned()
                .unwrap_or_default();

            let toolbar = customize_card(theme)
                .flex()
                .flex_col()
                .gap_3()
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("plugin-search")
                                .role(gpui::Role::SearchInput)
                                .aria_label("Search plugins")
                                .aria_value(query.clone())
                                .on_a11y_action(
                                    gpui::AccessibleAction::Focus,
                                    move |_, window, cx| {
                                        window.focus(
                                            &plugin_query_focus.read(cx).focus_handle(cx),
                                            cx,
                                        );
                                    },
                                )
                                .on_a11y_action(
                                    gpui::AccessibleAction::SetValue,
                                    move |data, _, cx| {
                                        if let Some(gpui::accesskit::ActionData::Value(value)) =
                                            data
                                        {
                                            plugin_query_value.update(cx, |editor, cx| {
                                                editor.set_text(value.to_string(), cx)
                                            });
                                        }
                                    },
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|root, _, window, cx| {
                                        window.focus(
                                            &root.marketplace_source.read(cx).focus_handle(cx),
                                            cx,
                                        );
                                    }),
                                )
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .flex_1()
                                .h(px(34.0))
                                .px_2()
                                .rounded(px(8.0))
                                .bg(theme.background())
                                .border_1()
                                .border_color(theme.border())
                                .child(icons::icon(Icon::Search, 14.0, theme.faint()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .h_full()
                                        .child(root.plugin_query.clone()),
                                ),
                        )
                        .child(Button::new("Refresh").render(
                            "customize-plugins-refresh",
                            theme,
                            cx,
                            |root, _, cx| root.load_plugins(cx),
                        )),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("marketplace-source")
                                .role(gpui::Role::TextInput)
                                .aria_label("Marketplace URL or owner/repo")
                                .aria_value(root.marketplace_source.read(cx).text().to_string())
                                .on_a11y_action(
                                    gpui::AccessibleAction::Focus,
                                    move |_, window, cx| {
                                        window.focus(
                                            &marketplace_focus.read(cx).focus_handle(cx),
                                            cx,
                                        );
                                    },
                                )
                                .on_a11y_action(
                                    gpui::AccessibleAction::SetValue,
                                    move |data, _, cx| {
                                        if let Some(gpui::accesskit::ActionData::Value(value)) =
                                            data
                                        {
                                            marketplace_value.update(cx, |editor, cx| {
                                                editor.set_text(value.to_string(), cx)
                                            });
                                        }
                                    },
                                )
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(|root, _, window, cx| {
                                        window.focus(
                                            &root.plugin_query.read(cx).focus_handle(cx),
                                            cx,
                                        );
                                    }),
                                )
                                .flex()
                                .flex_row()
                                .items_center()
                                .gap_2()
                                .flex_1()
                                .h(px(34.0))
                                .px_2()
                                .rounded(px(8.0))
                                .bg(theme.background())
                                .border_1()
                                .border_color(theme.border())
                                .child(icons::icon(Icon::Globe, 14.0, theme.faint()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.0))
                                        .h_full()
                                        .child(root.marketplace_source.clone()),
                                ),
                        )
                        .child(Button::new("Add marketplace").render(
                            "customize-marketplace-add",
                            theme,
                            cx,
                            |root, _, cx| {
                                let source =
                                    root.marketplace_source.read(cx).text().trim().to_string();
                                if source.is_empty() {
                                    root.toast("Enter a marketplace URL or path.", true, cx);
                                    return;
                                }
                                root.change_plugin("add_marketplace".to_string(), source, None, cx);
                                root.marketplace_source
                                    .update(cx, |editor, cx| editor.clear(cx));
                            },
                        )),
                )
                .children(root.plugin_message.clone().map(|message| {
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.dim())
                        .child(message)
                }));
            content = content.child(toolbar);

            let mut installed_list = div().flex().flex_col().gap_2();
            let mut installed_shown = false;
            for plugin in installed {
                let name = plugin["name"].as_str().unwrap_or("Plugin").to_string();
                let description = plugin["description"].as_str().unwrap_or("").to_string();
                let version = plugin["version"].as_str().unwrap_or("").to_string();
                let haystack = format!("{name} {description}").to_ascii_lowercase();
                if !query.is_empty() && !haystack.contains(&query) {
                    continue;
                }
                installed_shown = true;
                let enabled = plugin["enabled"].as_bool().unwrap_or(false);
                let managed = plugin["managed"].as_bool().unwrap_or(false);
                let action_name = name.clone();
                let uninstall_name = name.clone();
                installed_list = installed_list.child(
                    customize_card(theme)
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(38.0))
                                .h(px(38.0))
                                .rounded(px(10.0))
                                .bg(theme.panel_2())
                                .child(icons::icon(Icon::Customize, 19.0, theme.accent())),
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
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_size(px(14.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(name),
                                        )
                                        .children((!version.is_empty()).then(|| {
                                            div()
                                                .text_size(px(10.5))
                                                .text_color(theme.faint())
                                                .child(version)
                                        }))
                                        .child(pill(
                                            if enabled { "Enabled" } else { "Disabled" },
                                            theme,
                                            false,
                                        )),
                                )
                                .child(
                                    div()
                                        .text_size(px(12.0))
                                        .text_color(theme.dim())
                                        .whitespace_normal()
                                        .child(if description.is_empty() {
                                            "Installed plugin".to_string()
                                        } else {
                                            description
                                        }),
                                ),
                        )
                        .child(
                            Button::new(if enabled { "Disable" } else { "Enable" }).render(
                                SharedString::from(format!("plugin-toggle-{action_name}")),
                                theme,
                                cx,
                                move |root, _, cx| {
                                    root.change_plugin(
                                        if enabled { "disable" } else { "enable" }.to_string(),
                                        action_name.clone(),
                                        None,
                                        cx,
                                    )
                                },
                            ),
                        )
                        .children((!managed).then(|| {
                            Button::new("Uninstall").danger().render(
                                SharedString::from(format!("plugin-uninstall-{uninstall_name}")),
                                theme,
                                cx,
                                move |root, _, cx| {
                                    root.change_plugin(
                                        "uninstall".to_string(),
                                        uninstall_name.clone(),
                                        None,
                                        cx,
                                    )
                                },
                            )
                        })),
                );
            }
            if !installed_shown {
                installed_list = installed_list.child(customize_empty(
                    theme,
                    Icon::Customize,
                    "No installed plugins",
                    "Add a marketplace below to discover plugins.",
                ));
            }
            content = content.child(
                div()
                    .text_size(px(12.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.dim())
                    .child("Installed"),
            );
            content = content.child(installed_list);

            if marketplaces.is_empty() {
                content = content.child(customize_empty(
                    theme,
                    Icon::Globe,
                    "No marketplaces",
                    "Add an owner/repository or a local marketplace path above.",
                ));
            }
            for marketplace in marketplaces {
                let marketplace_name = marketplace["name"]
                    .as_str()
                    .unwrap_or("Marketplace")
                    .to_string();
                let source = marketplace["source"]
                    .as_str()
                    .unwrap_or_default()
                    .to_string();
                let update_name = marketplace_name.clone();
                let remove_name = marketplace_name.clone();
                let mut group = customize_card(theme).flex().flex_col().gap_2().child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("projects-title")
                                .role(gpui::Role::Heading)
                                .aria_level(1)
                                .aria_label("Projects")
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .text_size(px(14.0))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(marketplace_name),
                                )
                                .child(
                                    div()
                                        .text_size(px(10.5))
                                        .text_color(theme.faint())
                                        .whitespace_normal()
                                        .child(source),
                                ),
                        )
                        .child(Button::new("Update").render(
                            SharedString::from(format!("marketplace-update-{update_name}")),
                            theme,
                            cx,
                            move |root, _, cx| {
                                root.change_plugin(
                                    "update_marketplace".to_string(),
                                    update_name.clone(),
                                    None,
                                    cx,
                                )
                            },
                        ))
                        .child(Button::new("Remove").danger().render(
                            SharedString::from(format!("marketplace-remove-{remove_name}")),
                            theme,
                            cx,
                            move |root, _, cx| {
                                root.change_plugin(
                                    "remove_marketplace".to_string(),
                                    remove_name.clone(),
                                    None,
                                    cx,
                                )
                            },
                        )),
                );
                if let Some(error) = marketplace["error"].as_str() {
                    group = group.child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.error())
                            .whitespace_normal()
                            .child(error.to_string()),
                    );
                }
                for plugin in marketplace["plugins"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
                {
                    let name = plugin["name"].as_str().unwrap_or("Plugin").to_string();
                    let description = plugin["description"].as_str().unwrap_or("").to_string();
                    let version = plugin["version"].as_str().unwrap_or("").to_string();
                    let haystack = format!("{name} {description}").to_ascii_lowercase();
                    if !query.is_empty() && !haystack.contains(&query) {
                        continue;
                    }
                    let installed_here = plugin["installed"].as_bool().unwrap_or(false);
                    let enabled_here = plugin["enabled"].as_bool().unwrap_or(false);
                    let action_name = name.clone();
                    let marketplace_id =
                        marketplace["name"].as_str().unwrap_or_default().to_string();
                    let mut row = div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_3()
                        .p_2()
                        .rounded(px(8.0))
                        .bg(theme.background())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .flex_1()
                                .min_w(px(0.0))
                                .child(
                                    div()
                                        .flex()
                                        .flex_row()
                                        .items_center()
                                        .gap_2()
                                        .child(
                                            div()
                                                .text_size(px(13.0))
                                                .font_weight(FontWeight::MEDIUM)
                                                .child(name),
                                        )
                                        .children((!version.is_empty()).then(|| {
                                            div()
                                                .text_size(px(10.5))
                                                .text_color(theme.faint())
                                                .child(version)
                                        })),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.5))
                                        .text_color(theme.dim())
                                        .whitespace_normal()
                                        .child(if description.is_empty() {
                                            "No description provided.".to_string()
                                        } else {
                                            description
                                        }),
                                ),
                        );
                    if installed_here {
                        row = row.child(pill(
                            if enabled_here { "Enabled" } else { "Disabled" },
                            theme,
                            false,
                        ));
                    } else {
                        row = row.child(Button::new("Install").primary().render(
                            SharedString::from(format!("plugin-install-{action_name}")),
                            theme,
                            cx,
                            move |root, _, cx| {
                                root.change_plugin(
                                    "install".to_string(),
                                    action_name.clone(),
                                    Some(marketplace_id.clone()),
                                    cx,
                                )
                            },
                        ));
                    }
                    group = group.child(row);
                }
                content = content.child(group);
            }
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
                    let exposure = source["exposure"]
                        .as_str()
                        .unwrap_or("deferred")
                        .to_string();
                    let scope = source["scope"].as_str().unwrap_or("project").to_string();
                    let detail = source["detail"].as_str().unwrap_or_default().to_string();
                    let label = if !enabled {
                        "Off"
                    } else if state == "connected" {
                        "Connected"
                    } else if state == "needs-auth" {
                        "Needs auth"
                    } else {
                        "Ready"
                    };
                    let toggle_name = name.clone();
                    let exposure_name = name.clone();
                    let authorize_name = name.clone();
                    list = list.child(
                        customize_card(theme)
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_3()
                            .child(icons::icon(Icon::Globe, 19.0, theme.dim()))
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
                                            .text_size(px(11.0))
                                            .text_color(theme.faint())
                                            .whitespace_normal()
                                            .child(detail),
                                    ),
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
                            )
                            .children((state == "needs-auth").then(|| {
                                Button::new("Authorize").primary().render(
                                    SharedString::from(format!("mcp-authorize-{authorize_name}")),
                                    theme,
                                    cx,
                                    move |root, _, cx| {
                                        root.authorize_mcp(
                                            authorize_name.clone(),
                                            scope.clone(),
                                            cx,
                                        )
                                    },
                                )
                            }))
                            .child(
                                Button::new(format!("Expose: {}", capitalised(&exposure))).render(
                                    SharedString::from(format!("mcp-exposure-{exposure_name}")),
                                    theme,
                                    cx,
                                    move |root, _, cx| {
                                        root.cycle_mcp_exposure(
                                            exposure_name.clone(),
                                            exposure.clone(),
                                            cx,
                                        )
                                    },
                                ),
                            )
                            .child(
                                Button::new(if enabled { "Disable" } else { "Enable" }).render(
                                    SharedString::from(format!("mcp-toggle-{toggle_name}")),
                                    theme,
                                    cx,
                                    move |root, _, cx| {
                                        root.toggle_mcp(toggle_name.clone(), !enabled, cx)
                                    },
                                ),
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
        .role(gpui::Role::Main)
        .aria_label("Customize")
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

#[cfg(test)]
mod accessibility_tests {
    use super::entry_accessibility_label;
    use crate::model::{Attachment, Entry};

    #[test]
    fn transcript_entries_have_speaker_and_content_labels() {
        let user = Entry::User {
            text: "Explain this file".to_string(),
            attachments: vec![Attachment::new(
                "shot.png".to_string(),
                "data:image/png;base64,iVBORw0KGgo=".to_string(),
            )],
        };
        assert_eq!(
            entry_accessibility_label(&user),
            "You said: Explain this file with 1 attachment"
        );

        let assistant = Entry::Assistant {
            text: "It opens the project.".to_string(),
            blocks: Vec::new(),
            streaming: false,
        };
        assert_eq!(
            entry_accessibility_label(&assistant),
            "Oxide said: It opens the project."
        );
    }
}
