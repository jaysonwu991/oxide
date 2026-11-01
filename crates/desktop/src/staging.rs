//! Shaping the working tree before a commit: staging a change, taking one back,
//! and reverting one to what the branch has.
//!
//! Everything here runs the reader's own `git`, in the project's repository, and
//! answers with what git said when it refuses. A hunk is described to this module
//! as the lines the window already holds — a kind, the text, and where the hunk
//! starts on each side — rather than as a patch it hands over: the patch is built
//! here, so a path is a path this module checked and never one a caller spelled.
//!
//! Reverting is the one destructive action: it is offered for a file the branch
//! already has, and refused for one git has never tracked, where "revert" could
//! only mean deleting work the reader never asked to lose.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};

/// One line of a hunk, in the shape the window's diff already has: `add`,
/// `remove`, or `context`.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct HunkLine {
    pub kind: String,
    #[serde(default)]
    pub text: String,
}

/// Where a hunk sits in each side of the file.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct Hunk {
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<HunkLine>,
}

fn git(root: &Path, args: &[&str]) -> Result<Vec<u8>> {
    let output = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .context("running git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("{stderr}");
    }
    Ok(output.stdout)
}

/// Git with a patch on its standard input: what a hunk is applied from.
fn git_patch(root: &Path, args: &[&str], patch: &str) -> Result<()> {
    let mut child = Command::new("git")
        .arg("--no-optional-locks")
        .arg("-C")
        .arg(root)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running git")?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(patch.as_bytes())
            .context("writing the patch")?;
    }
    let output = child.wait_with_output().context("waiting for git")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        bail!("{stderr}");
    }
    Ok(())
}

fn root(project: &Path) -> Result<PathBuf> {
    let view = crate::git::view(project);
    if !view.repo {
        bail!("This project is not in a Git repository.");
    }
    Ok(PathBuf::from(view.root))
}

/// A path as git spells one, checked before it is used: relative, inside the
/// repository, and with no `..` to climb out of it.
fn checked(path: &str) -> Result<&str> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        bail!("`{path}` is not a path inside the repository.");
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) => {}
            _ => bail!("`{path}` is not a path inside the repository."),
        }
    }
    Ok(path)
}

/// What the index holds for one file: whether the change is staged, whether it
/// is still in the working tree, and whether git has ever tracked it.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct FileState {
    pub path: String,
    pub staged: bool,
    pub worktree: bool,
    pub untracked: bool,
}

/// Every file the working tree differs in — or the index does — with the state
/// each is in, so the review pane offers the action that applies rather than
/// every action it knows.
pub fn states(project: &Path) -> Result<Vec<FileState>> {
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
    let mut states = Vec::new();
    for entry in status
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        if entry.len() < 4 {
            bail!("Invalid Git status entry.");
        }
        let index = entry[0];
        let work = entry[1];
        let path = std::str::from_utf8(&entry[3..])
            .context("Git path is not UTF-8")?
            .to_string();
        states.push(FileState {
            path,
            staged: index != b' ' && index != b'?',
            worktree: work != b' ',
            untracked: index == b'?',
        });
    }
    Ok(states)
}

pub fn stage(project: &Path, path: &str) -> Result<()> {
    let root = root(project)?;
    git(&root, &["add", "--", checked(path)?])?;
    Ok(())
}

pub fn unstage(project: &Path, path: &str) -> Result<()> {
    let root = root(project)?;
    // `restore --staged` needs a commit to restore from; a repository with no
    // commit yet has only the index to clear, which `rm --cached` does.
    let path = checked(path)?;
    match git(&root, &["restore", "--staged", "--", path]) {
        Ok(_) => Ok(()),
        Err(error) => {
            if String::from_utf8_lossy(&git(&root, &["rev-parse", "HEAD"]).unwrap_or_default())
                .trim()
                .is_empty()
            {
                git(&root, &["rm", "--cached", "--quiet", "--", path])?;
                return Ok(());
            }
            Err(error)
        }
    }
}

