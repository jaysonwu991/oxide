use anyhow::{Context, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// A shadow repo left over from a killed run keeps this lock, and every later
/// commit would fail until it is removed. Only reclaim it once it is old enough
/// that no live process can still be holding it.
const STALE_LOCK_AGE: Duration = Duration::from_secs(300);

/// Directories a shadow snapshot never records: build output and dependency
/// trees are both huge and useless to diff. Written to the shadow repo's
/// `info/exclude`, and skipped when a folder that is not a git work tree is
/// measured (see `work_tree_is_bounded`), since such a folder may have no
/// `.gitignore` to say so.
const EXCLUDED_DIRS: &[&str] = &["target", "node_modules", ".venv", "dist", "build"];

/// How much a snapshot may record of a directory that is not inside a git work
/// tree. There, git has no `.gitignore` to leave noise out of `add -A`, so a
/// folder that is not a project — a downloads directory, a media library — is
/// measured before a shadow repo is made for it. Inside a git work tree the
/// project's own ignores bound the work, so nothing is measured.
const MAX_UNTRACKED_FILES: usize = 20_000;
const MAX_UNTRACKED_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Snapshots {
    git_dir: PathBuf,
    work_tree: PathBuf,
}

impl Snapshots {
    /// A handle on the shadow repo at an explicit pair of paths, created when it
    /// is not there yet: [`open`](Self::open) resolves the paths from a project,
    /// and a caller that owns both — a front-end keeping the repo somewhere of
    /// its own, a test — can name them.
    pub fn at(git_dir: PathBuf, work_tree: PathBuf) -> Result<Self> {
        let snapshots = Self { git_dir, work_tree };
        snapshots.ensure_repo()?;
        Ok(snapshots)
    }

    pub fn open(cwd: &Path) -> Result<Self> {
        if !snapshot_scope_is_safe(cwd) {
            anyhow::bail!(
                "snapshots disabled: {} is not a project directory",
                cwd.display()
            );
        }
        // Keyed by where the project is rather than by the remote it is a clone
        // of (`memory::local_project_id`): the shadow repo reaches into the work
        // tree, so a baseline taken in one checkout must never be restored into
        // another clone of the same remote.
        let git_dir = crate::config::config_dir()
            .context("no config directory")?
            .join("snapshots")
            .join(crate::memory::local_project_id(cwd));
        Self::at(git_dir, cwd.to_path_buf())
    }

    fn ensure_repo(&self) -> Result<()> {
        std::fs::create_dir_all(self.git_dir.join("info"))?;
        self.clear_stale_lock();
        if !self.git_dir.join("HEAD").exists() {
            Command::new("git")
                .args(["init", "--quiet", "--bare"])
                .arg(&self.git_dir)
                .output()
                .context("running git init")?;
        }
        let exclude: String = EXCLUDED_DIRS
            .iter()
            .map(|dir| format!("{dir}/\n"))
            .collect();
        std::fs::write(self.git_dir.join("info/exclude"), exclude)?;
        if self.git(&["rev-parse", "HEAD"]).is_err() {
            self.git(&["commit", "--quiet", "--allow-empty", "-m", "initial"])?;
        }
        Ok(())
    }

    fn git_bytes(&self, args: &[&str]) -> Result<Vec<u8>> {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=oxide",
                "-c",
                "user.email=oxide@localhost",
                "-c",
                "core.autocrlf=false",
            ])
            .arg(format!("--git-dir={}", self.git_dir.display()))
            .arg(format!("--work-tree={}", self.work_tree.display()))
            .args(args)
            .output()
            .context("running git for snapshots")?;
        if !output.status.success() {
            anyhow::bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(output.stdout)
    }

    fn git(&self, args: &[&str]) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.git_bytes(args)?)
            .trim()
            .to_string())
    }

    pub fn commit(&self, label: &str) -> Result<Option<String>> {
        self.git(&["add", "-A"])?;
        if self.git(&["status", "--porcelain"])?.trim().is_empty() {
            return Ok(None);
        }
        self.git(&["commit", "--quiet", "-m", label])?;
        Ok(Some(self.git(&["rev-parse", "HEAD"])?))
    }

    /// The state a run starts from: the project's work tree as it stands,
    /// recorded in the shadow repo so a front-end can list the files the run
    /// changes — including ones no tool call named, like a formatter's or a
    /// shell command's — and put them back. `None` when the project must not be
    /// snapshotted (see [`snapshot_scope_is_safe`]), which is a run without a
    /// change listing rather than a failed run. A project that is not a git
    /// clone is snapshotted all the same.
    pub fn baseline(cwd: &Path) -> Option<(Snapshots, String)> {
        let snapshots = Snapshots::open(cwd).ok()?;
        let base = snapshots.mark().ok()?;
        Some((snapshots, base))
    }

    /// Records the work tree as it stands and returns the commit a run's own
    /// changes are compared against: a fresh commit when anything is different
    /// from the previous state, otherwise the current `HEAD` — so two runs in a
    /// row with nothing changed between them still start from what is on disk.
    pub fn mark(&self) -> Result<String> {
        self.mark_named("baseline")
    }

    /// The same, under a message: the state a run starts from (`mark`) or the
    /// state a finished turn left, which a later
    /// [`unchanged_since`](Self::unchanged_since) compares against.
    pub fn mark_named(&self, message: &str) -> Result<String> {
        if let Some(commit) = self.commit(message)? {
            return Ok(commit);
        }
        self.git(&["rev-parse", "HEAD"])
    }

    /// The files that changed since `base`, one entry per path, each with the
    /// line counts and the preview `crate::diff` renders. The work tree is
    /// staged first so a file a run deleted reads as deleted rather than as an
    /// unstaged removal.
    pub fn changes_since(&self, base: &str) -> Result<crate::changes::TurnChanges> {
        use crate::changes::ChangeStatus;

        self.git(&["add", "-A"])?;
        let status = self.git(&[
            "diff",
            "--cached",
            "--no-renames",
            "--name-status",
            "-z",
            base,
        ])?;
        // The counts come from git rather than from the rendered preview: a
        // change too large to preview is a one-line summary with no `+`/`-`
        // lines to count, so counting the preview would report a real change as
        // none.
        let numstat = self.git(&["diff", "--cached", "--no-renames", "--numstat", "-z", base])?;
        let counts: HashMap<String, crate::changes::Numstat> =
            crate::changes::parse_numstat(&numstat)
                .into_iter()
                .collect();
        let mut files = Vec::new();
        for (status, path) in crate::changes::parse_name_status(&status) {
            let old = match status {
                ChangeStatus::Added => None,
                _ => self.content_at(base, &path).ok(),
            };
            let new = match status {
                ChangeStatus::Deleted => None,
                _ => std::fs::read(self.work_tree.join(&path)).ok(),
            };
            let mut change = crate::changes::file_change(status, path, old, new);
            match counts.get(&change.path) {
                // git's own verdict on a file it will not diff (a NUL in it),
                // which is what the front-ends paint the row from.
                Some(count) if count.binary => {
                    change.binary = true;
                    change.diff = String::new();
                    change.added = 0;
                    change.removed = 0;
                }
                Some(count) if !change.binary => {
                    change.added = count.added;
                    change.removed = count.removed;
                }
                _ => {}
            }
            files.push(change);
        }
        Ok(crate::changes::summarize(files))
    }

    /// Whether the work tree still holds what `revision` recorded — the guard an
    /// undo takes before it puts the whole work tree back to a run's baseline,
    /// so restoring an older turn cannot discard what came after it. Staged
    /// first, exactly as [`changes_since`](Self::changes_since) stages, so a file
    /// the run created counts as a difference rather than being left aside.
    pub fn unchanged_since(&self, revision: &str) -> Result<bool> {
        self.git(&["add", "-A"])?;
        let listed = self.git(&[
            "diff",
            "--cached",
            "--no-renames",
            "--name-only",
            "-z",
            revision,
        ])?;
        Ok(listed.is_empty())
    }

    /// Puts the work tree back to `base` — the state a run started from — once
    /// it is known to still hold what `after` recorded. `after` is the state
    /// that run left behind, so the restore cannot also take a change made
    /// since, including one from a later run whose own baseline is the one to
    /// restore; an older run is refused with the reason rather than silently
    /// done. `None` is a caller with no marker to check (a front-end built
    /// before they existed), which is taken at its word.
    pub fn restore_turn(&self, base: &str, after: Option<&str>) -> Result<()> {
        if let Some(after) = after {
            if !self.unchanged_since(after)? {
                anyhow::bail!(
                    "the project has changed since that turn — undo the newest turn first"
                );
            }
        }
        self.restore(base)
    }

    /// One file's content at `revision`, as it stands in the shadow repo rather
    /// than as the work tree has it now — what a front-end draws on the left of
    /// a diff against what is on disk.
    pub fn content_at(&self, revision: &str, path: &str) -> Result<Vec<u8>> {
        self.git_bytes(&["show", &format!("{revision}:{path}")])
    }

    /// The same read, with a path the revision does not hold answering `None`
    /// rather than an error: a file the run added is not in its baseline at all,
    /// and that is the one side that reads as empty. A revision the repo no
    /// longer has — a shadow repo that was pruned, a card from another project —
    /// is still an error, so a front-end can tell an absent side from one it
    /// could not read instead of painting both as an empty file.
    pub fn content_at_opt(&self, revision: &str, path: &str) -> Result<Option<Vec<u8>>> {
        let err = match self.content_at(revision, path) {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(err) => err,
        };
        // `git show` fails the same way for a path the revision does not hold
        // and for a revision that is not there, so the revision is asked about
        // on its own before the path is read as absent.
        if self.has_revision(revision) {
            Ok(None)
        } else {
            Err(err)
        }
    }

    /// Whether the shadow repo has this revision as a commit — what tells a path
    /// that is missing from a baseline apart from a baseline that is missing.
    fn has_revision(&self, revision: &str) -> bool {
        self.git(&[
            "rev-parse",
            "--verify",
            "--quiet",
            &format!("{revision}^{{commit}}"),
        ])
        .is_ok()
    }

    /// Puts the work tree back to `base`, discarding everything a run wrote
    /// since it. Untracked files that were never staged are left alone, so a
    /// front-end can offer this as the inverse of a listed change without it
    /// reaching further than what was listed.
    pub fn restore(&self, base: &str) -> Result<()> {
        self.git(&["reset", "--hard", "--quiet", base])?;
        Ok(())
    }

    pub fn undo(&self) -> Result<bool> {
        let head = self.git(&["rev-parse", "HEAD"]).unwrap_or_default();
        let parent = match self.git(&["rev-parse", "HEAD~1"]) {
            Ok(parent) => parent,
            Err(_) => return Ok(false),
        };
        std::fs::write(self.redo_path(), &head)?;
        self.git(&["reset", "--hard", "--quiet", &parent])?;
        Ok(true)
    }

    pub fn redo(&self) -> Result<bool> {
        let target = match std::fs::read_to_string(self.redo_path()) {
            Ok(target) => target,
            Err(_) => return Ok(false),
        };
        if target.trim().is_empty() {
            return Ok(false);
        }
        self.git(&["reset", "--hard", "--quiet", target.trim()])?;
        let _ = std::fs::remove_file(self.redo_path());
        Ok(true)
    }

    fn redo_path(&self) -> PathBuf {
        self.git_dir.join("redo")
    }

    fn clear_stale_lock(&self) {
        let lock = self.git_dir.join("index.lock");
        let Ok(metadata) = std::fs::metadata(&lock) else {
            return;
        };
        let stale = metadata
            .modified()
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > STALE_LOCK_AGE);
        if stale {
            let _ = std::fs::remove_file(lock);
        }
    }
}

