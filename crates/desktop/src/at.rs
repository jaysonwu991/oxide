//! The `@path` completion the desktop composer offers.
//!
//! The rules themselves live in `oxide_core::at`, shared with the terminal, so
//! a reference completes the same way in both. What is desktop-specific is the
//! two sides of the bridge: a browser reports a caret as a UTF-16 index while
//! the core works in bytes, and the project's path list is worth walking once
//! rather than on every keystroke.

use oxide_core::at::{self, AtSuggestion};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The answer to one completion: the rows to offer, and the range of the
/// composer's value they replace — or no rows at all, to close the list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AtAnswer {
    pub start: usize,
    pub end: usize,
    pub rows: Vec<AtSuggestion>,
}

/// The project's files and folders, walked once and kept until something
/// changes them. A turn is where files appear, so the app drops this when a
/// turn ends rather than completing over a listing that predates the work.
#[derive(Default)]
pub struct PathCache {
    entry: Mutex<Option<(PathBuf, Arc<Vec<String>>)>>,
}

impl PathCache {
    /// The project's own paths, from the shared walker, so a suggestion never
    /// names build output, dependencies or anything else `.gitignore` excludes.
    pub fn paths(&self, cwd: &Path) -> Arc<Vec<String>> {
        let mut entry = self
            .entry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some((root, paths)) = entry.as_ref() {
            if root == cwd {
                return Arc::clone(paths);
            }
        }
        let paths = Arc::new(oxide_core::tools::workspace_paths(cwd));
        *entry = Some((cwd.to_path_buf(), Arc::clone(&paths)));
        paths
    }

    pub fn clear(&self) {
        let mut entry = self
            .entry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *entry = None;
    }
}

/// The rows for the reference at `caret` in `value`, with the range they replace
/// expressed the way the composer counts: `caret` and the returned offsets are
/// the webview's own UTF-16 indices.
///
/// A value with no reference under the caret answers with no rows, which is how
/// the composer closes a list for a token that is done.
pub fn suggestions(cwd: &Path, cache: &PathCache, value: &str, caret: usize) -> AtAnswer {
    let empty = AtAnswer {
        start: caret,
        end: caret,
        rows: Vec::new(),
    };
    let Some(token) = at::token(value, byte_offset(value, caret)) else {
        return empty;
    };
    let paths = cache.paths(cwd);
    AtAnswer {
        start: utf16_offset(value, token.start),
        end: utf16_offset(value, token.end),
        rows: at::suggestions(&paths, Some(&token), at::MAX_AT_SUGGESTIONS),
    }
}

/// The byte offset a UTF-16 caret index names. A caret inside a surrogate pair
/// lands just past the character it is in, since a text field cannot hold one
/// between the halves.
fn byte_offset(value: &str, caret: usize) -> usize {
    let mut units = 0;
    for (byte, ch) in value.char_indices() {
        if units >= caret {
            return byte;
        }
        units += ch.len_utf16();
    }
    value.len()
}

/// The UTF-16 index a byte offset names.
fn utf16_offset(value: &str, byte: usize) -> usize {
    value
        .get(..byte)
        .unwrap_or(value)
        .chars()
        .map(char::len_utf16)
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use oxide_core::at::AtKind;

    fn project(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_at_{name}_{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/main.rs"), "fn main() {}").unwrap();
        std::fs::write(dir.join("README.md"), "oxide").unwrap();
        dir
    }

    #[test]
    fn completes_a_reference_from_the_project() {
        let dir = project("complete");
        let cache = PathCache::default();
        let answer = suggestions(&dir, &cache, "review @mai", 11);
        assert_eq!(answer.start, 7);
        assert_eq!(answer.end, 11);
        assert_eq!(
            answer.rows,
            [AtSuggestion {
                label: "src/main.rs".to_string(),
                kind: AtKind::File,
                // A file takes a space, so the next word can be typed.
                insert: "@src/main.rs ".to_string(),
            }]
        );

        // A folder keeps its own token open, so the query goes on narrowing
        // inside it rather than closing the reference with a space.
        let answer = suggestions(&dir, &cache, "see @sr", 7);
        assert_eq!(answer.rows[0].label, "src/");
        assert_eq!(answer.rows[0].kind, AtKind::Folder);
        assert_eq!(answer.rows[0].insert, "@src/");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn counts_the_caret_the_way_the_webview_does() {
        let dir = project("caret");
        let cache = PathCache::default();
        // A character outside the BMP is two UTF-16 units: the token has to be
        // measured the way the text field measures it, not in bytes.
        let value = "😀 see @sr";
        let answer = suggestions(
            &dir,
            &cache,
            value,
            value.chars().map(char::len_utf16).sum(),
        );
        assert_eq!((answer.start, answer.end), (7, 10));
        assert_eq!(answer.rows[0].label, "src/");

        // A caret outside a reference offers nothing and replaces nothing.
        let answer = suggestions(&dir, &cache, "a plain message", 15);
        assert!(answer.rows.is_empty());
        assert_eq!((answer.start, answer.end), (15, 15));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn walks_the_project_once_until_it_is_dropped() {
        let dir = project("cache");
        let cache = PathCache::default();
        let first = cache.paths(&dir);
        assert!(Arc::ptr_eq(&first, &cache.paths(&dir)));
        assert!(first.contains(&"src/main.rs".to_string()));

        // A file written after the walk is what dropping the cache is for.
        std::fs::write(dir.join("NEW.md"), "new").unwrap();
        assert!(!cache.paths(&dir).contains(&"NEW.md".to_string()));
        cache.clear();
        assert!(cache.paths(&dir).contains(&"NEW.md".to_string()));

        // Another project is not the cached one.
        let other = project("cache_other");
        assert!(cache.paths(&other).contains(&"README.md".to_string()));
        assert!(!cache.paths(&other).is_empty());
        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&other).ok();
    }
}
