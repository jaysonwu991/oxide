//! The docked workspace surfaces: review tabs, the PTY, pull requests and the
//! file viewer.

use crate::app::{PanelTab, Root};
use crate::icons::{self, Icon};
use crate::theme::Theme;
use crate::views::{icon_button, tip};
use gpui::prelude::*;
use gpui::{div, px, Context, FontWeight, SharedString, Window};
use serde_json::Value;

fn tab_button(
    id: &'static str,
    label: &'static str,
    active: bool,
    theme: &Theme,
    cx: &mut Context<Root>,
    tab: PanelTab,
) -> impl IntoElement {
    div()
        .id(id)
        .role(gpui::Role::Tab)
        .aria_label(label)
        .aria_selected(active)
        .flex()
        .items_center()
        .justify_center()
        .flex_none()
        .h(px(26.0))
        .px_2()
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(px(10.5))
        .text_color(if active { theme.text() } else { theme.dim() })
        .bg(if active {
            theme.panel_3()
        } else {
            gpui::transparent_black()
        })
        .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
        .child(label)
        .on_click(cx.listener(move |root, _, window, cx| root.select_panel(tab, window, cx)))
}

pub fn panel_tabs(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    div()
        .id("workspace-tabs")
        .role(gpui::Role::TabList)
        .aria_label("Workspace panels")
        .flex()
        .flex_row()
        .items_center()
        .gap_1()
        .child(tab_button(
            "panel-tab-changes",
            "Changes",
            root.panel_tab == PanelTab::Changes,
            theme,
            cx,
            PanelTab::Changes,
        ))
        .child(tab_button(
            "panel-tab-terminal",
            "Terminal",
            root.panel_tab == PanelTab::Terminal,
            theme,
            cx,
            PanelTab::Terminal,
        ))
        .child(tab_button(
            "panel-tab-pulls",
            "Pulls",
            root.panel_tab == PanelTab::Pulls,
            theme,
            cx,
            PanelTab::Pulls,
        ))
}

fn panel_header(root: &mut Root, title: &str, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    div()
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
                .child(panel_tabs(root, theme, cx))
                .child(div().flex_1())
                .child(
                    icon_button("panel-close", Icon::Close, 26.0, theme, false)
                        .tooltip(tip("Close panel", theme))
                        .on_click(cx.listener(|root, _, _, cx| {
                            root.close_workspace_panel(cx);
                        })),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .child(
                    div()
                        .id("workspace-panel-title")
                        .role(gpui::Role::Heading)
                        .aria_level(2)
                        .aria_label(title.to_string())
                        .flex_1()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title.to_string()),
                ),
        )
}

pub fn panel(
    root: &mut Root,
    theme: &Theme,
    _window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let mut column = div()
        .id("workspace-panel")
        .role(gpui::Role::Region)
        .aria_label(match root.panel_tab {
            PanelTab::Changes => "Changes",
            PanelTab::Terminal => "Terminal",
            PanelTab::Pulls => "Pull requests",
        })
        .flex()
        .flex_col()
        .flex_none()
        .w(px(root.changes_width))
        .h_full()
        .bg(theme.panel())
        .border_l_1()
        .border_color(theme.border());

    match root.panel_tab {
        PanelTab::Changes => {}
        PanelTab::Terminal => column = column.child(terminal(root, theme, cx)),
        PanelTab::Pulls => column = column.child(pulls(root, theme, cx)),
    }
    column
}

