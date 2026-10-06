//! The transcript's rendered rows and how each one attaches to the row above
//! it. A row drawn because the pane is too narrow is the pane's line break, not
//! the text's, so a copy joins those rows back into the sentence they came from
//! instead of keeping the width the pane happened to be drawn at.

use ratatui::text::Line;

/// How a rendered row attaches to the row above it. Every continuation carries
/// how many cells of the pane's own decoration — a hanging indent, or the `│ `
/// a quote is drawn behind — the row was drawn after, since that decoration is
/// the pane's and not the text's.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Join {
    /// The row starts a line the text itself has.
    #[default]
    Line,
    /// The row continues the one above, which the wrap broke at a space: the
    /// join puts that space back.
    Space { prefix: usize },
    /// The row continues the one above inside a word, so the join puts the
    /// word back together with nothing between the pieces.
    Word { prefix: usize },
    /// The row is the rest of a line the pane split. The line's own whitespace
    /// is part of the text, so the join runs the pieces back together as they
    /// are and drops only the indent the pane drew.
    Split { prefix: usize },
}

impl Join {
    /// The same attachment, on a row this pane drew after `prefix` cells of its
    /// own decoration, so a copy can take that back off it.
    pub fn after(self, prefix: usize) -> Self {
        match self {
            Join::Line => Join::Line,
            Join::Space { .. } => Join::Space { prefix },
            Join::Word { .. } => Join::Word { prefix },
            Join::Split { .. } => Join::Split { prefix },
        }
    }
}

/// A link a rendered row carries: the row's own span it is drawn as, the
/// character range of that span it covers, and the URL it names. A labelled
/// link is drawn as its label, so the target is nowhere in the row's text for
/// a click to find and is recorded here instead.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct RowLink {
    pub span: usize,
    pub start: usize,
    pub end: usize,
    pub url: String,
}

/// Rendered rows and, for each one, how it attaches to the row above it and
/// the links it was drawn with.
#[derive(Default)]
pub struct Rows {
    pub lines: Vec<Line<'static>>,
    pub joins: Vec<Join>,
    /// Parallel to [`Rows::lines`]: the links each row names, if any.
    pub links: Vec<Vec<RowLink>>,
}

impl Rows {
    /// A row the text itself starts.
    pub fn push(&mut self, line: Line<'static>) {
        self.push_join(line, Join::Line);
    }

    /// A row carrying how the wrap continued it from the row above.
    pub fn push_join(&mut self, line: Line<'static>, join: Join) {
        self.push_join_links(line, join, Vec::new());
    }

    /// A row carrying the links it was drawn with.
    pub fn push_join_links(&mut self, line: Line<'static>, join: Join, links: Vec<RowLink>) {
        self.lines.push(line);
        self.joins.push(join);
        self.links.push(links);
    }

    pub fn extend(&mut self, other: Rows) {
        self.lines.extend(other.lines);
        self.joins.extend(other.joins);
        self.links.extend(other.links);
    }

    pub fn append(&mut self, other: &mut Rows) {
        self.lines.append(&mut other.lines);
        self.joins.append(&mut other.joins);
        self.links.append(&mut other.links);
    }

    /// Take the first row off, with the join and links it was drawn with, so a
    /// caller that re-dresses it can put it back.
    pub fn remove_first(&mut self) -> Option<(Line<'static>, Join, Vec<RowLink>)> {
        if self.lines.is_empty() {
            return None;
        }
        let join = self.joins.remove(0);
        let line = self.lines.remove(0);
        let links = self.links.remove(0);
        Some((line, join, links))
    }

    /// Put a row at the top, as a line the text starts, with the links it was
    /// drawn with.
    pub fn insert_first(&mut self, line: Line<'static>, links: Vec<RowLink>) {
        self.lines.insert(0, line);
        self.joins.insert(0, Join::Line);
        self.links.insert(0, links);
    }

    /// Drop the last row, with the join it was drawn with.
    pub fn pop(&mut self) {
        self.lines.pop();
        self.joins.pop();
        self.links.pop();
    }