/// Takes the working tree back to what the branch has for one file. A file the
/// branch never had is refused rather than deleted.
pub fn revert(project: &Path, path: &str) -> Result<()> {
    let root = root(project)?;
    let path = checked(path)?;
    let tracked = git(&root, &["ls-files", "--error-unmatch", "--", path]).is_ok();
    if !tracked {
        bail!("`{path}` is not tracked by this branch, so there is nothing to revert it to.");
    }
    git(&root, &["restore", "--worktree", "--", path])?;
    Ok(())
}

pub fn stage_all(project: &Path) -> Result<()> {
    let root = root(project)?;
    git(&root, &["add", "--all"])?;
    Ok(())
}

pub fn unstage_all(project: &Path) -> Result<()> {
    let root = root(project)?;
    if git(&root, &["rev-parse", "--verify", "HEAD"]).is_ok() {
        git(&root, &["restore", "--staged", "--", "."])?;
    } else {
        git(&root, &["rm", "--cached", "-r", "--quiet", "--", "."])?;
    }
    Ok(())
}

/// Reverts every file the branch has, and says nothing about the ones it never
/// had: those are a turn's new files, and losing them is a decision the reader
/// makes a file at a time.
pub fn revert_all(project: &Path) -> Result<()> {
    let root = root(project)?;
    git(&root, &["restore", "--worktree", "--", "."])?;
    Ok(())
}

/// The patch one hunk makes, as git reads it. The counts are left to git:
/// `--recount` rewrites them from the lines, so a hunk cut out of a larger diff
/// still applies as the change it is.
fn hunk_patch(path: &str, hunk: &Hunk) -> Result<String> {
    if hunk.lines.is_empty() {
        bail!("There is nothing in that hunk to apply.");
    }
    let mut patch = String::new();
    patch.push_str(&format!("diff --git a/{path} b/{path}\n"));
    patch.push_str(&format!("--- a/{path}\n+++ b/{path}\n"));
    patch.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        hunk.old_start,
        hunk.lines.iter().filter(|line| line.kind != "add").count(),
        hunk.new_start,
        hunk.lines
            .iter()
            .filter(|line| line.kind != "remove")
            .count(),
    ));
    for line in &hunk.lines {
        let marker = match line.kind.as_str() {
            "add" => "+",
            "remove" => "-",
            "context" => " ",
            other => bail!("`{other}` is not a line of a diff."),
        };
        patch.push_str(marker);
        patch.push_str(&line.text);
        // A patch's lines end where the file does: a line without its newline
        // would swallow the next marker.
        patch.push('\n');
    }
    Ok(patch)
}

/// Stages one hunk of a file, leaving the rest of it in the working tree.
pub fn stage_hunk(project: &Path, path: &str, hunk: &Hunk) -> Result<()> {
    let root = root(project)?;
    let patch = hunk_patch(checked(path)?, hunk)?;
    git_patch(
        &root,
        &["apply", "--cached", "--recount", "--whitespace=nowarn", "-"],
        &patch,
    )
}

/// Stages the parts of a file that are already in the index back out of it,
/// leaving the rest staged.
pub fn unstage_hunk(project: &Path, path: &str, hunk: &Hunk) -> Result<()> {
    let root = root(project)?;
    let patch = hunk_patch(checked(path)?, hunk)?;
    git_patch(
        &root,
        &[
            "apply",
            "--cached",
            "--reverse",
            "--recount",
            "--whitespace=nowarn",
            "-",
        ],
        &patch,
    )
}