fn terminal(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let mut column = panel_header(root, "Terminal", theme, cx);
    let stem = if root.terminal.cwd.is_empty() {
        "Shell".to_string()
    } else {
        root.terminal.cwd.clone()
    };
    column = column.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .px_3()
            .py_1()
            .border_b_1()
            .border_color(theme.border())
            .font_family("Menlo")
            .text_size(px(10.5))
            .text_color(theme.faint())
            .child(format!("{}×{}", root.terminal.cols, root.terminal.rows))
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .overflow_hidden()
                    .whitespace_nowrap()
                    .text_ellipsis()
                    .child(stem),
            )
            .children(root.terminal.exited.then(|| {
                div()
                    .id("terminal-restart")
                    .px_2()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .text_color(theme.accent())
                    .hover(move |style| style.bg(theme.panel_2()))
                    .child("Restart")
                    .on_click(cx.listener(|root, _, _, cx| {
                        root.restart_terminal(cx);
                    }))
            })),
    );

    if let Some(error) = root.terminal.error.clone() {
        column = column.child(
            div()
                .px_3()
                .py_2()
                .text_size(px(11.5))
                .text_color(theme.error())
                .child(error),
        );
    }

    let contents = root.terminal.contents();
    let mut surface = div()
        .id("terminal-surface")
        .role(gpui::Role::Terminal)
        .aria_label(if root.terminal.cwd.is_empty() {
            "Terminal".to_string()
        } else {
            format!("Terminal at {}", root.terminal.cwd)
        })
        .aria_value(contents.clone())
        .track_focus(&root.terminal_focus)
        .key_context("Terminal")
        .focus(move |style| style.border_2().border_color(theme.accent()))
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_scroll()
        .p_2()
        .bg(theme.background())
        .font_family("Menlo")
        .text_size(px(11.0))
        .line_height(px(17.0))
        .text_color(theme.text())
        .on_key_down(cx.listener(|root, event, _, cx| root.terminal_key(event, cx)))
        .on_click(cx.listener(|root, _, window, cx| {
            window.focus(&root.terminal_focus, cx);
        }));

    if contents.is_empty() {
        surface = surface.child(div().text_color(theme.faint()).child(
            if root.terminal.id.is_some() {
                "Waiting for the shell…"
            } else {
                "Select a project to open a shell."
            },
        ));
    } else {
        for row in contents.lines() {
            surface = surface.child(
                div()
                    .flex_none()
                    .min_h(px(17.0))
                    .whitespace_nowrap()
                    .child(row.to_string()),
            );
        }
    }
    column.child(surface)
}

fn pulls(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let mut column = panel_header(root, "Pull requests", theme, cx);
    let mut list = div()
        .id("pulls-list")
        .role(gpui::Role::List)
        .aria_label("Pull requests")
        .flex()
        .flex_col()
        .flex_none()
        .max_h(px(210.0))
        .overflow_scroll()
        .p_2()
        .gap_1()
        .border_b_1()
        .border_color(theme.border());

    if let Some(error) = root.pulls_error.clone() {
        list = list.child(
            div()
                .p_2()
                .text_size(px(11.5))
                .text_color(theme.error())
                .child(error),
        );
    } else if root.pulls.is_empty() {
        list = list.child(
            div()
                .p_2()
                .text_size(px(11.5))
                .text_color(theme.faint())
                .child("This repository has no open requests."),
        );
    }
    for pull in root.pulls.clone() {
        let number = pull["number"].as_u64().unwrap_or_default();
        let title = pull["title"].as_str().unwrap_or("Pull request").to_string();
        let head = pull["head"].as_str().unwrap_or_default();
        let state = pull["state"].as_str().unwrap_or_default();
        let draft = pull["draft"].as_bool().unwrap_or(false);
        list = list.child(
            div()
                .id(SharedString::from(format!("pull-{number}")))
                .role(gpui::Role::Button)
                .aria_label(format!("#{number} {title}"))
                .aria_selected(root.pull_open == Some(number))
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded(px(7.0))
                .cursor_pointer()
                .bg(if root.pull_open == Some(number) {
                    theme.panel_3()
                } else {
                    theme.background()
                })
                .hover(move |style| style.bg(theme.panel_2()))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_2()
                        .child(
                            div()
                                .flex_none()
                                .font_family("Menlo")
                                .text_size(px(10.5))
                                .text_color(theme.accent())
                                .child(format!("#{number}")),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(11.5))
                                .text_color(theme.text())
                                .child(title),
                        ),
                )
                .child(
                    div().text_size(px(10.5)).text_color(theme.faint()).child(
                        [state, if draft { "draft" } else { "" }, head]
                            .into_iter()
                            .filter(|part| !part.is_empty())
                            .collect::<Vec<_>>()
                            .join(" · "),
                    ),
                )
                .on_click(cx.listener(move |root, _, _, cx| root.load_pull(number, cx))),
        );
    }
    column = column.child(list);

    let mut detail = div()
        .id("pull-detail")
        .role(gpui::Role::Region)
        .aria_label("Pull request details")
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_scroll()
        .p_3()
        .gap_3();
    if root.pull_loading {
        detail = detail.child(
            div()
                .text_size(px(11.5))
                .text_color(theme.faint())
                .child("Reading the pull request…"),
        );
    } else if let Some(pull) = root.pull_detail.clone() {
        detail = pull_detail(detail, &pull, theme, cx);
    } else {
        detail = detail.child(
            div()
                .text_size(px(11.5))
                .text_color(theme.faint())
                .child("Choose a pull request to read it."),
        );
    }
    column.child(detail)
}

