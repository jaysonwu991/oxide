//! The window's dialogs: settings, models, themes, language, shortcuts, trust,
//! approvals, questions and confirmations.
//!
//! They are the same surfaces the Electron window opens — the same commands,
//! the same stored preferences, the same approval broker — drawn as GPUI panels
//! over the conversation.

use crate::app::{Button, ConfirmAction, Overlay, Root};
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{div, hsla, px, Context, FontWeight, SharedString, Window};
use serde_json::{json, Value};

pub fn layer(
    root: &mut Root,
    theme: &Theme,
    _scale: f32,
    window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let overlay = root.overlay.clone();
    let mut scrim = div()
        .absolute()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(hsla(0.0, 0.0, 0.0, 0.45));

    let Some(overlay) = overlay else {
        return scrim;
    };

    let panel = match overlay.clone() {
        Overlay::Settings => settings(root, theme, cx),
        Overlay::Models => models(root, theme, cx),
        Overlay::Themes => themes(root, theme, cx),
        Overlay::Language => language(root, theme, cx),
        Overlay::Shortcuts => shortcuts(theme, cx),
        Overlay::Trust { project } => trust(root, theme, project, cx),
        Overlay::Approval { chat, id } => approval(root, theme, chat, id, cx),
        Overlay::Question { chat, id } => question(root, theme, chat, id, cx),
        Overlay::Confirm {
            title,
            body,
            action,
        } => confirm(theme, title, body, action, cx),
        Overlay::Connect { provider } => connect(theme, provider, cx),
        Overlay::NewProject => new_project(theme, cx),
        Overlay::SessionActions { id, title, pinned } => {
            session_actions(theme, id, title, pinned, cx)
        }
        Overlay::ProjectActions {
            id,
            path,
            name,
            pinned,
        } => project_actions(theme, id, path, name, pinned, cx),
        Overlay::Archived => archived(root, theme, cx),
    };

    scrim = scrim.child(panel);
    let _ = window;
    scrim
}

fn dialog(theme: &Theme, title: &str, width: f32) -> gpui::Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .w(px(width))
        .max_h(px(620.0))
        .p_4()
        .bg(theme.panel())
        .border_1()
        .border_color(theme.border())
        .rounded(px(14.0))
        .child(
            div().flex().flex_row().items_center().child(
                div()
                    .flex_1()
                    .text_size(px(15.0))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text())
                    .child(title.to_string()),
            ),
        )
}

fn close_button(theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    div()
        .id("overlay-close")
        .flex()
        .items_center()
        .justify_center()
        .w(px(26.0))
        .h(px(26.0))
        .rounded(px(7.0))
        .cursor_pointer()
        .text_color(theme.dim())
        .hover(move |style| style.bg(theme.panel_2()))
        .child("×")
        .on_click(cx.listener(|root, _, _, cx| {
            root.overlay = None;
            cx.notify();
        }))
}

