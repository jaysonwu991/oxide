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
use crate::tui::rows::{Join, RowLink, Rows};

/// Renders `text` as styled rows that fit `width` columns, keeping `indent`
/// columns free on the first line for a prefix that shares it.
fn render_body(text: &str, width: usize, indent: usize, palette: &Palette) -> Rows {
    let text = crate::tools::sanitize_terminal_output(text);
    let (blocks, links) = parse_blocks(&text, palette);
    let mut renderer = Renderer::new(width, palette, links);
    renderer.first_width = Some(width.saturating_sub(indent));
    renderer.render_blocks(&blocks, &[], true);
    renderer.finish()
}

/// Renders `text` and places a speaker `prefix` inline on the first line.
///
/// The prefix shares the first line, so that line is wrapped to what is left of
/// `width` and the rest of the body flows at the full width. Wrapping the body
/// first and re-wrapping only its first line would break the paragraph a second
/// time and leave an orphaned line behind.
pub(crate) fn render_with_prefix(
    text: &str,
    width: usize,
    prefix: Vec<Span<'static>>,
    theme: &Theme,
) -> Rows {
    let indent: usize = prefix.iter().map(|span| span.content.chars().count()).sum();
    let palette = Palette::new(theme);
    let mut rows = render_body(text, width, indent, &palette);
    let Some((first, _, links)) = rows.remove_first() else {
        rows.push(Line::from(prefix));
        return rows;
    };
    let mut spans = prefix;
    // The links were recorded against the body's own spans, which now sit
    // behind the prefix that shares the line.
    let shift = spans.len();
    let links = links
        .into_iter()
        .map(|link| RowLink {
            span: link.span + shift,
            ..link
        })
        .collect();
    spans.extend(first.spans);
    rows.insert_first(Line::from(spans), links);
    rows
}