/// Snapshots hash every file in the work tree with `git add -A`, so they are
/// only safe for a bounded project. Refuse the user's home directory and its
/// ancestors (running oxide in `$HOME` otherwise indexes the entire home
/// folder, which can take minutes and gigabytes), anything that holds the
/// config directory — where the shadow repos themselves live — and a folder
/// that is neither a git work tree nor project-sized. What is left is a
/// project, whether or not it is a git clone.
fn snapshot_scope_is_safe(cwd: &Path) -> bool {
    if !cwd.is_dir() {
        return false;
    }
    // A volume root, or any ancestor of the home directory, holds everything.
    if cwd.parent().is_none() {
        return false;
    }
    if let Some(home) = dirs::home_dir() {
        if cwd == home || home.starts_with(cwd) {
            return false;
        }
    }
    if let Some(config) = crate::config::config_dir() {
        if config.starts_with(cwd) {
            return false;
        }
    }
    inside_git_work_tree(cwd) || work_tree_is_bounded(cwd)
}

fn inside_git_work_tree(path: &Path) -> bool {
    let mut current = Some(path);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return true;
        }
        current = dir.parent();
    }
    false
}

/// Whether a directory that is not a git work tree is small enough to record.
fn work_tree_is_bounded(cwd: &Path) -> bool {
    bounded_within(cwd, MAX_UNTRACKED_FILES, MAX_UNTRACKED_BYTES)
}

