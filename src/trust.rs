//! Project trust. A repository may contain project-local resources (agents,
//! commands, prompts, skills, plugins, SYSTEM.md) that change how the agent
//! behaves or execute code. Before those are loaded, the user approves the
//! project once; the decision is stored per directory in `trust.json` and the
//! closest matching ancestor applies, matching Pi's model.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

const TRUST_FILE: &str = "trust.json";

/// Fallback behavior when no saved decision applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DefaultTrust {
    /// Prompt interactively; ignore project resources in non-interactive modes.
    #[default]
    Ask,
    /// Trust project resources without prompting.
    Always,
    /// Never load project resources unless approved for this run.
    Never,
}

impl DefaultTrust {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "ask" => Some(Self::Ask),
            "always" | "trust" => Some(Self::Always),
            "never" | "deny" => Some(Self::Never),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Ask => "ask",
            Self::Always => "always",
            Self::Never => "never",
        }
    }
}

/// Saved directory decisions. `true` = trusted, `false` = declined.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TrustStore {
    decisions: BTreeMap<String, bool>,
}

impl TrustStore {
    pub fn path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("oxide")
            .join(TRUST_FILE)
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading trust decisions at {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing trust decisions at {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path())
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        std::fs::write(path, text)
            .with_context(|| format!("writing trust decisions to {}", path.display()))?;
        Ok(())
    }

    pub fn set(&mut self, dir: &Path, trusted: bool) {
        self.decisions.insert(canonical(dir), trusted);
    }

    /// The closest saved decision for `dir` or one of its ancestors.
    pub fn decision(&self, dir: &Path) -> Option<bool> {
        let mut current = Some(dir.to_path_buf());
        while let Some(path) = current {
            if let Some(decision) = self.decisions.get(&canonical(&path)) {
                return Some(*decision);
            }
            current = path.parent().map(Path::to_path_buf);
        }
        None
    }
}

fn canonical(dir: &Path) -> String {
    dir.canonicalize()
        .unwrap_or_else(|_| dir.to_path_buf())
        .to_string_lossy()
        .to_string()
}

/// The resolved trust decision for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    Trusted,
    Untrusted,
}

impl Trust {
    pub fn is_trusted(self) -> bool {
        matches!(self, Trust::Trusted)
    }
}

/// Resolves trust for `cwd` without prompting. `Approve`/`Decline` are the CLI
/// overrides; otherwise a saved decision applies, then `default`. Interactive
/// callers handle [`DefaultTrust::Ask`] separately with [`needs_prompt`].
/// Resolves trust without prompting: an explicit override wins, then a saved
/// decision, then `default`. Interactive callers handle [`DefaultTrust::Ask`]
/// themselves with the trust prompt, so `Ask` resolves to untrusted here.
pub fn resolve(
    store: &TrustStore,
    cwd: &Path,
    override_decision: Option<bool>,
    default: DefaultTrust,
) -> Trust {
    if let Some(decision) = override_decision {
        return if decision {
            Trust::Trusted
        } else {
            Trust::Untrusted
        };
    }
    if let Some(decision) = store.decision(cwd) {
        return if decision {
            Trust::Trusted
        } else {
            Trust::Untrusted
        };
    }
    match default {
        DefaultTrust::Always => Trust::Trusted,
        DefaultTrust::Ask | DefaultTrust::Never => Trust::Untrusted,
    }
}

/// True when the project has resources that require trust.
pub fn requires_trust(cwd: &Path) -> bool {
    let Some(root) = crate::ecosystem::project_root(cwd) else {
        return false;
    };
    let oxide = root.join(".oxide");
    let claude = root.join(".claude");
    if oxide.join("SYSTEM.md").is_file()
        || oxide.join("APPEND_SYSTEM.md").is_file()
        || has_entries(&oxide.join("plugins"))
        || has_entries(&oxide.join("agents"))
        || has_entries(&oxide.join("commands"))
        || has_entries(&oxide.join("prompts"))
        || has_entries(&oxide.join("skills"))
    {
        return true;
    }
    if claude.join("SYSTEM.md").is_file()
        || claude.join("APPEND_SYSTEM.md").is_file()
        || has_entries(&claude.join("plugins"))
        || has_entries(&claude.join("agents"))
        || has_entries(&claude.join("commands"))
        || has_entries(&claude.join("prompts"))
        || has_entries(&claude.join("skills"))
    {
        return true;
    }
    false
}

