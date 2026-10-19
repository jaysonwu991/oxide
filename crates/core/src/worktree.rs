//! Git worktrees: a thread's own checkout of a project.
//!
//! A worktree is where a conversation works when its changes should not land in
//! the folder the reader has open — the run edits, builds and tests a complete
//! copy of the repository while the checkout it was made from stays as it was.
//! Each one is a real `git worktree`, detached or on a named branch. A chat can
//! move between its managed checkout and Local while retaining its history,
//! staged state and uncommitted files.
//!
//! The checkouts live out of the project's way, under the configuration
//! directory (`<config>/Oxide/worktrees/<project key>/<name>`), keyed by where
//! the project sits rather than by the remote it is a clone of — a worktree is
//! files, and the files of two clones are not the same files. Keeping them there
//! is what leaves the project's own `git status` alone: a worktree inside the
//! folder would appear in every listing and in every diff the reader looks at.
//! A registry beside them records the name, branch, project and creation time,
//! while the branch a worktree is on right now is read from its own `HEAD`, the
//! way the composer's branch row reads a folder's.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

/// The prefix every worktree branch carries, so a branch Oxide made is never
/// mistaken for one the reader named — and so `remove` knows what it may take.
pub const BRANCH_PREFIX: &str = "oxide/";

/// How long a generated worktree name may run before it is cut at a dash. Long
/// enough to read as the task it was named after, short enough that the branch
/// and the folder stay usable in a terminal.
const NAME_LIMIT: usize = 40;

/// One worktree of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    /// The name it was created under, which is also its folder's name.
    pub name: String,
    /// The branch it is on now, read from its own `HEAD`.
    pub branch: String,
    /// The checkout, absolute.
    pub path: String,
    /// The project it is a worktree of, absolute.
    pub project: String,
    #[serde(default)]
    pub created_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
}

impl Worktree {
    pub fn exists(&self) -> bool {
        Path::new(&self.path).is_dir()
    }

    /// Whether `cwd` is this worktree or inside it, compared by path component
    /// so `.../fix-bug-old` is not read as `.../fix-bug`.
    pub fn holds(&self, cwd: &Path) -> bool {
        cwd.starts_with(Path::new(&self.path))
    }
}

/// The worktrees this machine holds, rooted at one directory: the real one
/// (`<config>/Oxide/worktrees`) for every caller, and a caller's own for a test
/// that wants them beside its fixture.
#[derive(Debug, Clone)]
pub struct Worktrees {
    root: PathBuf,
}

impl Default for Worktrees {
    fn default() -> Self {
        Self::open()
    }
}

impl Worktrees {
    /// The registry every front-end reads: the terminal's `oxide worktree`, the
    /// desktop app and the VS Code panel all create and list the same folders.
    pub fn open() -> Self {
        Self::at(crate::config::config_dir_or_default().join("worktrees"))
    }

    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The folder a project's worktrees are kept in: one per project, keyed by
    /// where the project sits.
    fn dir_of(&self, project: &Path) -> PathBuf {
        self.root.join(crate::memory::local_project_id(project))
    }

    fn registry(&self, project: &Path) -> PathBuf {
        self.dir_of(project).join("worktrees.json")
    }

