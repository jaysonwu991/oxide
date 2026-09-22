use crate::config::Reasoning;
use crate::tools::DiffPreview;
use crate::tui::app::{
    App, ChatItem, ConnectState, ConnectStep, ListRow, MarketplacePane, Selection, SubagentState,
    Tone, UsageField,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Frame;

const MIN_INPUT_ROWS: usize = 1;
const MAX_INPUT_ROWS: usize = 12;
/// Blank rows kept above the conversation so the first line (banner or chat)
/// is not flush with the terminal's top edge.
const MESSAGE_TOP_PAD: u16 = 1;
const MAX_MODEL_ROWS: usize = 10;
const MAX_SESSION_ROWS: usize = 12;
const MAX_SUGGESTION_ROWS: usize = 8;
/// Description columns kept next to a command name so a long name never eats
/// the whole suggestion line.
const MIN_SUGGESTION_DESCRIPTION: usize = 12;

const FILE_TOOLS: [&str; 4] = ["read_file", "write_file", "patch", "edit"];

// Crossterm maps unsuffixed ANSI colors to dark variants, so accents use the
// light variants to remain readable on common dark terminal backgrounds.

/// Block-letter wordmark shown on the welcome screen.
const BANNER: [&str; 6] = [
    " ██████╗  ██╗  ██╗ ██╗ ██████╗  ███████╗",
    "██╔═══██╗ ╚██╗██╔╝ ██║ ██╔══██╗ ██╔════╝",
    "██║   ██║  ╚███╔╝  ██║ ██║  ██║ █████╗  ",
    "██║   ██║  ██╔██╗  ██║ ██║  ██║ ██╔══╝  ",
    "╚██████╔╝ ██╔╝ ██╗ ██║ ██████╔╝ ███████╗",
    " ╚═════╝  ╚═╝  ╚═╝ ╚═╝ ╚═════╝  ╚══════╝",
];

fn is_file_tool(name: &str) -> bool {
    FILE_TOOLS.contains(&crate::tools::canonical_tool_name(name))
}

/// A rounded panel with a colored border and title, shared by the input and
/// popup surfaces. An empty title leaves the top border unbroken.
fn panel(title: &str, color: Color) -> Block<'static> {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color));
    if title.is_empty() {
        block
    } else {
        block.title(Span::styled(
            format!(" {title} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
    }
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let areas = main_areas(frame.area(), app);

    draw_messages(frame, app, areas[0]);
    draw_input(frame, app, areas[1]);
    draw_footer(frame, app, areas[2]);
    if let Some(area) = areas.get(3) {
        draw_usage_bar(frame, app, *area);
    }

    if app.connect.is_some() {
        draw_connect(frame, app);
    } else if app.trust.is_some() {
        draw_trust(frame, app);
    } else if app.models.is_some() {
        draw_models(frame, app);
    } else if app.sessions.is_some() {
        draw_sessions(frame, app);
    } else if app.marketplaces.is_some() {
        draw_marketplaces(frame, app);
    } else if app.usage_modal.is_some() {
        draw_usage(frame, app);
    } else if !app.suggestions.is_empty() {
        draw_suggestions(frame, app, areas[0]);
    }
}

/// Messages, the composer, the footer, and - when the Portkey spend bar is
/// enabled - one full-width row for it at the bottom of the screen.
fn main_areas(area: Rect, app: &App) -> Vec<Rect> {
    let input_width = area.width as usize;
    let input_rows = input_rows(&app.input, input_width) as u16;
    let mut constraints = vec![
        Constraint::Min(3),
        // One gap row above the composer, the top and bottom rules, then
        // the wrapped input rows.
        Constraint::Length(input_rows + 3),
        Constraint::Length(if app.extension_statuses.is_empty() {
            2
        } else {
            3
        }),
    ];
    if app.usage.is_some() {
        constraints.push(Constraint::Length(1));
    }
    Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area)
        .to_vec()
}

fn draw_trust(frame: &mut Frame, app: &App) {
    let Some(state) = &app.trust else {
        return;
    };
    let area = centered_rect(74, 46, frame.area());
    frame.render_widget(Clear, area);

    let mut lines = vec![Line::from(Span::styled(
        format!(" Trust project {}?", state.dir),
        Style::default()
            .fg(app.theme.tool)
            .add_modifier(Modifier::BOLD),
    ))];
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "This project contains local resources the agent will load and, for",
        Style::default().fg(app.theme.info),
    )));
    lines.push(Line::from(Span::styled(
        "plugins, execute. Only trust repositories you have reviewed.",
        Style::default().fg(app.theme.info),
    )));
    lines.push(Line::from(""));
    for resource in &state.resources {
        lines.push(Line::from(Span::styled(
            format!("   • {resource}"),
            Style::default().fg(app.theme.assistant),
        )));
    }
    lines.push(Line::from(""));
    let options = ["Trust and load resources", "Do not load resources"];
    for (index, label) in options.iter().enumerate() {
        let selected = state.selected == index;
        let style = if selected {
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.assistant)
        };
        lines.push(Line::from(Span::styled(
            format!(" {} {label}", if selected { "›" } else { " " }),
            style,
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "←/→ choose · Enter confirm · Esc decline for this session",
        Style::default().fg(app.theme.info),
    )));

    frame.render_widget(
        Paragraph::new(lines).block(panel("project trust", app.theme.tool)),
        area,
    );
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn draw_connect(frame: &mut Frame, app: &App) {
    let Some(state) = &app.connect else {
        return;
    };
    let area = centered_rect(72, 54, frame.area());
    frame.render_widget(Clear, area);

    let (prompt, value) = match &state.step {
        ConnectStep::Provider => (
            "Choose a provider, or type a custom provider name",
            state.input.clone(),
        ),
        ConnectStep::Key { provider } => (
            crate::auth::provider_option(provider)
                .map(|option| option.key_url)
                .unwrap_or("Paste the API key for this provider"),
            "*".repeat(state.input.chars().count()),
        ),
        ConnectStep::Options { .. } => (
            "Optional settings — blank keeps the provider default",
            state.input.clone(),
        ),
    };
    let title = match &state.step {
        ConnectStep::Provider => " connect ",
        ConnectStep::Key { provider } | ConnectStep::Options { provider } => provider.as_str(),
    };
    let connected = match &state.step {
        ConnectStep::Key { provider } => state.is_connected(provider),
        _ => false,
    };

    // Build the panel up front so the input can be truncated to one line and
    // the cursor placed at its end without wrapping off the panel.
    let block = panel(title, app.theme.accent);
    let inner = block.inner(area);
    let shown = truncate(&value, inner.width.saturating_sub(2) as usize);

    let mut lines = vec![Line::from(Span::styled(
        prompt,
        Style::default().fg(app.theme.info),
    ))];
    let mut cursor: Option<(usize, u16)> = None;
    match &state.step {
        ConnectStep::Provider => {
            lines.push(Line::from(""));
            for (index, option) in crate::auth::KNOWN_PROVIDERS.iter().enumerate() {
                let selected = state.input.is_empty() && state.selected == index;
                let marker = if selected { "›" } else { " " };
                let style = if selected {
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(app.theme.assistant)
                };
                let mut spans = vec![
                    Span::styled(format!(" {marker} {:<10}", option.label), style),
                    Span::styled(option.description, Style::default().fg(app.theme.info)),
                ];
                if state.is_connected(option.name) {
                    spans.push(Span::styled(
                        " · connected",
                        Style::default().fg(app.theme.success),
                    ));
                }
                lines.push(Line::from(spans));
            }
            lines.push(Line::from(""));
            if let Some(error) = &state.error {
                lines.push(Line::from(Span::styled(
                    format!("error: {error}"),
                    Style::default().fg(app.theme.error),
                )));
                lines.push(Line::from(""));
            }
            let input_line = lines.len();
            lines.push(Line::from(vec![
                Span::styled("> ", Style::default().fg(app.theme.accent)),
                Span::styled(shown.clone(), Style::default().fg(app.theme.assistant)),
            ]));
            cursor = Some((input_line, 2 + shown.chars().count() as u16));
        }
        ConnectStep::Key { .. } => {
            lines.push(Line::from(""));
            if connected {
                lines.push(Line::from(Span::styled(
                    "This provider is connected — Enter reuses the stored key.",
                    Style::default().fg(app.theme.success),
                )));
                lines.push(Line::from(""));
            }
            if let Some(error) = &state.error {
                lines.push(Line::from(Span::styled(
                    format!("error: {error}"),
                    Style::default().fg(app.theme.error),
                )));
                lines.push(Line::from(""));
            }
            let input_line = lines.len();
            lines.push(Line::from(vec![
                Span::styled("> ", Style::default().fg(app.theme.accent)),
                Span::styled(shown.clone(), Style::default().fg(app.theme.assistant)),
            ]));
            cursor = Some((input_line, 2 + shown.chars().count() as u16));
        }
        ConnectStep::Options { provider } => {
            lines.push(Line::from(""));
            let value_width = inner.width.saturating_sub(18) as usize;
            for field in ConnectState::option_fields(provider) {
                let focused = field == state.focus;
                let marker = if focused { "›" } else { " " };
                let style = if focused {
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(app.theme.assistant)
                };
                let row_value = if focused {
                    state.input.clone()
                } else {
                    state.value_for(field)
                };
                let display = if row_value.is_empty() {
                    "—".to_string()
                } else {
                    truncate(&row_value, value_width)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!(" {marker} {:<14}", field.label()), style),
                    Span::styled(display, Style::default().fg(app.theme.info)),
                ]));
                if focused {
                    let column = 17 + state.input.chars().count().min(value_width) as u16;
                    cursor = Some((lines.len() - 1, column));
                }
            }
            lines.push(Line::from(""));
            if let Some(error) = &state.error {
                lines.push(Line::from(Span::styled(
                    format!("error: {error}"),
                    Style::default().fg(app.theme.error),
                )));
                lines.push(Line::from(""));
            }
        }
    }
    lines.push(Line::from(Span::styled(
        match (state.step.clone(), connected) {
            (ConnectStep::Provider, _) => "↑/↓ choose · Enter continue · Esc cancel",
            (ConnectStep::Key { .. }, true) => {
                "Enter use stored key · type to replace it · Backspace back · Esc cancel"
            }
            (ConnectStep::Key { .. }, false) => "Enter connect · Backspace back · Esc cancel",
            (ConnectStep::Options { .. }, _) => {
                "↑/↓ field · Enter next/save · Backspace back · Esc cancel"
            }
        },
        Style::default().fg(app.theme.info),
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
    if let Some((line, column)) = cursor {
        if inner.width > 0 && inner.height > line as u16 {
            frame.set_cursor_position((
                (inner.x + column).min(inner.x + inner.width.saturating_sub(1)),
                inner.y + line as u16,
            ));
        }
    }
}

/// The `/usage` settings dialog: a labeled form over the Portkey spend-bar
/// settings. The selected row is edited in place and the terminal cursor is
/// placed at the end of its value, including the masked API key.
fn draw_usage(frame: &mut Frame, app: &App) {
    let Some(state) = &app.usage_modal else {
        return;
    };
    let area = centered_rect(72, 56, frame.area());
    frame.render_widget(Clear, area);

    let block = panel(" portkey usage ", app.theme.accent);
    let inner = block.inner(area);
    let value_width = inner.width.saturating_sub(13) as usize;

    let mut lines = vec![
        Line::from(Span::styled(
            "Portkey spend bar",
            Style::default().fg(app.theme.info),
        )),
        Line::from(""),
    ];
    for (index, field) in UsageField::ALL.iter().enumerate() {
        let selected = index == state.selected;
        let marker = if selected { "›" } else { " " };
        let style = if selected {
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(app.theme.assistant)
        };
        let value = if selected && state.editing {
            usage_edit_value(&state.input, *field)
        } else {
            usage_display(&state.settings, *field)
        };
        let value_style = if selected {
            Style::default().fg(app.theme.assistant)
        } else {
            Style::default().fg(app.theme.info)
        };
        lines.push(Line::from(vec![
            Span::styled(format!(" {marker} {:<10}", field.label()), style),
            Span::styled(truncate(&value, value_width), value_style),
        ]));
    }
    lines.push(Line::from(""));
    if let Some(error) = &state.error {
        lines.push(Line::from(Span::styled(
            format!("error: {error}"),
            Style::default().fg(app.theme.error),
        )));
        lines.push(Line::from(""));
    }
    let hint = if state.editing {
        if state.field() == UsageField::ApiKey {
            "Enter apply · Esc cancel · empty uses the provider key"
        } else {
            "Enter apply · Esc cancel"
        }
    } else if state.field().is_toggle() {
        "↑/↓ move · Enter toggle · Esc save & close"
    } else {
        "↑/↓ move · Enter edit · Esc save & close"
    };
    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(app.theme.info),
    )));

    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
    if state.editing && inner.width > 0 {
        let value = truncate(&usage_edit_value(&state.input, state.field()), value_width);
        let column = 13 + value.chars().count() as u16;
        frame.set_cursor_position((
            (inner.x + column).min(inner.x + inner.width.saturating_sub(1)),
            inner.y + 2 + state.selected as u16,
        ));
    }
}

/// The value an editing row shows: the API key is masked like a password.
fn usage_edit_value(input: &str, field: UsageField) -> String {
    match field {
        UsageField::ApiKey => "*".repeat(input.chars().count()),
        _ => input.to_string(),
    }
}

/// The current value shown for a usage row when it is not being edited.
fn usage_display(settings: &crate::portkey_usage::UsageSettings, field: UsageField) -> String {
    match field {
        UsageField::Enabled => if settings.enabled { "on" } else { "off" }.to_string(),
        UsageField::User => {
            if settings.user.trim().is_empty() {
                "(unset — firstname.lastname)".to_string()
            } else {
                settings.user.clone()
            }
        }
        UsageField::Metadata => settings.metadata_key.clone(),
        UsageField::Budget => settings
            .budget
            .map(|budget| settings.currency.format(budget))
            .unwrap_or_else(|| "none".to_string()),
        UsageField::Currency => settings.currency.name().to_string(),
        UsageField::ApiKey => {
            if settings.api_key.trim().is_empty() {
                "(provider key)".to_string()
            } else {
                super::mask(&settings.api_key)
            }
        }
        UsageField::Endpoint => settings.base_url.clone(),
    }
}

/// Pi-style model picker: a provider hint, a search input, the model list with
/// current/default markers, the selected model's name, and key hints.
fn draw_models(frame: &mut Frame, app: &App) {
    const POST_LIST_ROWS: usize = 7;

    let Some(state) = &app.models else {
        return;
    };
    let area = centered_rect(82, 84, frame.area());
    frame.render_widget(Clear, area);

    let block = panel("", app.theme.accent);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let accent = Style::default().fg(app.theme.accent);
    let dim = Style::default().fg(app.theme.dim);
    let error = Style::default().fg(app.theme.error);
    let mut lines: Vec<Line> = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Models from every logged-in provider. Use /login to add more.",
            Style::default().fg(app.theme.tool),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", accent),
            Span::raw(state.filter.clone()),
        ]),
        Line::from(""),
    ];

    let cursor_x = inner.x + 2 + state.filter.chars().count() as u16;
    frame.set_cursor_position((
        cursor_x.min(inner.x + inner.width.saturating_sub(1)),
        inner.y + 3,
    ));

    let models = state.filtered();
    let selected = state.selected.min(models.len().saturating_sub(1));
    let capacity = (inner.height as usize)
        .saturating_sub(lines.len() + POST_LIST_ROWS)
        .max(1);

    if let Some(message) = &state.error {
        lines.push(Line::from(Span::styled(format!("  {message}"), error)));
    } else if !state.loading && models.is_empty() {
        lines.push(Line::from(Span::styled("  No matching models", dim)));
    } else if !models.is_empty() {
        let visible = models.len().min(MAX_MODEL_ROWS).min(capacity);
        let start = selected
            .saturating_sub(visible / 2)
            .min(models.len().saturating_sub(visible));
        let end = start + visible;
        for (index, choice) in models[start..end].iter().enumerate() {
            let absolute = start + index;
            let is_selected = absolute == selected;
            let is_current = choice.model == state.current && choice.provider == state.provider;
            let is_default = state.default.as_deref() == Some(choice.model.as_str());
            let mut spans = vec![
                Span::styled(if is_selected { "→ " } else { "  " }, accent),
                Span::styled(if is_current { "✓ " } else { "  " }, accent),
                Span::styled(
                    choice.model.clone(),
                    if is_selected {
                        accent
                    } else {
                        Style::default().fg(app.theme.assistant)
                    },
                ),
                Span::styled(format!(" [{}]", choice.provider), dim),
            ];
            if is_default {
                spans.push(Span::styled(" · default", dim));
            }
            lines.push(Line::from(spans));
        }
        if start > 0 || end < models.len() {
            lines.push(Line::from(Span::styled(
                format!("  ({}/{})", selected + 1, models.len()),
                dim,
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            format!(
                "  Model Name: {}",
                crate::config::model_label(&models[selected].model)
            ),
            dim,
        )));
    }

    lines.push(Line::from(""));
    if state.loading {
        lines.push(Line::from(Span::styled(
            "  Refreshing model catalogs…",
            dim,
        )));
    } else if state.refreshed {
        lines.push(Line::from(Span::styled(
            "  Model catalogs refreshed.",
            Style::default().fg(app.theme.success),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Enter to select · Ctrl+S to set as default · Escape/Ctrl+C to cancel",
        dim,
    )));
    lines.push(Line::from(""));

    frame.render_widget(Paragraph::new(Text::from(lines)), inner);
}

fn draw_sessions(frame: &mut Frame, app: &App) {
    let Some(state) = &app.sessions else {
        return;
    };
    let area = centered_rect(78, 66, frame.area());
    frame.render_widget(Clear, area);

    let title = if state.renaming {
        " rename session ".to_string()
    } else if state.confirm_delete {
        " delete session? ".to_string()
    } else if state.filter.is_empty() {
        " sessions ".to_string()
    } else {
        format!(" sessions · {} ", state.filter)
    };
    let block = panel(&title, app.theme.accent);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.renaming {
        let mut lines = vec![Line::from(Span::styled(
            "Rename the selected session",
            Style::default().fg(app.theme.info),
        ))];
        if let Some(summary) = state.selected_session() {
            let label = summary
                .name
                .clone()
                .unwrap_or_else(|| summary.preview.clone());
            lines.push(Line::from(Span::styled(
                format!("session: {label}"),
                Style::default().fg(app.theme.assistant),
            )));
        }
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::styled("> ", Style::default().fg(app.theme.accent)),
            Span::styled(
                state.rename_input.clone(),
                Style::default().fg(app.theme.assistant),
            ),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Enter save · Esc cancel",
            Style::default().fg(app.theme.info),
        )));
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    }

    if state.confirm_delete {
        let label = state
            .selected_session()
            .map(|summary| {
                summary
                    .name
                    .clone()
                    .unwrap_or_else(|| summary.preview.clone())
            })
            .unwrap_or_default();
        let lines = vec![
            Line::from(Span::styled(
                format!("Delete session `{label}`?"),
                Style::default()
                    .fg(app.theme.error)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "The session file will be moved to trash when available.",
                Style::default().fg(app.theme.info),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Enter/y confirm · Esc/n cancel",
                Style::default().fg(app.theme.info),
            )),
        ];
        frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
        return;
    }

    if let Some(error) = &state.error {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("error: {error}"),
                Style::default().fg(app.theme.error),
            )),
            inner,
        );
        return;
    }

    let sessions = state.filtered();
    if sessions.is_empty() {
        let message = if state.all.is_empty() {
            "no sessions for this project yet"
        } else {
            "no matching sessions"
        };
        frame.render_widget(
            Paragraph::new(Span::styled(message, Style::default().fg(app.theme.info))),
            inner,
        );
        return;
    }

    let rows = inner.height.saturating_sub(1) as usize;
    let visible = sessions.len().min(MAX_SESSION_ROWS).min(rows.max(1));
    let offset = state
        .selected
        .saturating_sub(visible.saturating_sub(1))
        .min(sessions.len().saturating_sub(visible));
    let now = now_secs();
    let items: Vec<ListItem> = sessions[offset..offset + visible]
        .iter()
        .map(|summary| {
            let name = summary
                .name
                .clone()
                .unwrap_or_else(|| summary.preview.clone());
            let mut meta = format!(
                "{} msg{} · {}",
                summary.message_count,
                if summary.message_count == 1 { "" } else { "s" },
                relative_time(now, summary.modified_at)
            );
            if state.show_paths {
                meta.push_str(&format!(" · {}", summary.cwd));
            } else {
                meta.push_str(&format!(" · {}", &summary.id[..summary.id.len().min(8)]));
            }
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {name} "),
                    Style::default().fg(app.theme.assistant),
                ),
                Span::styled(meta, Style::default().fg(app.theme.info)),
            ]))
        })
        .collect();
    let list = List::new(items)
        .highlight_style(
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .highlight_symbol("> ");
    let mut list_state = ListState::default();
    list_state.select(Some(state.selected.saturating_sub(offset)));
    frame.render_stateful_widget(list, inner, &mut list_state);
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn relative_time(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86400)
    }
}