fn selectable(
    id: impl Into<gpui::ElementId>,
    label: String,
    detail: Option<String>,
    selected: bool,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> impl IntoElement {
    let mut row = div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .px_2()
        .py_1()
        .rounded(px(7.0))
        .cursor_pointer()
        .hover(move |style| style.bg(theme.panel_2()))
        .child(
            div().flex_1().min_w(px(0.0)).flex().flex_col().child(
                div()
                    .text_size(px(12.5))
                    .text_color(if selected {
                        theme.accent()
                    } else {
                        theme.text()
                    })
                    .child(label),
            ),
        );
    if let Some(detail) = detail {
        row = row.child(
            div()
                .flex_none()
                .text_size(px(11.0))
                .text_color(theme.faint())
                .child(detail),
        );
    }
    if selected {
        row = row.child(div().text_color(theme.accent()).child("✓"));
    }
    row.on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

fn setting_row(
    id: impl Into<gpui::ElementId>,
    label: &str,
    detail: &str,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .flex_col()
        .gap_1()
        .p_2()
        .rounded(px(8.0))
        .bg(theme.panel_2())
        .cursor_pointer()
        .hover(move |style| style.bg(theme.panel_3()))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(theme.text())
                .child(label.to_string()),
        )
        .child(
            div()
                .text_size(px(11.5))
                .text_color(theme.dim())
                .child(detail.to_string()),
        )
        .on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

// ---------- settings ----------

fn settings(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let prefs = root.prefs.clone();
    let send_key = prefs["send_key"].as_str().unwrap_or("enter").to_string();
    let sidebar = prefs["sidebar_visible"].as_bool().unwrap_or(true);
    let predictions = prefs["predictions"].as_bool().unwrap_or(false);
    let notifications = prefs["notifications"]
        .as_str()
        .unwrap_or("background")
        .to_string();
    let scale = prefs["font_scale"].as_f64().unwrap_or(1.0);
    let version = env!("CARGO_PKG_VERSION");

    let mut panel = dialog(theme, "Settings", 560.0)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .id("settings-body")
                .overflow_y_scroll()
                .max_h(px(440.0))
                .child(setting_row(
                    "setting-send-key",
                    "Send with",
                    &format!("Enter, or {send_key} — currently {send_key}"),
                    theme,
                    cx,
                    move |root, _, cx| {
                        let next = if send_key == "enter" {
                            "cmd+enter"
                        } else {
                            "enter"
                        };
                        root.save_prefs(json!({ "send_key": next }), cx);
                    },
                ))
                .child(setting_row(
                    "setting-sidebar",
                    "Sidebar",
                    if sidebar { "Shown" } else { "Hidden" },
                    theme,
                    cx,
                    move |root, _, cx| root.save_prefs(json!({ "sidebar_visible": !sidebar }), cx),
                ))
                .child(setting_row(
                    "setting-predictions",
                    "Composer predictions",
                    if predictions {
                        "On, as the terminal's ghost text"
                    } else {
                        "Off"
                    },
                    theme,
                    cx,
                    move |root, _, cx| root.save_prefs(json!({ "predictions": !predictions }), cx),
                ))
                .child(setting_row(
                    "setting-notifications",
                    "Notifications",
                    &format!("{notifications} — approvals and questions are announced"),
                    theme,
                    cx,
                    move |root, _, cx| {
                        let next = match notifications.as_str() {
                            "background" => "always",
                            "always" => "off",
                            _ => "background",
                        };
                        root.save_prefs(json!({ "notifications": next }), cx)
                    },
                ))
                .child(setting_row(
                    "setting-scale",
                    "Text size",
                    &format!("{:.0}%", scale * 100.0),
                    theme,
                    cx,
                    move |root, _, cx| {
                        let next = match scale {
                            value if value < 0.9 => 1.0,
                            value if value < 1.1 => 1.2,
                            value if value < 1.3 => 1.4,
                            _ => 0.8,
                        };
                        root.save_prefs(json!({ "font_scale": next }), cx)
                    },
                )),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .flex_wrap()
                .gap_2()
                .child(
                    Button::new("Models").render("settings-models", theme, cx, |root, _, cx| {
                        root.open_overlay(Overlay::Models, cx)
                    }),
                )
                .child(
                    Button::new("Themes").render("settings-themes", theme, cx, |root, _, cx| {
                        root.open_overlay(Overlay::Themes, cx)
                    }),
                )
                .child(Button::new("Language").render(
                    "settings-language",
                    theme,
                    cx,
                    |root, _, cx| root.open_overlay(Overlay::Language, cx),
                ))
                .child(Button::new("Shortcuts").render(
                    "settings-shortcuts",
                    theme,
                    cx,
                    |root, _, cx| root.open_overlay(Overlay::Shortcuts, cx),
                )),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .border_t_1()
                .border_color(theme.border())
                .pt_2()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(11.5))
                        .text_color(theme.faint())
                        .child(format!("Oxide Beta {version} — GPUI front-end")),
                )
                .child(
                    div()
                        .id("settings-close")
                        .text_size(px(11.5))
                        .text_color(theme.dim())
                        .cursor_pointer()
                        .child("Close")
                        .on_click(cx.listener(|root, _, _, cx| {
                            root.overlay = None;
                            cx.notify();
                        })),
                ),
        );

    panel = panel.child(close_button(theme, cx));
    panel
}

// ---------- models ----------

