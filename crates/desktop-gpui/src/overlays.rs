//! The window's dialogs: settings, models, themes, language, shortcuts, trust,
//! approvals, questions and confirmations.
//!
//! They are the same surfaces the Electron window opens — the same commands,
//! the same stored preferences, the same approval broker — drawn as GPUI panels
//! over the conversation.

use crate::app::{rule, Button, ConfirmAction, Overlay, Root, SettingsTab};
use crate::icons::{self, Icon};
use crate::theme::Theme;
use crate::views::{icon_button, pill, SETTINGS_NAVIGATION_WIDTH};
use gpui::prelude::*;
use gpui::{div, hsla, px, Context, Div, Focusable, FontWeight, SharedString, Stateful, Window};
use serde_json::{json, Value};

pub fn layer(
    root: &mut Root,
    theme: &Theme,
    _scale: f32,
    window: &mut Window,
    cx: &mut Context<Root>,
) -> impl IntoElement {
    let overlay = root.overlay.clone();
    let Some(overlay) = overlay else {
        return div().id("overlay-hidden");
    };
    let dialog_label = match &overlay {
        Overlay::Settings => "Settings".to_string(),
        Overlay::About => "About Oxide".to_string(),
        Overlay::ImagePreview { name, .. } => format!("Image {name}"),
        Overlay::Updates => "Software update".to_string(),
        Overlay::Help => "Slash commands".to_string(),
        Overlay::CommandMenu => "Command menu".to_string(),
        Overlay::Mcp => "MCP servers".to_string(),
        Overlay::Agents => "Agents".to_string(),
        Overlay::Shortcuts => "Keyboard shortcuts".to_string(),
        Overlay::Trust { .. } => "Project trust".to_string(),
        Overlay::Approval { .. } => "Tool approval".to_string(),
        Overlay::Question { .. } => "Question".to_string(),
        Overlay::Confirm { title, .. } => title.clone(),
        Overlay::Connect { provider } => format!("Connect {provider}"),
        Overlay::FileViewer => "File viewer".to_string(),
        Overlay::NewProject => "Create project".to_string(),
        Overlay::SessionActions { .. } => "Chat actions".to_string(),
        Overlay::ProjectActions { .. } => "Project actions".to_string(),
        Overlay::Approvals => "Tool approvals".to_string(),
        Overlay::ComputerUse => "Computer use".to_string(),
        Overlay::Worktrees => "Git worktrees".to_string(),
        Overlay::WorktreeBranch { .. } => "Create branch".to_string(),
        Overlay::Projects => "Projects".to_string(),
        Overlay::RenameSession { .. } => "Rename chat".to_string(),
    };

    let rail_popover = matches!(&overlay, Overlay::CommandMenu);
    let create_project = matches!(&overlay, Overlay::NewProject);
    let mut scrim = div()
        .id("overlay")
        .role(gpui::Role::Dialog)
        .aria_label(dialog_label)
        .absolute()
        .size_full()
        .flex();
    scrim = if rail_popover {
        scrim
            .items_start()
            .justify_start()
            .pt(px(58.0))
            .pl(px(56.0))
            .bg(gpui::transparent_black())
    } else {
        scrim.items_center().justify_center().bg(hsla(
            0.0,
            0.0,
            0.0,
            if create_project { 0.2 } else { 0.55 },
        ))
    };

    let panel = match overlay.clone() {
        Overlay::Settings => settings_page(root, theme, cx),
        Overlay::About => about_dialog(theme, cx),
        Overlay::ImagePreview { name, image } => image_preview(theme, name, image, cx),
        Overlay::Updates => update_dialog(root, theme, cx),
        Overlay::Help => command_help(root, theme, cx),
        Overlay::CommandMenu => command_menu(root, theme, cx),
        Overlay::Mcp => mcp_servers(root, theme, cx),
        Overlay::Agents => agents_picker(root, theme, cx),
        Overlay::Shortcuts => shortcuts(theme, cx),
        Overlay::Trust {
            project,
            onboarding,
        } => trust(root, theme, project, onboarding, cx),
        Overlay::Approval { chat, id } => approval(root, theme, chat, id, cx),
        Overlay::Question { chat, id } => question(root, theme, chat, id, cx),
        Overlay::Confirm {
            title,
            body,
            action,
        } => confirm(theme, title, body, action, cx),
        Overlay::Connect { provider } => connect(root, theme, provider, cx),
        Overlay::FileViewer => crate::workspace::file_viewer(root, theme, window, cx),
        Overlay::NewProject => new_project(root, theme, cx),
        Overlay::SessionActions {
            id,
            title,
            project,
            pinned,
        } => session_actions(theme, id, title, project, pinned, cx),
        Overlay::ProjectActions {
            id,
            path,
            name,
            pinned,
        } => project_actions(theme, id, path, name, pinned, cx),
        Overlay::Approvals => approvals(root, theme, cx),
        Overlay::ComputerUse => computer_use(root, theme, cx),
        Overlay::Worktrees => worktrees(root, theme, cx),
        Overlay::WorktreeBranch { project, name } => {
            worktree_branch(root, theme, project, name, cx)
        }
        Overlay::Projects => projects(root, theme, cx),
        Overlay::RenameSession { project, id, title } => {
            rename_session(root, theme, project, id, title, cx)
        }
    };

    let dismissible = !matches!(
        &overlay,
        Overlay::Settings
            | Overlay::Approval { .. }
            | Overlay::Question { .. }
            | Overlay::Trust { .. }
    ) && !(matches!(&overlay, Overlay::Updates) && root.updating_update);
    if dismissible {
        scrim = scrim.child(
            div()
                .id("overlay-dismiss-outside")
                .on_mouse_down_out(cx.listener(|root, _, _, cx| {
                    root.overlay = None;
                    cx.notify();
                }))
                .child(panel),
        );
    } else {
        scrim = scrim.child(panel);
    }
    let _ = window;
    scrim
}

fn dialog(theme: &Theme, title: &str, width: f32) -> gpui::Div {
    dialog_frame(theme, title, width, None)
}

fn dialog_with_close(theme: &Theme, title: &str, width: f32, cx: &mut Context<Root>) -> gpui::Div {
    dialog_frame(
        theme,
        title,
        width,
        Some(close_button(theme, cx).into_any_element()),
    )
}

fn dialog_frame(
    theme: &Theme,
    title: &str,
    width: f32,
    close: Option<gpui::AnyElement>,
) -> gpui::Div {
    div()
        .relative()
        .flex()
        .flex_col()
        .gap_3()
        .w(px(width))
        .max_h(px(680.0))
        .p(px(20.0))
        .bg(theme.panel())
        .border_1()
        .border_color(theme.border())
        .rounded(px(12.0))
        .shadow_xl()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(16.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text())
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis()
                        .child(title.to_string()),
                )
                .children(close),
        )
}

/// The way out of a dialog, in words as well as in the glyph: a bare `\u{00d7}`
/// asks a reader to know what it does, and the same control answers one keystroke
/// away as well.
fn close_button(theme: &Theme, cx: &mut Context<Root>) -> impl IntoElement {
    let group = SharedString::from("overlay-close-button");
    div()
        .id("overlay-close")
        .group(group.clone())
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .w(px(26.0))
        .h(px(26.0))
        .rounded(px(6.0))
        .cursor_pointer()
        .bg(gpui::transparent_black())
        .hover(move |style| style.bg(theme.panel_3()).text_color(theme.text()))
        .tooltip(crate::views::tip("Close \u{00b7} Esc", theme))
        .child(
            div()
                .opacity(0.72)
                .group_hover(group, |style| style.opacity(1.0))
                .child(icons::icon(Icon::Close, 14.0, theme.dim())),
        )
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
        .w_full()
        .min_w(px(0.0))
        .min_h(px(34.0))
        .px_2()
        .py(px(6.0))
        .rounded(px(7.0))
        .cursor_pointer()
        .bg(if selected {
            theme.panel_2()
        } else {
            gpui::transparent_black()
        })
        .hover(move |style| style.bg(theme.panel_2()));
    let mut copy = div()
        .flex_1()
        .min_w(px(0.0))
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(
            div()
                .text_size(px(12.5))
                .text_color(if selected {
                    theme.accent()
                } else {
                    theme.text()
                })
                .child(label),
        );
    if let Some(detail) = detail {
        copy = copy.child(
            div()
                .text_size(px(11.0))
                .line_height(px(16.0))
                .text_color(theme.faint())
                .whitespace_normal()
                .child(detail),
        );
    }
    row = row.child(copy);
    if selected {
        row = row.child(div().text_color(theme.accent()).child("✓"));
    }
    row.on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

// ---------- settings ----------

/// The settings sheet: the page's own two columns — the sections down the left,
/// the active section's fields in a card on the right. Everything it writes goes
/// through `save_prefs`, so the terminal and the Electron fallback see the same
/// values.
pub fn settings_page(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let tab = root.settings_tab;

    let mut navigation = div()
        .id("settings-navigation")
        .role(gpui::Role::Navigation)
        .aria_label("Settings sections")
        .flex()
        .flex_col()
        .flex_none()
        .w(px(SETTINGS_NAVIGATION_WIDTH))
        .h_full()
        .px(px(14.0))
        .pt(px(24.0))
        .pb(px(16.0))
        .gap(px(4.0))
        .relative()
        .bg(theme.sidebar())
        .border_r_1()
        .border_color(theme.border())
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .px(px(12.0))
                .pb(px(24.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(17.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text())
                        .child("Settings"),
                ),
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
                "settings-tab-plugins",
                Icon::Customize,
                "Plugins",
                false,
                theme,
                cx,
                |root, _, cx| {
                    root.rail = crate::app::Rail::Customize;
                    root.customize_tab = crate::app::CustomizeTab::Plugins;
                    root.overlay = None;
                    cx.notify();
                },
            ),
            nav_tab(
                "settings-tab-skills",
                Icon::Chat,
                "Skills",
                false,
                theme,
                cx,
                |root, _, cx| {
                    root.rail = crate::app::Rail::Customize;
                    root.customize_tab = crate::app::CustomizeTab::Skills;
                    root.overlay = None;
                    cx.notify();
                },
            ),
            nav_tab(
                "settings-tab-mcp",
                Icon::Globe,
                "MCP servers",
                false,
                theme,
                cx,
                |root, _, cx| {
                    root.rail = crate::app::Rail::Customize;
                    root.customize_tab = crate::app::CustomizeTab::McpServers;
                    root.overlay = None;
                    cx.notify();
                },
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
            .absolute()
            .left_0()
            .right_0()
            .bottom_2()
            .px(px(14.0))
            .child(
                div()
                    .id("settings-back")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(40.0))
                    .px(px(12.0))
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .text_size(px(13.0))
                    .text_color(theme.dim())
                    .hover(move |style| style.bg(theme.panel_2()).text_color(theme.text()))
                    .child(icons::icon(Icon::Back, 17.0, theme.faint()))
                    .child("Back to chat")
                    .on_click(cx.listener(|root, _, _, cx| root.navigate_back(cx))),
            ),
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

    let rows: Vec<Stateful<Div>> = match tab {
        SettingsTab::General => general_fields(root, theme, cx),
        SettingsTab::Appearance => appearance_fields(root, theme, cx),
        SettingsTab::Notifications => notification_fields(root, theme, cx),
    };

    let page = div()
        .flex()
        .flex_col()
        .w_full()
        .max_w(px(820.0))
        .mx_auto()
        .gap(px(24.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(7.0))
                .child(
                    div()
                        .id("settings-title")
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
                        .text_size(px(13.0))
                        .line_height(px(21.0))
                        .text_color(theme.dim())
                        .child(hint),
                ),
        )
        .child(fields(theme, rows));

    let content = div()
        .id("settings-content")
        .role(gpui::Role::Main)
        .aria_label("Settings")
        .flex()
        .flex_col()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .h_full()
        .overflow_y_scroll()
        .px(px(48.0))
        .pt(px(48.0))
        .pb(px(64.0))
        .bg(theme.background())
        .child(page);

    div()
        .flex()
        .flex_row()
        .flex_1()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .h_full()
        .bg(theme.background())
        .overflow_hidden()
        .child(navigation)
        .child(content)
}

fn general_fields(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> Vec<Stateful<Div>> {
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
                "Composer predictions",
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
                "Keep awake",
                awake,
                theme,
                cx,
                move |root, _, cx| root.save_prefs(json!({ "keep_awake": !awake }), cx),
            ),
        ),
    ]
}

