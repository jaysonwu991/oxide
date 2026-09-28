//! The local projects Oxide knows about.
//!
//! The desktop records every folder the user added to it in
//! `<config>/Oxide/desktop/projects.json`. A run is told about those folders —
//! beside the one it is in — through its system prompt, so a question about a
//! repository elsewhere on the machine ("can you reach the api-service repo?")
//! is answered from the list, and so the model knows it may reach into a
//! sibling project by absolute path, instead of locating either with a scan of
//! the home directory, which reads every unrelated file on the machine and is
//! killed by the command timeout.

use serde::Deserialize;
use std::path::{Path, PathBuf};

/// How many other projects the prompt names before it only counts the rest. A
/// project beside this one is what a question about a sibling repository means,
/// so the nearest ones survive the cut.
pub const MAX_LISTED: usize = 24;

/// A folder the user added to Oxide, as the desktop sidebar lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Project {
    /// The display name (the folder's basename unless the user renamed it).
    pub name: String,
    pub path: PathBuf,
}

/// The projects on this machine: the one this run is in, and the others the
/// desktop knows about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Workspaces {
    current: PathBuf,
    others: Vec<Project>,
    /// How many known projects the list left out.
    omitted: usize,
}

impl Workspaces {
    /// The projects a run in `cwd` is told about.
    pub fn load(cwd: &Path) -> Self {
        Self::load_from(cwd, &registry_path())
    }

    /// Loads the registry at an explicit path, so a test does not read the real
    /// one. A registry that is missing, unreadable or malformed leaves the run
    /// knowing nothing but its own directory.
    pub fn load_from(cwd: &Path, registry: &Path) -> Self {
        // `-C .` hands the run a relative path while the desktop recorded the
        // project at its full one, so the run's directory is resolved before
        // anything is compared with it.
        let current = normalize(cwd);
        let mut workspaces = Self {
            current: current.clone(),
            ..Self::default()
        };
        let Some(registry) = Registry::load(registry) else {
            return workspaces;
        };

        let parent = current.parent().map(Path::to_path_buf);
        let mut rows: Vec<(bool, u64, Project)> = Vec::new();
        for entry in registry.projects {
            // A folder that is gone — a removed checkout, an unmounted volume —
            // has no place in a list that says where the projects are.
            if !entry.path.is_dir() || same_dir(&entry.path, &current) {
                continue;
            }
            let name = entry.name.trim();
            rows.push((
                parent
                    .as_deref()
                    .zip(entry.path.parent())
                    .is_some_and(|(parent, entry_parent)| same_dir(entry_parent, parent)),
                entry.last_opened_at.unwrap_or(0),
                Project {
                    name: if name.is_empty() {
                        display_name(&entry.path)
                    } else {
                        name.to_string()
                    },
                    path: entry.path,
                },
            ));
        }

        // The sibling of this project comes first, then the most recently
        // opened: that is the order a question about another repository wants.
        rows.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then(b.1.cmp(&a.1))
                .then_with(|| a.2.name.to_lowercase().cmp(&b.2.name.to_lowercase()))
        });
        workspaces.omitted = rows.len().saturating_sub(MAX_LISTED);
        rows.truncate(MAX_LISTED);
        workspaces.others = rows.into_iter().map(|(_, _, project)| project).collect();
        workspaces
    }

    /// The other projects, nearest first.
    pub fn others(&self) -> &[Project] {
        &self.others
    }

    /// This run's own project directory.
    pub fn current(&self) -> &Path {
        &self.current
    }

    /// The `# Workspaces` section of the system prompt. `None` for a run whose
    /// directory is unknown (a hand-built `Config` in a test), which has nothing
    /// to say about where it is.
    pub fn section(&self) -> Option<String> {
        let current = self.current.to_string_lossy();
        if current.trim().is_empty() {
            return None;
        }
        let mut text = format!("# Workspaces\nThis run is in `{current}`.");
        if self.others.is_empty() {
            text.push_str(
                " This project is the only folder Oxide has on this machine, so a repository the \
                 user names lives elsewhere: ask for its path, or list the folder it is likely in \
                 (`ls` the parent) — do not scan the home directory (`find ~`), which reads every \
                 unrelated project on the machine before the command times out.",
            );
            return Some(text);
        }
        text.push_str(
            " The other folders added to Oxide on this machine are listed below. Each is an \
             ordinary directory with no sandbox in the way: `read`, `grep`, `find`, `ls`, `write` \
             and `edit` take absolute paths, so a file in another project can be read, searched \
             and edited from this run — but a `bash` command starts in this project's root, so \
             name the path instead of changing directory into it. Resolve a repository the user \
             names here, or by listing the folder beside this one, before searching: do not scan \
             the home directory (`find ~`), which reads every unrelated file on the machine.",
        );
        for project in &self.others {
            text.push_str(&format!(
                "\n- {} — {}",
                project.name,
                project.path.to_string_lossy()
            ));
        }
        if self.omitted > 0 {
            text.push_str(&format!(
                "\n({} more, most recently opened first, are not listed.)",
                self.omitted
            ));
        }
        Some(text)
    }
}