fn models(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let data = root.models.clone();
    let mut panel = dialog(theme, "Models", 560.0);
    let mut body = div()
        .id("models-body")
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(460.0))
        .overflow_y_scroll();

    if let Some(error) = data["error"].as_str() {
        body = body.child(
            div()
                .text_size(px(12.5))
                .text_color(theme.error())
                .child(error.to_string()),
        );
        body = body.child(
            div()
                .flex()
                .flex_col()
                .gap_2()
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.dim())
                        .child(
                            "Connect a provider with `oxide connect` in the terminal, then reopen this list.",
                        ),
                )
                .child(Button::new("How connecting works").render(
                    "models-connect",
                    theme,
                    cx,
                    |root, _, cx| {
                        root.open_overlay(
                            Overlay::Connect {
                                provider: "a provider".to_string(),
                            },
                            cx,
                        )
                    },
                )),
        );
    }

    let providers = data["providers"].as_array().cloned().unwrap_or_default();
    for provider in providers {
        let name = provider["provider"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let current = provider["current"].as_str().unwrap_or_default().to_string();
        let active = provider["active"].as_bool().unwrap_or(false);
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .pt_2()
                .child(
                    div()
                        .text_size(px(12.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if active { theme.accent() } else { theme.text() })
                        .child(name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.0))
                        .text_color(theme.faint())
                        .child(current.clone()),
                ),
        );
        if let Some(error) = provider["error"].as_str() {
            body = body.child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.error())
                    .child(error.to_string()),
            );
        }
        for model in provider["models"].as_array().cloned().unwrap_or_default() {
            let (label, detail) = match &model {
                Value::String(name) => (name.clone(), None),
                other => (
                    other["id"]
                        .as_str()
                        .or_else(|| other["name"].as_str())
                        .unwrap_or_default()
                        .to_string(),
                    other["name"].as_str().map(str::to_string),
                ),
            };
            let selected = label == current && active;
            let provider_name = name.clone();
            let model_name = label.clone();
            body = body.child(selectable(
                SharedString::from(format!("model-{label}")),
                label,
                detail,
                selected,
                theme,
                cx,
                move |root, _, cx| root.set_model(provider_name.clone(), model_name.clone(), cx),
            ));
        }
    }

    panel = panel.child(body);
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .justify_between()
            .items_center()
            .gap_2()
            .child(
                Button::new("Refresh").render("models-refresh", theme, cx, |root, _, cx| {
                    let project = root.current_project().unwrap_or_default();
                    root.call(
                        "list_models",
                        json!({ "project": project, "refresh": true }),
                        cx,
                        |root, result, _| {
                            if let Ok(value) = result {
                                root.models = value;
                            }
                        },
                    )
                }),
            )
            .child(close_button(theme, cx)),
    );
    panel
}

// ---------- themes ----------

fn themes(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let names = root.theme_names.clone();
    let current = theme.name.clone();
    let mut panel = dialog(theme, "Themes", 460.0);
    let mut body = div()
        .id("themes-body")
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(440.0))
        .overflow_y_scroll();
    if names.is_empty() {
        body = body.child(
            div()
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child("No themes were found beside the configuration."),
        );
    }
    for name in names {
        let selected = name == current;
        let choice = name.clone();
        body = body.child(selectable(
            SharedString::from(format!("theme-{name}")),
            name,
            None,
            selected,
            theme,
            cx,
            move |root, _, cx| root.set_theme(choice.clone(), cx),
        ));
    }
    panel = panel.child(body);
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .justify_end()
            .child(close_button(theme, cx)),
    );
    panel
}

// ---------- language ----------

fn language(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let languages = root.languages.clone();
    let mut panel = dialog(theme, "Language", 420.0);
    let mut body = div()
        .flex()
        .flex_col()
        .gap_1()
        .id("language-body")
        .max_h(px(400.0))
        .overflow_y_scroll();
    for language in languages {
        let tag = language["tag"].as_str().unwrap_or_default().to_string();
        let label = language["nativeLabel"]
            .as_str()
            .or_else(|| language["label"].as_str())
            .unwrap_or_default()
            .to_string();
        let selected = tag == root.strings_language();
        let choice = tag.clone();
        body = body.child(selectable(
            SharedString::from(format!("language-{tag}")),
            label,
            Some(tag),
            selected,
            theme,
            cx,
            move |root, _, cx| {
                let project = root.current_project().unwrap_or_default();
                root.call(
                    "set_language",
                    json!({ "project": project, "tag": choice.clone() }),
                    cx,
                    |root, result, cx| match result {
                        Ok(value) => {
                            root.strings = crate::app::Strings::load(&value);
                            root.languages =
                                value["languages"].as_array().cloned().unwrap_or_default();
                            root.overlay = None;
                            cx.notify();
                        }
                        Err(error) => root.toast(error, true, cx),
                    },
                );
            },
        ));
    }
    panel = panel.child(body);
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .justify_end()
            .child(close_button(theme, cx)),
    );
    panel
}

