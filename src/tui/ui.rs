use crate::config::{Mode, Reasoning};
use crate::tools::DiffPreview;
use crate::tui::app::{App, ChatItem, ConnectStep};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Frame;

const MIN_INPUT_ROWS: usize = 3;
const MAX_INPUT_ROWS: usize = 12;
const MAX_MODEL_ROWS: usize = 12;
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
    let input_width = frame.area().width.saturating_sub(4) as usize;
    let input_rows = input_rows(&app.input, input_width) as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),
            Constraint::Length(input_rows + 2),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_messages(frame, app, chunks[0]);
    draw_input(frame, app, chunks[1]);
    draw_footer(frame, app, chunks[2]);

    if app.connect.is_some() {
        draw_connect(frame, app);
    } else if app.trust.is_some() {
        draw_trust(frame, app);
    } else if app.models.is_some() {
        draw_models(frame, app);
    } else if !app.suggestions.is_empty() {
        draw_suggestions(frame, app, chunks[0]);
    }
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
        .map(|model| ListItem::new(Line::from(*model)))
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

fn draw_suggestions(frame: &mut Frame, app: &App, area: Rect) {
    if app.suggestions.is_empty() || area.height < 3 {
        return;
    }
    let count = app.suggestions.len().min(MAX_SUGGESTION_ROWS);
    let height = count as u16 + 2;
    let width = area.width.min(64);
    let popup = Rect {
        x: area.x,
        y: area.y + area.height.saturating_sub(height),
        width,
        height,
    };
    frame.render_widget(Clear, popup);

    let offset = app.suggestion_index.saturating_sub(count.saturating_sub(1));
    let items: Vec<ListItem> = app.suggestions[offset..offset + count]
        .iter()
        .map(|hint| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("/{}", hint.name),
                    Style::default().fg(app.theme.accent),
                ),
                Span::styled(
                    format!("  {}", hint.description),
                    Style::default().fg(app.theme.info),
                ),
            ]))
        })
        .collect();
    let block = panel("commands", app.theme.border);
    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD | Modifier::REVERSED),
        )
        .highlight_symbol("> ");
    let mut list_state = ListState::default();
    list_state.select(Some(app.suggestion_index.saturating_sub(offset)));
    frame.render_stateful_widget(list, popup, &mut list_state);
}

/// Pi-style footer: current status, working directory, session name, token
/// totals, context usage, model, mode, and thinking level.
fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let dim = Style::default().fg(app.theme.info);
    let mut spans = if app.busy {
        let secs = app
            .busy_since
            .map(|start| start.elapsed().as_secs())
            .unwrap_or(0);
        vec![
            Span::styled(
                format!(" {} ", spinner(app.busy_since)),
                Style::default().fg(app.theme.tool),
            ),
            Span::styled(
                format!("{} · {secs}s · Esc clear/quit", app.status),
                Style::default().fg(app.theme.tool),
            ),
        ]
    } else {
        vec![
            Span::styled(" ● ", Style::default().fg(app.theme.accent)),
            Span::styled(app.status.clone(), dim),
        ]
    };
    spans.push(Span::styled(format!(" · {}", display_path(&app.cwd)), dim));
    if let Some(name) = &app.session_name {
        spans.push(Span::styled(format!(" · {name}"), dim));
    }
    if app.tokens_in > 0 || app.tokens_out > 0 {
        spans.push(Span::styled(
            format!(
                " · ↑{} ↓{}",
                compact_tokens(app.tokens_in),
                compact_tokens(app.tokens_out)
            ),
            dim,
        ));
    }
    if app.context_limit > 0 && app.context_used > 0 {
        let pct = (app.context_used as f64 / app.context_limit as f64 * 100.0).round() as u64;
        let color = if pct >= 85 {
            app.theme.error
        } else if pct >= 60 {
            app.theme.tool
        } else {
            app.theme.info
        };
        spans.push(Span::styled(
            format!(" · {pct}% ctx"),
            Style::default().fg(color),
        ));
    }
    let right = Line::from(vec![
        Span::styled(
            app.model.clone(),
            Style::default()
                .fg(app.theme.accent)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" · {} ", app.mode.label()), mode_style(app.mode)),
        Span::styled(
            format!("{} ", app.reasoning.label()),
            reasoning_style(app.reasoning),
        ),
    ]);
    frame.render_widget(
        Paragraph::new(justified_line(spans, right, area.width as usize)),
        area,
    );
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

