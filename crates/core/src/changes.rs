//! The files a run changed, as a front-end lists them.
//!
//! A front-end that shows a turn's work needs more than the per-tool previews
//! `crate::tools` returns: a file a shell command rewrote, a formatter touched
//! or an MCP server generated was never named by a tool call. The source is the
//! project's shadow snapshot (see `crate::snapshots`), so the listing is a diff
//! of the whole work tree against the state the run started from, and each
//! entry carries the same compact preview `crate::diff` renders for a single
//! edit plus the line counts a summary is built from.
//!
//! The git-facing half lives in `crate::snapshots::Snapshots`; everything here
//! is pure, so the parsing and the counting are covered by `cargo test`.

use crate::diff;
use serde::{Deserialize, Serialize};

/// How a path changed between the baseline and the work tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChangeStatus {
    Added,
    Modified,
    Deleted,
}

impl ChangeStatus {
    /// The letter `git status` prints for it, which is what a front-end badges
    /// the row with.
    pub fn letter(&self) -> char {
        match self {
            ChangeStatus::Added => 'A',
            ChangeStatus::Modified => 'M',
            ChangeStatus::Deleted => 'D',
        }
    }
}

/// One changed file: what happened to it, how many lines moved, and the preview
/// to paint for it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileChange {
    /// The path as git reports it, relative to the project root.
    pub path: String,
    pub status: ChangeStatus,
    pub added: usize,
    pub removed: usize,
    /// A file whose content is not text. It has no lines to count, so a
    /// front-end says so rather than painting a whole-file rewrite.
    pub binary: bool,
    /// The compact line-numbered preview `crate::diff` renders, in the same
    /// format the tools' own previews use. Empty for a binary file, for one
    /// too large to diff, and for a mode-only change.
    pub diff: String,
}

/// Every file a run changed, with the totals a card's header shows.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TurnChanges {
    pub files: Vec<FileChange>,
    pub added: usize,
    pub removed: usize,
}

impl TurnChanges {
    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// The file and the counts it carries, given the two sides of the change
/// (`None` for a side that does not exist: a file the run added or removed).
/// A side that is not valid UTF-8 makes the change binary.
pub fn file_change(
    status: ChangeStatus,
    path: impl Into<String>,
    old: Option<Vec<u8>>,
    new: Option<Vec<u8>>,
) -> FileChange {
    let path = path.into();
    let binary = FileChange {
        path: path.clone(),
        status,
        added: 0,
        removed: 0,
        binary: true,
        diff: String::new(),
    };
    let (Some(old), Some(new)) = (utf8(old), utf8(new)) else {
        return binary;
    };
    let diff = diff::preview(&old, &new).unwrap_or_default();
    let (added, removed) = count(&diff);
    FileChange {
        path,
        status,
        added,
        removed,
        binary: false,
        diff,
    }
}

/// `None` (a file that is not there) reads as empty text, so an added file
/// diffs as all additions and a deleted one as all removals.
fn utf8(bytes: Option<Vec<u8>>) -> Option<String> {
    match bytes {
        None => Some(String::new()),
        Some(bytes) => String::from_utf8(bytes).ok(),
    }
}

/// The added and removed line counts of a rendered preview. Its first character
/// is the `+`/`-`/` ` marker, so a line's own text can never be mistaken for
/// one, and the `⋯` a gap is marked with reads as neither.
pub fn count(diff: &str) -> (usize, usize) {
    let mut added = 0;
    let mut removed = 0;
    for line in diff.lines() {
        match line.as_bytes().first() {
            Some(b'+') => added += 1,
            Some(b'-') => removed += 1,
            _ => {}
        }
    }
    (added, removed)
}

/// Totals a listing's header shows, from its own entries.
pub fn summarize(files: Vec<FileChange>) -> TurnChanges {
    let added = files.iter().map(|file| file.added).sum();
    let removed = files.iter().map(|file| file.removed).sum();
    TurnChanges {
        files,
        added,
        removed,
    }
}

/// The entries of `git diff --name-status -z` / `--raw -z`, in the order git
/// listed them. The output is NUL-separated with one status per path; a rename
/// (which `--no-renames` suppresses, so it only arrives when a caller asks for
/// them) carries a second path, and the new name is the file to show.
pub fn parse_name_status(output: &str) -> Vec<(ChangeStatus, String)> {
    let mut entries = Vec::new();
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    while let Some(status) = fields.next() {
        let letter = status.chars().next().unwrap_or('M');
        if matches!(letter, 'R' | 'C') {
            fields.next();
            if let Some(path) = fields.next() {
                entries.push((ChangeStatus::Modified, path.to_string()));
            }
            continue;
        }
        let Some(path) = fields.next() else {
            break;
        };
        let status = match letter {
            'A' => ChangeStatus::Added,
            'D' => ChangeStatus::Deleted,
            _ => ChangeStatus::Modified,
        };
        entries.push((status, path.to_string()));
    }
    entries
}

/// One path's line counts as `git diff --numstat` reports them, in the two
/// columns git prints. A binary file has no lines to count, which git says with
/// a `-` in each column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Numstat {
    pub added: usize,
    pub removed: usize,
    pub binary: bool,
}