fn pull_detail(
    mut detail: gpui::Stateful<gpui::Div>,
    pull: &Value,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> gpui::Stateful<gpui::Div> {
    let number = pull["number"].as_u64().unwrap_or_default();
    let title = pull["title"].as_str().unwrap_or("Pull request");
    let state = pull["state"].as_str().unwrap_or_default();
    let head = pull["head"].as_str().unwrap_or_default();
    let base = pull["base"].as_str().unwrap_or_default();
    let author = pull["author"].as_str().unwrap_or_default();
    let additions = pull["additions"].as_u64().unwrap_or_default();
    let deletions = pull["deletions"].as_u64().unwrap_or_default();
    let changed = pull["changed_files"].as_u64().unwrap_or_default();

    detail = detail
        .child(
            div()
                .text_size(px(16.0))
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_normal()
                .child(format!("#{number} {title}")),
        )
        .child(
            div()
                .text_size(px(10.5))
                .text_color(theme.faint())
                .whitespace_normal()
                .child(
                    [
                        state.to_string(),
                        if head.is_empty() {
                            String::new()
                        } else {
                            format!("{head} → {base}")
                        },
                        if author.is_empty() {
                            String::new()
                        } else {
                            format!("@{author}")
                        },
                        format!("+{additions} −{deletions}"),
                        format!("{changed} files"),
                    ]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join(" · "),
                ),
        );

    if let Some(url) = pull["url"].as_str().filter(|url| !url.is_empty()) {
        let url = url.to_string();
        detail = detail.child(
            div()
                .id("pull-open-browser")
                .flex()
                .flex_row()
                .items_center()
                .gap_1()
                .text_size(px(11.5))
                .text_color(theme.accent())
                .cursor_pointer()
                .child("Open on the forge")
                .on_click(cx.listener(move |_, _, _, cx| cx.open_url(&url))),
        );
    }

    if let Some(body) = pull["body"].as_str().filter(|body| !body.trim().is_empty()) {
        detail = detail.child(section_title("Description", theme)).child(
            div()
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(theme.dim())
                .whitespace_normal()
                .child(body.trim().to_string()),
        );
    }

    if let Some(checks) = pull["checks"]
        .as_array()
        .filter(|checks| !checks.is_empty())
    {
        detail = detail.child(section_title("Checks", theme));
        for check in checks {
            let state = check["state"].as_str().unwrap_or("pending");
            let color = match state {
                "pass" => theme.success(),
                "fail" => theme.error(),
                _ => theme.faint(),
            };
            detail = detail.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .text_size(px(11.5))
                    .child(
                        div()
                            .flex_none()
                            .w(px(7.0))
                            .h(px(7.0))
                            .rounded(px(4.0))
                            .bg(color),
                    )
                    .child(
                        div()
                            .flex_1()
                            .child(check["name"].as_str().unwrap_or("Check").to_string()),
                    )
                    .child(div().text_color(theme.faint()).child(state.to_string())),
            );
        }
    }

    if let Some(files) = pull["files"].as_array().filter(|files| !files.is_empty()) {
        detail = detail.child(section_title("Files", theme));
        for file in files {
            let path = file["path"].as_str().unwrap_or_default().to_string();
            let shown = path.clone();
            detail = detail.child(
                div()
                    .id(SharedString::from(format!("pull-file-{path}")))
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .min_h(px(28.0))
                    .px_1()
                    .rounded(px(5.0))
                    .cursor_pointer()
                    .font_family("Menlo")
                    .text_size(px(10.5))
                    .hover(move |style| style.bg(theme.panel_2()))
                    .child(div().flex_1().child(shown))
                    .child(
                        div()
                            .text_color(theme.success())
                            .child(format!("+{}", file["additions"].as_u64().unwrap_or(0))),
                    )
                    .child(
                        div()
                            .text_color(theme.error())
                            .child(format!("−{}", file["deletions"].as_u64().unwrap_or(0))),
                    )
                    .on_click(cx.listener(move |root, _, _, cx| root.open_file(path.clone(), cx))),
            );
        }
    }

    if let Some(comments) = pull["comments"]
        .as_array()
        .filter(|comments| !comments.is_empty())
    {
        detail = detail.child(section_title("Conversation", theme));
        for comment in comments {
            detail = detail.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .rounded(px(7.0))
                    .bg(theme.background())
                    .child(
                        div()
                            .text_size(px(10.5))
                            .text_color(theme.faint())
                            .child(format!(
                                "{} · {}",
                                comment["author"].as_str().unwrap_or("Reviewer"),
                                comment["created_at"].as_str().unwrap_or_default()
                            )),
                    )
                    .child(
                        div()
                            .text_size(px(11.5))
                            .text_color(theme.dim())
                            .whitespace_normal()
                            .child(comment["body"].as_str().unwrap_or_default().to_string()),
                    ),
            );
        }
    }
    detail
}