/// `<config>/Oxide/desktop/projects.json`, the registry the desktop app writes.
pub fn registry_path() -> PathBuf {
    crate::config::config_dir_or_default().join("desktop/projects.json")
}

/// The part of the desktop's registry this module reads. Only the path, the
/// display name and how recently the project was opened matter here, and each
/// is optional so a registry written by another build still parses.
#[derive(Debug, Deserialize)]
struct Registry {
    #[serde(default)]
    projects: Vec<Entry>,
}

#[derive(Debug, Deserialize)]
struct Entry {
    #[serde(default)]
    name: String,
    path: PathBuf,
    #[serde(default)]
    last_opened_at: Option<u64>,
}

impl Registry {
    fn load(path: &Path) -> Option<Self> {
        let text = std::fs::read_to_string(path).ok()?;
        let mut registry: Self = serde_json::from_str(&text).ok()?;
        registry
            .projects
            .retain(|entry| !entry.path.as_os_str().is_empty());
        Some(registry)
    }
}

/// The run's own directory as the machine names it: a relative `-C .` or
/// `-C ../oxide` is resolved against the process's working directory, and the
/// result is resolved once more where the filesystem can, so a symlinked
/// checkout answers to the path the desktop recorded. An empty path stays
/// empty — a hand-built [`Workspaces`] has no directory to name.
fn normalize(path: &Path) -> PathBuf {
    if path.as_os_str().is_empty() {
        return path.to_path_buf();
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        match std::env::current_dir() {
            Ok(dir) => dir.join(path),
            Err(_) => return path.to_path_buf(),
        }
    };
    std::fs::canonicalize(&absolute).unwrap_or(absolute)
}