    /// The worktrees of `project`, newest first. A worktree whose folder is gone
    /// — removed by hand, or by `git worktree remove` in a terminal — is left
    /// out rather than listed as something to open.
    pub fn list(&self, project: &Path) -> Vec<Worktree> {
        let mut entries = self.read_registry(project);
        for entry in &mut entries {
            if let Some(branch) = current_branch(Path::new(&entry.path)) {
                entry.branch = branch;
            }
        }
        entries.retain(Worktree::exists);
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.created_at));
        entries
    }

    pub fn find(&self, project: &Path, name: &str) -> Option<Worktree> {
        self.list(project)
            .into_iter()
            .find(|entry| entry.name == name)
    }

    pub fn create_branch(&self, project: &Path, name: &str, branch: &str) -> Result<Worktree> {
        let mut entry = self
            .find(project, name)
            .context("worktree no longer exists")?;
        let branch = branch.trim();
        git(
            Path::new(&entry.path),
            &["check-ref-format", "--branch", branch],
        )?;
        git(Path::new(&entry.path), &["switch", "-c", branch])?;
        entry.branch = branch.to_string();
        Ok(entry)
    }

    /// Move a stored chat and its Git state together, returning its new cwd.
    pub fn handoff(
        &self,
        project: &Path,
        source: &Path,
        id: &str,
        to_worktree: bool,
    ) -> Result<PathBuf> {
        let project = repo_root(project)?;
        let source = canonical(source);
        let mut log = crate::session::SessionLog::open_id(&source, id)?;
        if canonical(Path::new(log.cwd())) != source {
            bail!("this chat belongs to another checkout; reopen it before handing off");
        }
        let mut entries = self.read_registry(&project);
        let entry = if to_worktree {
            if source != project {
                bail!("this chat is already in a worktree");
            }
            match entries
                .iter()
                .find(|entry| entry.session.as_deref() == Some(id) && entry.exists())
            {
                Some(entry) => entry.clone(),
                None => self.create_isolated(&project, None, None, true, Some(&source), false)?,
            }
        } else {
            entries
                .iter()
                .find(|entry| canonical(Path::new(&entry.path)) == source)
                .cloned()
                .context("this chat is not in a managed worktree")?
        };
        entries = self.read_registry(&project);
        if let Some(bound) = entries.iter_mut().find(|each| each.path == entry.path) {
            if bound
                .session
                .as_deref()
                .is_some_and(|session| session != id)
            {
                bail!("this worktree is associated with another chat");
            }
            bound.session = Some(id.to_string());
        }
        write_registry(&self.registry(&project), &entries)?;
        let destination = if to_worktree {
            PathBuf::from(entry.path)
        } else {
            project
        };
        move_checkout(&source, &destination, || log.relocate(&destination))?;
        Ok(destination)
    }

    /// The worktree `cwd` is in, if any: a folder inside one belongs to it the
    /// way a folder inside a clone belongs to that clone.
    pub fn view(&self, cwd: &Path) -> Option<Worktree> {
        let relative = cwd.strip_prefix(&self.root).ok()?;
        let mut parts = relative.components();
        let (Some(Component::Normal(key)), Some(Component::Normal(name))) =
            (parts.next(), parts.next())
        else {
            return None;
        };
        let folder = self.root.join(key).join(name);
        let project_dir = folder.parent()?;
        let entries = read_registry_at(&project_dir.join("worktrees.json"));
        let canonical = folder.canonicalize().unwrap_or(folder);
        entries.into_iter().find(|entry| {
            Path::new(&entry.path)
                .canonicalize()
                .unwrap_or_else(|_| PathBuf::from(&entry.path))
                == canonical
        })
    }

    /// The project a folder belongs to, which for a worktree is the checkout it
    /// was made from: trust, the change listing and the project's own resources
    /// are the project's, not the copy's.
    pub fn project_of(&self, cwd: &Path) -> Option<PathBuf> {
        self.view(cwd).map(|entry| PathBuf::from(entry.project))
    }

    /// The folder a run's project resources are resolved against: the project a
    /// worktree was made from, or the folder itself when it is not one.
    pub fn trust_root(cwd: &Path) -> PathBuf {
        Self::open()
            .project_of(cwd)
            .unwrap_or_else(|| cwd.to_path_buf())
    }

    /// Creates a worktree of `project` named `name`, or the first free name
    /// derived from it. `base` is the commit or branch to start from, `HEAD`
    /// when nothing is named.
    ///
    /// A name that is already taken is an error: the reader asked for that one,
    /// and silently handing back another worktree is not the thing they asked
    /// for. [`ensure`](Self::ensure) is the create-or-reuse the terminal's
    /// `--worktree` wants.
    pub fn create(
        &self,
        project: &Path,
        name: Option<&str>,
        base: Option<&str>,
    ) -> Result<Worktree> {
        let project = canonical(project);
        let repo = repo_root(&project)?;
        let start = match base {
            Some(base) => base.to_string(),
            None => git(&repo, &["rev-parse", "--verify", "HEAD"])
                .context("reading the commit to branch from")?,
        };
        let wanted = name.map(slug).filter(|name| !name.is_empty());
        if let Some(name) = &wanted {
            if self.find(&project, name).is_some() {
                bail!("a worktree named `{name}` already exists");
            }
        }
        self.make(&project, &repo, wanted.as_deref(), &start, false, true)
    }

    /// Desktop checkouts may stay detached until the reader names a branch.
    pub fn create_isolated(
        &self,
        project: &Path,
        name: Option<&str>,
        base: Option<&str>,
        detached: bool,
        source: Option<&Path>,
        copy_changes: bool,
    ) -> Result<Worktree> {
        let project = canonical(project);
        let repo = repo_root(&project)?;
        let source = source.unwrap_or(&repo);
        // A source must belong to this repository, including another worktree.
        let common = |path: &Path| -> Result<PathBuf> {
            let dir = git(path, &["rev-parse", "--git-common-dir"])?;
            Ok(canonical(&path.join(dir)))
        };
        if common(source)? != common(&repo)? {
            bail!("the source checkout belongs to another repository");
        }
        let start = git(
            source,
            &[
                "rev-parse",
                "--verify",
                "--end-of-options",
                &format!("{}^{{commit}}", base.unwrap_or("HEAD")),
            ],
        )?;
        let wanted = name.map(slug).filter(|name| !name.is_empty());
        let entry = self.make(&project, &repo, wanted.as_deref(), &start, detached, false)?;
        let copied = (|| -> Result<()> {
            copy_worktree_includes(source, Path::new(&entry.path))?;
            if copy_changes {
                copy_local_changes(source, Path::new(&entry.path))?;
            }
            Ok(())
        })();
        if let Err(error) = copied {
            // Only this newly created checkout is rolled back; the source
            // remains untouched even when applying to another base fails.
            let _ = git(&repo, &["worktree", "remove", "--force", &entry.path]);
            let mut entries = self.read_registry(&project);
            entries.retain(|item| item.path != entry.path);
            write_registry(&self.registry(&project), &entries)?;
            return Err(error);
        }
        Ok(entry)
    }

    /// Creates the worktree, or hands back the one already holding that name:
    /// the terminal's `--worktree=fix-bug` opens the same checkout twice rather
    /// than refusing the second run.
    pub fn ensure(
        &self,
        project: &Path,
        name: Option<&str>,
        base: Option<&str>,
    ) -> Result<Worktree> {
        let canonical = canonical(project);
        if let Some(name) = name.map(slug).filter(|name| !name.is_empty()) {
            if let Some(found) = self.find(&canonical, &name) {
                return Ok(found);
            }
        }
        self.create(&canonical, name, base)
    }

    fn make(
        &self,
        project: &Path,
        repo: &Path,
        wanted: Option<&str>,
        start: &str,
        detached: bool,
        allow_reuse: bool,
    ) -> Result<Worktree> {
        let dir = self.dir_of(project);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let stem = wanted.unwrap_or("worktree");
        let mut entries = self.read_registry(project);
        // A worktree removed by hand leaves its registry entry behind; the name
        // it held is free again, since the folder it named is not there.
        entries.retain(Worktree::exists);
        // A name is taken by a registered worktree, by a folder of its own or by
        // a branch the repository already holds and has checked out somewhere: a
        // branch can only be checked out in one place, so the next name is tried
        // rather than a checkout that git would refuse.
        let mut chosen = None;
        for round in 0..64 {
            let name = if round == 0 {
                stem.to_string()
            } else {
                format!("{stem}-{}", round + 1)
            };
            let path = dir.join(&name);
            let branch = format!("{BRANCH_PREFIX}{name}");
            if entries.iter().any(|entry| entry.name == name)
                || path.exists()
                || (!detached && branch_checked_out(repo, &branch))
                || (!detached
                    && !allow_reuse
                    && git(
                        repo,
                        &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
                    )
                    .is_ok())
            {
                if wanted.is_some() {
                    bail!("a worktree named `{name}` already exists");
                }
                continue;
            }
            chosen = Some((name, path, branch));
            break;
        }
        let Some((name, path, branch)) = chosen else {
            bail!("no free worktree name for `{stem}`");
        };

        let path_text = path.display().to_string();
        // A branch left behind by a worktree that was removed is resumed rather
        // than refused: its commits are still the work that was started there.
        let reuse = !detached
            && git(
                repo,
                &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
            )
            .is_ok();
        let mut command = Command::new("git");
        command
            .current_dir(repo)
            .arg("--no-optional-locks")
            .args(["worktree", "add"]);
        if detached {
            command.arg("--detach");
        } else if !reuse {
            command.arg("-b").arg(&branch);
        }
        command.arg(&path_text);
        if reuse {
            command.arg(&branch);
        } else {
            command.arg(start);
        }
        let output = command.output().context("running git worktree add")?;
        if !output.status.success() {
            bail!("{}", git_error(&output.stderr));
        }

        let entry = Worktree {
            name,
            branch: if detached {
                start[..7].to_string()
            } else {
                branch
            },
            path: path_text,
            // The repository rather than the folder the command was run from: a
            // worktree made from a subdirectory is that repository's, and the
            // project its trust and its threads are resolved against is the
            // checkout a reader would name.
            project: repo.display().to_string(),
            created_at: now_secs(),
            session: None,
        };
        entries.push(entry.clone());
        write_registry(&self.registry(project), &entries)?;
        Ok(entry)
    }

    /// Removes a worktree's checkout and forgets it. Its branch stays in the
    /// repository: the commits are the work that was started there, and reading
    /// the folder back through [`create`](Self::create) with the same name
    /// resumes it rather than starting over.
    pub fn remove(&self, project: &Path, name: &str) -> Result<Worktree> {
        let project = canonical(project);
        let Some(mut entry) = self.find(&project, name) else {
            bail!("no worktree named `{name}`");
        };
        if Path::new(&entry.path).is_dir() {
            let repo = repo_root(&project)?;
            let checkout = Path::new(&entry.path);
            if !git(
                checkout,
                &["status", "--porcelain", "--untracked-files=all"],
            )?
            .is_empty()
            {
                bail!("worktree has uncommitted changes; commit or move them before removing it");
            }
            // Detached commits need a ref before their checkout disappears.
            if git(checkout, &["symbolic-ref", "--quiet", "HEAD"]).is_err() {
                let head = git(checkout, &["rev-parse", "HEAD"])?;
                let mut branch = format!("{BRANCH_PREFIX}{}", entry.name);
                if let Ok(existing) = git(
                    &repo,
                    &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
                ) {
                    if existing != head {
                        branch = format!(
                            "{branch}-{}",
                            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
                        );
                    }
                }
                if git(
                    &repo,
                    &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
                )
                .is_err()
                {
                    git(&repo, &["branch", &branch, &head])?;
                }
                entry.branch = branch;
            }
            let output = Command::new("git")
                .current_dir(&repo)
                .arg("--no-optional-locks")
                .args(["worktree", "remove", &entry.path])
                .output()
                .context("running git worktree remove")?;
            if !output.status.success() {
                bail!("{}", git_error(&output.stderr));
            }
        }
        let mut entries = self.read_registry(&project);
        entries.retain(|each| each.name != entry.name);
        write_registry(&self.registry(&project), &entries)?;
        Ok(entry)
    }

    fn read_registry(&self, project: &Path) -> Vec<Worktree> {
        read_registry_at(&self.registry(project))
    }
}

