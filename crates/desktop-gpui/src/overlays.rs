//! The window's dialogs: settings, models, themes, language, shortcuts, trust,
//! approvals, questions and confirmations.
//!
//! They are the same surfaces the Electron window opens — the same commands,
//! the same stored preferences, the same approval broker — drawn as GPUI panels
//! over the conversation.

use crate::app::{Button, ConfirmAction, Overlay, Root, SettingsTab};
use crate::icons::{self, Icon};
use crate::theme::Theme;
use gpui::prelude::*;
use gpui::{div, hsla, px, Context, Div, FontWeight, SharedString, Stateful, Window};
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
        Overlay::Approvals => approvals(root, theme, cx),
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

/// The way out of a dialog, in words as well as in the glyph: a bare `\u{00d7}`
/// asks a reader to know what it does, and the same control answers one keystroke
/// away as well.
fn close_button(theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    div()
        .id("overlay-close")
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(7.0))
        .cursor_pointer()
        .border_1()
        .border_color(theme.border())
        .bg(theme.panel_2())
        .text_size(px(12.0))
        .text_color(theme.text())
        .hover(move |style| style.bg(theme.panel_3()))
        .tooltip(crate::views::tip("Close \u{00b7} Esc", theme))
        .child(icons::icon(Icon::Close, 14.0, theme.dim()))
        .child("Close")
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

// ---------- settings ----------

/// The settings sheet: the page's own two columns — the sections down the left,
/// the active section's fields in a card on the right. Everything it writes goes
/// through `save_prefs`, so the terminal and the stable window see the same
/// values.
fn settings(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let version = env!("CARGO_PKG_VERSION");
    let tab = root.settings_tab;

    let mut navigation = div()
        .flex()
        .flex_col()
        .flex_none()
        .w(px(232.0))
        .h_full()
        .px(px(16.0))
        .pt(px(24.0))
        .pb(px(16.0))
        .gap(px(3.0))
        .bg(theme.sidebar())
        .border_r_1()
        .border_color(theme.border())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px(px(6.0))
                .pb(px(18.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(19.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text())
                        .child("Settings"),
                )
                .child(close_button(theme, cx)),
        );

    navigation = navigation.child(nav_group(
        "Personal",
        theme,
        vec![
            nav_tab(
                "settings-tab-general",
                Icon::Settings,
                "General",
                tab == SettingsTab::General,
                theme,
                cx,
                |root, _, cx| {
                    root.settings_tab = SettingsTab::General;
                    cx.notify();
                },
            ),
            nav_tab(
                "settings-tab-appearance",
                Icon::Image,
                "Appearance",
                tab == SettingsTab::Appearance,
                theme,
                cx,
                |root, _, cx| {
                    root.settings_tab = SettingsTab::Appearance;
                    cx.notify();
                },
            ),
            nav_tab(
                "settings-tab-notifications",
                Icon::Bell,
                "Notifications",
                tab == SettingsTab::Notifications,
                theme,
                cx,
                |root, _, cx| {
                    root.settings_tab = SettingsTab::Notifications;
                    cx.notify();
                },
            ),
        ],
    ));

    navigation = navigation.child(nav_group(
        "Integrations",
        theme,
        vec![
            nav_tab(
                "settings-tab-models",
                Icon::Globe,
                "Models",
                false,
                theme,
                cx,
                |root, _, cx| root.open_overlay(Overlay::Models, cx),
            ),
            nav_tab(
                "settings-tab-shortcuts",
                Icon::Keyboard,
                "Shortcuts",
                false,
                theme,
                cx,
                |root, _, cx| root.open_overlay(Overlay::Shortcuts, cx),
            ),
        ],
    ));

    navigation = navigation.child(div().flex_1()).child(
        div()
            .px(px(10.0))
            .text_size(px(11.0))
            .text_color(theme.faint())
            .child(format!("Oxide Beta {version}")),
    );

    let (title, hint) = match tab {
        SettingsTab::General => (
            "General",
            "How the composer sends, and how a turn in flight is followed up.",
        ),
        SettingsTab::Appearance => (
            "Appearance",
            "The theme, the language and the size this window paints with.",
        ),
        SettingsTab::Notifications => (
            "Notifications",
            "What the window announces when a turn finishes or waits on you.",
        ),
    };

    let rows = match tab {
        SettingsTab::General => general_fields(root, theme, cx),
        SettingsTab::Appearance => appearance_fields(root, theme, cx),
        SettingsTab::Notifications => notification_fields(root, theme, cx),
    };

    let content = div()
        .id("settings-content")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .h_full()
        .overflow_y_scroll()
        .px(px(40.0))
        .pt(px(36.0))
        .pb(px(28.0))
        .gap(px(24.0))
        .bg(theme.background())
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.0))
                .child(
                    div()
                        .text_size(px(22.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text())
                        .child(title),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(20.0))
                        .text_color(theme.dim())
                        .child(hint),
                ),
        )
        .child(fields(theme, rows));

    div()
        .flex()
        .flex_row()
        .w(px(880.0))
        .h(px(600.0))
        .max_w_full()
        .max_h_full()
        .bg(theme.background())
        .border_1()
        .border_color(theme.border())
        .rounded(px(16.0))
        .overflow_hidden()
        .child(navigation)
        .child(content)
}