fn section_title(title: &str, theme: &Theme) -> gpui::Div {
    div()
        .text_size(px(11.0))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.text())
        .child(title.to_string())
}

pub fn file_viewer(
    root: &mut Root,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let Some(viewer) = root.file_viewer.clone() else {
        return div();
    };
    let rendered = viewer.kind == "markdown";
    let file_description: SharedString = if viewer.image.is_some() {
        "Image preview".into()
    } else {
        viewer.text.chars().take(8_000).collect::<String>().into()
    };
    let mut panel = div()
        .flex()
        .flex_col()
        .w(px(860.0))
        .h(px(720.0))
        .max_w_full()
        .max_h_full()
        .rounded(px(12.0))
        .bg(theme.panel())
        .border_1()
        .border_color(theme.border())
        .shadow_xl()
        .overflow_hidden()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px_3()
                .py_2()
                .border_b_1()
                .border_color(theme.border())
                .child(icons::icon(Icon::File, 15.0, theme.faint()))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .font_family("Menlo")
                        .text_size(px(11.5))
                        .child(viewer.path.clone()),
                )
                .children(
                    (rendered && !viewer.loading && viewer.error.is_none()).then(|| {
                        div()
                            .id("file-source-toggle")
                            .h(px(26.0))
                            .px_2()
                            .flex()
                            .items_center()
                            .rounded(px(6.0))
                            .cursor_pointer()
                            .text_size(px(11.0))
                            .text_color(theme.dim())
                            .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
                            .child(if viewer.source { "Preview" } else { "Source" })
                            .on_click(cx.listener(|root, _, _, cx| root.toggle_file_source(cx)))
                    }),
                )
                .child(
                    icon_button("file-viewer-close", Icon::Close, 26.0, theme, false)
                        .tooltip(tip("Close · Esc", theme))
                        .on_click(cx.listener(|root, _, _, cx| {
                            root.overlay = None;
                            root.file_viewer = None;
                            cx.notify();
                        })),
                ),
        );

    let mut body = div()
        .id("file-viewer-body")
        .role(gpui::Role::Dialog)
        .aria_label(format!("File: {}", viewer.path))
        .aria_description(file_description)
        .flex()
        .flex_col()
        .flex_1()
        .min_h(px(0.0))
        .overflow_scroll()
        .p_4()
        .gap_3();
    if viewer.loading {
        body = body.child(
            div()
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child("Reading the file…"),
        );
    } else if let Some(error) = viewer.error {
        body = body.child(
            div()
                .text_size(px(12.0))
                .text_color(theme.error())
                .whitespace_normal()
                .child(error),
        );
    } else if let Some(image) = viewer.image {
        body = body.child(
            gpui::img(image)
                .w_full()
                .max_h(px(620.0))
                .object_fit(gpui::ObjectFit::Contain),
        );
    } else if viewer.kind == "markdown" && !viewer.source {
        body = body.child(crate::views::markdown_document(
            &viewer.text,
            theme,
            790.0,
            window,
        ));
    } else if viewer.kind == "csv" && !viewer.source {
        body = body.child(csv_table(&viewer.text, theme));
    } else {
        if viewer.kind == "html" && !viewer.source {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.faint())
                    .child("HTML is shown as source in this viewer."),
            );
        }
        body = body.child(
            div()
                .w_full()
                .font_family("Menlo")
                .text_size(px(11.0))
                .line_height(px(17.0))
                .text_color(theme.text())
                .whitespace_nowrap()
                .child(viewer.text),
        );
    }
    if viewer.truncated {
        body = body.child(
            div()
                .text_size(px(11.0))
                .text_color(theme.faint())
                .child("This file is longer than the viewer reads, so it is cut here."),
        );
    }
    panel = panel.child(body);
    panel
}

