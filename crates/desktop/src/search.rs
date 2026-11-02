//! Searching the threads a reader has: what a phrase from a conversation, or a
//! project or worktree it ran in, turns up.
//!
//! The search is over the same text the window paints — the display of each
//! stored message — so a phrase found here is a phrase the reader can see when
//! the thread opens. A log that cannot be read is skipped rather than failing the
//! whole search: one damaged file in a store of hundreds must not be the reason
//! nothing is found.

use crate::manager::SessionView;
use anyhow::Result;
use oxide_core::session::SessionLog;
use serde::Serialize;
use std::path::Path;

/// How much of the line comes back around a match, in characters rather than
/// bytes: a phrase in Chinese is not cut through the middle of a character, and
/// a long line still reads as the line it was found on.
const SNIPPET_BEFORE: usize = 42;
const SNIPPET_AFTER: usize = 96;

/// Matches counted in one thread, which is the point where the row's count stops
/// being something a reader reads and starts being something a search spends
/// time on.
const MATCH_LIMIT: usize = 200;

#[derive(Debug, Clone, Serialize)]
pub struct SearchHit {
    #[serde(flatten)]
    pub session: SessionView,
    /// The line the first match was found on, clipped around it. Empty when the
    /// phrase matched the thread's own name and no message.
    pub snippet: String,
    pub matches: usize,
    /// Whether the thread's name or preview carries the phrase, which the row
    /// says so a reader knows why a thread with no visible line is in the list.
    pub in_title: bool,
}

/// Every thread whose name, preview or messages carry the phrase, most recently
/// used first — the views arrive in that order and are not re-sorted, since a
/// search answers "where does this come up", not "what should I open".
pub fn search(views: &[SessionView], query: &str, limit: usize) -> Vec<SearchHit> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() || limit == 0 {
        return Vec::new();
    }
    let mut hits = Vec::new();
    for view in views {
        if hits.len() >= limit {
            break;
        }
        let in_title = view
            .summary
            .name
            .as_deref()
            .is_some_and(|name| name.to_lowercase().contains(&needle))
            || view.summary.preview.to_lowercase().contains(&needle);
        let in_folder = std::iter::once(view.summary.cwd.as_str())
            .chain(view.project.as_deref())
            .chain(view.worktree.as_deref())
            .any(|folder| folder.to_lowercase().contains(&needle));
        let (snippet, matches) = scan(&view.summary.path, &needle).unwrap_or_default();
        if in_title || in_folder || matches > 0 {
            hits.push(SearchHit {
                session: view.clone(),
                snippet,
                matches,
                in_title,
            });
        }
    }
    hits
}

/// The first match's line and how many the messages hold.
fn scan(path: &Path, needle: &str) -> Result<(String, usize)> {
    let log = SessionLog::open(path.to_path_buf())?;
    let messages = log.messages()?;
    let mut matches = 0;
    let mut snippet = String::new();
    for message in &messages {
        let Some(text) = message.display() else {
            continue;
        };
        let lower = text.to_lowercase();
        let mut from = 0;
        while let Some(at) = lower[from..].find(needle) {
            let index = from + at;
            matches += 1;
            if snippet.is_empty() {
                snippet = clip(&text, &lower, index);
            }
            if matches >= MATCH_LIMIT {
                return Ok((snippet, matches));
            }
            from = index + needle.len().max(1);
        }
    }
    Ok((snippet, matches))
}

