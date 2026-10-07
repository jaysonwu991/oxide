//! The repository the composer's bar names.
//!
//! The branch is read out of the repository's own `HEAD` file rather than by
//! running git: naming the branch costs a file read, works on a machine with no
//! git on `PATH`, and answers the two layouts a project may be in — a checkout,
//! whose `.git` is a directory, and a worktree or a submodule, whose `.git` is a
//! file pointing at the directory that holds its real `HEAD`. The VS Code panel
//! reads a branch by the same rule (`editors/vscode/src/core/git.ts`); this is
//! that rule in the half that speaks Rust.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// What the bar says about the folder's repository. A folder that is not inside
/// one has no branch, and a detached HEAD has no branch either — it is named by
/// the commit it is on — so each of those is a field of its own rather than an
/// empty branch that would read as a name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct GitView {
    pub repo: bool,
    /// The folder the `.git` the branch was read from sits in, so the tooltip
    /// says which repository a project inside another one belongs to.
    pub root: String,
    pub branch: String,
    /// The commit HEAD holds when no branch does, short.
    pub detached: String,
}

/// The repository `folder` is in, if any: the closest `.git` at or above it
/// wins, so a folder inside a clone reports that clone's branch and a submodule
/// reports its own rather than its parent's.
pub fn view(folder: &Path) -> GitView {
    view_with(folder, &read_entry)
}

/// The same read over injected file reads, so every layout can be held by tests
/// without a repository on the machine that runs them.
fn view_with(folder: &Path, entry: &dyn Fn(&Path) -> Entry) -> GitView {
    let mut view = GitView::default();
    for dir in folder.ancestors() {
        let dot_git = dir.join(".git");
        let head = match entry(&dot_git) {
            // A checkout keeps its HEAD in a `.git` directory of its own.
            Entry::Directory => Some(entry(&dot_git.join("HEAD"))),
            // A worktree or a submodule points at the directory that does: an
            // absolute path for the one, one relative to this folder for the
            // other.
            Entry::Text(pointer) => {
                let git_dir = git_dir_of(&pointer).map(|git_dir| {
                    if git_dir.is_absolute() {
                        git_dir
                    } else {
                        dir.join(git_dir)
                    }
                });
                Some(git_dir.map_or(Entry::Missing, |git_dir| entry(&git_dir.join("HEAD"))))
            }
            Entry::Missing => None,
        };
        let Some(head) = head else { continue };
        // A `.git` is where the walk ends, whether or not its HEAD can be read:
        // this folder belongs to that repository, and the branch of one above it
        // is not this folder's — a submodule whose git directory is gone has no
        // branch to show rather than its parent's.
        view.repo = true;
        view.root = dir.display().to_string();
        if let Entry::Text(head) = head {
            match branch_of(&head) {
                Some(Branch::Named(name)) => view.branch = name,
                Some(Branch::Detached(id)) => view.detached = id,
                None => {}
            }
        }
        break;
    }
    view
}

enum Branch {
    Named(String),
    Detached(String),
}

/// `ref: refs/heads/main` → `main`. A HEAD that holds a commit id rather than a
/// ref is that commit, short: there is no branch to name, and saying the commit
/// is the whole truth about where the folder stands.
fn branch_of(head: &str) -> Option<Branch> {
    let line = head.trim();
    if let Some(name) = line.strip_prefix("ref:") {
        let name = name.trim();
        let name = name.strip_prefix("refs/heads/").unwrap_or(name);
        return (!name.is_empty()).then(|| Branch::Named(name.to_string()));
    }
    // Git's two object formats: a SHA-1 id is 40 hex characters and a SHA-256 is
    // 64, so a repository on either names the commit it is detached at.
    let hex = line.len() >= 7 && line.len() <= 64 && line.chars().all(|c| c.is_ascii_hexdigit());
    hex.then(|| Branch::Detached(line[..7].to_string()))
}

/// The `gitdir:` target of a `.git` file, which is how a worktree or a submodule
/// points at the directory holding its real HEAD.
fn git_dir_of(text: &str) -> Option<PathBuf> {
    let line = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))?;
    let path = PathBuf::from(line.trim());
    (!path.as_os_str().is_empty()).then_some(path)
}

/// What a path the walk asks about is. A `.git` directory and a `.git` file are
/// both repositories while a directory answers no text of its own, so the two
/// are told apart — and told apart from a folder with no repository above it.
enum Entry {
    Text(String),
    Directory,
    Missing,
}