/// Like [`justified`], but the right side is a styled span sequence.
fn justified_line(left: Vec<Span<'static>>, right: Line<'static>, width: usize) -> Line<'static> {
    let left_len: usize = left.iter().map(|span| span.content.chars().count()).sum();
    let right_len: usize = right.spans.iter().map(|s| s.content.chars().count()).sum();
    if left_len + right_len <= width {
        let mut spans = left;
        spans.push(Span::raw(" ".repeat(width - left_len - right_len)));
        spans.extend(right.spans);
        return Line::from(spans);
    }

    if right_len >= width {
        let text = right
            .spans
            .iter()
            .map(|span| span.content.to_string())
            .collect::<String>();
        return Line::from(Span::styled(
            truncate(&text, width),
            Style::default().fg(Color::Gray),
        ));
    }

    let left_width = width.saturating_sub(right_len + 1);
    let mut spans = truncate_spans(left, left_width);
    spans.push(Span::raw(" "));
    spans.extend(right.spans);
    Line::from(spans)
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

fn mode_style(mode: Mode) -> Style {
    let (fg, bg) = match mode {
        Mode::Build => (Color::Black, Color::LightCyan),
        Mode::AutoEdit => (Color::Black, Color::LightYellow),
        Mode::Plan => (Color::Black, Color::LightMagenta),
    };
    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
}

fn reasoning_style(reasoning: Reasoning) -> Style {
    let (fg, bg) = match reasoning {
        Reasoning::Auto => (Color::Black, Color::LightGreen),
        Reasoning::Off => (Color::Black, Color::Gray),
        Reasoning::Low => (Color::Black, Color::LightCyan),
        Reasoning::Medium => (Color::Black, Color::LightBlue),
        Reasoning::High => (Color::Black, Color::LightMagenta),
    };
    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
}

fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let inner = Rect {
        x: area.x + 1,
        width: area.width.saturating_sub(2),
        ..area
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
    let paragraph = Paragraph::new(app.lines[start..end].to_vec()).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);
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

    for index in start..count {
        let offset = app.lines.len();
        app.line_offsets.push(offset);
        render_item_themed(
            &app.items[index],
            width,
            app.expand_tools,
            &app.theme,
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
    lines: &mut Vec<Line<'static>>,
) {
    let bold = Modifier::BOLD;
    match item {
        ChatItem::Banner => render_banner_themed(width, theme, lines),
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
            if let Some(path) = file_tool_path(name, args) {
                let (verb, color) = if matches!(
                    crate::tools::canonical_tool_name(name),
                    "write_file" | "patch" | "edit"
                ) {
                    ("Edit", theme.tool)
                } else {
                    ("Read", theme.accent)
                };
                lines.push(action_line(verb, &path, color, bold, width));
            } else if let Some(command) = bash_command(name, args) {
                lines.push(action_line("Run", &command, theme.tool, bold, width));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("⚙ ", Style::default().fg(theme.tool)),
                    Span::styled(
                        name.clone(),
                        Style::default().fg(theme.tool).add_modifier(bold),
                    ),
                    Span::styled(
                        format!(" {}", tool_arg_summary(name, args)),
                        Style::default().fg(theme.info),
                    ),
                ]));
            }
        }
        ChatItem::ToolProgress { name, output } => {
            if crate::tools::canonical_tool_name(name) != "bash" {
                lines.push(Line::from(vec![
                    Span::styled("⋯ ", Style::default().fg(theme.info)),
                    Span::styled(name.clone(), Style::default().fg(theme.info)),
                ]));
            }
            push_tool_body(lines, output, width, expand_tools, theme.info);
        }
        ChatItem::ToolResult {
            name,
            args,
            output,
            diff,
        } => {
            if let Some(diff) = diff {
                render_diff(diff, width, expand_tools, theme, lines);
                if output.starts_with("error:") {
                    push_wrapped(lines, output, width, Style::default().fg(theme.error));
                } else if let Some((_, rest)) = output.split_once("\n\n") {
                    if !rest.trim().is_empty() {
                        push_wrapped(lines, rest, width, Style::default().fg(theme.info));
                    }
                }
            } else if let Some(path) = file_tool_path(name, args) {
                if crate::tools::canonical_tool_name(name) == "read_file" {
                    if output.starts_with("error:") {
                        lines.push(action_line("Read failed", &path, theme.error, bold, width));
                        push_wrapped(lines, output, width, Style::default().fg(theme.error));
                    } else {
                        lines.push(action_line("Read", &path, theme.success, bold, width));
                    }
                } else if output.starts_with("error:") {
                    lines.push(action_line("Edit failed", &path, theme.error, bold, width));
                    push_wrapped(lines, output, width, Style::default().fg(theme.error));
                } else {
                    lines.push(action_line("Edited", &path, theme.success, bold, width));
                    if let Some((_, rest)) = output.split_once("\n\n") {
                        if !rest.trim().is_empty() {
                            push_wrapped(lines, rest, width, Style::default().fg(theme.info));
                        }
                    }
                }
            } else if let Some(command) = bash_command(name, args) {
                let exit = bash_exit_code(output);
                let failed = exit
                    .map(|code| code != 0)
                    .unwrap_or_else(|| output.starts_with("error:"));
                let color = if failed { theme.error } else { theme.success };
                let subject = match exit {
                    Some(code) => format!("{command} · exit {code}"),
                    None => command,
                };
                let verb = if failed { "Run failed" } else { "Ran" };
                lines.push(action_line(verb, &subject, color, bold, width));
                if exit.is_none() && !output.trim().is_empty() {
                    push_wrapped(lines, output, width, Style::default().fg(theme.error));
                } else if expand_tools {
                    push_tool_body(lines, output, width, true, theme.info);
                } else if bash_has_body(output) {
                    push_collapsed_hint(
                        lines,
                        width,
                        output.lines().count().saturating_sub(1),
                        theme.info,
                    );
                }
            } else {
                lines.push(Line::from(vec![
                    Span::styled("↳ ", Style::default().fg(theme.info)),
                    Span::styled(name.clone(), Style::default().fg(theme.info)),
                ]));
                if expand_tools {
                    push_tool_body(lines, output, width, true, theme.info);
                } else if !output.trim().is_empty() {
                    push_collapsed_hint(lines, width, output.lines().count(), theme.info);
                }
            }
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

/// Centers the block-letter wordmark, falling back to plain text when the
/// terminal is too narrow for the art.
fn render_banner_themed(width: usize, theme: &crate::theme::Theme, lines: &mut Vec<Line<'static>>) {
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
        return;
    }
    for (index, art) in BANNER.iter().enumerate() {
        let pad = (width - art.chars().count()) / 2;
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled(
                (*art).to_string(),
                Style::default()
                    .fg(if index + 1 == BANNER.len() {
                        theme.dim
                    } else {
                        theme.accent
                    })
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
}

#[cfg(test)]
fn render_item(item: &ChatItem, width: usize, expand_tools: bool, lines: &mut Vec<Line<'static>>) {
    render_item_themed(
        item,
        width,
        expand_tools,
        &crate::theme::Theme::dark(),
        lines,
    );
}

#[cfg(test)]
fn render_banner(width: usize, lines: &mut Vec<Line<'static>>) {
    render_banner_themed(width, &crate::theme::Theme::dark(), lines);
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let border_color = if app.busy {
        app.theme.info
    } else {
        reasoning_color(app.reasoning, &app.theme)
    };
    let title = if app.attachments.is_empty() {
        "message".to_string()
    } else {
        format!("{} attachment(s)", app.attachments.len())
    };
    let block = panel(&title, border_color);
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
    let input = if app.input.is_empty() && !app.busy {
        Span::styled(
            "Ask Oxide anything about your code…",
            Style::default().fg(app.theme.info),
        )
    } else {
        Span::raw(app.input.as_str())
    };
    let paragraph = Paragraph::new(input)
        .wrap(Wrap { trim: false })
        .scroll((input_scroll(&app.input, width), 0));
    frame.render_widget(paragraph, text_area);

    if !app.busy && app.connect.is_none() && app.models.is_none() {
        let lines = wrap(&app.input, width);
        let last = lines.last().map(|line| line.chars().count()).unwrap_or(0);
        let x = text_area.x + last as u16;
        let x = x.min(text_area.x + text_area.width.saturating_sub(1));
        let cursor_line = lines.len().clamp(1, MAX_INPUT_ROWS) - 1;
        let y = text_area.y + cursor_line as u16;
        frame.set_cursor_position((x, y));
    }
}

fn input_rows(input: &str, width: usize) -> usize {
    wrap(input, width)
        .len()
        .clamp(MIN_INPUT_ROWS, MAX_INPUT_ROWS)
}

fn input_scroll(input: &str, width: usize) -> u16 {
    wrap(input, width).len().saturating_sub(MAX_INPUT_ROWS) as u16
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

fn action_line(
    verb: &str,
    subject: &str,
    color: Color,
    bold: Modifier,
    width: usize,
) -> Line<'static> {
    let prefix = 2 + verb.chars().count() + 1;
    let subject = truncate(subject, width.saturating_sub(prefix));
    Line::from(vec![
        Span::styled("→ ", Style::default().fg(color)),
        Span::styled(
            verb.to_string(),
            Style::default().fg(color).add_modifier(bold),
        ),
        Span::styled(format!(" {subject}"), Style::default().fg(color)),
    ])
}

/// Render a shell command as `$ <command>`, truncating to a single line.
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

/// Render a tool's output. Output is hidden by default so the conversation
/// stays a compact action list (like the opencode reference); Ctrl+O reveals
/// the full body.
fn push_tool_body(
    lines: &mut Vec<Line<'static>>,
    output: &str,
    width: usize,
    expand_tools: bool,
    color: Color,
) {
    if !expand_tools {
        return;
    }
    push_wrapped(lines, output, width, Style::default().fg(color));
}

/// A one-line affordance shown when a tool body is hidden, mirroring the
/// opencode "click to expand" hint.
fn push_collapsed_hint(
    lines: &mut Vec<Line<'static>>,
    width: usize,
    hidden_lines: usize,
    color: Color,
) {
    let hint = format!("  ⋯ {hidden_lines} lines · Ctrl+O to expand");
    lines.push(Line::from(Span::styled(
        truncate(&hint, width),
        Style::default().fg(color),
    )));
}

/// Whether a `bash` result has output beyond the trailing exit-code line.
fn bash_has_body(output: &str) -> bool {
    output
        .lines()
        .filter(|line| !line.starts_with("[exit: ") && !line.starts_with("[exit code: "))
        .any(|line| !line.trim().is_empty())
}

/// How many diff lines to show before the user expands the view with Ctrl+O.
const DIFF_PREVIEW_LINES: usize = 12;

/// Render a file edit as a colored, line-numbered diff, mirroring the opencode
/// edit view.
fn render_diff(
    diff: &DiffPreview,
    width: usize,
    expand_tools: bool,
    theme: &crate::theme::Theme,
    lines: &mut Vec<Line<'static>>,
) {
    lines.push(action_line(
        "Edited",
        &diff.path,
        theme.success,
        Modifier::BOLD,
        width,
    ));
    let all: Vec<&str> = diff.text.lines().collect();
    let limit = if expand_tools {
        all.len()
    } else {
        DIFF_PREVIEW_LINES.min(all.len())
    };
    for line in &all[..limit] {
        lines.push(Line::from(Span::styled(
            truncate(line, width),
            diff_line_style(line, theme),
        )));
    }
    if limit < all.len() {
        push_collapsed_hint(lines, width, all.len() - limit, theme.info);
    }
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

fn wrap(text: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut out = Vec::new();
    for raw in text.split('\n') {
        if raw.is_empty() {
            out.push(String::new());
            continue;
        }
        let mut line = String::new();
        let mut count = 0usize;
        for word in raw.split_inclusive(' ') {
            let len = word.chars().count();
            if count > 0 && count + len > width {
                out.push(std::mem::take(&mut line));
                count = 0;
            }
            if len > width {
                for ch in word.chars() {
                    if count >= width {
                        out.push(std::mem::take(&mut line));
                        count = 0;
                    }
                    line.push(ch);
                    count += 1;
                }
            } else {
                line.push_str(word);
                count += len;
            }
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_rows_grows_and_clamps() {
        assert_eq!(input_rows("", 10), MIN_INPUT_ROWS);
        assert_eq!(input_rows("hello", 10), MIN_INPUT_ROWS);
        assert_eq!(input_rows("hello\nworld", 10), MIN_INPUT_ROWS);
        assert_eq!(input_rows(&"a".repeat(200), 10), MAX_INPUT_ROWS);
    }

    #[test]
    fn input_scroll_follows_tail() {
        assert_eq!(input_scroll("hi", 10), 0);
        assert_eq!(
            input_scroll(&"a".repeat(200), 10),
            (20 - MAX_INPUT_ROWS) as u16
        );
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
    fn justified_right_aligns_and_truncates() {
        let line = justified_line(vec![Span::raw("ab")], Line::from(Span::raw("cd")), 6);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "ab  cd");

        let line = justified_line(
            vec![Span::raw("left")],
            Line::from(Span::raw("long-right")),
            8,
        );
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text.chars().count(), 8);
        assert!(text.ends_with('…'));

        let line = justified_line(
            vec![Span::raw("very-long-left-side")],
            Line::from(Span::raw("right")),
            12,
        );
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "very-… right");
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("hello", 10), "hello");
        assert_eq!(truncate("hello", 3), "he…");
        assert_eq!(truncate("hello", 0), "");
        assert_eq!(truncate("héllo", 2), "h…");
    }

    #[test]
    fn banner_centers_or_falls_back_when_narrow() {
        let mut wide = Vec::new();
        render_banner(80, &mut wide);
        assert_eq!(wide.len(), BANNER.len());

        let mut narrow = Vec::new();
        render_banner(10, &mut narrow);
        assert_eq!(narrow.len(), 1);
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
        assert_eq!(
            wrap("the quick brown fox", 9),
            ["the ", "quick ", "brown fox"]
        );
        assert_eq!(wrap("a".repeat(25).as_str(), 10).len(), 3);
    }

    fn line_text(line: &Line) -> String {
        line.spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect()
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
        assert_eq!(line_text(&lines[0]), "→ Read src/main.rs");

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
        assert_eq!(line_text(&lines[0]), "→ Edit src/main.rs");

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "read_file".into(),
                args: args.into(),
                output: "     1\tfn main() {}".into(),
                diff: None,
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(line_text(&lines[0]), "→ Read src/main.rs");

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "write_file".into(),
                args: args.into(),
                output: "wrote 12 bytes to /x".into(),
                diff: None,
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(line_text(&lines[0]), "→ Edited src/main.rs");
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
        assert_eq!(line_text(&lines[0]), "→ Run cargo test --all");

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "bash".into(),
                args: args.into(),
                output: "ok\n[exit: 0]".into(),
                diff: None,
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(line_text(&lines[0]), "→ Ran cargo test --all · exit 0");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].spans[1].style.fg, Some(Color::LightGreen));

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "bash".into(),
                args: args.into(),
                output: "boom\n[exit: 1]".into(),
                diff: None,
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(
            line_text(&lines[0]),
            "→ Run failed cargo test --all · exit 1"
        );
        assert_eq!(lines[0].spans[1].style.fg, Some(Color::LightRed));
    }

    #[test]
    fn badges_use_dark_text_on_bright_backgrounds() {
        for mode in [Mode::Build, Mode::AutoEdit, Mode::Plan] {
            assert_eq!(mode_style(mode).fg, Some(Color::Black));
        }
        for reasoning in [
            Reasoning::Auto,
            Reasoning::Off,
            Reasoning::Low,
            Reasoning::Medium,
            Reasoning::High,
        ] {
            assert_eq!(reasoning_style(reasoning).fg, Some(Color::Black));
        }
    }

    #[test]
    fn long_bash_command_truncates_to_one_line() {
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
        assert_eq!(lines.len(), 1);
        let text = line_text(&lines[0]);
        assert_eq!(text.chars().count(), 40);
        assert!(text.ends_with('…'));
    }

    #[test]
    fn tool_output_is_hidden_until_expanded() {
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
        assert_eq!(expanded.len(), 10);
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
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(line_text(&lines[0]), "→ Edited src/main.rs");
        assert_eq!(lines[1].spans[0].style.fg, Some(Color::Gray));
        assert_eq!(lines[2].spans[0].style.fg, Some(Color::LightRed));
        assert_eq!(lines[3].spans[0].style.fg, Some(Color::LightGreen));
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
    fn status_and_model_share_the_row_below_the_input_box() {
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
        let mut terminal = Terminal::new(TestBackend::new(69, 24)).unwrap();
        terminal.draw(|frame| draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer().clone();

        let input_top = row_of(&buffer, "╭").expect("input box border");
        let status = row_of(&buffer, "ready").expect("status row");
        assert!(
            status > input_top,
            "status should render below the input box (input at {input_top}, status at {status})"
        );

        let input_bottom = row_of(&buffer, "╰").expect("input box bottom border");
        assert_eq!(status, input_bottom + 1);
        assert_eq!(row_of(&buffer, "gpt-4o"), Some(status));

        let status_text: String = (0..buffer.area.width)
            .map(|x| buffer[(x, status)].symbol())
            .collect();
        assert!(!status_text.contains("Enter send"));
        assert!(!status_text.contains("Ctrl+O"));
    }
}