fn appearance_fields(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> Vec<Stateful<Div>> {
    let prefs = root.prefs.clone();
    let theme_name = root.current_theme.clone();
    let language = root.language.clone();
    let scale = prefs["font_scale"].as_f64().unwrap_or(1.0);
    let sidebar = prefs["sidebar_visible"].as_bool().unwrap_or(true);

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

    let mut themes = div().flex().flex_row().flex_wrap().gap(px(10.0));
    for name in root.theme_names.clone() {
        let choice = name.clone();
        let preview = root.theme_previews.get(&name).cloned().unwrap_or_else(|| {
            if theme_name == name {
                theme.clone()
            } else {
                Theme::default()
            }
        });
        themes = themes.child(theme_card(
            SharedString::from(format!("settings-theme-{name}")),
            &capitalised(&name),
            &preview,
            theme_name == name,
            theme,
            cx,
            move |root, _, cx| root.set_theme(choice.clone(), cx),
        ));
    }

    let mut languages = segments(theme).flex_wrap().max_w(px(560.0));
    for row in &root.languages {
        let Some(tag) = row["tag"].as_str() else {
            continue;
        };
        let label = row["nativeLabel"]
            .as_str()
            .or_else(|| row["label"].as_str())
            .unwrap_or(tag);
        let choice = tag.to_string();
        languages = languages.child(segment(
            SharedString::from(format!("settings-language-{tag}")),
            label,
            language == tag,
            theme,
            cx,
            move |root, _, cx| root.set_language(choice.clone(), cx),
        ));
    }

    vec![
        field_tall(
            "Theme",
            "The palette the window, the terminal and the Electron fallback all read.",
            theme,
            themes,
        ),
        field_tall(
            "Language",
            "The strings the window draws, from the shared catalog.",
            theme,
            languages,
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
                "Sidebar",
                sidebar,
                theme,
                cx,
                move |root, _, cx| root.save_prefs(json!({ "sidebar_visible": !sidebar }), cx),
            ),
        ),
    ]
}

fn notification_fields(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> Vec<Stateful<Div>> {
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
                "Approval requests",
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
                "Question requests",
                questions,
                theme,
                cx,
                move |root, _, cx| root.save_prefs(json!({ "notify_questions": !questions }), cx),
            ),
        ),
    ]
}

/// The card a section's rows sit in, with a hairline between them.
fn fields(theme: &Theme, rows: Vec<Stateful<Div>>) -> gpui::Div {
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

fn field(label: &str, hint: &str, theme: &Theme, control: impl IntoElement) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("settings-field-{label}")))
        .role(gpui::Role::Group)
        .aria_label(label.to_string())
        .aria_description(hint.to_string())
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

fn field_tall(label: &str, hint: &str, theme: &Theme, control: impl IntoElement) -> Stateful<Div> {
    div()
        .id(SharedString::from(format!("settings-field-{label}")))
        .role(gpui::Role::Group)
        .aria_label(label.to_string())
        .aria_description(hint.to_string())
        .flex()
        .flex_col()
        .gap(px(12.0))
        .px(px(24.0))
        .py(px(20.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(5.0))
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
        .child(control)
}

fn input_box(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    placeholder: &'static str,
    role: gpui::Role,
    editor: gpui::Entity<crate::editor::Editor>,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> Stateful<Div> {
    let focus_editor = editor.clone();
    let click_editor = editor.clone();
    let value_editor = editor.clone();
    let value = editor.read(cx).text().to_string();
    div()
        .id(id)
        .role(role)
        .aria_label(label)
        .aria_placeholder(placeholder)
        .aria_value(value)
        .on_a11y_action(gpui::AccessibleAction::Focus, move |_, window, cx| {
            window.focus(&focus_editor.read(cx).focus_handle(cx), cx);
        })
        .on_a11y_action(gpui::AccessibleAction::SetValue, move |data, _, cx| {
            if let Some(gpui::accesskit::ActionData::Value(value)) = data {
                value_editor.update(cx, |editor, cx| editor.set_text(value.to_string(), cx));
            }
        })
        .on_mouse_down(
            gpui::MouseButton::Left,
            cx.listener(move |_, _, window, cx| {
                window.focus(&click_editor.read(cx).focus_handle(cx), cx);
            }),
        )
        .h(px(34.0))
        .px_2()
        .flex()
        .items_center()
        .rounded(px(8.0))
        .bg(theme.background())
        .border_1()
        .border_color(theme.border())
        .child(editor)
}

fn capitalised(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().collect::<String>() + characters.as_str(),
        None => String::new(),
    }
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
        .role(gpui::Role::Tab)
        .aria_label(label.to_string())
        .aria_selected(active)
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
        .role(gpui::Role::RadioButton)
        .aria_label(label.to_string())
        .aria_selected(active)
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

fn theme_card(
    id: impl Into<gpui::ElementId>,
    label: &str,
    preview: &Theme,
    active: bool,
    theme: &Theme,
    cx: &mut Context<Root>,
    on_click: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> Stateful<Div> {
    let mut swatches = div()
        .flex()
        .flex_row()
        .h(px(62.0))
        .w_full()
        .overflow_hidden()
        .rounded(px(8.0));
    for slot in ["background", "panel", "accent", "text", "success", "tool"] {
        swatches = swatches.child(div().flex_1().h_full().bg(preview.color(slot)));
    }
    div()
        .id(id)
        .role(gpui::Role::RadioButton)
        .aria_label(label.to_string())
        .aria_selected(active)
        .flex()
        .flex_col()
        .gap(px(7.0))
        .w(px(142.0))
        .p(px(7.0))
        .rounded(px(12.0))
        .border_1()
        .border_color(if active {
            theme.accent()
        } else {
            theme.border()
        })
        .bg(if active {
            theme.panel_3()
        } else {
            theme.panel_2()
        })
        .cursor_pointer()
        .hover(move |style| style.border_color(theme.dim()))
        .child(swatches)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .h(px(22.0))
                .px(px(2.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if active { theme.text() } else { theme.dim() })
                        .child(label.to_string()),
                )
                .children(active.then(|| icons::icon(Icon::Check, 13.0, theme.accent()))),
        )
        .on_click(cx.listener(move |root, _, window, cx| on_click(root, window, cx)))
}

fn about_dialog(theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    dialog_with_close(theme, "About Oxide", 430.0, cx)
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(14.0))
                .child(icons::mark(44.0))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.0))
                        .child(
                            div()
                                .text_size(px(17.0))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text())
                                .child("Oxide for Desktop"),
                        )
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme.dim())
                                .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                        ),
                ),
        )
        .child(
            div()
                .mt(px(12.0))
                .text_size(px(12.0))
                .line_height(px(18.0))
                .text_color(theme.dim())
                .child(
                    "The native GPUI desktop renderer, sharing configuration, sessions, approvals and tools with the terminal, editor and Electron fallback.",
                ),
        )
}

fn image_preview(
    theme: &Theme,
    name: String,
    image: std::sync::Arc<gpui::Image>,
    cx: &mut Context<Root>,
) -> gpui::Div {
    dialog_with_close(theme, &name, 900.0, cx).child(
        div()
            .flex()
            .items_center()
            .justify_center()
            .w_full()
            .h(px(600.0))
            .overflow_hidden()
            .rounded(px(9.0))
            .bg(theme.background())
            .child(
                gpui::img(image)
                    .w_full()
                    .h_full()
                    .object_fit(gpui::ObjectFit::Contain),
            ),
    )
}