// ---------- shortcuts ----------

fn shortcuts(theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let rows = [
        ("New chat", "⌘N"),
        ("Close chat", "⌘W"),
        ("Show or hide the sidebar", "⌘B"),
        ("Settings", "⌘,"),
        ("Changes pane", "⌘⏎"),
        ("Model picker", "⌘⇧M"),
        ("Back / forward", "⌘[ / ⌘]"),
        ("Send", "Enter"),
        ("New line", "⇧⏎"),
        ("Close a dialog", "Esc"),
    ];
    let mut panel = dialog(theme, "Keyboard shortcuts", 420.0);
    let mut body = div().flex().flex_col().gap_1();
    for (label, keys) in rows {
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .py_1()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.5))
                        .text_color(theme.text())
                        .child(label),
                )
                .child(
                    div()
                        .text_size(px(11.5))
                        .text_color(theme.dim())
                        .child(keys),
                ),
        );
    }
    panel = panel.child(body);
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .justify_end()
            .child(close_button(theme, cx)),
    );
    panel
}

// ---------- trust ----------

fn trust(root: &mut Root, theme: &Theme, project: String, cx: &mut Context<Root>) -> gpui::Div {
    let info = root.info.clone();
    let trusted = info["trust"].as_bool().unwrap_or(false);
    let mut panel = dialog(theme, "Project access", 480.0).child(
        div()
            .text_size(px(12.5))
            .line_height(px(19.0))
            .text_color(theme.dim())
            .child(format!(
                "{project}\n\nA trusted project loads its own skills, plugins, MCP servers and hooks. Its tools can already read any path this process can, so trust is about the project's resources, not about file access — the same gate the terminal and the Electron window use."
            )),
    );
    if let Some(agents) = info["agents"].as_array() {
        if !agents.is_empty() {
            let mut rows = div().flex().flex_col().gap_1();
            for agent in agents {
                rows = rows.child(div().text_size(px(11.5)).text_color(theme.faint()).child(
                    format!(
                        "{} — {}",
                        agent["name"].as_str().unwrap_or_default(),
                        agent["description"].as_str().unwrap_or_default()
                    ),
                ));
            }
            panel = panel.child(rows);
        }
    }
    let path = project.clone();
    panel = panel
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(
                    Button::new(if trusted {
                        "Stop trusting"
                    } else {
                        "Trust project"
                    })
                    .primary()
                    .render("trust-toggle", theme, cx, move |root, _, cx| {
                        root.set_trust(path.clone(), !trusted, cx)
                    }),
                )
                .child(
                    Button::new("Cancel").render("trust-cancel", theme, cx, |root, _, cx| {
                        root.overlay = None;
                        cx.notify();
                    }),
                ),
        )
        .child(close_button(theme, cx));
    panel
}

// ---------- approvals ----------

fn approval(
    root: &mut Root,
    theme: &Theme,
    chat: usize,
    id: u64,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let card = root.chats.get(chat).and_then(|chat| {
        chat.entries.iter().find_map(|entry| match entry {
            crate::model::Entry::Approval(card) if card.id == id => Some(card.clone()),
            _ => None,
        })
    });
    let (tool, detail) = card
        .map(|card| (card.tool, card.detail))
        .unwrap_or_else(|| ("tool".to_string(), String::new()));

    let mut panel = dialog(theme, "Allow this tool?", 540.0)
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded(px(8.0))
                .bg(theme.panel_2())
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.tool())
                        .child(tool),
                )
                .child(
                    div()
                        .text_size(px(12.0))
                        .line_height(px(18.0))
                        .text_color(theme.dim())
                        .child(detail),
                ),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .gap_2()
                .child(Button::new("Allow once").primary().render(
                    ("approval-once", id),
                    theme,
                    cx,
                    move |root, _, cx| root.resolve_approval(id, "once", cx),
                ))
                .child(Button::new("Always allow").render(
                    ("approval-always", id),
                    theme,
                    cx,
                    move |root, _, cx| root.resolve_approval(id, "always", cx),
                ))
                .child(Button::new("Deny").danger().render(
                    ("approval-deny", id),
                    theme,
                    cx,
                    move |root, _, cx| root.resolve_approval(id, "deny", cx),
                )),
        );
    let _ = &mut panel;
    panel
}

