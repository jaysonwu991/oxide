//! The `@path` completion a composer offers while a reference is being typed.
//!
//! The terminal and the desktop app both complete the same token from the same
//! list (`crate::tools::workspace_paths`), so a reference reads the same way in
//! either front-end. The VS Code extension cannot link this crate, so
//! `editors/vscode/src/core/at.ts` mirrors these rules.

use serde::Serialize;

/// How many rows a bare `@` may offer before the list is cut, so a large
/// project does not hand a front-end the whole tree.
pub const MAX_AT_SUGGESTIONS: usize = 200;

/// The `@path` token the cursor is in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AtToken {
    /// Where the `@` is.
    pub start: usize,
    /// Just past the token: the cursor, or the end of a token the cursor is
    /// inside.
    pub end: usize,
    /// What follows the `@`, over the whole token rather than only the part
    /// behind the cursor, so a caret walked back into a half-typed path still
    /// completes the path it is in.
    pub query: String,
}

/// One row a composer offers for a token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AtSuggestion {
    /// The project-relative path being offered; a folder ends in `/`.
    pub label: String,
    pub kind: AtKind,
    /// What takes the token's place: `@path`, with a space after a file so the
    /// next word can be typed and without one after a folder, so the query goes
    /// on narrowing inside it.
    pub insert: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AtKind {
    File,
    Folder,
}

/// The `@path` token the cursor is in, or `None` when it is in none.
///
/// A reference starts at a word boundary and runs to the next whitespace, which
/// is how the CLI's own `@file` expansion reads one too: `mail me at a@b.com`
/// names no file, and a cursor parked inside a half-typed `@src/ma|in.rs`
/// completes the whole token rather than the part behind it.
pub fn token(value: &str, cursor: usize) -> Option<AtToken> {
    let before = value.get(..cursor)?;
    let typed = before.rsplit(char::is_whitespace).next().unwrap_or("");
    let query = typed.strip_prefix('@')?;
    let start = cursor - typed.len();
    let rest = value.get(cursor..)?.split(char::is_whitespace).next()?;
    let end = cursor + rest.len();
    Some(AtToken {
        start,
        end,
        query: format!("{query}{rest}"),
    })
}

/// The rows to offer for a token, best first and cut to `limit`.
pub fn suggestions(paths: &[String], token: Option<&AtToken>, limit: usize) -> Vec<AtSuggestion> {
    let Some(token) = token else {
        return Vec::new();
    };
    if limit == 0 {
        return Vec::new();
    }
    let query = token.query.to_lowercase();
    let mut ranked: Vec<(u8, &str)> = paths
        .iter()
        .filter_map(|path| rank_of(path, &query).map(|rank| (rank, path.as_str())))
        .collect();
    ranked.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(b.1)));
    ranked.truncate(limit);
    ranked.into_iter().map(|(_, path)| row(path)).collect()
}

fn row(path: &str) -> AtSuggestion {
    let folder = path.ends_with('/');
    AtSuggestion {
        label: path.to_string(),
        kind: if folder { AtKind::Folder } else { AtKind::File },
        insert: if folder {
            format!("@{path}")
        } else {
            format!("@{path} ")
        },
    }
}