/// Takes one hunk out of the working tree, leaving the file's other hunks as they
/// are.
pub fn revert_hunk(project: &Path, path: &str, hunk: &Hunk) -> Result<()> {
    let root = root(project)?;
    let patch = hunk_patch(checked(path)?, hunk)?;
    git_patch(
        &root,
        &[
            "apply",
            "--reverse",
            "--recount",
            "--whitespace=nowarn",
            "-",
        ],
        &patch,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("oxide_stage_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "-q", "-b", "main"],
            vec!["config", "user.email", "t@example.com"],
            vec!["config", "user.name", "T"],
            // The tests compare the bytes git writes back, and a Windows runner
            // checks files out with CRLF unless the repository says otherwise:
            // the checkout policy is the machine's, so the fixture pins it.
            vec!["config", "core.autocrlf", "false"],
        ] {
            assert!(Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(&args)
                .status()
                .unwrap()
                .success());
        }
        std::fs::write(root.join("notes.md"), "one\ntwo\nthree\n").unwrap();
        for args in [vec!["add", "--all"], vec!["commit", "-q", "-m", "first"]] {
            assert!(Command::new("git")
                .arg("-C")
                .arg(&root)
                .args(&args)
                .status()
                .unwrap()
                .success());
        }
        root
    }

    fn status(root: &Path) -> String {
        String::from_utf8(git(root, &["status", "--porcelain=v1"]).unwrap()).unwrap()
    }

    /// A file's contents as the branch holds them: the line endings a checkout
    /// writes are the machine's, so a comparison of what git restored is made
    /// one way on every platform.
    fn body(path: &Path) -> String {
        std::fs::read_to_string(path).unwrap().replace("\r\n", "\n")
    }

    #[test]
    fn staging_a_file_puts_it_in_the_index() {
        let root = repo("file");
        std::fs::write(root.join("notes.md"), "one\nchanged\nthree\n").unwrap();
        assert!(
            status(&root).starts_with(" M notes.md"),
            "{}",
            status(&root)
        );
        stage(&root, "notes.md").unwrap();
        assert!(
            status(&root).starts_with("M  notes.md"),
            "{}",
            status(&root)
        );
        unstage(&root, "notes.md").unwrap();
        assert!(
            status(&root).starts_with(" M notes.md"),
            "{}",
            status(&root)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn staging_everything_takes_the_new_files_too() {
        let root = repo("all");
        std::fs::write(root.join("notes.md"), "changed\n").unwrap();
        std::fs::write(root.join("fresh.md"), "new\n").unwrap();
        stage_all(&root).unwrap();
        let staged = status(&root);
        assert!(staged.contains("M  notes.md"), "{staged}");
        assert!(staged.contains("A  fresh.md"), "{staged}");
        unstage_all(&root).unwrap();
        let cleared = status(&root);
        assert!(cleared.contains(" M notes.md"), "{cleared}");
        assert!(cleared.contains("?? fresh.md"), "{cleared}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reverting_a_file_puts_the_branchs_copy_back() {
        let root = repo("revert");
        std::fs::write(root.join("notes.md"), "one\ntwo\nthree\nfour\n").unwrap();
        revert(&root, "notes.md").unwrap();
        assert_eq!(body(&root.join("notes.md")), "one\ntwo\nthree\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_the_branch_never_had_is_not_reverted_away() {
        let root = repo("untracked");
        std::fs::write(root.join("fresh.md"), "new\n").unwrap();
        let refused = revert(&root, "fresh.md").unwrap_err().to_string();
        assert!(refused.contains("not tracked"), "{refused}");
        assert!(root.join("fresh.md").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reverting_everything_leaves_the_new_files_alone() {
        let root = repo("revert-all");
        std::fs::write(root.join("notes.md"), "changed\n").unwrap();
        std::fs::write(root.join("fresh.md"), "new\n").unwrap();
        revert_all(&root).unwrap();
        assert_eq!(body(&root.join("notes.md")), "one\ntwo\nthree\n");
        assert!(root.join("fresh.md").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_path_that_climbs_out_of_the_repository_is_refused() {
        let root = repo("escape");
        for path in ["../outside.md", "/etc/passwd", "a/../../b.md"] {
            let refused = stage(&root, path).unwrap_err().to_string();
            assert!(refused.contains("not a path inside"), "{path}: {refused}");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    fn hunk(against: &str) -> Hunk {
        // The hunk the change makes against `one\ntwo\nthree\n`.
        let _ = against;
        Hunk {
            old_start: 1,
            new_start: 1,
            lines: vec![
                HunkLine {
                    kind: "context".into(),
                    text: "one".into(),
                },
                HunkLine {
                    kind: "remove".into(),
                    text: "two".into(),
                },
                HunkLine {
                    kind: "add".into(),
                    text: "changed".into(),
                },
                HunkLine {
                    kind: "context".into(),
                    text: "three".into(),
                },
            ],
        }
    }

    #[test]
    fn staging_one_hunk_stages_only_that_hunk() {
        let root = repo("hunk");
        // Two hunks of one file: line 2 and the last line.
        std::fs::write(root.join("notes.md"), "one\nchanged\nthree\nfour\n").unwrap();
        stage_hunk(&root, "notes.md", &hunk("")).unwrap();
        let staged =
            String::from_utf8(git(&root, &["diff", "--cached", "--unified=0"]).unwrap()).unwrap();
        assert!(staged.contains("+changed"), "{staged}");
        assert!(!staged.contains("+four"), "{staged}");
        // The line the hunk did not name is still a change in the working tree.
        let working = String::from_utf8(git(&root, &["diff"]).unwrap()).unwrap();
        assert!(working.contains("+four"), "{working}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn taking_a_hunk_back_out_of_the_index_leaves_it_working() {
        let root = repo("unstage-hunk");
        // Two hunks of one file, so staging one leaves the file half-staged: the
        // index holds that hunk and the working tree holds both.
        std::fs::write(root.join("notes.md"), "one\nchanged\nthree\nfour\n").unwrap();
        stage_hunk(&root, "notes.md", &hunk("")).unwrap();
        assert!(
            status(&root).starts_with("MM notes.md"),
            "{}",
            status(&root)
        );
        unstage_hunk(&root, "notes.md", &hunk("")).unwrap();
        assert!(
            status(&root).starts_with(" M notes.md"),
            "{}",
            status(&root)
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn reverting_one_hunk_puts_that_hunk_back() {
        let root = repo("revert-hunk");
        std::fs::write(root.join("notes.md"), "one\nchanged\nthree\nfour\n").unwrap();
        revert_hunk(&root, "notes.md", &hunk("")).unwrap();
        let body = std::fs::read_to_string(root.join("notes.md")).unwrap();
        assert!(body.contains("two"), "{body}");
        assert!(!body.contains("changed"), "{body}");
        assert!(body.contains("four"), "{body}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_states_say_which_half_of_a_file_is_where() {
        let root = repo("states");
        std::fs::write(root.join("notes.md"), "one\nchanged\nthree\nfour\n").unwrap();
        std::fs::write(root.join("fresh.md"), "new\n").unwrap();
        stage(&root, "notes.md").unwrap();
        std::fs::write(root.join("notes.md"), "one\nchanged\nthree\nfour\nfive\n").unwrap();

        let states = states(&root).unwrap();
        let notes = states
            .iter()
            .find(|state| state.path == "notes.md")
            .unwrap();
        assert!(notes.staged && notes.worktree, "{notes:?}");
        let fresh = states
            .iter()
            .find(|state| state.path == "fresh.md")
            .unwrap();
        assert!(
            fresh.untracked && fresh.worktree && !fresh.staged,
            "{fresh:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_empty_hunk_is_refused_rather_than_applied() {
        let root = repo("empty-hunk");
        let empty = Hunk {
            old_start: 1,
            new_start: 1,
            lines: Vec::new(),
        };
        let refused = stage_hunk(&root, "notes.md", &empty)
            .unwrap_err()
            .to_string();
        assert!(refused.contains("nothing in that hunk"), "{refused}");
        let _ = std::fs::remove_dir_all(&root);
    }
}
