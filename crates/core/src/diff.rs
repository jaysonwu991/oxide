//! Line-oriented diff previews for file edits, rendered compactly for the TUI.
//!
//! The output is a unified-style listing with old and new line numbers and a
//! leading ` `/`-`/`+` marker per line. Gaps between changed regions are marked
//! with `⋯` so long files stay readable.

const CONTEXT: usize = 3;
const MAX_DIFF_LINES: usize = 2_000;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Context,
    Add,
    Remove,
}

struct Op {
    kind: Kind,
    old: Option<usize>,
    new: Option<usize>,
    text: String,
}

/// Render a preview of the change from `old` to `new`, or `None` when the two
/// are identical. Oversized inputs fall back to a one-line summary.
pub fn preview(old: &str, new: &str) -> Option<String> {
    if old == new {
        return None;
    }
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();
    if old_lines.len() > MAX_DIFF_LINES || new_lines.len() > MAX_DIFF_LINES {
        return Some(format!(
            "(diff omitted: {} -> {} lines)",
            old_lines.len(),
            new_lines.len()
        ));
    }
    let ops = diff_ops(&old_lines, &new_lines);
    Some(render(&ops, old_lines.len().max(new_lines.len())))
}

fn diff_ops(old: &[&str], new: &[&str]) -> Vec<Op> {
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
            ops.push(Op {
                kind: Kind::Context,
                old: Some(i + 1),
                new: Some(j + 1),
                text: old[i].to_string(),
            });
            i += 1;
            j += 1;
        } else if dp[i + 1][j] >= dp[i][j + 1] {
            ops.push(Op {
                kind: Kind::Remove,
                old: Some(i + 1),
                new: None,
                text: old[i].to_string(),
            });
            i += 1;
        } else {
            ops.push(Op {
                kind: Kind::Add,
                old: None,
                new: Some(j + 1),
                text: new[j].to_string(),
            });
            j += 1;
        }
    }
    while i < n {
        ops.push(Op {
            kind: Kind::Remove,
            old: Some(i + 1),
            new: None,
            text: old[i].to_string(),
        });
        i += 1;
    }
    while j < m {
        ops.push(Op {
            kind: Kind::Add,
            old: None,
            new: Some(j + 1),
            text: new[j].to_string(),
        });
        j += 1;
    }
    ops
}

fn render(ops: &[Op], total: usize) -> String {
    let width = total.to_string().len().max(3);
    let mut keep = vec![false; ops.len()];
    for (index, op) in ops.iter().enumerate() {
        if op.kind == Kind::Context {
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

fn format_op(op: &Op, width: usize) -> String {
    let old = op.old.map(|n| n.to_string()).unwrap_or_default();
    let new = op.new.map(|n| n.to_string()).unwrap_or_default();
    let marker = match op.kind {
        Kind::Context => ' ',
        Kind::Add => '+',
        Kind::Remove => '-',
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
}
