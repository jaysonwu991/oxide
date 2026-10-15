//! Read-only repository changes for the desktop's Changes pane.

use anyhow::{bail, Context, Result};
use oxide_core::changes::{file_change, summarize, ChangeStatus, TurnChanges};
use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

pub type FileSides = (Option<Vec<u8>>, Option<Vec<u8>>);

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("running git")?;
    if !output.status.success() {
        bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(output.stdout)
}

fn root(project: &Path) -> Result<PathBuf> {
    let view = crate::git::view(project);
    if !view.repo {
        bail!("This project is not in a Git repository.");
    }
    Ok(PathBuf::from(view.root))
}

pub fn changes(project: &Path) -> Result<TurnChanges> {
    let root = root(project)?;
    let status = git(
        &root,
        &[
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=all",
            "--no-renames",
        ],
    )?;
    let mut files = Vec::new();
    let mut seen = HashSet::new();
    for entry in status
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        if entry.len() < 4 {
            bail!("Invalid Git status entry.");
        }
        let path = std::str::from_utf8(&entry[3..]).context("Git path is not UTF-8")?;
        if !seen.insert(path) {
            continue;
        }
        let (old, new) = sides_at(&root, path)?;
        let kind = if old.is_none() {
            ChangeStatus::Added
        } else if new.is_none() {
            ChangeStatus::Deleted
        } else {
            ChangeStatus::Modified
        };
        files.push(file_change(kind, path, old, new));
    }
    Ok(summarize(files))
}

pub fn sides(project: &Path, path: &str) -> Result<FileSides> {
    sides_at(&root(project)?, path)
}

fn sides_at(root: &Path, path: &str) -> Result<FileSides> {
    if path.is_empty()
        || !Path::new(path)
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
    {
        bail!("Invalid repository file path.");
    }
    // Literal pathspecs keep names containing wildcard characters exact.
    let full = root.join(path);
    let spec = format!(":(literal){path}");
    let has_head = git(root, &["rev-parse", "--verify", "HEAD"]).is_ok();
    let entry = if has_head {
        git(root, &["ls-tree", "-z", "HEAD", "--", &spec])?
    } else {
        Vec::new()
    };
    let submodule = entry.starts_with(b"160000 ");
    let old = if submodule {
        Some(vec![0])
    } else if !entry.is_empty() {
        Some(git(root, &["show", &format!("HEAD:{path}")])?)
    } else {
        None
    };
    // Do not follow a symlink (including a parent) outside the repository.
    for parent in Path::new(path)
        .ancestors()
        .skip(1)
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        if root
            .join(parent)
            .symlink_metadata()
            .is_ok_and(|meta| meta.file_type().is_symlink())
        {
            bail!("Cannot read a change through a symbolic link.");
        }
    }
    let new = match full.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => Some(
            std::fs::read_link(&full)?
                .to_string_lossy()
                .as_bytes()
                .to_vec(),
        ),
        Ok(meta) if meta.is_dir() => {
            if submodule || old.is_none() {
                Some(vec![0])
            } else {
                None
            }
        }
        Ok(_) => Some(std::fs::read(&full).with_context(|| format!("reading {path}"))?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("reading {path}")),
    };
    Ok((old, new))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_new_files_before_the_first_commit() {
        let dir = std::env::temp_dir().join(format!("oxide_repo_unborn_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init"]).unwrap();
        std::fs::write(dir.join("new.txt"), "new\n").unwrap();
        git(&dir, &["add", "."]).unwrap();
        let changed = changes(&dir).unwrap();
        assert_eq!(
            (changed.files.len(), changed.added, changed.removed),
            (1, 1, 0)
        );
        assert_eq!(changed.files[0].status, ChangeStatus::Added);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn includes_staged_unstaged_deleted_and_untracked_files() {
        let dir = std::env::temp_dir().join(format!("oxide_repo_changes_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        git(&dir, &["init"]).unwrap();
        for name in ["staged.txt", "unstaged.txt", "deleted.txt"] {
            std::fs::write(dir.join(name), "before\n").unwrap();
        }
        git(&dir, &["add", "."]).unwrap();
        git(
            &dir,
            &[
                "-c",
                "user.name=Test",
                "-c",
                "user.email=test@example.com",
                "commit",
                "-m",
                "base",
            ],
        )
        .unwrap();
        std::fs::write(dir.join("staged.txt"), "after\n").unwrap();
        git(&dir, &["add", "staged.txt"]).unwrap();
        std::fs::write(dir.join("unstaged.txt"), "after\n").unwrap();
        std::fs::remove_file(dir.join("deleted.txt")).unwrap();
        std::fs::write(dir.join("new [file].txt"), "new\n").unwrap();
        let changed = changes(&dir).unwrap();
        assert_eq!(changed.files.len(), 4);
        assert_eq!((changed.added, changed.removed), (3, 3));
        assert!(sides(&dir, "../outside").is_err());
        assert_eq!(sides(&dir, "new [file].txt").unwrap().0, None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