/// How well a path answers a query: the nearest name first (`agent.rs` for
/// `age`), then a path that starts with it, then one that mentions it anywhere.
/// A query already carrying a `/` is a path being walked, so the whole prefix is
/// what counts. `None` is no match.
fn rank_of(path: &str, query: &str) -> Option<u8> {
    let whole = path.to_lowercase();
    // A folder the reference already spells exactly is what taking its row would
    // give back — the query itself. Leaving it out is what makes the row take the
    // reference *into* the folder and offer what is inside it, rather than
    // running a completion that changes nothing.
    if whole.ends_with('/') && whole == query {
        return None;
    }
    let trimmed = whole.strip_suffix('/').unwrap_or(&whole);
    let name = trimmed.rsplit('/').next().unwrap_or(trimmed);
    if query.contains('/') {
        return if whole.starts_with(query) {
            Some(0)
        } else if whole.contains(query) {
            Some(1)
        } else {
            None
        };
    }
    if name.starts_with(query) {
        Some(0)
    } else if whole.starts_with(query) {
        Some(1)
    } else if whole.contains(query) {
        Some(2)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths() -> Vec<String> {
        [
            "crates/core/src/",
            "crates/core/src/agent.rs",
            "crates/core/src/tools.rs",
            "crates/desktop/ui/app.js",
            "docs/",
            "docs/vscode.md",
            "editors/vscode/src/chat.ts",
            "src/main.rs",
        ]
        .iter()
        .map(|path| path.to_string())
        .collect()
    }

    fn labels(value: &str, cursor: usize) -> Vec<String> {
        suggestions(&paths(), token(value, cursor).as_ref(), MAX_AT_SUGGESTIONS)
            .into_iter()
            .map(|row| row.label)
            .collect()
    }

    #[test]
    fn reads_the_reference_the_cursor_is_in() {
        assert_eq!(
            token("@", 1),
            Some(AtToken {
                start: 0,
                end: 1,
                query: String::new()
            })
        );
        assert_eq!(
            token("look at @src/ma", 15).map(|token| (token.start, token.end, token.query)),
            Some((8, 15, "src/ma".to_string()))
        );
        // The cursor need not be at the end of the token it is in.
        assert_eq!(
            token("look at @src/ma", 14).map(|token| (token.start, token.end, token.query)),
            Some((8, 15, "src/ma".to_string()))
        );
        // The whole token, not the half of it behind the cursor.
        assert_eq!(
            token("see @src/main.rs and", 6).map(|token| (token.start, token.end, token.query)),
            Some((4, 16, "src/main.rs".to_string()))
        );
        // An address is not a path, and a cursor outside a reference has none.
        assert_eq!(token("mail me at a@b.com", 17), None);
        assert_eq!(token("@src ", 5), None);
        assert_eq!(token("@src", 0), None);
        assert_eq!(token("no reference here", 17), None);
    }

    #[test]
    fn leaves_a_file_ready_for_the_next_word_and_a_folder_open() {
        let rows = suggestions(&paths(), token("@docs/vscode.md", 15).as_ref(), 200);
        assert_eq!(
            rows[0],
            AtSuggestion {
                label: "docs/vscode.md".to_string(),
                kind: AtKind::File,
                insert: "@docs/vscode.md ".to_string(),
            }
        );
        let rows = suggestions(&paths(), token("@docs", 5).as_ref(), 200);
        assert_eq!(
            rows[0],
            AtSuggestion {
                label: "docs/".to_string(),
                kind: AtKind::Folder,
                insert: "@docs/".to_string(),
            }
        );
        // The folder the reference already spells is left out, so taking a row
        // walks into it instead of completing it to what is already typed.
        let rows = suggestions(&paths(), token("@docs/", 6).as_ref(), 200);
        assert_eq!(labels("@docs/", 6), ["docs/vscode.md"]);
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn offers_a_folder_beside_the_files_under_it() {
        assert_eq!(
            labels("@crates/core/", 13),
            [
                "crates/core/src/",
                "crates/core/src/agent.rs",
                "crates/core/src/tools.rs"
            ]
        );
        // A folder the reference already spells is left out, so taking a row
        // walks into it instead of completing it to what is already typed.
        assert_eq!(
            labels("@crates/core/src/", 17),
            ["crates/core/src/agent.rs", "crates/core/src/tools.rs"]
        );
        // The same query without the slash is a name being typed, so the folder
        // is still offered and Enter enters it.
        assert_eq!(labels("@src", 4)[0], "crates/core/src/");
    }

    #[test]
    fn puts_the_nearest_name_first_then_the_path_then_a_mention() {
        assert_eq!(
            labels("@src", 4),
            [
                "crates/core/src/",
                "src/main.rs",
                "crates/core/src/agent.rs",
                "crates/core/src/tools.rs",
                "editors/vscode/src/chat.ts"
            ]
        );
        // A query with a `/` in it is a path being walked, so the whole prefix
        // is what counts rather than the name at the end of it.
        assert_eq!(
            labels("@crates/core/src/ag", 18),
            ["crates/core/src/agent.rs"]
        );
    }

    #[test]
    fn matches_case_insensitively() {
        assert_eq!(labels("@AGENT", 6), ["crates/core/src/agent.rs"]);
        assert_eq!(labels("@Docs/VsCode", 12), ["docs/vscode.md"]);
    }

    #[test]
    fn offers_the_whole_sorted_list_for_a_bare_at_and_caps_it() {
        let all = labels("@", 1);
        assert_eq!(all.len(), paths().len());
        let mut sorted = paths();
        sorted.sort();
        assert_eq!(all, sorted);
        assert_eq!(suggestions(&paths(), token("@", 1).as_ref(), 3).len(), 3);
        assert!(suggestions(&paths(), token("@", 1).as_ref(), 0).is_empty());
    }

    #[test]
    fn offers_nothing_for_a_path_that_is_not_there() {
        assert!(labels("@nowhere/at/all.txt", 19).is_empty());
        assert!(suggestions(&paths(), None, 200).is_empty());
        // Stray punctuation is part of the token, so a reference followed by a
        // comma simply matches nothing.
        assert!(labels("@docs/vscode.md,", 16).is_empty());
    }
}
