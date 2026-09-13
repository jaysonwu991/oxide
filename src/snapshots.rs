use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone)]
pub struct Snapshots {
    git_dir: PathBuf,
    work_tree: PathBuf,
}

impl Snapshots {
    pub fn open(cwd: &Path) -> Result<Self> {
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