fn update_dialog(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let mut panel = if root.updating_update {
        dialog(theme, "Updates", 560.0)
    } else {
        dialog_with_close(theme, "Updates", 560.0, cx)
    };

    if root.updating_update {
        let progress = root.update_progress.clone().unwrap_or_else(|| json!({}));
        let stage = progress["stage"].as_str().unwrap_or("checking");
        let version = progress["version"].as_str().unwrap_or_default();
        let received = progress["received"].as_u64().unwrap_or(0);
        let total = progress["total"].as_u64().unwrap_or(0);
        let transfer = received as f64 / 1_048_576.0;
        let size = total as f64 / 1_048_576.0;
        let (heading, detail) = match stage {
            "downloading" if version.is_empty() => (
                "Downloading the update".to_string(),
                "The verified installer is being downloaded.".to_string(),
            ),
            "downloading" => (
                format!("Downloading Oxide {version}"),
                if total > 0 {
                    format!("{transfer:.1} of {size:.1} MB")
                } else {
                    format!("{transfer:.1} MB downloaded")
                },
            ),
            "verifying" => (
                "Verifying the download".to_string(),
                "Checking the installer against the release’s SHA-256 checksum.".to_string(),
            ),
            "installing" => (
                format!(
                    "Installing Oxide{}",
                    if version.is_empty() {
                        String::new()
                    } else {
                        format!(" {version}")
                    }
                ),
                "Putting the new app in place.".to_string(),
            ),
            _ => (
                "Checking for updates".to_string(),
                "Looking for the newest Oxide release.".to_string(),
            ),
        };
        panel = panel
            .child(
                div()
                    .mt(px(2.0))
                    .text_size(px(15.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text())
                    .child(heading),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .text_color(theme.dim())
                    .child(detail),
            );
        if stage == "downloading" || stage == "verifying" {
            let fraction = if total == 0 {
                0.08
            } else {
                (received as f32 / total as f32).clamp(0.0, 1.0)
            };
            panel = panel.child(
                div()
                    .mt(px(4.0))
                    .h(px(6.0))
                    .w_full()
                    .overflow_hidden()
                    .rounded(px(3.0))
                    .bg(theme.panel_3())
                    .child(
                        div()
                            .h_full()
                            .w(px((480.0 * fraction).max(6.0)))
                            .rounded(px(3.0))
                            .bg(theme.accent()),
                    ),
            );
        }
        let mut actions = div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(px(8.0))
            .mt(px(12.0));
        if matches!(stage, "checking" | "downloading" | "verifying") {
            actions = actions.child(Button::new("Cancel").render(
                "updates-cancel",
                theme,
                cx,
                |root, _, cx| root.cancel_update(cx),
            ));
        }
        return panel.child(actions);
    }

    if root.checking_updates {
        return panel
            .child(
                div()
                    .mt(px(2.0))
                    .text_size(px(15.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text())
                    .child("Checking for updates"),
            )
            .child(
                div()
                    .text_size(px(13.0))
                    .text_color(theme.dim())
                    .child("Looking for the newest Oxide release."),
            );
    }
    if let Some(error) = root.update_failure.as_deref() {
        let restart_failed = root.update_restart_failed;
        let installable = root
            .update_check
            .as_ref()
            .and_then(|check| check["installable"].as_bool())
            .unwrap_or(false);
        return panel
            .child(
                div()
                    .mt(px(2.0))
                    .text_size(px(15.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text())
                    .child(if restart_failed {
                        "Couldn’t restart Oxide"
                    } else {
                        "Couldn’t finish the update"
                    }),
            )
            .child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.color("tool_error_bg"))
                    .bg(theme.wash("tool_error_bg", 0.72))
                    .text_size(px(11.5))
                    .line_height(px(17.0))
                    .text_color(theme.error())
                    .child(error.to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .mt(px(12.0))
                    .child(
                        Button::new(if restart_failed {
                            "Restart Again"
                        } else {
                            "Try Again"
                        })
                        .primary()
                        .render(
                            "updates-install-retry",
                            theme,
                            cx,
                            move |root, _, cx| {
                                if restart_failed {
                                    root.restart_for_update(cx);
                                } else if installable {
                                    root.install_update(cx);
                                } else {
                                    root.download_update(cx);
                                }
                            },
                        ),
                    ),
            );
    }
    if let Some(error) = root.update_error.as_deref() {
        return panel
            .child(
                div()
                    .mt(px(2.0))
                    .text_size(px(15.0))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(theme.text())
                    .child("Couldn’t check for updates"),
            )
            .child(
                div()
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .text_color(theme.dim())
                    .child("The release information couldn’t be loaded. Try again in a moment."),
            )
            .child(
                div()
                    .px(px(12.0))
                    .py(px(10.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.color("tool_error_bg"))
                    .bg(theme.wash("tool_error_bg", 0.72))
                    .text_size(px(11.5))
                    .line_height(px(17.0))
                    .text_color(theme.error())
                    .child(error.to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_row()
                    .justify_end()
                    .gap(px(8.0))
                    .mt(px(12.0))
                    .child(Button::new("Try Again").primary().render(
                        "updates-retry",
                        theme,
                        cx,
                        |root, _, cx| root.check_for_updates(cx),
                    )),
            );
    }
    if let Some(result) = root.update_result.as_ref() {
        let version = result["version"].as_str().unwrap_or("the new release");
        let text = result["text"].as_str().unwrap_or_default();
        let path = result["path"].as_str().unwrap_or_default();
        let installed = result.get("pending").is_some();
        let pending = result["pending"].as_bool().unwrap_or(false);
        let heading = if !installed {
            format!("Oxide {version} is downloaded")
        } else if pending {
            format!("The Oxide {version} installer is running")
        } else {
            format!("Oxide {version} is ready")
        };
        panel = panel.child(
            div()
                .mt(px(2.0))
                .text_size(px(15.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text())
                .child(heading),
        );
        if !text.is_empty() {
            panel = panel.child(
                div()
                    .text_size(px(12.5))
                    .line_height(px(18.0))
                    .text_color(theme.dim())
                    .child(text.to_string()),
            );
        }
        if !path.is_empty() {
            panel = panel.child(
                div()
                    .text_size(px(11.5))
                    .line_height(px(17.0))
                    .text_color(theme.faint())
                    .child(path.to_string()),
            );
        }
        let mut actions = div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(px(8.0))
            .mt(px(12.0));
        if installed && !pending {
            actions = actions.child(Button::new("Restart and Install").primary().render(
                "updates-restart",
                theme,
                cx,
                |root, _, cx| root.restart_for_update(cx),
            ));
        }
        return panel.child(actions);
    }
    let Some(check) = root.update_check.as_ref() else {
        return panel;
    };
    let available = check["updateAvailable"].as_bool().unwrap_or(false);
    let installable = check["installable"].as_bool().unwrap_or(false);
    let current = check["current"].as_str().unwrap_or("unknown");
    let latest = check["latest"].as_str().unwrap_or("unknown");
    let released = check["releasedAt"].as_str().unwrap_or_default();
    let asset = check["asset"]["name"].as_str().unwrap_or_default();
    let advice = check["advice"].as_str().unwrap_or_default();
    let release_url = check["releaseUrl"].as_str().unwrap_or_default().to_string();
    let notes = check["notes"].as_str().unwrap_or_default();

    panel = panel
        .child(
            div()
                .text_size(px(17.0))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text())
                .child(if available {
                    format!("Oxide {latest} is available")
                } else {
                    "Oxide is up to date".to_string()
                }),
        )
        .child(
            div()
                .mt(px(4.0))
                .text_size(px(12.0))
                .text_color(theme.dim())
                .child(format!(
                    "Current {current} · Latest {latest}{}",
                    if released.is_empty() {
                        String::new()
                    } else {
                        format!(" · {released}")
                    }
                )),
        );
    if available {
        if !asset.is_empty() {
            panel = panel.child(
                div()
                    .mt(px(10.0))
                    .text_size(px(12.0))
                    .text_color(theme.dim())
                    .child(format!("Download: {asset}")),
            );
        }
        if !advice.is_empty() {
            panel = panel.child(
                div()
                    .mt(px(8.0))
                    .text_size(px(12.0))
                    .line_height(px(18.0))
                    .text_color(theme.dim())
                    .child(advice.to_string()),
            );
        }
        if !notes.is_empty() {
            panel = panel.child(
                div()
                    .id("update-notes")
                    .mt(px(10.0))
                    .max_h(px(190.0))
                    .overflow_y_scroll()
                    .px(px(10.0))
                    .py(px(8.0))
                    .rounded(px(8.0))
                    .border_1()
                    .border_color(theme.border())
                    .bg(theme.background())
                    .text_size(px(11.5))
                    .line_height(px(17.0))
                    .text_color(theme.dim())
                    .child(notes.to_string()),
            );
        }
    }
    let mut actions = div()
        .flex()
        .flex_row()
        .justify_end()
        .gap(px(8.0))
        .mt(px(12.0));
    if !release_url.is_empty() {
        actions = actions.child(Button::new("Release Notes").render(
            "updates-release",
            theme,
            cx,
            move |_, _, cx| cx.open_url(&release_url),
        ));
    }
    if available && !asset.is_empty() {
        actions = actions.child(if installable {
            Button::new("Install Update").primary().render(
                "updates-install",
                theme,
                cx,
                |root, _, cx| root.install_update(cx),
            )
        } else {
            Button::new("Download").primary().render(
                "updates-download",
                theme,
                cx,
                |root, _, cx| root.download_update(cx),
            )
        });
    }
    panel.child(actions)
}

fn switch(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
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
        .role(gpui::Role::Switch)
        .aria_label(label)
        .aria_toggled(if on {
            gpui::Toggled::True
        } else {
            gpui::Toggled::False
        })
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

// ---------- models ----------

fn command_menu(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let query = root.command_query.read(cx).text().to_ascii_lowercase();
    let command_focus = root.command_query.clone();
    let command_value = root.command_query.clone();
    let entries = [
        ("new-chat", "New chat", "⌘T"),
        ("new-tab", "New tab", ""),
        ("close-chat", "Close chat", "⌘W"),
        ("toggle-sidebar", "Show or hide the sidebar", "⌘B"),
        ("back", "Back to the previous chat", "⌘["),
        ("forward", "Forward to the next chat", "⌘]"),
        ("open-project", "Open a folder…", "⌘O"),
        ("search", "Search chats…", "⌘⇧F"),
        ("panel", "Show or hide the panel", "⌘J"),
        ("changes", "Review changes", "⌘⏎"),
        ("terminal", "Open terminal", "⌘⇧T"),
        ("pulls", "Review pull requests", "⌘⇧P"),
        ("connect", "Connect a provider…", ""),
        ("projects", "Manage projects…", ""),
        ("customize", "Customize plugins and integrations…", ""),
        ("worktrees", "Git worktrees…", "⌘⇧G"),
        ("model", "Choose a model…", "⌘⇧M"),
        ("computer-use", "Computer use…", ""),
        ("permissions", "Tool approvals…", ""),
        ("mcps", "MCP servers…", ""),
        ("settings", "Settings…", "⌘,"),
        ("shortcuts", "Keyboard shortcuts", ""),
        ("updates", "Check for updates…", ""),
    ];
    let panel = dialog(theme, "Command menu", 430.0).child(
        div()
            .id("command-search")
            .role(gpui::Role::SearchInput)
            .aria_label("Search commands")
            .aria_value(query.clone())
            .on_a11y_action(gpui::AccessibleAction::Focus, move |_, window, cx| {
                window.focus(&command_focus.read(cx).focus_handle(cx), cx);
            })
            .on_a11y_action(gpui::AccessibleAction::SetValue, move |data, _, cx| {
                if let Some(gpui::accesskit::ActionData::Value(value)) = data {
                    command_value.update(cx, |editor, cx| editor.set_text(value.to_string(), cx));
                }
            })
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .h(px(36.0))
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
                    .child(root.command_query.clone()),
            ),
    );
    let mut list = div()
        .id("command-menu-list")
        .role(gpui::Role::Menu)
        .aria_label("Commands")
        .flex()
        .flex_col()
        .gap(px(2.0))
        .max_h(px(430.0))
        .overflow_y_scroll();
    let mut shown = 0;
    for (id, label, hint) in entries {
        if !query.is_empty() && !label.to_ascii_lowercase().contains(&query) {
            continue;
        }
        shown += 1;
        let action = id.to_string();
        list = list.child(
            div()
                .id(SharedString::from(format!("command-menu-{id}")))
                .role(gpui::Role::MenuItem)
                .aria_label(label)
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .min_h(px(34.0))
                .px_2()
                .rounded(px(7.0))
                .cursor_pointer()
                .hover(move |style| style.bg(theme.panel_2()))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(12.5))
                        .text_color(theme.text())
                        .child(label),
                )
                .children((!hint.is_empty()).then(|| {
                    div()
                        .flex_none()
                        .text_size(px(10.5))
                        .text_color(theme.faint())
                        .child(hint)
                }))
                .on_click(cx.listener(move |root, _, window, cx| {
                    run_command_menu_action(&action, root, window, cx)
                })),
        );
    }
    if shown == 0 {
        list = list.child(
            div()
                .p_3()
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child("No command matches that"),
        );
    }
    panel.child(list)
}

fn run_command_menu_action(
    action: &str,
    root: &mut Root,
    window: &mut Window,
    cx: &mut Context<Root>,
) {
    root.overlay = None;
    match action {
        "new-chat" | "new-tab" => root.new_chat(window, cx),
        "close-chat" => root.close_chat(root.active, cx),
        "toggle-sidebar" => {
            root.sidebar_visible = !root.sidebar_visible;
            let visible = root.sidebar_visible;
            root.save_prefs(json!({ "sidebar_visible": visible }), cx);
        }
        "back" => root.navigate_back(cx),
        "forward" => root.navigate_forward(cx),
        "open-project" => root.open_new_project(window, cx),
        "search" => root.open_search(window, cx),
        "panel" => root.toggle_details(cx),
        "changes" => root.open_repository_changes(cx),
        "terminal" => root.open_terminal(window, cx),
        "pulls" => root.open_pulls(cx),
        "connect" => root.open_connect("a provider".to_string(), cx),
        "projects" => {
            root.rail = crate::app::Rail::Projects;
            cx.notify();
        }
        "customize" => {
            root.rail = crate::app::Rail::Customize;
            root.load_plugins(cx);
            cx.notify();
        }
        "worktrees" => root.open_worktrees(cx),
        "model" => root.open_model_menu(cx),
        "computer-use" => root.open_computer_use(cx),
        "permissions" => root.open_overlay(Overlay::Approvals, cx),
        "mcps" => root.open_overlay(Overlay::Mcp, cx),
        "settings" => root.open_overlay(Overlay::Settings, cx),
        "shortcuts" => root.open_overlay(Overlay::Shortcuts, cx),
        "updates" => root.check_for_updates(cx),
        _ => cx.notify(),
    }
}

fn command_help(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let commands = root.commands.clone();
    let mut panel = dialog_with_close(theme, "Slash commands", 620.0, cx).child(
        div()
            .text_size(px(12.5))
            .line_height(px(19.0))
            .text_color(theme.dim())
            .child(
                "Commands run by Oxide, plus the commands and skills this project makes available.",
            ),
    );
    if commands.is_empty() {
        panel = panel.child(
            div()
                .p_3()
                .rounded(px(9.0))
                .bg(theme.panel_2())
                .text_size(px(12.5))
                .text_color(theme.faint())
                .child("No commands are available for this project."),
        );
    } else {
        let mut list = div()
            .id("command-help-list")
            .flex()
            .flex_col()
            .gap(px(2.0))
            .max_h(px(440.0))
            .overflow_y_scroll();
        for command in commands {
            let name = command["name"].as_str().unwrap_or_default().to_string();
            let description = command["description"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            let arguments = command["arguments"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_3()
                    .min_h(px(36.0))
                    .px_2()
                    .rounded(px(7.0))
                    .hover({
                        let theme = theme.clone();
                        move |style| style.bg(theme.panel_2())
                    })
                    .child(
                        div()
                            .flex_none()
                            .w(px(130.0))
                            .font_family("Menlo")
                            .text_size(px(12.0))
                            .text_color(theme.text())
                            .child(format!("/{name}")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .text_size(px(11.5))
                            .text_color(theme.dim())
                            .child(description),
                    )
                    .children((!arguments.is_empty()).then(|| {
                        div()
                            .flex_none()
                            .text_size(px(10.5))
                            .text_color(theme.faint())
                            .child(arguments)
                    })),
            );
        }
        panel = panel.child(list);
    }
    panel
}

fn mcp_servers(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let servers = root.sources.clone();
    let mut panel = dialog_with_close(theme, "MCP servers", 580.0, cx).child(
        div()
            .text_size(px(12.5))
            .line_height(px(19.0))
            .text_color(theme.dim())
            .child("The MCP servers this project loads and their connection status."),
    );
    if servers.is_empty() {
        panel = panel.child(
            div()
                .p_3()
                .rounded(px(9.0))
                .bg(theme.panel_2())
                .text_size(px(12.5))
                .text_color(theme.faint())
                .child("No MCP servers are configured for this project."),
        );
    } else {
        let mut list = div()
            .id("mcp-server-list")
            .flex()
            .flex_col()
            .gap_1()
            .max_h(px(430.0))
            .overflow_y_scroll();
        for server in servers {
            let name = server["name"].as_str().unwrap_or_default().to_string();
            let detail = server["detail"].as_str().unwrap_or_default().to_string();
            let transport = server["transport"].as_str().unwrap_or_default();
            let source = server["source"].as_str().unwrap_or_default();
            let tools = server["tools"].as_u64().unwrap_or(0);
            let status = server["status"].as_str().unwrap_or("Ready").to_string();
            let state = server["state"].as_str().unwrap_or_default();
            let enabled = server["enabled"].as_bool().unwrap_or(true);
            let status = if !enabled { "Off".to_string() } else { status };
            let color = if state == "connected" {
                theme.success()
            } else if !enabled || state == "disabled" {
                theme.faint()
            } else {
                theme.tool()
            };
            let facts = [transport, source]
                .into_iter()
                .filter(|fact| !fact.is_empty())
                .collect::<Vec<_>>()
                .join(" · ");
            list = list.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(3.0))
                    .px_2()
                    .py(px(8.0))
                    .rounded(px(9.0))
                    .bg(theme.panel_2())
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .gap_2()
                            .child(icons::icon(Icon::Globe, 14.0, theme.faint()))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .text_size(px(12.5))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(theme.text())
                                    .child(name),
                            )
                            .child(
                                div()
                                    .flex_none()
                                    .text_size(px(11.0))
                                    .text_color(color)
                                    .child(status),
                            ),
                    )
                    .children((!detail.is_empty()).then(|| {
                        div()
                            .ml(px(22.0))
                            .font_family("Menlo")
                            .text_size(px(10.5))
                            .text_color(theme.dim())
                            .child(detail)
                    }))
                    .children((!facts.is_empty() || tools > 0).then(|| {
                        let mut facts = facts;
                        if tools > 0 {
                            if !facts.is_empty() {
                                facts.push_str(" · ");
                            }
                            facts.push_str(&format!(
                                "{tools} tool{}",
                                if tools == 1 { "" } else { "s" }
                            ));
                        }
                        div()
                            .ml(px(22.0))
                            .text_size(px(10.5))
                            .text_color(theme.faint())
                            .child(facts)
                    })),
            );
        }
        panel = panel.child(list);
    }
    panel
}

