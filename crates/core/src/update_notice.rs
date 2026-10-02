//! The "update available" notice a front-end shows at launch, Pi's own startup
//! surface for a newer release.
//!
//! A launch must not wait on the network for it, so the answer is remembered in
//! `updates.json` beside the rest of Oxide's state and shown on the next launch
//! whether or not it could be refreshed; the lookup itself is redone only once
//! the remembered answer is old (`oxide update` is still the way to look right
//! now). What is remembered is one newest release per component, so a
//! front-end that shows its own notice — the terminal, for its own train —
//! neither waits for nor overwrites another one's answer.

use crate::updates::{self, Component, Release};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// How long a remembered release is trusted before a launch looks again. Long
/// enough that a daily launch costs one request, short enough that a release
/// cut today is offered tomorrow.
pub const REFRESH_AFTER_SECS: u64 = 6 * 60 * 60;

/// The file the remembered releases live in, beside `config.json`.
pub const FILE_NAME: &str = "updates.json";

/// The `settings.json` key that turns the launch check off.
const SETTING_NAME: &str = "checkForUpdates";
const ENV_NAME: &str = "OXIDE_CHECK_FOR_UPDATES";
/// The path override, which is how a test (or a caller with its own config
/// directory) redirects the file.
const FILE_ENV: &str = "OXIDE_UPDATES_FILE";

/// Two oxide processes — a terminal and the desktop app — can write the file at
/// the same time; read-modify-write is serialized within one of them.
static STORE_LOCK: Mutex<()> = Mutex::new(());

/// The newest release of one component, as the last launch found it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    /// The version the release names, without its tag's prefix.
    pub version: String,
    pub tag: String,
    /// The release page, which is where its notes are.
    pub url: String,
    /// When it was found, in Unix seconds.
    pub checked_at: u64,
}

impl Notice {
    pub fn from_release(release: &Release, repo: &str, checked_at: u64) -> Self {
        Self {
            version: release.version.clone(),
            tag: release.tag.clone(),
            url: release.page_url(repo),
            checked_at,
        }
    }

    /// Whether this remembers a release newer than the one running.
    pub fn is_update_for(&self, current: &str) -> bool {
        updates::is_newer(&self.version, current)
    }

    /// Whether it is time to look the release up again.
    pub fn is_stale(&self) -> bool {
        now_secs().saturating_sub(self.checked_at) >= REFRESH_AFTER_SECS
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    components: BTreeMap<String, Notice>,
}

/// The file the remembered releases are kept in.
pub fn path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(FILE_ENV) {
        return Some(PathBuf::from(path));
    }
    Some(crate::config::config_dir()?.join(FILE_NAME))
}

/// The newest release of a component the last launch found, whatever its age.
pub fn cached(component: Component) -> Option<Notice> {
    read(&path()?, component)
}

/// Looks for the newest release of a component and remembers it.
pub async fn refresh(component: Component) -> Result<Notice> {
    let repo = updates::repo();
    let client = updates::client()?;
    // A platform with no release of its own still has a version to be told
    // about, so an unresolvable one asks for the version alone.
    let platform = updates::platform().unwrap_or_default();
    let release = updates::latest(&client, &repo, component, platform).await?;
    remember(component, &release, &repo)
}

/// Remembers the newest release of a component, leaving every other component's
/// answer as it was.
pub fn remember(component: Component, release: &Release, repo: &str) -> Result<Notice> {
    let path = path().context("resolving the Oxide config directory")?;
    let notice = Notice::from_release(release, repo, now_secs());
    write(&path, component, &notice)?;
    Ok(notice)
}

/// Whether a launch should look for a newer release at all. `checkForUpdates`
/// in a `settings.json` decides — the project's own file winning over the
/// global one — and `OXIDE_CHECK_FOR_UPDATES` overrides both. On by default.
pub fn enabled(cwd: &Path) -> bool {
    if let Some(value) = env_bool(ENV_NAME) {
        return value;
    }
    enabled_within(&settings_paths(cwd)).unwrap_or(true)
}

/// Persists the flag into the global `settings.json`, preserving every other
/// key, and returns the file written. A project `.oxide/settings.json` can
/// still override it.
pub fn save(enabled: bool) -> Result<PathBuf> {
    let path = crate::config::settings_path();
    crate::config::save_setting_to(&path, SETTING_NAME, serde_json::Value::Bool(enabled))?;
    Ok(path)
}

fn settings_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = vec![crate::config::settings_path()];
    if let Some(root) = crate::ecosystem::project_root(cwd) {
        paths.push(root.join(".oxide").join("settings.json"));
    }
    paths
}

fn enabled_within(paths: &[PathBuf]) -> Option<bool> {
    for path in paths.iter().rev() {
        let Ok(text) = std::fs::read_to_string(path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if let Some(enabled) = value.get(SETTING_NAME).and_then(serde_json::Value::as_bool) {
            return Some(enabled);
        }
    }
    None
}

fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<bool>().ok())
}

