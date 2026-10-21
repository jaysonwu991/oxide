//! The `@path` completion a composer offers while a reference is being typed.
//!
//! The terminal and the desktop app both complete the same token from the same
//! list (`crate::tools::workspace_paths`), so a reference reads the same way in
//! either front-end. The VS Code extension cannot link this crate, so
//! `editors/vscode/src/core/at.ts` mirrors these rules.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// Syntax ranges for composer decoration, without filesystem probes per frame.
/// Quoted paths and bare filenames with spaces stay together. Resolution when
/// sending still checks which files actually exist.
pub fn mention_ranges(input: &str) -> Vec<std::ops::Range<usize>> {
    let mut ranges: Vec<std::ops::Range<usize>> = Vec::new();
    for (start, _) in input.match_indices('@') {
        if (start > 0 && !input[..start].ends_with(char::is_whitespace))
            || ranges.last().is_some_and(|range| start < range.end)
        {
            continue;
        }
        let line = input[start + 1..].split('\n').next().unwrap_or_default();
        let limit = line.char_indices().nth(4096).map_or(line.len(), |(i, _)| i);
        let line = &line[..limit];
        let Some(first) = line.chars().next().filter(|ch| !ch.is_whitespace()) else {
            continue;
        };
        let length = if matches!(first, '\'' | '"') {
            let end = line[1..].find(first).map_or(line.len(), |index| index + 2);
            let suffix = line[end..].strip_prefix('#').map_or(0, |rest| {
                1 + rest
                    .chars()
                    .take_while(|ch| ch.is_ascii_digit() || *ch == '-')
                    .count()
            });
            end + suffix
        } else {
            let first_word = line.split_whitespace().next().unwrap_or_default();
            let mut length = first_word
                .trim_end_matches([',', '.', ';', '!', '?', ')', ']'])
                .len();
            let mut offset = 0;
            for word in line.split_whitespace().take(128) {
                let at = offset + line[offset..].find(word).unwrap_or_default();
                offset = at + word.len();
                if at > 0 && word.starts_with('@') {
                    break;
                }
                let candidate = word.trim_end_matches([',', '.', ';', '!', '?', ')', ']']);
                let path = candidate.split('#').next().unwrap_or_default();
                if path.rsplit_once('.').is_some_and(|(_, extension)| {
                    !extension.is_empty()
                        && extension.len() <= 12
                        && extension.chars().all(|ch| ch.is_ascii_alphanumeric())
                }) {
                    length = at + candidate.len();
                    break;
                }
                if at == 0 && word.ends_with('/') {
                    break;
                }
            }
            length
        };
        if length > 0 {
            ranges.push(start..start + 1 + length);
        }
    }
    ranges
}