fn agents_picker(root: &Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let current = root
        .chats
        .get(root.active)
        .and_then(|chat| chat.agent.clone());
    let agents = root.info["agents"].as_array().cloned().unwrap_or_default();
    let panel = dialog_with_close(theme, "Agent", 500.0, cx).child(
        div()
            .text_size(px(12.5))
            .line_height(px(19.0))
            .text_color(theme.dim())
            .child("Choose the default agent for this chat."),
    );
    let mut list = div()
        .id("agent-picker-list")
        .flex()
        .flex_col()
        .gap_1()
        .w_full()
        .min_w(px(0.0))
        .max_h(px(500.0))
        .overflow_y_scroll();
    list = list.child(selectable(
        "agent-default",
        "Default agent".to_string(),
        Some("Use the project's normal instructions.".to_string()),
        current.is_none(),
        theme,
        cx,
        |root, _, cx| root.set_agent(None, cx),
    ));
    for agent in agents {
        let name = agent["name"].as_str().unwrap_or("Agent").to_string();
        let description = agent["description"].as_str().map(str::to_string);
        let selected = current.as_deref() == Some(name.as_str());
        let choice = name.clone();
        list = list.child(selectable(
            SharedString::from(format!("agent-{name}")),
            name,
            description,
            selected,
            theme,
            cx,
            move |root, _, cx| root.set_agent(Some(choice.clone()), cx),
        ));
    }
    panel.child(list)
}