/// Pads `spans` to `width` and paints the whole row with `bg`, so a selected
/// list row reads as one solid band instead of stopping at its last glyph.
fn filled_line(spans: Vec<Span<'static>>, width: usize, bg: Option<Color>) -> Line<'static> {
    let Some(bg) = bg else {
        return Line::from(spans);
    };
    let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    let pad = width.saturating_sub(used);
    let mut spans = spans;
    if pad > 0 {
        spans.push(Span::styled(" ".repeat(pad), Style::default().bg(bg)));
    }
    Line::from(spans).style(Style::default().bg(bg))
}

/// The interactive marketplace browser behind `/marketplaces`.
fn draw_marketplaces(frame: &mut Frame, app: &App) {
    let Some(state) = &app.marketplaces else {
        return;
    };
    let theme = &app.theme;
    let area = centered_rect(90, 88, frame.area());
    frame.render_widget(Clear, area);

    let title = if state.adding {
        " add marketplace ".to_string()
    } else if state.confirm_remove {
        " remove marketplace? ".to_string()
    } else {
        let installed = state.installed_count();
        if installed > 0 {
            format!(
                " marketplaces · {} · {installed} installed ",
                state.all.len()
            )
        } else {
            format!(" marketplaces · {} ", state.all.len())
        }
    };
    let block = panel(&title, theme.accent);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 || inner.width == 0 {
        return;
    }

    if state.adding {
        draw_marketplace_add(frame, state, inner, theme);
        return;
    }
    if state.confirm_remove {
        draw_marketplace_remove(frame, state, inner, theme);
        return;
    }

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Min(3),
            Constraint::Length(3),
        ])
        .split(inner);

    draw_marketplace_header(frame, state, rows[0], theme);

    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(38),
            Constraint::Length(1),
            Constraint::Min(24),
        ])
        .split(rows[1]);

    frame.render_widget(
        Block::default()
            .borders(Borders::LEFT)
            .border_style(Style::default().fg(theme.border)),
        columns[1],
    );

    let detail = Rect {
        x: columns[2].x.saturating_add(1),
        width: columns[2].width.saturating_sub(1),
        ..columns[2]
    };
    draw_marketplace_list(frame, state, columns[0], theme);
    draw_marketplace_detail(frame, state, detail, theme);
    draw_marketplace_footer(frame, state, rows[2], theme);
}

fn draw_marketplace_header(
    frame: &mut Frame,
    state: &crate::tui::app::MarketplacesState,
    area: Rect,
    theme: &crate::theme::Theme,
) {
    let dim = Style::default().fg(theme.dim);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Marketplaces publish plugins from a git repository or local path.",
            Style::default().fg(theme.info),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(state.filter.clone(), Style::default().fg(theme.assistant)),
            if state.filter.is_empty() {
                Span::styled("filter marketplaces or plugins…", dim)
            } else {
                Span::raw("")
            },
        ]),
        Line::from(""),
    ];
    frame.render_widget(Paragraph::new(lines), area);
    let x = area.x + 2 + state.filter.chars().count() as u16;
    frame.set_cursor_position((x.min(area.x + area.width.saturating_sub(1)), area.y + 3));
}

fn draw_marketplace_list(
    frame: &mut Frame,
    state: &crate::tui::app::MarketplacesState,
    area: Rect,
    theme: &crate::theme::Theme,
) {
    let marketplaces = state.marketplaces();
    if marketplaces.is_empty() {
        let message = if state.all.is_empty() {
            "No marketplaces yet.\n\nPress Ctrl+A to add one."
        } else {
            "No matching marketplaces."
        };
        frame.render_widget(
            Paragraph::new(Span::styled(message, Style::default().fg(theme.dim)))
                .wrap(Wrap { trim: true }),
            area,
        );
        return;
    }

    let focused = state.pane == MarketplacePane::Marketplaces;
    let selected = state.selected.min(marketplaces.len() - 1);
    let capacity = area.height.max(1) as usize;
    let start = selected
        .saturating_sub(capacity / 2)
        .min(marketplaces.len().saturating_sub(capacity));
    let end = (start + capacity).min(marketplaces.len());

    let mut lines: Vec<Line> = Vec::new();
    for (offset, marketplace) in marketplaces[start..end].iter().enumerate() {
        let absolute = start + offset;
        let is_selected = absolute == selected;
        let marker = if is_selected {
            if focused {
                "→ "
            } else {
                "· "
            }
        } else {
            "  "
        };
        let name_style = if is_selected && focused {
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme.assistant)
        };
        let installed = marketplace
            .plugins
            .iter()
            .filter(|plugin| plugin.installed)
            .count();
        let mut spans = vec![
            Span::styled(marker, Style::default().fg(theme.accent)),
            Span::styled(truncate(&marketplace.name, 22), name_style),
            Span::styled(
                format!("  {}", marketplace.plugins.len()),
                Style::default().fg(theme.dim),
            ),
        ];
        if installed > 0 {
            spans.push(Span::styled(
                format!("  ✓{installed}"),
                Style::default().fg(theme.success),
            ));
        }
        let bg = (is_selected && focused).then_some(theme.tool_pending_bg);
        lines.push(filled_line(spans, area.width as usize, bg));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_marketplace_detail(
    frame: &mut Frame,
    state: &crate::tui::app::MarketplacesState,
    area: Rect,
    theme: &crate::theme::Theme,
) {
    let Some(marketplace) = state.selected_marketplace() else {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "Select a marketplace to see its plugins.",
                Style::default().fg(theme.dim),
            )),
            area,
        );
        return;
    };

    let bold = Modifier::BOLD;
    let installed = marketplace
        .plugins
        .iter()
        .filter(|plugin| plugin.installed)
        .count();
    let mut lines: Vec<Line> = vec![Line::from(Span::styled(
        marketplace.name.clone(),
        Style::default().fg(theme.accent).add_modifier(bold),
    ))];
    if let Some(owner) = &marketplace.owner {
        lines.push(Line::from(Span::styled(
            format!("by {owner}"),
            Style::default().fg(theme.dim),
        )));
    }
    lines.push(Line::from(Span::styled(
        truncate(&marketplace.source, area.width as usize),
        Style::default().fg(theme.info),
    )));
    lines.push(Line::from(Span::styled(
        truncate_path(&marketplace.path.display().to_string(), area.width as usize),
        Style::default().fg(theme.dim),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "{} plugins · {} installed",
            marketplace.plugins.len(),
            installed
        ),
        Style::default().fg(theme.dim),
    )));
    if let Some(error) = &marketplace.error {
        lines.push(Line::from(Span::styled(
            format!("⚠ {error}"),
            Style::default().fg(theme.error),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Plugins",
        Style::default().fg(theme.tool).add_modifier(bold),
    )));

    let focused = state.pane == MarketplacePane::Plugins;
    let plugins = state.plugins();
    let available = (area.height as usize).saturating_sub(lines.len());
    if plugins.is_empty() {
        let message = if marketplace.plugins.is_empty() {
            "  this marketplace lists no plugins"
        } else {
            "  no matching plugins"
        };
        lines.push(Line::from(Span::styled(
            message.to_string(),
            Style::default().fg(theme.dim),
        )));
    } else if available > 0 {
        let selected = state.plugin_selected.min(plugins.len() - 1);
        let start = selected
            .saturating_sub(available / 2)
            .min(plugins.len().saturating_sub(available.min(plugins.len())));
        let end = (start + available).min(plugins.len());
        for (offset, plugin) in plugins[start..end].iter().enumerate() {
            let absolute = start + offset;
            let is_selected = absolute == selected;
            let marker = if is_selected {
                if focused {
                    "→ "
                } else {
                    "· "
                }
            } else {
                "  "
            };
            let (icon, icon_style) = match (plugin.installed, plugin.enabled) {
                (true, true) => ("✓ ", Style::default().fg(theme.success)),
                (true, false) => ("○ ", Style::default().fg(theme.dim)),
                (false, _) => ("+ ", Style::default().fg(theme.accent)),
            };
            let name_style = if is_selected && focused {
                Style::default().fg(theme.accent).add_modifier(bold)
            } else {
                Style::default().fg(theme.assistant)
            };
            let mut spans = vec![
                Span::styled(marker, Style::default().fg(theme.accent)),
                Span::styled(icon, icon_style),
                Span::styled(plugin.name.clone(), name_style),
            ];
            if let Some(version) = &plugin.version {
                spans.push(Span::styled(
                    format!(" v{version}"),
                    Style::default().fg(theme.dim),
                ));
            }
            if plugin.installed {
                spans.push(Span::styled(
                    if plugin.enabled {
                        "  enabled"
                    } else {
                        "  disabled"
                    },
                    Style::default().fg(theme.dim),
                ));
            }
            if let Some(description) = &plugin.description {
                let used: usize = spans.iter().map(|span| span.content.chars().count()).sum();
                let room = (area.width as usize).saturating_sub(used + 4);
                if room > 8 {
                    spans.push(Span::styled(
                        format!("  — {}", truncate(description, room)),
                        Style::default().fg(theme.dim),
                    ));
                }
            }
            let bg = (is_selected && focused).then_some(theme.tool_pending_bg);
            lines.push(filled_line(spans, area.width as usize, bg));
        }
        if start > 0 || end < plugins.len() {
            lines.push(Line::from(Span::styled(
                format!("  ({}/{})", selected + 1, plugins.len()),
                Style::default().fg(theme.dim),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_marketplace_footer(
    frame: &mut Frame,
    state: &crate::tui::app::MarketplacesState,
    area: Rect,
    theme: &crate::theme::Theme,
) {
    let mut lines: Vec<Line> = Vec::new();
    if state.busy {
        lines.push(Line::from(Span::styled(
            "  working…",
            Style::default().fg(theme.tool),
        )));
    } else if let Some(error) = &state.error {
        lines.push(Line::from(Span::styled(
            format!("  {error}"),
            Style::default().fg(theme.error),
        )));
    } else if let Some(message) = &state.message {
        lines.push(Line::from(Span::styled(
            format!("  {message}"),
            Style::default().fg(theme.success),
        )));
    } else {
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "  ↑/↓ move · Tab pane · Enter install/disable · Ctrl+U update · Ctrl+A add · Ctrl+X remove · Ctrl+R reload · Esc close",
        Style::default().fg(theme.dim),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

fn draw_marketplace_add(
    frame: &mut Frame,
    state: &crate::tui::app::MarketplacesState,
    area: Rect,
    theme: &crate::theme::Theme,
) {
    let bold = Modifier::BOLD;
    let lines = vec![
        Line::from(Span::styled(
            "Add a marketplace",
            Style::default().fg(theme.accent).add_modifier(bold),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Git URL, owner/repo shorthand, or local path with a",
            Style::default().fg(theme.info),
        )),
        Line::from(Span::styled(
            ".oxide/marketplace.json or .claude-plugin/marketplace.json.",
            Style::default().fg(theme.info),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::styled(
                state.add_input.clone(),
                Style::default().fg(theme.assistant),
            ),
        ]),
        Line::from(""),
        Line::from(Span::styled(
            "Enter add · Esc cancel",
            Style::default().fg(theme.dim),
        )),
    ];
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
    let x = area.x + 2 + state.add_input.chars().count() as u16;
    frame.set_cursor_position((x.min(area.x + area.width.saturating_sub(1)), area.y + 5));
}

fn draw_marketplace_remove(
    frame: &mut Frame,
    state: &crate::tui::app::MarketplacesState,
    area: Rect,
    theme: &crate::theme::Theme,
) {
    let Some(marketplace) = state.selected_marketplace() else {
        return;
    };
    let installed = marketplace
        .plugins
        .iter()
        .filter(|plugin| plugin.installed)
        .count();
    let mut lines = vec![
        Line::from(Span::styled(
            format!("Remove marketplace `{}`?", marketplace.name),
            Style::default()
                .fg(theme.error)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            marketplace.source.clone(),
            Style::default().fg(theme.info),
        )),
        Line::from(""),
    ];
    if installed > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "This also uninstalls {installed} installed plugin{}.",
                if installed == 1 { "" } else { "s" }
            ),
            Style::default().fg(theme.info),
        )));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(Span::styled(
        "Enter/y confirm · Esc/n cancel",
        Style::default().fg(theme.dim),
    )));
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), area);
}

#[derive(Clone, Copy)]
struct SuggestionWindow {
    popup: Rect,
    offset: usize,
    count: usize,
}

fn suggestion_window(app: &App, area: Rect) -> Option<SuggestionWindow> {
    if app.suggestions.is_empty() || area.height < 3 || area.width < 6 {
        return None;
    }
    let count = app
        .suggestions
        .len()
        .min(MAX_SUGGESTION_ROWS)
        .min(area.height.saturating_sub(2) as usize);
    if count == 0 {
        return None;
    }
    let height = count as u16 + 2;
    let popup = Rect {
        x: area.x.saturating_add(1),
        y: area.y + area.height.saturating_sub(height),
        // Match Pi's editor width instead of a fixed cap, so a long command
        // name and its description are not clipped on a wide terminal.
        width: area.width.saturating_sub(2),
        height,
    };
    let offset = app
        .suggestion_index
        .saturating_sub(count.saturating_sub(1))
        .min(app.suggestions.len().saturating_sub(count));
    Some(SuggestionWindow {
        popup,
        offset,
        count,
    })
}

pub(crate) fn suggestion_index_at(
    app: &App,
    terminal_area: Rect,
    column: u16,
    row: u16,
) -> Option<usize> {
    let areas = main_areas(terminal_area, app);
    let window = suggestion_window(app, areas[0])?;
    let inner = Rect {
        x: window.popup.x.saturating_add(1),
        y: window.popup.y.saturating_add(1),
        width: window.popup.width.saturating_sub(2),
        height: window.popup.height.saturating_sub(2),
    };
    if column < inner.x
        || column >= inner.x.saturating_add(inner.width)
        || row < inner.y
        || row >= inner.y.saturating_add(inner.height)
    {
        return None;
    }
    Some(window.offset + usize::from(row - inner.y))
}

fn draw_suggestions(frame: &mut Frame, app: &App, area: Rect) {
    let Some(window) = suggestion_window(app, area) else {
        return;
    };
    frame.render_widget(Clear, window.popup);

    let is_command = app.input.starts_with('/');
    // The border and the highlight symbol sit outside the item text.
    let content_width = window.popup.width.saturating_sub(4) as usize;
    let hints = &app.suggestions[window.offset..window.offset + window.count];
    let widest = hints
        .iter()
        .map(|hint| hint.name.chars().count())
        .max()
        .unwrap_or(0);
    // Size the name column to the widest entry so a long command name is not
    // cut off; a fixed cap hid the tail that distinguishes sibling commands.
    // Only the description is sacrificed, and only when space runs out.
    let reserved = if is_command {
        3 + MIN_SUGGESTION_DESCRIPTION
    } else {
        0
    };
    let name_width = widest
        .min(content_width.saturating_sub(1 + reserved))
        .max(1);
    let description_width = content_width.saturating_sub(1 + name_width + 3);
    let items: Vec<ListItem> = hints
        .iter()
        .map(|hint| {
            let prefix = if is_command { "/" } else { "@" };
            let name = if is_command {
                truncate(&hint.name, name_width)
            } else {
                truncate_path(&hint.name, name_width)
            };
            let mut spans = vec![Span::styled(
                format!("{prefix}{name:<name_width$}"),
                Style::default()
                    .fg(app.theme.accent)
                    .add_modifier(Modifier::BOLD),
            )];
            if is_command && !hint.description.is_empty() {
                spans.push(Span::styled(
                    format!("   {}", truncate(&hint.description, description_width)),
                    Style::default().fg(app.theme.info),
                ));
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let title = if is_command {
        "commands · click or Tab to complete"
    } else {
        "files · click or Tab to complete"
    };
    let block = panel(title, app.theme.border);
    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("› ");
    let mut list_state = ListState::default();
    list_state.select(Some(app.suggestion_index.saturating_sub(window.offset)));
    frame.render_stateful_widget(list, window.popup, &mut list_state);
}

/// Two-row footer: project context above, runtime usage and model controls
/// below. The stats read left-to-right while the active model stays
/// right-aligned, echoing Pi's layout with Oxide's semantic colors.
fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let has_statuses = !app.extension_statuses.is_empty();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints(if has_statuses {
            vec![
                Constraint::Length(1),
                Constraint::Length(1),
                Constraint::Length(1),
            ]
        } else {
            vec![Constraint::Length(1), Constraint::Length(1)]
        })
        .split(area);
    let width = area.width as usize;
    let dim = Style::default().fg(app.theme.dim);

    // Row one: where we are, the branch we are on, and the session name.
    let mut location = display_path(&app.cwd);
    if let Some(branch) = &app.git_branch {
        location.push_str(&format!(" ({branch})"));
    }
    if let Some(name) = &app.session_name {
        location.push_str(&format!(" • {name}"));
    }
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            truncate_dots(&location, width),
            dim,
        ))),
        rows[0],
    );

    // Row two: usage on the left, the model and thinking level on the right.
    let mut parts: Vec<String> = Vec::new();
    if app.tokens_in > 0 {
        parts.push(format!("↑{}", format_tokens(app.tokens_in)));
    }
    if app.tokens_out > 0 {
        parts.push(format!("↓{}", format_tokens(app.tokens_out)));
    }
    if app.tokens_cache_read > 0 {
        parts.push(format!("R{}", format_tokens(app.tokens_cache_read)));
    }
    if app.tokens_cache_write > 0 {
        parts.push(format!("W{}", format_tokens(app.tokens_cache_write)));
    }
    if app.tokens_cache_read + app.tokens_cache_write > 0 {
        if let Some(hit) = app.cache_hit_rate {
            parts.push(format!("CH{hit:.1}%"));
        }
    }
    if app.cost > 0.0 {
        parts.push(format!("${:.3}", app.cost));
    }

    let mut left: Vec<Span<'static>> = Vec::new();
    for part in &parts {
        if !left.is_empty() {
            left.push(Span::styled(" ", dim));
        }
        left.push(Span::styled(part.clone(), dim));
    }
    if app.context_limit > 0 {
        if !left.is_empty() {
            left.push(Span::styled(" ", dim));
        }
        let auto = if app.auto_compact { " (auto)" } else { "" };
        let (text, color) = if app.context_used > 0 {
            let pct = context_percent(app.context_used, app.context_limit);
            (
                format!("{pct}%/{}{auto}", format_tokens(app.context_limit)),
                context_color(pct, &app.theme),
            )
        } else {
            (
                format!("?/{}{auto}", format_tokens(app.context_limit)),
                app.theme.dim,
            )
        };
        left.push(Span::styled(text, Style::default().fg(color)));
    }

    let model = if app.model.is_empty() {
        "no-model".to_string()
    } else {
        app.model.clone()
    };
    let mut right = model;
    if app.show_thinking {
        if app.reasoning == Reasoning::Off {
            right.push_str(" • thinking off");
        } else {
            right.push_str(&format!(" • {}", app.reasoning.label()));
        }
    }
    if app.available_providers > 1 && !app.provider.is_empty() {
        right = format!("({}) {right}", app.provider);
    }
    frame.render_widget(
        Paragraph::new(aligned_row(left, vec![Span::styled(right, dim)], width)),
        rows[1],
    );

    if has_statuses {
        let statuses = app
            .extension_statuses
            .values()
            .map(|text| sanitize_status(text))
            .collect::<Vec<_>>()
            .join(" ");
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_dots(&statuses, width),
                dim,
            ))),
            rows[2],
        );
    }
}