/// Resolve references by probing paths, never by walking outside the project.
/// Bare names with spaces use the longest existing file prefix on the line.
pub fn referenced_files(input: &str, cwd: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for (index, _) in input.match_indices('@') {
        if index > 0 && !input[..index].ends_with(char::is_whitespace) {
            continue;
        }
        let rest = &input[index + 1..];
        let line = rest.lines().next().unwrap_or_default();
        // Bound filesystem probes even when a pasted document contains an @.
        let end = line.char_indices().nth(4096).map_or(line.len(), |(i, _)| i);
        let line = &line[..end];
        let candidates: Vec<&str> =
            if let Some(quote) = line.chars().next().filter(|c| matches!(c, '\'' | '"')) {
                line[1..]
                    .find(quote)
                    .map(|end| vec![&line[1..end + 1]])
                    .unwrap_or_default()
            } else {
                let mut ends: Vec<usize> = line
                    .char_indices()
                    .filter_map(|(i, c)| c.is_whitespace().then_some(i))
                    .take(128)
                    .collect();
                ends.push(line.len());
                ends.into_iter()
                    .rev()
                    .map(|end| {
                        line[..end].trim_end_matches([',', '.', ';', '!', '?', ')', ']', '"', '\''])
                    })
                    .collect()
            };
        for raw in candidates {
            if raw.is_empty() {
                continue;
            }
            let raw = raw.rsplit_once('#').map_or(raw, |(path, range)| {
                let mut numbers = range.split('-');
                let start = numbers.next().and_then(|n| n.parse::<usize>().ok());
                let end = numbers.next().map(|n| n.parse::<usize>());
                if start.is_some_and(|n| n > 0)
                    && end.is_none_or(|end| end.is_ok_and(|n| n >= start.unwrap()))
                    && numbers.next().is_none()
                {
                    path
                } else {
                    raw
                }
            });
            let path = crate::media::expand_path(raw, cwd);
            if path.is_file() {
                if !found.contains(&path) {
                    found.push(path);
                }
                break;
            }
        }
    }
    found
}

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
/// A reference starts at a word boundary and runs to the next whitespace (or
/// the closing quote for a quoted path), which
/// is how the CLI's own `@file` expansion reads one too: `mail me at a@b.com`
/// names no file, and a cursor parked inside a half-typed `@src/ma|in.rs`
/// completes the whole token rather than the part behind it.
pub fn token(value: &str, cursor: usize) -> Option<AtToken> {
    value.get(..cursor)?;
    for (start, _) in value.match_indices('@').rev() {
        if start >= cursor || (start > 0 && !value[..start].ends_with(char::is_whitespace)) {
            continue;
        }
        let rest = &value[start + 1..];
        let (query, end) =
            if let Some(quote) = rest.chars().next().filter(|ch| matches!(ch, '\'' | '"')) {
                let body = &rest[1..];
                let length = body
                    .find(quote)
                    .unwrap_or_else(|| body.find('\n').unwrap_or(body.len()));
                let end = start + 2 + length + usize::from(body[length..].starts_with(quote));
                (body[..length].to_string(), end)
            } else {
                let length = rest.find(char::is_whitespace).unwrap_or(rest.len());
                (rest[..length].to_string(), start + 1 + length)
            };
        if cursor <= end {
            return Some(AtToken { start, end, query });
        }
    }
    None
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
    let reference = if path.contains(char::is_whitespace) {
        format!("@\"{path}{}", if folder { "" } else { "\"" })
    } else {
        format!("@{path}")
    };
    AtSuggestion {
        label: path.to_string(),
        kind: if folder { AtKind::Folder } else { AtKind::File },
        insert: if folder {
            reference
        } else {
            format!("{reference} ")
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

    #[test]
    fn decorates_full_filenames_without_changing_the_input() {
        for (input, expected) in [
            ("read @src/main.rs and @x", vec!["@src/main.rs", "@x"]),
            (
                "Use @Packages to Country - Things to do component - Reviewed.xlsx please",
                vec!["@Packages to Country - Things to do component - Reviewed.xlsx"],
            ),
            (
                "Compare @\"screen shot.png\" and @'目录/表格 一.xlsx'",
                vec!["@\"screen shot.png\"", "@'目录/表格 一.xlsx'"],
            ),
            (
                "read @\"source file.rs\"#2-5 now",
                vec!["@\"source file.rs\"#2-5"],
            ),
            (
                "mail@example.com @ @docs/ then @notes.md, please",
                vec!["@docs/", "@notes.md"],
            ),
        ] {
            let ranges = mention_ranges(input);
            let mentions: Vec<_> = ranges.iter().map(|range| &input[range.clone()]).collect();
            assert_eq!(mentions, expected, "{input}");
        }
    }

    #[test]
    fn resolves_spaced_references_inside_the_project_without_a_tree_search() {
        let base = std::env::temp_dir().join(format!("oxide_at_files_{}", std::process::id()));
        let project = base.join("project");
        let sibling = base.join("sibling");
        std::fs::create_dir_all(project.join("nested")).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let sheet = project.join("Packages to Country - Things to do component - Reviewed.xlsx");
        let image = project.join("nested/screen shot.png");
        std::fs::write(&sheet, "sheet").unwrap();
        std::fs::write(&image, "image").unwrap();
        std::fs::write(project.join("Packages"), "shorter prefix").unwrap();
        std::fs::write(sibling.join("outside.xlsx"), "outside").unwrap();

        assert_eq!(referenced_files(
            "Submit @Packages to Country - Things to do component - Reviewed.xlsx to falcon_packages.",
            &project,
        ), vec![sheet.clone()]);
        assert_eq!(referenced_files(
            "Compare @\"nested/screen shot.png\" and @'Packages to Country - Things to do component - Reviewed.xlsx' please",
            &project,
        ), vec![image, sheet]);
        assert!(
            referenced_files("@outside.xlsx mail@example.com @missing.png", &project).is_empty()
        );
        let prose = format!("@Packages {}", "extra words ".repeat(200));
        assert_eq!(
            referenced_files(&prose, &project),
            vec![project.join("Packages")]
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn completes_quoted_paths_with_spaces_and_walks_into_folders() {
        let paths = vec![
            "reviewed copy/".into(),
            "reviewed copy/first entity.xlsx".into(),
        ];
        let folder = suggestions(&paths, token("@reviewed", 9).as_ref(), 200).remove(0);
        assert_eq!(folder.insert, "@\"reviewed copy/");
        let value = folder.insert;
        let rows = suggestions(&paths, token(&value, value.len()).as_ref(), 200);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].insert, "@\"reviewed copy/first entity.xlsx\" ");
        assert_eq!(
            token("@\"reviewed copy/first entity.xlsx\" next", 20)
                .unwrap()
                .query,
            "reviewed copy/first entity.xlsx"
        );
        assert!(token("@\"reviewed copy/first entity.xlsx\" next", 38).is_none());
    }

    #[test]
    fn resolves_line_references_and_keeps_hashes_in_real_filenames() {
        let dir = std::env::temp_dir().join(format!("oxide_at_ranges_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("source.rs");
        let hash = dir.join("issue#draft.md");
        std::fs::write(&file, "one\ntwo").unwrap();
        std::fs::write(&hash, "draft").unwrap();
        assert_eq!(
            referenced_files("@source.rs#2-5 and @issue#draft.md", &dir),
            vec![file, hash]
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

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