/// Whether two paths name the same directory. The registry and the run can
/// spell one path differently — through a symlink, or with a `..` the kernel
/// resolved — so the filesystem's own answer decides when it has one.
fn same_dir(a: &Path, b: &Path) -> bool {
    if a.components().eq(b.components()) {
        return true;
    }
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "oxide_ws_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A path as the filesystem resolves it, which is how the section prints
    /// the run's own directory: a macOS temp directory is reached through
    /// `/var`, and the kernel names the same directory `/private/var`.
    fn canonical(path: &Path) -> String {
        std::fs::canonicalize(path)
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .to_string()
    }

    /// A registry with one entry per `(name, path, last_opened_at)`, written
    /// through serde the way the desktop writes it: a Windows path is full of
    /// backslashes, so a hand-built JSON string would not parse on Windows.
    fn registry(root: &Path, entries: &[(&str, &Path, u64)]) -> PathBuf {
        let projects: Vec<serde_json::Value> = entries
            .iter()
            .map(|(name, path, opened)| {
                let path = path.to_string_lossy();
                serde_json::json!({
                    "id": path,
                    "path": path,
                    "name": name,
                    "last_opened_at": opened,
                })
            })
            .collect();
        let path = root.join("projects.json");
        let text = serde_json::json!({ "projects": projects }).to_string();
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn a_sibling_project_is_named_first() {
        let root = temp_dir("order");
        let here = root.join("Projects/site");
        let sibling = root.join("Projects/api-service");
        let far = root.join("elsewhere/web-tools");
        for dir in [&here, &sibling, &far] {
            std::fs::create_dir_all(dir).unwrap();
        }
        // The far project was opened most recently; the sibling still leads.
        let store = registry(
            &root,
            &[("web-tools", &far, 90), ("api-service", &sibling, 10)],
        );

        let workspaces = Workspaces::load_from(&here, &store);
        let names: Vec<&str> = workspaces
            .others()
            .iter()
            .map(|project| project.name.as_str())
            .collect();
        assert_eq!(names, ["api-service", "web-tools"]);

        let section = workspaces.section().unwrap();
        assert!(section.contains("# Workspaces"));
        assert!(section.contains(&canonical(&here)));
        assert!(section.contains("absolute paths"), "{section}");
        assert!(section.contains("find ~"), "{section}");
        assert!(section.contains(&sibling.to_string_lossy().to_string()));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn only_the_most_recently_opened_survive_the_cap() {
        let root = temp_dir("cap");
        let here = root.join("Projects/here");
        std::fs::create_dir_all(&here).unwrap();
        let mut dirs = Vec::new();
        let mut entries = Vec::new();
        for index in 0..(MAX_LISTED + 3) {
            let dir = root.join(format!("Projects/p{index:02}"));
            std::fs::create_dir_all(&dir).unwrap();
            // Every one is a sibling, so recency decides the cut.
            entries.push((format!("p{index:02}"), dir.clone(), index as u64));
            dirs.push(dir);
        }
        let borrowed: Vec<(&str, &Path, u64)> = entries
            .iter()
            .map(|(name, path, opened)| (name.as_str(), path.as_path(), *opened))
            .collect();
        let store = registry(&root, &borrowed);

        let workspaces = Workspaces::load_from(&here, &store);
        assert_eq!(workspaces.others().len(), MAX_LISTED);
        assert_eq!(workspaces.others()[0].name, "p26");
        assert_eq!(workspaces.others().last().unwrap().name, "p03");
        let section = workspaces.section().unwrap();
        assert!(section.contains("3 more"), "{section}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_list_skips_this_project_and_folders_that_are_gone() {
        let root = temp_dir("skip");
        let here = root.join("Projects/here");
        let gone = root.join("Projects/deleted");
        std::fs::create_dir_all(&here).unwrap();
        let store = registry(&root, &[("here", &here, 5), ("deleted", &gone, 9)]);

        let workspaces = Workspaces::load_from(&here, &store);
        assert!(workspaces.others().is_empty(), "{:?}", workspaces.others());
        // A run whose only project is its own still says where it is and how to
        // find another repository.
        let section = workspaces.section().unwrap();
        assert!(section.contains(&canonical(&here)));
        assert!(section.contains("only folder Oxide has"), "{section}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_broken_registry_costs_nothing() {
        let root = temp_dir("broken");
        let here = root.join("Projects/here");
        std::fs::create_dir_all(&here).unwrap();
        let store = root.join("projects.json");

        // Missing.
        assert!(Workspaces::load_from(&here, &store).others().is_empty());
        // Malformed.
        std::fs::write(&store, "{ not json").unwrap();
        let workspaces = Workspaces::load_from(&here, &store);
        assert!(workspaces.others().is_empty());
        assert_eq!(
            std::fs::canonicalize(workspaces.current()).unwrap(),
            std::fs::canonicalize(&here).unwrap()
        );
        // Well-formed but empty, and an entry with no path at all.
        std::fs::write(&store, "{\"projects\":[{\"name\":\"nameless\"}]}").unwrap();
        assert!(Workspaces::load_from(&here, &store).others().is_empty());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unknown_directory_says_nothing() {
        let workspaces = Workspaces::default();
        assert_eq!(workspaces.section(), None);
    }

    #[test]
    fn a_relative_run_directory_is_the_project_it_names() {
        // `oxide -C .` hands the run a relative path while the desktop recorded
        // the project at its full one: the run must not be listed beside itself.
        let root = temp_dir("relative");
        let here = std::env::current_dir().unwrap();
        let store = registry(&root, &[("here", &here, 1)]);

        let workspaces = Workspaces::load_from(Path::new("."), &store);
        assert!(workspaces.others().is_empty(), "{:?}", workspaces.others());
        assert!(workspaces.current().is_absolute());
        assert_eq!(
            std::fs::canonicalize(workspaces.current()).unwrap(),
            std::fs::canonicalize(&here).unwrap()
        );
        let section = workspaces.section().unwrap();
        assert!(section.contains("only folder Oxide has"), "{section}");

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_spelling_of_this_project_is_not_a_second_one() {
        let root = temp_dir("symlink");
        let real = root.join("Projects/site");
        std::fs::create_dir_all(&real).unwrap();
        let link = root.join("shortcut");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        // The run is in the project through the link, and the registry names it
        // by its real path: one project, not two.
        let store = registry(&root, &[("site", &real, 1)]);

        let workspaces = Workspaces::load_from(&link, &store);
        assert!(workspaces.others().is_empty(), "{:?}", workspaces.others());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_entry_without_a_name_falls_back_to_the_folder() {
        let root = temp_dir("nameless");
        let here = root.join("Projects/here");
        let other = root.join("Projects/data-pipeline");
        std::fs::create_dir_all(&here).unwrap();
        std::fs::create_dir_all(&other).unwrap();
        let store = root.join("projects.json");
        let text = serde_json::json!({
            "projects": [{ "path": other.to_string_lossy(), "name": "  " }],
        })
        .to_string();
        std::fs::write(&store, text).unwrap();

        let workspaces = Workspaces::load_from(&here, &store);
        assert_eq!(workspaces.others()[0].name, "data-pipeline");
        assert!(workspaces.section().unwrap().contains("data-pipeline"));

        std::fs::remove_dir_all(&root).ok();
    }
}