/// The line around a match, one line of it: the text either side of the phrase
/// in the case it was stored, clipped by characters and joined so a snippet from
/// a wrapped paragraph still reads as one row.
fn clip(text: &str, lower: &str, at: usize) -> String {
    debug_assert!(lower.is_char_boundary(at));
    let mut folded_bytes = 0;
    let start_char = text
        .chars()
        .take_while(|character| {
            if folded_bytes >= at {
                return false;
            }
            folded_bytes += character.to_lowercase().map(char::len_utf8).sum::<usize>();
            folded_bytes <= at
        })
        .count();
    let start = start_char.saturating_sub(SNIPPET_BEFORE);
    let mut characters = text.chars().skip(start);
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.extend(characters.by_ref().take(start_char - start + SNIPPET_AFTER));
    if characters.next().is_some() {
        out.push('…');
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::llm::{Message, MessageContent};
    use oxide_core::session::SessionSummary;

    fn view(id: &str, name: Option<&str>, preview: &str, path: std::path::PathBuf) -> SessionView {
        SessionView {
            summary: SessionSummary {
                id: id.to_string(),
                name: name.map(str::to_string),
                cwd: "/tmp/oxide".to_string(),
                created_at: 1,
                modified_at: 2,
                message_count: 1,
                preview: preview.to_string(),
                path,
            },
            pinned: false,
            worktree: None,
            project: None,
            pulls: Vec::new(),
        }
    }

    fn thread(dir: &Path, said: &[&str]) -> std::path::PathBuf {
        let cwd = dir.join("project");
        std::fs::create_dir_all(&cwd).unwrap();
        let log = SessionLog::create(&cwd).unwrap();
        for text in said {
            let message = Message {
                role: "user".into(),
                content: Some(MessageContent::Text((*text).into())),
                tool_calls: None,
                tool_call_id: None,
                thinking: None,
            };
            log.append(&message).unwrap();
        }
        log.summary().unwrap().path
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_search_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn finds_the_line_a_phrase_is_on() {
        let dir = temp("line");
        let path = thread(&dir, &["nothing here", "the login redirect loops"]);
        let view = view("s1", None, "the login redirect loops", path);
        let hits = search(&[view], "REDIRECT", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matches, 1);
        assert!(hits[0].snippet.contains("the login redirect loops"));
        assert!(hits[0].in_title, "the preview carries the phrase too");
    }

    #[test]
    fn counts_every_match_and_says_where_it_started() {
        let dir = temp("count");
        let path = thread(&dir, &["redirect one", "redirect two", "redirect three"]);
        let view = view("s2", None, "nothing in the preview", path);
        let hits = search(&[view], "redirect", 10);
        assert_eq!(hits[0].matches, 3);
        assert!(hits[0].snippet.contains("redirect one"));
        assert!(!hits[0].in_title);
    }

    #[test]
    fn finds_a_thread_by_its_name_alone() {
        let dir = temp("name");
        let path = thread(&dir, &["unrelated"]);
        let view = view("s3", Some("Login redirect"), "", path);
        let hits = search(&[view], "redirect", 10);
        assert_eq!(hits.len(), 1);
        assert!(hits[0].in_title);
        assert_eq!(hits[0].matches, 0);
        assert!(hits[0].snippet.is_empty());
    }

    #[test]
    fn nothing_carries_the_phrase_and_nothing_comes_back() {
        let dir = temp("miss");
        let path = thread(&dir, &["a different sentence"]);
        let view = view("s4", Some("A thread"), "a different sentence", path);
        assert!(search(&[view], "redirect", 10).is_empty());
        assert!(search(&[], "redirect", 10).is_empty());
    }

    #[test]
    fn a_blank_phrase_finds_nothing_instead_of_everything() {
        let dir = temp("blank");
        let path = thread(&dir, &["anything"]);
        let view = view("s5", Some("anything"), "anything", path);
        assert!(search(std::slice::from_ref(&view), "", 10).is_empty());
        assert!(search(&[view], "   ", 10).is_empty());
    }

    #[test]
    fn the_limit_is_the_rows_a_reader_was_promised() {
        let dir = temp("limit");
        let mut views = Vec::new();
        for index in 0..4 {
            let id = format!("s{index}");
            let path = thread(&dir, &["redirect"]);
            views.push(view(&id, None, "redirect", path));
        }
        assert_eq!(search(&views, "redirect", 2).len(), 2);
        assert_eq!(search(&views, "redirect", 0).len(), 0);
    }

    #[test]
    fn a_log_that_cannot_be_read_does_not_stop_the_others() {
        let dir = temp("damaged");
        let good = thread(&dir, &["redirect"]);
        let broken = dir.join("broken.jsonl");
        std::fs::write(&broken, "{not a session\n").unwrap();
        let views = vec![
            view("s6", None, "", broken),
            view("s7", None, "redirect", good),
        ];
        let hits = search(&views, "redirect", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].session.summary.id, "s7");
    }

    #[test]
    fn a_long_line_is_clipped_around_the_match_with_ellipses() {
        let dir = temp("clip");
        let filler = "word ".repeat(80);
        let said = format!("{filler}redirect{filler}");
        let path = thread(&dir, &[said.as_str()]);
        let view = view("s8", None, "", path);
        let hits = search(&[view], "redirect", 10);
        let snippet = &hits[0].snippet;
        assert!(
            snippet.starts_with('…') && snippet.ends_with('…'),
            "{snippet}"
        );
        assert!(snippet.contains("redirect"));
        assert!(snippet.chars().count() < 200, "{}", snippet.chars().count());
    }

    #[test]
    fn a_phrase_in_the_middle_of_a_multiline_message_reads_as_one_row() {
        let dir = temp("multiline");
        let path = thread(&dir, &["first line\nsecond line with redirect\nthird"]);
        let view = view("s9", None, "", path);
        let hits = search(&[view], "redirect", 10);
        assert!(!hits[0].snippet.contains('\n'), "{}", hits[0].snippet);
        assert!(hits[0].snippet.contains("second line with redirect"));
    }

    #[test]
    fn finds_project_and_worktree_paths_without_message_matches() {
        let dir = temp("folders");
        let path = thread(&dir, &["unrelated"]);
        let mut view = view("folder", None, "", path);
        view.project = Some("/projects/oxide".into());
        view.worktree = Some("/worktrees/fix-login".into());
        for query in ["PROJECTS/OXIDE", "fix-login", "/tmp/oxide"] {
            let hits = search(std::slice::from_ref(&view), query, 10);
            assert_eq!(hits.len(), 1, "{query}");
            assert_eq!(hits[0].matches, 0);
            assert!(!hits[0].in_title);
        }
    }

    #[test]
    fn a_snippet_stays_at_its_match_after_expanding_lowercase_characters() {
        let text = format!("{} redirect {}", "İ".repeat(200), "tail ".repeat(100));
        let lower = text.to_lowercase();
        let snippet = clip(&text, &lower, lower.find("redirect").unwrap());
        assert!(snippet.contains("redirect"), "{snippet}");
        assert!(snippet.starts_with('…') && snippet.ends_with('…'));
        assert!(snippet.chars().count() < 150);
    }
}