fn csv_table(text: &str, theme: &Theme) -> gpui::Div {
    let mut table = div()
        .flex()
        .flex_col()
        .w_full()
        .font_family("Menlo")
        .text_size(px(10.5))
        .border_1()
        .border_color(theme.border());
    for (index, row) in text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(500)
        .enumerate()
    {
        let mut rendered = div()
            .flex()
            .flex_row()
            .w_full()
            .border_b_1()
            .border_color(theme.border())
            .bg(if index == 0 {
                theme.panel_2()
            } else {
                theme.background()
            });
        for cell in row.split(',').take(12) {
            rendered = rendered.child(
                div()
                    .flex_1()
                    .min_w(px(80.0))
                    .px_2()
                    .py_1()
                    .border_r_1()
                    .border_color(theme.border())
                    .whitespace_nowrap()
                    .child(cell.trim_matches('"').to_string()),
            );
        }
        table = table.child(rendered);
    }
    table
}

pub(crate) fn terminal_key_bytes(
    event: &gpui::KeyDownEvent,
    application_cursor: bool,
) -> Option<String> {
    let stroke = &event.keystroke;
    if stroke.modifiers.platform {
        return None;
    }
    if stroke.modifiers.control {
        let key = stroke.key.to_ascii_lowercase();
        if key == "space" {
            return Some("\0".to_string());
        }
        let mut chars = key.chars();
        let first = chars.next()?;
        if chars.next().is_none() && first.is_ascii() {
            return Some(char::from((first as u8) & 0x1f).to_string());
        }
    }
    let mapped = match stroke.key.as_str() {
        "enter" => "\r",
        "backspace" => "\u{7f}",
        "tab" => "\t",
        "escape" => "\u{1b}",
        "delete" => "\u{1b}[3~",
        "home" => "\u{1b}[H",
        "end" => "\u{1b}[F",
        "pageup" => "\u{1b}[5~",
        "pagedown" => "\u{1b}[6~",
        "up" => {
            if application_cursor {
                "\u{1b}OA"
            } else {
                "\u{1b}[A"
            }
        }
        "down" => {
            if application_cursor {
                "\u{1b}OB"
            } else {
                "\u{1b}[B"
            }
        }
        "right" => {
            if application_cursor {
                "\u{1b}OC"
            } else {
                "\u{1b}[C"
            }
        }
        "left" => {
            if application_cursor {
                "\u{1b}OD"
            } else {
                "\u{1b}[D"
            }
        }
        _ => "",
    };
    if !mapped.is_empty() {
        return Some(mapped.to_string());
    }
    stroke
        .key_char
        .clone()
        .filter(|text| !text.is_empty() && !stroke.modifiers.alt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{Keystroke, Modifiers};

    fn key(key: &str, key_char: Option<&str>) -> gpui::KeyDownEvent {
        gpui::KeyDownEvent {
            keystroke: Keystroke {
                modifiers: Modifiers::default(),
                key: key.to_string(),
                key_char: key_char.map(str::to_string),
            },
            is_held: false,
            prefer_character_input: false,
        }
    }

    #[test]
    fn terminal_keys_follow_the_shells_own_encoding() {
        assert_eq!(
            terminal_key_bytes(&key("enter", None), false).as_deref(),
            Some("\r")
        );
        assert_eq!(
            terminal_key_bytes(&key("up", None), true).as_deref(),
            Some("\u{1b}OA")
        );
        assert_eq!(
            terminal_key_bytes(&key("up", None), false).as_deref(),
            Some("\u{1b}[A")
        );
        assert_eq!(
            terminal_key_bytes(&key("s", Some("s")), false).as_deref(),
            Some("s")
        );
    }
}
