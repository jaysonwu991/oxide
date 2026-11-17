//! Line-oriented diffs for file edits: the same alignment rendered compactly
//! for the TUI (a unified-style listing with old and new line numbers, a leading
//! ` `/`-`/`+` marker per line, and `⋯` for the gaps between changed regions)
//! and as the lines themselves, which a front-end that paints both sides of a
//! change reads.

const CONTEXT: usize = 3;
const MAX_DIFF_LINES: usize = 2_000;
const MAX_COUNT_WORK: usize = 50_000_000;

/// What one line of a change is: text both sides have, or text only one does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LineKind {
    Context,
    Add,
    Remove,
}

/// One aligned line of a change. The side it is missing from carries no number,
/// which is the empty cell a split view paints for it.
#[derive(Debug)]
pub struct Line {
    pub kind: LineKind,
    pub old: Option<usize>,
    pub new: Option<usize>,
    pub text: String,
}

/// The aligned lines of `old` → `new`, or why there are none to paint: two
/// identical sides, or one too large to align line by line.
pub enum Diff {
    Same,
    Omitted { old: usize, new: usize },
    Lines(Vec<Line>),
}

/// Aligns two texts line by line. A caller that paints a diff of its own — the
/// desktop's review, which shows both sides of a file rather than the hunks a
/// preview keeps — reads this; [`preview`] renders the same alignment compactly
/// for the terminal.
pub fn lines(old: &str, new: &str) -> Diff {
    if old == new {
        return Diff::Same;
    }
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    if old_lines.len() > MAX_DIFF_LINES || new_lines.len() > MAX_DIFF_LINES {
        return Diff::Omitted {
            old: old_lines.len(),
            new: new_lines.len(),
        };
    }
    Diff::Lines(align(&old_lines, &new_lines))
}

/// Render a preview of the change from `old` to `new`, or `None` when the two
/// are identical. Oversized inputs fall back to a one-line summary.
pub fn preview(old: &str, new: &str) -> Option<String> {
    match lines(old, new) {
        Diff::Same => None,
        Diff::Omitted { old, new } => Some(format!("(diff omitted: {old} -> {new} lines)")),
        Diff::Lines(lines) => {
            // A numbered line reaches the length of its side, so the widest
            // number is the longer side's line count either way.
            let total = lines
                .iter()
                .filter_map(|line| line.old.max(line.new))
                .max()
                .unwrap_or(0);
            Some(render(&lines, total))
        }
    }
}