/// Walks `cwd` until it is known to be over one of the bounds, skipping the
/// directories the shadow repo excludes and never following a symlink (git
/// records the link itself, so what it points at is not part of the tree).
fn bounded_within(cwd: &Path, max_files: usize, max_bytes: u64) -> bool {
    let mut files = 0usize;
    let mut bytes = 0u64;
    let mut pending = vec![cwd.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_symlink() {
                continue;
            }
            if kind.is_dir() {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if name != ".git" && !EXCLUDED_DIRS.contains(&name.as_ref()) {
                    pending.push(entry.path());
                }
                continue;
            }
            files += 1;
            bytes += entry.metadata().map(|meta| meta.len()).unwrap_or(0);
            if files > max_files || bytes > max_bytes {
                return false;
            }
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::changes::ChangeStatus;

    #[test]
    fn commits_and_undoes_changes() {
        let root = std::env::temp_dir().join(format!("oxide_snap_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();

        let snapshots = Snapshots {
            git_dir: root.join("shadow"),
            work_tree: work.clone(),
        };
        snapshots.ensure_repo().unwrap();

        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        assert!(snapshots.commit("turn").unwrap().is_some());
        std::fs::write(work.join("a.txt"), "two\n").unwrap();
        assert!(snapshots.commit("turn").unwrap().is_some());

        assert!(snapshots.undo().unwrap());
        assert_eq!(
            std::fs::read_to_string(work.join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(snapshots.redo().unwrap());
        assert_eq!(
            std::fs::read_to_string(work.join("a.txt")).unwrap(),
            "two\n"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn refuses_volume_roots_home_and_the_config_directory() {
        assert!(!snapshot_scope_is_safe(Path::new("/")));
        assert!(!snapshot_scope_is_safe(Path::new("")));
        let missing = std::env::temp_dir().join(format!("oxide_snap_gone_{}", std::process::id()));
        assert!(!snapshot_scope_is_safe(&missing));
        if let Some(home) = dirs::home_dir() {
            assert!(!snapshot_scope_is_safe(&home));
            if let Some(parent) = home.parent() {
                assert!(!snapshot_scope_is_safe(parent));
            }
            // The shadow repos live under the config directory, so a work tree
            // holding it would index its own snapshots.
            if let Some(config) = crate::config::config_dir().filter(|dir| dir.exists()) {
                if let Some(parent) = config.parent().filter(|dir| dir.is_dir()) {
                    assert!(!snapshot_scope_is_safe(parent));
                }
                assert!(!snapshot_scope_is_safe(&config));
            }
        }
    }

    #[test]
    fn accepts_a_git_work_tree_and_a_plain_project_directory() {
        let root = std::env::temp_dir().join(format!("oxide_snap_scope_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        // A folder that is not a clone is a project like any other: the shadow
        // repo is oxide's own, so a turn's changes are listed there too.
        assert!(snapshot_scope_is_safe(&root));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert!(snapshot_scope_is_safe(&root));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn measures_a_directory_that_is_not_a_git_work_tree() {
        let root = std::env::temp_dir().join(format!("oxide_snap_size_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/main.rs"), "fn main() {}\n").unwrap();
        std::fs::write(root.join("README.md"), "# Readme\n").unwrap();

        assert!(bounded_within(&root, 2, 4096));
        assert!(!bounded_within(&root, 1, 4096));
        assert!(!bounded_within(&root, 2, 4));

        // Build output and dependencies are left out of the shadow repo, so a
        // project that keeps them there is still measured as project-sized.
        std::fs::create_dir_all(root.join("node_modules/pkg")).unwrap();
        for index in 0..20 {
            std::fs::write(
                root.join("node_modules/pkg").join(format!("f{index}.js")),
                "x",
            )
            .unwrap();
        }
        std::fs::remove_file(root.join("README.md")).unwrap();
        assert!(bounded_within(&root, 2, 4096));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn lists_what_changed_since_a_baseline() {
        let root = std::env::temp_dir().join(format!("oxide_snap3_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();

        let snapshots = Snapshots {
            git_dir: root.join("shadow"),
            work_tree: work.clone(),
        };
        snapshots.ensure_repo().unwrap();

        std::fs::write(work.join("keep.txt"), "one\ntwo\n").unwrap();
        std::fs::write(work.join("gone.txt"), "bye\n").unwrap();
        let base = snapshots.mark().unwrap();
        // Nothing has happened yet, so a run that does nothing lists nothing.
        assert!(snapshots.changes_since(&base).unwrap().is_empty());

        std::fs::write(work.join("keep.txt"), "one\nTWO\n").unwrap();
        std::fs::write(work.join("new.txt"), "hello\n").unwrap();
        std::fs::remove_file(work.join("gone.txt")).unwrap();
        // A file a shell command wrote is listed the same as an edited one.
        std::fs::write(work.join("logo.bin"), [0x00, 0xff, 0x01]).unwrap();

        let changes = snapshots.changes_since(&base).unwrap();
        let listed: Vec<(&str, ChangeStatus)> = changes
            .files
            .iter()
            .map(|file| (file.path.as_str(), file.status))
            .collect();
        assert_eq!(
            listed,
            vec![
                ("gone.txt", ChangeStatus::Deleted),
                ("keep.txt", ChangeStatus::Modified),
                ("logo.bin", ChangeStatus::Added),
                ("new.txt", ChangeStatus::Added),
            ]
        );
        assert_eq!(changes.added, 2);
        assert_eq!(changes.removed, 2);
        let keep = changes.files.iter().find(|f| f.path == "keep.txt").unwrap();
        assert!(keep.diff.contains("one"), "{}", keep.diff);
        assert!(changes.files.iter().any(|f| f.binary));

        // Undoing the run puts every file back, including the removed one, and
        // takes away the files the run created.
        snapshots.restore(&base).unwrap();
        assert_eq!(
            std::fs::read_to_string(work.join("keep.txt")).unwrap(),
            "one\ntwo\n"
        );
        assert!(work.join("gone.txt").exists());
        assert!(!work.join("new.txt").exists());
        assert!(!work.join("logo.bin").exists());
        assert!(snapshots.changes_since(&base).unwrap().is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_absent_side_is_none_and_a_missing_revision_is_an_error() {
        let root = std::env::temp_dir().join(format!("oxide_snap_opt_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();

        let snapshots = Snapshots {
            git_dir: root.join("shadow"),
            work_tree: work.clone(),
        };
        snapshots.ensure_repo().unwrap();

        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        let base = snapshots.mark().unwrap();

        assert_eq!(
            snapshots.content_at_opt(&base, "a.txt").unwrap(),
            Some(b"one\n".to_vec())
        );
        // The file the run added is not in the baseline at all, which is the one
        // absence that reads as an empty side.
        std::fs::write(work.join("b.txt"), "new\n").unwrap();
        assert_eq!(snapshots.content_at_opt(&base, "b.txt").unwrap(), None);
        // A baseline the repo does not have is not an absent file: the caller is
        // told rather than handed an empty side to paint.
        assert!(snapshots.content_at_opt("0000000", "a.txt").is_err());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn marks_the_state_a_run_starts_from() {
        let root = std::env::temp_dir().join(format!("oxide_snap4_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();

        let snapshots = Snapshots {
            git_dir: root.join("shadow"),
            work_tree: work.clone(),
        };
        snapshots.ensure_repo().unwrap();

        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        let first = snapshots.mark().unwrap();
        // A second mark with nothing changed between them is the same commit,
        // so a later turn is still compared against the state on disk.
        assert_eq!(snapshots.mark().unwrap(), first);
        std::fs::write(work.join("a.txt"), "two\n").unwrap();
        assert_ne!(snapshots.mark().unwrap(), first);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn skips_unchanged_commits() {
        let root = std::env::temp_dir().join(format!("oxide_snap2_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();

        let snapshots = Snapshots {
            git_dir: root.join("shadow"),
            work_tree: work.clone(),
        };
        snapshots.ensure_repo().unwrap();
        assert!(snapshots.commit("turn").unwrap().is_none());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn counts_a_change_too_large_to_preview() {
        let root = std::env::temp_dir().join(format!("oxide_snap5_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();

        let snapshots = Snapshots {
            git_dir: root.join("shadow"),
            work_tree: work.clone(),
        };
        snapshots.ensure_repo().unwrap();

        let over = |lines: usize| {
            (0..lines)
                .map(|n| format!("line {n}\n"))
                .collect::<String>()
        };
        std::fs::write(work.join("big.txt"), over(2_100)).unwrap();
        let base = snapshots.mark().unwrap();
        let mut grown = over(2_100);
        grown.push_str("added\n");
        std::fs::write(work.join("big.txt"), grown).unwrap();

        // The preview is a one-line summary with no `+`/`-` line in it, so the
        // counts have to come from git rather than from counting the preview —
        // otherwise a file this size reads as a change with nothing in it.
        let changes = snapshots.changes_since(&base).unwrap();
        assert_eq!(changes.files.len(), 1);
        assert!(
            changes.files[0].diff.starts_with("(diff omitted"),
            "{}",
            changes.files[0].diff
        );
        assert_eq!((changes.files[0].added, changes.files[0].removed), (1, 0));
        assert_eq!((changes.added, changes.removed), (1, 0));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn tells_whether_the_work_tree_still_holds_a_revision() {
        let root = std::env::temp_dir().join(format!("oxide_snap6_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();

        let snapshots = Snapshots {
            git_dir: root.join("shadow"),
            work_tree: work.clone(),
        };
        snapshots.ensure_repo().unwrap();

        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        let base = snapshots.mark().unwrap();
        assert!(snapshots.unchanged_since(&base).unwrap());

        // An edit after the revision is a difference, and so is a file the
        // revision never saw — both are what an undo would discard.
        std::fs::write(work.join("a.txt"), "two\n").unwrap();
        assert!(!snapshots.unchanged_since(&base).unwrap());
        snapshots.restore(&base).unwrap();
        std::fs::write(work.join("new.txt"), "new\n").unwrap();
        assert!(!snapshots.unchanged_since(&base).unwrap());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn restores_a_turn_only_while_nothing_came_after_it() {
        let root = std::env::temp_dir().join(format!("oxide_snap7_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();

        let snapshots = Snapshots {
            git_dir: root.join("shadow"),
            work_tree: work.clone(),
        };
        snapshots.ensure_repo().unwrap();

        // A run: the state it found, what it wrote, and the state it left.
        std::fs::write(work.join("a.txt"), "one\n").unwrap();
        let base = snapshots.mark().unwrap();
        std::fs::write(work.join("a.txt"), "two\n").unwrap();
        std::fs::write(work.join("b.txt"), "new\n").unwrap();
        let after = snapshots.mark_named("turn").unwrap();

        // An edit of the reader's own is not this turn's to discard.
        std::fs::write(work.join("a.txt"), "three\n").unwrap();
        let refused = snapshots.restore_turn(&base, Some(&after)).unwrap_err();
        assert!(refused.to_string().contains("changed since that turn"));
        assert_eq!(
            std::fs::read_to_string(work.join("a.txt")).unwrap(),
            "three\n"
        );

        // The newest turn's own undo puts back what it wrote and takes away what
        // it created.
        let after = snapshots.mark_named("turn").unwrap();
        snapshots.restore_turn(&base, Some(&after)).unwrap();
        assert_eq!(
            std::fs::read_to_string(work.join("a.txt")).unwrap(),
            "one\n"
        );
        assert!(!work.join("b.txt").exists());

        // A card with no marker is taken at its word, as it was before one.
        std::fs::write(work.join("a.txt"), "four\n").unwrap();
        snapshots.restore_turn(&base, None).unwrap();
        assert_eq!(
            std::fs::read_to_string(work.join("a.txt")).unwrap(),
            "one\n"
        );

        std::fs::remove_dir_all(&root).ok();
    }
}
