use crate::config::{Mode, Reasoning};
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

const FILE_TOOLS: [&str; 3] = ["read_file", "write_file", "patch"];

/// Persistent keybinding reminder pinned above the input box, kept to two
/// short lines so the full set stays visible on an 80-column terminal.
const TIPS: [&str; 2] = [
    "Enter send · Shift+Tab mode · Ctrl+R reasoning · Ctrl+O tools · Ctrl+C quit",
    "↑/↓ history · PgUp/PgDn/wheel scroll",
];

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
    FILE_TOOLS.contains(&name)
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
            Constraint::Length(2),
            Constraint::Length(input_rows + 2),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_messages(frame, app, chunks[0]);
    draw_tips(frame, chunks[1]);
    draw_input(frame, app, chunks[2]);
    draw_info(frame, app, chunks[3]);
    draw_footer(frame, app, chunks[4]);

    if app.connect.is_some() {
        draw_connect(frame, app);
    } else if app.models.is_some() {
        draw_models(frame, app);
    } else if !app.suggestions.is_empty() {
        draw_suggestions(frame, app, chunks[0]);
    }
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
    let area = centered_rect(70, 40, frame.area());
    frame.render_widget(Clear, area);

    let (prompt, value) = match &state.step {
        ConnectStep::Provider => (
            "Provider: 1 openai · 2 deepseek · 3 anthropic, or type a name",
            state.input.clone(),
        ),
        ConnectStep::Key { .. } => ("Enter API key", "*".repeat(state.input.chars().count())),
    };
    let title = match &state.step {
        ConnectStep::Provider => " connect ",
        ConnectStep::Key { provider } => provider.as_str(),
    };

    let mut lines = vec![
        Line::from(Span::styled(prompt, Style::default().fg(Color::DarkGray))),
        Line::from(""),
    ];
    if let Some(error) = &state.error {
        lines.push(Line::from(Span::styled(
            format!("error: {error}"),
            Style::default().fg(Color::Red),
        )));
        lines.push(Line::from(""));
    }
    lines.push(Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::Cyan)),
        Span::styled(value, Style::default().fg(Color::White)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Enter confirm · Esc cancel",
        Style::default().fg(Color::DarkGray),
    )));

    let block = panel(title, Color::Cyan);
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
    let block = panel(&title, Color::Cyan);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if state.loading {
        frame.render_widget(
            Paragraph::new(Span::styled(
                "loading models…",
                Style::default().fg(Color::DarkGray),
            )),
            inner,
        );
        return;
    }
    if let Some(error) = &state.error {
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!("error: {error}"),
                Style::default().fg(Color::Red),
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
                Style::default().fg(Color::DarkGray),
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
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
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
                Span::styled(format!("/{}", hint.name), Style::default().fg(Color::Cyan)),
                Span::styled(
                    format!("  {}", hint.description),
                    Style::default().fg(Color::DarkGray),
                ),
            ]))
        })
        .collect();
    let block = panel("commands", Color::DarkGray);
    let list = List::new(items)
        .block(block)
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("> ");
    let mut list_state = ListState::default();
    list_state.select(Some(app.suggestion_index.saturating_sub(offset)));
    frame.render_stateful_widget(list, popup, &mut list_state);
}

/// Compact mode · model · reasoning line shown just below the input, with the
/// current status right-aligned.
fn draw_info(frame: &mut Frame, app: &App, area: Rect) {
    let left = vec![
        Span::styled(format!(" {} ", app.mode.label()), mode_style(app.mode)),
        Span::raw(" "),
        Span::styled(
            app.model.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!(" {} ", app.reasoning.label()),
            reasoning_style(app.reasoning),
        ),
    ];
    let status_style = if app.busy {
        Style::default().fg(Color::Yellow)
    } else if app.status == "ready" {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };
    let right = Span::styled(app.status.clone(), status_style);
    frame.render_widget(
        Paragraph::new(justified(left, right, area.width as usize)),
        area,
    );
}