fn align(old: &[&str], new: &[&str]) -> Vec<Line> {
    let n = old.len();
    let m = new.len();
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if old[i] == new[j] {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }

    let mut ops = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n && j < m {
        if old[i] == new[j] {
            ops.push(Line {
                kind: LineKind::Context,
                old: Some(i + 1),
                new: Some(j + 1),
                text: old[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(Line {
                kind: LineKind::Remove,
                old: Some(i + 1),
                new: None,
                text: old[i].to_string(),
            });
            i += 1;
        } else {
            ops.push(Line {
                kind: LineKind::Add,
                old: None,
                new: Some(j + 1),
                text: new[j].to_string(),
            });
            j += 1;
        }
    }
    while i < n {
        ops.push(Line {
            kind: LineKind::Remove,
            old: Some(i + 1),
            new: None,
            text: old[i].to_string(),
        });
        i += 1;
    }
    while j < m {
        ops.push(Line {
            kind: LineKind::Add,
            old: None,
            new: Some(j + 1),
            text: new[j].to_string(),
        });
        j += 1;
    }
    ops
}

/// The added and removed line counts of the change from `old` to `new`, with no
/// size limit: a file too large to preview still counts its lines, the way
/// `git diff --numstat` does. The count is the minimal edit script (Myers), which
/// is what the preview's alignment reports for a file small enough to align. A
/// rewrite too large to search within the work budget falls back to matching
/// lines regardless of order, which never counts more lines than changed.
pub fn counts(old: &str, new: &str) -> (usize, usize) {
    let old: Vec<&str> = old.lines().collect();
    let new: Vec<&str> = new.lines().collect();
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let suffix = old[prefix..]
        .iter()
        .rev()
        .zip(new[prefix..].iter().rev())
        .take_while(|(a, b)| a == b)
        .count();
    let old = &old[prefix..old.len() - suffix];
    let new = &new[prefix..new.len() - suffix];
    let common = common_lines(old, new);
    (new.len() - common, old.len() - common)
}

fn common_lines(old: &[&str], new: &[&str]) -> usize {
    let (n, m) = (old.len(), new.len());
    if n == 0 || m == 0 {
        return 0;
    }
    let max = n + m;
    let offset = max as isize + 1;
    let mut furthest = vec![0usize; 2 * max + 3];
    for d in 0..=max {
        if d.saturating_mul(max) > MAX_COUNT_WORK {
            return unordered_common_lines(old, new);
        }
        let d = d as isize;
        for k in (-d..=d).step_by(2) {
            let index = (k + offset) as usize;
            let mut x = if k == -d || (k != d && furthest[index - 1] < furthest[index + 1]) {
                furthest[index + 1]
            } else {
                furthest[index - 1] + 1
            };
            let mut y = (x as isize - k) as usize;
            while x < n && y < m && old[x] == new[y] {
                x += 1;
                y += 1;
            }
            furthest[index] = x;
            if x >= n && y >= m {
                return (n + m - d as usize) / 2;
            }
        }
    }
    0
}

fn unordered_common_lines(old: &[&str], new: &[&str]) -> usize {
    let mut remaining = std::collections::HashMap::new();
    for line in old {
        *remaining.entry(*line).or_insert(0usize) += 1;
    }
    new.iter()
        .filter(|line| match remaining.get_mut(*line) {
            Some(left) if *left > 0 => {
                *left -= 1;
                true
            }
            _ => false,
        })
        .count()
}

fn render(ops: &[Line], total: usize) -> String {
    let width = total.to_string().len().max(3);
    let mut keep = vec![false; ops.len()];
    for (index, op) in ops.iter().enumerate() {
        if op.kind == LineKind::Context {
            continue;
        }
        let start = index.saturating_sub(CONTEXT);
        let end = (index + CONTEXT + 1).min(ops.len());
        for slot in keep.iter_mut().take(end).skip(start) {
            *slot = true;
        }
    }

    let mut out: Vec<String> = Vec::new();
    let mut previous = false;
    for (index, op) in ops.iter().enumerate() {
        if !keep[index] {
            previous = false;
            continue;
        }
        if !previous && !out.is_empty() {
            out.push(format!(" {:>width$} {:>width$}  ⋯", "", ""));
        }
        out.push(format_op(op, width));
        previous = true;
    }
    out.join("\n")
}

fn format_op(op: &Line, width: usize) -> String {
    let old = op.old.map(|n| n.to_string()).unwrap_or_default();
    let new = op.new.map(|n| n.to_string()).unwrap_or_default();
    let marker = match op.kind {
        LineKind::Context => ' ',
        LineKind::Add => '+',
        LineKind::Remove => '-',
    };
    format!("{marker}{old:>width$} {new:>width$}  {}", op.text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identical_input_has_no_preview() {
        assert!(preview("a\nb\n", "a\nb\n").is_none());
    }

    #[test]
    fn add_and_remove_are_marked() {
        let diff = preview("a\nb\nc\n", "a\nB\nc\n").unwrap();
        assert!(diff.contains("-  2      b"), "{diff}");
        assert!(diff.contains("+      2  B"), "{diff}");
    }

    #[test]
    fn distant_changes_are_split_by_a_gap() {
        let old: String = (0..40).map(|n| format!("line {n}\n")).collect();
        let mut new: String = old.clone();
        new = new.replace("line 0\n", "line zero\n");
        new = new.replace("line 39\n", "line thirty-nine\n");
        let diff = preview(&old, &new).unwrap();
        assert!(diff.contains("⋯"), "{diff}");
        assert!(diff.contains("line zero"), "{diff}");
        assert!(diff.contains("line thirty-nine"), "{diff}");
    }

    #[test]
    fn empty_old_shows_all_additions() {
        let diff = preview("", "one\ntwo\n").unwrap();
        assert!(diff.contains("+      1  one"), "{diff}");
        assert!(diff.contains("+      2  two"), "{diff}");
    }

    #[test]
    fn aligned_lines_carry_the_number_of_each_side() {
        let Diff::Lines(lines) = lines("a\nb\n", "a\nc\n") else {
            panic!("expected an alignment");
        };
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].kind, LineKind::Context);
        assert_eq!((lines[0].old, lines[0].new), (Some(1), Some(1)));
        assert_eq!(lines[0].text, "a");
        // The removed side has no number on the new side, and the other way
        // round, which is the empty cell a split view paints.
        assert_eq!(lines[1].kind, LineKind::Remove);
        assert_eq!((lines[1].old, lines[1].new), (Some(2), None));
        assert_eq!(lines[2].kind, LineKind::Add);
        assert_eq!((lines[2].old, lines[2].new), (None, Some(2)));
        assert_eq!(lines[2].text, "c");
    }

    #[test]
    fn identical_sides_have_nothing_to_paint() {
        assert!(matches!(lines("a\nb\n", "a\nb\n"), Diff::Same));
    }

    #[test]
    fn an_oversized_side_is_reported_rather_than_aligned() {
        let old: String = (0..MAX_DIFF_LINES + 1)
            .map(|n| format!("line {n}\n"))
            .collect();
        assert!(matches!(
            lines(&old, "one line\n"),
            Diff::Omitted { old: 2_001, new: 1 }
        ));
    }

    #[test]
    fn the_preview_keeps_the_width_of_the_longer_side() {
        // Three lines on one side and one on the other still pad to three
        // digits, which is what the terminal's columns are aligned by.
        let diff = preview("a\nb\nc\n", "a\n").unwrap();
        assert!(diff.contains("-  2      b"), "{diff}");
    }

    #[test]
    fn counts_agree_with_the_aligned_preview() {
        let cases = [
            ("", ""),
            ("", "one\ntwo\n"),
            ("one\ntwo\n", ""),
            ("a\nb\nc\n", "a\nB\nc\n"),
            ("a\nb\nc\nd\n", "b\na\nd\nc\ne\n"),
            ("x\ny\nx\ny\n", "y\nx\ny\nx\n"),
        ];
        for (old, new) in cases {
            let aligned = match lines(old, new) {
                Diff::Lines(lines) => (
                    lines.iter().filter(|l| l.kind == LineKind::Add).count(),
                    lines.iter().filter(|l| l.kind == LineKind::Remove).count(),
                ),
                _ => (0, 0),
            };
            assert_eq!(counts(old, new), aligned, "{old:?} -> {new:?}");
        }
    }

    #[test]
    fn a_file_too_large_to_preview_still_counts_its_lines() {
        let old: String = (0..8_000).map(|n| format!("line {n}\n")).collect();
        let new = old
            .replace("line 10\n", "line ten\n")
            .replace("line 4000\n", "")
            .replace("line 7000\n", "line 7000\nextra a\nextra b\n");
        assert!(matches!(lines(&old, &new), Diff::Omitted { .. }));
        assert_eq!(counts(&old, &new), (3, 2));
    }

    #[test]
    fn a_rewrite_past_the_work_budget_falls_back_to_unordered_matching() {
        let old: String = (0..40_000).map(|n| format!("old {n}\n")).collect();
        let new: String = (0..40_000).map(|n| format!("new {n}\n")).collect();
        assert_eq!(counts(&old, &new), (40_000, 40_000));
    }
}