// ---------- questions ----------

fn question(
    root: &mut Root,
    theme: &Theme,
    chat: usize,
    id: u64,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let card = root.chats.get(chat).and_then(|chat| {
        chat.entries.iter().find_map(|entry| match entry {
            crate::model::Entry::Question(card) if card.id == id => Some(card.clone()),
            _ => None,
        })
    });
    let questions = card
        .as_ref()
        .map(|card| card.questions.clone())
        .unwrap_or(Value::Null);
    let list = questions.as_array().cloned().unwrap_or_default();
    let header = list
        .first()
        .and_then(|question| question["header"].as_str())
        .unwrap_or("A question")
        .to_string();

    let mut panel = dialog(theme, &header, 560.0);
    let mut body = div()
        .id("question-body")
        .flex()
        .flex_col()
        .gap_3()
        .max_h(px(440.0))
        .overflow_y_scroll();
    let mut answers: Vec<(String, Vec<String>)> = Vec::new();

    for question in list {
        let text = question["question"]
            .as_str()
            .unwrap_or_default()
            .to_string();
        let options = question["options"].as_array().cloned().unwrap_or_default();
        let mut block = div()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded(px(9.0))
            .bg(theme.panel_2())
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme.text())
                    .child(text.clone()),
            );
        let mut labels = Vec::new();
        for option in options {
            let label = option["label"]
                .as_str()
                .or_else(|| option["value"].as_str())
                .unwrap_or_default()
                .to_string();
            let description = option["description"].as_str().map(str::to_string);
            labels.push(label.clone());
            let question_text = text.clone();
            let value = label.clone();
            block = block.child(selectable(
                SharedString::from(format!("answer-{label}")),
                label,
                description,
                false,
                theme,
                cx,
                move |root, _, cx| {
                    root.resolve_question(
                        id,
                        json!([{ "question": question_text, "values": [value] }]),
                        cx,
                    )
                },
            ));
        }
        if labels.is_empty() {
            block = block.child(
                div()
                    .text_size(px(11.5))
                    .text_color(theme.faint())
                    .child("Type an answer in the composer after dismissing this dialog."),
            );
        }
        answers.push((text, labels));
        body = body.child(block);
    }

    panel = panel.child(body);
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .justify_between()
            .items_center()
            .child(Button::new("Dismiss").render(
                "question-dismiss",
                theme,
                cx,
                move |root, _, cx| root.resolve_question(id, json!([]), cx),
            ))
            .child(close_button(theme, cx)),
    );
    panel
}

// ---------- session and project actions ----------

