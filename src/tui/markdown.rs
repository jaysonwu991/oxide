//! Dependency-free Markdown rendering for assistant messages.
//!
//! Parses the subset of Markdown that models typically emit — headings,
//! paragraphs, fenced code, lists, blockquotes, rules, tables, and inline
//! emphasis, code, and links — and lays it out as styled `ratatui` lines that
//! fit a given width. The parser tolerates partial input so a reply can be
//! rendered while it is still streaming.

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::theme::Theme;

/// Renders `text` as styled lines that fit `width` columns.
pub(crate) fn render(text: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
    let palette = Palette::new(theme);
    let blocks = parse_blocks(text, &palette);
    let mut renderer = Renderer::new(width, &palette);
    renderer.render_blocks(&blocks, &[], true);
    renderer.finish()
}

/// Renders `text` and places a speaker `prefix` inline on the first line,
/// re-wrapping that line so the prefix never overflows `width`.
pub(crate) fn render_with_prefix(
    text: &str,
    width: usize,
    prefix: Vec<Span<'static>>,
    theme: &Theme,
) -> Vec<Line<'static>> {
    let mut lines = render(text, width, theme);
    if lines.is_empty() {
        return vec![Line::from(prefix)];
    }
    let first = lines.remove(0);
    let mut spans = prefix;
    spans.extend(first.spans);
    let mut merged = wrap_spans(&spans, width);
    merged.extend(lines);
    merged
}

/// Colors and modifiers the renderer draws from, derived from the active theme.
struct Palette {
    text: Style,
    heading: Style,
    inline_code: Style,
    code_block: Style,
    link: Style,
    link_url: Style,
    bullet: Style,
    quote: Style,
    rule: Style,
    table_header: Style,
    dim: Style,
}

