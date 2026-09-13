use crate::config::{Mode, Reasoning};
use crate::tui::app::{App, ChatItem, ConnectStep};
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap,
};
use ratatui::Frame;

const MAX_INPUT_ROWS: usize = 8;
const MAX_MODEL_ROWS: usize = 12;
const MAX_SUGGESTION_ROWS: usize = 8;

/// A rounded panel with a colored border and title, shared by the conversation,
/// input and popup surfaces.
fn panel(title: &str, color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ))
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let input_width = frame.area().width.saturating_sub(2) as usize;
    let input_rows = input_rows(&app.input, input_width) as u16;
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(input_rows + 2),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_header(frame, app, chunks[0]);
    draw_messages(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);
    draw_status(frame, app, chunks[3]);

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
    let cwd = format!("{} ", app.cwd);
    let cwd_width = cwd.chars().count() as u16;
    let cols = Layout::horizontal([Constraint::Min(0), Constraint::Length(cwd_width)]).split(area);

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
    frame.render_widget(Paragraph::new(title), cols[0]);
    frame.render_widget(
        Paragraph::new(
            Line::from(Span::styled(cwd, Style::default().fg(Color::DarkGray)))
                .alignment(Alignment::Right),
        ),
        cols[1],
    );
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
    let block = panel("conversation", Color::DarkGray);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width.saturating_sub(2) as usize;
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
        render_item(&app.items[index], width, &mut app.lines);
        app.lines.push(Line::from(""));
    }
}

fn render_item(item: &ChatItem, width: usize, lines: &mut Vec<Line<'static>>) {
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
                Span::styled(format!(" {args}"), Style::default().fg(Color::DarkGray)),
            ]));
        }
        ChatItem::ToolProgress { name, output } => {
            lines.push(Line::from(vec![
                Span::styled("⋯ ", Style::default().fg(Color::DarkGray)),
                Span::styled(name.clone(), Style::default().fg(Color::DarkGray)),
            ]));
            push_wrapped(lines, output, width, Style::default().fg(Color::DarkGray));
        }
        ChatItem::ToolResult { name, output } => {
            lines.push(Line::from(vec![
                Span::styled("↳ ", Style::default().fg(Color::DarkGray)),
                Span::styled(name.clone(), Style::default().fg(Color::DarkGray)),
            ]));
            push_wrapped(lines, output, width, Style::default().fg(Color::DarkGray));
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
        "message".to_string()
    } else {
        format!("message · {} attachment(s)", app.attachments.len())
    };
    let block = panel(&title, border_color);
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let width = inner.width as usize;
    let paragraph = Paragraph::new(app.input.as_str())
        .wrap(Wrap { trim: false })
        .scroll((input_scroll(&app.input, width), 0));
    frame.render_widget(paragraph, inner);

    if !app.busy && app.connect.is_none() && app.models.is_none() {
        let lines = wrap(&app.input, width);
        let last = lines.last().map(|line| line.chars().count()).unwrap_or(0);
        let x = inner.x + last as u16;
        let x = x.min(inner.x + inner.width.saturating_sub(1));
        let y = inner.y + input_rows(&app.input, width) as u16 - 1;
        frame.set_cursor_position((x, y));
    }
}

fn input_rows(input: &str, width: usize) -> usize {
    wrap(input, width).len().clamp(1, MAX_INPUT_ROWS)
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
    let hint = if app.busy {
        let secs = app
            .busy_since
            .map(|start| start.elapsed().as_secs())
            .unwrap_or(0);
        format!("working… {secs}s · Esc to cancel")
    } else {
        "Enter send · / commands · Shift+Tab mode · Ctrl+R reasoning · Ctrl+C quit".to_string()
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.status),
            Style::default()
                .fg(status_fg)
                .bg(status_bg)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::styled(hint, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
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
        for ch in raw.chars() {
            if count >= width {
                out.push(std::mem::take(&mut line));
                count = 0;
            }
            line.push(ch);
            count += 1;
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
        assert_eq!(input_rows("", 10), 1);
        assert_eq!(input_rows("hello", 10), 1);
        assert_eq!(input_rows("hello\nworld", 10), 2);
        assert_eq!(input_rows(&"a".repeat(100), 10), MAX_INPUT_ROWS);
    }

    #[test]
    fn input_scroll_follows_tail() {
        assert_eq!(input_scroll("hi", 10), 0);
        assert_eq!(
            input_scroll(&"a".repeat(100), 10),
            (10 - MAX_INPUT_ROWS) as u16
        );
    }
}