// ---------- shortcuts ----------

fn shortcuts(theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let rows = [
        ("New chat", "⌘N"),
        ("Close chat", "⌘W"),
        ("Show or hide the sidebar", "⌘B"),
        ("Activities", "⌘⇧A"),
        ("Settings", "⌘,"),
        ("Changes pane", "⌘⏎"),
        ("Model picker", "⌘⇧M"),
        ("Git worktrees", "⌘⇧G"),
        ("Open a sidebar chat", "⌘1–⌘9"),
        ("Back / forward", "⌘[ / ⌘]"),
        ("Send", "Enter"),
        ("New line", "⇧⏎"),
        ("Close a dialog", "Esc"),
    ];
    let mut panel = dialog_with_close(theme, "Keyboard shortcuts", 420.0, cx);
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
    panel
}

// ---------- trust ----------

fn trust(
    root: &mut Root,
    theme: &Theme,
    project: String,
    onboarding: bool,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let info = root.info.clone();
    let trusted = !onboarding && info["trust"]["trusted"].as_bool().unwrap_or(false);
    let required = info["trust"]["required"].as_bool().unwrap_or(false);
    let resources = info["trust"]["resources"]
        .as_array()
        .map(|resources| {
            resources
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let mut panel = if onboarding {
        dialog(theme, "Project access", 480.0)
    } else {
        dialog_with_close(theme, "Project access", 480.0, cx)
    }
    .child(
        div()
            .text_size(px(12.5))
            .line_height(px(19.0))
            .text_color(theme.dim())
            .child(format!(
                "{project}\n\n{}\n\nA trusted project loads its own skills, plugins, MCP servers and hooks. Its tools can already read any path this process can, so trust is about the project's resources, not about file access — the same gate the terminal and the Electron window use.",
                if onboarding {
                    "Choose whether this project's own resources may load."
                } else if trusted {
                    "Project resources are trusted."
                } else if required {
                    "This folder has project resources that can reshape the agent."
                } else {
                    "This folder does not currently contain project resources that require trust."
                }
            )),
    );
    if !resources.is_empty() {
        panel = panel.child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .text_size(px(11.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.faint())
                        .child("GATED RESOURCES"),
                )
                .children(resources.into_iter().map(|resource| {
                    div()
                        .font_family("Menlo")
                        .text_size(px(11.0))
                        .text_color(theme.dim())
                        .child(resource)
                })),
        );
    }
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
    let allow_path = project.clone();
    let deny_path = project.clone();
    let mut actions = div().flex().flex_row().gap_2().child(
        Button::new(if trusted {
            "Stop trusting"
        } else {
            "Trust project"
        })
        .primary()
        .render("trust-toggle", theme, cx, move |root, _, cx| {
            root.set_trust(allow_path.clone(), !trusted, cx)
        }),
    );
    if !trusted {
        actions = actions.child(Button::new("Don't trust").render(
            "trust-deny",
            theme,
            cx,
            move |root, _, cx| root.set_trust(deny_path.clone(), false, cx),
        ));
    }
    if !onboarding {
        actions = actions.child(Button::new("Cancel").render(
            "trust-cancel",
            theme,
            cx,
            |root, _, cx| {
                root.overlay = None;
                cx.notify();
            },
        ));
    }
    panel = panel.child(actions);
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

    let mut panel = dialog_with_close(theme, &header, 560.0, cx);
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
            .justify_start()
            .items_center()
            .child(Button::new("Dismiss").render(
                "question-dismiss",
                theme,
                cx,
                move |root, _, cx| root.resolve_question(id, json!([]), cx),
            )),
    );
    panel
}

// ---------- session and project actions ----------

fn session_actions(
    theme: &Theme,
    id: String,
    title: String,
    project: String,
    pinned: bool,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let name = if title.trim().is_empty() {
        id.clone()
    } else {
        title.clone()
    };
    let mut panel = dialog_with_close(theme, &name, 460.0, cx).child(
        div()
            .text_size(px(11.5))
            .text_color(theme.faint())
            .child("What to do with this chat"),
    );
    let pin_id = id.clone();
    let rename_id = id.clone();
    let archive_id = id.clone();
    let rename_title = name.clone();
    panel = panel
        .child(Button::new("Rename").render(
            SharedString::from(format!("session-rename-{id}")),
            theme,
            cx,
            move |root, _, cx| {
                root.session_name
                    .update(cx, |editor, cx| editor.set_text(rename_title.clone(), cx));
                root.overlay = Some(Overlay::RenameSession {
                    project: project.clone(),
                    id: rename_id.clone(),
                    title: rename_title.clone(),
                });
                cx.notify();
            },
        ))
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
        .child(Button::new("Archive thread").render(
            SharedString::from(format!("session-archive-action-{id}")),
            theme,
            cx,
            move |root, _, cx| root.archive_session(archive_id.clone(), cx),
        ));
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
    let mut panel = dialog_with_close(theme, &name, 520.0, cx).child(
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
        ));
    panel
}

fn rename_session(
    root: &mut Root,
    theme: &Theme,
    project: String,
    id: String,
    _title: String,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let project_for_save = project.clone();
    let id_for_save = id.clone();
    dialog_with_close(theme, "Rename chat", 440.0, cx)
        .child(editor_field(
            "session-title",
            "Chat title",
            "Chat title",
            root.session_name.clone(),
            theme,
            cx,
        ))
        .child(div().flex().flex_row().justify_end().gap_2().child(
            Button::new("Save").primary().render(
                "rename-session-save",
                theme,
                cx,
                move |root, _, cx| {
                    root.rename_session(project_for_save.clone(), id_for_save.clone(), cx)
                },
            ),
        ))
}

// ---------- worktrees ----------

fn worktrees(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let worktrees = root.worktrees.clone();
    let current = root.current_project();
    let owner = root.current_project_owner();
    let error = root.worktrees_error.clone();
    let has_session = root
        .chats
        .get(root.active)
        .is_some_and(|chat| chat.session.is_some());
    let on_worktree = root.current_worktree().is_some();
    let mut panel = dialog_with_close(theme, "Git worktrees", 620.0, cx).child(
        div()
            .text_size(px(12.5))
            .line_height(px(19.0))
            .text_color(theme.dim())
            .child(
                "Run a chat in an isolated checkout. The project's local folder stays in place.",
            ),
    );

    if let Some(error) = error {
        panel = panel.child(
            div()
                .px_3()
                .py_2()
                .rounded(px(8.0))
                .bg(theme.color("tool_error_bg"))
                .text_size(px(12.0))
                .text_color(theme.error())
                .child(format!("Could not read worktrees: {error}")),
        );
    }

    let mut rows = div()
        .id("worktree-list")
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(250.0))
        .overflow_y_scroll();
    rows = rows.child(worktree_row(
        "Local checkout",
        owner.as_deref().unwrap_or("This project's own folder"),
        !on_worktree,
        theme,
        cx,
        |root, window, cx| root.select_worktree(window, None, cx),
    ));
    for worktree in &worktrees {
        let path = worktree.path.clone();
        let project = worktree.project.clone();
        let branch_project = project.clone();
        let remove_project = project.clone();
        let name = worktree.name.clone();
        let branch_name = worktree.name.clone();
        let selected = current.as_deref() == Some(path.as_str());
        let mut row = worktree_row(
            &format!("{} · {}", worktree.name, worktree.branch),
            &worktree.path,
            selected,
            theme,
            cx,
            move |root, window, cx| root.select_worktree(window, Some(path.clone()), cx),
        );
        row = row
            .child(
                icon_button(
                    SharedString::from(format!("worktree-branch-{name}")),
                    Icon::Branch,
                    24.0,
                    theme,
                    false,
                )
                .tooltip(crate::views::tip("Create a branch here", theme))
                .on_click(cx.listener(move |root, _, _, cx| {
                    cx.stop_propagation();
                    root.worktree_branch.update(cx, |editor, cx| editor.clear(cx));
                    root.overlay = Some(Overlay::WorktreeBranch {
                        project: branch_project.clone(),
                        name: branch_name.clone(),
                    });
                    cx.notify();
                })),
            )
            .child(
                icon_button(
                    SharedString::from(format!("worktree-remove-{name}")),
                    Icon::Close,
                    24.0,
                    theme,
                    false,
                )
                .tooltip(crate::views::tip("Remove this worktree", theme))
                .on_click(cx.listener(move |root, _, _, cx| {
                    cx.stop_propagation();
                    root.overlay = Some(Overlay::Confirm {
                        title: "Remove worktree".to_string(),
                        body: format!(
                            "Remove the worktree {name}? Its commits stay on a branch in the repository."
                        ),
                        action: ConfirmAction::RemoveWorktree {
                            project: remove_project.clone(),
                            name: name.clone(),
                        },
                    });
                    cx.notify();
                })),
            );
        rows = rows.child(row);
    }
    panel = panel.child(rows);

    if has_session {
        panel = panel.child(
            Button::new(if on_worktree {
                "Hand off chat to Local"
            } else {
                "Hand off chat to Worktree"
            })
            .render("worktree-handoff", theme, cx, |root, _, cx| {
                root.handoff_current_session(cx)
            }),
        );
    }

    panel = panel
        .child(div().h(px(1.0)).w_full().bg(theme.border()))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Create worktree"),
        )
        .child(editor_field(
            "worktree-name",
            "Worktree name",
            "Worktree name",
            root.worktree_name.clone(),
            theme,
            cx,
        ))
        .child(editor_field(
            "worktree-base",
            "Worktree base revision",
            "HEAD (current checkout)",
            root.worktree_base.clone(),
            theme,
            cx,
        ))
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.dim())
                        .child("Start detached"),
                )
                .child(switch(
                    "worktree-detached",
                    "Start detached",
                    root.worktree_detached,
                    theme,
                    cx,
                    |root, _, cx| {
                        root.worktree_detached = !root.worktree_detached;
                        cx.notify();
                    },
                )),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .justify_between()
                .gap_3()
                .child(
                    div()
                        .text_size(px(12.0))
                        .text_color(theme.dim())
                        .child("Include uncommitted changes"),
                )
                .child(switch(
                    "worktree-copy-changes",
                    "Include uncommitted changes",
                    root.worktree_copy_changes,
                    theme,
                    cx,
                    |root, _, cx| {
                        root.worktree_copy_changes = !root.worktree_copy_changes;
                        cx.notify();
                    },
                )),
        )
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .child(Button::new("Create").primary().render(
                    "worktree-create",
                    theme,
                    cx,
                    |root, _, cx| root.create_worktree(cx),
                ))
                .child(Button::new("Refresh").render(
                    "worktree-refresh",
                    theme,
                    cx,
                    |root, _, cx| root.load_worktrees(cx),
                )),
        );
    panel
}

