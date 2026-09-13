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
const COLLAPSE_MIN_LINES: usize = 4;

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
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(input_rows + 2),
            Constraint::Length(2),
        ])
        .split(frame.area());

    draw_header(frame, app, chunks[0]);
    draw_messages(frame, app, chunks[1]);
    draw_cwd(frame, app, chunks[2]);
    draw_input(frame, app, chunks[3]);
    draw_status(frame, app, chunks[4]);

    if app.connect.is_some() {
        draw_connect(frame, app);
    } else if app.models.is_some() {
        draw_models(frame, app);
    } else if !app.suggestions.is_empty() {
        draw_suggestions(frame, app, chunks[1]);
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

fn draw_header(frame: &mut Frame, app: &App, area: Rect) {
    let title = Line::from(vec![
        Span::styled(
            " oxide ",
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled("model ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            app.model.clone(),
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(format!(" {} ", app.mode.label()), mode_style(app.mode)),
        Span::raw(" "),
        Span::styled(
            format!(" {} ", app.reasoning.label()),
            reasoning_style(app.reasoning),
        ),
    ]);
    frame.render_widget(Paragraph::new(title), area);
}

fn draw_cwd(frame: &mut Frame, app: &App, area: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!(" {}", display_path(&app.cwd)),
            Style::default().fg(Color::DarkGray),
        ))),
        area,
    );
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
        app.lines.push(Line::from(""));
    }
}

fn render_item(item: &ChatItem, width: usize, expand_tools: bool, lines: &mut Vec<Line<'static>>) {
    let bold = Modifier::BOLD;
    match item {
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
        ChatItem::ToolProgress { name, output } => {
            lines.push(Line::from(vec![
                Span::styled("⋯ ", Style::default().fg(Color::DarkGray)),
                Span::styled(name.clone(), Style::default().fg(Color::DarkGray)),
            ]));
            push_tool_body(lines, name, output, width, expand_tools);
        }
        ChatItem::ToolResult { name, output } => {
            lines.push(Line::from(vec![
                Span::styled("↳ ", Style::default().fg(Color::DarkGray)),
                Span::styled(name.clone(), Style::default().fg(Color::DarkGray)),
            ]));
            push_tool_body(lines, name, output, width, expand_tools);
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

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let (status_fg, status_bg) = if app.busy {
        (Color::Black, Color::Yellow)
    } else if app.status == "ready" {
        (Color::Black, Color::Green)
    } else {
        (Color::White, Color::DarkGray)
    };
    let pill = format!(" {} ", app.status);
    let pad = " ".repeat(pill.chars().count() + 2);
    let (primary, secondary) = if app.busy {
        let secs = app
            .busy_since
            .map(|start| start.elapsed().as_secs())
            .unwrap_or(0);
        (
            format!("{secs}s elapsed · Esc to cancel"),
            "↑/↓ scroll conversation".to_string(),
        )
    } else {
        (
            "Enter send · Shift+Tab mode · Ctrl+R reasoning".to_string(),
            "/ commands · Ctrl+O tools · Ctrl+C quit · ↑/↓ scroll".to_string(),
        )
    };
    let dim = Style::default().fg(Color::DarkGray);
    let lines = vec![
        Line::from(vec![
            Span::styled(
                pill,
                Style::default()
                    .fg(status_fg)
                    .bg(status_bg)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(primary, dim),
        ]),
        Line::from(Span::styled(format!("{pad}{secondary}"), dim)),
    ];
    frame.render_widget(Paragraph::new(lines), area);
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

/// Render a tool's output, collapsing long file reads/writes to a single
/// summary line unless the user expands them with Ctrl+O.
fn push_tool_body(
    lines: &mut Vec<Line<'static>>,
    name: &str,
    output: &str,
    width: usize,
    expand_tools: bool,
) {
    let dim = Style::default().fg(Color::DarkGray);
    let line_count = output.lines().count();
    if is_file_tool(name) && !expand_tools && line_count >= COLLAPSE_MIN_LINES {
        lines.push(Line::from(Span::styled(
            format!("  {line_count} lines collapsed · Ctrl+O to expand"),
            dim,
        )));
        return;
    }
    push_wrapped(lines, output, width, dim);
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
}