fn read(path: &Path, component: Component) -> Option<Notice> {
    let _guard = STORE_LOCK.lock().ok()?;
    load(path).components.get(component.as_str()).cloned()
}

fn write(path: &Path, component: Component, notice: &Notice) -> Result<()> {
    let _guard = STORE_LOCK
        .lock()
        .ok()
        .context("locking the remembered releases")?;
    let mut store = load(path);
    store
        .components
        .insert(component.as_str().to_string(), notice.clone());
    if let Some(parent) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text =
        serde_json::to_string_pretty(&store).context("serializing the remembered releases")?;
    // Written through a temporary name, so a reader never sees half a file.
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    std::fs::write(&temporary, text).with_context(|| format!("writing {}", temporary.display()))?;
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("writing {}", path.display()));
    }
    Ok(())
}

fn load(path: &Path) -> Store {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("oxide-update-notice-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn notice(version: &str, checked_at: u64) -> Notice {
        let release = Release::new(Component::Cli, &format!("v{version}"), None, "darwin-arm64");
        Notice::from_release(&release, "acme/oxide", checked_at)
    }

    #[test]
    fn a_remembered_release_is_read_back_beside_the_others() {
        let dir = temp_dir("store");
        let path = dir.join(FILE_NAME);
        write(&path, Component::Cli, &notice("0.34.0", 1_000)).unwrap();
        write(&path, Component::Extension, &notice("0.9.0", 1_000)).unwrap();

        let cli = read(&path, Component::Cli).expect("the CLI's release");
        assert_eq!(cli.version, "0.34.0");
        assert_eq!(cli.tag, "v0.34.0");
        assert_eq!(
            cli.url,
            "https://github.com/acme/oxide/releases/tag/v0.34.0"
        );
        // Remembering one component does not erase another's answer.
        assert_eq!(
            read(&path, Component::Extension)
                .expect("the extension's release")
                .version,
            "0.9.0"
        );
        assert_eq!(read(&path, Component::Desktop), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_remembered_release_is_only_offered_when_it_is_newer() {
        let notice = notice("0.34.0", 1_000);
        assert!(notice.is_update_for("0.33.9"));
        assert!(notice.is_update_for("v0.33.9"));
        assert!(!notice.is_update_for("0.34.0"));
        assert!(!notice.is_update_for("0.35.0"));
        // A version that does not parse is a source build: nothing to offer it.
        assert!(notice.is_update_for("0.0.0"));
    }

    #[test]
    fn an_old_answer_is_looked_up_again() {
        assert!(!notice("0.34.0", now_secs()).is_stale());
        assert!(notice("0.34.0", now_secs() - REFRESH_AFTER_SECS).is_stale());
        assert!(notice("0.34.0", 0).is_stale());
    }

    #[test]
    fn an_unreadable_store_is_no_remembered_release() {
        let dir = temp_dir("broken");
        let path = dir.join(FILE_NAME);
        std::fs::write(&path, "{not json").unwrap();
        assert_eq!(read(&path, Component::Cli), None);
        // A malformed store is replaced rather than refusing the write.
        write(&path, Component::Cli, &notice("0.34.0", 1_000)).unwrap();
        assert_eq!(
            read(&path, Component::Cli).map(|n| n.version),
            Some("0.34.0".into())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_launch_check_defaults_on_and_a_project_can_turn_it_off() {
        let dir = temp_dir("settings");
        let global = dir.join("settings.json");
        let project = dir.join(".oxide").join("settings.json");

        assert_eq!(enabled_within(&[global.clone(), project.clone()]), None);
        assert!(enabled_within(std::slice::from_ref(&global)).unwrap_or(true));

        std::fs::write(&global, r#"{"checkForUpdates": false}"#).unwrap();
        assert_eq!(enabled_within(std::slice::from_ref(&global)), Some(false));
        // Malformed or unrelated settings say nothing about the flag.
        std::fs::write(dir.join("other.json"), r#"{"hideThinkingBlock": true}"#).unwrap();
        assert_eq!(enabled_within(&[dir.join("other.json")]), None);

        // The project's own file wins over the global one, both ways round.
        std::fs::create_dir_all(project.parent().unwrap()).unwrap();
        std::fs::write(&project, r#"{"checkForUpdates": true}"#).unwrap();
        assert_eq!(
            enabled_within(&[global.clone(), project.clone()]),
            Some(true)
        );
        std::fs::write(&global, r#"{"checkForUpdates": true}"#).unwrap();
        std::fs::write(&project, r#"{"checkForUpdates": false}"#).unwrap();
        assert_eq!(enabled_within(&[global, project]), Some(false));
        std::fs::remove_dir_all(&dir).ok();
    }
}
