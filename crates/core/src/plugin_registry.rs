//! Claude Code-style plugin packages and marketplaces.
//!
//! A *marketplace* is a directory (or git repository) containing a manifest
//! (`.oxide/marketplace.json` or, for Claude Code compatibility,
//! `.claude-plugin/marketplace.json`) that lists plugins. A *plugin* is a
//! directory containing a manifest (`.oxide/plugin.json` or
//! `.claude-plugin/plugin.json`) plus bundled resources: `commands/`,
//! `agents/`, `skills/`, `hooks/` (declared in the manifest) and `mcpServers`.
//! Installed plugins live under the Oxide config directory
//! (`<config>/Oxide/plugins/`) and are loaded into the ecosystem at startup,
//! mirroring Claude Code's plugin model.

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const STATE_FILE: &str = "config.json";

fn default_true() -> bool {
    true
}

/// The directory that holds installed plugins, marketplaces, and the plugin
/// state file.
pub fn install_root() -> PathBuf {
    crate::config::config_dir_or_default().join("plugins")
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginState {
    #[serde(default)]
    pub marketplaces: BTreeMap<String, MarketplaceRecord>,
    #[serde(default)]
    pub plugins: BTreeMap<String, InstalledPlugin>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketplaceRecord {
    pub name: String,
    pub source: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPlugin {
    pub name: String,
    #[serde(default)]
    pub marketplace: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub path: PathBuf,
}

fn state_path_in(root: &Path) -> PathBuf {
    root.join(STATE_FILE)
}

fn load_state_in(root: &Path) -> Result<PluginState> {
    let path = state_path_in(root);
    if !path.exists() {
        return Ok(PluginState::default());
    }
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("reading plugin state at {}", path.display()))?;
    serde_json::from_str(&text)
        .with_context(|| format!("parsing plugin state at {}", path.display()))
}

fn save_state_in(root: &Path, state: &PluginState) -> Result<()> {
    let path = state_path_in(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(state)?;
    std::fs::write(&path, format!("{text}\n"))
        .with_context(|| format!("writing plugin state to {}", path.display()))
}

pub fn load_state() -> Result<PluginState> {
    load_state_in(&install_root())
}

// ---------------------------------------------------------------------------
// Manifests
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Owner {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

/// The plugin-package manifest, read from `.oxide/plugin.json` (preferred) or
/// `.claude-plugin/plugin.json` (Claude Code compatibility).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub author: Option<Owner>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default, rename = "mcpServers")]
    pub mcp_servers: Option<serde_json::Value>,
    #[serde(default)]
    pub hooks: Option<serde_json::Value>,
}

/// The marketplace manifest, read from `.oxide/marketplace.json` (preferred)
/// or `.claude-plugin/marketplace.json` (Claude Code compatibility).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketplaceManifest {
    pub name: String,
    #[serde(default)]
    pub owner: Option<Owner>,
    #[serde(default)]
    pub plugins: Vec<MarketplacePlugin>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MarketplacePlugin {
    pub name: String,
    /// A git URL, a local path, or `{"source": ..., "repo": ...}`.
    #[serde(default)]
    pub source: serde_json::Value,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub author: Option<Owner>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub homepage: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

pub fn plugin_manifest(dir: &Path) -> Result<PluginManifest> {
    let text = read_manifest(dir, "plugin.json")?;
    serde_json::from_str(&text)
        .with_context(|| format!("parsing plugin manifest in {}", dir.display()))
}

pub fn marketplace_manifest(dir: &Path) -> Result<MarketplaceManifest> {
    let text = read_manifest(dir, "marketplace.json")?;
    serde_json::from_str(&text)
        .with_context(|| format!("parsing marketplace manifest in {}", dir.display()))
}

/// Manifest lookup order for a plugin package or marketplace. The oxide-native
/// `.oxide/<file>` location wins when both are present; the Claude Code
/// `.claude-plugin/<file>` location is read otherwise for compatibility.
fn manifest_candidates(dir: &Path, file: &str) -> Vec<PathBuf> {
    vec![
        dir.join(".oxide").join(file),
        dir.join(".claude-plugin").join(file),
    ]
}

fn read_manifest(dir: &Path, file: &str) -> Result<String> {
    for candidate in manifest_candidates(dir, file) {
        if candidate.is_file() {
            return std::fs::read_to_string(&candidate)
                .with_context(|| format!("reading {file} at {}", candidate.display()));
        }
    }
    bail!(
        "no `.oxide/{file}` or `.claude-plugin/{file}` found in {}",
        dir.display()
    )
}

/// Extracts a plugin `source` value (string or object) into a source string.
pub fn source_string(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => Some(text.clone()),
        serde_json::Value::Object(object) => object
            .get("repo")
            .or_else(|| object.get("url"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}

/// Splits a `name@marketplace` reference into its parts. A bare name (or a
/// name with an empty marketplace) yields `(name, None)`.
pub fn split_ref(reference: &str) -> (String, Option<String>) {
    match reference.split_once('@') {
        Some((name, marketplace)) if !name.is_empty() && !marketplace.is_empty() => {
            (name.to_string(), Some(marketplace.to_string()))
        }
        _ => (reference.to_string(), None),
    }
}

// ---------------------------------------------------------------------------
// Installed plugin discovery (used by the ecosystem)
// ---------------------------------------------------------------------------

pub struct EnabledPlugin {
    pub name: String,
    pub path: PathBuf,
    pub manifest: PluginManifest,
}

/// Installed, enabled plugins whose directories still exist and whose manifests
/// parse. Broken entries are skipped so one bad plugin cannot break startup.
pub fn enabled_plugins() -> Vec<EnabledPlugin> {
    let Ok(state) = load_state() else {
        return Vec::new();
    };
    let mut plugins = Vec::new();
    for installed in state.plugins.values() {
        if !installed.enabled || !installed.path.is_dir() {
            continue;
        }
        match plugin_manifest(&installed.path) {
            Ok(manifest) => plugins.push(EnabledPlugin {
                name: installed.name.clone(),
                path: installed.path.clone(),
                manifest,
            }),
            Err(err) => eprintln!("[plugin] skipping `{}`: {err:#}", installed.name),
        }
    }
    plugins.sort_by(|a, b| a.name.cmp(&b.name));
    plugins
}

// ---------------------------------------------------------------------------
// Source resolution
// ---------------------------------------------------------------------------

fn is_git_source(source: &str) -> bool {
    source.starts_with("http://")
        || source.starts_with("https://")
        || source.starts_with("git@")
        || source.starts_with("ssh://")
        || source.starts_with("git://")
        || source.ends_with(".git")
}

/// Expands GitHub's `owner/repo` shorthand into a clone URL, matching the
/// syntax Claude Code accepts for marketplaces. Anything that already looks
/// like a URL or a local path is returned unchanged.
fn expand_source(source: &str) -> String {
    let trimmed = source.trim();
    if is_git_source(trimmed) {
        return trimmed.to_string();
    }
    if trimmed.starts_with(['.', '/', '~']) || Path::new(trimmed).exists() {
        return trimmed.to_string();
    }
    let mut parts = trimmed.split('/');
    let (Some(owner), Some(repo), None) = (parts.next(), parts.next(), parts.next()) else {
        return trimmed.to_string();
    };
    if owner.is_empty() || repo.is_empty() || owner.contains(':') || repo.contains(':') {
        return trimmed.to_string();
    }
    format!("https://github.com/{owner}/{repo}.git")
}

fn git_name(source: &str) -> String {
    let trimmed = source.trim_end_matches('/').trim_end_matches(".git");
    Path::new(trimmed)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "marketplace".to_string())
}

fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "plugin".to_string()
    } else {
        cleaned
    }
}

async fn git_clone(source: &str, dest: &Path) -> Result<()> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).with_context(|| format!("removing {}", dest.display()))?;
    }
    let output = tokio::process::Command::new("git")
        .args(["clone", "--depth", "1", source])
        .arg(dest)
        .output()
        .await
        .context("running git clone (is git installed?)")?;
    if !output.status.success() {
        bail!(
            "git clone failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

async fn run_git(dir: &Path, args: &[&str]) -> Result<()> {
    let output = tokio::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .await
        .with_context(|| {
            format!(
                "running git {} (is git installed?)",
                args.first().unwrap_or(&"")
            )
        })?;
    if !output.status.success() {
        bail!(
            "git {} failed: {}",
            args.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

fn copy_dir(src: &Path, dest: &Path) -> Result<()> {
    if dest.exists() {
        std::fs::remove_dir_all(dest).with_context(|| format!("removing {}", dest.display()))?;
    }
    std::fs::create_dir_all(dest).with_context(|| format!("creating {}", dest.display()))?;
    for entry in std::fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry.context("reading directory entry")?;
        let source_path = entry.path();
        let target = dest.join(entry.file_name());
        if source_path.is_dir() {
            copy_dir(&source_path, &target)?;
        } else {
            std::fs::copy(&source_path, &target)
                .with_context(|| format!("copying {}", source_path.display()))?;
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Marketplaces
// ---------------------------------------------------------------------------

/// Adds a marketplace from a git URL or local path, recording it in state.
/// Returns a human-readable summary.
pub async fn add_marketplace(source: &str) -> Result<String> {
    add_marketplace_in(&install_root(), source).await
}

async fn add_marketplace_in(root: &Path, source: &str) -> Result<String> {
    let source = expand_source(source);
    let dir = if is_git_source(&source) {
        let name = git_name(&source);
        let dir = root.join("marketplaces").join(sanitize(&name));
        git_clone(&source, &dir).await?;
        dir
    } else {
        PathBuf::from(&source)
    };
    if !manifest_candidates(&dir, "marketplace.json")
        .iter()
        .any(|path| path.is_file())
    {
        bail!(
            "no `.oxide/marketplace.json` or `.claude-plugin/marketplace.json` found in {}",
            dir.display()
        );
    }

    let manifest = marketplace_manifest(&dir)?;
    let plugin_count = manifest.plugins.len();
    let mut state = load_state_in(root)?;
    state.marketplaces.insert(
        manifest.name.clone(),
        MarketplaceRecord {
            name: manifest.name.clone(),
            source: source.to_string(),
            path: dir.clone(),
        },
    );
    save_state_in(root, &state)?;
    Ok(format!(
        "added marketplace `{}` ({} plugin{})",
        manifest.name,
        plugin_count,
        if plugin_count == 1 { "" } else { "s" }
    ))
}

pub fn remove_marketplace(name: &str) -> Result<String> {
    remove_marketplace_in(&install_root(), name)
}

fn remove_marketplace_in(root: &Path, name: &str) -> Result<String> {
    let mut state = load_state_in(root)?;
    let Some(record) = state.marketplaces.remove(name) else {
        bail!("no marketplace named `{name}`");
    };
    let removed: Vec<InstalledPlugin> = state
        .plugins
        .values()
        .filter(|plugin| plugin.marketplace.as_deref() == Some(name))
        .cloned()
        .collect();
    for plugin in &removed {
        state.plugins.remove(&plugin.name);
    }
    save_state_in(root, &state)?;
    if record.path.starts_with(root) {
        std::fs::remove_dir_all(&record.path).ok();
    }
    for plugin in &removed {
        if plugin.path.starts_with(root) {
            std::fs::remove_dir_all(&plugin.path).ok();
        }
    }
    let plugin_dir = root.join(sanitize(name));
    if plugin_dir != root {
        std::fs::remove_dir(&plugin_dir).ok();
    }
    Ok(format!("removed marketplace `{name}`"))
}

/// Fetches the latest marketplace manifest from its git remote. Git-backed
/// marketplaces are fast-forwarded to the remote head; local marketplaces are
/// live directories, so there is nothing to fetch. Returns a summary.
pub async fn update_marketplace(name: &str) -> Result<String> {
    update_marketplace_in(&install_root(), name).await
}

async fn update_marketplace_in(root: &Path, name: &str) -> Result<String> {
    let state = load_state_in(root)?;
    let Some(record) = state.marketplaces.get(name) else {
        bail!("no marketplace named `{name}`");
    };
    if !is_git_source(&record.source) {
        let count = marketplace_manifest(&record.path)
            .map(|manifest| manifest.plugins.len())
            .unwrap_or(0);
        return Ok(format!(
            "marketplace `{name}` is a local directory — nothing to fetch ({count} plugin{})",
            if count == 1 { "" } else { "s" }
        ));
    }
    run_git(&record.path, &["fetch", "--depth", "1", "origin"]).await?;
    run_git(&record.path, &["reset", "--hard", "FETCH_HEAD"]).await?;
    let count = marketplace_manifest(&record.path)
        .map(|manifest| manifest.plugins.len())
        .unwrap_or(0);
    Ok(format!(
        "updated marketplace `{name}` ({count} plugin{})",
        if count == 1 { "" } else { "s" }
    ))
}

pub fn list_marketplaces() -> Result<String> {
    list_marketplaces_in(&install_root())
}

fn list_marketplaces_in(root: &Path) -> Result<String> {
    let state = load_state_in(root)?;
    if state.marketplaces.is_empty() {
        return Ok(
            "no marketplaces configured — use `oxide plugin marketplace add <url|path>`"
                .to_string(),
        );
    }
    let mut lines = vec![format!("marketplaces ({}):", state.marketplaces.len())];
    for record in state.marketplaces.values() {
        let count = state
            .plugins
            .values()
            .filter(|plugin| plugin.marketplace.as_deref() == Some(record.name.as_str()))
            .count();
        lines.push(format!("  {} — {}", record.name, record.source));
        lines.push(format!(
            "      path: {} · {count} installed",
            record.path.display()
        ));
    }
    Ok(lines.join("\n"))
}

/// A marketplace and its plugins, shaped for the interactive `/marketplaces`
/// browser. Manifests are read best-effort, so a broken checkout still shows a
/// row with an error instead of hiding the marketplace entirely.
#[derive(Debug, Clone)]
pub struct MarketplaceOverview {
    pub name: String,
    pub source: String,
    pub path: PathBuf,
    pub owner: Option<String>,
    pub plugins: Vec<MarketplacePluginOverview>,
    pub error: Option<String>,
}

/// One plugin a marketplace offers, with its local install state.
#[derive(Debug, Clone)]
pub struct MarketplacePluginOverview {
    pub name: String,
    pub description: Option<String>,
    pub version: Option<String>,
    pub installed: bool,
    pub enabled: bool,
}

/// Loads every configured marketplace with its available plugins and their
/// install state, for the `/marketplaces` overlay.
pub fn marketplace_overview() -> Result<Vec<MarketplaceOverview>> {
    marketplace_overview_in(&install_root())
}

fn marketplace_overview_in(root: &Path) -> Result<Vec<MarketplaceOverview>> {
    let state = load_state_in(root)?;
    let marketplaces = state
        .marketplaces
        .values()
        .map(|record| {
            let (manifest, error) = match marketplace_manifest(&record.path) {
                Ok(manifest) => (manifest, None),
                Err(err) => (MarketplaceManifest::default(), Some(format!("{err:#}"))),
            };
            let plugins = manifest
                .plugins
                .iter()
                .map(|entry| {
                    let installed = state.plugins.get(&entry.name);
                    MarketplacePluginOverview {
                        name: entry.name.clone(),
                        description: entry
                            .description
                            .clone()
                            .or_else(|| installed.and_then(|plugin| plugin.description.clone())),
                        version: entry
                            .version
                            .clone()
                            .or_else(|| installed.and_then(|plugin| plugin.version.clone())),
                        installed: installed.is_some(),
                        enabled: installed.map(|plugin| plugin.enabled).unwrap_or(false),
                    }
                })
                .collect();
            MarketplaceOverview {
                name: record.name.clone(),
                source: record.source.clone(),
                path: record.path.clone(),
                owner: manifest.owner.as_ref().and_then(|owner| owner.name.clone()),
                plugins,
                error,
            }
        })
        .collect();
    Ok(marketplaces)
}

// ---------------------------------------------------------------------------
// Plugins
// ---------------------------------------------------------------------------

/// Finds the plugin entry and its marketplace record for a name, optionally
/// restricted to a specific marketplace.
fn find_marketplace_plugin<'a>(
    state: &'a PluginState,
    name: &str,
    marketplace: Option<&str>,
) -> Option<(&'a MarketplaceRecord, MarketplacePlugin)> {
    if let Some(marketplace_name) = marketplace {
        let record = state.marketplaces.get(marketplace_name)?;
        let manifest = marketplace_manifest(&record.path).ok()?;
        let plugin = manifest.plugins.iter().find(|plugin| plugin.name == name)?;
        return Some((record, plugin.clone()));
    }
    for record in state.marketplaces.values() {
        let Ok(manifest) = marketplace_manifest(&record.path) else {
            continue;
        };
        if let Some(plugin) = manifest.plugins.iter().find(|plugin| plugin.name == name) {
            return Some((record, plugin.clone()));
        }
    }
    None
}

/// Installs a plugin from a known marketplace (git URL or local path source).
/// Returns a human-readable summary.
pub async fn install(name: &str, marketplace: Option<&str>) -> Result<String> {
    install_in(&install_root(), name, marketplace).await
}

async fn install_in(root: &Path, name: &str, marketplace: Option<&str>) -> Result<String> {
    let state = load_state_in(root)?;
    let Some((record, entry)) = find_marketplace_plugin(&state, name, marketplace) else {
        if let Some(marketplace) = marketplace {
            if !state.marketplaces.contains_key(marketplace) {
                bail!(
                    "marketplace `{marketplace}` is not configured — add it with \
                     `oxide plugin marketplace add <url|owner/repo>`"
                );
            }
            bail!("no plugin named `{name}` in marketplace `{marketplace}`");
        }
        bail!(
            "no plugin named `{name}` in the configured marketplaces — add one with \
             `oxide plugin marketplace add <url|owner/repo>`"
        );
    };
    let source = source_string(&entry.source)
        .with_context(|| format!("plugin `{name}` has no `source` in its marketplace entry"))?;

    let dest = root.join(sanitize(&record.name)).join(sanitize(name));
    if is_git_source(&source) {
        git_clone(&source, &dest).await?;
    } else {
        let source_path = PathBuf::from(&source);
        let source_path = if source_path.is_absolute() {
            source_path
        } else {
            record.path.join(source_path)
        };
        if !source_path.is_dir() {
            bail!("plugin source `{source}` does not exist");
        }
        copy_dir(&source_path, &dest)?;
    }

    let manifest = plugin_manifest(&dest).with_context(|| {
        format!("plugin `{name}` is missing `.oxide/plugin.json` or `.claude-plugin/plugin.json`")
    })?;

    let mut state = load_state_in(root)?;
    state.plugins.insert(
        name.to_string(),
        InstalledPlugin {
            name: name.to_string(),
            marketplace: Some(record.name.clone()),
            description: manifest.description.clone(),
            version: manifest.version.clone(),
            enabled: true,
            path: dest,
        },
    );
    save_state_in(root, &state)?;

    let mut summary = format!("installed plugin `{name}`");
    if let Some(version) = &manifest.version {
        summary.push_str(&format!(" v{version}"));
    }
    if let Some(description) = &manifest.description {
        summary.push_str(&format!(" — {description}"));
    }
    summary.push_str(" (restart to load hooks and MCP servers)");
    Ok(summary)
}

/// Resolves an installed plugin by `name` or `name@marketplace`, verifying the
/// marketplace when the reference names one. Installed plugins are keyed by
/// name alone, so the bare form always matches.
fn find_installed<'a>(
    state: &'a PluginState,
    reference: &str,
) -> Option<(&'a String, &'a InstalledPlugin)> {
    let (name, marketplace) = split_ref(reference);
    let (key, installed) = state.plugins.get_key_value(&name)?;
    if let Some(marketplace) = marketplace {
        if installed.marketplace.as_deref() != Some(marketplace.as_str()) {
            return None;
        }
    }
    Some((key, installed))
}

pub fn uninstall(reference: &str) -> Result<String> {
    uninstall_in(&install_root(), reference)
}

fn uninstall_in(root: &Path, reference: &str) -> Result<String> {
    let mut state = load_state_in(root)?;
    let Some((name, installed)) = find_installed(&state, reference)
        .map(|(name, installed)| (name.clone(), installed.clone()))
    else {
        bail!("no installed plugin named `{reference}`");
    };
    state.plugins.remove(&name);
    save_state_in(root, &state)?;
    if installed.path.starts_with(root) {
        std::fs::remove_dir_all(&installed.path).ok();
        if let Some(parent) = installed.path.parent() {
            if parent != root {
                std::fs::remove_dir(parent).ok();
            }
        }
    }
    Ok(format!("uninstalled plugin `{name}`"))
}

pub fn set_enabled(reference: &str, enabled: bool) -> Result<String> {
    set_enabled_in(&install_root(), reference, enabled)
}

fn set_enabled_in(root: &Path, reference: &str, enabled: bool) -> Result<String> {
    let mut state = load_state_in(root)?;
    let Some(name) = find_installed(&state, reference).map(|(name, _)| name.clone()) else {
        bail!("no installed plugin named `{reference}`");
    };
    let Some(installed) = state.plugins.get_mut(&name) else {
        bail!("no installed plugin named `{reference}`");
    };
    installed.enabled = enabled;
    save_state_in(root, &state)?;
    Ok(format!(
        "plugin `{name}` {}",
        if enabled { "enabled" } else { "disabled" }
    ))
}

pub fn list() -> Result<String> {
    list_in(&install_root())
}

fn list_in(root: &Path) -> Result<String> {
    let state = load_state_in(root)?;
    if state.plugins.is_empty() {
        return Ok(
            "no plugins installed — use `oxide plugin install <name>@<marketplace>`".to_string(),
        );
    }
    let mut lines = vec![format!("plugins ({}):", state.plugins.len())];
    for installed in state.plugins.values() {
        let status = if installed.enabled {
            "enabled"
        } else {
            "disabled"
        };
        let version = installed
            .version
            .as_deref()
            .map(|version| format!(" v{version}"))
            .unwrap_or_default();
        let marketplace = installed
            .marketplace
            .as_deref()
            .map(|marketplace| format!("@{marketplace}"))
            .unwrap_or_default();
        lines.push(format!(
            "  {}{} — {}{}",
            installed.name, marketplace, status, version
        ));
        if let Some(description) = &installed.description {
            lines.push(format!("      {}", description));
        }
        lines.push(format!("      path: {}", installed.path.display()));
    }
    Ok(lines.join("\n"))
}

// ---------------------------------------------------------------------------
// Hook shim
// ---------------------------------------------------------------------------

/// Generates (and caches) a JS shim that translates Claude Code command hooks
/// declared in a plugin manifest into the oxide hook-host protocol. Returns the
/// shim path when the manifest declares hooks, otherwise `None`.
pub fn hook_shim_path(name: &str, manifest: &PluginManifest) -> Option<PathBuf> {
    let hooks = manifest.hooks.as_ref()?;
    let object = hooks.as_object()?;
    if object.is_empty() {
        return None;
    }
    let source = hook_shim_source(hooks)?;
    let dir = install_root().join("generated");
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join(format!("{}.mjs", sanitize(name)));
    std::fs::write(&path, source).ok()?;
    Some(path)
}

const HOOK_SHIM_TEMPLATE: &str = r#"import { spawn } from "node:child_process";

export default async () => {
  const hooks = __HOOKS__;

  const run = (command, stdinText) => new Promise((resolve) => {
    const child = spawn(command, { shell: true, cwd: process.cwd() });
    let stdout = "";
    child.stdout.on("data", (chunk) => { stdout += chunk; });
    child.stderr.on("data", () => {});
    child.on("close", () => resolve(stdout));
    child.on("error", () => resolve(""));
    child.stdin.write(stdinText);
    child.stdin.end();
  });

  const apply = async (event, input, output) => {
    const entries = hooks[event];
    if (!Array.isArray(entries)) return;
    for (const entry of entries) {
      const matcher = entry.matcher || ".*";
      let re;
      try { re = new RegExp(matcher); } catch { continue; }
      if (!re.test(input.tool)) continue;
      for (const hook of entry.hooks || []) {
        const type = hook.type || "command";
        if (type !== "command" || !hook.command) continue;
        const stdinText = JSON.stringify({
          session_id: "oxide",
          transcript_path: "",
          cwd: process.cwd(),
          hook_event_name: event,
          tool_name: input.tool,
          tool_input: input.args || {},
        });
        const stdout = await run(hook.command, stdinText);
        const trimmed = stdout.trim();
        if (!trimmed) continue;
        let parsed;
        try { parsed = JSON.parse(trimmed); } catch { continue; }
        const specific = parsed.hookSpecificOutput || {};
        if (event === "PreToolUse") {
          const updated = specific.updatedInput;
          if (updated && typeof updated === "object") output.args = updated;
        } else if (event === "PostToolUse") {
          if (parsed.decision === "block") output.terminate = true;
          const extra = specific.additionalContext;
          if (typeof extra === "string" && extra.length > 0) {
            output.output = output.output ? `${output.output}\n${extra}` : extra;
          }
        }
      }
    }
  };

  return {
    "tool.execute.before": (input, output) => apply("PreToolUse", input, output),
    "tool.execute.after": (input, output) => apply("PostToolUse", input, output),
  };
};
"#;

fn hook_shim_source(hooks: &serde_json::Value) -> Option<String> {
    let json = serde_json::to_string(hooks).ok()?;
    Some(HOOK_SHIM_TEMPLATE.replace("__HOOKS__", &json))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_dir(tag: &str) -> PathBuf {
        let unique = format!(
            "oxide-plugin-registry-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_marketplace(dir: &Path, name: &str, plugins: &[(&str, &str)]) {
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        let list: Vec<serde_json::Value> = plugins
            .iter()
            .map(|(name, source)| serde_json::json!({"name": name, "source": source}))
            .collect();
        std::fs::write(
            dir.join(".claude-plugin/marketplace.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "name": name,
                "plugins": list,
            }))
            .unwrap(),
        )
        .unwrap();
    }

    fn write_plugin(dir: &Path, name: &str, extra: serde_json::Value) {
        std::fs::create_dir_all(dir.join(".claude-plugin")).unwrap();
        let mut manifest = serde_json::json!({
            "name": name,
            "version": "1.2.3",
            "description": "a test plugin",
        });
        if let Some(object) = manifest.as_object_mut() {
            for (key, value) in extra.as_object().unwrap_or(&serde_json::Map::new()) {
                object.insert(key.clone(), value.clone());
            }
        }
        std::fs::write(
            dir.join(".claude-plugin/plugin.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn parses_source_strings_and_objects() {
        assert_eq!(
            source_string(&serde_json::json!("https://github.com/a/b.git")),
            Some("https://github.com/a/b.git".to_string())
        );
        assert_eq!(
            source_string(&serde_json::json!({"source": "github", "repo": "a/b"})),
            Some("a/b".to_string())
        );
        assert_eq!(
            source_string(&serde_json::json!({"url": "a/b"})),
            Some("a/b".to_string())
        );
        assert_eq!(source_string(&serde_json::json!(5)), None);
    }

    #[test]
    fn detects_git_sources_and_names() {
        assert!(is_git_source("https://github.com/a/b.git"));
        assert!(is_git_source("git@github.com:a/b.git"));
        assert!(is_git_source("ssh://git@github.com/a/b"));
        assert!(!is_git_source("/tmp/some/path"));
        assert!(!is_git_source("relative/dir"));
        assert_eq!(git_name("https://github.com/a/b.git"), "b");
        assert_eq!(git_name("https://github.com/a/b/"), "b");
    }

    #[test]
    fn expands_github_shorthand_and_leaves_paths_alone() {
        assert_eq!(
            expand_source("Skyscanner/skyscanner-claude-plugins"),
            "https://github.com/Skyscanner/skyscanner-claude-plugins.git"
        );
        assert_eq!(
            expand_source("https://github.com/a/b.git"),
            "https://github.com/a/b.git"
        );
        assert_eq!(expand_source("./local"), "./local");
        assert_eq!(expand_source("../local"), "../local");
        assert_eq!(expand_source("/abs/path"), "/abs/path");
        assert_eq!(expand_source("~/mp"), "~/mp");
        assert_eq!(expand_source("plain"), "plain");
        assert_eq!(expand_source("a/b/c"), "a/b/c");
    }

    #[tokio::test]
    async fn install_names_an_unconfigured_marketplace() {
        let root = temp_dir("missing-mp");
        let err = install_in(&root, "hello", Some("skyscanner-claude-plugins"))
            .await
            .unwrap_err();
        let text = format!("{err:#}");
        assert!(text.contains("not configured"), "{text}");
        assert!(text.contains("oxide plugin marketplace add"), "{text}");
        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn update_flags_unknown_and_local_marketplaces() {
        let root = temp_dir("update-local");
        let err = update_marketplace_in(&root, "nope").await.unwrap_err();
        assert!(format!("{err:#}").contains("no marketplace named"));

        let dir = root.join("local-mp");
        write_marketplace(&dir, "local-mp", &[("hello", "../plugin-src")]);
        add_marketplace_in(&root, dir.to_str().unwrap())
            .await
            .unwrap();
        let summary = update_marketplace_in(&root, "local-mp").await.unwrap();
        assert!(summary.contains("local directory"), "{summary}");

        std::fs::remove_dir_all(&root).ok();
    }

    fn git_available() -> bool {
        std::process::Command::new("git")
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    fn git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("running git");
        assert!(status.success(), "git {args:?} failed");
    }

    #[tokio::test]
    async fn updates_a_git_marketplace_from_its_remote() {
        if !git_available() {
            return;
        }
        let root = temp_dir("update-git");
        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(&work, &["init", "-q", "-b", "main", "."]);
        git(&work, &["config", "user.email", "t@example.com"]);
        git(&work, &["config", "user.name", "tester"]);
        write_marketplace(&work, "test-mp", &[("hello", "../plugin-src")]);
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "first"]);

        let remote = root.join("remote.git");
        let status = std::process::Command::new("git")
            .args(["clone", "--bare", "-q"])
            .arg(&work)
            .arg(&remote)
            .status()
            .unwrap();
        assert!(status.success());

        // Clone once at the first commit, then push a second plugin upstream.
        let source = remote.to_string_lossy().to_string();
        add_marketplace_in(&root, &source).await.unwrap();
        assert_eq!(marketplace_overview_in(&root).unwrap()[0].plugins.len(), 1);

        write_marketplace(
            &work,
            "test-mp",
            &[("hello", "../plugin-src"), ("world", "../plugin-src")],
        );
        git(&work, &["add", "-A"]);
        git(&work, &["commit", "-qm", "second"]);
        git(&work, &["remote", "add", "origin", &source]);
        git(&work, &["push", "-q", "origin", "main"]);

        let summary = update_marketplace_in(&root, "test-mp").await.unwrap();
        assert!(
            summary.contains("updated marketplace `test-mp`"),
            "{summary}"
        );
        assert_eq!(marketplace_overview_in(&root).unwrap()[0].plugins.len(), 2);

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn sanitizes_directory_names() {
        assert_eq!(sanitize("owner/my-plugin"), "owner_my-plugin");
        assert_eq!(sanitize("plain"), "plain");
        assert_eq!(sanitize("///"), "___");
        assert_eq!(sanitize(""), "plugin");
    }

    #[test]
    fn splits_plugin_references() {
        assert_eq!(
            split_ref("name@marketplace"),
            ("name".to_string(), Some("marketplace".to_string()))
        );
        assert_eq!(split_ref("name"), ("name".to_string(), None));
        assert_eq!(split_ref("name@"), ("name@".to_string(), None));
    }

    #[test]
    fn hook_shim_generation_embeds_hooks_json() {
        let hooks = serde_json::json!({
            "PostToolUse": [{
                "matcher": "Write|Edit",
                "hooks": [{"type": "command", "command": "fmt"}]
            }]
        });
        let source = hook_shim_source(&hooks).unwrap();
        assert!(source.contains(r#""PostToolUse""#));
        assert!(source.contains("tool.execute.before"));
        assert!(source.contains("tool.execute.after"));
        assert!(source.contains("fmt"));
        assert!(!source.contains("__HOOKS__"));
    }

    #[test]
    fn manifest_and_marketplace_round_trip() {
        let dir = temp_dir("manifests");
        write_plugin(
            &dir.join("p"),
            "p",
            serde_json::json!({"mcpServers": {"fs": {"command": "npx"}}}),
        );
        let manifest = plugin_manifest(&dir.join("p")).unwrap();
        assert_eq!(manifest.name, "p");
        assert_eq!(manifest.version.as_deref(), Some("1.2.3"));
        assert!(manifest.mcp_servers.is_some());

        write_marketplace(&dir.join("m"), "m", &[("p", "./p")]);
        let marketplace = marketplace_manifest(&dir.join("m")).unwrap();
        assert_eq!(marketplace.name, "m");
        assert_eq!(marketplace.plugins.len(), 1);
        assert_eq!(marketplace.plugins[0].name, "p");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn reads_oxide_native_manifests_and_prefers_them() {
        let dir = temp_dir("oxide-manifest");
        let pkg = dir.join("pkg");
        std::fs::create_dir_all(pkg.join(".oxide")).unwrap();
        std::fs::create_dir_all(pkg.join(".claude-plugin")).unwrap();
        std::fs::write(
            pkg.join(".oxide/plugin.json"),
            r#"{"name":"oxide-name","version":"9.9.9"}"#,
        )
        .unwrap();
        std::fs::write(
            pkg.join(".claude-plugin/plugin.json"),
            r#"{"name":"claude-name"}"#,
        )
        .unwrap();

        let manifest = plugin_manifest(&pkg).unwrap();
        assert_eq!(manifest.name, "oxide-name");
        assert_eq!(manifest.version.as_deref(), Some("9.9.9"));

        // Claude Code-only packages still load.
        let claude_only = dir.join("claude-only");
        std::fs::create_dir_all(claude_only.join(".claude-plugin")).unwrap();
        std::fs::write(
            claude_only.join(".claude-plugin/plugin.json"),
            r#"{"name":"claude-only"}"#,
        )
        .unwrap();
        assert_eq!(plugin_manifest(&claude_only).unwrap().name, "claude-only");

        // Marketplace manifests follow the same lookup.
        let mp = dir.join("mp");
        std::fs::create_dir_all(mp.join(".oxide")).unwrap();
        std::fs::write(
            mp.join(".oxide/marketplace.json"),
            r#"{"name":"oxide-mp","plugins":[]}"#,
        )
        .unwrap();
        assert_eq!(marketplace_manifest(&mp).unwrap().name, "oxide-mp");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn copies_local_plugin_directories() {
        let dir = temp_dir("copy");
        std::fs::create_dir_all(dir.join("src/sub")).unwrap();
        std::fs::write(dir.join("src/a.txt"), "a").unwrap();
        std::fs::write(dir.join("src/sub/b.txt"), "b").unwrap();

        copy_dir(&dir.join("src"), &dir.join("dst")).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("dst/a.txt")).unwrap(), "a");
        assert_eq!(
            std::fs::read_to_string(dir.join("dst/sub/b.txt")).unwrap(),
            "b"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn installs_lists_disables_and_uninstalls_local_plugins() {
        let root = temp_dir("install");
        let marketplace_dir = root.join("marketplace-src");
        let plugin_dir = root.join("plugin-src");
        write_marketplace(&marketplace_dir, "test-mp", &[("hello", "../plugin-src")]);
        write_plugin(&plugin_dir, "hello", serde_json::json!({}));

        // The marketplace source is a local path, resolved relative to root.
        let summary = add_marketplace_in(&root, marketplace_dir.to_str().unwrap())
            .await
            .unwrap();
        assert!(summary.contains("added marketplace `test-mp`"));

        let summary = install_in(&root, "hello", None).await.unwrap();
        assert!(summary.contains("installed plugin `hello`"));
        assert!(summary.contains("v1.2.3"));

        let state = load_state_in(&root).unwrap();
        assert!(state.plugins.contains_key("hello"));
        assert!(state.plugins["hello"].enabled);

        let text = list_in(&root).unwrap();
        assert!(text.contains("hello"));
        assert!(text.contains("enabled"));
        assert!(text.contains("@test-mp"));

        assert!(set_enabled_in(&root, "hello@test-mp", false).is_ok());
        assert!(!load_state_in(&root).unwrap().plugins["hello"].enabled);
        assert!(set_enabled_in(&root, "hello@other-mp", true).is_err());

        assert!(uninstall_in(&root, "hello@other-mp").is_err());
        assert!(uninstall_in(&root, "hello@test-mp").is_ok());
        assert!(load_state_in(&root).unwrap().plugins.is_empty());
        assert!(!state.plugins["hello"].path.exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn marketplace_overview_reports_plugins_and_install_state() {
        let root = temp_dir("overview");
        let marketplace_dir = root.join("marketplace-src");
        let plugin_dir = root.join("plugin-src");
        write_marketplace(
            &marketplace_dir,
            "test-mp",
            &[("hello", "../plugin-src"), ("other", "../plugin-src")],
        );
        write_plugin(&plugin_dir, "hello", serde_json::json!({}));

        add_marketplace_in(&root, marketplace_dir.to_str().unwrap())
            .await
            .unwrap();
        install_in(&root, "hello", None).await.unwrap();

        let overview = marketplace_overview_in(&root).unwrap();
        assert_eq!(overview.len(), 1);
        let marketplace = &overview[0];
        assert_eq!(marketplace.name, "test-mp");
        assert_eq!(marketplace.plugins.len(), 2);

        let hello = marketplace
            .plugins
            .iter()
            .find(|plugin| plugin.name == "hello")
            .expect("hello plugin");
        assert!(hello.installed);
        assert!(hello.enabled);
        assert_eq!(hello.version.as_deref(), Some("1.2.3"));
        assert_eq!(hello.description.as_deref(), Some("a test plugin"));

        let other = marketplace
            .plugins
            .iter()
            .find(|plugin| plugin.name == "other")
            .expect("other plugin");
        assert!(!other.installed);
        assert!(!other.enabled);

        std::fs::remove_dir_all(&root).ok();
    }

    #[tokio::test]
    async fn removing_a_marketplace_removes_its_plugins() {
        let root = temp_dir("remove-mp");
        let marketplace_dir = root.join("marketplace-src");
        let plugin_dir = root.join("plugin-src");
        write_marketplace(&marketplace_dir, "test-mp", &[("hello", "../plugin-src")]);
        write_plugin(&plugin_dir, "hello", serde_json::json!({}));

        add_marketplace_in(&root, marketplace_dir.to_str().unwrap())
            .await
            .unwrap();
        install_in(&root, "hello", None).await.unwrap();

        let summary = remove_marketplace_in(&root, "test-mp").unwrap();
        assert!(summary.contains("removed marketplace `test-mp`"));
        assert!(load_state_in(&root).unwrap().plugins.is_empty());

        std::fs::remove_dir_all(&root).ok();
    }
}