/// The Portkey spend bar: one full-width row with the user label and the
/// session, today, and month columns, painted with the theme's bar colors.
fn draw_usage_bar(frame: &mut Frame, app: &App, area: Rect) {
    let Some(bar) = &app.usage else {
        return;
    };
    let base = Style::default()
        .bg(app.theme.usage_bar_bg)
        .fg(app.theme.usage_bar_fg);
    let label = Style::default()
        .bg(app.theme.usage_bar_bg)
        .fg(app.theme.usage_bar_label)
        .add_modifier(Modifier::BOLD);
    let lead = format!("→ {} · ", bar.user);
    let columns = truncate_dots(
        &bar.columns(app.cost),
        (area.width as usize).saturating_sub(lead.chars().count()),
    );
    let spans = vec![Span::styled(lead, label), Span::styled(columns, base)];
    frame.render_widget(Paragraph::new(Line::from(spans)).style(base), area);
}

/// Truncates to `width` with a three-dot ellipsis, matching Pi's footer.
fn truncate_dots(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width <= 3 {
        return text.chars().take(width).collect();
    }
    let mut out: String = text.chars().take(width - 3).collect();
    out.push_str("...");
    out
}

/// Collapses control characters so an extension status stays on one line.
fn sanitize_status(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Joins left- and right-aligned span groups on a single row, padding with a
/// gap so the right group ends at `width`. Falls back to a truncated single
/// line when there is not enough room for both.
fn aligned_row(left: Vec<Span<'static>>, right: Vec<Span<'static>>, width: usize) -> Line<'static> {
    let measure = |spans: &[Span<'static>]| -> usize {
        spans.iter().map(|span| span.content.chars().count()).sum()
    };
    let left_width = measure(&left);
    let right_width = measure(&right);
    if left_width + right_width + 1 > width {
        let mut spans = left;
        spans.extend(right);
        return Line::from(truncate_spans(spans, width));
    }
    let mut spans = left;
    spans.push(Span::raw(" ".repeat(width - left_width - right_width)));
    spans.extend(right);
    Line::from(spans)
}

/// The spinner glyph rotates once per tick, so the editor border animates
/// while the agent is working without a separate timer.
fn spinner(since: Option<std::time::Instant>) -> &'static str {
    const FRAMES: [&str; 4] = ["⠋", "⠙", "⠹", "⠸"];
    let tick = since
        .map(|start| start.elapsed().as_millis() / 120)
        .unwrap_or(0);
    FRAMES[(tick as usize) % FRAMES.len()]
}

/// Formats token counts compactly (1234 -> "1.2k").
fn compact_tokens(value: u64) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 1_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn truncate_spans(spans: Vec<Span<'static>>, width: usize) -> Vec<Span<'static>> {
    let total: usize = spans.iter().map(|span| span.content.chars().count()).sum();
    if total <= width {
        return spans;
    }
    if width == 0 {
        return Vec::new();
    }

    let mut remaining = width - 1;
    let mut truncated = Vec::new();
    for span in spans {
        if remaining == 0 {
            break;
        }
        let text: String = span.content.chars().take(remaining).collect();
        remaining = remaining.saturating_sub(text.chars().count());
        if !text.is_empty() {
            truncated.push(Span::styled(text, span.style));
        }
    }
    truncated.push(Span::raw("…"));
    truncated
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(width - 1).collect();
    out.push('…');
    out
}

/// Truncates a path from the left so the filename (the distinguishing part)
/// stays visible, snapping to a path separator when one is available.
fn truncate_path(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count <= width {
        return text.to_string();
    }
    if width <= 1 {
        return "…".chars().take(width).collect();
    }
    let tail: String = text.chars().skip(count - (width - 1)).collect();
    let tail = match tail.split_once('/') {
        Some((_, rest)) if !rest.is_empty() => format!("/{rest}"),
        _ => tail,
    };
    let mut out = String::from("…");
    out.push_str(&tail);
    out
}

/// Abbreviate a path under the user's home directory with a leading `~`.
fn display_path(path: &str) -> String {
    if let Some(home) = dirs::home_dir() {
        let home = home.to_string_lossy();
        if path == home {
            return "~".to_string();
        }
        if let Some(rest) = path
            .strip_prefix(home.as_ref())
            .and_then(|r| r.strip_prefix('/'))
        {
            return format!("~/{rest}");
        }
    }
    path.to_string()
}

fn context_percent(used: u64, limit: u64) -> u64 {
    (used as f64 / limit as f64 * 100.0).round() as u64
}

/// Context usage color escalates from muted to warning to error, matching Pi's
/// `>90` / `>70` thresholds.
fn context_color(pct: u64, theme: &crate::theme::Theme) -> Color {
    if pct > 90 {
        theme.error
    } else if pct > 70 {
        theme.tool
    } else {
        theme.dim
    }
}

/// Formats token counts the way Pi's footer does: `999`, `1.2k`, `12k`,
/// `1.2M`, `12M`.
fn format_tokens(value: u64) -> String {
    if value < 1_000 {
        value.to_string()
    } else if value < 10_000 {
        format!("{:.1}k", value as f64 / 1_000.0)
    } else if value < 1_000_000 {
        format!("{}k", (value as f64 / 1_000.0).round() as u64)
    } else if value < 10_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else {
        format!("{}M", (value as f64 / 1_000_000.0).round() as u64)
    }
}

/// The editor border color reflects the active thinking level (Pi behavior).
fn reasoning_color(reasoning: Reasoning, theme: &crate::theme::Theme) -> Color {
    match reasoning {
        Reasoning::Auto => theme.thinking_low,
        Reasoning::Off => theme.thinking_off,
        Reasoning::Low => theme.thinking_low,
        Reasoning::Medium => theme.thinking_medium,
        Reasoning::High => theme.thinking_high,
    }
}

fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let top_pad = MESSAGE_TOP_PAD.min(area.height);
    let inner = Rect {
        x: area.x + 1,
        y: area.y + top_pad,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(top_pad),
    };

    let width = inner.width as usize;
    sync_lines(app, width);

    let total = app.lines.len() as u16;
    let view = inner.height;
    app.view_height = view;
    if app.auto_scroll {
        app.scroll = total.saturating_sub(view);
    } else {
        app.scroll = app.scroll.min(total.saturating_sub(view));
    }

    let start = app.scroll as usize;
    let end = (start + view as usize).min(app.lines.len());
    let content = selection_lines(app, start, end);
    // Every message line is already wrapped to the viewport width, so the
    // paragraph must not wrap again: `Wrap` inserts a phantom empty row before
    // any line that exactly fills the width, which desynchronises the tool
    // panel backgrounds from their text.
    let paragraph = Paragraph::new(content);
    frame.render_widget(paragraph, inner);
}

/// The visible conversation lines with the active selection background applied.
/// Only the rendered window is cloned, and only while a selection exists.
fn selection_lines(app: &App, start: usize, end: usize) -> Vec<Line<'static>> {
    let Some(selection) = app.selection else {
        return app.lines[start..end].to_vec();
    };
    let highlight = Style::default()
        .bg(app.theme.accent)
        .fg(Color::Black)
        .add_modifier(Modifier::BOLD);
    app.lines[start..end]
        .iter()
        .enumerate()
        .map(|(offset, line)| highlight_line(line, start + offset, &selection, highlight))
        .collect()
}

/// Splits a line's spans so the selected cells carry the highlight style.
fn highlight_line(
    line: &Line<'static>,
    line_index: usize,
    selection: &Selection,
    highlight: Style,
) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    let mut column = 0usize;
    for span in &line.spans {
        let mut run: Option<(String, bool)> = None;
        for (offset, ch) in span.content.chars().enumerate() {
            let selected = selection.contains(line_index, column + offset);
            match &mut run {
                Some((text, current)) if *current == selected => text.push(ch),
                Some((text, current)) => {
                    spans.push(Span::styled(
                        std::mem::take(text),
                        if *current {
                            span.style.patch(highlight)
                        } else {
                            span.style
                        },
                    ));
                    run = Some((ch.to_string(), selected));
                }
                None => run = Some((ch.to_string(), selected)),
            }
        }
        if let Some((text, selected)) = run {
            spans.push(Span::styled(
                text,
                if selected {
                    span.style.patch(highlight)
                } else {
                    span.style
                },
            ));
        }
        column += span.content.chars().count();
    }
    Line::from(spans).style(line.style)
}

/// Maps a terminal cell to an absolute conversation line and column, for mouse
/// selection. Returns `None` when the cell is outside the message viewport.
/// Columns are clamped to the viewport so drags past an edge still select.
pub(crate) fn message_position_at(
    app: &App,
    terminal_area: Rect,
    column: u16,
    row: u16,
) -> Option<(usize, usize)> {
    let areas = main_areas(terminal_area, app);
    let message_area = areas[0];
    let top_pad = MESSAGE_TOP_PAD.min(message_area.height);
    if message_area.width <= 2 || message_area.height <= top_pad {
        return None;
    }
    let inner = Rect {
        x: message_area.x + 1,
        y: message_area.y + top_pad,
        width: message_area.width.saturating_sub(2),
        height: message_area.height.saturating_sub(top_pad),
    };
    if row < inner.y || row >= inner.y.saturating_add(inner.height) {
        return None;
    }
    let max_column = inner.x.saturating_add(inner.width).saturating_sub(1);
    let column = column.clamp(inner.x, max_column) - inner.x;
    let line = app.scroll as usize + usize::from(row - inner.y);
    Some((line, usize::from(column)))
}

/// Incrementally rebuild rendered lines from the first item explicitly marked
/// dirty. Appends are detected from the cached item count, avoiding full-history
/// hashing on every streamed delta or spinner tick.
fn sync_lines(app: &mut App, width: usize) {
    if app.render_width != width {
        app.lines.clear();
        app.line_offsets.clear();
        app.render_dirty_from = Some(0);
        app.render_width = width;
    }

    let count = app.items.len();
    let cached = app.line_offsets.len();
    if cached > count {
        app.mark_render_dirty(count.saturating_sub(1));
    }
    let start = app
        .render_dirty_from
        .unwrap_or(cached)
        .min(cached)
        .min(count);
    if start == count && cached == count {
        return;
    }

    let cut = app
        .line_offsets
        .get(start)
        .copied()
        .unwrap_or(app.lines.len());
    app.lines.truncate(cut);
    app.line_offsets.truncate(start);

    let running = app.running_tool.as_ref().map(|(name, started)| Running {
        name: name.as_str(),
        elapsed: started.elapsed(),
        subagent: app.subagent.as_ref(),
    });
    for index in start..count {
        let offset = app.lines.len();
        app.line_offsets.push(offset);
        render_item_themed(
            &app.items[index],
            width,
            app.expand_tools,
            app.show_thinking_blocks,
            &app.theme,
            running,
            &mut app.lines,
        );
        if app.lines.len() > offset {
            app.lines.push(Line::from(""));
        }
    }
    app.render_dirty_from = None;
}

/// The tool call currently running: its name, elapsed time, and the subagent it
/// spawned, if it is a `task` call.
#[derive(Clone, Copy)]
struct Running<'a> {
    name: &'a str,
    elapsed: std::time::Duration,
    subagent: Option<&'a SubagentState>,
}