    /// Drop the first `count` rows.
    pub fn drain_front(&mut self, count: usize) {
        self.lines.drain(..count.min(self.lines.len()));
        self.joins.drain(..count.min(self.joins.len()));
        self.links.drain(..count.min(self.links.len()));
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// The text rendered rows hold, with the pane's own line breaks taken back
/// out. A row that continues the one above it is joined onto it — at the space
/// the wrap broke, or with nothing where it broke inside a word — and without
/// the decoration the pane drew it with, so a copied paragraph reflows in its
/// destination rather than keeping the width it was drawn at. A row the text
/// itself starts stays a line of its own.
pub fn text(rows: impl IntoIterator<Item = (String, Join)>) -> String {
    let rows: Vec<(String, Join)> = rows.into_iter().collect();
    let mut out = String::new();
    for (index, (row, join)) in rows.iter().enumerate() {
        // A row the pane split mid-line is padded by nothing: the whitespace it
        // ends with is the line's own, so only its last chunk drops it.
        let continues_split = matches!(
            rows.get(index + 1).map(|(_, join)| *join),
            Some(Join::Split { .. })
        );
        let row = if continues_split {
            row.as_str()
        } else {
            row.trim_end()
        };
        if index == 0 || *join == Join::Line {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(row);
            continue;
        }
        let (row, separator) = match join {
            // A split row's own leading whitespace is the line's, so only the
            // pane's decoration comes off it; the others are a wrap's
            // continuation, where what leads is the indent the pane drew.
            Join::Split { prefix } => (row.chars().skip(*prefix).collect::<String>(), ""),
            Join::Space { prefix } => (
                row.chars()
                    .skip(*prefix)
                    .collect::<String>()
                    .trim_start()
                    .to_string(),
                " ",
            ),
            Join::Word { prefix } => (
                row.chars()
                    .skip(*prefix)
                    .collect::<String>()
                    .trim_start()
                    .to_string(),
                "",
            ),
            Join::Line => unreachable!("a row that starts a line was handled above"),
        };
        if row.is_empty() {
            continue;
        }
        if !separator.is_empty() && !out.is_empty() && !out.ends_with('\n') {
            out.push(' ');
        }
        out.push_str(&row);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(texts: &[(&str, Join)]) -> Vec<(String, Join)> {
        texts
            .iter()
            .map(|(text, join)| ((*text).to_string(), *join))
            .collect()
    }

    fn space(prefix: usize) -> Join {
        Join::Space { prefix }
    }

    fn word(prefix: usize) -> Join {
        Join::Word { prefix }
    }

    #[test]
    fn a_continuation_is_joined_onto_the_line_it_came_from() {
        let text = text(rows(&[
            ("Security is a thing", Join::Line),
            ("  that spans", space(0)),
            ("two lines", space(0)),
        ]));
        assert_eq!(text, "Security is a thing that spans two lines");
    }

    #[test]
    fn a_line_the_text_starts_is_kept_apart() {
        let text = text(rows(&[
            ("first", Join::Line),
            ("second", Join::Line),
            ("continued", space(0)),
        ]));
        assert_eq!(text, "first\nsecond continued");
    }

    #[test]
    fn a_word_the_wrap_broke_comes_back_together() {
        let text = text(rows(&[
            ("a-long-identif", Join::Line),
            ("  ier-name", word(0)),
        ]));
        assert_eq!(text, "a-long-identifier-name");
    }

    #[test]
    fn a_quote_the_pane_wrapped_is_one_line_with_one_bar() {
        let text = text(rows(&[
            ("│ Security is a thing", Join::Line),
            ("│ that spans a line", space(2)),
        ]));
        assert_eq!(text, "│ Security is a thing that spans a line");
    }

    #[test]
    fn trailing_whitespace_comes_off_every_row() {
        let text = text(rows(&[("alpha  ", Join::Line), ("beta ", Join::Line)]));
        assert_eq!(text, "alpha\nbeta");
    }

    #[test]
    fn a_line_the_pane_split_keeps_its_own_indent() {
        let text = text(rows(&[
            ("  let value = compute(", Join::Line),
            ("    argument);", Join::Split { prefix: 0 }),
        ]));
        assert_eq!(text, "  let value = compute(    argument);");
    }

    #[test]
    fn a_split_row_drops_the_indent_the_pane_drew_it_after() {
        let text = text(rows(&[
            ("│ let value = compute", Join::Line),
            ("│     (argument);", Join::Split { prefix: 2 }),
        ]));
        assert_eq!(text, "│ let value = compute    (argument);");
    }

    #[test]
    fn a_line_the_pane_split_keeps_the_whitespace_inside_it() {
        let text = text(rows(&[
            ("return  ", Join::Line),
            ("answer; ", Join::Split { prefix: 0 }),
        ]));
        assert_eq!(text, "return  answer;");
    }

    #[test]
    fn a_line_the_pane_split_three_ways_keeps_every_space_in_it() {
        let text = text(rows(&[
            ("let  x ", Join::Line),
            ("=  call", Join::Split { prefix: 0 }),
            ("();    ", Join::Split { prefix: 0 }),
        ]));
        assert_eq!(text, "let  x =  call();");
    }

    #[test]
    fn a_blank_row_above_a_continuation_does_not_start_it_with_a_space() {
        let text = text(rows(&[("   ", Join::Line), ("  rest", space(0))]));
        assert_eq!(text, "rest");
    }

    #[test]
    fn no_rows_is_no_text() {
        assert_eq!(text(Vec::new()), "");
    }

    #[test]
    fn a_row_keeps_its_attachment_when_the_pane_dresses_it() {
        assert_eq!(Join::Space { prefix: 0 }.after(2), space(2));
        assert_eq!(Join::Word { prefix: 0 }.after(2), word(2));
        assert_eq!(
            Join::Split { prefix: 0 }.after(2),
            Join::Split { prefix: 2 }
        );
        assert_eq!(Join::Line.after(2), Join::Line);
    }

    fn link(url: &str) -> RowLink {
        RowLink {
            span: 0,
            start: 0,
            end: 4,
            url: url.to_string(),
        }
    }

    #[test]
    fn a_rows_links_ride_along_with_it() {
        let mut rows = Rows::default();
        rows.push_join_links(Line::from("one"), Join::Line, vec![link("https://a")]);
        rows.push(Line::from("two"));
        rows.push(Line::from("three"));
        rows.drain_front(1);
        assert_eq!(rows.links, vec![Vec::new(), Vec::new()]);

        let mut other = Rows::default();
        other.push_join_links(Line::from("four"), Join::Line, vec![link("https://b")]);
        rows.extend(other);
        assert_eq!(rows.links.len(), rows.lines.len());
        assert_eq!(rows.links[2], vec![link("https://b")]);

        let (_, _, taken) = rows.remove_first().expect("a row to take");
        assert!(taken.is_empty());
        rows.insert_first(Line::from("zero"), vec![link("https://c")]);
        assert_eq!(rows.links.len(), rows.lines.len());
        assert_eq!(rows.links[0], vec![link("https://c")]);

        rows.pop();
        assert_eq!(rows.links.len(), rows.lines.len());
    }
}