fn session_actions(
    theme: &Theme,
    id: String,
    title: String,
    pinned: bool,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let name = if title.trim().is_empty() {
        id.clone()
    } else {
        title.clone()
    };
    let mut panel = dialog(theme, &name, 460.0).child(
        div()
            .text_size(px(11.5))
            .text_color(theme.faint())
            .child("What to do with this chat"),
    );
    let pin_id = id.clone();
    let archive_id = id.clone();
    let archive_title = name.clone();
    panel = panel
        .child(
            Button::new(if pinned {
                "Unpin from the sidebar"
            } else {
                "Pin to the sidebar"
            })
            .render(
                SharedString::from(format!("session-pin-{id}")),
                theme,
                cx,
                move |root, _, cx| root.set_session_pinned(pin_id.clone(), !pinned, cx),
            ),
        )
        .child(Button::new("Archive").render(
            SharedString::from(format!("session-archive-{id}")),
            theme,
            cx,
            move |root, _, cx| {
                root.overlay = Some(Overlay::Confirm {
                    title: "Archive this chat?".to_string(),
                    body: format!(
                        "Archive {archive_title}? It leaves the sidebar, and Archived puts it back."
                    ),
                    action: ConfirmAction::ArchiveSession(archive_id.clone()),
                });
                cx.notify();
            },
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(close_button(theme, cx)),
        );
    panel
}

fn project_actions(
    theme: &Theme,
    id: String,
    path: String,
    name: String,
    pinned: bool,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let mut panel = dialog(theme, &name, 520.0).child(
        div()
            .text_size(px(11.5))
            .text_color(theme.faint())
            .child(path.clone()),
    );
    let pin_id = id.clone();
    let remove_id = id.clone();
    let remove_path = path.clone();
    panel = panel
        .child(
            Button::new(if pinned { "Unpin project" } else { "Pin project" }).render(
                SharedString::from(format!("project-pin-{id}")),
                theme,
                cx,
                move |root, _, cx| root.set_project_pinned(pin_id.clone(), !pinned, cx),
            ),
        )
        .child(Button::new("Remove from the sidebar").danger().render(
            SharedString::from(format!("project-remove-{id}")),
            theme,
            cx,
            move |root, _, cx| {
                root.overlay = Some(Overlay::Confirm {
                    title: "Remove project".to_string(),
                    body: format!(
                        "Remove {remove_path} from the sidebar? The folder and its chats stay on disk."
                    ),
                    action: ConfirmAction::RemoveProject(remove_id.clone()),
                });
                cx.notify();
            },
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(close_button(theme, cx)),
        );
    panel
}

fn archived(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let archived = root.archived.clone();
    let mut panel = dialog(theme, "Archived chats", 520.0);
    let mut body = div()
        .id("archived-body")
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(420.0))
        .overflow_y_scroll();
    if archived.is_empty() {
        body = body.child(
            div()
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child("No archived chats"),
        );
    }
    for session in archived {
        let id = session.id.clone();
        let title = session.title.clone();
        body = body.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .py_1()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_size(px(12.5))
                        .child(title),
                )
                .child(Button::new("Restore").render(
                    SharedString::from(format!("restore-{id}")),
                    theme,
                    cx,
                    move |root, _, cx| root.unarchive_session(id.clone(), cx),
                )),
        );
    }
    panel = panel.child(body);
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .justify_end()
            .child(close_button(theme, cx)),
    );
    panel
}

// ---------- confirmations ----------

fn confirm(
    theme: &Theme,
    title: String,
    body: String,
    action: ConfirmAction,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let mut panel = dialog(theme, &title, 460.0).child(
        div()
            .text_size(px(12.5))
            .line_height(px(19.0))
            .text_color(theme.dim())
            .child(body),
    );
    let action_for_ok = action.clone();
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .justify_end()
            .gap_2()
            .child(
                Button::new("Cancel").render("confirm-cancel", theme, cx, |root, _, cx| {
                    root.overlay = None;
                    cx.notify();
                }),
            )
            .child(Button::new("Confirm").primary().render(
                "confirm-ok",
                theme,
                cx,
                move |root, _, cx| match action_for_ok.clone() {
                    ConfirmAction::RemoveProject(id) => root.remove_project(id, cx),
                    ConfirmAction::ArchiveSession(id) => root.archive_session(id, cx),
                    ConfirmAction::CloseChat(index) => {
                        root.overlay = None;
                        root.close_chat(index, cx);
                    }
                },
            )),
    );
    panel
}

// ---------- connect ----------

fn connect(theme: &Theme, provider: String, cx: &mut Context<Root>) -> gpui::Div {
    dialog(theme, "Connect a provider", 500.0)
        .child(
            div()
                .text_size(px(12.5))
                .line_height(px(19.0))
                .text_color(theme.dim())
                .child(format!(
                    "{provider} has no stored credential. Run `oxide connect {provider}` in a terminal — the desktop app and the CLI share `auth.json`, so the key appears here on the next refresh."
                )),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .child(close_button(theme, cx)),
        )
}

// ---------- new project ----------

fn new_project(theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    dialog(theme, "Add a project", 460.0)
        .child(
            div()
                .text_size(px(12.5))
                .line_height(px(19.0))
                .text_color(theme.dim())
                .child("A project is a folder on disk. Its chats, approvals and configuration are the ones the terminal and the Electron window see."),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(Button::new("Choose folder").primary().render(
                    "new-project-pick",
                    theme,
                    cx,
                    |root, _, cx| {
                        root.overlay = None;
                        root.add_project(cx);
                    },
                ))
                .child(Button::new("Cancel").render("new-project-cancel", theme, cx, |root, _, cx| {
                    root.overlay = None;
                    cx.notify();
                })),
        )
}