fn render_item_themed(
    item: &ChatItem,
    width: usize,
    expand_tools: bool,
    expand_thinking: bool,
    theme: &crate::theme::Theme,
    running: Option<Running<'_>>,
    lines: &mut Vec<Line<'static>>,
) {
    let bold = Modifier::BOLD;
    match item {
        ChatItem::Banner { info } => render_banner_themed(width, theme, info, lines),
        ChatItem::User(text) => {
            let prefix = vec![
                Span::styled("❯ ", Style::default().fg(theme.user).add_modifier(bold)),
                Span::styled("you", Style::default().fg(theme.user).add_modifier(bold)),
                Span::styled(" ", Style::default()),
            ];
            lines.extend(wrapped_with_prefix(prefix, text, width, Style::default()));
        }
        ChatItem::Thinking { text, millis } => {
            // Pi renders reasoning as italic, muted text with no panel or
            // background, and collapses it to a bare label once hidden.
            let header_style = Style::default().fg(theme.thinking_text);
            let body_style = header_style.add_modifier(Modifier::ITALIC);
            let label = match millis {
                Some(millis) => format!("Thought for {}", format_duration(*millis)),
                None => "Thinking".to_string(),
            };
            let mut header = vec![
                Span::styled("✦ ", header_style),
                Span::styled(label, header_style.add_modifier(bold)),
            ];
            let empty = text.trim().is_empty();
            if !expand_thinking && !empty {
                header.push(Span::styled(" · ", Style::default().fg(theme.dim)));
                header.push(Span::styled(
                    "Ctrl+T to expand",
                    Style::default().fg(theme.dim),
                ));
            }
            lines.extend(wrapped_with_prefix(header, "", width, Style::default()));
            if empty || !expand_thinking {
                return;
            }
            let text = crate::tools::sanitize_terminal_output(text);
            for line in wrap(text.trim(), width.saturating_sub(2).max(1)) {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(line, body_style),
                ]));
            }
        }
        ChatItem::Assistant(text) => {
            let prefix = vec![
                Span::styled(
                    "◆ ",
                    Style::default().fg(theme.assistant).add_modifier(bold),
                ),
                Span::styled(
                    "oxide",
                    Style::default().fg(theme.assistant).add_modifier(bold),
                ),
                Span::styled(" ", Style::default()),
            ];
            lines.extend(crate::tui::markdown::render_with_prefix(
                text, width, prefix, theme,
            ));
        }
        ChatItem::Tool { name, args } => {
            let inner = box_inner_width(width);
            let mut panel: Vec<Line<'static>> = Vec::new();
            if let Some(path) = file_tool_path(name, args) {
                let (verb, color) = if matches!(
                    crate::tools::canonical_tool_name(name),
                    "write_file" | "patch" | "edit"
                ) {
                    ("Edit", theme.tool)
                } else {
                    ("Read", theme.accent)
                };
                panel.extend(action_lines(verb, &path, color, bold, inner));
            } else if let Some(command) = bash_command(name, args) {
                let mut subject = format!("{command}{}", bash_timeout_suffix(name, args));
                if let Some(elapsed) = running.as_ref().filter(|active| active.name == name) {
                    subject.push_str(&format!(
                        " · Elapsed {}",
                        format_duration(elapsed.elapsed.as_millis() as u64)
                    ));
                }
                panel.extend(action_lines("Run", &subject, theme.tool, bold, inner));
            } else {
                let mut subject = format!(" {}", tool_arg_summary(name, args));
                let active = running.as_ref().filter(|active| active.name == name);
                if let Some(active) = active {
                    subject.push_str(&format!(
                        " · Elapsed {}",
                        format_duration(active.elapsed.as_millis() as u64)
                    ));
                }
                panel.extend(wrapped_with_prefix(
                    vec![
                        Span::styled("⚙ ", Style::default().fg(theme.tool)),
                        Span::styled(
                            name.clone(),
                            Style::default().fg(theme.tool).add_modifier(bold),
                        ),
                    ],
                    &subject,
                    inner,
                    Style::default().fg(theme.info),
                ));
                if let Some(state) = active.and_then(|active| active.subagent) {
                    panel.extend(wrapped_with_prefix(
                        vec![
                            Span::styled("↳ ", Style::default().fg(theme.info)),
                            Span::styled(
                                state.agent.clone(),
                                Style::default().fg(theme.info).add_modifier(bold),
                            ),
                        ],
                        &format!(
                            " · {} · {} call(s)",
                            activity_summary(&state.tool, &state.args),
                            state.tools
                        ),
                        inner,
                        Style::default().fg(theme.info),
                    ));
                }
            }
            push_bg_panel(lines, panel, width, theme.tool_pending_bg);
        }
        ChatItem::ToolProgress { name, output } => {
            let mut panel: Vec<Line<'static>> = Vec::new();
            if crate::tools::canonical_tool_name(name) != "bash" {
                panel.push(Line::from(vec![
                    Span::styled("⋯ ", Style::default().fg(theme.info)),
                    Span::styled(name.clone(), Style::default().fg(theme.info)),
                ]));
            }
            if !output.trim().is_empty() {
                push_tool_body(
                    &mut panel,
                    output,
                    box_inner_width(width),
                    Style::default().fg(theme.info),
                    expand_tools,
                    tool_preview(name),
                );
            }
            if !panel.is_empty() {
                push_bg_panel(lines, panel, width, theme.tool_pending_bg);
            }
        }
        ChatItem::ToolResult {
            name,
            args,
            output,
            diff,
            millis,
        } => {
            let inner = box_inner_width(width);
            let mut panel: Vec<Line<'static>> = Vec::new();
            let mut bg = theme.tool_success_bg;
            let command = bash_command(name, args);
            if let Some(diff) = diff {
                let failed = output.starts_with("error:");
                if failed {
                    bg = theme.tool_error_bg;
                }
                let (verb, color) = if failed {
                    ("Edit failed", theme.error)
                } else {
                    ("Edited", theme.success)
                };
                panel.extend(action_lines(verb, &diff.path, color, bold, inner));
                let (mut body, hidden) = diff_body(diff, expand_tools, inner, theme);
                if !body.is_empty() {
                    panel.push(Line::from(""));
                }
                panel.append(&mut body);
                if let Some(hidden) = hidden {
                    panel.push(collapsed_hint(hidden, theme.info, inner));
                }
                if failed {
                    push_tool_body(
                        &mut panel,
                        output,
                        inner,
                        Style::default().fg(theme.error),
                        expand_tools,
                        tool_preview(name),
                    );
                } else if let Some((_, rest)) = output.split_once("\n\n") {
                    if !rest.trim().is_empty() {
                        push_tool_body(
                            &mut panel,
                            rest,
                            inner,
                            Style::default().fg(theme.info),
                            expand_tools,
                            tool_preview(name),
                        );
                    }
                }
            } else if let Some(path) = file_tool_path(name, args) {
                if crate::tools::canonical_tool_name(name) == "read_file" {
                    if output.starts_with("error:") {
                        bg = theme.tool_error_bg;
                        panel.extend(action_lines("Read failed", &path, theme.error, bold, inner));
                        panel.push(Line::from(""));
                        push_tool_body(
                            &mut panel,
                            output,
                            inner,
                            Style::default().fg(theme.error),
                            expand_tools,
                            tool_preview(name),
                        );
                    } else {
                        panel.extend(action_lines("Read", &path, theme.success, bold, inner));
                        if !output.trim().is_empty() {
                            panel.push(Line::from(""));
                            push_tool_body(
                                &mut panel,
                                output,
                                inner,
                                Style::default().fg(theme.info),
                                expand_tools,
                                tool_preview(name),
                            );
                        }
                    }
                } else if output.starts_with("error:") {
                    bg = theme.tool_error_bg;
                    panel.extend(action_lines("Edit failed", &path, theme.error, bold, inner));
                    panel.push(Line::from(""));
                    push_tool_body(
                        &mut panel,
                        output,
                        inner,
                        Style::default().fg(theme.error),
                        expand_tools,
                        tool_preview(name),
                    );
                } else {
                    panel.extend(action_lines("Edited", &path, theme.success, bold, inner));
                    if let Some((_, rest)) = output.split_once("\n\n") {
                        if !rest.trim().is_empty() {
                            panel.push(Line::from(""));
                            push_tool_body(
                                &mut panel,
                                rest,
                                inner,
                                Style::default().fg(theme.info),
                                expand_tools,
                                tool_preview(name),
                            );
                        }
                    }
                }
            } else if let Some(command) = &command {
                let exit = bash_exit_code(output);
                let failed = exit
                    .map(|code| code != 0)
                    .unwrap_or_else(|| output.starts_with("error:"));
                let color = if failed { theme.error } else { theme.success };
                if failed {
                    bg = theme.tool_error_bg;
                }
                let timeout = bash_timeout_suffix(name, args);
                let subject = match exit {
                    Some(code) => format!("{command}{timeout} · exit {code}"),
                    None => format!("{command}{timeout}"),
                };
                let verb = if failed { "Run failed" } else { "Ran" };
                panel.extend(action_lines(verb, &subject, color, bold, inner));
                if exit.is_none() && !output.trim().is_empty() {
                    panel.push(Line::from(""));
                    push_tool_body(
                        &mut panel,
                        output,
                        inner,
                        Style::default().fg(theme.error),
                        expand_tools,
                        tool_preview(name),
                    );
                } else if bash_has_body(output) {
                    panel.push(Line::from(""));
                    push_tool_body(
                        &mut panel,
                        &bash_body(output),
                        inner,
                        Style::default().fg(theme.info),
                        expand_tools,
                        tool_preview(name),
                    );
                }
            } else {
                let failed = output.starts_with("error:");
                if failed {
                    bg = theme.tool_error_bg;
                }
                panel.push(Line::from(vec![
                    Span::styled("↳ ", Style::default().fg(theme.info)),
                    Span::styled(name.clone(), Style::default().fg(theme.info)),
                ]));
                if !output.trim().is_empty() {
                    panel.push(Line::from(""));
                    push_tool_body(
                        &mut panel,
                        output,
                        inner,
                        Style::default().fg(theme.info),
                        expand_tools,
                        tool_preview(name),
                    );
                }
            }
            // Shell timings always show (Pi behavior); other tools only report
            // when they were slow enough to be worth calling out.
            if *millis > 0 && (command.is_some() || *millis >= TOOL_TIME_THRESHOLD_MS) {
                panel.push(Line::from(""));
                panel.push(Line::from(Span::styled(
                    format!("Took {}", format_duration(*millis)),
                    Style::default().fg(theme.dim),
                )));
            }
            push_bg_panel(lines, panel, width, bg);
        }
        ChatItem::Compaction {
            summary,
            summarized,
            tokens_before,
            read_files,
            modified_files,
        } => {
            lines.push(Line::from(vec![
                Span::styled("✻ ", Style::default().fg(theme.accent).add_modifier(bold)),
                Span::styled(
                    format!(
                        "Compacted {summarized} messages (~{} tokens)",
                        compact_tokens(*tokens_before)
                    ),
                    Style::default().fg(theme.accent).add_modifier(bold),
                ),
            ]));
            push_wrapped(lines, summary, width, Style::default().fg(theme.dim));
            if !read_files.is_empty() {
                push_wrapped(
                    lines,
                    &format!("read: {}", read_files.join(", ")),
                    width,
                    Style::default().fg(theme.info),
                );
            }
            if !modified_files.is_empty() {
                push_wrapped(
                    lines,
                    &format!("modified: {}", modified_files.join(", ")),
                    width,
                    Style::default().fg(theme.info),
                );
            }
        }
        ChatItem::Error(text) => {
            push_wrapped(
                lines,
                &format!("✗ {text}"),
                width,
                Style::default().fg(theme.error),
            );
        }
        ChatItem::Info(text) => {
            push_wrapped(
                lines,
                &format!("· {text}"),
                width,
                Style::default().fg(theme.info),
            );
        }
        ChatItem::Listing { title, rows } => {
            render_listing(title, rows, width, theme, lines);
        }
        ChatItem::Status(text) => {
            push_wrapped(
                lines,
                text,
                width,
                Style::default().fg(theme.info).add_modifier(Modifier::DIM),
            );
        }
        ChatItem::Progress { label, since } => {
            let elapsed = since.elapsed();
            lines.push(Line::from(vec![
                Span::styled(progress_bar(elapsed), Style::default().fg(theme.tool)),
                Span::styled(
                    format!("  {label}… "),
                    Style::default().fg(theme.info).add_modifier(Modifier::DIM),
                ),
                Span::styled(
                    format!("{}s", elapsed.as_secs()),
                    Style::default().fg(theme.dim),
                ),
            ]));
        }
    }
}

/// A fixed-width indeterminate progress bar for a command whose completion is
/// unknown: the filled segment sweeps across the track so the transcript shows
/// movement while the result is being prepared.
fn progress_bar(elapsed: std::time::Duration) -> String {
    const WIDTH: usize = 12;
    const SEGMENT: usize = 3;
    let cycle = WIDTH + SEGMENT;
    let position = (elapsed.as_millis() / 100) as usize % cycle;
    let mut bar = String::with_capacity(WIDTH);
    for index in 0..WIDTH {
        let filled = index <= position && index + SEGMENT > position;
        bar.push(if filled { '█' } else { '░' });
    }
    bar
}

/// Renders the banner as the wordmark on top with the welcome info stacked
/// underneath, falling back to plain text on narrow terminals.
fn render_banner_themed(
    width: usize,
    theme: &crate::theme::Theme,
    info: &[String],
    lines: &mut Vec<Line<'static>>,
) {
    let art_width = BANNER
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    if art_width > width {
        lines.push(Line::from(Span::styled(
            "oxide",
            Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
        )));
        for (index, entry) in info.iter().enumerate() {
            if index > 0 {
                lines.push(Line::default());
            }
            push_wrapped(lines, entry, width.max(1), Style::default().fg(theme.info));
        }
        return;
    }

    for (index, art) in BANNER.iter().enumerate() {
        let pad = (width - art.chars().count()) / 2;
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled((*art).to_string(), art_style(index, theme)),
        ]));
    }
    lines.push(Line::default());
    for (index, entry) in info.iter().enumerate() {
        if index > 0 {
            lines.push(Line::default());
        }
        push_wrapped(lines, entry, width.max(1), Style::default().fg(theme.info));
    }
}

fn art_style(index: usize, theme: &crate::theme::Theme) -> Style {
    Style::default()
        .fg(if index + 1 == BANNER.len() {
            theme.dim
        } else {
            theme.accent
        })
        .add_modifier(Modifier::BOLD)
}

#[cfg(test)]
fn render_item(item: &ChatItem, width: usize, expand_tools: bool, lines: &mut Vec<Line<'static>>) {
    render_item_themed(
        item,
        width,
        expand_tools,
        true,
        &crate::theme::Theme::dark(),
        None,
        lines,
    );
}

#[cfg(test)]
fn render_banner(width: usize, info: &[String], lines: &mut Vec<Line<'static>>) {
    render_banner_themed(width, &crate::theme::Theme::dark(), info, lines);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    // Pi renders the editor as two full-width rules with no side borders,
    // corners or prompt, and embeds the working status in the top rule while
    // keeping the thinking-level color on the rules.
    let border_color = reasoning_color(app.reasoning, &app.theme);
    let area = Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    };
    let title = if app.busy {
        let secs = app
            .busy_since
            .map(|start| start.elapsed().as_secs())
            .unwrap_or(0);
        let mut text = format!(" {} {} · {secs}s", spinner(app.busy_since), app.status);
        // Pi advertises how to pull queued messages back into the editor
        // (`app.message.dequeue`) while they are still waiting.
        let queued = app.queued_count();
        if queued > 0 {
            text.push_str(&format!(
                " · {queued} queued · {} to edit",
                crate::tui::dequeue_key_label()
            ));
        }
        text.push_str(" · Esc clear · /exit quit ");
        vec![Span::styled(text, Style::default().fg(app.theme.tool))]
    } else if app.attachments.is_empty() {
        Vec::new()
    } else {
        vec![Span::styled(
            format!(" {} attachment(s) ", app.attachments.len()),
            Style::default().fg(border_color),
        )]
    };
    let title = truncate_spans(title, area.width.saturating_sub(2) as usize);
    let mut block = Block::default()
        .borders(Borders::TOP | Borders::BOTTOM)
        .border_type(BorderType::Plain)
        .border_style(Style::default().fg(border_color));
    if !title.is_empty() {
        block = block.title(Line::from(title));
    }
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let paragraph = Paragraph::new(composer_text(&app.input, &app.theme))
        .wrap(Wrap { trim: false })
        .scroll((input_scroll(&app.input, app.input_cursor, width), 0));
    frame.render_widget(paragraph, inner);

    // The composer stays editable while the agent runs so a message can be
    // typed and queued as steering, so keep its caret visible then too.
    if app.connect.is_none() && app.models.is_none() {
        let (cursor_row, cursor_column) =
            input_cursor_position(&app.input, app.input_cursor.min(app.input.len()), width);
        let scroll = input_scroll(&app.input, app.input_cursor, width) as usize;
        let x = inner.x + cursor_column.min(width.saturating_sub(1)) as u16;
        let y = inner.y + cursor_row.saturating_sub(scroll) as u16;
        frame.set_cursor_position((x, y));
    }
}

/// Builds the composer text, keeping explicit newlines as visual lines and
/// highlighting `@path` mentions so the referenced token stands out from the
/// surrounding prose.
fn composer_text<'a>(input: &'a str, theme: &crate::theme::Theme) -> Text<'a> {
    Text::from(
        input
            .split('\n')
            .map(|line| Line::from(mention_spans(line, theme)))
            .collect::<Vec<_>>(),
    )
}

fn mention_spans<'a>(input: &'a str, theme: &crate::theme::Theme) -> Vec<Span<'a>> {
    let mention = Style::default()
        .fg(theme.accent)
        .add_modifier(Modifier::BOLD | Modifier::UNDERLINED);
    let mut spans = Vec::new();
    let mut token_start: Option<usize> = None;
    for (index, ch) in input.char_indices() {
        if ch.is_whitespace() {
            if let Some(start) = token_start.take() {
                push_composer_token(&mut spans, &input[start..index], mention);
            }
            spans.push(Span::raw(&input[index..index + ch.len_utf8()]));
        } else if token_start.is_none() {
            token_start = Some(index);
        }
    }
    if let Some(start) = token_start {
        push_composer_token(&mut spans, &input[start..], mention);
    }
    if spans.is_empty() {
        spans.push(Span::raw(input));
    }
    spans
}

fn push_composer_token<'a>(spans: &mut Vec<Span<'a>>, token: &'a str, mention: Style) {
    if token.starts_with('@') && token.len() > 1 {
        spans.push(Span::styled(token, mention));
    } else {
        spans.push(Span::raw(token));
    }
}

fn input_rows(input: &str, width: usize) -> usize {
    wrap(input, width)
        .len()
        .clamp(MIN_INPUT_ROWS, MAX_INPUT_ROWS)
}

fn input_scroll(input: &str, cursor: usize, width: usize) -> u16 {
    let (row, _) = input_cursor_position(input, cursor.min(input.len()), width);
    row.saturating_sub(MAX_INPUT_ROWS - 1) as u16
}

fn input_cursor_position(input: &str, cursor: usize, width: usize) -> (usize, usize) {
    let target = input[..cursor.min(input.len())].chars().count();
    let lines = wrap_layout(input, width);
    let mut row = 0usize;
    for (index, line) in lines.iter().enumerate() {
        let next = lines
            .get(index + 1)
            .map(|next| next.start)
            .unwrap_or(usize::MAX);
        if target < next {
            let column = target
                .saturating_sub(line.start)
                .min(line.end.saturating_sub(line.start));
            return (index, column);
        }
        row = index;
    }
    (row, 0)
}

/// Path argument for a `read_file`/`write_file` call, when present.
fn file_tool_path(name: &str, args: &str) -> Option<String> {
    let name = crate::tools::canonical_tool_name(name);
    if !matches!(name, "read_file" | "write_file" | "patch" | "edit") {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(args).ok()?;
    value
        .get("path")
        .and_then(|v| v.as_str())
        .filter(|path| !path.is_empty())
        .map(str::to_string)
}

/// Render a tool action as `→ Verb <subject>`, wrapping the subject across
/// lines so its full text stays visible instead of being truncated. Every
/// rendered line is capped at `width` characters, mirroring Pi's wrapped
/// tool-call text.
/// Render a tool action header, hanging-indenting continuations under the
/// subject so a long command stays attached to its `Run`/`Ran` verb.
fn action_lines(
    verb: &str,
    subject: &str,
    color: Color,
    bold: Modifier,
    width: usize,
) -> Vec<Line<'static>> {
    let prefix = vec![
        Span::styled("→ ", Style::default().fg(color)),
        Span::styled(
            verb.to_string(),
            Style::default().fg(color).add_modifier(bold),
        ),
        Span::styled(" ", Style::default().fg(color)),
    ];
    let indent = prefix
        .iter()
        .map(|span| span.content.chars().count())
        .sum::<usize>();
    let wrap_width = width.saturating_sub(indent).max(1);
    let mut segments = wrap(subject, wrap_width).into_iter();
    let first = segments.next().unwrap_or_default();
    let mut spans = prefix;
    if !first.is_empty() {
        spans.push(Span::styled(first, Style::default().fg(color)));
    }
    let mut out = vec![Line::from(spans)];
    for segment in segments {
        out.push(Line::from(Span::styled(
            format!("{}{segment}", " ".repeat(indent)),
            Style::default().fg(color),
        )));
    }
    out
}