impl Palette {
    fn new(theme: &Theme) -> Self {
        let code_bg = theme.tool_pending_bg;
        Self {
            text: Style::default(),
            heading: Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
            inline_code: Style::default().fg(theme.tool).bg(code_bg),
            code_block: Style::default().fg(theme.info).bg(code_bg),
            link: Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::UNDERLINED),
            link_url: Style::default().fg(theme.dim),
            bullet: Style::default().fg(theme.accent),
            quote: Style::default().fg(theme.dim),
            rule: Style::default().fg(theme.dim),
            table_header: Style::default()
                .fg(theme.accent)
                .add_modifier(Modifier::BOLD),
            dim: Style::default().fg(theme.dim),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Block {
    Heading(Vec<Span<'static>>),
    /// Paragraph lines, each already parsed into inline spans so a hard line
    /// break in the source stays a line break on screen.
    Paragraph(Vec<Vec<Span<'static>>>),
    Code(Vec<String>),
    List {
        ordered: bool,
        start: u64,
        items: Vec<Vec<Block>>,
    },
    Quote(Vec<Block>),
    Rule,
    Table {
        header: Vec<Vec<Span<'static>>>,
        rows: Vec<Vec<Vec<Span<'static>>>>,
    },
}

/// A styled character, the unit wrapping operates on.
#[derive(Clone, Copy)]
struct StyledChar {
    ch: char,
    style: Style,
}

fn flatten(spans: &[Span<'static>]) -> Vec<StyledChar> {
    let mut out = Vec::new();
    for span in spans {
        for ch in span.content.chars() {
            out.push(StyledChar {
                ch,
                style: span.style,
            });
        }
    }
    out
}

fn coalesce(chars: &[StyledChar]) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    for c in chars {
        if let Some(last) = spans.last_mut() {
            if last.style == c.style {
                last.content.to_mut().push(c.ch);
                continue;
            }
        }
        spans.push(Span::styled(c.ch.to_string(), c.style));
    }
    spans
}

fn coalesce_spans(spans: Vec<Span<'static>>) -> Vec<Span<'static>> {
    let mut out: Vec<Span<'static>> = Vec::with_capacity(spans.len());
    for span in spans {
        if span.content.is_empty() {
            continue;
        }
        if let Some(last) = out.last_mut() {
            if last.style == span.style {
                last.content.to_mut().push_str(span.content.as_ref());
                continue;
            }
        }
        out.push(span);
    }
    out
}

fn span_width(spans: &[Span<'static>]) -> usize {
    spans.iter().map(|span| span.content.chars().count()).sum()
}

fn indent_width(spans: &[Span<'static>]) -> usize {
    span_width(spans)
}

fn spans_text(spans: &[Span<'static>]) -> String {
    spans.iter().map(|span| span.content.as_ref()).collect()
}

/// Word-wraps a flat sequence of styled characters, collapsing runs of
/// whitespace and hard-splitting words that are wider than the line.
fn wrap_chars(chars: &[StyledChar], width: usize) -> Vec<Vec<Span<'static>>> {
    let width = width.max(1);
    let mut lines: Vec<Vec<Span<'static>>> = Vec::new();
    let mut current: Vec<StyledChar> = Vec::new();
    let mut pending_space = false;
    let mut pending_space_style = Style::default();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].ch == '\n' {
            lines.push(coalesce(&current));
            current.clear();
            pending_space = false;
            i += 1;
            continue;
        }
        if chars[i].ch.is_whitespace() {
            pending_space = !current.is_empty();
            if pending_space {
                pending_space_style = chars[i].style;
            }
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && !chars[i].ch.is_whitespace() && chars[i].ch != '\n' {
            i += 1;
        }
        let word = &chars[start..i];
        let space = usize::from(pending_space && !current.is_empty());
        if !current.is_empty() && current.len() + space + word.len() > width {
            lines.push(coalesce(&current));
            current.clear();
            pending_space = false;
        }
        if current.is_empty() && word.len() > width {
            let mut offset = 0;
            while offset < word.len() {
                let take = (word.len() - offset).min(width);
                current.extend_from_slice(&word[offset..offset + take]);
                if offset + take < word.len() {
                    lines.push(coalesce(&current));
                    current.clear();
                }
                offset += take;
            }
        } else {
            if pending_space && !current.is_empty() {
                current.push(StyledChar {
                    ch: ' ',
                    style: pending_space_style,
                });
            }
            current.extend_from_slice(word);
        }
        pending_space = false;
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(coalesce(&current));
    }
    lines
}

fn wrap_spans(spans: &[Span<'static>], width: usize) -> Vec<Line<'static>> {
    let chars = flatten(spans);
    wrap_chars(&chars, width)
        .into_iter()
        .map(Line::from)
        .collect()
}

struct Renderer<'a> {
    width: usize,
    palette: &'a Palette,
    lines: Vec<Line<'static>>,
}

impl<'a> Renderer<'a> {
    fn new(width: usize, palette: &'a Palette) -> Self {
        Self {
            width: width.max(1),
            palette,
            lines: Vec::new(),
        }
    }

    fn finish(mut self) -> Vec<Line<'static>> {
        let is_blank =
            |line: &Line<'static>| line.spans.iter().all(|span| span.content.trim().is_empty());
        while self.lines.last().map(is_blank).unwrap_or(false) {
            self.lines.pop();
        }
        let leading = self.lines.iter().take_while(|line| is_blank(line)).count();
        self.lines.drain(..leading);
        self.lines
    }

    fn blank(&mut self) {
        if !self.lines.is_empty() {
            self.lines.push(Line::from(""));
        }
    }

    fn available(&self, ambient: &[Span<'static>]) -> usize {
        self.width.saturating_sub(indent_width(ambient)).max(1)
    }

    fn push(&mut self, spans: Vec<Span<'static>>) {
        self.lines.push(Line::from(spans));
    }

    fn wrap(&mut self, spans: &[Span<'static>], ambient: &[Span<'static>]) {
        let width = self.available(ambient);
        let chars = flatten(spans);
        for line in wrap_chars(&chars, width) {
            let mut spans = ambient.to_vec();
            spans.extend(line);
            self.push(spans);
        }
    }

    fn render_blocks(&mut self, blocks: &[Block], ambient: &[Span<'static>], spaced: bool) {
        for (index, block) in blocks.iter().enumerate() {
            if spaced && index > 0 {
                self.blank();
            }
            self.render_block(block, ambient);
        }
    }

    fn render_block(&mut self, block: &Block, ambient: &[Span<'static>]) {
        match block {
            Block::Heading(spans) => self.wrap(spans, ambient),
            Block::Paragraph(lines) => {
                for line in lines {
                    self.wrap(line, ambient);
                }
            }
            Block::Code(lines) => self.render_code(lines, ambient),
            Block::Rule => {
                let width = self.available(ambient);
                let mut spans = ambient.to_vec();
                spans.push(Span::styled("─".repeat(width), self.palette.rule));
                self.push(spans);
            }
            Block::Quote(inner) => {
                let mut nested = ambient.to_vec();
                nested.push(Span::styled("│ ", self.palette.quote));
                self.render_blocks(inner, &nested, true);
            }
            Block::List {
                ordered,
                start,
                items,
            } => self.render_list(*ordered, *start, items, ambient),
            Block::Table { header, rows } => self.render_table(header, rows, ambient),
        }
    }

    fn render_code(&mut self, lines: &[String], ambient: &[Span<'static>]) {
        let width = self.available(ambient);
        if lines.is_empty() {
            let mut spans = ambient.to_vec();
            spans.push(Span::styled(" ".repeat(width), self.palette.code_block));
            self.push(spans);
            return;
        }
        for raw in lines {
            let chars: Vec<char> = raw.chars().collect();
            if chars.is_empty() {
                let mut spans = ambient.to_vec();
                spans.push(Span::styled(" ".repeat(width), self.palette.code_block));
                self.push(spans);
                continue;
            }
            for piece in chars.chunks(width) {
                let text: String = piece.iter().collect();
                let padding = width - piece.len();
                let mut spans = ambient.to_vec();
                spans.push(Span::styled(
                    format!("{text}{}", " ".repeat(padding)),
                    self.palette.code_block,
                ));
                self.push(spans);
            }
        }
    }

    fn render_list(
        &mut self,
        ordered: bool,
        start: u64,
        items: &[Vec<Block>],
        ambient: &[Span<'static>],
    ) {
        for (index, blocks) in items.iter().enumerate() {
            let marker = if ordered {
                format!("{}. ", start + index as u64)
            } else {
                "• ".to_string()
            };
            let marker_width = marker.chars().count();
            let mut cont = ambient.to_vec();
            cont.push(Span::raw(" ".repeat(marker_width)));

            let mut sub = Renderer::new(self.width, self.palette);
            sub.render_blocks(blocks, &cont, false);
            let mut item_lines = sub.finish();

            if item_lines.is_empty() {
                let mut spans = ambient.to_vec();
                spans.push(Span::styled(marker, self.palette.bullet));
                self.push(spans);
                continue;
            }
            let first = item_lines.remove(0);
            let stripped = strip_prefix(first, indent_width(&cont));
            let mut spans = ambient.to_vec();
            spans.push(Span::styled(marker, self.palette.bullet));
            spans.extend(stripped);
            self.push(spans);
            self.lines.extend(item_lines);
        }
    }

    fn render_table(
        &mut self,
        header: &[Vec<Span<'static>>],
        rows: &[Vec<Vec<Span<'static>>>],
        ambient: &[Span<'static>],
    ) {
        let cols = header
            .len()
            .max(rows.iter().map(|r| r.len()).max().unwrap_or(0));
        if cols == 0 {
            return;
        }
        let avail = self.available(ambient);
        let mut widths = vec![1usize; cols];
        for (index, cell) in header.iter().enumerate() {
            widths[index] = widths[index].max(span_width(cell));
        }
        for row in rows {
            for (index, cell) in row.iter().enumerate() {
                if index < cols {
                    widths[index] = widths[index].max(span_width(cell));
                }
            }
        }
        let gap = 2;
        let total = |widths: &[usize]| widths.iter().sum::<usize>() + gap * (cols - 1);
        while total(&widths) > avail {
            let Some((index, _)) = widths
                .iter()
                .enumerate()
                .filter(|(_, w)| **w > 1)
                .max_by_key(|(_, w)| **w)
            else {
                break;
            };
            widths[index] -= 1;
        }

        self.push(self.table_row(header, &widths, gap, ambient, true));
        let mut separator = ambient.to_vec();
        for (index, width) in widths.iter().enumerate() {
            separator.push(Span::styled("─".repeat(*width), self.palette.rule));
            if index + 1 < cols {
                separator.push(Span::styled(" ".repeat(gap), self.palette.rule));
            }
        }
        self.push(separator);
        for row in rows {
            self.push(self.table_row(row, &widths, gap, ambient, false));
        }
    }

    fn table_row(
        &self,
        cells: &[Vec<Span<'static>>],
        widths: &[usize],
        gap: usize,
        ambient: &[Span<'static>],
        header: bool,
    ) -> Vec<Span<'static>> {
        let style = if header {
            self.palette.table_header
        } else {
            self.palette.text
        };
        let mut spans = ambient.to_vec();
        for (index, width) in widths.iter().enumerate() {
            let cell = cells
                .get(index)
                .map(|spans| spans_text(spans))
                .unwrap_or_default();
            spans.push(Span::styled(pad_cell(&cell, *width), style));
            if index + 1 < widths.len() {
                spans.push(Span::raw(" ".repeat(gap)));
            }
        }
        spans
    }
}

/// Removes the first `count` characters from a line, keeping the tail's styles.
fn strip_prefix(mut line: Line<'static>, count: usize) -> Vec<Span<'static>> {
    let mut remaining = count;
    let mut out = Vec::new();
    for span in line.spans.drain(..) {
        if remaining == 0 {
            out.push(span);
            continue;
        }
        let len = span.content.chars().count();
        if len <= remaining {
            remaining -= len;
        } else {
            let text: String = span.content.chars().skip(remaining).collect();
            remaining = 0;
            out.push(Span::styled(text, span.style));
        }
    }
    out
}

fn pad_cell(text: &str, width: usize) -> String {
    let count = text.chars().count();
    if count > width {
        if width == 0 {
            return String::new();
        }
        let mut truncated: String = text.chars().take(width.saturating_sub(1)).collect();
        truncated.push('…');
        truncated
    } else {
        format!("{text}{}", " ".repeat(width - count))
    }
}

fn leading_indent(line: &str) -> usize {
    line.chars().take_while(|c| *c == ' ' || *c == '\t').count()
}

fn deindent(line: &str, columns: usize) -> String {
    let mut index = 0;
    while index < columns && index < line.len() {
        let byte = line.as_bytes()[index];
        if byte == b' ' || byte == b'\t' {
            index += 1;
        } else {
            break;
        }
    }
    line[index..].to_string()
}

fn fence_open(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let fence: String = trimmed.chars().take_while(|c| *c == '`').collect();
    (fence.len() >= 3).then_some(fence)
}

fn fence_close(line: &str, fence: &str) -> bool {
    let trimmed = line.trim();
    trimmed.chars().filter(|c| *c == '`').count() >= fence.len()
        && trimmed.chars().all(|c| c == '`' || c.is_whitespace())
}

fn heading(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    let hashes = trimmed.chars().take_while(|c| *c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &trimmed[hashes..];
    if rest.is_empty() {
        return Some("");
    }
    if !rest.starts_with(' ') {
        return None;
    }
    Some(rest.trim().trim_end_matches('#').trim_end())
}

fn is_rule(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.chars().count() < 3 {
        return false;
    }
    let mut marker = None;
    for ch in trimmed.chars().filter(|c| !c.is_whitespace()) {
        if !matches!(ch, '-' | '*' | '_') {
            return false;
        }
        match marker {
            None => marker = Some(ch),
            Some(known) if known == ch => {}
            Some(_) => return false,
        }
    }
    marker.is_some()
}

struct Marker {
    ordered: bool,
    number: Option<u64>,
    indent: usize,
    /// Byte offset where the item content starts.
    content: usize,
}

fn list_marker(line: &str) -> Option<Marker> {
    let trimmed = line.trim_start();
    let indent = line.chars().count() - trimmed.chars().count();
    let base = line.len() - trimmed.len();
    for prefix in ["- ", "* ", "+ "] {
        if trimmed.starts_with(prefix) {
            return Some(Marker {
                ordered: false,
                number: None,
                indent,
                content: base + prefix.len(),
            });
        }
    }
    let digits: String = trimmed.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.len() > 9 {
        return None;
    }
    let after = &trimmed[digits.len()..];
    for separator in [". ", ") "] {
        if after.starts_with(separator) {
            return Some(Marker {
                ordered: true,
                number: digits.parse().ok(),
                indent,
                content: base + digits.len() + separator.len(),
            });
        }
    }
    None
}

fn is_table_separator(line: &str) -> bool {
    if !line.contains('|') {
        return false;
    }
    let cells = split_cells(line);
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.chars().all(|c| matches!(c, '-' | ':')) && cell.contains('-')
        })
}

fn is_table_start(lines: &[String], index: usize) -> bool {
    index + 1 < lines.len() && lines[index].contains('|') && is_table_separator(&lines[index + 1])
}

fn split_cells(line: &str) -> Vec<String> {
    let trimmed = line.trim().trim_matches('|');
    trimmed
        .split('|')
        .map(|cell| cell.trim().to_string())
        .collect()
}

fn starts_block(line: &str) -> bool {
    if line.trim().is_empty() {
        return false;
    }
    fence_open(line).is_some()
        || heading(line).is_some()
        || is_rule(line)
        || line.trim_start().starts_with('>')
        || list_marker(line).is_some()
}

fn parse_blocks(text: &str, palette: &Palette) -> Vec<Block> {
    let lines: Vec<String> = text.lines().map(|line| line.to_string()).collect();
    parse_lines(&lines, palette)
}

fn parse_lines(lines: &[String], palette: &Palette) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = &lines[i];
        if line.trim().is_empty() {
            i += 1;
            continue;
        }
        if let Some(fence) = fence_open(line) {
            let mut code = Vec::new();
            i += 1;
            while i < lines.len() && !fence_close(&lines[i], &fence) {
                code.push(lines[i].clone());
                i += 1;
            }
            if i < lines.len() {
                i += 1;
            }
            blocks.push(Block::Code(code));
            continue;
        }
        if line.trim_start().starts_with('>') {
            let mut inner = Vec::new();
            while i < lines.len() {
                let trimmed = lines[i].trim_start();
                let Some(rest) = trimmed.strip_prefix('>') else {
                    break;
                };
                inner.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
                i += 1;
            }
            blocks.push(Block::Quote(parse_lines(&inner, palette)));
            continue;
        }
        if let Some(text) = heading(line) {
            blocks.push(Block::Heading(parse_inline(text, palette.heading, palette)));
            i += 1;
            continue;
        }
        if is_rule(line) {
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if list_marker(line).is_some() {
            let (block, next) = parse_list(lines, i, palette);
            blocks.push(block);
            i = next;
            continue;
        }
        if is_table_start(lines, i) {
            let (block, next) = parse_table(lines, i, palette);
            blocks.push(block);
            i = next;
            continue;
        }
        let mut paragraph = Vec::new();
        while i < lines.len() {
            let current = &lines[i];
            if current.trim().is_empty() || starts_block(current) || is_table_start(lines, i) {
                break;
            }
            paragraph.push(parse_inline(current.trim_end(), palette.text, palette));
            i += 1;
        }
        blocks.push(Block::Paragraph(paragraph));
    }
    blocks
}

fn parse_list(lines: &[String], start: usize, palette: &Palette) -> (Block, usize) {
    let base = list_marker(&lines[start]).map(|m| m.indent).unwrap_or(0);
    let mut items: Vec<Vec<Block>> = Vec::new();
    let mut ordered = false;
    let mut number = 1u64;
    let mut i = start;
    while i < lines.len() {
        if lines[i].trim().is_empty() {
            let mut next = i + 1;
            while next < lines.len() && lines[next].trim().is_empty() {
                next += 1;
            }
            match lines.get(next).and_then(|line| list_marker(line)) {
                Some(marker) if marker.indent >= base => {
                    i = next;
                    continue;
                }
                _ => break,
            }
        }
        let Some(marker) = list_marker(&lines[i]) else {
            break;
        };
        if marker.indent != base {
            break;
        }
        if items.is_empty() {
            ordered = marker.ordered;
            number = marker.number.unwrap_or(1);
        } else if marker.ordered != ordered {
            break;
        }

        let mut item = vec![task_text(&lines[i][marker.content..])];
        let content_column = marker.content;
        i += 1;
        while i < lines.len() {
            if lines[i].trim().is_empty() {
                let mut next = i + 1;
                while next < lines.len() && lines[next].trim().is_empty() {
                    next += 1;
                }
                let belongs = match lines.get(next) {
                    Some(line) => match list_marker(line) {
                        Some(marker) => marker.indent > base,
                        None => leading_indent(line) > base,
                    },
                    None => false,
                };
                if belongs {
                    item.push(String::new());
                    i += 1;
                    continue;
                }
                break;
            }
            if leading_indent(&lines[i]) > base {
                item.push(deindent(&lines[i], content_column));
                i += 1;
            } else {
                break;
            }
        }
        items.push(parse_lines(&item, palette));
    }
    (
        Block::List {
            ordered,
            start: number,
            items,
        },
        i,
    )
}

/// Rewrites a leading `[ ]`/`[x]` task marker into a checkbox glyph.
fn task_text(content: &str) -> String {
    let trimmed = content.trim_start();
    for (marker, glyph) in [("[ ] ", "☐ "), ("[x] ", "☑ "), ("[X] ", "☑ ")] {
        if let Some(rest) = trimmed.strip_prefix(marker) {
            let indent = content.len() - trimmed.len();
            return format!("{}{glyph}{rest}", &content[..indent]);
        }
    }
    content.to_string()
}

fn parse_table(lines: &[String], start: usize, palette: &Palette) -> (Block, usize) {
    let header = split_cells(&lines[start])
        .iter()
        .map(|cell| parse_inline(cell, palette.table_header, palette))
        .collect();
    let mut rows = Vec::new();
    let mut i = start + 2;
    while i < lines.len() && !lines[i].trim().is_empty() && lines[i].contains('|') {
        rows.push(
            split_cells(&lines[i])
                .iter()
                .map(|cell| parse_inline(cell, palette.text, palette))
                .collect(),
        );
        i += 1;
    }
    (Block::Table { header, rows }, i)
}

fn parse_inline(text: &str, style: Style, palette: &Palette) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    parse_inline_into(text, style, palette, &mut out);
    coalesce_spans(out)
}

fn flush(literal: &mut String, style: Style, out: &mut Vec<Span<'static>>) {
    if !literal.is_empty() {
        out.push(Span::styled(std::mem::take(literal), style));
    }
}

fn parse_inline_into(text: &str, style: Style, palette: &Palette, out: &mut Vec<Span<'static>>) {
    let mut literal = String::new();
    let mut i = 0;
    while i < text.len() {
        let rest = &text[i..];

        if let Some(escaped) = rest.strip_prefix('\\') {
            if let Some(ch) = escaped.chars().next() {
                if ch.is_ascii_punctuation() {
                    literal.push(ch);
                    i += 1 + ch.len_utf8();
                    continue;
                }
            }
        }

        if rest.starts_with('`') {
            let ticks = rest.chars().take_while(|c| *c == '`').count();
            let marker = "`".repeat(ticks);
            if let Some(end) = rest[ticks..].find(&marker) {
                flush(&mut literal, style, out);
                let code = &rest[ticks..ticks + end];
                out.push(Span::styled(code.to_string(), palette.inline_code));
                i += ticks + end + ticks;
                continue;
            }
        }

        if rest.starts_with("**") || rest.starts_with("__") {
            let delimiter = &rest[..2];
            if let Some(end) = rest[2..].find(delimiter) {
                flush(&mut literal, style, out);
                parse_inline_into(
                    &rest[2..2 + end],
                    style.add_modifier(Modifier::BOLD),
                    palette,
                    out,
                );
                i += 2 + end + 2;
                continue;
            }
        }

        if let Some(stripped) = rest.strip_prefix("~~") {
            if let Some(end) = stripped.find("~~") {
                flush(&mut literal, style, out);
                parse_inline_into(
                    &stripped[..end],
                    style.add_modifier(Modifier::CROSSED_OUT),
                    palette,
                    out,
                );
                i += 2 + end + 2;
                continue;
            }
        }

        if rest.starts_with('*') && !rest.starts_with("**") {
            if let Some(end) = rest[1..].find('*') {
                flush(&mut literal, style, out);
                parse_inline_into(
                    &rest[1..1 + end],
                    style.add_modifier(Modifier::ITALIC),
                    palette,
                    out,
                );
                i += 1 + end + 1;
                continue;
            }
        }

        if rest.starts_with('_') && !rest.starts_with("__") {
            if let Some(end) = match_underscore(&rest[1..]) {
                flush(&mut literal, style, out);
                parse_inline_into(
                    &rest[1..1 + end],
                    style.add_modifier(Modifier::ITALIC),
                    palette,
                    out,
                );
                i += 1 + end + 1;
                continue;
            }
        }

        if rest.starts_with("![") || rest.starts_with('[') {
            if let Some((len, spans)) = parse_link(rest, style, palette) {
                flush(&mut literal, style, out);
                out.extend(spans);
                i += len;
                continue;
            }
        }

        if rest.starts_with('<') {
            if let Some(end) = rest.find('>') {
                let inner = &rest[1..end];
                if is_url(inner) {
                    flush(&mut literal, style, out);
                    out.push(Span::styled(inner.to_string(), palette.link));
                    i += end + 1;
                    continue;
                } else if looks_like_tag(inner) {
                    i += end + 1;
                    continue;
                }
            }
        }

        let ch = rest.chars().next().unwrap();
        literal.push(ch);
        i += ch.len_utf8();
    }
    flush(&mut literal, style, out);
}

/// Finds the closing `_` of an italic run, requiring word boundaries so
/// `snake_case` identifiers are left alone.
fn match_underscore(haystack: &str) -> Option<usize> {
    for (index, ch) in haystack.char_indices() {
        if ch == '_' {
            let next = haystack[index + 1..].chars().next();
            if next.map(|c| !c.is_alphanumeric()).unwrap_or(true) {
                return Some(index);
            }
        }
    }
    None
}

fn parse_link(rest: &str, style: Style, palette: &Palette) -> Option<(usize, Vec<Span<'static>>)> {
    if rest.starts_with("[^") || rest.starts_with("[ ") {
        return None;
    }
    let image = rest.starts_with("![");
    let text_start = if image { 2 } else { 1 };
    let close = rest[text_start..].find(']')? + text_start;
    let after = rest[close + 1..].strip_prefix('(')?;
    let url_end = after.find(')')?;
    let label = &rest[text_start..close];
    let url = after[..url_end].trim();

    let mut spans = Vec::new();
    if image {
        spans.push(Span::styled("🖼 ", palette.dim));
        let text = if label.is_empty() { url } else { label };
        spans.push(Span::styled(text.to_string(), palette.link));
    } else {
        parse_inline_into(label, style, palette, &mut spans);
        for span in spans.iter_mut() {
            span.style = palette.link;
        }
        if !url.is_empty() && url != label {
            spans.push(Span::styled(format!(" ({url})"), palette.link_url));
        }
    }
    Some((close + 1 + 1 + url_end + 1, spans))
}

fn is_url(text: &str) -> bool {
    text.starts_with("http://") || text.starts_with("https://") || text.starts_with("mailto:")
}

fn looks_like_tag(text: &str) -> bool {
    !text.is_empty()
        && !text.contains(char::is_whitespace)
        && text
            .chars()
            .next()
            .map(|c| c.is_alphabetic() || c == '/' || c == '!')
            .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(lines: &[Line]) -> String {
        lines
            .iter()
            .map(|line| {
                line.spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_text(markdown: &str, width: usize) -> String {
        text(&render(markdown, width, &Theme::dark()))
    }

    #[test]
    fn headings_drop_hashes_and_bold_their_text() {
        let theme = Theme::dark();
        let lines = render("## Summary", 40, &theme);
        assert_eq!(text(&lines), "Summary");
        assert!(lines[0]
            .spans
            .iter()
            .any(|span| span.style.add_modifier.contains(Modifier::BOLD)));
        assert_eq!(lines[0].spans[0].style.fg, Some(theme.accent));
    }

    #[test]
    fn inline_emphasis_code_and_strike_are_styled() {
        let theme = Theme::dark();
        let lines = render("a **bold** *ital* `code` ~~gone~~", 80, &theme);
        assert_eq!(text(&lines), "a bold ital code gone");
        let has = |modifier| {
            lines[0]
                .spans
                .iter()
                .any(|span| span.style.add_modifier.contains(modifier))
        };
        assert!(has(Modifier::BOLD));
        assert!(has(Modifier::ITALIC));
        assert!(has(Modifier::CROSSED_OUT));
        assert!(lines[0]
            .spans
            .iter()
            .any(|span| span.content.as_ref() == "code"
                && span.style.fg == Some(theme.tool)
                && span.style.bg == Some(theme.tool_pending_bg)));
    }

    #[test]
    fn unclosed_emphasis_stays_literal_while_streaming() {
        assert_eq!(render_text("**still typing", 40), "**still typing");
        assert_eq!(render_text("`partial", 40), "`partial");
    }

    #[test]
    fn fenced_code_keeps_its_lines_and_background() {
        let theme = Theme::dark();
        let lines = render("before\n\n```\nlet x = 1;\n```\n\nafter", 20, &theme);
        let body = text(&lines);
        assert!(body.contains("let x = 1;"));
        let code_line = lines
            .iter()
            .find(|line| text(std::slice::from_ref(line)).contains("let x = 1;"))
            .unwrap();
        assert_eq!(code_line.spans[0].style.bg, Some(theme.tool_pending_bg));
    }

    #[test]
    fn unordered_and_ordered_lists_render_markers() {
        assert_eq!(render_text("- one\n- two", 40), "• one\n• two");
        assert_eq!(render_text("1. one\n2. two", 40), "1. one\n2. two");
    }

    #[test]
    fn nested_lists_indent_under_the_parent_item() {
        let body = render_text("- one\n  - child\n- two", 40);
        assert_eq!(body, "• one\n  • child\n• two");
    }

    #[test]
    fn blockquotes_get_a_bar_and_rules_fill_the_width() {
        assert_eq!(render_text("> quoted", 40), "│ quoted");
        let rule = render_text("---", 10);
        assert_eq!(rule, "─".repeat(10));
    }

    #[test]
    fn links_show_their_target() {
        let body = render_text("[docs](https://example.com)", 80);
        assert_eq!(body, "docs (https://example.com)");
    }

    #[test]
    fn tables_align_into_columns() {
        let body = render_text("| a | b |\n| --- | --- |\n| 1 | 2 |", 40);
        let lines: Vec<&str> = body.lines().collect();
        assert!(lines[0].starts_with("a "));
        assert!(lines[0].contains('b'));
        assert!(lines[1].starts_with('─'));
        assert!(lines[2].starts_with('1'));
    }

    #[test]
    fn wrapped_lines_never_exceed_the_width() {
        let body = render_text("word ".repeat(40).trim(), 20);
        for line in body.lines() {
            assert!(line.chars().count() <= 20, "{line:?}");
        }
    }

    #[test]
    fn prefix_lands_inline_and_is_rewrapped() {
        let theme = Theme::dark();
        let prefix = vec![Span::raw("◆ oxide ")];
        let lines = render_with_prefix("## Summary", 40, prefix, &theme);
        assert_eq!(text(&lines), "◆ oxide Summary");
    }
}
