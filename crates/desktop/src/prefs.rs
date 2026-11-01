//! The window's own preferences.
//!
//! They live beside `projects.json` — `<config>/Oxide/desktop/prefs.json` — so
//! the folder list and the way the window behaves travel together, and a test
//! can point the store at a directory of its own. The file is read once at
//! launch and written when a control in the settings sheet changes something.
//!
//! A key this build does not know is kept as it is, so a preference written by
//! a newer window survives an older one saving; a known key holding a value the
//! window cannot use is refused, naming the key, rather than quietly replaced.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// How large the window draws itself. Electron's zoom factor, so this is the
/// scale rather than a point size: 1.0 is the window as it was drawn.
pub const MIN_FONT_SCALE: f64 = 0.7;
pub const MAX_FONT_SCALE: f64 = 1.6;

/// What a press of the send key does: `enter` sends and Shift+Enter is a
/// newline, or `mod-enter` sends and Enter is a newline.
pub const SEND_KEYS: [&str; 2] = ["enter", "mod-enter"];

/// What a message sent while a turn is running does: wait for the next run, or
/// steer the one in flight.
pub const FOLLOW_UP_BEHAVIORS: [&str; 2] = ["queue", "steer"];

/// The action a shortcut is bound to, in the shape both halves of the app use.
/// The window's own table is the list of these; this side only carries the
/// reader's overrides, so a name that means nothing yet is still kept.
pub type Shortcuts = BTreeMap<String, String>;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Prefs {
    #[serde(default = "default_send_key")]
    pub send_key: String,
    #[serde(default = "default_follow_up")]
    pub follow_up: String,
    #[serde(default = "default_font_scale")]
    pub font_scale: f64,
    #[serde(default = "default_true")]
    pub sidebar_visible: bool,
    /// Whether the composer offers the next message the reader is likely to send.
    /// Off by default: it is a model call after every turn.
    #[serde(default)]
    pub predictions: bool,
    #[serde(default)]
    pub shortcuts: Shortcuts,
    /// Everything this build does not name — a preference a newer window wrote.
    #[serde(flatten, default)]
    pub extra: Map<String, Value>,
}

fn default_send_key() -> String {
    "enter".to_string()
}

fn default_follow_up() -> String {
    "queue".to_string()
}

fn default_font_scale() -> f64 {
    1.0
}

fn default_true() -> bool {
    true
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            send_key: default_send_key(),
            follow_up: default_follow_up(),
            font_scale: default_font_scale(),
            sidebar_visible: true,
            predictions: false,
            shortcuts: Shortcuts::new(),
            extra: Map::new(),
        }
    }
}

impl Prefs {
    /// `<config>/Oxide/desktop/prefs.json`, the directory the project registry
    /// already lives in.
    pub fn path() -> Result<PathBuf> {
        let registry = oxide_core::workspaces::registry_path();
        Ok(registry
            .parent()
            .map(|dir| dir.join("prefs.json"))
            .unwrap_or_else(|| PathBuf::from("prefs.json")))
    }

    pub fn load() -> Result<Self> {
        Self::load_from(&Self::path()?)
    }