/// Render styled `prefix` spans followed by `subject`, wrapping so every line
/// is at most `width` characters. The prefix keeps its styling on the first
/// line and continuation lines use `continuation`.
fn wrapped_with_prefix(
    prefix: Vec<Span<'static>>,
    subject: &str,
    width: usize,
    continuation: Style,
) -> Vec<Line<'static>> {
    let head: String = prefix.iter().map(|span| span.content.as_ref()).collect();
    let subject = crate::tools::sanitize_terminal_output(subject);
    let wrapped = wrap(&format!("{head}{subject}"), width.max(1));
    let mut out = Vec::with_capacity(wrapped.len());
    for (index, line) in wrapped.into_iter().enumerate() {
        if index == 0 && line.starts_with(&head) {
            let rest = line[head.len()..].to_string();
            let mut spans = prefix.clone();
            if !rest.is_empty() {
                spans.push(Span::styled(rest, continuation));
            }
            out.push(Line::from(spans));
        } else {
            out.push(Line::from(Span::styled(line, continuation)));
        }
    }
    if out.is_empty() {
        out.push(Line::from(prefix));
    }
    out
}

/// Shell command for a `bash` call, flattened to a single line for display.
fn bash_command(name: &str, args: &str) -> Option<String> {
    if crate::tools::canonical_tool_name(name) != "bash" {
        return None;
    }
    let value: serde_json::Value = serde_json::from_str(args).ok()?;
    let command = value.get("command").and_then(|v| v.as_str())?;
    let command = command.split_whitespace().collect::<Vec<_>>().join(" ");
    if command.is_empty() {
        None
    } else {
        Some(command)
    }
}

fn bash_exit_code(output: &str) -> Option<i32> {
    output
        .lines()
        .rev()
        .find_map(|line| {
            line.strip_prefix("[exit: ")
                .or_else(|| line.strip_prefix("[exit code: "))
        })
        .and_then(|rest| rest.strip_suffix(']'))
        .and_then(|code| code.trim().parse().ok())
}

/// A ` (timeout Ns)` suffix for a `bash` call whose args set a timeout.
fn bash_timeout_suffix(name: &str, args: &str) -> String {
    if crate::tools::canonical_tool_name(name) != "bash" {
        return String::new();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(args) else {
        return String::new();
    };
    match value.get("timeout").and_then(|v| v.as_u64()) {
        Some(millis) if millis > 0 => format!(" (timeout {})", format_duration(millis)),
        _ => String::new(),
    }
}

/// Human-readable duration for tool timing: `420ms`, `1.2s`, `12s`.
fn format_duration(millis: u64) -> String {
    if millis < 1000 {
        return format!("{millis}ms");
    }
    let secs = millis as f64 / 1000.0;
    if secs.fract() == 0.0 {
        format!("{}s", secs as u64)
    } else {
        format!("{secs:.1}s")
    }
}

/// Summarize a tool call's arguments for display. File tools otherwise dump
/// their entire payload (e.g. `write_file` carries the full file content), so
/// show just the path and a compact size hint instead.
/// A compact one-line description of what a subagent is doing, for the `task`
/// panel under the call that spawned it.
fn activity_summary(tool: &str, args: &str) -> String {
    if let Some(command) = bash_command(tool, args) {
        return truncate(&command, 60);
    }
    if !is_file_tool(tool) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(args) {
            for key in ["pattern", "path", "url", "query", "prompt"] {
                if let Some(text) = value.get(key).and_then(|value| value.as_str()) {
                    return truncate(text, 60);
                }
            }
        }
    }
    truncate(tool_arg_summary(tool, args).trim(), 60)
}

fn tool_arg_summary(name: &str, args: &str) -> String {
    if !is_file_tool(name) {
        return args.to_string();
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(args) else {
        return args.to_string();
    };
    let path = value.get("path").and_then(|v| v.as_str()).unwrap_or("");
    let summary = match crate::tools::canonical_tool_name(name) {
        "write_file" => value
            .get("content")
            .and_then(|v| v.as_str())
            .map(|content| format!("{path} · {} lines", content.lines().count())),
        "edit" => value
            .get("edits")
            .and_then(|v| v.as_array())
            .map(|edits| format!("{path} · {} edit(s)", edits.len())),
        "read_file" => value
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|offset| format!("{path} · from line {offset}")),
        _ => None,
    };
    let summary = summary.unwrap_or_else(|| path.to_string());
    if summary.is_empty() {
        args.to_string()
    } else {
        summary
    }
}

/// Columns budgeted for a panel's horizontal padding (` ` + ` `).
const BOX_PAD: usize = 2;
/// Narrowest panel we will draw before letting content spill to the edges.
const BOX_MIN_WIDTH: usize = 4;

/// Width available for content inside a panel of the given outer `width`.
fn box_inner_width(width: usize) -> usize {
    width.max(BOX_MIN_WIDTH).saturating_sub(BOX_PAD).max(1)
}

/// Render `body` as a background-filled panel, matching Pi's tool renderer.
/// Every row is padded to the full width so the fill reads as a solid block.
fn push_bg_panel(
    lines: &mut Vec<Line<'static>>,
    body: Vec<Line<'static>>,
    width: usize,
    bg: Color,
) {
    let outer = width.max(BOX_MIN_WIDTH);
    let inner = box_inner_width(width);
    let fill = Style::default().bg(bg);
    lines.push(Line::from(Span::styled(" ".repeat(outer), fill)));
    for line in body {
        let used: usize = line
            .spans
            .iter()
            .map(|span| span.content.chars().count())
            .sum();
        let pad = inner.saturating_sub(used);
        let mut spans = Vec::with_capacity(line.spans.len() + 3);
        spans.push(Span::styled(" ", fill));
        spans.extend(
            line.spans
                .into_iter()
                .map(|span| Span::styled(span.content, span.style.bg(bg))),
        );
        spans.push(Span::styled(format!("{} ", " ".repeat(pad)), fill));
        lines.push(Line::from(spans));
    }
    lines.push(Line::from(Span::styled(" ".repeat(outer), fill)));
}

/// A `bash` result's output with the trailing exit-code line stripped, so the
/// panel holds only the command's own output.
fn bash_body(output: &str) -> String {
    output
        .lines()
        .filter(|line| !line.starts_with("[exit: ") && !line.starts_with("[exit code: "))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Rewrites compact JSON lines in tool output as indented blocks so `gh api`,
/// `curl`, and MCP results read like data instead of one unbroken line. Lines
/// that are not JSON are left untouched.
fn readable_output(text: &str) -> String {
    text.lines()
        .map(|line| {
            let trimmed = line.trim();
            if trimmed.len() < 2 || !(trimmed.starts_with('{') || trimmed.starts_with('[')) {
                return line.to_string();
            }
            match serde_json::from_str::<serde_json::Value>(trimmed) {
                Ok(value) => {
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| line.to_string())
                }
                Err(_) => line.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A one-line affordance shown when a tool body is shortened, mirroring Pi's
/// `... (N more lines, Ctrl+O to expand)` hint.
fn collapsed_hint(hidden_lines: usize, color: Color, width: usize) -> Line<'static> {
    collapsed_hint_with(hidden_lines, "more", color, width)
}

/// Like [`collapsed_hint`], but names which end of the output was dropped so a
/// tail preview can say `earlier`.
fn collapsed_hint_with(
    hidden_lines: usize,
    direction: &str,
    color: Color,
    width: usize,
) -> Line<'static> {
    let hint = format!("⋯ {hidden_lines} {direction} lines · Ctrl+O to expand");
    Line::from(Span::styled(
        truncate(&hint, width),
        Style::default().fg(color),
    ))
}

/// Whether a `bash` result has output beyond the trailing exit-code line.
fn bash_has_body(output: &str) -> bool {
    output
        .lines()
        .filter(|line| !line.starts_with("[exit: ") && !line.starts_with("[exit code: "))
        .any(|line| !line.trim().is_empty())
}

/// How many diff lines to show while the view is collapsed with Ctrl+O.
const DIFF_PREVIEW_LINES: usize = 12;

/// Non-shell tools only report a duration once they take at least this long,
/// so a `grep`/`read` that stalls is visible without cluttering every call.
const TOOL_TIME_THRESHOLD_MS: u64 = 500;

/// Build the colored, line-numbered diff body for a file edit. Returns the
/// visible lines and, when collapsed, how many were hidden.
fn diff_body(
    diff: &DiffPreview,
    expand_tools: bool,
    width: usize,
    theme: &crate::theme::Theme,
) -> (Vec<Line<'static>>, Option<usize>) {
    let text = crate::tools::sanitize_terminal_output(&diff.text);
    if text.trim().is_empty() {
        return (Vec::new(), None);
    }
    let all: Vec<&str> = text.lines().collect();
    let limit = if expand_tools {
        all.len()
    } else {
        DIFF_PREVIEW_LINES.min(all.len())
    };
    let body: Vec<Line<'static>> = all[..limit]
        .iter()
        .map(|line| {
            Line::from(Span::styled(
                truncate(line, width),
                diff_line_style(line, theme),
            ))
        })
        .collect();
    let hidden = (limit < all.len()).then_some(all.len() - limit);
    (body, hidden)
}

fn diff_line_style(line: &str, theme: &crate::theme::Theme) -> Style {
    match line.chars().next() {
        Some('+') => Style::default().fg(theme.success),
        Some('-') => Style::default().fg(theme.error),
        _ => Style::default().fg(theme.info),
    }
}

fn push_wrapped<'a>(lines: &mut Vec<Line<'a>>, text: &str, width: usize, style: Style) {
    let text = crate::tools::sanitize_terminal_output(text);
    for wrapped in wrap(&text, width.max(1)) {
        lines.push(Line::from(Span::styled(wrapped, style)));
    }
}

/// Wrap one paragraph with a leading indent, so the continuations of a detail
/// or note stay visually attached to the row they belong to.
fn push_indented_wrapped<'a>(
    lines: &mut Vec<Line<'a>>,
    text: &str,
    indent: usize,
    width: usize,
    style: Style,
) {
    let text = crate::tools::sanitize_terminal_output(text);
    let width = width.max(1);
    let indent = indent.min(width.saturating_sub(1));
    for segment in wrap(&text, width.saturating_sub(indent).max(1)) {
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(indent)),
            Span::styled(segment, style),
        ]));
    }
}

/// Color a [`Tone`] against the active theme. There is no dedicated warning
/// slot, so `tool` (yellow) carries caution states like `Needs Auth`.
fn tone_style(tone: Tone, theme: &crate::theme::Theme) -> Style {
    match tone {
        Tone::Plain => Style::default().fg(theme.info),
        Tone::Success => Style::default().fg(theme.success),
        Tone::Warning => Style::default().fg(theme.tool),
        Tone::Error => Style::default().fg(theme.error),
        Tone::Dim => Style::default().fg(theme.dim),
    }
}

/// Render a [`ChatItem::Listing`]: a titled block whose rows align their name
/// and status columns, color the status by tone, and hang-wrap long details
/// and notes under their row.
fn render_listing(
    title: &str,
    rows: &[ListRow],
    width: usize,
    theme: &crate::theme::Theme,
    lines: &mut Vec<Line<'static>>,
) {
    let width = width.max(1);
    push_wrapped(
        lines,
        &format!("· {title}"),
        width,
        Style::default().fg(theme.info).add_modifier(Modifier::BOLD),
    );
    if rows.is_empty() {
        push_indented_wrapped(lines, "(none)", 2, width, Style::default().fg(theme.dim));
        return;
    }
    let name_width = rows
        .iter()
        .map(|row| row.name.chars().count())
        .max()
        .unwrap_or(0);
    let status_width = rows
        .iter()
        .filter_map(|row| row.status.as_deref().map(|status| status.chars().count()))
        .max()
        .unwrap_or(0);
    let name_style = Style::default()
        .fg(theme.assistant)
        .add_modifier(Modifier::BOLD);
    let detail_style = Style::default().fg(theme.dim);
    for row in rows {
        let pad_name = row.status.is_some() || row.detail.is_some();
        let mut header_width = 2 + if pad_name {
            name_width
        } else {
            row.name.chars().count()
        };
        let mut spans = vec![Span::raw("  ")];
        spans.push(Span::styled(
            if pad_name {
                format!("{:<name_width$}", row.name)
            } else {
                row.name.clone()
            },
            name_style,
        ));
        if let Some(status) = &row.status {
            spans.push(Span::raw("  "));
            header_width += 2;
            let (text, len) = if row.detail.is_some() {
                (format!("{status:<status_width$}"), status_width)
            } else {
                (status.clone(), status.chars().count())
            };
            header_width += len;
            spans.push(Span::styled(text, tone_style(row.tone, theme)));
        }
        match &row.detail {
            Some(detail) if header_width + 2 + detail.chars().count() <= width => {
                spans.push(Span::raw("  "));
                spans.push(Span::styled(detail.clone(), detail_style));
                lines.push(Line::from(spans));
            }
            Some(detail) => {
                lines.push(Line::from(spans));
                push_indented_wrapped(lines, detail, 4, width, detail_style);
            }
            None => lines.push(Line::from(spans)),
        }
        for note in &row.notes {
            push_indented_wrapped(lines, note, 4, width, detail_style);
        }
    }
}

/// Spaces added to wrapped tool-output continuations so a long line stays
/// visually attached to its own prefix (e.g. a `file:line:` match header)
/// instead of reading as a new entry.
const TOOL_WRAP_INDENT: usize = 2;

/// Wrap a tool output body for display in a panel. Lines that fit are emitted
/// unchanged; wrapped continuations are indented so `grep`/`bash` output keeps
/// its `file:line:` structure readable.
fn push_tool_output(lines: &mut Vec<Line<'static>>, text: &str, width: usize, style: Style) {
    let width = width.max(1);
    let indent = TOOL_WRAP_INDENT.min(width.saturating_sub(1));
    for raw in text.split('\n') {
        if raw.chars().count() <= width {
            lines.push(Line::from(Span::styled(raw.to_string(), style)));
            continue;
        }
        for (index, segment) in wrap(raw, width.saturating_sub(indent).max(1))
            .into_iter()
            .enumerate()
        {
            if index == 0 {
                lines.push(Line::from(Span::styled(segment, style)));
            } else {
                lines.push(Line::from(Span::styled(
                    format!("{}{segment}", " ".repeat(indent)),
                    style,
                )));
            }
        }
    }
}

/// Default lines of a tool body shown before the `Ctrl+O` expand hint, so
/// every panel stays short enough to scan.
const TOOL_PREVIEW_LINES: usize = 10;
/// Shell output is previewed from the tail, where errors and results land.
const BASH_PREVIEW_LINES: usize = 5;
/// Code search results benefit from more context than a shell tail.
const GREP_PREVIEW_LINES: usize = 15;
/// File and directory listings show the most entries before collapsing.
const LIST_PREVIEW_LINES: usize = 20;

/// Which end of a long tool body to keep in the collapsed preview.
#[derive(Clone, Copy)]
enum Preview {
    Head(usize),
    Tail(usize),
}

/// Per-tool preview budgets, mirroring Pi's renderers: a shell command keeps
/// its tail, searches keep more lines, and everything else uses the default.
fn tool_preview(name: &str) -> Preview {
    match crate::tools::canonical_tool_name(name) {
        "bash" => Preview::Tail(BASH_PREVIEW_LINES),
        "grep" => Preview::Head(GREP_PREVIEW_LINES),
        "find" | "ls" => Preview::Head(LIST_PREVIEW_LINES),
        _ => Preview::Head(TOOL_PREVIEW_LINES),
    }
}

/// Render a tool body readably and keep it short: compact JSON lines are
/// expanded, and output longer than the tool's preview budget is cut with a
/// hint to expand. A tail preview puts the hint first so the newest lines read
/// last, like Pi's shell renderer.
fn push_tool_body(
    lines: &mut Vec<Line<'static>>,
    text: &str,
    width: usize,
    style: Style,
    expand_tools: bool,
    preview: Preview,
) {
    let readable = readable_output(&crate::tools::sanitize_terminal_output(text));
    let all: Vec<&str> = readable.lines().collect();
    let limit = match preview {
        Preview::Head(limit) | Preview::Tail(limit) => limit,
    };
    if expand_tools || all.len() <= limit {
        push_tool_output(lines, &readable, width, style);
        return;
    }
    let color = style.fg.unwrap_or(Color::Reset);
    match preview {
        Preview::Head(_) => {
            push_tool_output(lines, &all[..limit].join("\n"), width, style);
            lines.push(Line::from(""));
            lines.push(collapsed_hint(all.len() - limit, color, width));
        }
        Preview::Tail(_) => {
            lines.push(collapsed_hint_with(
                all.len() - limit,
                "earlier",
                color,
                width,
            ));
            lines.push(Line::from(""));
            push_tool_output(lines, &all[all.len() - limit..].join("\n"), width, style);
        }
    }
}

/// A visually wrapped composer line: its rendered text plus the character
/// range of the source it covers.
struct WrapLine {
    text: String,
    start: usize,
    end: usize,
}