/// Always-visible keybinding reminder pinned directly above the input box.
fn draw_tips(frame: &mut Frame, area: Rect) {
    let lines: Vec<Line> = TIPS
        .iter()
        .map(|tip| {
            Line::from(Span::styled(
                format!(" {tip}"),
                Style::default().fg(Color::DarkGray),
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// Bottom bar: working directory on the left, elapsed time on the right.
fn draw_footer(frame: &mut Frame, app: &App, area: Rect) {
    let dim = Style::default().fg(Color::DarkGray);
    let width = area.width as usize;
    let right = if app.busy {
        let secs = app
            .busy_since
            .map(|start| start.elapsed().as_secs())
            .unwrap_or(0);
        Span::styled(
            format!("{secs}s · Esc to cancel "),
            Style::default().fg(Color::Yellow),
        )
    } else {
        Span::raw("")
    };
    let full = format!(" {}", display_path(&app.cwd));
    let short = format!(
        " {}",
        app.cwd
            .rsplit('/')
            .next()
            .filter(|name| !name.is_empty())
            .unwrap_or(&app.cwd)
    );
    let left = if full.chars().count() + right.content.chars().count() + 40 <= width {
        full
    } else {
        short
    };
    frame.render_widget(
        Paragraph::new(justified(vec![Span::styled(left, dim)], right, width)),
        area,
    );
}

/// Lay out left-aligned spans and a right-aligned span, truncating the right
/// side with an ellipsis when the row is too narrow for both.
fn justified(left: Vec<Span<'static>>, right: Span<'static>, width: usize) -> Line<'static> {
    let left_len: usize = left.iter().map(|span| span.content.chars().count()).sum();
    let right_len = right.content.chars().count();
    let mut spans = left;
    if left_len + right_len <= width {
        spans.push(Span::raw(" ".repeat(width - left_len - right_len)));
        spans.push(right);
    } else if width > left_len + 1 {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            truncate(&right.content, width - left_len - 1),
            right.style,
        ));
    }
    Line::from(spans)
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

fn mode_color(mode: Mode) -> Color {
    match mode {
        Mode::Build => Color::Cyan,
        Mode::AutoEdit => Color::Yellow,
        Mode::Plan => Color::Magenta,
    }
}

fn mode_style(mode: Mode) -> Style {
    let (fg, bg) = match mode {
        Mode::Build => (Color::Black, Color::Cyan),
        Mode::AutoEdit => (Color::Black, Color::Yellow),
        Mode::Plan => (Color::Black, Color::Magenta),
    };
    Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD)
}

fn reasoning_style(reasoning: Reasoning) -> Style {
    let (fg, bg) = match reasoning {
        Reasoning::Auto => (Color::Black, Color::Green),
        Reasoning::Off => (Color::White, Color::DarkGray),
        Reasoning::Low => (Color::Black, Color::Cyan),
        Reasoning::Medium => (Color::White, Color::Blue),
        Reasoning::High => (Color::White, Color::Magenta),
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

    let paragraph = Paragraph::new(app.lines.clone())
        .wrap(Wrap { trim: false })
        .scroll((app.scroll, 0));
    frame.render_widget(paragraph, inner);
}

/// Incrementally rebuild the rendered lines, reusing everything before the
/// first changed conversation item. Items are append-mostly, so a cache keyed by
/// per-item signatures keeps redraws proportional to what actually changed.
fn sync_lines(app: &mut App, width: usize) {
    if app.render_width != width {
        app.lines.clear();
        app.line_offsets.clear();
        app.signatures.clear();
        app.render_width = width;
    }

    let count = app.items.len();
    if app.signatures.len() > count {
        let cut = app
            .line_offsets
            .get(count)
            .copied()
            .unwrap_or(app.lines.len());
        app.lines.truncate(cut);
        app.line_offsets.truncate(count);
        app.signatures.truncate(count);
    }

    let mut start = 0;
    while start < count
        && start < app.signatures.len()
        && app.signatures[start] == app.items[start].signature()
    {
        start += 1;
    }
    if start == count {
        return;
    }

    let cut = app
        .line_offsets
        .get(start)
        .copied()
        .unwrap_or(app.lines.len());
    app.lines.truncate(cut);
    app.line_offsets.truncate(start);
    app.signatures.truncate(start);

    for index in start..count {
        let signature = app.items[index].signature();
        let offset = app.lines.len();
        app.line_offsets.push(offset);
        app.signatures.push(signature);
        render_item(&app.items[index], width, app.expand_tools, &mut app.lines);
        if app.lines.len() > offset {
            app.lines.push(Line::from(""));
        }
    }
}

fn render_item(item: &ChatItem, width: usize, expand_tools: bool, lines: &mut Vec<Line<'static>>) {
    let bold = Modifier::BOLD;
    match item {
        ChatItem::Banner => render_banner(width, lines),
        ChatItem::User(text) => {
            lines.push(Line::from(vec![
                Span::styled("❯ ", Style::default().fg(Color::Cyan).add_modifier(bold)),
                Span::styled("you", Style::default().fg(Color::Cyan).add_modifier(bold)),
            ]));
            push_wrapped(lines, text, width, Style::default());
        }
        ChatItem::Assistant(text) => {
            lines.push(Line::from(vec![
                Span::styled("◆ ", Style::default().fg(Color::Green).add_modifier(bold)),
                Span::styled(
                    "oxide",
                    Style::default().fg(Color::Green).add_modifier(bold),
                ),
            ]));
            push_wrapped(lines, text, width, Style::default());
        }
        ChatItem::Tool { name, args } => {
            if let Some(path) = file_tool_path(name, args) {
                let (verb, color) = if name == "write_file" {
                    ("Edit", Color::Yellow)
                } else {
                    ("Read", Color::Cyan)
                };
                lines.push(action_line(verb, &path, color, bold, width));
            } else if let Some(command) = bash_command(name, args) {
                lines.push(action_line("Run", &command, Color::Blue, bold, width));
            } else {
                lines.push(Line::from(vec![
                    Span::styled("⚙ ", Style::default().fg(Color::Yellow)),
                    Span::styled(
                        name.clone(),
                        Style::default().fg(Color::Yellow).add_modifier(bold),
                    ),
                    Span::styled(
                        format!(" {}", tool_arg_summary(name, args)),
                        Style::default().fg(Color::DarkGray),
                    ),
                ]));
            }
        }
        ChatItem::ToolProgress { name, output } => {
            if name != "bash" {
                lines.push(Line::from(vec![
                    Span::styled("⋯ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(name.clone(), Style::default().fg(Color::DarkGray)),
                ]));
            }
            push_tool_body(lines, output, width, expand_tools);
        }
        ChatItem::ToolResult { name, args, output } => {
            if let Some(path) = file_tool_path(name, args) {
                if name == "read_file" {
                    if output.starts_with("error:") {
                        push_wrapped(lines, output, width, Style::default().fg(Color::Red));
                    }
                } else if output.starts_with("error:") {
                    lines.push(action_line("Edit failed", &path, Color::Red, bold, width));
                    push_wrapped(lines, output, width, Style::default().fg(Color::Red));
                } else {
                    lines.push(action_line("Edited", &path, Color::Green, bold, width));
                    if let Some((_, rest)) = output.split_once("\n\n") {
                        if !rest.trim().is_empty() {
                            push_wrapped(lines, rest, width, Style::default().fg(Color::DarkGray));
                        }
                    }
                }
            } else if let Some(command) = bash_command(name, args) {
                let exit = bash_exit_code(output);
                let failed = exit
                    .map(|code| code != 0)
                    .unwrap_or_else(|| output.starts_with("error:"));
                let color = if failed { Color::Red } else { Color::Green };
                lines.push(action_line("Ran", &command, color, bold, width));
                if exit.is_none() && !output.trim().is_empty() {
                    push_wrapped(lines, output, width, Style::default().fg(Color::Red));
                }
            } else {
                lines.push(Line::from(vec![
                    Span::styled("↳ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(name.clone(), Style::default().fg(Color::DarkGray)),
                ]));
                push_tool_body(lines, output, width, expand_tools);
            }
        }
        ChatItem::Error(text) => {
            push_wrapped(
                lines,
                &format!("✗ {text}"),
                width,
                Style::default().fg(Color::Red),
            );
        }
        ChatItem::Info(text) => {
            push_wrapped(
                lines,
                &format!("· {text}"),
                width,
                Style::default().fg(Color::DarkGray),
            );
        }
    }
}

/// Centers the block-letter wordmark, falling back to plain text when the
/// terminal is too narrow for the art.
fn render_banner(width: usize, lines: &mut Vec<Line<'static>>) {
    let art_width = BANNER
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0);
    if art_width > width {
        lines.push(Line::from(Span::styled(
            "oxide",
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )));
        return;
    }
    let colors = [
        Color::Cyan,
        Color::LightCyan,
        Color::LightMagenta,
        Color::Magenta,
        Color::LightMagenta,
        Color::Cyan,
    ];
    for (index, art) in BANNER.iter().enumerate() {
        let pad = (width - art.chars().count()) / 2;
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(pad)),
            Span::styled(
                (*art).to_string(),
                Style::default()
                    .fg(colors[index % colors.len()])
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
    }
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let border_color = if app.busy {
        Color::DarkGray
    } else {
        mode_color(app.mode)
    };
    let title = if app.attachments.is_empty() {
        String::new()
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
    let paragraph = Paragraph::new(app.input.as_str())
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
    if name != "read_file" && name != "write_file" {
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
        Span::styled(format!(" {subject}"), Style::default().fg(Color::DarkGray)),
    ])
}

/// Shell command for a `bash` call, flattened to a single line for display.
fn bash_command(name: &str, args: &str) -> Option<String> {
    if name != "bash" {
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
        .find_map(|line| line.strip_prefix("[exit code: "))
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
    let summary = match name {
        "write_file" => value
            .get("content")
            .and_then(|v| v.as_str())
            .map(|content| format!("{path} · {} lines", content.lines().count())),
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
fn push_tool_body(lines: &mut Vec<Line<'static>>, output: &str, width: usize, expand_tools: bool) {
    if !expand_tools {
        return;
    }
    push_wrapped(lines, output, width, Style::default().fg(Color::DarkGray));
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
    fn justified_right_aligns_and_truncates() {
        let line = justified(vec![Span::raw("ab")], Span::raw("cd"), 6);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
        assert_eq!(text, "ab  cd");

        let line = justified(vec![Span::raw("left")], Span::raw("long-right"), 8);
        let text: String = line
            .spans
            .iter()
            .map(|span| span.content.as_ref())
            .collect();
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
            },
            80,
            false,
            &mut lines,
        );
        assert!(lines.is_empty());

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "write_file".into(),
                args: args.into(),
                output: "wrote 12 bytes to /x".into(),
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(line_text(&lines[0]), "→ Edited src/main.rs");
    }

    #[test]
    fn bash_renders_run_and_ran_actions() {
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
                output: "ok\n[exit code: 0]".into(),
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(line_text(&lines[0]), "→ Ran cargo test --all");
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].spans[1].style.fg, Some(Color::Green));

        let mut lines = Vec::new();
        render_item(
            &ChatItem::ToolResult {
                name: "bash".into(),
                args: args.into(),
                output: "boom\n[exit code: 1]".into(),
            },
            80,
            false,
            &mut lines,
        );
        assert_eq!(lines[0].spans[1].style.fg, Some(Color::Red));
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
}
