use crate::tui::app::{App, ChatItem};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

pub fn draw(frame: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(3),
            Constraint::Length(1),
        ])
        .split(frame.area());

    draw_header(frame, app, chunks[0]);
    draw_messages(frame, app, chunks[1]);
    draw_input(frame, app, chunks[2]);
    draw_status(frame, app, chunks[3]);
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
        Span::styled(app.model.clone(), Style::default().fg(Color::Cyan)),
        Span::styled("  ·  ", Style::default().fg(Color::DarkGray)),
        Span::styled(app.cwd.clone(), Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(Paragraph::new(title), area);
}

fn draw_messages(frame: &mut Frame, app: &mut App, area: Rect) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title(Span::styled(
            " conversation ",
            Style::default().fg(Color::DarkGray),
        ));
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
    match item {
        ChatItem::User(text) => {
            lines.push(Line::from(Span::styled(
                "you",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )));
            push_wrapped(lines, text, width, Style::default());
        }
        ChatItem::Assistant(text) => {
            lines.push(Line::from(Span::styled(
                "assistant",
                Style::default()
                    .fg(Color::Green)
                    .add_modifier(Modifier::BOLD),
            )));
            push_wrapped(lines, text, width, Style::default());
        }
        ChatItem::Tool { name, args } => {
            lines.push(Line::from(Span::styled(
                format!("tool: {name} {args}"),
                Style::default().fg(Color::Yellow),
            )));
        }
        ChatItem::ToolProgress { name, output } => {
            lines.push(Line::from(Span::styled(
                format!("progress: {name}"),
                Style::default().fg(Color::DarkGray),
            )));
            push_wrapped(lines, output, width, Style::default().fg(Color::DarkGray));
        }
        ChatItem::ToolResult { name, output } => {
            lines.push(Line::from(Span::styled(
                format!("result: {name}"),
                Style::default().fg(Color::DarkGray),
            )));
            push_wrapped(lines, output, width, Style::default().fg(Color::DarkGray));
        }
        ChatItem::Error(text) => {
            push_wrapped(
                lines,
                &format!("error: {text}"),
                width,
                Style::default().fg(Color::Red),
            );
        }
        ChatItem::Info(text) => {
            push_wrapped(lines, text, width, Style::default().fg(Color::DarkGray));
        }
    }
}

fn draw_input(frame: &mut Frame, app: &App, area: Rect) {
    let border_color = if app.busy {
        Color::DarkGray
    } else {
        Color::Cyan
    };
    let title = if app.attachments.is_empty() {
        " message ".to_string()
    } else {
        format!(" message · {} attachment(s) ", app.attachments.len())
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(title, Style::default().fg(border_color)));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let paragraph = Paragraph::new(app.input.clone()).wrap(Wrap { trim: false });
    frame.render_widget(paragraph, inner);

    if !app.busy {
        let x = inner.x + app.input.chars().count() as u16;
        let x = x.min(inner.x + inner.width.saturating_sub(1));
        frame.set_cursor_position((x, inner.y));
    }
}

fn draw_status(frame: &mut Frame, app: &App, area: Rect) {
    let hint = if app.busy {
        "working…  Esc to quit"
    } else {
        "Enter send · @image · Ctrl+V paste · ↑/↓ scroll · Ctrl+C quit"
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.status),
            Style::default().fg(Color::Black).bg(Color::DarkGray),
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
        for ch in raw.chars() {
            if line.chars().count() >= width {
                out.push(std::mem::take(&mut line));
            }
            line.push(ch);
        }
        out.push(line);
    }
    out
}