/// The entries of `git diff --numstat -z`, keyed by path in the order git
/// listed them. `git` counts the lines that moved, which the preview cannot
/// always do: a change too large to render is a one-line summary with no `+`/`-`
/// lines in it (see [`diff::preview`]), so counting the preview would report a
/// real change as none. A rename carries both names in the fields after the
/// counts (only when a caller asks for renames), and the new name is the file
/// to show, as in [`parse_name_status`].
pub fn parse_numstat(output: &str) -> Vec<(String, Numstat)> {
    let mut entries = Vec::new();
    let mut fields = output.split('\0').filter(|field| !field.is_empty());
    while let Some(record) = fields.next() {
        let mut columns = record.splitn(3, '\t');
        let added = columns.next().unwrap_or("");
        let removed = columns.next().unwrap_or("");
        let path = columns.next().unwrap_or("");
        let path = if path.is_empty() {
            fields.next();
            match fields.next() {
                Some(path) => path.to_string(),
                None => break,
            }
        } else {
            path.to_string()
        };
        entries.push((
            path,
            Numstat {
                added: added.parse().unwrap_or(0),
                removed: removed.parse().unwrap_or(0),
                binary: added == "-" || removed == "-",
            },
        ));
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_status_is_read_per_path() {
        let entries = parse_name_status("M\0src/main.rs\0A\0new.txt\0D\0gone.txt\0");
        assert_eq!(
            entries,
            vec![
                (ChangeStatus::Modified, "src/main.rs".to_string()),
                (ChangeStatus::Added, "new.txt".to_string()),
                (ChangeStatus::Deleted, "gone.txt".to_string()),
            ]
        );
        // A rename reports both names; the new one is the file to show, and the
        // trailing NUL leaves nothing behind.
        assert_eq!(
            parse_name_status("R100\0old.rs\0new.rs\0"),
            vec![(ChangeStatus::Modified, "new.rs".to_string())]
        );
        assert!(parse_name_status("").is_empty());
        assert_eq!(ChangeStatus::Added.letter(), 'A');
    }

    #[test]
    fn numstat_is_read_per_path() {
        assert_eq!(
            parse_numstat("8\t2\tsrc/agent.rs\0-\t-\tlogo.png\0"),
            vec![
                (
                    "src/agent.rs".to_string(),
                    Numstat {
                        added: 8,
                        removed: 2,
                        binary: false,
                    }
                ),
                (
                    "logo.png".to_string(),
                    Numstat {
                        added: 0,
                        removed: 0,
                        binary: true,
                    }
                ),
            ]
        );
        // A rename's two names follow an empty path field.
        assert_eq!(
            parse_numstat("0\t0\t\0old.rs\0new.rs\0"),
            vec![(
                "new.rs".to_string(),
                Numstat {
                    added: 0,
                    removed: 0,
                    binary: false,
                }
            )]
        );
        assert!(parse_numstat("").is_empty());
    }

    #[test]
    fn an_added_file_counts_as_additions() {
        let change = file_change(
            ChangeStatus::Added,
            "new.txt",
            None,
            Some(b"one\ntwo\n".to_vec()),
        );
        assert_eq!(change.added, 2);
        assert_eq!(change.removed, 0);
        assert!(!change.binary);
        assert!(change.diff.contains("+      1  one"), "{}", change.diff);
    }

    #[test]
    fn a_removed_file_counts_as_removals() {
        let change = file_change(
            ChangeStatus::Deleted,
            "gone.txt",
            Some(b"one\ntwo\n".to_vec()),
            None,
        );
        assert_eq!(change.added, 0);
        assert_eq!(change.removed, 2);
    }

    #[test]
    fn a_binary_side_is_reported_rather_than_diffed() {
        let change = file_change(
            ChangeStatus::Modified,
            "logo.png",
            Some(vec![0x89, 0x50, 0x4e, 0x47, 0x00, 0xff]),
            Some(vec![0x89, 0x50, 0x4e, 0x47, 0x01, 0xfe]),
        );
        assert!(change.binary);
        assert!(change.diff.is_empty());
        assert_eq!((change.added, change.removed), (0, 0));
    }

    #[test]
    fn an_unchanged_side_leaves_no_diff() {
        // A mode-only change: `git` lists the path, the content is identical,
        // so there is nothing to paint and nothing to count.
        let change = file_change(
            ChangeStatus::Modified,
            "run.sh",
            Some(b"same\n".to_vec()),
            Some(b"same\n".to_vec()),
        );
        assert!(!change.binary);
        assert!(change.diff.is_empty());
        assert_eq!((change.added, change.removed), (0, 0));
    }

    #[test]
    fn totals_add_up_across_files() {
        let files = vec![
            file_change(ChangeStatus::Added, "a", None, Some(b"x\ny\n".to_vec())),
            file_change(
                ChangeStatus::Modified,
                "b",
                Some(b"x\n".to_vec()),
                Some(b"z\n".to_vec()),
            ),
        ];
        let changes = summarize(files);
        assert_eq!(changes.files.len(), 2);
        assert_eq!(changes.added, 3);
        assert_eq!(changes.removed, 1);
        assert!(!changes.is_empty());
        assert!(summarize(Vec::new()).is_empty());
    }
}