/// A one-line name for a worktree, from the task it was started for: `Fix the
/// login bug` is `fix-the-login-bug`. Anything that is not a letter or a digit
/// separates words, since a branch may hold only the shapes git accepts, and a
/// name that comes out empty is left to the caller to supply.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() {
            out.push(character.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_matches('-');
    if trimmed.len() <= NAME_LIMIT {
        return trimmed.to_string();
    }
    let cut = &trimmed[..NAME_LIMIT];
    cut.trim_end_matches('-').to_string()
}

/// The repository `project` belongs to.
fn repo_root(project: &Path) -> Result<PathBuf> {
    let output = Command::new("git")
        .current_dir(project)
        .arg("--no-optional-locks")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .context("running git")?;
    if !output.status.success() {
        bail!(
            "{} is not in a git repository, so there is nothing to make a worktree of",
            project.display()
        );
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        bail!("{} is not in a git repository", project.display());
    }
    Ok(canonical(Path::new(&text)))
}

fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .current_dir(cwd)
        .arg("--no-optional-locks")
        .args(args)
        .output()
        .context("running git")?;
    if !output.status.success() {
        bail!("{}", git_error(&output.stderr));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn copy_local_changes(source: &Path, destination: &Path) -> Result<()> {
    use std::io::Write;
    use std::process::Stdio;

    let patch = Command::new("git")
        .current_dir(source)
        .args(["diff", "--binary", "--no-ext-diff", "HEAD", "--"])
        .output()
        .context("reading uncommitted changes")?;
    if !patch.status.success() {
        bail!("{}", git_error(&patch.stderr));
    }
    if !patch.stdout.is_empty() {
        let mut apply = Command::new("git")
            .current_dir(destination)
            .args(["apply", "--binary", "--whitespace=nowarn", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("applying uncommitted changes")?;
        let written = apply
            .stdin
            .take()
            .context("opening git apply input")?
            .write_all(&patch.stdout);
        let output = apply.wait_with_output()?;
        if !output.status.success() {
            bail!("{}", git_error(&output.stderr));
        }
        written?;
    }
    let untracked = Command::new("git")
        .current_dir(source)
        .args(["ls-files", "--others", "--exclude-standard", "-z"])
        .output()?;
    if !untracked.status.success() {
        bail!("{}", git_error(&untracked.stderr));
    }
    for path in untracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
    {
        let relative =
            Path::new(std::str::from_utf8(path).context("untracked filename is not UTF-8")?);
        if relative.is_absolute()
            || relative
                .components()
                .any(|part| !matches!(part, Component::Normal(_)))
        {
            bail!("invalid untracked path");
        }
        let from = source.join(relative);
        let to = destination.join(relative);
        for root in [source, destination] {
            let mut ancestor = root.to_path_buf();
            for part in relative.components() {
                ancestor.push(part);
                if ancestor
                    .symlink_metadata()
                    .is_ok_and(|meta| meta.file_type().is_symlink())
                {
                    bail!("cannot copy through symlink {}", ancestor.display());
                }
            }
        }
        if from.symlink_metadata()?.file_type().is_symlink() {
            bail!("cannot copy untracked symlink {}", relative.display());
        }
        if to.symlink_metadata().is_ok() {
            bail!(
                "untracked file {} already exists in the starting commit",
                relative.display()
            );
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::copy(&from, &to).with_context(|| format!("copying {}", relative.display()))?;
    }
    Ok(())
}

fn copy_worktree_includes(source: &Path, destination: &Path) -> Result<()> {
    let ignored = git_paths(
        source,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let includes = source.join(".worktreeinclude");
    let matched = if includes.is_file() && !includes.symlink_metadata()?.file_type().is_symlink() {
        git_paths(
            source,
            &[
                "ls-files",
                "--others",
                "--ignored",
                &format!("--exclude-from={}", includes.display()),
                "-z",
            ],
        )?
    } else {
        Vec::new()
    };
    for path in ignored {
        if path != "AGENTS.override.md" && !matched.contains(&path) {
            continue;
        }
        let relative = Path::new(&path);
        let mut safe = true;
        for root in [source, destination] {
            let mut ancestor = root.to_path_buf();
            for part in relative.components() {
                if !matches!(part, Component::Normal(_)) {
                    safe = false;
                    break;
                }
                ancestor.push(part);
                if ancestor
                    .symlink_metadata()
                    .is_ok_and(|meta| meta.file_type().is_symlink())
                {
                    safe = false;
                    break;
                }
            }
        }
        let from = source.join(relative);
        let to = destination.join(relative);
        if !safe || to.exists() || !from.is_file() {
            continue;
        }
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut input = std::fs::File::open(&from)?;
        let mut output = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&to)?;
        std::io::copy(&mut input, &mut output)?;
        std::fs::set_permissions(&to, std::fs::metadata(&from)?.permissions())?;
    }
    Ok(())
}

fn move_checkout(
    source: &Path,
    destination: &Path,
    finalize: impl FnOnce() -> Result<()>,
) -> Result<()> {
    if !git(
        destination,
        &["status", "--porcelain", "--untracked-files=all"],
    )?
    .is_empty()
    {
        bail!("destination has uncommitted changes; commit or move them before handing off");
    }
    let head = git(source, &["rev-parse", "HEAD"])?;
    let staged = git(source, &["write-tree"])?;
    let branch = git(source, &["symbolic-ref", "--quiet", "HEAD"]).ok();
    let old_head = git(destination, &["rev-parse", "HEAD"])?;
    let old_branch = git(destination, &["symbolic-ref", "--quiet", "HEAD"]).ok();
    let snapshot = snapshot_checkout(source, &head)?;
    // Retain recovery refs even after success, including a destination that was
    // detached. Moving a checkout must never strand its previous commits.
    let recovery = format!(
        "refs/oxide/handoffs/{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
        std::process::id()
    );
    git(
        source,
        &["update-ref", &format!("{recovery}/source"), &snapshot],
    )?;
    git(
        destination,
        &["update-ref", &format!("{recovery}/destination"), &old_head],
    )?;
    let ignored = git_paths(
        destination,
        &[
            "ls-files",
            "--others",
            "--ignored",
            "--exclude-standard",
            "-z",
        ],
    )?;
    let incoming = git_paths(source, &["ls-tree", "-r", "--name-only", "-z", &snapshot])?;
    for path in &ignored {
        if incoming.iter().any(|file| {
            file == path
                || file.starts_with(&format!("{path}/"))
                || path.starts_with(&format!("{file}/"))
        }) {
            bail!("ignored destination file {path} would be overwritten");
        }
    }
    let mut destination_changed = false;
    let mut source_changed = false;
    let result = (|| -> Result<()> {
        git(destination, &["update-ref", "--no-deref", "HEAD", &head])?;
        destination_changed = true;
        git(destination, &["read-tree", "--reset", "-u", &snapshot])?;
        git(destination, &["read-tree", &staged])?;
        git(source, &["update-ref", "--no-deref", "HEAD", &head])?;
        source_changed = true;
        // First mark all transferred files in the index, then return the source
        // to its commit. Only the transferred untracked files are removed.
        git(source, &["read-tree", &snapshot])?;
        git(source, &["read-tree", "--reset", "-u", &head])?;
        if let Some(branch) = &branch {
            git(destination, &["symbolic-ref", "HEAD", branch])?;
        }
        finalize()
    })();
    if let Err(error) = result {
        let rollback = (|| -> Result<()> {
            if destination_changed {
                git(
                    destination,
                    &["update-ref", "--no-deref", "HEAD", &old_head],
                )?;
                git(destination, &["read-tree", &snapshot])?;
                git(destination, &["read-tree", "--reset", "-u", &old_head])?;
                if let Some(branch) = &old_branch {
                    git(destination, &["symbolic-ref", "HEAD", branch])?;
                }
            }
            if source_changed {
                git(source, &["update-ref", "--no-deref", "HEAD", &head])?;
                git(source, &["read-tree", "--reset", "-u", &snapshot])?;
                git(source, &["read-tree", &staged])?;
                if let Some(branch) = &branch {
                    git(source, &["symbolic-ref", "HEAD", branch])?;
                }
            }
            Ok(())
        })();
        return match rollback {
            Ok(()) => Err(error.context("handoff failed; both checkouts restored")),
            Err(rollback) => Err(error.context(format!(
                "rollback failed: {rollback}; recovery is at {recovery}"
            ))),
        };
    }
    Ok(())
}

fn git_paths(cwd: &Path, args: &[&str]) -> Result<Vec<String>> {
    let output = Command::new("git").current_dir(cwd).args(args).output()?;
    if !output.status.success() {
        bail!("{}", git_error(&output.stderr));
    }
    output
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            Ok(std::str::from_utf8(path)
                .context("filename is not UTF-8")?
                .to_string())
        })
        .collect()
}

fn snapshot_checkout(cwd: &Path, head: &str) -> Result<String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let unique = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let folder =
        std::env::temp_dir().join(format!("oxide-handoff-{}-{unique}", std::process::id()));
    std::fs::create_dir(&folder)?;
    let index = folder.join("index");
    let run = |args: &[&str]| -> Result<String> {
        let output = Command::new("git")
            .current_dir(cwd)
            .env("GIT_INDEX_FILE", &index)
            .args(args)
            .output()?;
        if !output.status.success() {
            bail!("{}", git_error(&output.stderr));
        }
        Ok(String::from_utf8(output.stdout)?.trim().to_string())
    };
    let result = (|| {
        run(&["read-tree", head])?;
        run(&["add", "--all", "--", "."])?;
        let tree = run(&["write-tree"])?;
        run(&[
            "-c",
            "user.name=Oxide",
            "-c",
            "user.email=oxide@localhost",
            "commit-tree",
            &tree,
            "-p",
            head,
            "-m",
            "Oxide handoff recovery",
        ])
    })();
    let _ = std::fs::remove_dir_all(folder);
    result
}

fn git_error(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.is_empty() {
        "git failed".to_string()
    } else {
        text.to_string()
    }
}

/// Whether `branch` is checked out in the repository — here or in another of its
/// worktrees. Git refuses to add a worktree on a branch that is already in one.
fn branch_checked_out(repo: &Path, branch: &str) -> bool {
    let Ok(listing) = git(repo, &["worktree", "list", "--porcelain"]) else {
        return false;
    };
    listing
        .lines()
        .any(|line| line.trim() == format!("branch refs/heads/{branch}"))
}

/// The branch a worktree is on, read from the `HEAD` its `.git` file points at —
/// the read `crate::git` makes for a folder, applied to a checkout whose `.git`
/// is always a file.
fn current_branch(worktree: &Path) -> Option<String> {
    let pointer = std::fs::read_to_string(worktree.join(".git")).ok()?;
    let git_dir = pointer
        .lines()
        .find_map(|line| line.trim().strip_prefix("gitdir:"))?
        .trim();
    if git_dir.is_empty() {
        return None;
    }
    let git_dir = if Path::new(git_dir).is_absolute() {
        PathBuf::from(git_dir)
    } else {
        worktree.join(git_dir)
    };
    let head = std::fs::read_to_string(git_dir.join("HEAD")).ok()?;
    let Some(name) = head.trim().strip_prefix("ref:") else {
        return Some(head.trim().chars().take(7).collect());
    };
    let name = name.trim();
    let name = name.strip_prefix("refs/heads/").unwrap_or(name);
    (!name.is_empty()).then(|| name.to_string())
}

fn read_registry_at(path: &Path) -> Vec<Worktree> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    serde_json::from_str(&text).unwrap_or_default()
}