fn worktree_row(
    label: &str,
    detail: &str,
    selected: bool,
    theme: &Theme,
    cx: &mut Context<Root>,
    select: impl Fn(&mut Root, &mut Window, &mut Context<Root>) + 'static,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(format!("worktree-{label}")))
        .flex()
        .flex_row()
        .items_center()
        .gap_2()
        .min_h(px(42.0))
        .px_2()
        .rounded(px(8.0))
        .cursor_pointer()
        .bg(if selected {
            theme.panel_2()
        } else {
            theme.panel()
        })
        .hover(move |style| style.bg(theme.panel_2()))
        .child(icons::icon(
            if label == "Local checkout" {
                Icon::Folder
            } else {
                Icon::Worktree
            },
            16.0,
            if selected {
                theme.accent()
            } else {
                theme.dim()
            },
        ))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .flex_1()
                .min_w(px(0.0))
                .child(
                    div()
                        .text_size(px(12.5))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme.text())
                        .child(label.to_string()),
                )
                .child(
                    div()
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .font_family("Menlo")
                        .text_size(px(10.5))
                        .text_color(theme.faint())
                        .child(detail.to_string()),
                ),
        )
        .children(selected.then(|| pill("Active", theme, true)))
        .on_click(cx.listener(move |root, _, window, cx| select(root, window, cx)))
}

fn editor_field(
    id: &'static str,
    label: &'static str,
    placeholder: &'static str,
    editor: gpui::Entity<crate::editor::Editor>,
    theme: &Theme,
    cx: &mut Context<Root>,
) -> Stateful<Div> {
    input_box(
        id,
        label,
        placeholder,
        gpui::Role::TextInput,
        editor,
        theme,
        cx,
    )
}

fn worktree_branch(
    root: &mut Root,
    theme: &Theme,
    project: String,
    name: String,
    cx: &mut Context<Root>,
) -> gpui::Div {
    let label = format!("Create a branch in {name}");
    let project_for_create = project.clone();
    let name_for_create = name.clone();
    dialog_with_close(theme, &label, 460.0, cx)
        .child(
            div()
                .text_size(px(12.5))
                .text_color(theme.dim())
                .child("The worktree switches to this branch."),
        )
        .child(editor_field(
            "worktree-branch",
            "Branch name",
            "Branch name",
            root.worktree_branch.clone(),
            theme,
            cx,
        ))
        .child(div().flex().flex_row().justify_end().gap_2().child(
            Button::new("Create branch").primary().render(
                "worktree-branch-create",
                theme,
                cx,
                move |root, _, cx| {
                    root.create_worktree_branch(
                        project_for_create.clone(),
                        name_for_create.clone(),
                        cx,
                    )
                },
            ),
        ))
}

fn projects(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let projects = root.projects.clone();
    let current = root.current_project_owner();
    let mut panel = dialog_with_close(theme, "Projects", 480.0, cx).child(
        div()
            .text_size(px(12.5))
            .text_color(theme.dim())
            .child("Choose the folder the next chat runs in."),
    );
    let mut list = div()
        .id("composer-project-list")
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(360.0))
        .overflow_y_scroll();
    for project in projects {
        let path = project.path.clone();
        let name = if project.name.is_empty() {
            project.display_path.clone()
        } else {
            project.name.clone()
        };
        let selected = current.as_deref() == Some(path.as_str());
        list = list.child(
            div()
                .id(SharedString::from(format!("composer-project-{path}")))
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .h(px(40.0))
                .px_2()
                .rounded(px(8.0))
                .cursor_pointer()
                .bg(if selected {
                    theme.panel_2()
                } else {
                    gpui::transparent_black()
                })
                .hover(move |style| style.bg(theme.panel_2()))
                .child(icons::icon(Icon::Folder, 16.0, theme.dim()))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w(px(0.0))
                        .child(
                            div()
                                .text_size(px(12.5))
                                .text_color(theme.text())
                                .child(name),
                        )
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .font_family("Menlo")
                                .text_size(px(10.5))
                                .text_color(theme.faint())
                                .child(project.display_path),
                        ),
                )
                .children(selected.then(|| pill("Current", theme, true)))
                .on_click(cx.listener(move |root, _, window, cx| {
                    root.select_project(window, path.clone(), cx)
                })),
        );
    }
    panel = panel.child(list);
    panel = panel.child(div().flex().flex_row().items_center().gap_2().child(
        Button::new("Add project").render(
            "composer-projects-add",
            theme,
            cx,
            |root, window, cx| root.open_new_project(window, cx),
        ),
    ));
    panel
}

// ---------- approvals ----------

