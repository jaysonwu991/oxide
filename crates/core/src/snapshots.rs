use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

/// A shadow repo left over from a killed run keeps this lock, and every later
/// commit would fail until it is removed. Only reclaim it once it is old enough
/// that no live process can still be holding it.
const STALE_LOCK_AGE: Duration = Duration::from_secs(300);

#[derive(Debug, Clone)]
pub struct Snapshots {
    git_dir: PathBuf,
    work_tree: PathBuf,
}

impl Snapshots {
    pub fn open(cwd: &Path) -> Result<Self> {
        if !snapshot_scope_is_safe(cwd) {
            anyhow::bail!(
                "snapshots disabled: {} is not inside a bounded git work tree",
                cwd.display()
            );
        }
        let git_dir = dirs::config_dir()
            .context("no config directory")?
            .join("oxide")
            .join("snapshots")
            .join(crate::memory::project_id(cwd));
        let snapshots = Self {
            git_dir,
            work_tree: cwd.to_path_buf(),
        };
        snapshots.ensure_repo()?;
        Ok(snapshots)
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
        let exclude = "target/\nnode_modules/\n.venv/\ndist/\nbuild/\n";
        std::fs::write(self.git_dir.join("info/exclude"), exclude)?;
        if self.git(&["rev-parse", "HEAD"]).is_err() {
            self.git(&["commit", "--quiet", "--allow-empty", "-m", "initial"])?;
        }
        Ok(())
    }

    fn git(&self, args: &[&str]) -> Result<String> {
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
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    pub fn commit(&self, label: &str) -> Result<Option<String>> {
        self.git(&["add", "-A"])?;
        if self.git(&["status", "--porcelain"])?.trim().is_empty() {
            return Ok(None);
        }
        self.git(&["commit", "--quiet", "-m", label])?;
        Ok(Some(self.git(&["rev-parse", "HEAD"])?))
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
/// folder, which can take minutes and gigabytes) and anything that is not
/// inside a git work tree.
fn snapshot_scope_is_safe(cwd: &Path) -> bool {
    if let Some(home) = dirs::home_dir() {
        if cwd == home || home.starts_with(cwd) {
            return false;
        }
    }
    inside_git_work_tree(cwd)
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn refuses_home_directory_and_non_git_trees() {
        assert!(!snapshot_scope_is_safe(Path::new("/")));
        if let Some(home) = dirs::home_dir() {
            assert!(!snapshot_scope_is_safe(&home));
            if let Some(parent) = home.parent() {
                assert!(!snapshot_scope_is_safe(parent));
            }
        }

        let root = std::env::temp_dir().join(format!("oxide_snap_scope_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        assert!(!snapshot_scope_is_safe(&root));
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert!(snapshot_scope_is_safe(&root));
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
}