fn read_entry(path: &Path) -> Entry {
    if path.is_dir() {
        return Entry::Directory;
    }
    match std::fs::read_to_string(path) {
        Ok(text) => Entry::Text(text),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Entry::Missing,
        // A marker that is here but cannot be read is still a repository whose
        // HEAD is out of reach: the walk stops on it rather than stepping over
        // it to a repository above.
        Err(_) => Entry::Text(String::new()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn files(entries: &[(&str, &str)]) -> HashMap<PathBuf, String> {
        entries
            .iter()
            .map(|(path, text)| (PathBuf::from(path), (*text).to_string()))
            .collect()
    }

    /// The fixture's filesystem: a path a test wrote is that file's text, a path
    /// something else sits under is a directory, and the rest is not there.
    fn view_over(root: &str, entries: &[(&str, &str)]) -> GitView {
        let map = files(entries);
        view_with(Path::new(root), &|path| {
            if let Some(text) = map.get(path) {
                return Entry::Text(text.clone());
            }
            let under = format!("{}/", path.display());
            if map
                .keys()
                .any(|key| key.display().to_string().starts_with(&under))
            {
                return Entry::Directory;
            }
            Entry::Missing
        })
    }

    #[test]
    fn a_project_reports_the_branch_its_head_is_on() {
        let view = view_over(
            "/work/app",
            &[("/work/app/.git/HEAD", "ref: refs/heads/fix/bar\n")],
        );
        assert_eq!(
            view,
            GitView {
                repo: true,
                root: "/work/app".to_string(),
                branch: "fix/bar".to_string(),
                detached: String::new(),
            }
        );
    }

    #[test]
    fn the_closest_repository_wins_and_names_its_own_root() {
        let view = view_over(
            "/work/app/packages/web",
            &[("/work/app/.git/HEAD", "ref: refs/heads/main\n")],
        );
        assert!(view.repo);
        assert_eq!(view.branch, "main");
        assert_eq!(view.root, "/work/app");
    }

    #[test]
    fn a_worktree_is_read_through_the_git_file_that_points_at_it() {
        let view = view_over(
            "/work/app.worktrees/feature",
            &[
                (
                    "/work/app.worktrees/feature/.git",
                    "gitdir: /work/app/.git/worktrees/feature\n",
                ),
                (
                    "/work/app/.git/worktrees/feature/HEAD",
                    "ref: refs/heads/feature\n",
                ),
            ],
        );
        assert_eq!(view.branch, "feature");
        assert_eq!(view.root, "/work/app.worktrees/feature");
    }

    /// A submodule's pointer is relative to the folder the `.git` file sits in,
    /// and is joined as written — git's own `../.git/modules/<name>`.
    #[test]
    fn a_submodule_reads_the_git_dir_relative_to_its_own_folder() {
        let view = view_over(
            "/work/app/vendor/lib",
            &[
                ("/work/app/vendor/lib/.git", "gitdir: ../.git/modules/lib\n"),
                (
                    "/work/app/vendor/lib/../.git/modules/lib/HEAD",
                    "ref: refs/heads/vendor\n",
                ),
            ],
        );
        assert_eq!(view.branch, "vendor");
    }

    #[test]
    fn a_detached_head_is_named_by_its_commit() {
        let view = view_over(
            "/work/app",
            &[(
                "/work/app/.git/HEAD",
                "9cdea1c5e65e8c23c7e35d867d5c35f826bb3020\n",
            )],
        );
        assert_eq!(view.branch, "");
        assert_eq!(view.detached, "9cdea1c");
    }

    /// A repository whose HEAD cannot be read is where the walk ends: a
    /// submodule whose git directory is gone belongs to that repository rather
    /// than to the parent clone, so the parent's branch is not shown for it.
    #[test]
    fn a_repository_with_no_readable_head_stops_the_walk() {
        let view = view_over(
            "/work/app/vendor/lib",
            &[
                ("/work/app/.git/HEAD", "ref: refs/heads/main\n"),
                ("/work/app/vendor/lib/.git", "gitdir: ../.git/modules/lib\n"),
            ],
        );
        assert!(view.repo);
        assert_eq!(view.root, "/work/app/vendor/lib");
        assert_eq!(view.branch, "");
        assert_eq!(view.detached, "");
    }

    /// A SHA-256 repository's object ids are 64 hex characters, so a detached
    /// HEAD there is named by its commit the same way a SHA-1 one is.
    #[test]
    fn a_sha256_detached_head_is_named_by_its_commit() {
        let view = view_over(
            "/work/app",
            &[(
                "/work/app/.git/HEAD",
                "d2b90c9f5f3a4e6c8d1b0a7e2f4c9d3a5b8e0f1c7a4d6e9b2c5f8031a7d4e6b\n",
            )],
        );
        assert_eq!(view.branch, "");
        assert_eq!(view.detached, "d2b90c9");
    }

    #[test]
    fn a_folder_outside_a_repository_has_no_branch() {
        let view = view_over("/work/plain", &[("/work/plain/README.md", "hello")]);
        assert_eq!(view, GitView::default());
        assert!(!view.repo);
    }

    #[test]
    fn a_head_that_holds_neither_a_ref_nor_a_commit_names_nothing() {
        assert!(branch_of("").is_none());
        assert!(branch_of("not a ref").is_none());
    }

    /// The read is the real one here: a `.git` directory written by hand is
    /// enough to name a branch, and no git binary is needed to hold that.
    #[test]
    fn a_repository_on_disk_is_read_without_running_git() {
        let root = std::env::temp_dir().join(format!("oxide_desktop_git_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".git")).expect("creating the fixture");
        std::fs::write(root.join(".git/HEAD"), "ref: refs/heads/main\n").expect("writing HEAD");
        assert_eq!(view(&root).branch, "main");
        assert_eq!(view(&root).root, root.display().to_string());
        let _ = std::fs::remove_dir_all(&root);
    }
}