/// The approvals the composer's lock names: the tools this project runs without
/// asking first. It is the same store the terminal and the Electron window
/// read, and clearing it puts every call back behind a prompt.
fn approvals(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let tools = root.approvals.clone();
    let mut panel = dialog_with_close(theme, "Tool approvals", 470.0, cx).child(
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
    panel
}

// ---------- computer use ----------

fn computer_use(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let chat = root.chats.get(root.active);
    let activity = chat.and_then(|chat| chat.computer_use());
    let can_stop = chat.is_some_and(|chat| chat.running())
        && activity.as_ref().is_some_and(|activity| activity.running);
    let policy = root.computer_use_policy.clone();
    let mut panel = dialog_with_close(theme, "Computer use", 620.0, cx).child(
        div()
            .flex()
            .flex_col()
            .gap_2()
            .p_3()
            .rounded(px(10.0))
            .bg(theme.panel_2())
            .child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .child(icons::icon(Icon::Pointer, 16.0, theme.accent()))
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(13.0))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme.text())
                            .child(if let Some(activity) = &activity {
                                activity.status_label()
                            } else {
                                "No active Computer Use session"
                            }),
                    )
                    .children(activity.as_ref().map(|activity| {
                        div()
                            .flex_none()
                            .text_size(px(11.0))
                            .text_color(theme.faint())
                            .child(
                                [activity.app.clone(), Some(activity.action.clone())]
                                    .into_iter()
                                    .flatten()
                                    .collect::<Vec<_>>()
                                    .join(" · "),
                            )
                    })),
            )
            .children(activity.as_ref().and_then(|activity| {
                activity.image.as_ref().map(|image| {
                    let data_url = image.data_url.clone();
                    let name = activity
                        .image_app
                        .clone()
                        .unwrap_or_else(|| image.name.clone());
                    let preview_image = image.image.clone();
                    div()
                        .id("computer-use-preview")
                        .flex()
                        .items_center()
                        .justify_center()
                        .w_full()
                        .max_h(px(190.0))
                        .overflow_hidden()
                        .rounded(px(8.0))
                        .bg(theme.background())
                        .cursor_pointer()
                        .children(preview_image.map(|image| {
                            gpui::img(image)
                                .w_full()
                                .max_h(px(190.0))
                        }))
                        .on_click(cx.listener(move |root, _, _, cx| {
                            if let Some(image) =
                                crate::model::image_from_data_url(&data_url)
                            {
                                root.overlay = Some(Overlay::ImagePreview {
                                    name: name.clone(),
                                    image,
                                });
                                cx.notify();
                            }
                        }))
                })
            }))
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.faint())
                    .child(
                        activity
                            .as_ref()
                            .map_or(
                                "No screenshot captured yet. Accessibility actions do not always return an image.",
                                |activity| activity.caption(),
                            ),
                    ),
            )
            .children(can_stop.then(|| {
                Button::new("Stop task").danger().render(
                    "computer-use-stop",
                    theme,
                    cx,
                    |root, _, cx| root.stop(cx),
                )
            })),
    );

    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.5))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.text())
                    .child("App access"),
            )
            .child(div().text_size(px(11.0)).text_color(theme.faint()).child(
                if policy.server.is_empty() {
                    "No server".to_string()
                } else {
                    policy.server.clone()
                },
            )),
    );

    if policy.loading {
        panel = panel.child(
            div()
                .p_3()
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child("Loading app access…"),
        );
        return panel;
    }
    if let Some(error) = policy.error {
        panel = panel.child(
            div()
                .p_3()
                .rounded(px(9.0))
                .bg(theme.color("tool_error_bg"))
                .text_size(px(12.0))
                .text_color(theme.error())
                .child(error),
        );
        return panel;
    }

    let default = policy.value["default"]
        .as_str()
        .unwrap_or("allow")
        .to_string();
    panel = panel.child(
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .text_size(px(12.0))
                    .text_color(theme.dim())
                    .child("For apps nobody has decided about"),
            )
            .child(
                Button::new(if default == "deny" {
                    "Block by default"
                } else {
                    "Allow by default"
                })
                .render("computer-use-default", theme, cx, move |root, _, cx| {
                    let mode = if default == "deny" { "allow" } else { "deny" };
                    root.change_computer_use_policy(
                        "set_default".to_string(),
                        None,
                        Some(mode.to_string()),
                        cx,
                    );
                }),
            ),
    );

    let mut apps = policy.value["apps"].as_array().cloned().unwrap_or_default();
    apps.sort_by(|left, right| {
        right["isRunning"]
            .as_bool()
            .unwrap_or(false)
            .cmp(&left["isRunning"].as_bool().unwrap_or(false))
            .then_with(|| {
                app_name(left)
                    .to_ascii_lowercase()
                    .cmp(&app_name(right).to_ascii_lowercase())
            })
    });

    if apps.is_empty() {
        panel = panel.child(
            div()
                .p_3()
                .rounded(px(9.0))
                .bg(theme.panel_2())
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child("No apps found."),
        );
        return panel;
    }

    let mut list = div()
        .id("computer-use-apps")
        .flex()
        .flex_col()
        .max_h(px(220.0))
        .overflow_y_scroll()
        .border_1()
        .border_color(theme.border())
        .rounded(px(9.0));
    for (index, app) in apps.into_iter().enumerate() {
        let id = app["id"].as_str().unwrap_or_default().to_string();
        let name = app_name(&app);
        let status = computer_policy_status(&app, &policy.value);
        let app_id = id.clone();
        list = list.child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
                .min_h(px(48.0))
                .px_2()
                .py_1()
                .border_b_1()
                .border_color(theme.border())
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(2.0))
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(12.0))
                                .text_color(theme.text())
                                .child(name),
                        )
                        .child(
                            div()
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .text_size(px(10.5))
                                .text_color(theme.faint())
                                .child(format!("{status} · {id}")),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .gap_1()
                        .child(Button::new("Allow").render(
                            ("computer-use-allow", index),
                            theme,
                            cx,
                            {
                                let id = app_id.clone();
                                move |root, _, cx| {
                                    root.change_computer_use_policy(
                                        "allow".to_string(),
                                        Some(id.clone()),
                                        None,
                                        cx,
                                    )
                                }
                            },
                        ))
                        .child(Button::new("Once").render(
                            ("computer-use-once", index),
                            theme,
                            cx,
                            {
                                let id = app_id.clone();
                                move |root, _, cx| {
                                    root.change_computer_use_policy(
                                        "grant_once".to_string(),
                                        Some(id.clone()),
                                        None,
                                        cx,
                                    )
                                }
                            },
                        ))
                        .child(Button::new("Block").danger().render(
                            ("computer-use-deny", index),
                            theme,
                            cx,
                            {
                                let id = app_id.clone();
                                move |root, _, cx| {
                                    root.change_computer_use_policy(
                                        "deny".to_string(),
                                        Some(id.clone()),
                                        None,
                                        cx,
                                    )
                                }
                            },
                        ))
                        .child(Button::new("Revoke").render(
                            ("computer-use-revoke", index),
                            theme,
                            cx,
                            move |root, _, cx| {
                                root.change_computer_use_policy(
                                    "revoke".to_string(),
                                    Some(app_id.clone()),
                                    None,
                                    cx,
                                )
                            },
                        )),
                ),
        );
    }
    panel.child(list)
}

fn app_name(app: &Value) -> String {
    app["displayName"]
        .as_str()
        .or_else(|| app["id"].as_str())
        .unwrap_or("App")
        .to_string()
}

fn computer_policy_status(app: &Value, policy: &Value) -> &'static str {
    if app["policy"].as_str() == Some("[approved for one call]") {
        return "Allowed once";
    }
    if app["policy"].as_str() == Some("[blocked by app policy]") {
        return "Blocked";
    }
    let name = app["displayName"].as_str().unwrap_or_default();
    let id = app["id"].as_str().unwrap_or_default();
    let named = |key: &str| {
        policy[key].as_array().is_some_and(|names| {
            names
                .iter()
                .filter_map(Value::as_str)
                .any(|value| value.eq_ignore_ascii_case(name) || value.eq_ignore_ascii_case(id))
        })
    };
    if named("denied") {
        "Blocked"
    } else if named("allowed") {
        "Always allowed"
    } else if app["isRunning"].as_bool().unwrap_or(false) {
        "Running"
    } else {
        "Not running"
    }
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
                    ConfirmAction::RemoveWorktree { project, name } => {
                        root.remove_worktree(project, name, cx)
                    }
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

fn connect(root: &mut Root, theme: &Theme, provider: String, cx: &mut Context<Root>) -> gpui::Div {
    let query = root.provider_filter.read(cx).text().to_ascii_lowercase();
    let provider_filter_focus = root.provider_filter.clone();
    let provider_filter_value = root.provider_filter.clone();
    let providers = root.providers.clone();
    let selected = if providers
        .iter()
        .any(|row| row["name"].as_str() == Some(provider.as_str()))
    {
        provider.clone()
    } else {
        providers
            .iter()
            .find(|row| row["active"].as_bool().unwrap_or(false))
            .or_else(|| {
                providers
                    .iter()
                    .find(|row| row["stored"].as_bool().unwrap_or(false))
            })
            .or_else(|| providers.first())
            .and_then(|row| row["name"].as_str())
            .unwrap_or_default()
            .to_string()
    };
    if selected != provider {
        root.overlay = Some(Overlay::Connect {
            provider: selected.clone(),
        });
    }

    let selected_row = providers
        .iter()
        .find(|row| row["name"].as_str() == Some(selected.as_str()))
        .cloned();
    let keyless = selected_row.as_ref().is_some_and(|row| {
        row["local"].as_bool().unwrap_or(false) || row["credential"].as_str() == Some("external")
    });
    let stored = selected_row
        .as_ref()
        .is_some_and(|row| row["stored"].as_bool().unwrap_or(false));
    let provider_label = selected_row
        .as_ref()
        .and_then(|row| row["label"].as_str())
        .unwrap_or("a provider")
        .to_string();
    let selected_for_action = selected.clone();
    let mut rows = div()
        .id("provider-list")
        .role(gpui::Role::ListBox)
        .aria_label("Providers")
        .flex()
        .flex_col()
        .gap_1()
        .max_h(px(270.0))
        .overflow_y_scroll();
    let matching = providers
        .into_iter()
        .filter(|row| {
            query.is_empty()
                || format!(
                    "{} {} {}",
                    row["name"].as_str().unwrap_or_default(),
                    row["label"].as_str().unwrap_or_default(),
                    row["description"].as_str().unwrap_or_default()
                )
                .to_ascii_lowercase()
                .contains(&query)
        })
        .collect::<Vec<_>>();
    if matching.is_empty() {
        rows = rows.child(
            div()
                .p_3()
                .text_size(px(12.0))
                .text_color(theme.faint())
                .child("No provider matches that search."),
        );
    }
    for row in matching {
        let name = row["name"].as_str().unwrap_or_default().to_string();
        let label = row["label"].as_str().unwrap_or(&name).to_string();
        let description = row["description"].as_str().unwrap_or_default().to_string();
        let active = row["active"].as_bool().unwrap_or(false);
        let saved = row["stored"].as_bool().unwrap_or(false);
        let is_selected = name == selected;
        let clear_name = name.clone();
        rows = rows.child(
            div()
                .id(SharedString::from(format!("provider-{name}")))
                .role(gpui::Role::RadioButton)
                .aria_label(label.clone())
                .aria_description(format!("{name} · {description}"))
                .aria_selected(is_selected)
                .flex()
                .flex_col()
                .gap_1()
                .p_2()
                .rounded(px(7.0))
                .cursor_pointer()
                .bg(if is_selected {
                    theme.panel_3()
                } else {
                    theme.background()
                })
                .hover(move |style| style.bg(theme.panel_2()))
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(12.5))
                                .font_weight(FontWeight::MEDIUM)
                                .child(label),
                        )
                        .children(active.then(|| pill("In use", theme, false)))
                        .children(saved.then(|| pill("Stored", theme, false))),
                )
                .child(
                    div()
                        .text_size(px(10.5))
                        .text_color(theme.faint())
                        .whitespace_normal()
                        .child(format!("{name} · {description}")),
                )
                .on_click(cx.listener(move |root, _, _, cx| {
                    root.provider_key.update(cx, |editor, cx| editor.clear(cx));
                    root.overlay = Some(Overlay::Connect {
                        provider: clear_name.clone(),
                    });
                    cx.notify();
                })),
        );
    }

    let mut fields = div().flex().flex_col().gap_2();
    if keyless {
        let note = if selected_row
            .as_ref()
            .is_some_and(|row| row["local"].as_bool().unwrap_or(false))
        {
            format!("{provider_label} is a server on this machine — no key needed.")
        } else {
            format!(
                "{provider_label} authorizes with a credential this machine already has — no key needed."
            )
        };
        fields = fields.child(
            div()
                .text_size(px(11.5))
                .text_color(theme.dim())
                .child(note),
        );
    } else {
        fields = fields
            .child(
                div()
                    .text_size(px(11.0))
                    .text_color(theme.faint())
                    .child(if stored {
                        "Leave the key empty to reuse the stored credential."
                    } else {
                        "Paste the provider's API key."
                    }),
            )
            .child(input_box(
                "provider-key",
                "API key",
                "Paste your API key",
                gpui::Role::PasswordInput,
                root.provider_key.clone(),
                theme,
                cx,
            ));
    }
    fields = fields
        .child(input_box(
            "provider-model",
            "Model (optional)",
            "Model (optional)",
            gpui::Role::TextInput,
            root.provider_model.clone(),
            theme,
            cx,
        ))
        .child(input_box(
            "provider-url",
            "Base URL (optional)",
            "Base URL (optional)",
            gpui::Role::UrlInput,
            root.provider_url.clone(),
            theme,
            cx,
        ));

    dialog_with_close(theme, "Connect a provider", 540.0, cx)
        .child(
            div()
                .id("provider-search")
                .role(gpui::Role::SearchInput)
                .aria_label("Search providers")
                .aria_value(query.clone())
                .on_a11y_action(gpui::AccessibleAction::Focus, move |_, window, cx| {
                    window.focus(&provider_filter_focus.read(cx).focus_handle(cx), cx);
                })
                .on_a11y_action(gpui::AccessibleAction::SetValue, move |data, _, cx| {
                    if let Some(gpui::accesskit::ActionData::Value(value)) = data {
                        provider_filter_value
                            .update(cx, |editor, cx| editor.set_text(value.to_string(), cx));
                    }
                })
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|root, _, window, cx| {
                        window.focus(&root.provider_filter.read(cx).focus_handle(cx), cx);
                    }),
                )
                .h(px(36.0))
                .px_2()
                .flex()
                .flex_row()
                .items_center()
                .gap_2()
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
                        .child(root.provider_filter.clone()),
                ),
        )
        .child(rows)
        .child(rule(theme))
        .child(
            div()
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .child(format!("Connect {provider_label}")),
        )
        .child(fields)
        .child(
            div()
                .flex()
                .flex_row()
                .justify_end()
                .gap_2()
                .child(
                    Button::new("Cancel").render("connect-cancel", theme, cx, |root, _, cx| {
                        root.overlay = None;
                        cx.notify();
                    }),
                )
                .child(Button::new("Connect").primary().render(
                    "connect-submit",
                    theme,
                    cx,
                    move |root, _, cx| {
                        if selected_for_action.is_empty() {
                            root.toast("Choose a provider first.", true, cx);
                        } else {
                            root.login_provider(selected_for_action.clone(), cx);
                        }
                    },
                )),
        )
}