fn has_entries(dir: &Path) -> bool {
    std::fs::read_dir(dir)
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false)
}

/// The number of project-local resources gated behind trust, for the prompt.
pub fn project_resources(cwd: &Path) -> Vec<String> {
    let Some(root) = crate::ecosystem::project_root(cwd) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (dir, label) in [
        (root.join(".oxide"), "oxide"),
        (root.join(".claude"), "claude"),
    ] {
        for (sub, name) in [
            ("agents", "agents"),
            ("commands", "commands"),
            ("prompts", "prompts"),
            ("skills", "skills"),
            ("plugins", "plugins"),
        ] {
            if has_entries(&dir.join(sub)) {
                found.push(format!("{label}/{name}"));
            }
        }
        for file in ["SYSTEM.md", "APPEND_SYSTEM.md"] {
            if dir.join(file).is_file() {
                found.push(format!("{label}/{file}"));
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "oxide-trust-{tag}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn closest_ancestor_decision_wins() {
        let root = temp_dir("ancestor");
        let child = root.join("a/b");
        std::fs::create_dir_all(&child).unwrap();

        let mut store = TrustStore::default();
        store.set(&root, true);
        assert_eq!(store.decision(&child), Some(true));

        store.set(&child, false);
        assert_eq!(store.decision(&child), Some(false));
        assert_eq!(store.decision(&root), Some(true));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn resolve_precedence() {
        let dir = temp_dir("resolve");
        let mut store = TrustStore::default();
        store.set(&dir, false);

        // Saved decision beats the default.
        assert_eq!(
            resolve(&store, &dir, None, DefaultTrust::Always),
            Trust::Untrusted
        );
        // Explicit override beats the saved decision and the default.
        assert_eq!(
            resolve(&store, &dir, Some(true), DefaultTrust::Never),
            Trust::Trusted
        );
        assert_eq!(
            resolve(&store, &dir, Some(false), DefaultTrust::Always),
            Trust::Untrusted
        );

        let mut trusted_store = TrustStore::default();
        trusted_store.set(&dir, true);
        assert_eq!(
            resolve(&trusted_store, &dir, None, DefaultTrust::Never),
            Trust::Trusted
        );

        // No saved decision: the default applies.
        let empty = TrustStore::default();
        assert_eq!(
            resolve(&empty, &dir, None, DefaultTrust::Always),
            Trust::Trusted
        );
        assert_eq!(
            resolve(&empty, &dir, None, DefaultTrust::Never),
            Trust::Untrusted
        );
        assert_eq!(
            resolve(&empty, &dir, None, DefaultTrust::Ask),
            Trust::Untrusted
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn detects_project_resources() {
        let root = temp_dir("resources");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert!(!requires_trust(&root));

        std::fs::create_dir_all(root.join(".oxide/commands")).unwrap();
        std::fs::write(root.join(".oxide/commands/build.md"), "run").unwrap();
        assert!(requires_trust(&root));
        assert!(project_resources(&root)
            .iter()
            .any(|r| r == "oxide/commands"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn claude_plugins_require_trust() {
        let root = temp_dir("claude_plugins");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        assert!(!requires_trust(&root));

        std::fs::create_dir_all(root.join(".claude/plugins")).unwrap();
        std::fs::write(root.join(".claude/plugins/hook.js"), "export default 1").unwrap();
        assert!(requires_trust(&root));
        assert!(project_resources(&root)
            .iter()
            .any(|r| r == "claude/plugins"));

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn default_trust_parses() {
        assert_eq!(DefaultTrust::parse("ask"), Some(DefaultTrust::Ask));
        assert_eq!(DefaultTrust::parse("ALWAYS"), Some(DefaultTrust::Always));
        assert_eq!(DefaultTrust::parse("never"), Some(DefaultTrust::Never));
        assert_eq!(DefaultTrust::parse("bogus"), None);
    }
}