/// Wraps text the same way ratatui's `Paragraph` does, reporting the source
/// character range for every visual line so cursor placement stays in sync.
fn wrap_layout(text: &str, width: usize) -> Vec<WrapLine> {
    let width = width.max(1);
    let mut out = Vec::new();
    let segments: Vec<&str> = text.split('\n').collect();
    let last = segments.len().saturating_sub(1);
    let mut base = 0usize;
    for (index, raw) in segments.into_iter().enumerate() {
        out.extend(wrap_segment(raw, width, base));
        base += raw.chars().count();
        if index != last {
            base += 1;
        }
    }
    out
}

fn wrap_segment(raw: &str, width: usize, base: usize) -> Vec<WrapLine> {
    if raw.is_empty() {
        return vec![WrapLine {
            text: String::new(),
            start: base,
            end: base,
        }];
    }
    let chars: Vec<(char, usize)> = raw
        .chars()
        .enumerate()
        .map(|(offset, ch)| (ch, base + offset))
        .collect();
    let mut lines: Vec<WrapLine> = Vec::new();
    let mut pending_line: Vec<(char, usize)> = Vec::new();
    let mut line_width = 0usize;
    let mut pending_word: Vec<(char, usize)> = Vec::new();
    let mut word_width = 0usize;
    let mut pending_ws: Vec<(char, usize)> = Vec::new();
    let mut ws_width = 0usize;
    let mut non_ws_prev = false;

    fn flush(line: &mut Vec<(char, usize)>, lines: &mut Vec<WrapLine>, base: usize) {
        if line.is_empty() {
            return;
        }
        let start = line.first().map(|&(_, index)| index).unwrap_or(base);
        let end = line.last().map(|&(_, index)| index + 1).unwrap_or(start);
        lines.push(WrapLine {
            text: line.iter().map(|&(ch, _)| ch).collect(),
            start,
            end,
        });
        line.clear();
    }

    for &(ch, index) in &chars {
        let is_ws = ch.is_whitespace();
        let word_found = non_ws_prev && is_ws;
        let untrimmed_overflow = pending_line.is_empty() && word_width + ws_width + 1 > width;

        if word_found || untrimmed_overflow {
            pending_line.append(&mut pending_ws);
            line_width += ws_width;
            ws_width = 0;
            pending_line.append(&mut pending_word);
            line_width += word_width;
            word_width = 0;
        }

        let line_full = line_width >= width;
        let pending_word_overflow = line_width + ws_width + word_width >= width;

        if line_full || pending_word_overflow {
            let mut remaining = width.saturating_sub(line_width);
            flush(&mut pending_line, &mut lines, base);
            line_width = 0;
            while !pending_ws.is_empty() && remaining > 0 {
                ws_width = ws_width.saturating_sub(1);
                remaining -= 1;
                pending_ws.remove(0);
            }
            if is_ws && pending_ws.is_empty() {
                continue;
            }
        }

        if is_ws {
            ws_width += 1;
            pending_ws.push((ch, index));
        } else {
            word_width += 1;
            pending_word.push((ch, index));
        }
        non_ws_prev = !is_ws;
    }

    pending_line.append(&mut pending_ws);
    pending_line.append(&mut pending_word);
    flush(&mut pending_line, &mut lines, base);
    if lines.is_empty() {
        lines.push(WrapLine {
            text: String::new(),
            start: base,
            end: base,
        });
    }
    lines
}

