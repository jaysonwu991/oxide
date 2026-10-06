//! The transcript's rendered rows and how each one attaches to the row above
//! it. A row drawn because the pane is too narrow is the pane's line break, not
//! the text's, so a copy joins those rows back into the sentence they came from
//! instead of keeping the width the pane happened to be drawn at.

use ratatui::text::Line;

/// How a rendered row attaches to the row above it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Join {
    /// The row starts a line the text itself has.
    #[default]
    Line,
    /// The row continues the one above, which the wrap broke at a space: the
    /// join puts that space back.
    Space,
    /// The row continues the one above inside a word, so the join puts the
    /// word back together with nothing between the pieces.
    Word,
    /// The row is the rest of a line the pane split, drawn after `prefix` cells
    /// of the pane's own indent. The line's own whitespace is part of the text,
    /// so the join runs the pieces back together as they are and drops only the
    /// indent the pane drew.
    Split { prefix: usize },
}

/// Rendered rows and, for each one, how it attaches to the row above it.
#[derive(Default)]
pub struct Rows {
    pub lines: Vec<Line<'static>>,
    pub joins: Vec<Join>,
}

impl Rows {
    /// A row the text itself starts.
    pub fn push(&mut self, line: Line<'static>) {
        self.push_join(line, Join::Line);
    }

    /// A row carrying how the wrap continued it from the row above.
    pub fn push_join(&mut self, line: Line<'static>, join: Join) {
        self.lines.push(line);
        self.joins.push(join);
    }

    pub fn extend(&mut self, other: Rows) {
        self.lines.extend(other.lines);
        self.joins.extend(other.joins);
    }

    pub fn append(&mut self, other: &mut Rows) {
        self.lines.append(&mut other.lines);
        self.joins.append(&mut other.joins);
    }

    /// Take the first row off, with the join it was drawn with, so a caller
    /// that re-dresses it can put it back.
    pub fn remove_first(&mut self) -> Option<(Line<'static>, Join)> {
        if self.lines.is_empty() {
            return None;
        }
        let join = self.joins.remove(0);
        let line = self.lines.remove(0);
        Some((line, join))
    }

    /// Put a row at the top, as a line the text starts.
    pub fn insert_first(&mut self, line: Line<'static>) {
        self.lines.insert(0, line);
        self.joins.insert(0, Join::Line);
    }

    /// Drop the last row, with the join it was drawn with.
    pub fn pop(&mut self) {
        self.lines.pop();
        self.joins.pop();
    }

    /// Drop the first `count` rows.
    pub fn drain_front(&mut self, count: usize) {
        self.lines.drain(..count.min(self.lines.len()));
        self.joins.drain(..count.min(self.joins.len()));
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }
}

/// The text rendered rows hold, with the pane's own line breaks taken back
/// out. A row that continues the one above it is joined onto it — at the space
/// the wrap broke, or with nothing where it broke inside a word — and without
/// the indent the wrap drew it with, so a copied paragraph reflows in its
/// destination rather than keeping the width it was drawn at. A row the text
/// itself starts stays a line of its own.
pub fn text(rows: impl IntoIterator<Item = (String, Join)>) -> String {
    let mut out = String::new();
    for (index, (row, join)) in rows.into_iter().enumerate() {
        let row = row.trim_end();
        if index == 0 || join == Join::Line {
            if index > 0 {
                out.push('\n');
            }
            out.push_str(row);
            continue;
        }
        let (row, separator) = match join {
            Join::Split { prefix } => (row.chars().skip(prefix).collect::<String>(), ""),
            Join::Space => (row.trim_start().to_string(), " "),
            Join::Word => (row.trim_start().to_string(), ""),
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

    #[test]
    fn a_continuation_is_joined_onto_the_line_it_came_from() {
        let text = text(rows(&[
            ("Security is a thing", Join::Line),
            ("  that spans", Join::Space),
            ("two lines", Join::Space),
        ]));
        assert_eq!(text, "Security is a thing that spans two lines");
    }

    #[test]
    fn a_line_the_text_starts_is_kept_apart() {
        let text = text(rows(&[
            ("first", Join::Line),
            ("second", Join::Line),
            ("continued", Join::Space),
        ]));
        assert_eq!(text, "first\nsecond continued");
    }

    #[test]
    fn a_word_the_wrap_broke_comes_back_together() {
        let text = text(rows(&[
            ("a-long-identif", Join::Line),
            ("  ier-name", Join::Word),
        ]));
        assert_eq!(text, "a-long-identifier-name");
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
    fn a_blank_row_above_a_continuation_does_not_start_it_with_a_space() {
        let text = text(rows(&[("   ", Join::Line), ("  rest", Join::Space)]));
        assert_eq!(text, "rest");
    }

    #[test]
    fn no_rows_is_no_text() {
        assert_eq!(text(Vec::new()), "");
    }
}