    /// A store that is absent is a window nobody has configured yet, which is
    /// the defaults rather than an error.
    pub fn load_from(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
        };
        let mut prefs: Prefs = serde_json::from_str(&text)
            .with_context(|| format!("parsing desktop preferences at {}", path.display()))?;
        prefs.settle();
        Ok(prefs)
    }

    pub fn save(&self) -> Result<()> {
        self.save_to(&Self::path()?)
    }

    pub fn save_to(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = serde_json::to_string_pretty(self)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, text).with_context(|| format!("writing {}", tmp.display()))?;
        std::fs::rename(&tmp, path).with_context(|| format!("replacing {}", path.display()))?;
        Ok(())
    }

    /// Takes a change from the window: the keys it names, in the order it names
    /// them, and nothing else. A shortcut set to `null` drops that override, so
    /// the action goes back to the binding it shipped with.
    pub fn apply(&mut self, patch: &Map<String, Value>) -> Result<()> {
        for (key, value) in patch {
            match key.as_str() {
                "send_key" => self.send_key = choice(key, value, &SEND_KEYS)?,
                "follow_up" => self.follow_up = choice(key, value, &FOLLOW_UP_BEHAVIORS)?,
                "font_scale" => self.font_scale = scale(value)?,
                "sidebar_visible" => self.sidebar_visible = boolean(key, value)?,
                "predictions" => self.predictions = boolean(key, value)?,
                "shortcuts" => self.set_shortcuts(value)?,
                _ => {
                    self.extra.insert(key.clone(), value.clone());
                }
            }
        }
        self.settle();
        Ok(())
    }

    fn set_shortcuts(&mut self, value: &Value) -> Result<()> {
        let Some(entries) = value.as_object() else {
            anyhow::bail!("invalid `shortcuts`: expected an object of action names");
        };
        for (action, chord) in entries {
            if chord.is_null() {
                self.shortcuts.remove(action);
                continue;
            }
            let Some(chord) = chord.as_str() else {
                anyhow::bail!("invalid `shortcuts.{action}`: expected a binding or null");
            };
            let chord = chord.trim();
            if chord.is_empty() {
                anyhow::bail!("invalid `shortcuts.{action}`: expected a binding or null");
            }
            self.shortcuts.insert(action.clone(), chord.to_string());
        }
        Ok(())
    }

    /// What a file on disk may say without the window refusing to open: a scale
    /// outside the range a hand edit could reach, or a word from a newer build,
    /// falls back to the value the window shipped with.
    fn settle(&mut self) {
        if !SEND_KEYS.contains(&self.send_key.as_str()) {
            self.send_key = default_send_key();
        }
        if !FOLLOW_UP_BEHAVIORS.contains(&self.follow_up.as_str()) {
            self.follow_up = default_follow_up();
        }
        if !self.font_scale.is_finite() {
            self.font_scale = default_font_scale();
        }
        self.font_scale = self.font_scale.clamp(MIN_FONT_SCALE, MAX_FONT_SCALE);
        self.shortcuts
            .retain(|action, chord| !action.trim().is_empty() && !chord.trim().is_empty());
    }
}

fn choice(key: &str, value: &Value, options: &[&str]) -> Result<String> {
    let Some(text) = value.as_str() else {
        anyhow::bail!("invalid `{key}`: expected one of {}", options.join(", "));
    };
    if !options.contains(&text) {
        anyhow::bail!("invalid `{key}`: expected one of {}", options.join(", "));
    }
    Ok(text.to_string())
}

fn scale(value: &Value) -> Result<f64> {
    let Some(number) = value.as_f64() else {
        anyhow::bail!("invalid `font_scale`: expected a number");
    };
    if !number.is_finite() {
        anyhow::bail!("invalid `font_scale`: expected a number");
    }
    Ok(number.clamp(MIN_FONT_SCALE, MAX_FONT_SCALE))
}

fn boolean(key: &str, value: &Value) -> Result<bool> {
    value
        .as_bool()
        .ok_or_else(|| anyhow::anyhow!("invalid `{key}`: expected true or false"))
}