// ---------- new project ----------

fn new_project(root: &mut Root, theme: &Theme, cx: &mut Context<Root>) -> gpui::Div {
    let folders = root.new_project_folders.clone();
    let project_name_focus = root.project_name.clone();
    let project_name_value = root.project_name.clone();
    let mut panel = div()
        .flex()
        .flex_col()
        .w(px(520.0))
        .p(px(20.0))
        .rounded(px(20.0))
        .bg(theme.panel_2())
        .shadow_xl()
        .child(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap_3()
                .mb(px(16.0))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(20.0))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text())
                        .child("Create project"),
                )
                .child(
                    icon_button("new-project-close", Icon::Close, 24.0, theme, false)
                        .tooltip(crate::views::tip("Close", theme))
                        .on_click(cx.listener(|root, _, _, cx| {
                            root.overlay = None;
                            cx.notify();
                        })),
                ),
        )
        .child(
            div()
                .id("new-project-name")
                .role(gpui::Role::TextInput)
                .aria_label("Project name")
                .aria_value(root.project_name.read(cx).text().to_string())
                .on_a11y_action(gpui::AccessibleAction::Focus, move |_, window, cx| {
                    window.focus(&project_name_focus.read(cx).focus_handle(cx), cx);
                })
                .on_a11y_action(gpui::AccessibleAction::SetValue, move |data, _, cx| {
                    if let Some(gpui::accesskit::ActionData::Value(value)) = data {
                        project_name_value
                            .update(cx, |editor, cx| editor.set_text(value.to_string(), cx));
                    }
                })
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(|root, _, window, cx| {
                        window.focus(&root.project_name.read(cx).focus_handle(cx), cx);
                    }),
                )
                .flex()
                .flex_row()
                .items_center()
                .h(px(40.0))
                .mb(px(14.0))
                .border_1()
                .border_color(theme.accent())
                .rounded(px(12.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .flex_none()
                        .w(px(40.0))
                        .h_full()
                        .border_r_1()
                        .border_color(theme.border())
                        .child(icons::icon(Icon::Folder, 18.0, theme.dim())),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .h_full()
                        .px(px(10.0))
                        .child(root.project_name.clone()),
                ),
        )
        .child(
            div()
                .mb(px(8.0))
                .text_size(px(13.0))
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme.text())
                .child("Source folders"),
        );

    if folders.is_empty() {
        panel = panel.child(
            div()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.0))
                .min_h(px(104.0))
                .p(px(16.0))
                .rounded(px(12.0))
                .border_1()
                .border_color(theme.border())
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(6.0))
                        .text_size(px(13.0))
                        .text_color(theme.dim())
                        .child("Add a folder on this computer")
                        .child(icons::icon(Icon::ChevronDown, 12.0, theme.dim())),
                )
                .child(
                    div()
                        .id("new-project-pick")
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap(px(5.0))
                        .h(px(28.0))
                        .px(px(10.0))
                        .rounded(px(20.0))
                        .bg(theme.panel_3())
                        .cursor_pointer()
                        .text_size(px(13.0))
                        .text_color(theme.text())
                        .hover(move |style| style.bg(theme.panel()))
                        .child(icons::icon(Icon::Plus, 14.0, theme.dim()))
                        .child("Add")
                        .on_click(cx.listener(|root, _, _, cx| root.add_project(cx))),
                ),
        );
    } else {
        let mut rows = div().flex().flex_col().gap(px(4.0));
        for (index, folder) in folders.into_iter().enumerate() {
            let remove_folder = folder.clone();
            rows = rows.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap_2()
                    .h(px(36.0))
                    .px(px(10.0))
                    .rounded(px(8.0))
                    .bg(theme.panel_2())
                    .child(icons::icon(Icon::Folder, 14.0, theme.dim()))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .font_family("Menlo")
                            .text_size(px(11.0))
                            .text_color(theme.dim())
                            .child(folder),
                    )
                    .child(
                        icon_button(
                            SharedString::from(format!("new-project-remove-{index}")),
                            Icon::Close,
                            24.0,
                            theme,
                            false,
                        )
                        .tooltip(crate::views::tip("Remove source folder", theme))
                        .on_click(cx.listener(move |root, _, _, cx| {
                            root.new_project_folders
                                .retain(|path| path != &remove_folder);
                            cx.notify();
                        })),
                    ),
            );
        }
        panel = panel.child(
            div()
                .flex()
                .flex_col()
                .gap(px(10.0))
                .min_h(px(104.0))
                .p(px(16.0))
                .rounded(px(12.0))
                .border_1()
                .border_color(theme.border())
                .child(rows)
                .child(
                    div()
                        .id("new-project-pick")
                        .flex()
                        .flex_row()
                        .items_center()
                        .justify_center()
                        .gap(px(5.0))
                        .h(px(28.0))
                        .px(px(10.0))
                        .rounded(px(20.0))
                        .bg(theme.panel_3())
                        .cursor_pointer()
                        .text_size(px(13.0))
                        .text_color(theme.text())
                        .hover(move |style| style.bg(theme.panel()))
                        .child(icons::icon(Icon::Plus, 14.0, theme.dim()))
                        .child("Add")
                        .on_click(cx.listener(|root, _, _, cx| root.add_project(cx))),
                ),
        );
    }

    panel.child(
        div()
            .flex()
            .flex_row()
            .justify_end()
            .gap(px(16.0))
            .mt(px(32.0))
            .child(
                Button::new("Cancel")
                    .render("new-project-cancel", theme, cx, |root, _, cx| {
                        root.overlay = None;
                        cx.notify();
                    })
                    .h(px(34.0))
                    .px(px(18.0))
                    .rounded(px(10.0))
                    .bg(gpui::transparent_black())
                    .text_size(px(13.5))
                    .text_color(theme.dim())
                    .hover(move |style| style.bg(theme.panel()).text_color(theme.text())),
            )
            .child(
                Button::new("Create project")
                    .primary()
                    .render("new-project-create", theme, cx, |root, _, cx| {
                        root.create_project(cx)
                    })
                    .h(px(34.0))
                    .px(px(18.0))
                    .rounded(px(10.0))
                    .text_size(px(13.5)),
            ),
    )
}

#[cfg(test)]
mod tests {
    use super::computer_policy_status;
    use serde_json::json;

    #[test]
    fn computer_use_settings_name_the_same_decisions_as_the_runtime() {
        let app = json!({
            "id": "com.microsoft.edgemac",
            "displayName": "Microsoft Edge",
            "isRunning": true
        });
        let once = json!({ "oneTime": "Microsoft Edge" });
        assert_eq!(
            computer_policy_status(
                &json!({ "id": "com.microsoft.edgemac", "policy": "[approved for one call]" }),
                &once
            ),
            "Allowed once"
        );
        assert_eq!(
            computer_policy_status(&app, &json!({ "allowed": ["Microsoft Edge"] })),
            "Always allowed"
        );
        assert_eq!(
            computer_policy_status(&app, &json!({ "denied": ["com.microsoft.edgemac"] })),
            "Blocked"
        );
        assert_eq!(computer_policy_status(&app, &json!({})), "Running");
    }
}