/// Colors and modifiers the renderer draws from, derived from the active theme.
struct Palette {
    text: Style,
    heading: Style,
    inline_code: Style,
    code_block: Style,
    link: Style,
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

/// The links a reply drew, by the label each was drawn under: a link reads as
/// its label alone, so the row it lands on keeps the target for the click.
type Links = Vec<(String, String)>;

/// The URL the link-style text `label` names. A label the pane wrapped is drawn
/// as one span per row, each holding a piece of it, so a piece that names
/// exactly one label answers as well — unless the piece is a URL of its own,
/// which is drawn as a link whether or not a label holds it.
fn link_url(links: &[(String, String)], label: &str) -> Option<String> {
    if let Some((_, url)) = links.iter().find(|(text, _)| text == label) {
        return Some(url.clone());
    }
    if is_url(label) {
        return None;
    }
    let mut pieces = links.iter().filter(|(text, _)| text.contains(label));
    let (_, url) = pieces.next()?;
    pieces.next().is_none().then(|| url.clone())
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
/// whitespace and hard-splitting words that are wider than the line. The first
/// line may be given a narrower `first_width`, for content that shares it with
/// a prefix that is not part of `chars`. Every row after the first carries how
/// it attaches to the row above it, so the copy can put the words back on the
/// line the pane broke.
fn wrap_chars(chars: &[StyledChar], width: usize, first_width: usize) -> Vec<Row> {
    let width = width.max(1);
    let mut limit = first_width.max(1);
    let mut lines: Vec<Row> = Vec::new();
    let mut current: Vec<StyledChar> = Vec::new();
    let mut pending_space = false;
    let mut pending_space_style = Style::default();
    // How the row being built attaches to the row above it, decided by the
    // break that started it: the first row of the text is a line of its own.
    let mut join = Join::Line;
    let mut i = 0;
    while i < chars.len() {
        if chars[i].ch == '\n' {
            // A newline in the parsed inline text is a line the text itself
            // starts, so the copy keeps the break.
            lines.push((coalesce(&current), join));
            current.clear();
            pending_space = false;
            limit = width;
            join = Join::Line;
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
        if !current.is_empty() && current.len() + space + word.len() > limit {
            // The pane broke here between two words, so the copy puts a space
            // back; a word it had to split is joined with nothing at all.
            lines.push((coalesce(&current), join));
            current.clear();
            pending_space = false;
            limit = width;
            join = if space == 1 {
                Join::Space { prefix: 0 }
            } else {
                Join::Word { prefix: 0 }
            };
        }
        if current.is_empty() && word.len() > limit {
            let mut offset = 0;
            while offset < word.len() {
                let take = (word.len() - offset).min(limit);
                current.extend_from_slice(&word[offset..offset + take]);
                if offset + take < word.len() {
                    lines.push((coalesce(&current), join));
                    current.clear();
                    limit = width;
                    join = Join::Word { prefix: 0 };
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
        lines.push((coalesce(&current), join));
    }
    lines
}

/// One wrapped row: its spans, and how it attaches to the row above it.
type Row = (Vec<Span<'static>>, Join);

struct Renderer<'a> {
    width: usize,
    palette: &'a Palette,
    /// The links this reply drew, by label, for the rows that carry one.
    links: Links,
    rows: Rows,
    /// Width budget for the first line this renderer emits, for a speaker prefix
    /// that shares it. Consumed by the first line actually wrapped.
    first_width: Option<usize>,
}

impl<'a> Renderer<'a> {
    fn new(width: usize, palette: &'a Palette, links: Links) -> Self {
        Self {
            width: width.max(1),
            palette,
            links,
            rows: Rows::default(),
            first_width: None,
        }
    }

    /// The content width for the line about to be wrapped, and for the lines
    /// after it. `first_width` is the width of the first *line*, which a speaker
    /// prefix shares, so the indent that line already carries comes off it.
    fn take_first(&mut self, available: usize, indent: usize) -> (usize, usize) {
        match self.first_width.take() {
            Some(first) => (first.saturating_sub(indent).clamp(1, available), available),
            None => (available, available),
        }
    }

    fn finish(mut self) -> Rows {
        let is_blank =
            |line: &Line<'static>| line.spans.iter().all(|span| span.content.trim().is_empty());
        while self.rows.lines.last().map(is_blank).unwrap_or(false) {
            self.rows.pop();
        }
        let leading = self
            .rows
            .lines
            .iter()
            .take_while(|line| is_blank(line))
            .count();
        self.rows.drain_front(leading);
        self.rows
    }

    fn blank(&mut self) {
        if !self.rows.is_empty() {
            self.rows.push(Line::from(""));
        }
    }

    fn available(&self, ambient: &[Span<'static>]) -> usize {
        self.width.saturating_sub(indent_width(ambient)).max(1)
    }

    fn push(&mut self, spans: Vec<Span<'static>>) {
        self.push_join(spans, Join::Line);
    }

    /// A row the pane wrapped: its text is the rest of the line above it. Every
    /// row records the links it draws, since the URL of a labelled link is not
    /// part of the row's text.
    fn push_join(&mut self, spans: Vec<Span<'static>>, join: Join) {
        let links = self.row_links(&spans);
        self.rows.push_join_links(Line::from(spans), join, links);
    }

    /// The links a row draws: every span the palette drew as a link, with the
    /// URL its label names.
    fn row_links(&self, spans: &[Span<'static>]) -> Vec<RowLink> {
        if self.links.is_empty() {
            return Vec::new();
        }
        spans
            .iter()
            .enumerate()
            .filter(|(_, span)| span.style == self.palette.link)
            .filter_map(|(span, drawn)| {
                link_url(&self.links, &drawn.content).map(|url| RowLink {
                    span,
                    start: 0,
                    end: drawn.content.chars().count(),
                    url,
                })
            })
            .collect()
    }

    fn wrap(&mut self, spans: &[Span<'static>], ambient: &[Span<'static>]) {
        let width = self.available(ambient);
        let chars = flatten(spans);
        let (first, rest) = if chars.is_empty() {
            (width, width)
        } else {
            self.take_first(width, indent_width(ambient))
        };
        // Every row is drawn after the ambient decoration (a quote's `│ `, a
        // list's hanging indent), and each row records how much of it to take
        // back off when the row is copied.
        let prefix = indent_width(ambient);
        for (line, join) in wrap_chars(&chars, rest, first) {
            let mut spans = ambient.to_vec();
            spans.extend(line);
            self.push_join(spans, join.after(prefix));
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
                let (first, _) = self.take_first(width, indent_width(ambient));
                let mut spans = ambient.to_vec();
                spans.push(Span::styled("─".repeat(first), self.palette.rule));
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
        let (first, rest) = self.take_first(width, indent_width(ambient));
        let mut limit = first;
        if lines.is_empty() {
            let mut spans = ambient.to_vec();
            spans.push(Span::styled(" ".repeat(limit), self.palette.code_block));
            self.push(spans);
            return;
        }
        for raw in lines {
            let chars: Vec<char> = raw.chars().collect();
            if chars.is_empty() {
                let mut spans = ambient.to_vec();
                spans.push(Span::styled(" ".repeat(limit), self.palette.code_block));
                self.push(spans);
                limit = rest;
                continue;
            }
            let mut offset = 0;
            let mut first_row = true;
            while offset < chars.len() {
                let take = (chars.len() - offset).min(limit);
                let text: String = chars[offset..offset + take].iter().collect();
                let padding = limit - take;
                let mut spans = ambient.to_vec();
                spans.push(Span::styled(
                    format!("{text}{}", " ".repeat(padding)),
                    self.palette.code_block,
                ));
                // A code line the pane split stays one line when it is copied,
                // with the padding and the indent it was drawn after dropped.
                let join = if first_row {
                    Join::Line
                } else {
                    Join::Split {
                        prefix: indent_width(ambient),
                    }
                };
                self.push_join(spans, join);
                limit = rest;
                offset += take;
                first_row = false;
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

            let mut sub = Renderer::new(self.width, self.palette, self.links.clone());
            sub.first_width = self.first_width.take();
            sub.render_blocks(blocks, &cont, false);
            let pending = sub.first_width.take();
            let mut item_rows = sub.finish();

            if item_rows.is_empty() {
                let mut spans = ambient.to_vec();
                spans.push(Span::styled(marker, self.palette.bullet));
                self.push(spans);
                self.first_width = pending;
                continue;
            }
            let Some((first, _, _)) = item_rows.remove_first() else {
                continue;
            };
            let stripped = strip_prefix(first, indent_width(&cont));
            let mut spans = ambient.to_vec();
            spans.push(Span::styled(marker, self.palette.bullet));
            spans.extend(stripped);
            self.push(spans);
            // The rest of the item keeps the joins it was wrapped with, so a
            // continuation of its first line is one line again when copied.
            self.rows.append(&mut item_rows);
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
        // A table is laid out as one block, so the narrower budget of a first
        // line shared with a prefix applies to all of its rows.
        let (first, rest) = self.take_first(avail, indent_width(ambient));
        let avail = first.min(rest);
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
            let cell: &[Span<'static>] = cells
                .get(index)
                .map(|spans| spans.as_slice())
                .unwrap_or(&[]);
            spans.extend(pad_spans(cell, *width, style));
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

/// A cell's spans, padded or shortened to `width`, keeping the styles they were
/// parsed with so a link in the cell is still drawn as the link's own span.
fn pad_spans(spans: &[Span<'static>], width: usize, style: Style) -> Vec<Span<'static>> {
    let count = span_width(spans);
    if count <= width {
        let mut out = spans.to_vec();
        if count < width {
            out.push(Span::styled(" ".repeat(width - count), style));
        }
        return out;
    }
    if width == 0 {
        return Vec::new();
    }
    let mut remaining = width - 1;
    let mut out = Vec::new();
    for span in spans {
        if remaining == 0 {
            break;
        }
        let take = span.content.chars().count().min(remaining);
        out.push(Span::styled(
            span.content.chars().take(take).collect::<String>(),
            span.style,
        ));
        remaining -= take;
    }
    out.push(Span::styled("…", style));
    out
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

fn parse_blocks(text: &str, palette: &Palette) -> (Vec<Block>, Links) {
    let lines: Vec<String> = text.lines().map(|line| line.to_string()).collect();
    let mut links = Links::new();
    let blocks = parse_lines(&lines, palette, &mut links);
    (blocks, links)
}

fn parse_lines(lines: &[String], palette: &Palette, links: &mut Links) -> Vec<Block> {
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
            blocks.push(Block::Quote(parse_lines(&inner, palette, links)));
            continue;
        }
        if let Some(text) = heading(line) {
            blocks.push(Block::Heading(parse_inline(
                text,
                palette.heading,
                palette,
                links,
            )));
            i += 1;
            continue;
        }
        if is_rule(line) {
            blocks.push(Block::Rule);
            i += 1;
            continue;
        }
        if list_marker(line).is_some() {
            let (block, next) = parse_list(lines, i, palette, links);
            blocks.push(block);
            i = next;
            continue;
        }
        if is_table_start(lines, i) {
            let (block, next) = parse_table(lines, i, palette, links);
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
            paragraph.push(parse_inline(
                current.trim_end(),
                palette.text,
                palette,
                links,
            ));
            i += 1;
        }
        blocks.push(Block::Paragraph(paragraph));
    }
    blocks
}

fn parse_list(
    lines: &[String],
    start: usize,
    palette: &Palette,
    links: &mut Links,
) -> (Block, usize) {
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
        items.push(parse_lines(&item, palette, links));
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

fn parse_table(
    lines: &[String],
    start: usize,
    palette: &Palette,
    links: &mut Links,
) -> (Block, usize) {
    let header = split_cells(&lines[start])
        .iter()
        .map(|cell| parse_inline(cell, palette.table_header, palette, links))
        .collect();
    let mut rows = Vec::new();
    let mut i = start + 2;
    while i < lines.len() && !lines[i].trim().is_empty() && lines[i].contains('|') {
        rows.push(
            split_cells(&lines[i])
                .iter()
                .map(|cell| parse_inline(cell, palette.text, palette, links))
                .collect(),
        );
        i += 1;
    }
    (Block::Table { header, rows }, i)
}

fn parse_inline(
    text: &str,
    style: Style,
    palette: &Palette,
    links: &mut Links,
) -> Vec<Span<'static>> {
    let mut out = Vec::new();
    parse_inline_into(text, style, palette, links, &mut out);
    coalesce_spans(out)
}

fn flush(literal: &mut String, style: Style, out: &mut Vec<Span<'static>>) {
    if !literal.is_empty() {
        out.push(Span::styled(std::mem::take(literal), style));
    }
}

fn parse_inline_into(
    text: &str,
    style: Style,
    palette: &Palette,
    links: &mut Links,
    out: &mut Vec<Span<'static>>,
) {
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
                    links,
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
                    links,
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
                    links,
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
                    links,
                    out,
                );
                i += 1 + end + 1;
                continue;
            }
        }

        if rest.starts_with("![") || rest.starts_with('[') {
            if let Some((len, spans)) = parse_link(rest, style, palette, links) {
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

        // A URL written without brackets is still a link, like Pi's autolink
        // handling; the reader can click it to open the browser.
        if let Some((len, url)) = take_bare_url(rest) {
            let boundary = text[..i]
                .chars()
                .next_back()
                .map(|c| !c.is_alphanumeric())
                .unwrap_or(true);
            if boundary {
                flush(&mut literal, style, out);
                out.push(Span::styled(url.to_string(), palette.link));
                i += len;
                continue;
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

/// The label a link is drawn as, and the URL it names. The target is recorded
/// with the palette rather than drawn, so a link reads as its label the way it
/// does in every other front-end, and a click is answered from what was
/// recorded.
fn parse_link(
    rest: &str,
    style: Style,
    palette: &Palette,
    links: &mut Links,
) -> Option<(usize, Vec<Span<'static>>)> {
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
    }
    let label_start = spans.len();
    parse_inline_into(label, style, palette, links, &mut spans);
    if spans.len() == label_start {
        spans.push(Span::styled(url.to_string(), palette.link));
    }
    for span in spans[label_start..].iter_mut() {
        span.style = palette.link;
    }
    if !url.is_empty() {
        links.push((spans_text(&spans[label_start..]), url.to_string()));
    }

    Some((close + 1 + 1 + url_end + 1, spans))
}

fn is_url(text: &str) -> bool {
    text.starts_with("http://") || text.starts_with("https://") || text.starts_with("mailto:")
}

/// The bare URL beginning `rest` and the bytes it spans. Trailing sentence
/// punctuation is left to the surrounding text so the link it produces ends at
/// the URL.
pub(crate) fn take_bare_url(rest: &str) -> Option<(usize, &str)> {
    if !is_url(rest) {
        return None;
    }
    let end = rest
        .char_indices()
        .find(|(_, ch)| {
            ch.is_whitespace() || matches!(ch, '<' | '>' | '"' | '\'' | '`' | ')' | ']')
        })
        .map(|(index, _)| index)
        .unwrap_or(rest.len());
    let url = rest[..end].trim_end_matches(['.', ',', ';', ':', '!', '?']);
    (!url.is_empty()).then_some((url.len(), url))
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

    fn render(markdown: &str, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        render_with_prefix(markdown, width, Vec::new(), theme).lines
    }

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

    fn speaker_prefix() -> Vec<Span<'static>> {
        vec![Span::styled(
            "\u{25c6} Oxide ".to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        )]
    }

    fn line_width(line: &Line) -> usize {
        line.spans
            .iter()
            .map(|span| span.content.chars().count())
            .sum()
    }

    #[test]
    fn a_reply_that_fills_the_first_line_reflows_instead_of_orphaning_a_tail() {
        let body = "**Short answer: no code change is needed in either, but both do get the new behavior \u{2014} and I found one real asymmetry while checking, which I fixed.**";
        let lines = render_with_prefix(body, 137, speaker_prefix(), &Theme::dark()).lines;
        let rendered: Vec<String> = lines
            .iter()
            .map(|line| text(std::slice::from_ref(line)))
            .collect();

        assert_eq!(
            rendered,
            vec![
                "\u{25c6} Oxide Short answer: no code change is needed in either, but both do get the new behavior \u{2014} and I found one real asymmetry while".to_string(),
                "checking, which I fixed.".to_string(),
            ]
        );
        assert!(line_width(&lines[0]) <= 137);
    }

    #[test]
    fn a_speaker_prefix_never_widens_a_line_past_the_pane() {
        let bodies = [
            "a paragraph long enough that it has to wrap more than once across the pane",
            "## A heading that runs on for long enough to wrap",
            "- a bullet whose text is long enough to wrap onto another line\n- a second bullet",
            "1. an ordered item that also wraps when the pane is narrow\n2. another",
            "> a quoted paragraph that wraps across several lines of the pane",
            "```\nlet value = \"a code line long enough that it has to be chunked\";\n```",
            "| column one | column two |\n| --- | --- |\n| a long cell value | another long cell value |",
            "---",
            "`inline code` and **bold** and a tail of words that keeps going",
        ];

        for width in [16usize, 24, 40, 61, 80] {
            for body in bodies {
                let lines = render_with_prefix(body, width, speaker_prefix(), &Theme::dark()).lines;
                for line in &lines {
                    let rendered = text(std::slice::from_ref(line));
                    assert!(
                        line_width(line) <= width,
                        "{width} cells wide: {rendered:?}"
                    );
                }
            }
        }
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
    fn a_link_reads_as_its_label_and_keeps_its_target_beside_the_row() {
        let theme = Theme::dark();
        let rows = render_with_prefix(
            "see [docs](https://example.com) now",
            80,
            Vec::new(),
            &theme,
        );
        assert_eq!(
            rows.lines
                .iter()
                .map(|line| line
                    .spans
                    .iter()
                    .map(|span| span.content.as_ref())
                    .collect::<String>())
                .collect::<Vec<_>>()
                .join("\n"),
            "see docs now"
        );
        assert_eq!(
            rows.links,
            vec![vec![RowLink {
                span: 1,
                start: 0,
                end: 4,
                url: "https://example.com".to_string(),
            }]]
        );
    }

    #[test]
    fn a_label_the_pane_wrapped_still_names_its_target() {
        let theme = Theme::dark();
        let rows = render_with_prefix(
            "[a release by its version](https://example.com/pull/176)",
            18,
            Vec::new(),
            &theme,
        );
        assert!(rows.lines.len() > 1, "the label wrapped");
        assert_eq!(rows.links.len(), rows.lines.len());
        assert!(
            rows.links.iter().all(|links| links.len() == 1),
            "every row of the label names its target: {:?}",
            rows.links
        );
    }

    #[test]
    fn a_link_a_speaker_prefix_shares_the_line_still_names_its_target() {
        let rows = render_with_prefix(
            "see [docs](https://example.com)",
            80,
            speaker_prefix(),
            &Theme::dark(),
        );
        let link = rows.links[0].first().expect("the link's row");
        assert_eq!(rows.lines[0].spans[link.span].content.as_ref(), "docs");
        assert_eq!(link.url, "https://example.com");
    }

    #[test]
    fn a_link_in_a_table_names_its_target_too() {
        let theme = Theme::dark();
        let rows = render_with_prefix(
            "| pr | state |\n| --- | --- |\n| [#176](https://example.com/176) | open |",
            60,
            Vec::new(),
            &theme,
        );
        let (row, link) = rows
            .links
            .iter()
            .enumerate()
            .flat_map(|(row, links)| links.iter().map(move |link| (row, link)))
            .find(|(_, link)| link.url == "https://example.com/176")
            .expect("the table cell's link");
        assert_eq!(rows.lines[row].spans[link.span].content.as_ref(), "#176");
    }

    #[test]
    fn an_image_is_drawn_as_a_picture_and_a_link_without_a_label_as_its_url() {
        assert_eq!(
            render_text("![shot](https://example.com/a.png)", 80),
            "🖼 shot"
        );
        assert_eq!(
            render_text("[](https://example.com)", 80),
            "https://example.com"
        );
    }

    #[test]
    fn bare_urls_are_styled_as_links() {
        let theme = Theme::dark();
        let lines = render("see https://example.com now", 80, &theme);
        let span = lines[0]
            .spans
            .iter()
            .find(|span| span.content.contains("https://example.com"))
            .expect("url span");
        assert!(span.style.add_modifier.contains(Modifier::UNDERLINED));
        assert_eq!(text(&lines), "see https://example.com now");

        // A URL glued to a word is not a link, and a trailing period stays text.
        let lines = render("xhttps://example.com", 80, &theme);
        assert!(!lines[0]
            .spans
            .iter()
            .any(|span| span.style.add_modifier.contains(Modifier::UNDERLINED)));
        let lines = render("see https://example.com.", 80, &theme);
        assert_eq!(text(&lines), "see https://example.com.");
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
    fn prefix_lands_inline_on_the_first_line() {
        let theme = Theme::dark();
        let prefix = vec![Span::raw("◆ oxide ")];
        let lines = render_with_prefix("## Summary", 40, prefix, &theme).lines;
        assert_eq!(text(&lines), "◆ oxide Summary");
    }

    /// What a copy of `markdown` carries, which is the rows put back together
    /// without the pane's own wrapping.
    fn copy(markdown: &str, width: usize) -> String {
        let rows = render_body(markdown, width, 0, &Palette::new(&Theme::dark()));
        crate::tui::rows::text(
            rows.lines
                .iter()
                .zip(&rows.joins)
                .map(|(line, join)| (text(std::slice::from_ref(line)), *join)),
        )
    }

    #[test]
    fn a_copy_of_a_wrapped_quote_carries_one_bar_however_narrow_the_pane_is() {
        let body = "the refused read names the app that holds the grant, not the tool that asked";
        for width in [24usize, 30, 40, 61] {
            let panel = render(&format!("> {body}"), width, &Theme::dark());
            assert!(panel.len() > 1, "the pane wrapped the quote at {width}");
            let copied = copy(&format!("> {body}"), width);
            assert_eq!(copied, format!("│ {body}"), "copied at {width}");
            assert_eq!(copied.matches('│').count(), 1, "copied at {width}");
        }
    }

    #[test]
    fn a_copy_of_a_wrapped_quote_keeps_a_word_the_pane_split_in_two() {
        let quote = "> the grant belongs to /Applications/Visual Studio Code.app and not to oxide";
        let copied = copy(quote, 26);
        assert_eq!(
            copied,
            "│ the grant belongs to /Applications/Visual Studio Code.app and not to oxide"
        );
    }

    #[test]
    fn a_copy_of_a_split_code_line_keeps_the_whitespace_inside_it() {
        let rows = render_body(
            "```\nreturn  answer;\n```",
            8,
            0,
            &Palette::new(&Theme::dark()),
        );
        let rendered: Vec<String> = rows
            .lines
            .iter()
            .map(|line| text(std::slice::from_ref(line)))
            .collect();
        assert_eq!(rendered.len(), 2, "the pane split the line: {rendered:?}");
        assert_eq!(
            rendered,
            vec!["return  ".to_string(), "answer; ".to_string()]
        );
        assert_eq!(copy("```\nreturn  answer;\n```", 8), "return  answer;");
    }
}
