use crate::config::Reasoning;
use crate::tools::DiffPreview;
use crate::tui::app::{App, ChatItem, ConnectStep, Selection};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Frame;

const MIN_INPUT_ROWS: usize = 3;
const MAX_INPUT_ROWS: usize = 12;
/// Blank rows kept above the conversation so the first line (banner or chat)
/// is not flush with the terminal's top edge.
const MESSAGE_TOP_PAD: u16 = 1;
const MAX_MODEL_ROWS: usize = 12;
const MAX_SESSION_ROWS: usize = 12;
const MAX_SUGGESTION_ROWS: usize = 8;

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
    let [messages, input, footer] = main_areas(frame.area(), app);

    draw_messages(frame, app, messages);
    draw_input(frame, app, input);
    draw_footer(frame, app, footer);

    if app.connect.is_some() {
        draw_connect(frame, app);
    } else if app.trust.is_some() {
        draw_trust(frame, app);
    } else if app.models.is_some() {
        draw_models(frame, app);
    } else if app.sessions.is_some() {
        draw_sessions(frame, app);
    } else if !app.suggestions.is_empty() {
        draw_suggestions(frame, app, messages);
    }
}

fn main_areas(area: Rect, app: &App) -> [Rect; 3] {
    let input_width = area.width.saturating_sub(4) as usize;
    let input_rows = input_rows(&app.input, input_width) as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(input_rows + 2),
            Constraint::Length(2),
        ])
        .split(area);
    [chunks[0], chunks[1], chunks[2]]
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
    };
    let title = match &state.step {
        ConnectStep::Provider => " connect ",
        ConnectStep::Key { provider } => provider.as_str(),
    };

    let mut lines = vec![Line::from(Span::styled(
        prompt,
        Style::default().fg(app.theme.info),
    ))];
    if matches!(state.step, ConnectStep::Provider) {
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
            lines.push(Line::from(vec![
                Span::styled(format!(" {marker} {:<10}", option.label), style),
                Span::styled(option.description, Style::default().fg(app.theme.info)),
            ]));
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
    lines.push(Line::from(vec![
        Span::styled("> ", Style::default().fg(app.theme.accent)),
        Span::styled(value, Style::default().fg(app.theme.assistant)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        match state.step {
            ConnectStep::Provider => "↑/↓ choose · Enter continue · Esc cancel",
            ConnectStep::Key { .. } => "Enter connect · Backspace back · Esc cancel",
        },
        Style::default().fg(app.theme.info),
    )));

    let block = panel(title, app.theme.accent);
    frame.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn draw_models(frame: &mut Frame, app: &App) {
    let Some(state) = &app.models else {
        return;
    };
    let area = centered_rect(70, 60, frame.area());
    frame.render_widget(Clear, area);

    let title = if state.filter.is_empty() {
        " models ".to_string()
    } else {
        format!(" models · {} ", state.filter)
    };
    let block = panel(&title, app.theme.accent);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.loading {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "loading models…",
                Style::default().fg(app.theme.info),
            )),
            inner,
        );
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

    let models = state.filtered();
    if models.is_empty() {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "no matching models",
                Style::default().fg(app.theme.info),
            )),
            inner,
        );
        return;
    }

    let rows = inner.height.saturating_sub(1) as usize;
    let visible = models.len().min(MAX_MODEL_ROWS).min(rows.max(1));
    let offset = state
        .selected
        .saturating_sub(visible.saturating_sub(1))
        .min(models.len().saturating_sub(visible));
    let items: Vec<ListItem> = models[offset..offset + visible]
        .iter()
        .map(|model| ListItem::new(Line::from(crate::config::model_label(model))))
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
        width: area.width.saturating_sub(2).min(72),
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
    let [message_area, _, _] = main_areas(terminal_area, app);
    let window = suggestion_window(app, message_area)?;
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
    let name_cap = if is_command {
        24
    } else {
        (window.popup.width.saturating_sub(5) as usize).max(8)
    };
    let name_width = app.suggestions[window.offset..window.offset + window.count]
        .iter()
        .map(|hint| hint.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(name_cap);
    let items: Vec<ListItem> = app.suggestions[window.offset..window.offset + window.count]
        .iter()
        .map(|hint| {
            let prefix = if is_command { "/" } else { "@" };
            let name = if is_command {
                truncate(&hint.name, name_width)
            } else {
                truncate_path(&hint.name, name_width)
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{prefix}{name:<name_width$}"),
                    Style::default()
                        .fg(app.theme.accent)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("   {}", hint.description),
                    Style::default().fg(app.theme.info),
                ),
            ]))
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
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Length(1)])
        .split(area);
    let width = area.width as usize;
    let info = Style::default().fg(app.theme.info);
    let accent = Style::default()
        .fg(app.theme.accent)
        .add_modifier(Modifier::BOLD);

    // Row one: where we are, and the branch we are on.
    let mut project = vec![
        Span::styled(" ", info),
        Span::styled(display_path(&app.cwd), info),
    ];
    if let Some(branch) = &app.git_branch {
        project.push(Span::styled("  ", info));
        project.push(Span::styled(
            format!("⑂ {branch}"),
            Style::default().fg(app.theme.success),
        ));
    }
    let mut session = Vec::new();
    if let Some(name) = &app.session_name {
        session.push(Span::styled(
            format!("{name} "),
            Style::default().fg(app.theme.dim),
        ));
    }
    frame.render_widget(
        Paragraph::new(aligned_row(project, session, width)),
        rows[0],
    );

    // Row two: usage on the left, the model and thinking level on the right.
    let mut stats = vec![Span::styled(" ", info)];
    if app.tokens_in > 0 || app.tokens_out > 0 {
        stats.push(Span::styled(
            format!(
                "↑ {}  ↓ {}  ",
                compact_tokens(app.tokens_in),
                compact_tokens(app.tokens_out)
            ),
            Style::default().fg(app.theme.tool),
        ));
    }
    if app.context_limit > 0 && app.context_used > 0 {
        let pct = context_percent(app.context_used, app.context_limit);
        stats.push(Span::styled("⧉ ", info));
        stats.push(Span::styled(
            format!("{pct}%  "),
            Style::default().fg(context_color(pct, &app.theme)),
        ));
    }
    stats.push(Span::styled("·  ", info));
    stats.push(Span::styled(app.mode.label().to_string(), accent));

    let controls = vec![
        Span::styled(crate::config::model_label(&app.model).to_string(), accent),
        Span::styled(" · ", info),
        Span::styled(
            app.reasoning.label().to_string(),
            Style::default().fg(reasoning_color(app.reasoning, &app.theme)),
        ),
        Span::styled(" ", info),
    ];
    frame.render_widget(Paragraph::new(aligned_row(stats, controls, width)), rows[1]);
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