fn wrap(text: &str, width: usize) -> Vec<String> {
    wrap_layout(text, width)
        .into_iter()
        .map(|line| line.text)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Mode;

    #[test]
    fn input_rows_grows_and_clamps() {
        assert_eq!(input_rows("", 10), MIN_INPUT_ROWS);
        assert_eq!(input_rows("hello", 10), MIN_INPUT_ROWS);
        assert_eq!(input_rows("hello\nworld", 10), 2);
        assert_eq!(input_rows(&"a".repeat(200), 10), MAX_INPUT_ROWS);
    }

    #[test]
    fn progress_bar_keeps_its_width_as_it_sweeps() {
        let start = progress_bar(std::time::Duration::ZERO);
        let later = progress_bar(std::time::Duration::from_millis(500));
        assert_eq!(start.chars().count(), later.chars().count());
        assert!(start.chars().all(|ch| ch == '█' || ch == '░'));
        assert_ne!(start, later);
    }

    #[test]
    fn progress_item_renders_a_bar_and_label() {
        let mut lines = Vec::new();
        render_item(
            &ChatItem::Progress {
                label: "Loading plugins".into(),
                since: std::time::Instant::now(),
            },
            80,
            false,
            &mut lines,
        );
        let text: String = lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .map(|span| span.content.as_ref())
            .collect();
        assert!(text.contains("Loading plugins"));
        assert!(text.contains('█') || text.contains('░'));
    }

    #[test]
    fn input_scroll_follows_cursor() {
        assert_eq!(input_scroll("hi", 2, 10), 0);
        assert_eq!(
            input_scroll(&"a".repeat(200), 200, 10),
            (20 - MAX_INPUT_ROWS) as u16
        );
        assert_eq!(input_scroll(&"a".repeat(200), 5, 10), 0);
    }

    #[test]
    fn input_cursor_position_handles_wrapping_and_newlines() {
        assert_eq!(input_cursor_position("hello", 2, 10), (0, 2));
        assert_eq!(input_cursor_position("hello\nworld", 8, 10), (1, 2));
        assert_eq!(input_cursor_position("abcdefghijk", 11, 5), (2, 1));
    }

    #[test]
    fn whitespace_only_input_stays_on_one_line() {
        assert_eq!(wrap(" ", 10), vec![" "]);
        assert_eq!(wrap("   ", 10), vec!["   "]);
        assert_eq!(input_rows(" ", 10), MIN_INPUT_ROWS);
        assert_eq!(input_cursor_position(" ", 1, 10), (0, 1));
        assert_eq!(input_cursor_position("  ", 2, 10), (0, 2));
    }

    #[test]
    fn render_cache_rebuilds_only_from_the_dirty_item() {
        let mut app = App::new("model".into(), "/tmp".into(), Mode::Build, Reasoning::Auto);
        app.items.push(ChatItem::User("first".into()));
        app.push_assistant_delta("second".into());
        sync_lines(&mut app, 40);
        let first_offset = app.line_offsets[0];

        app.push_assistant_delta(" updated".into());
        assert_eq!(app.render_dirty_from, Some(1));
        sync_lines(&mut app, 40);

        assert_eq!(app.line_offsets[0], first_offset);
        assert!(app
            .lines
            .iter()
            .any(|line| line_text(line).contains("updated")));
        assert_eq!(app.render_dirty_from, None);
    }

    #[test]
    fn message_position_at_maps_cells_to_absolute_lines() {
        let mut app = App::new("model".into(), "/tmp".into(), Mode::Build, Reasoning::Auto);
        app.scroll = 4;
        let area = Rect {
            x: 0,
            y: 0,
            width: 40,
            height: 24,
        };
        let areas = main_areas(area, &app);
        let message_area = areas[0];
        let inner_y = message_area.y + MESSAGE_TOP_PAD;
        assert_eq!(message_position_at(&app, area, 0, inner_y), Some((4, 0)));
        assert_eq!(
            message_position_at(&app, area, 3, inner_y + 2),
            Some((6, 2))
        );
        assert_eq!(message_position_at(&app, area, 0, message_area.y), None);
        assert_eq!(
            message_position_at(&app, area, 999, inner_y),
            Some((4, usize::from(message_area.width - 2 - 1)))
        );
    }

    #[test]
    fn styled_spans_truncate_from_the_right() {
        let spans = truncate_spans(vec![Span::raw("left"), Span::raw("-right")], 8);
        let text: String = spans.iter().map(|span| span.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 8);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 3), "he…");
        assert_eq!(truncate("hello", 0), "");
        assert_eq!(truncate("héllo", 2), "h…");
    }

    #[test]
    fn path_truncation_keeps_the_distinguishing_tail() {
        assert_eq!(truncate_path("src/main.rs", 20), "src/main.rs");
        let a = truncate_path(
            "libs/shared/landing-page/src/features/topDestinations/index.tsx",
            24,
        );
        let b = truncate_path("libs/shared/landing-page/src/features/hero/index.tsx", 24);
        assert!(a.starts_with('…'), "{a:?}");
        assert!(a.ends_with("index.tsx"), "{a:?}");
        assert_ne!(a, b);
        assert!(a.chars().count() <= 24);
    }

    #[test]
    fn composer_highlights_only_mentions() {
        let theme = crate::theme::Theme::dark();
        let spans = mention_spans("read @src/main.rs and @x", &theme);
        let mentions: Vec<&str> = spans
            .iter()
            .filter(|span| span.style.add_modifier.contains(Modifier::UNDERLINED))
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(mentions, vec!["@src/main.rs", "@x"]);
    }

    #[test]
    fn composer_text_keeps_newlines_and_mentions() {
        let theme = crate::theme::Theme::dark();
        let text = composer_text("read @a\n@b", &theme);
        assert_eq!(text.lines.len(), 2);
        let mentions: Vec<&str> = text
            .lines
            .iter()
            .flat_map(|line| line.spans.iter())
            .filter(|span| span.style.add_modifier.contains(Modifier::UNDERLINED))
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(mentions, vec!["@a", "@b"]);
    }

    #[test]
    fn banner_stacks_wordmark_above_info() {
        let info = vec![
            "Build things.".to_string(),
            "1 agent · 0 plugins".to_string(),
        ];
        let mut wide = Vec::new();
        render_banner(80, &info, &mut wide);
        let head: Vec<String> = wide
            .iter()
            .take(BANNER.len())
            .map(|line| line_text(line).trim().to_string())
            .collect();
        let art: Vec<String> = BANNER.iter().map(|art| art.trim().to_string()).collect();
        assert_eq!(head, art);
        let info_text: Vec<String> = wide.iter().skip(BANNER.len() + 1).map(line_text).collect();
        assert_eq!(info_text, vec!["Build things.", "", "1 agent · 0 plugins"]);
        for line in &wide {
            assert!(line_text(line).chars().count() <= 80);
        }

        let mut medium = Vec::new();
        render_banner(60, &info, &mut medium);
        assert!(medium.len() > BANNER.len());
        for line in &medium {
            assert!(line_text(line).chars().count() <= 60);
        }

        let mut narrow = Vec::new();
        render_banner(10, &info, &mut narrow);
        assert_eq!(line_text(&narrow[0]), "oxide");
        assert!(narrow.len() > 1);
    }

    #[test]
    fn file_tool_args_are_abbreviated() {
        let write = r#"{"path":"src/main.rs","content":"a\nb\nc"}"#;
        assert_eq!(
            tool_arg_summary("write_file", write),
            "src/main.rs · 3 lines"
        );
        let read = r#"{"path":"src/main.rs","offset":10}"#;
        assert_eq!(
            tool_arg_summary("read_file", read),
            "src/main.rs · from line 10"
        );
        assert_eq!(tool_arg_summary("bash", "ls -la"), "ls -la");
    }

    #[test]
    fn display_path_abbreviates_home() {
        if let Some(home) = dirs::home_dir() {
            let home = home.to_string_lossy();
            assert_eq!(display_path(&home), "~");
            assert_eq!(display_path(&format!("{home}/projects/x")), "~/projects/x");
        }
        assert_eq!(display_path("/tmp/other"), "/tmp/other");
    }

    #[test]
    fn wrap_prefers_word_boundaries() {
        assert_eq!(wrap("the quick brown fox", 9), ["the quick", "brown fox"]);
        assert_eq!(wrap("a".repeat(25).as_str(), 10).len(), 3);
    }

    #[test]
    fn cursor_matches_wrapped_line_breaks() {
        let text = "Replace TopDestinations component with PackagesCards component and move data \
                    fetching inside topDestinations into packagesCards so that PackagesCards can \
                    display the same data for packageHolidaysInCountry/Region page type @topDestinations";
        let (row, column) = input_cursor_position(text, text.len(), 153);
        assert_eq!((row, column), (1, 84));
    }

    #[test]
    fn wrapped_tool_output_is_hanging_indented() {
        let output = "src/main.rs:42: let value = some_long_function_name(argument_one, argument_two, argument_three);";
        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "grep".into(),
                args: r#"{"pattern":"value"}"#.into(),
                output: output.into(),
                diff: None,
                millis: 0,
            },
            60,
            true,
            &mut lines,
        );
        for line in &lines {
            assert!(
                line_text(line).chars().count() <= 60,
                "{:?}",
                line_text(line)
            );
        }
        let continuations = lines
            .iter()
            .map(line_text)
            .filter(|text| text.starts_with("   ") && !text.trim().is_empty())
            .count();
        assert!(
            continuations >= 1,
            "expected a hanging-indented continuation"
        );
    }

    #[test]
    fn listing_aligns_status_columns_and_wraps_details() {
        let theme = crate::theme::Theme::dark();
        let rows = vec![
            ListRow::new("atlassian")
                .status("Connected", Tone::Success)
                .detail("remote: https://mcp.atlassian.com/v1/mcp"),
            ListRow::new("a-very-long-server-name")
                .status("Needs Auth", Tone::Warning)
                .detail("remote: https://mcp.example.com/mcp"),
            ListRow::new("local")
                .status("Disabled", Tone::Dim)
                .note("local: npx some-server --with-a-fairly-long-argument-list"),
        ];

        let mut wide = Vec::new();
        render_listing("MCP servers (3)", &rows, 120, &theme, &mut wide);
        assert_eq!(line_text(&wide[0]), "· MCP servers (3)");
        assert_eq!(wide[0].spans.last().unwrap().style.fg, Some(theme.info));
        let connected = wide
            .iter()
            .find(|line| line_text(line).contains("Connected"))
            .expect("connected row");
        let needs_auth = wide
            .iter()
            .find(|line| line_text(line).contains("Needs Auth"))
            .expect("needs-auth row");
        assert_eq!(
            line_text(connected).find("Connected"),
            line_text(needs_auth).find("Needs Auth")
        );
        let tone_of = |needle: &str| {
            wide.iter()
                .flat_map(|line| line.spans.iter())
                .find(|span| span.content.trim() == needle)
                .map(|span| span.style.fg)
                .unwrap()
        };
        assert_eq!(tone_of("Connected"), Some(theme.success));
        assert_eq!(tone_of("Needs Auth"), Some(theme.tool));
        assert_eq!(tone_of("Disabled"), Some(theme.dim));

        let mut narrow = Vec::new();
        render_listing("MCP servers (3)", &rows, 48, &theme, &mut narrow);
        for line in &narrow {
            assert!(
                line_text(line).chars().count() <= 48,
                "{:?}",
                line_text(line)
            );
        }
        assert!(
            narrow.iter().any(|line| {
                let text = line_text(line);
                text.starts_with("    ") && !text.trim().is_empty()
            }),
            "expected a hanging-indented note"
        );
    }

    #[test]
    fn compact_json_tool_output_is_pretty_printed() {
        let json = r#"{"id":4056548378,"path":"a/b.java","position":38}"#;
        let pretty = readable_output(json);
        assert!(pretty.contains("{\n  \"id\": 4056548378,"), "{pretty}");
        assert!(pretty.contains("\n  \"path\": \"a/b.java\","), "{pretty}");

        assert_eq!(readable_output("plain text"), "plain text");
        assert_eq!(readable_output("{not json}"), "{not json}");
    }

    #[test]
    fn streaming_thinking_block_shows_its_body_italic() {
        let theme = crate::theme::Theme::dark();
        let mut lines = Vec::new();
        render_item_themed(
            &ChatItem::Thinking {
                text: "because the file moved".into(),
                millis: None,
            },
            80,
            true,
            true,
            &theme,
            None,
            &mut lines,
        );
        assert_eq!(line_text(&lines[0]), "✦ Thinking");
        assert_eq!(line_text(&lines[1]), "  because the file moved");
        assert_eq!(lines[0].spans[1].style.fg, Some(theme.thinking_text));
        assert!(lines[1]
            .spans
            .iter()
            .any(|span| span.style.add_modifier.contains(Modifier::ITALIC)));
    }

    #[test]
    fn finished_thinking_block_shows_a_duration() {
        let mut lines = Vec::new();
        render_item(
            &ChatItem::Thinking {
                text: "reasoning".into(),
                millis: Some(2400),
            },
            80,
            true,
            &mut lines,
        );
        assert_eq!(line_text(&lines[0]), "✦ Thought for 2.4s");
    }

    #[test]
    fn hidden_thinking_block_collapses_to_a_hint() {
        let theme = crate::theme::Theme::dark();
        let mut lines = Vec::new();
        render_item_themed(
            &ChatItem::Thinking {
                text: "because the file moved".into(),
                millis: Some(1500),
            },
            80,
            true,
            false,
            &theme,
            None,
            &mut lines,
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(
            line_text(&lines[0]),
            "✦ Thought for 1.5s · Ctrl+T to expand"
        );
    }

    #[test]
    fn empty_thinking_block_renders_only_its_label() {
        let theme = crate::theme::Theme::dark();
        let mut lines = Vec::new();
        render_item_themed(
            &ChatItem::Thinking {
                text: "  ".into(),
                millis: None,
            },
            80,
            true,
            true,
            &theme,
            None,
            &mut lines,
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "✦ Thinking");
    }

    #[test]
    fn thinking_body_wraps_within_the_width() {
        let mut lines = Vec::new();
        render_item(
            &ChatItem::Thinking {
                text: "a long stretch of reasoning that must wrap onto several narrow lines".into(),
                millis: None,
            },
            30,
            true,
            &mut lines,
        );
        for line in &lines {
            assert!(
                line_text(line).chars().count() <= 30,
                "{:?}",
                line_text(line)
            );
        }
        assert!(lines.len() > 2);
    }

    fn line_text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
    }

    /// The first non-empty row of a tool panel, stripped of background-fill
    /// padding. Tool headers now live inside the panel, not above it.
    fn panel_line(lines: &[Line]) -> String {
        lines
            .iter()
            .map(line_text)
            .find(|text| !text.trim().is_empty())
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// A panel's visible rows joined, ignoring background-fill padding rows.
    fn panel_text(lines: &[Line]) -> String {
        lines
            .iter()
            .map(line_text)
            .filter(|text| !text.trim().is_empty())
            .map(|text| text.trim().to_string())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn file_tools_render_concise_actions() {
        let args = r#"{"path":"src/main.rs"}"#;

        let mut lines = Vec::new();
        render_item(
            &ChatItem::Tool {
                name: "read_file".into(),
                args: args.into(),
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Read src/main.rs");

        let mut lines = Vec::new();
        render_item(
            &ChatItem::Tool {
                name: "write_file".into(),
                args: args.into(),
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Edit src/main.rs");

        let long_read = (1..=20)
            .map(|index| format!("{index:>6}\tline {index}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "read_file".into(),
                args: args.into(),
                output: long_read,
                diff: None,
                millis: 0,
            },
            80,
            false,
            &mut lines,
        );
        let text = panel_text(&lines);
        assert!(text.starts_with("→ Read src/main.rs"));
        assert!(text.contains("line 1"), "{text}");
        assert!(!text.contains("line 11"), "{text}");
        assert!(text.contains("Ctrl+O to expand"), "{text}");

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "read_file".into(),
                args: args.into(),
                output: "     1\tfn main() {}".into(),
                diff: None,
                millis: 0,
            },
            80,
            true,
            &mut lines,
        );
        assert!(panel_text(&lines).starts_with("→ Read src/main.rs"));
        assert!(panel_text(&lines).contains("fn main() {}"));

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "write_file".into(),
                args: args.into(),
                output: "wrote 12 bytes to /x".into(),
                diff: None,
                millis: 0,
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Edited src/main.rs");
    }

    #[test]
    fn speaker_labels_render_inline_with_message_text() {
        let mut lines = Vec::new();
        render_item(&ChatItem::User("hello there".into()), 80, false, &mut lines);
        assert_eq!(line_text(&lines[0]), "❯ you hello there");

        let mut lines = Vec::new();
        render_item(
            &ChatItem::Assistant("here is the answer".into()),
            80,
            false,
            &mut lines,
        );
        assert_eq!(line_text(&lines[0]), "◆ oxide here is the answer");
    }

    #[test]
    fn assistant_messages_render_markdown_blocks() {
        let mut lines = Vec::new();
        render_item(
            &ChatItem::Assistant("## Summary\n\n- **bold** item\n\n```\nsome code\n```".into()),
            60,
            false,
            &mut lines,
        );
        let text: Vec<String> = lines.iter().map(line_text).collect();
        assert_eq!(text[0], "◆ oxide Summary");
        assert!(text.iter().any(|line| line.contains("• bold item")));
        assert!(text.iter().any(|line| line.contains("some code")));
        for line in &text {
            assert!(line.chars().count() <= 60, "{line:?}");
        }
    }

    #[test]
    fn compaction_renders_summary_and_files() {
        let mut lines = Vec::new();
        render_item(
            &ChatItem::Compaction {
                summary: "Ship the release".into(),
                summarized: 12,
                tokens_before: 48_000,
                read_files: vec!["src/a.rs".into()],
                modified_files: vec!["src/b.rs".into()],
            },
            80,
            true,
            &mut lines,
        );
        let text: String = lines.iter().map(line_text).collect::<Vec<_>>().join("\n");
        assert!(text.contains("Compacted 12 messages"));
        assert!(text.contains("48.0k"));
        assert!(text.contains("Ship the release"));
        assert!(text.contains("read: src/a.rs"));
        assert!(text.contains("modified: src/b.rs"));
    }

    #[test]
    fn bash_renders_command_and_result() {
        let args = r#"{"command":"cargo   test\n--all"}"#;

        let mut lines = Vec::new();
        render_item(
            &ChatItem::Tool {
                name: "bash".into(),
                args: args.into(),
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Run cargo test --all");

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "bash".into(),
                args: args.into(),
                output: "ok\n[exit: 0]".into(),
                diff: None,
                millis: 0,
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Ran cargo test --all · exit 0");
        assert_eq!(lines.len(), 5);
        assert_eq!(lines[1].spans[1].style.fg, Some(Color::LightGreen));

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "bash".into(),
                args: args.into(),
                output: "boom\n[exit: 1]".into(),
                diff: None,
                millis: 0,
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Run failed cargo test --all · exit 1");
        assert_eq!(lines[1].spans[1].style.fg, Some(Color::LightRed));
    }

    #[test]
    fn long_bash_command_wraps_to_show_full_text() {
        let command = format!("echo {}", "a".repeat(80));
        let args = format!(r#"{{"command":"{command}"}}"#);

        let mut lines = Vec::new();
        render_item(
            &ChatItem::Tool {
                name: "bash".into(),
                args,
            },
            40,
            false,
            &mut lines,
        );
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line_text(line).chars().count() <= 40, "{line:?}");
        }
        let text = panel_text(&lines);
        assert!(text.starts_with("→ Run echo"));
        assert_eq!(text.matches('a').count(), 80);
    }

    #[test]
    fn long_bash_action_hangs_indented() {
        let command = format!("echo {}", "a".repeat(80));
        let args = format!(r#"{{"command":"{command}"}}"#);
        let mut lines = Vec::new();
        render_item(
            &ChatItem::Tool {
                name: "bash".into(),
                args,
            },
            40,
            false,
            &mut lines,
        );
        let rows: Vec<String> = lines.iter().map(line_text).collect();
        for row in &rows {
            assert!(row.chars().count() <= 40, "{row:?}");
        }
        // One panel leading space plus the six-column `→ Run ` prefix.
        assert!(
            rows.iter().any(|row| row.starts_with("       a")),
            "expected a hanging-indented continuation: {rows:?}"
        );
    }

    #[test]
    fn generic_tool_wraps_arguments() {
        let args = format!(r#"{{"pattern":"{}"}}"#, "x".repeat(60));
        let mut lines = Vec::new();
        render_item(
            &ChatItem::Tool {
                name: "grep".into(),
                args,
            },
            30,
            false,
            &mut lines,
        );
        assert!(lines.len() > 1);
        for line in &lines {
            assert!(line_text(line).chars().count() <= 30, "{line:?}");
        }
        let text = panel_text(&lines);
        assert!(text.starts_with("⚙ grep"));
        assert_eq!(text.matches('x').count(), 60);
    }

    #[test]
    fn tool_output_preview_budget_depends_on_the_tool() {
        let output = (0..25)
            .map(|index| format!("row-{index:02}"))
            .collect::<Vec<_>>()
            .join("\n");

        // Shell output previews its tail so the newest lines and errors show.
        let mut bash = Vec::new();
        render_item(
            &ChatItem::ToolProgress {
                name: "bash".into(),
                output: output.clone(),
            },
            80,
            false,
            &mut bash,
        );
        let text = panel_text(&bash);
        assert!(text.contains("row-24"), "{text}");
        assert!(text.contains("row-20"), "{text}");
        assert!(!text.contains("row-19"), "{text}");
        assert!(text.contains("earlier lines"), "{text}");
        assert!(text.contains("Ctrl+O to expand"), "{text}");

        // Searches preview the first lines, with a larger budget than a shell.
        let mut grep = Vec::new();
        render_item(
            &ChatItem::ToolProgress {
                name: "grep".into(),
                output: output.clone(),
            },
            80,
            false,
            &mut grep,
        );
        let text = panel_text(&grep);
        assert!(text.contains("row-00"), "{text}");
        assert!(text.contains("row-14"), "{text}");
        assert!(!text.contains("row-15"), "{text}");
        assert!(text.contains("Ctrl+O to expand"), "{text}");

        let mut expanded = Vec::new();
        render_item(
            &ChatItem::ToolProgress {
                name: "bash".into(),
                output,
            },
            80,
            true,
            &mut expanded,
        );
        let text = panel_text(&expanded);
        assert!(text.contains("row-00"), "{text}");
        assert!(text.contains("row-24"), "{text}");
        assert!(!text.contains("Ctrl+O to expand"), "{text}");
    }

    #[test]
    fn edit_result_renders_colored_diff() {
        let diff = DiffPreview {
            path: "src/main.rs".into(),
            text: "   1   1  a\n-  2      b\n+      2  B".into(),
        };
        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "write_file".into(),
                args: r#"{"path":"src/main.rs"}"#.into(),
                output: "wrote 12 bytes to /x".into(),
                diff: Some(diff),
                millis: 0,
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Edited src/main.rs");
        assert_eq!(
            lines[1].spans[0].style.bg,
            Some(Color::Rgb(0x28, 0x32, 0x28))
        );
        assert_eq!(lines[3].spans[1].style.fg, Some(Color::Gray));
        assert_eq!(lines[4].spans[1].style.fg, Some(Color::LightRed));
        assert_eq!(lines[5].spans[1].style.fg, Some(Color::LightGreen));
        assert_eq!(
            lines[6].spans[0].style.bg,
            Some(Color::Rgb(0x28, 0x32, 0x28))
        );
    }

    #[test]
    fn tool_body_renders_as_a_background_panel() {
        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "bash".into(),
                args: r#"{"command":"echo hi"}"#.into(),
                output: "hi\n[exit: 0]".into(),
                diff: None,
                millis: 0,
            },
            80,
            true,
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Ran echo hi · exit 0");
        assert_eq!(
            lines[1].spans[0].style.bg,
            Some(Color::Rgb(0x28, 0x32, 0x28))
        );
        assert_eq!(lines[3].spans[1].content.as_ref(), "hi");
        assert_eq!(
            lines[4].spans[0].style.bg,
            Some(Color::Rgb(0x28, 0x32, 0x28))
        );
        for line in &lines {
            assert!(
                line_text(line).chars().count() <= 80,
                "{:?}",
                line_text(line)
            );
        }

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "grep".into(),
                args: r#"{"pattern":"x"}"#.into(),
                output: "match".into(),
                diff: None,
                millis: 0,
            },
            80,
            true,
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "↳ grep");
        assert_eq!(
            lines[1].spans[0].style.bg,
            Some(Color::Rgb(0x28, 0x32, 0x28))
        );
        assert!(line_text(&lines[2]).trim().is_empty());
        assert_eq!(lines[3].spans[1].content.as_ref(), "match");
        assert_eq!(
            lines[4].spans[0].style.bg,
            Some(Color::Rgb(0x28, 0x32, 0x28))
        );
    }

    #[test]
    fn panel_backgrounds_stay_on_their_text_rows() {
        use crate::config::{Mode, Reasoning};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new("m".into(), "/tmp".into(), Mode::Build, Reasoning::Auto);
        app.items.push(ChatItem::ToolResult {
            name: "grep".into(),
            args: r#"{"pattern":"x"}"#.into(),
            output: "match".into(),
            diff: None,
            millis: 0,
        });
        let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let bg = Some(Color::Rgb(0x28, 0x32, 0x28));

        // Without the phantom wrapping row, the header and body sit two rows
        // apart and both carry the panel background on the same row.
        let header = row_of(buffer, "↳ grep").expect("header row");
        let body = row_of(buffer, "match").expect("body row");
        assert_eq!(body, header + 2);
        assert_eq!(buffer[(1, header)].style().bg, bg);
        assert_eq!(buffer[(1, body)].style().bg, bg);
    }

    #[test]
    fn bash_call_and_result_show_timeout_and_duration() {
        let args = r#"{"command":"cargo test","timeout":420000}"#;
        let mut call = Vec::new();
        render_item(
            &ChatItem::Tool {
                name: "bash".into(),
                args: args.into(),
            },
            80,
            false,
            &mut call,
        );
        assert_eq!(panel_line(&call), "→ Run cargo test (timeout 420s)");

        let mut result = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "bash".into(),
                args: args.into(),
                output: "ok\n[exit: 0]".into(),
                diff: None,
                millis: 1234,
            },
            80,
            true,
            &mut result,
        );
        assert_eq!(
            panel_line(&result),
            "→ Ran cargo test (timeout 420s) · exit 0"
        );
        assert!(
            result
                .iter()
                .any(|line| line_text(line).contains("Took 1.2s")),
            "{:?}",
            result.iter().map(line_text).collect::<Vec<_>>()
        );
        let took = result
            .iter()
            .find(|line| line_text(line).contains("Took 1.2s"))
            .expect("Took line");
        assert_eq!(took.spans[1].style.bg, Some(Color::Rgb(0x28, 0x32, 0x28)));
    }

    #[test]
    fn slow_non_shell_tools_report_a_duration() {
        let mut slow = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "grep".into(),
                args: r#"{"pattern":"x"}"#.into(),
                output: "no matches".into(),
                diff: None,
                millis: 900,
            },
            80,
            true,
            &mut slow,
        );
        assert!(
            slow.iter()
                .any(|line| line_text(line).contains("Took 900ms")),
            "{:?}",
            slow.iter().map(line_text).collect::<Vec<_>>()
        );

        let mut fast = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "grep".into(),
                args: r#"{"pattern":"x"}"#.into(),
                output: "no matches".into(),
                diff: None,
                millis: 40,
            },
            80,
            true,
            &mut fast,
        );
        assert!(
            !fast.iter().any(|line| line_text(line).contains("Took")),
            "{:?}",
            fast.iter().map(line_text).collect::<Vec<_>>()
        );
    }

    #[test]
    fn formats_tool_durations() {
        assert_eq!(format_duration(0), "0ms");
        assert_eq!(format_duration(420), "420ms");
        assert_eq!(format_duration(1200), "1.2s");
        assert_eq!(format_duration(12000), "12s");
    }

    fn running<'a>(name: &'a str, millis: u64) -> Running<'a> {
        Running {
            name,
            elapsed: std::time::Duration::from_millis(millis),
            subagent: None,
        }
    }

    #[test]
    fn running_task_shows_elapsed_and_the_subagent_activity() {
        let theme = crate::theme::Theme::dark();
        let state = SubagentState {
            agent: "rust-reviewer".into(),
            tool: "grep".into(),
            args: r#"{"pattern":"fn resolve_tool"}"#.into(),
            tools: 7,
        };
        let mut lines = Vec::new();
        render_item_themed(
            &ChatItem::Tool {
                name: "task".into(),
                args: r#"{"prompt":"review the diff","subagent_type":"rust-reviewer"}"#.into(),
            },
            100,
            false,
            true,
            &theme,
            Some(Running {
                name: "task",
                elapsed: std::time::Duration::from_millis(42_000),
                subagent: Some(&state),
            }),
            &mut lines,
        );
        let text: Vec<String> = lines.iter().map(line_text).collect();
        assert!(
            text.iter().any(|line| line.contains("Elapsed 42s")),
            "{text:?}"
        );
        let activity = text
            .iter()
            .find(|line| line.contains('↳'))
            .unwrap_or_else(|| panic!("no activity line in {text:?}"));
        assert!(activity.contains("rust-reviewer"), "{activity}");
        assert!(activity.contains("fn resolve_tool"), "{activity}");
        assert!(activity.contains("7 call(s)"), "{activity}");
    }

    #[test]
    fn the_running_tool_panel_ticks_its_elapsed() {
        let mut app = App::new("m".into(), "/tmp".into(), Mode::Build, Reasoning::Auto);
        app.items.push(ChatItem::Tool {
            name: "task".into(),
            args: r#"{"prompt":"review","subagent_type":"rust-reviewer"}"#.into(),
        });
        app.running_tool = Some(("task".into(), std::time::Instant::now()));
        sync_lines(&mut app, 100);
        assert!(
            panel_line(&app.lines).contains("Elapsed 0ms"),
            "{:?}",
            panel_line(&app.lines)
        );

        // What the 500ms ticker does: age the start time, mark the panel dirty,
        // and re-render it.
        let started = std::time::Instant::now() - std::time::Duration::from_secs(3);
        app.running_tool = Some(("task".into(), started));
        app.mark_running_tool_dirty();
        sync_lines(&mut app, 100);
        assert!(
            panel_line(&app.lines).contains("Elapsed 3s"),
            "{:?}",
            panel_line(&app.lines)
        );
    }

    #[test]
    fn a_status_tip_renders_dim_without_a_bullet() {
        let mut lines = Vec::new();
        render_item_themed(
            &ChatItem::Status("copied 12 chars".into()),
            80,
            false,
            true,
            &crate::theme::Theme::dark(),
            None,
            &mut lines,
        );
        assert_eq!(lines.len(), 1);
        assert_eq!(line_text(&lines[0]), "copied 12 chars");
        let style = lines[0].spans[0].style;
        assert!(style.add_modifier.contains(Modifier::DIM));
    }

    #[test]
    fn a_running_tool_with_queued_messages_advertises_the_dequeue_key() {
        let mut app = App::new("m".into(), "/tmp".into(), Mode::Build, Reasoning::Auto);
        app.busy = true;
        app.busy_since = Some(std::time::Instant::now());
        app.status = "thinking...".into();
        app.follow_ups
            .push(crate::llm::Message::user("one more thing"));

        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 12)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let row: String = buffer
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .chunks(120)
            .map(|row| row.concat())
            .find(|row| row.contains("queued"))
            .expect("a queued hint row");
        assert!(row.contains("1 queued"), "{row}");
        assert!(row.contains(crate::tui::dequeue_key_label()), "{row}");
    }

    #[test]
    fn a_finished_tool_has_no_elapsed_or_activity() {
        let mut lines = Vec::new();
        render_item_themed(
            &ChatItem::Tool {
                name: "task".into(),
                args: r#"{"prompt":"x","subagent_type":"rust-reviewer"}"#.into(),
            },
            100,
            false,
            true,
            &crate::theme::Theme::dark(),
            None,
            &mut lines,
        );
        let text: Vec<String> = lines.iter().map(line_text).collect();
        assert!(
            !text.iter().any(|line| line.contains("Elapsed")),
            "{text:?}"
        );
        assert!(!text.iter().any(|line| line.contains('↳')), "{text:?}");
    }

    #[test]
    fn running_bash_shows_live_elapsed() {
        let mut lines = Vec::new();
        render_item_themed(
            &ChatItem::Tool {
                name: "bash".into(),
                args: r#"{"command":"sleep 30"}"#.into(),
            },
            80,
            false,
            true,
            &crate::theme::Theme::dark(),
            Some(running("bash", 2400)),
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Run sleep 30 · Elapsed 2.4s");

        // A different tool's elapsed must not leak onto this call.
        let mut lines = Vec::new();
        render_item_themed(
            &ChatItem::Tool {
                name: "bash".into(),
                args: r#"{"command":"ls"}"#.into(),
            },
            80,
            false,
            true,
            &crate::theme::Theme::dark(),
            Some(running("grep", 2400)),
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Run ls");
    }

    fn row_of(buffer: &ratatui::buffer::Buffer, needle: &str) -> Option<u16> {
        (0..buffer.area.height).find(|y| {
            let row: String = (0..buffer.area.width)
                .map(|x| buffer[(x, *y)].symbol())
                .collect();
            row.contains(needle)
        })
    }

    #[test]
    fn model_picker_matches_pi_layout() {
        use crate::config::{Mode, Reasoning};
        use crate::tui::app::{ModelChoice, ModelsState};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(
            "deepseek-flash".into(),
            "/tmp/project".into(),
            Mode::Build,
            Reasoning::Auto,
        );
        app.models = Some(ModelsState::ready(
            vec![
                ModelChoice::new("deepseek", "deepseek-flash"),
                ModelChoice::new("deepseek", "deepseek-v4-pro"),
                ModelChoice::new("portkey", "deepseek-flash"),
            ],
            "deepseek".to_string(),
            "deepseek-flash".to_string(),
            Some("deepseek-v4-pro".to_string()),
        ));
        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        let text: String = (0..buffer.area.height)
            .map(|y| {
                let row: String = (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect();
                format!("{row}\n")
            })
            .collect();

        assert!(text.contains("Models from every logged-in provider. Use /login to add more."));
        assert!(text.contains("deepseek-flash [deepseek]"));
        assert!(text.contains("deepseek-flash [portkey]"));
        assert!(text.contains("deepseek-v4-pro [deepseek] · default"));
        assert!(text.contains("Model Name: DeepSeek V4.1 Flash"));
        assert!(text.contains("Model catalogs refreshed."));
        assert!(
            text.contains("Enter to select · Ctrl+S to set as default · Escape/Ctrl+C to cancel")
        );
    }

    #[test]
    fn footer_token_format_matches_pi() {
        assert_eq!(format_tokens(999), "999");
        assert_eq!(format_tokens(1_200), "1.2k");
        assert_eq!(format_tokens(12_000), "12k");
        assert_eq!(format_tokens(107_800), "108k");
        assert_eq!(format_tokens(1_200_000), "1.2M");
        assert_eq!(format_tokens(12_000_000), "12M");
    }

    #[test]
    fn footer_truncates_with_three_dots() {
        assert_eq!(truncate_dots("abcdefgh", 5), "ab...");
        assert_eq!(truncate_dots("abc", 5), "abc");
    }

    #[test]
    fn footer_shows_cache_cost_and_extension_statuses() {
        use crate::config::{Mode, Reasoning};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(
            "gpt-4o".into(),
            "/tmp/project".into(),
            Mode::Build,
            Reasoning::Auto,
        );
        app.tokens_in = 1_000;
        app.tokens_out = 500;
        app.tokens_cache_read = 800;
        app.tokens_cache_write = 200;
        app.cache_hit_rate = Some(80.0);
        app.cost = 0.1234;
        app.context_used = 20;
        app.context_limit = 100;
        app.extension_statuses.insert("a".into(), "first".into());
        app.extension_statuses.insert("b".into(), "second".into());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let row = row_of(buffer, "CH80.0%").expect("cache hit rate");
        let footer: String = (0..buffer.area.width)
            .map(|x| buffer[(x, row)].symbol())
            .collect();
        assert!(footer.contains("R800"));
        assert!(footer.contains("W200"));
        assert!(footer.contains("$0.123"));
        assert!(row_of(buffer, "first second").is_some());
    }

    #[test]
    fn footer_stacks_project_above_stats_and_right_aligned_model() {
        use crate::config::{Mode, Reasoning};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(
            "gpt-4o".into(),
            "/tmp/project".into(),
            Mode::Build,
            Reasoning::Auto,
        );
        app.status = "ready".into();
        app.git_branch = Some("main".into());
        let mut terminal = Terminal::new(TestBackend::new(69, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let project_row = row_of(&buffer, "/tmp/project").expect("project row");
        let controls_row = project_row + 1;
        assert_eq!(row_of(&buffer, "(main)"), Some(project_row));
        assert_eq!(row_of(&buffer, "gpt-4o"), Some(controls_row));
        assert_eq!(row_of(&buffer, "auto"), Some(controls_row));
        // The composer's bottom rule sits directly above the footer.
        let border: String = (0..buffer.area.width)
            .map(|x| buffer[(x, project_row - 1)].symbol())
            .collect();
        assert_eq!(border, "─".repeat(buffer.area.width as usize));

        let text: String = (0..buffer.area.width)
            .map(|x| buffer[(x, controls_row)].symbol())
            .collect();
        assert!(text.trim_end().ends_with("gpt-4o • auto"));
        assert!(!text.contains("Enter send"));
        assert!(!text.contains("Ctrl+O"));
    }

    #[test]
    fn composer_rule_shows_working_status_without_duplicating_footer_usage() {
        use crate::config::{Mode, Reasoning};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(
            "gpt-4o".into(),
            "/tmp/project".into(),
            Mode::Build,
            Reasoning::Auto,
        );
        app.status = "ready".into();
        app.busy = true;
        app.busy_since = Some(std::time::Instant::now());
        app.tokens_in = 107_800;
        app.tokens_out = 4_800;
        app.context_used = 20;
        app.context_limit = 100;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        let input_row = row_of(buffer, "ready").expect("working status on the top rule");
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, input_row)].symbol())
            .collect();
        assert!(row.contains("ready"));
        assert!(row.contains("Esc clear · /exit quit"));
        assert!(!row.contains("108k"));
        assert!(!row.contains("20%"));

        let footer_row = row_of(buffer, "108k").expect("usage moves to the footer");
        let footer: String = (0..buffer.area.width)
            .map(|x| buffer[(x, footer_row)].symbol())
            .collect();
        assert!(footer.contains("↑108k"));
        assert!(footer.contains("↓4.8k"));
        assert!(footer.contains("20%/100"));
        assert!(footer.contains("(auto)"));
    }

    #[test]
    fn composer_keeps_its_caret_while_the_agent_is_busy() {
        use crate::config::{Mode, Reasoning};
        use ratatui::backend::{Backend, TestBackend};
        use ratatui::layout::Position;
        use ratatui::Terminal;

        // A multiline message long enough to scroll the composer, so the caret
        // checks follow the wrapped, scrolled position and not just column 0.
        let input = "first line\nsecond line\nthird line\nfourth line\nqueued steering";
        let caret = |busy: bool| {
            let mut app = App::new(
                "gpt-4o".into(),
                "/tmp/project".into(),
                Mode::Build,
                Reasoning::Auto,
            );
            app.input = input.into();
            app.input_cursor = app.input.len();
            app.busy = busy;
            app.busy_since = busy.then(std::time::Instant::now);
            // Each render gets a fresh backend, whose cursor starts at the
            // origin; a busy frame that places no caret would leave it there.
            let mut terminal = Terminal::new(TestBackend::new(60, 24)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            terminal.backend_mut().get_cursor_position().unwrap()
        };

        let idle = caret(false);
        let busy = caret(true);
        assert_ne!(busy, Position::ORIGIN);
        assert_eq!(busy, idle);
    }

    #[test]
    fn suggestion_hit_testing_tracks_the_visible_window() {
        use crate::config::{Mode, Reasoning};

        let mut app = App::new(
            "gpt-4o".into(),
            "/tmp/project".into(),
            Mode::Build,
            Reasoning::Auto,
        );
        app.set_input("/".to_string());
        app.suggestions = (0..10)
            .map(|index| crate::tui::app::CommandHint {
                name: format!("command-{index}"),
                description: String::new(),
            })
            .collect();
        app.suggestion_index = 9;
        let area = Rect::new(0, 0, 80, 24);

        assert_eq!(suggestion_index_at(&app, area, 3, 9), Some(2));
        assert_eq!(suggestion_index_at(&app, area, 3, 16), Some(9));
        assert_eq!(suggestion_index_at(&app, area, 0, 9), None);
    }

    #[test]
    fn suggestion_keeps_a_long_command_name_and_ellipsizes_its_description() {
        use crate::config::{Mode, Reasoning};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(
            "gpt-4o".into(),
            "/tmp/project".into(),
            Mode::Build,
            Reasoning::Auto,
        );
        app.set_input("/add".to_string());
        app.suggestions = vec![crate::tui::app::CommandHint {
            name: "add-editorial-cross-links-page-type".to_string(),
            description: "Add a new page type to the editorial cross-links feature. Use when the \
                          user wants to add a new page type for a specific vertical."
                .to_string(),
        }];
        let mut terminal = Terminal::new(TestBackend::new(120, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let title_row = row_of(buffer, "click or Tab to complete").expect("suggestion title");
        let item_row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, title_row + 1)].symbol())
            .collect();
        // The command name is shown in full; only the description is clipped,
        // and it ends with an ellipsis rather than mid-word.
        assert!(
            item_row.contains("/add-editorial-cross-links-page-type"),
            "{item_row}"
        );
        assert!(item_row.contains("Add a new page type"), "{item_row}");
        assert!(item_row.contains('…'), "{item_row}");
    }

    #[test]
    fn usage_bar_takes_the_bottom_row_when_enabled() {
        use crate::config::{Mode, Reasoning};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(
            "gpt-4o".into(),
            "/tmp/project".into(),
            Mode::Build,
            Reasoning::Auto,
        );
        app.cost = 0.0;
        let settings = crate::portkey_usage::UsageSettings {
            enabled: true,
            user: "firstname.lastname".into(),
            budget: Some(600.0),
            ..Default::default()
        };
        let mut bar = crate::portkey_usage::UsageBar::new(&settings);
        bar.apply(Ok(crate::portkey_usage::Snapshot {
            today: 20.61,
            month: 220.69,
        }));
        app.usage = Some(bar);

        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        let row = row_of(buffer, "Today: $20.61").expect("usage bar row");
        assert_eq!(row, buffer.area.height - 1);
        let text: String = (0..buffer.area.width)
            .map(|x| buffer[(x, row)].symbol())
            .collect();
        assert!(text.starts_with("→ firstname.lastname"));
        assert!(text.contains("Session: $0.00"));
        assert!(text.contains("Month: $220.69 / $600.00"));

        let theme = crate::theme::Theme::dark();
        for x in 0..buffer.area.width {
            assert_eq!(buffer[(x, row)].bg, theme.usage_bar_bg);
        }
    }

    #[test]
    fn usage_bar_is_absent_by_default() {
        use crate::config::{Mode, Reasoning};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(
            "gpt-4o".into(),
            "/tmp/project".into(),
            Mode::Build,
            Reasoning::Auto,
        );
        assert!(app.usage.is_none());
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        assert_eq!(row_of(terminal.backend().buffer(), "Session:"), None);
    }

    #[test]
    fn footer_omits_branch_outside_a_git_repo() {
        use crate::config::{Mode, Reasoning};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(
            "gpt-4o".into(),
            "/path/that/is/not/a/repository".into(),
            Mode::Build,
            Reasoning::Auto,
        );
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        assert_eq!(row_of(buffer, "(main)"), None);
    }

    #[test]
    fn marketplaces_overlay_renders_marketplace_and_plugins() {
        use crate::config::{Mode, Reasoning};
        use crate::plugin_registry::{MarketplaceOverview, MarketplacePluginOverview};
        use crate::tui::app::MarketplacesState;
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut app = App::new(
            "gpt-4o".into(),
            "/tmp/project".into(),
            Mode::Build,
            Reasoning::Auto,
        );
        app.marketplaces = Some(MarketplacesState::ready(vec![MarketplaceOverview {
            name: "skyscanner".into(),
            source: "https://github.com/Skyscanner/plugins".into(),
            path: "/tmp/marketplaces/skyscanner".into(),
            owner: Some("Skyscanner".into()),
            plugins: vec![
                MarketplacePluginOverview {
                    name: "onboarding-guide".into(),
                    description: Some("a guide".into()),
                    version: Some("1.0.0".into()),
                    installed: true,
                    enabled: true,
                },
                MarketplacePluginOverview {
                    name: "android-core".into(),
                    description: None,
                    version: None,
                    installed: false,
                    enabled: false,
                },
            ],
            error: None,
        }]));

        let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        assert!(row_of(buffer, "marketplaces").is_some(), "title");
        assert!(row_of(buffer, "skyscanner").is_some(), "marketplace row");
        assert!(row_of(buffer, "Plugins").is_some(), "plugins header");
        assert!(
            row_of(buffer, "onboarding-guide").is_some(),
            "installed plugin"
        );
        assert!(row_of(buffer, "android-core").is_some(), "available plugin");
        assert!(row_of(buffer, "Ctrl+A add").is_some(), "footer hint");

        // A cramped terminal must not panic in any of the sub-layouts.
        let mut small = Terminal::new(TestBackend::new(40, 12)).unwrap();
        small.draw(|frame| draw(frame, &mut app)).unwrap();
    }

    #[test]
    fn dialog_inputs_place_a_cursor_at_the_end_of_the_value() {
        use crate::config::{Mode, Reasoning};
        use crate::tui::app::{ConnectState, UsageState};
        use ratatui::backend::{Backend, TestBackend};
        use ratatui::Terminal;

        let new_app = || {
            App::new(
                "gpt-4o".into(),
                "/tmp/project".into(),
                Mode::Build,
                Reasoning::Auto,
            )
        };

        let connect_caret = |input: &str| {
            let mut app = new_app();
            app.connect = Some(ConnectState {
                step: ConnectStep::Key {
                    provider: "openai".into(),
                },
                input: input.into(),
                connected: Vec::new(),
                ..ConnectState::new()
            });
            let mut terminal = Terminal::new(TestBackend::new(80, 28)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            terminal.backend_mut().get_cursor_position().unwrap()
        };
        let bare = connect_caret("");
        let typed = connect_caret("abcd");
        assert!(typed.y > 0, "the key input has a cursor");
        assert_eq!(typed.x - bare.x, 4, "cursor follows the masked key");
        assert_eq!(typed.y, bare.y);

        let usage_caret = |input: &str| {
            let mut app = new_app();
            let mut state = UsageState::new(crate::portkey_usage::UsageSettings::default());
            state.selected = UsageField::ALL
                .iter()
                .position(|field| *field == UsageField::ApiKey)
                .unwrap();
            state.editing = true;
            state.input = input.into();
            app.usage_modal = Some(state);
            let mut terminal = Terminal::new(TestBackend::new(80, 28)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            terminal.backend_mut().get_cursor_position().unwrap()
        };
        let bare = usage_caret("");
        let typed = usage_caret("abcd");
        assert!(typed.y > 0, "the usage input has a cursor");
        assert_eq!(typed.x - bare.x, 4, "cursor follows the masked API key");
        assert_eq!(typed.y, bare.y);

        let options_caret = |input: &str| {
            let mut app = new_app();
            app.connect = Some(ConnectState {
                step: ConnectStep::Options {
                    provider: "portkey".into(),
                },
                input: input.into(),
                focus: crate::tui::app::ConnectField::Model,
                ..ConnectState::new()
            });
            let mut terminal = Terminal::new(TestBackend::new(80, 28)).unwrap();
            terminal.draw(|frame| draw(frame, &mut app)).unwrap();
            terminal.backend_mut().get_cursor_position().unwrap()
        };
        let bare = options_caret("");
        let typed = options_caret("abcd");
        assert!(typed.y > 0, "the options row has a cursor");
        assert_eq!(typed.x - bare.x, 4, "cursor follows the option value");
        assert_eq!(typed.y, bare.y);
    }
}

#[cfg(test)]
mod wrap_parity_tests {
    use super::*;
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    fn ratatui_lines(text: &str, width: u16, height: u16) -> Vec<String> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw(|frame| {
                let paragraph = Paragraph::new(text).wrap(Wrap { trim: false });
                frame.render_widget(paragraph, frame.area());
            })
            .unwrap();
        let buffer = terminal.backend().buffer().clone();
        let mut out = Vec::new();
        for y in 0..height {
            let mut line = String::new();
            for x in 0..width {
                line.push_str(buffer[(x, y)].symbol());
            }
            out.push(line.trim_end().to_string());
        }
        out
    }

    #[test]
    fn carriage_returns_never_reach_the_buffer() {
        use crate::config::{Mode, Reasoning};
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let output = "Cloning into 'x'...\nUpdating files:  74% (1879/2512)\rUpdating files:  75% (1884/2512)\rUpdating files: 100% (2512/2512), done.\n[exit: 0]";
        let mut app = App::new("m".into(), "/tmp".into(), Mode::Build, Reasoning::Auto);
        app.items.push(ChatItem::ToolResult {
            name: "bash".into(),
            args: "{\"command\":\"git clone x\"}".into(),
            output: output.into(),
            diff: None,
            millis: 0,
        });
        let mut terminal = Terminal::new(TestBackend::new(80, 28)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let mut screen = String::new();
        for y in 0..28 {
            for x in 0..80 {
                screen.push_str(buffer[(x, y)].symbol());
            }
            screen.push('\n');
        }
        assert!(!screen.contains('\r'), "carriage return leaked: {screen:?}");
        assert!(screen.contains("Updating files: 100% (2512/2512), done."));
        assert!(!screen.contains("74%"));
    }

    #[test]
    fn wrap_matches_ratatui_line_breaks() {
        let texts = [
            "the quick brown fox",
            "aaaaaaaaaaaaaaaaaaaaaaaaa",
            "foo  bar  baz",
            "hello world ",
            "12 34 56 78 9 10 11 12 13",
            "a bb ccc dddd eeeee ffffff ggggggg",
            "word ",
            " supercalifragilisticexpialidocious tail",
        ];
        for text in texts {
            for width in 4u16..30 {
                let ours: Vec<String> = wrap(text, width as usize)
                    .into_iter()
                    .map(|line| line.trim_end().to_string())
                    .filter(|line| !line.is_empty())
                    .collect();
                let theirs: Vec<String> = ratatui_lines(text, width, 20)
                    .into_iter()
                    .filter(|line| !line.is_empty())
                    .collect();
                assert_eq!(ours, theirs, "text={text:?} width={width}");
            }
        }
    }
}
