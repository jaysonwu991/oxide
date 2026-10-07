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
    view_with(folder, &read_file)
}

/// The same read over injected file reads, so every layout can be held by tests
/// without a repository on the machine that runs them. A directory answers
/// nothing, which is how a `.git` directory is told from a `.git` file.
fn view_with(folder: &Path, read: &dyn Fn(&Path) -> Option<String>) -> GitView {
    let mut view = GitView::default();
    for dir in folder.ancestors() {
        let dot_git = dir.join(".git");
        let head = match read(&dot_git.join("HEAD")) {
            Some(head) => head,
            None => {
                // A worktree stores an absolute path; a submodule's is relative
                // to the folder the `.git` file sits in.
                let Some(git_dir) = read(&dot_git).as_deref().and_then(git_dir_of) else {
                    continue;
                };
                let git_dir = if git_dir.is_absolute() {
                    git_dir
                } else {
                    dir.join(git_dir)
                };
                match read(&git_dir.join("HEAD")) {
                    Some(head) => head,
                    None => continue,
                }
            }
        };
        view.repo = true;
        view.root = dir.display().to_string();
        match branch_of(&head) {
            Some(Branch::Named(name)) => view.branch = name,
            Some(Branch::Detached(id)) => view.detached = id,
            None => {}
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
    let hex = line.len() >= 7 && line.len() <= 40 && line.chars().all(|c| c.is_ascii_hexdigit());
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

fn read_file(path: &Path) -> Option<String> {
    std::fs::read_to_string(path).ok()
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

    fn view_over(root: &str, entries: &[(&str, &str)]) -> GitView {
        let map = files(entries);
        view_with(Path::new(root), &|path| map.get(path).cloned())
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