fn general_fields(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> Vec<gpui::Div> {
    let prefs = root.prefs.clone();
    let send_key = prefs["send_key"].as_str().unwrap_or("enter").to_string();
    let follow_up = prefs["follow_up"].as_str().unwrap_or("queue").to_string();
    let predictions = prefs["predictions"].as_bool().unwrap_or(false);
    let awake = prefs["keep_awake"].as_bool().unwrap_or(false);

    let mut send = segments(theme);
    let current = send_key.clone();
    send = send.child(segment(
        "settings-send-enter",
        "Enter",
        current == "enter",
        theme,
        cx,
        |root, _, cx| root.save_prefs(json!({ "send_key": "enter" }), cx),
    ));
    send = send.child(segment(
        "settings-send-cmd-enter",
        "\u{2318}Enter",
        send_key != "enter",
        theme,
        cx,
        |root, _, cx| root.save_prefs(json!({ "send_key": "cmd+enter" }), cx),
    ));

    let mut follow = segments(theme);
    follow = follow.child(segment(
        "settings-follow-queue",
        "Queue",
        follow_up == "queue",
        theme,
        cx,
        |root, _, cx| root.save_prefs(json!({ "follow_up": "queue" }), cx),
    ));
    follow = follow.child(segment(
        "settings-follow-steer",
        "Steer",
        follow_up == "steer",
        theme,
        cx,
        |root, _, cx| root.save_prefs(json!({ "follow_up": "steer" }), cx),
    ));

    vec![
        field(
            "Send with",
            "Enter sends and \u{2318}Enter starts a new line, or the other way round.",
            theme,
            send,
        ),
        field(
            "Follow-ups",
            "Send a note while a turn is running: it joins the queue, or steers the turn in flight.",
            theme,
            follow,
        ),
        field(
            "Composer predictions",
            "Offer the next words as ghost text after the caret.",
            theme,
            switch(
                "settings-predictions",
                predictions,
                theme,
                cx,
                move |root, _, cx| root.save_prefs(json!({ "predictions": !predictions }), cx),
            ),
        ),
        field(
            "Keep awake",
            "Hold the display awake while a turn runs.",
            theme,
            switch(
                "settings-keep-awake",
                awake,
                theme,
                cx,
                move |root, _, cx| root.save_prefs(json!({ "keep_awake": !awake }), cx),
            ),
        ),
    ]
}

fn appearance_fields(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> Vec<gpui::Div> {
    let prefs = root.prefs.clone();
    let theme_name = root.current_theme.clone();
    let language = root.language.clone();
    let scale = prefs["font_scale"].as_f64().unwrap_or(1.0);
    let sidebar = prefs["sidebar_visible"].as_bool().unwrap_or(true);
    let language_label = root
        .languages
        .iter()
        .find(|row| row["tag"].as_str() == Some(language.as_str()))
        .and_then(|row| row["label"].as_str())
        .unwrap_or("English")
        .to_string();

    let mut sizes = segments(theme);
    for (id, label, value) in [
        ("settings-scale-80", "80%", 0.8_f64),
        ("settings-scale-100", "100%", 1.0),
        ("settings-scale-120", "120%", 1.2),
        ("settings-scale-140", "140%", 1.4),
    ] {
        sizes = sizes.child(segment(
            id,
            label,
            (scale - value).abs() < 0.01,
            theme,
            cx,
            move |root, _, cx| root.save_prefs(json!({ "font_scale": value }), cx),
        ));
    }

    vec![
        field(
            "Theme",
            "The palette the window, the terminal and the stable app all read.",
            theme,
            link_button(
                "settings-open-themes",
                &theme_name,
                theme,
                cx,
                |root, _, cx| root.open_overlay(Overlay::Themes, cx),
            ),
        ),
        field(
            "Language",
            "The strings the window draws, from the shared catalog.",
            theme,
            link_button(
                "settings-open-language",
                &language_label,
                theme,
                cx,
                |root, _, cx| root.open_overlay(Overlay::Language, cx),
            ),
        ),
        field(
            "Text size",
            "The scale every label and line in the window uses.",
            theme,
            sizes,
        ),
        field(
            "Sidebar",
            "Show the project tree beside the conversation.",
            theme,
            switch(
                "settings-sidebar",
                sidebar,
                theme,
                cx,
                move |root, _, cx| root.save_prefs(json!({ "sidebar_visible": !sidebar }), cx),
            ),
        ),
    ]
}

fn notification_fields(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> Vec<gpui::Div> {
    let prefs = root.prefs.clone();
    let mode = prefs["notifications"]
        .as_str()
        .unwrap_or("background")
        .to_string();
    let approvals = prefs["notify_approvals"].as_bool().unwrap_or(true);
    let questions = prefs["notify_questions"].as_bool().unwrap_or(true);

    let mut modes = segments(theme);
    for (id, label, value) in [
        ("settings-notify-off", "Off", "off"),
        ("settings-notify-background", "Background", "background"),
        ("settings-notify-always", "Always", "always"),
    ] {
        modes = modes.child(segment(
            id,
            label,
            mode == value,
            theme,
            cx,
            move |root, _, cx| root.save_prefs(json!({ "notifications": value }), cx),
        ));
    }

    vec![
        field(
            "Notifications",
            "When a turn finishes: only while the window is in the background, always, or never.",
            theme,
            modes,
        ),
        field(
            "Approval requests",
            "Announce a tool that is waiting for an approval.",
            theme,
            switch(
                "settings-notify-approvals",
                approvals,
                theme,
                cx,
                move |root, _, cx| root.save_prefs(json!({ "notify_approvals": !approvals }), cx),
            ),
        ),
        field(
            "Question requests",
            "Announce a question the turn asked and is waiting on.",
            theme,
            switch(
                "settings-notify-questions",
                questions,
                theme,
                cx,
                move |root, _, cx| root.save_prefs(json!({ "notify_questions": !questions }), cx),
            ),
        ),
    ]
}

/// The card a section's rows sit in, with a hairline between them.
fn fields(theme: &Theme, rows: Vec<gpui::Div>) -> gpui::Div {
    let mut card = div()
        .flex()
        .flex_col()
        .w_full()
        .border_1()
        .border_color(theme.border())
        .rounded(px(14.0))
        .overflow_hidden()
        .bg(theme.panel());
    for (index, row) in rows.into_iter().enumerate() {
        if index > 0 {
            card = card.child(div().h(px(1.0)).w_full().bg(theme.border()));
        }
        card = card.child(row);
    }
    card
}

fn field(label: &str, hint: &str, theme: &Theme, control: impl IntoElement) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(24.0))
        .px(px(24.0))
        .py(px(20.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .text_size(px(14.0))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text())
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .text_size(px(12.5))
                        .line_height(px(20.0))
                        .text_color(theme.dim())
                        .child(hint.to_string()),
                ),
        )
        .child(div().flex_none().child(control))
}