fn write_registry(path: &Path, entries: &[Worktree]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(entries)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

fn canonical(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A git repository with one commit and a file in it, so a worktree has
    /// something to branch from.
    fn repo(name: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("oxide_worktree_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        for args in [
            vec!["init", "--quiet"],
            vec!["config", "user.email", "oxide@example.com"],
            vec!["config", "user.name", "Oxide"],
            vec!["config", "commit.gpgsign", "false"],
        ] {
            git(&root, &args).unwrap();
        }
        std::fs::write(root.join("README.md"), "hello\n").unwrap();
        git(&root, &["add", "README.md"]).unwrap();
        git(&root, &["commit", "-m", "first", "--quiet"]).unwrap();
        root
    }

    fn worktrees(name: &str) -> (PathBuf, Worktrees) {
        let project = repo(name);
        let root = project.parent().unwrap().join(format!(
            "{}_worktrees",
            project.file_name().unwrap().to_string_lossy()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = Worktrees::at(root);
        (project, store)
    }

    /// Removes what one test made and nothing else: the repository and the
    /// worktrees beside it. The repository's parent is the machine's temp
    /// directory, shared with every other test and every other program, so it is
    /// never the thing to delete.
    fn cleanup(project: &Path) {
        let _ = std::fs::remove_dir_all(project);
        if let Some(name) = project.file_name() {
            let sibling = project.with_file_name(format!("{}_worktrees", name.to_string_lossy()));
            let _ = std::fs::remove_dir_all(sibling);
        }
    }

    #[test]
    fn a_name_comes_from_the_task_it_names() {
        assert_eq!(slug("Fix the login bug"), "fix-the-login-bug");
        assert_eq!(slug("  spaced   out  "), "spaced-out");
        assert_eq!(
            slug("feat/auth: retry & backoff"),
            "feat-auth-retry-backoff"
        );
        assert_eq!(slug(""), "");
        assert_eq!(slug("!!!"), "");
    }

    #[test]
    fn a_long_name_is_cut_at_a_word_within_the_limit() {
        let name = slug("a very long description of a change that runs past the limit");
        assert!(name.len() <= NAME_LIMIT, "{name}");
        assert!(!name.ends_with('-'));
        assert!(name.starts_with("a-very-long-description"));
    }

    #[test]
    fn isolated_named_creation_does_not_silently_resume_an_old_branch() {
        let (project, store) = worktrees("isolated-old-branch");
        store.create(&project, Some("old"), None).unwrap();
        store.remove(&project, "old").unwrap();
        assert!(store
            .create_isolated(&project, Some("old"), Some("HEAD"), false, None, false)
            .is_err());
        cleanup(&project);
    }

    #[test]
    fn setup_files_use_worktreeinclude_patterns_and_skip_unlisted_ignored_files() {
        let (project, store) = worktrees("include");
        std::fs::write(project.join(".gitignore"), ".env*\nAGENTS.override.md\n").unwrap();
        std::fs::write(project.join(".worktreeinclude"), ".env*\n!.env.secret\n").unwrap();
        std::fs::write(project.join(".env.local"), "setup\n").unwrap();
        std::fs::write(project.join(".env.secret"), "stay here\n").unwrap();
        std::fs::write(project.join("AGENTS.override.md"), "instructions\n").unwrap();
        git(&project, &["add", ".gitignore", ".worktreeinclude"]).unwrap();
        git(&project, &["commit", "-qm", "setup rules"]).unwrap();
        let entry = store
            .create_isolated(&project, Some("setup"), None, true, None, false)
            .unwrap();
        let checkout = Path::new(&entry.path);
        assert_eq!(
            std::fs::read_to_string(checkout.join(".env.local")).unwrap(),
            "setup\n"
        );
        assert!(checkout.join("AGENTS.override.md").exists());
        assert!(!checkout.join(".env.secret").exists());
        cleanup(&project);
    }

    #[test]
    fn handoff_moves_staged_unstaged_and_untracked_work_and_returns_to_the_same_checkout() {
        let (project, store) = worktrees("handoff");
        let entry = store
            .create_isolated(&project, Some("chat"), None, true, None, false)
            .unwrap();
        let checkout = Path::new(&entry.path);
        let branch = git(&project, &["symbolic-ref", "HEAD"]).unwrap();
        std::fs::write(project.join("README.md"), "staged\n").unwrap();
        git(&project, &["add", "."]).unwrap();
        std::fs::write(project.join("README.md"), "unstaged\n").unwrap();
        std::fs::write(project.join("untracked.bin"), [0, 255]).unwrap();
        let status = git(&project, &["status", "--porcelain"]).unwrap();
        let staged = git(&project, &["diff", "--cached"]).unwrap();
        move_checkout(&project, checkout, || Ok(())).unwrap();
        assert_eq!(git(&project, &["status", "--porcelain"]).unwrap(), "");
        assert_eq!(git(checkout, &["status", "--porcelain"]).unwrap(), status);
        assert_eq!(git(checkout, &["diff", "--cached"]).unwrap(), staged);
        assert_eq!(git(checkout, &["symbolic-ref", "HEAD"]).unwrap(), branch);
        assert!(git(&project, &["symbolic-ref", "--quiet", "HEAD"]).is_err());
        move_checkout(checkout, &project, || Ok(())).unwrap();
        assert_eq!(git(&project, &["status", "--porcelain"]).unwrap(), status);
        assert_eq!(git(&project, &["diff", "--cached"]).unwrap(), staged);
        assert_eq!(
            std::fs::read(project.join("untracked.bin")).unwrap(),
            [0, 255]
        );
        assert_eq!(git(checkout, &["status", "--porcelain"]).unwrap(), "");
        cleanup(&project);
    }

    #[test]
    fn handoff_refuses_a_dirty_destination_and_rolls_back_a_failed_session_move() {
        let (project, store) = worktrees("handoff-failure");
        let entry = store
            .create_isolated(&project, Some("chat"), None, true, None, false)
            .unwrap();
        let checkout = Path::new(&entry.path);
        std::fs::write(project.join("README.md"), "keep this\n").unwrap();
        std::fs::write(checkout.join("occupied"), "other task\n").unwrap();
        assert!(move_checkout(&project, checkout, || Ok(())).is_err());
        assert_eq!(
            std::fs::read_to_string(checkout.join("occupied")).unwrap(),
            "other task\n"
        );
        std::fs::remove_file(checkout.join("occupied")).unwrap();
        let source_head = git(&project, &["symbolic-ref", "HEAD"]).unwrap();
        let destination_head = git(checkout, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(project.join("new.txt"), "new\n").unwrap();
        assert!(move_checkout(&project, checkout, || anyhow::bail!(
            "session store refused"
        ))
        .is_err());
        assert_eq!(
            git(&project, &["symbolic-ref", "HEAD"]).unwrap(),
            source_head
        );
        assert_eq!(
            git(checkout, &["rev-parse", "HEAD"]).unwrap(),
            destination_head
        );
        assert_eq!(
            std::fs::read_to_string(project.join("README.md")).unwrap(),
            "keep this\n"
        );
        assert_eq!(
            std::fs::read_to_string(project.join("new.txt")).unwrap(),
            "new\n"
        );
        assert_eq!(git(checkout, &["status", "--porcelain"]).unwrap(), "");
        cleanup(&project);
    }

    #[test]
    fn a_detached_checkout_can_be_given_a_branch_without_losing_changes() {
        let (project, store) = worktrees("name-detached");
        let entry = store
            .create_isolated(&project, Some("chat"), None, true, None, false)
            .unwrap();
        std::fs::write(Path::new(&entry.path).join("README.md"), "work\n").unwrap();
        assert_eq!(
            store
                .create_branch(&project, "chat", "oxide/my-task")
                .unwrap()
                .branch,
            "oxide/my-task"
        );
        assert_eq!(store.list(&project)[0].branch, "oxide/my-task");
        assert_eq!(
            std::fs::read_to_string(Path::new(&entry.path).join("README.md")).unwrap(),
            "work\n"
        );
        assert!(store.create_branch(&project, "chat", "--bad").is_err());
        cleanup(&project);
    }

    #[test]
    fn isolated_checkouts_copy_changes_without_changing_the_source() {
        let (project, store) = worktrees("isolated");
        std::fs::write(project.join("README.md"), "staged\n").unwrap();
        git(&project, &["add", "README.md"]).unwrap();
        std::fs::write(project.join("README.md"), "unstaged\n").unwrap();
        std::fs::write(project.join("new.bin"), [0, 1, 255]).unwrap();
        let before = git(&project, &["status", "--porcelain"]).unwrap();
        let entry = store
            .create_isolated(&project, Some("copy"), None, true, None, true)
            .unwrap();
        let checkout = Path::new(&entry.path);
        assert_eq!(
            git(checkout, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap(),
            "HEAD"
        );
        assert_eq!(
            std::fs::read_to_string(checkout.join("README.md")).unwrap(),
            "unstaged\n"
        );
        assert_eq!(
            std::fs::read(checkout.join("new.bin")).unwrap(),
            [0, 1, 255]
        );
        assert_eq!(git(&project, &["status", "--porcelain"]).unwrap(), before);
        assert_eq!(store.list(&project)[0].branch, entry.branch);
        assert!(store.remove(&project, "copy").is_err());
        cleanup(&project);
    }

    #[test]
    fn isolated_checkouts_start_at_the_requested_commit_and_preserve_detached_commits() {
        let (project, store) = worktrees("isolated-base");
        let first = git(&project, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(project.join("later.txt"), "later\n").unwrap();
        git(&project, &["add", "."]).unwrap();
        git(&project, &["commit", "-qm", "later"]).unwrap();
        let entry = store
            .create_isolated(&project, Some("old"), Some(&first), true, None, false)
            .unwrap();
        let checkout = Path::new(&entry.path);
        assert!(!checkout.join("later.txt").exists());
        std::fs::write(checkout.join("README.md"), "detached work\n").unwrap();
        git(checkout, &["add", "."]).unwrap();
        git(checkout, &["commit", "-qm", "detached"]).unwrap();
        let commit = git(checkout, &["rev-parse", "HEAD"]).unwrap();
        let removed = store.remove(&project, "old").unwrap();
        assert_eq!(
            git(&project, &["rev-parse", &removed.branch]).unwrap(),
            commit
        );
        assert!(!checkout.exists());
        cleanup(&project);
    }

    #[test]
    fn failed_change_copy_rolls_back_only_the_new_checkout() {
        let (project, store) = worktrees("isolated-failure");
        let first = git(&project, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(project.join("README.md"), "different committed base\n").unwrap();
        git(&project, &["add", "."]).unwrap();
        git(&project, &["commit", "-qm", "different"]).unwrap();
        std::fs::write(project.join("README.md"), "local edit\n").unwrap();
        assert!(store
            .create_isolated(&project, Some("conflict"), Some(&first), true, None, true)
            .is_err());
        assert!(store.list(&project).is_empty());
        assert_eq!(
            std::fs::read_to_string(project.join("README.md")).unwrap(),
            "local edit\n"
        );
        cleanup(&project);
    }

    #[test]
    fn isolated_checkouts_reject_a_source_from_another_repository() {
        let (project, store) = worktrees("isolated-source");
        let (other, _) = worktrees("other-source");
        assert!(store
            .create_isolated(&project, Some("wrong"), None, true, Some(&other), true)
            .is_err());
        assert!(store.list(&project).is_empty());
        cleanup(&project);
        cleanup(&other);
    }

    #[test]
    fn creates_a_checkout_on_its_own_branch() {
        let (project, store) = worktrees("create");
        let entry = store.create(&project, Some("fix login"), None).unwrap();
        assert_eq!(entry.name, "fix-login");
        assert_eq!(entry.branch, "oxide/fix-login");
        assert!(Path::new(&entry.path).join("README.md").is_file());
        assert_eq!(
            git(
                Path::new(&entry.path),
                &["rev-parse", "--abbrev-ref", "HEAD"]
            )
            .unwrap(),
            "oxide/fix-login"
        );
        // The project's own working tree is untouched: no worktree is drawn in
        // it, and its branch is still the one it was on.
        assert_eq!(git(&project, &["status", "--porcelain"]).unwrap(), "");
        cleanup(&project);
    }

    #[test]
    fn a_created_worktree_is_listed_and_found_from_inside_it() {
        let (project, store) = worktrees("list");
        let entry = store.create(&project, Some("add worktree"), None).unwrap();
        let listed = store.list(&project);
        assert_eq!(listed, vec![entry.clone()]);
        assert_eq!(store.find(&project, "add-worktree"), Some(entry.clone()));
        assert_eq!(store.view(Path::new(&entry.path)), Some(entry.clone()));
        let nested = Path::new(&entry.path).join("src/deep");
        assert_eq!(store.view(&nested), Some(entry.clone()));
        assert_eq!(
            store.project_of(Path::new(&entry.path)),
            Some(project.canonicalize().unwrap())
        );
        // A folder that is not one of ours is not read as a worktree, whatever
        // its name begins with.
        assert!(store.view(&project).is_none());
        cleanup(&project);
    }

    #[test]
    fn the_branch_listed_is_the_one_the_worktree_is_on_now() {
        let (project, store) = worktrees("branch");
        let entry = store.create(&project, Some("switch"), None).unwrap();
        git(
            Path::new(&entry.path),
            &["checkout", "-b", "topic", "--quiet"],
        )
        .unwrap();
        assert_eq!(store.list(&project)[0].branch, "topic");
        cleanup(&project);
    }

    #[test]
    fn naming_a_taken_worktree_is_an_error_and_ensuring_hands_it_back() {
        let (project, store) = worktrees("ensure");
        let first = store.ensure(&project, Some("same"), None).unwrap();
        assert_eq!(store.ensure(&project, Some("same"), None).unwrap(), first);
        let refused = store.create(&project, Some("same"), None);
        assert!(refused.is_err(), "a second worktree took the same name");
        cleanup(&project);
    }

    #[test]
    fn two_unnamed_worktrees_are_given_their_own_names() {
        let (project, store) = worktrees("unnamed");
        let first = store.create(&project, None, None).unwrap();
        let second = store.create(&project, None, None).unwrap();
        assert_eq!(first.name, "worktree");
        assert_eq!(second.name, "worktree-2");
        assert_ne!(first.path, second.path);
        cleanup(&project);
    }

    #[test]
    fn removing_takes_the_checkout_and_leaves_the_branch() {
        let (project, store) = worktrees("remove");
        let entry = store.create(&project, Some("gone"), None).unwrap();
        store.remove(&project, "gone").unwrap();
        assert!(!Path::new(&entry.path).exists());
        assert!(store.list(&project).is_empty());
        assert!(git(
            &project,
            &["rev-parse", "--verify", "refs/heads/oxide/gone"]
        )
        .is_ok());
        // The branch is resumed rather than refused when the name comes back.
        let again = store.create(&project, Some("gone"), None).unwrap();
        assert_eq!(again.branch, "oxide/gone");
        cleanup(&project);
    }

    #[test]
    fn removing_refuses_uncommitted_work_and_keeps_the_registry() {
        let (project, store) = worktrees("dirty-remove");
        let entry = store.create(&project, Some("dirty"), None).unwrap();
        let checkout = Path::new(&entry.path);
        std::fs::write(checkout.join("README.md"), "work in progress\n").unwrap();
        assert!(store.remove(&project, "dirty").is_err());
        assert_eq!(store.find(&project, "dirty"), Some(entry.clone()));
        assert_eq!(
            std::fs::read_to_string(checkout.join("README.md")).unwrap(),
            "work in progress\n"
        );
        git(checkout, &["checkout", "--", "README.md"]).unwrap();
        std::fs::write(checkout.join("untracked.txt"), "new work\n").unwrap();
        assert!(store.remove(&project, "dirty").is_err());
        assert!(checkout.join("untracked.txt").is_file());
        assert_eq!(store.find(&project, "dirty"), Some(entry));
        cleanup(&project);
    }

    #[test]
    fn a_worktree_removed_by_hand_is_not_listed() {
        let (project, store) = worktrees("byhand");
        let entry = store.create(&project, Some("hand"), None).unwrap();
        git(&project, &["worktree", "remove", "--force", &entry.path]).unwrap();
        assert!(store.list(&project).is_empty());
        cleanup(&project);
    }

    /// A worktree belongs to the repository, not to the folder the command was
    /// run from: making one in a subdirectory still lists under the project, and
    /// is found from the root or from another subdirectory.
    #[test]
    fn a_worktree_made_in_a_subdirectory_belongs_to_the_repository() {
        let (project, store) = worktrees("subdir");
        let nested = project.join("crates/app");
        std::fs::create_dir_all(&nested).unwrap();
        let entry = store.create(&nested, Some("nested"), None).unwrap();
        assert_eq!(Path::new(&entry.project), project.canonicalize().unwrap());
        assert_eq!(store.list(&project).len(), 1);
        assert_eq!(store.list(&nested).len(), 1);
        assert_eq!(
            store.project_of(Path::new(&entry.path)),
            Some(project.canonicalize().unwrap())
        );
        cleanup(&project);
    }

    #[test]
    fn a_worktree_starts_from_the_commit_it_was_asked_for() {
        let (project, store) = worktrees("base");
        let first = git(&project, &["rev-parse", "HEAD"]).unwrap();
        std::fs::write(project.join("later.txt"), "later\n").unwrap();
        git(&project, &["add", "later.txt"]).unwrap();
        git(&project, &["commit", "-m", "second", "--quiet"]).unwrap();
        let entry = store.create(&project, Some("old"), Some(&first)).unwrap();
        assert!(!Path::new(&entry.path).join("later.txt").exists());
        cleanup(&project);
    }

    #[test]
    fn a_folder_that_is_not_a_repository_cannot_make_one() {
        let root =
            std::env::temp_dir().join(format!("oxide_worktree_plain_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = Worktrees::at(root.join("worktrees"));
        let refused = store.create(&root, Some("nope"), None);
        assert!(refused.is_err());
        assert!(store.list(&root).is_empty());
        cleanup(&root);
    }

    #[test]
    fn a_repository_with_no_commits_has_nothing_to_branch_from() {
        let root =
            std::env::temp_dir().join(format!("oxide_worktree_empty_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        git(&root, &["init", "--quiet"]).unwrap();
        let store = Worktrees::at(root.join("worktrees"));
        let refused = store.create(&root, Some("nope"), None);
        assert!(refused.is_err());
        cleanup(&root);
    }
}