/// Context usage color escalates from muted to warning to error.
fn context_color(pct: u64, theme: &crate::theme::Theme) -> Color {
    if pct >= 85 {
        theme.error
    } else if pct >= 60 {
        theme.tool
    } else {
        theme.info
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
    let paragraph = Paragraph::new(content).wrap(Wrap { trim: false });
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
    let [message_area, _, _] = main_areas(terminal_area, app);
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

    let running = app
        .running_tool
        .as_ref()
        .map(|(name, started)| (name.clone(), started.elapsed()));
    for index in start..count {
        let offset = app.lines.len();
        app.line_offsets.push(offset);
        render_item_themed(
            &app.items[index],
            width,
            app.expand_tools,
            &app.theme,
            running
                .as_ref()
                .map(|(name, elapsed)| (name.as_str(), *elapsed)),
            &mut app.lines,
        );
        if app.lines.len() > offset {
            app.lines.push(Line::from(""));
        }
    }
    app.render_dirty_from = None;
}

fn render_item_themed(
    item: &ChatItem,
    width: usize,
    expand_tools: bool,
    theme: &crate::theme::Theme,
    running: Option<(&str, std::time::Duration)>,
    lines: &mut Vec<Line<'static>>,
) {
    let bold = Modifier::BOLD;
    match item {
        ChatItem::Banner { info } => render_banner_themed(width, theme, info, lines),
        ChatItem::User(text) => {
            lines.push(Line::from(vec![
                Span::styled("❯ ", Style::default().fg(theme.user).add_modifier(bold)),
                Span::styled("you", Style::default().fg(theme.user).add_modifier(bold)),
            ]));
            push_wrapped(lines, text, width, Style::default());
        }
        ChatItem::Assistant(text) => {
            lines.push(Line::from(vec![
                Span::styled(
                    "◆ ",
                    Style::default().fg(theme.assistant).add_modifier(bold),
                ),
                Span::styled(
                    "oxide",
                    Style::default().fg(theme.assistant).add_modifier(bold),
                ),
            ]));
            push_wrapped(lines, text, width, Style::default());
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
                if let Some((running_name, elapsed)) = running {
                    if running_name == name {
                        subject.push_str(&format!(
                            " · Elapsed {}",
                            format_duration(elapsed.as_millis() as u64)
                        ));
                    }
                }
                panel.extend(action_lines("Run", &subject, theme.tool, bold, inner));
            } else {
                panel.extend(wrapped_with_prefix(
                    vec![
                        Span::styled("⚙ ", Style::default().fg(theme.tool)),
                        Span::styled(
                            name.clone(),
                            Style::default().fg(theme.tool).add_modifier(bold),
                        ),
                    ],
                    &format!(" {}", tool_arg_summary(name, args)),
                    inner,
                    Style::default().fg(theme.info),
                ));
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
            if expand_tools && !output.trim().is_empty() {
                push_wrapped(
                    &mut panel,
                    output,
                    box_inner_width(width),
                    Style::default().fg(theme.info),
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
                panel.append(&mut body);
                if let Some(hidden) = hidden {
                    panel.push(collapsed_hint(hidden, theme.info, inner));
                }
                if failed {
                    push_tool_output(&mut panel, output, inner, Style::default().fg(theme.error));
                } else if let Some((_, rest)) = output.split_once("\n\n") {
                    if !rest.trim().is_empty() {
                        push_tool_output(&mut panel, rest, inner, Style::default().fg(theme.info));
                    }
                }
            } else if let Some(path) = file_tool_path(name, args) {
                if crate::tools::canonical_tool_name(name) == "read_file" {
                    if output.starts_with("error:") {
                        bg = theme.tool_error_bg;
                        panel.extend(action_lines("Read failed", &path, theme.error, bold, inner));
                        push_tool_output(
                            &mut panel,
                            output,
                            inner,
                            Style::default().fg(theme.error),
                        );
                    } else {
                        panel.extend(action_lines("Read", &path, theme.success, bold, inner));
                    }
                } else if output.starts_with("error:") {
                    bg = theme.tool_error_bg;
                    panel.extend(action_lines("Edit failed", &path, theme.error, bold, inner));
                    push_tool_output(&mut panel, output, inner, Style::default().fg(theme.error));
                } else {
                    panel.extend(action_lines("Edited", &path, theme.success, bold, inner));
                    if let Some((_, rest)) = output.split_once("\n\n") {
                        if !rest.trim().is_empty() {
                            push_tool_output(
                                &mut panel,
                                rest,
                                inner,
                                Style::default().fg(theme.info),
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
                    push_tool_output(&mut panel, output, inner, Style::default().fg(theme.error));
                } else if bash_has_body(output) {
                    if expand_tools {
                        push_tool_output(
                            &mut panel,
                            &bash_body(output),
                            inner,
                            Style::default().fg(theme.info),
                        );
                    } else {
                        panel.push(collapsed_hint(
                            output.lines().count().saturating_sub(1),
                            theme.info,
                            inner,
                        ));
                    }
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
                    if expand_tools {
                        push_tool_output(
                            &mut panel,
                            output,
                            inner,
                            Style::default().fg(theme.info),
                        );
                    } else {
                        panel.push(collapsed_hint(output.lines().count(), theme.info, inner));
                    }
                }
            }
            // Shell timings always show (Pi behavior); other tools only report
            // when they were slow enough to be worth calling out.
            if *millis > 0 && (command.is_some() || *millis >= TOOL_TIME_THRESHOLD_MS) {
                panel.push(Line::from(Span::styled(
                    format!("Took {}", format_duration(*millis)),
                    Style::default().fg(theme.dim),
                )));
            }
            push_bg_panel(lines, panel, width, bg);
        }
        ChatItem::Thought(millis) => {
            lines.push(Line::from(Span::styled(
                format!("+ Thought: {millis}ms"),
                Style::default().fg(theme.info),
            )));
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
    }
}

/// Gap between the wordmark and the info column.
const BANNER_COLUMN_GAP: usize = 3;
/// Minimum width kept for the right-hand banner column before the layout
/// falls back to stacking the wordmark above the info text.
const BANNER_MIN_RIGHT: usize = 24;

/// Renders the banner as the wordmark beside the welcome info when there is
/// room, falling back to stacked (or plain-text) layouts on narrow terminals.
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
        for entry in info {
            push_wrapped(lines, entry, width.max(1), Style::default().fg(theme.info));
        }
        return;
    }

    let right_width = width.saturating_sub(art_width + BANNER_COLUMN_GAP);
    if right_width >= BANNER_MIN_RIGHT {
        render_banner_columns(art_width, right_width, theme, info, lines);
        return;
    }

    for (index, art) in BANNER.iter().enumerate() {
        let pad = (width - art.chars().count()) / 2;
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled((*art).to_string(), art_style(index, theme)),
        ]));
    }
    for entry in info {
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

/// Render the wordmark and the welcome info side by side, wrapping the info to
/// the right-hand column width.
fn render_banner_columns(
    art_width: usize,
    right_width: usize,
    theme: &crate::theme::Theme,
    info: &[String],
    lines: &mut Vec<Line<'static>>,
) {
    let mut right: Vec<String> = Vec::new();
    for (index, entry) in info.iter().enumerate() {
        if index > 0 {
            right.push(String::new());
        }
        right.extend(wrap(entry, right_width.max(1)));
    }
    for row in 0..BANNER.len().max(right.len()) {
        let mut spans = Vec::with_capacity(3);
        match BANNER.get(row) {
            Some(art) => {
                let pad = art_width - art.chars().count();
                spans.push(Span::styled(
                    format!("{art}{}", " ".repeat(pad)),
                    art_style(row, theme),
                ));
            }
            None => spans.push(Span::raw(" ".repeat(art_width))),
        }
        spans.push(Span::raw(" ".repeat(BANNER_COLUMN_GAP)));
        if let Some(text) = right.get(row) {
            spans.push(Span::styled(text.clone(), Style::default().fg(theme.info)));
        }
        lines.push(Line::from(spans));
    }
}

#[cfg(test)]
fn render_item(item: &ChatItem, width: usize, expand_tools: bool, lines: &mut Vec<Line<'static>>) {
    render_item_themed(
        item,
        width,
        expand_tools,
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
    let border_color = if app.busy {
        app.theme.info
    } else {
        reasoning_color(app.reasoning, &app.theme)
    };
    let area = Rect {
        y: area.y.saturating_add(1),
        height: area.height.saturating_sub(1),
        ..area
    };
    let label = if app.attachments.is_empty() {
        "message".to_string()
    } else {
        format!("{} attachment(s)", app.attachments.len())
    };
    let title = truncate_spans(
        input_title(app, label, border_color),
        area.width.saturating_sub(2) as usize,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_color))
        .title(Line::from(title));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let text_area = Rect {
        x: inner.x + 2,
        width: inner.width.saturating_sub(2),
        ..inner
    };
    let prompt = Rect {
        x: inner.x,
        y: inner.y,
        width: inner.width.min(2),
        height: 1,
    };
    frame.render_widget(
        Paragraph::new(Span::styled(
            "> ",
            Style::default()
                .fg(border_color)
                .add_modifier(Modifier::BOLD),
        )),
        prompt,
    );

    let width = text_area.width as usize;
    let input: Text = if app.input.is_empty() && !app.busy {
        Text::from(Line::from(Span::styled(
            "Ask Oxide anything about your code…",
            Style::default().fg(app.theme.info),
        )))
    } else {
        composer_text(&app.input, &app.theme)
    };
    let paragraph = Paragraph::new(input)
        .wrap(Wrap { trim: false })
        .scroll((input_scroll(&app.input, app.input_cursor, width), 0));
    frame.render_widget(paragraph, text_area);

    if !app.busy && app.connect.is_none() && app.models.is_none() {
        let (cursor_row, cursor_column) =
            input_cursor_position(&app.input, app.input_cursor.min(app.input.len()), width);
        let scroll = input_scroll(&app.input, app.input_cursor, width) as usize;
        let x = text_area.x + cursor_column as u16;
        let x = x.min(text_area.x + text_area.width.saturating_sub(1));
        let y = text_area.y + cursor_row.saturating_sub(scroll) as u16;
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

fn input_title(app: &App, label: String, border_color: Color) -> Vec<Span<'static>> {
    let mut title = vec![Span::styled(
        format!(" {label} "),
        Style::default()
            .fg(border_color)
            .add_modifier(Modifier::BOLD),
    )];
    if app.busy {
        let secs = app
            .busy_since
            .map(|start| start.elapsed().as_secs())
            .unwrap_or(0);
        title.push(Span::styled(
            format!(
                "· {} {} · {secs}s · Esc clear/quit ",
                spinner(app.busy_since),
                app.status
            ),
            Style::default().fg(app.theme.tool),
        ));
    } else {
        title.push(Span::styled("· ", Style::default().fg(app.theme.accent)));
        title.push(Span::styled(
            format!("{} ", app.status),
            Style::default().fg(app.theme.info),
        ));
    }
    title
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
fn action_lines(
    verb: &str,
    subject: &str,
    color: Color,
    bold: Modifier,
    width: usize,
) -> Vec<Line<'static>> {
    wrapped_with_prefix(
        vec![
            Span::styled("→ ", Style::default().fg(color)),
            Span::styled(
                verb.to_string(),
                Style::default().fg(color).add_modifier(bold),
            ),
            Span::styled(" ", Style::default().fg(color)),
        ],
        subject,
        width,
        Style::default().fg(color),
    )
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

/// A one-line affordance shown when a tool body is hidden, mirroring the
/// opencode "click to expand" hint.
fn collapsed_hint(hidden_lines: usize, color: Color, width: usize) -> Line<'static> {
    let hint = format!("⋯ {hidden_lines} lines · Ctrl+O to expand");
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
    if diff.text.trim().is_empty() {
        return (Vec::new(), None);
    }
    let all: Vec<&str> = diff.text.lines().collect();
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
    for wrapped in wrap(text, width.max(1)) {
        lines.push(Line::from(Span::styled(wrapped, style)));
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
        assert_eq!(input_rows("hello\nworld", 10), MIN_INPUT_ROWS);
        assert_eq!(input_rows(&"a".repeat(200), 10), MAX_INPUT_ROWS);
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
        let [message_area, _, _] = main_areas(area, &app);
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
    fn banner_columns_or_falls_back_when_narrow() {
        let info = vec![
            "Build things.".to_string(),
            "1 agent · 0 plugins".to_string(),
        ];
        let mut wide = Vec::new();
        render_banner(80, &info, &mut wide);
        assert_eq!(wide.len(), BANNER.len());
        let first = line_text(&wide[0]);
        assert!(first.starts_with(BANNER[0]));
        assert!(first.contains("Build things."));
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
            false,
            &mut lines,
        );
        assert_eq!(panel_line(&lines), "→ Read src/main.rs");

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
        assert_eq!(lines.len(), 4);
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
    fn tool_output_is_hidden_only_when_collapsed() {
        let output = (0..10)
            .map(|index| format!("line {index}"))
            .collect::<Vec<_>>()
            .join("\n");

        let mut collapsed = Vec::new();
        render_item(
            &ChatItem::ToolProgress {
                name: "bash".into(),
                output: output.clone(),
            },
            80,
            false,
            &mut collapsed,
        );
        assert!(collapsed.is_empty());

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
        assert_eq!(expanded.len(), 12);
        assert_eq!(
            expanded[0].spans[0].style.bg,
            Some(Color::Rgb(0x28, 0x28, 0x32))
        );
        assert_eq!(
            expanded[11].spans[0].style.bg,
            Some(Color::Rgb(0x28, 0x28, 0x32))
        );
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
        assert_eq!(lines[2].spans[1].style.fg, Some(Color::Gray));
        assert_eq!(lines[3].spans[1].style.fg, Some(Color::LightRed));
        assert_eq!(lines[4].spans[1].style.fg, Some(Color::LightGreen));
        assert_eq!(
            lines[5].spans[0].style.bg,
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
        assert_eq!(lines[2].spans[1].content.as_ref(), "hi");
        assert_eq!(
            lines[3].spans[0].style.bg,
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
        assert_eq!(lines[2].spans[1].content.as_ref(), "match");
        assert_eq!(
            lines[3].spans[0].style.bg,
            Some(Color::Rgb(0x28, 0x32, 0x28))
        );
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
            &crate::theme::Theme::dark(),
            Some(("bash", std::time::Duration::from_millis(2400))),
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
            &crate::theme::Theme::dark(),
            Some(("grep", std::time::Duration::from_millis(2400))),
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

        let input_top = row_of(&buffer, "message").expect("input title");
        assert_eq!(row_of(&buffer, "ready"), Some(input_top));
        let input_bottom = row_of(&buffer, "╰").expect("input box bottom border");
        let project_row = input_bottom + 1;
        let controls_row = input_bottom + 2;
        assert_eq!(row_of(&buffer, "/tmp/project"), Some(project_row));
        assert_eq!(row_of(&buffer, "⑂"), Some(project_row));
        assert_eq!(row_of(&buffer, "main"), Some(project_row));
        assert_eq!(row_of(&buffer, "gpt-4o"), Some(controls_row));
        assert_eq!(row_of(&buffer, "build"), Some(controls_row));
        assert_eq!(row_of(&buffer, "auto"), Some(controls_row));

        let text: String = (0..buffer.area.width)
            .map(|x| buffer[(x, controls_row)].symbol())
            .collect();
        assert!(text.starts_with(" ·  build"));
        assert!(text.trim_end().ends_with("gpt-4o · auto"));
        assert!(!text.contains("Enter send"));
        assert!(!text.contains("Ctrl+O"));
    }

    #[test]
    fn input_title_shows_status_without_duplicating_footer_usage() {
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
        app.tokens_in = 107_800;
        app.tokens_out = 4_800;
        app.context_used = 20;
        app.context_limit = 100;
        let mut terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();

        let input_row = row_of(buffer, "message").expect("input title");
        let row: String = (0..buffer.area.width)
            .map(|x| buffer[(x, input_row)].symbol())
            .collect();
        assert!(row.contains("message · ready"));
        assert!(!row.contains("107.8k"));
        assert!(!row.contains("20%"));
        assert_ne!(row_of(buffer, "Ask Oxide"), Some(input_row));

        let footer_row = row_of(buffer, "107.8k").expect("usage moves to the footer");
        let footer: String = (0..buffer.area.width)
            .map(|x| buffer[(x, footer_row)].symbol())
            .collect();
        assert!(footer.contains("↓ 4.8k"));
        assert!(footer.contains("20%"));
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

        assert_eq!(suggestion_index_at(&app, area, 3, 8), Some(2));
        assert_eq!(suggestion_index_at(&app, area, 3, 15), Some(9));
        assert_eq!(suggestion_index_at(&app, area, 0, 8), None);
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
        assert_eq!(row_of(buffer, "⑂"), None);
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