fn nav_group(title: &str, theme: &Theme, rows: Vec<Stateful<Div>>) -> gpui::Div {
    let mut group = div().flex().flex_col().gap(px(3.0)).mb(px(20.0)).child(
        div()
            .px(px(10.0))
            .pb(px(5.0))
            .text_size(px(12.0))
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme.dim())
            .child(title.to_string()),
    );
    for row in rows {
        group = group.child(row);
    }
    group
}

fn nav_tab(
    id: impl Into<gpui::ElementId>,
    mark: Icon,
    label: &str,
    active: bool,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> Stateful<Div> {
    let color = if active { theme.text() } else { theme.dim() };
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(10.0))
        .h(px(40.0))
        .px(px(12.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .bg(if active {
            theme.panel_3()
        } else {
            theme.sidebar()
        })
        .hover(move |style| style.bg(theme.panel_2()))
        .child(icons::icon(mark, 18.0, color))
        .child(
            div()
                .text_size(px(13.0))
                .text_color(color)
                .child(label.to_string()),
        )
        .on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

/// A row of choices: the selected one wears the panel's own surface.
fn segments(theme: &Theme) -> gpui::Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(3.0))
        .p(px(3.0))
        .border_1()
        .border_color(theme.border())
        .rounded(px(9.0))
        .bg(theme.background())
}

fn segment(
    id: impl Into<gpui::ElementId>,
    label: &str,
    active: bool,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .text_size(px(12.0))
        .text_color(if active { theme.text() } else { theme.dim() })
        .bg(if active {
            theme.panel_3()
        } else {
            theme.background()
        })
        .hover(move |style| style.bg(theme.panel_2()))
        .child(label.to_string())
        .on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

fn switch(
    id: impl Into<gpui::ElementId>,
    on: bool,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> Stateful<Div> {
    // The track is 38x22, but the control a pointer has to hit is the padding
    // around it: a switch is a small thing to ask anyone to land on.
    let mut track = div()
        .flex()
        .flex_row()
        .items_center()
        .flex_none()
        .w(px(38.0))
        .h(px(22.0))
        .p(px(4.0))
        .rounded(px(11.0))
        .bg(if on { theme.accent() } else { theme.panel_3() })
        .border_1()
        .border_color(if on { theme.accent() } else { theme.faint() });
    track = if on {
        track.justify_end()
    } else {
        track.justify_start()
    };
    track = track.child(
        div()
            .flex_none()
            .w(px(12.0))
            .h(px(12.0))
            .rounded(px(6.0))
            .bg(if on { theme.background() } else { theme.dim() }),
    );
    div()
        .id(id)
        .flex()
        .items_center()
        .flex_none()
        .p(px(6.0))
        .rounded(px(8.0))
        .cursor_pointer()
        .hover(move |style| style.bg(theme.panel_2()))
        .child(track)
        .on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

fn link_button(
    id: impl Into<gpui::ElementId>,
    label: &str,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .flex_row()
        .items_center()
        .gap(px(6.0))
        .h(px(30.0))
        .px(px(10.0))
        .rounded(px(7.0))
        .cursor_pointer()
        .border_1()
        .border_color(theme.border())
        .bg(theme.panel_2())
        .text_size(px(12.0))
        .text_color(theme.text())
        .hover(move |style| style.bg(theme.panel_3()))
        .child(label.to_string())
        .child(icons::icon(Icon::ChevronRight, 12.0, theme.dim()))
        .on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
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

// ---------- approvals ----------

/// The approvals the composer's lock names: the tools this project runs without
/// asking first. It is the same store the terminal and the Electron window
/// read, and clearing it puts every call back behind a prompt.
fn approvals(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let tools = root.approvals.clone();
    let mut panel = dialog(theme, "Tool approvals", 470.0).child(
        div()
            .text_size(px(12.5))
            .line_height(px(19.0))
            .text_color(theme.dim())
            .child(
                "Tools this project runs without asking first. The terminal and the Electron window read the same list.",
            ),
    );

    if tools.is_empty() {
        panel = panel.child(
            div()
                .p_3()
                .rounded(px(10.0))
                .bg(theme.panel_2())
                .text_size(px(12.5))
                .text_color(theme.faint())
                .child("Nothing is pre-approved: every call asks before it runs."),
        );
    } else {
        let mut list = div()
            .id("approvals-list")
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(320.0))
            .overflow_y_scroll();
        for tool in tools {
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(30.0))
                    .px_2()
                    .rounded(px(8.0))
                    .bg(theme.panel_2())
                    .text_size(px(12.5))
                    .text_color(theme.text())
                    .child(icons::icon(Icon::Check, 13.0, theme.success()))
                    .child(tool),
            );
        }
        panel = panel
            .child(list)
            .child(Button::new("Clear approvals").danger().render(
                "approvals-clear",
                theme,
                cx,
                |root, _, cx| root.clear_approvals(cx),
            ));
    }
    panel.child(
        div()
            .flex()
            .flex_row()
            .justify_end()
            .child(close_button(theme, cx)),
    )
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