/// The preferences as the window reads them, which is also what the settings
/// sheet is drawn from.
pub fn describe(prefs: &Prefs) -> Value {
    json!({
        "send_key": prefs.send_key,
        "follow_up": prefs.follow_up,
        "font_scale": prefs.font_scale,
        "sidebar_visible": prefs.sidebar_visible,
        "predictions": prefs.predictions,
        "shortcuts": prefs.shortcuts,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_path(name: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let serial = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "oxide_prefs_{}_{serial}_{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("desktop/prefs.json")
    }

    #[test]
    fn a_window_nobody_has_configured_reads_the_defaults() {
        let path = temp_path("absent");
        let prefs = Prefs::load_from(&path).unwrap();
        assert_eq!(prefs, Prefs::default());
        assert_eq!(prefs.send_key, "enter");
        assert_eq!(prefs.follow_up, "queue");
        assert_eq!(prefs.font_scale, 1.0);
        assert!(prefs.sidebar_visible);
        assert!(prefs.shortcuts.is_empty());
    }

    #[test]
    fn what_is_written_is_what_is_read_back() {
        let path = temp_path("roundtrip");
        let mut prefs = Prefs::default();
        prefs
            .apply(
                json!({
                    "send_key": "mod-enter",
                    "follow_up": "steer",
                    "font_scale": 1.25,
                    "sidebar_visible": false,
                    "shortcuts": { "command-menu": "mod+shift+k" },
                })
                .as_object()
                .unwrap(),
            )
            .unwrap();
        prefs.save_to(&path).unwrap();
        assert_eq!(Prefs::load_from(&path).unwrap(), prefs);
        assert_eq!(prefs.shortcuts["command-menu"], "mod+shift+k");
    }

    #[test]
    fn a_scale_outside_the_range_is_held_at_the_edge() {
        let mut prefs = Prefs::default();
        prefs
            .apply(json!({ "font_scale": 4.0 }).as_object().unwrap())
            .unwrap();
        assert_eq!(prefs.font_scale, MAX_FONT_SCALE);
        prefs
            .apply(json!({ "font_scale": 0.1 }).as_object().unwrap())
            .unwrap();
        assert_eq!(prefs.font_scale, MIN_FONT_SCALE);
    }

    #[test]
    fn a_value_the_window_cannot_use_is_refused_by_name() {
        let prefs = Prefs::default();
        let refused = |patch: Value| prefs.clone().apply(patch.as_object().unwrap());
        assert!(refused(json!({ "send_key": "sometimes" }))
            .unwrap_err()
            .to_string()
            .contains("send_key"));
        assert!(refused(json!({ "follow_up": 3 }))
            .unwrap_err()
            .to_string()
            .contains("follow_up"));
        assert!(refused(json!({ "sidebar_visible": "yes" }))
            .unwrap_err()
            .to_string()
            .contains("sidebar_visible"));
        assert!(refused(json!({ "shortcuts": [1, 2] }))
            .unwrap_err()
            .to_string()
            .contains("shortcuts"));
        // Nothing was taken from a patch that was refused halfway.
        assert_eq!(prefs, Prefs::default());
    }

    #[test]
    fn a_hand_edited_file_falls_back_rather_than_refusing_to_open() {
        let path = temp_path("hand-edited");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            json!({
                "send_key": "whatever-a-newer-build-calls-it",
                "follow_up": "queue",
                "font_scale": 9.0,
                "shortcuts": { "": "mod+k", "command-menu": "  " },
            })
            .to_string(),
        )
        .unwrap();
        let prefs = Prefs::load_from(&path).unwrap();
        assert_eq!(prefs.send_key, "enter");
        assert_eq!(prefs.font_scale, MAX_FONT_SCALE);
        assert!(prefs.shortcuts.is_empty());
    }

    #[test]
    fn a_key_a_newer_window_wrote_survives_a_save_from_this_one() {
        let path = temp_path("newer-key");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            json!({ "send_key": "enter", "notifications": "always" }).to_string(),
        )
        .unwrap();
        let mut prefs = Prefs::load_from(&path).unwrap();
        prefs
            .apply(json!({ "font_scale": 1.1 }).as_object().unwrap())
            .unwrap();
        prefs.save_to(&path).unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["notifications"], "always");
        assert_eq!(written["font_scale"], 1.1);
    }

    #[test]
    fn a_shortcut_set_to_null_goes_back_to_the_binding_it_shipped_with() {
        let mut prefs = Prefs::default();
        prefs
            .apply(
                json!({ "shortcuts": { "command-menu": "mod+shift+k", "settings": "mod+," } })
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        prefs
            .apply(
                json!({ "shortcuts": { "command-menu": null } })
                    .as_object()
                    .unwrap(),
            )
            .unwrap();
        assert_eq!(prefs.shortcuts.keys().collect::<Vec<_>>(), vec!["settings"]);
    }
}
