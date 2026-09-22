//! Semantic theme colors for front-ends other than the TUI.
//!
//! Reads the same `.oxide/themes/<name>.json` (project) and
//! `<config>/oxide/themes/<name>.json` (global) files the CLI uses, and
//! resolves every slot to a `#rrggbb` string so a web front-end can map them
//! onto CSS variables. The built-in `dark` and `light` palettes provide both
//! the surface colors (window, sidebar, panels) and the semantic slots; theme
//! files override individual slots on top of them.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// A theme resolved to CSS-ready colors: slot name → `#rrggbb`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeColors {
    pub name: String,
    pub colors: BTreeMap<String, String>,
}

const DARK: &[(&str, &str)] = &[
    ("background", "#1c1c1c"),
    ("sidebar", "#131313"),
    ("panel", "#202020"),
    ("panel_2", "#2a2a2a"),
    ("panel_3", "#303030"),
    ("border", "#303030"),
    ("text", "#ededed"),
    ("dim", "#9a9a9a"),
    ("faint", "#6f6f6f"),
    ("accent", "#a9c7ff"),
    ("user", "#a9c7ff"),
    ("assistant", "#ededed"),
    ("success", "#78d38a"),
    ("tool", "#e0b072"),
    ("error", "#f08a82"),
    ("info", "#9a9a9a"),
    ("tool_pending_bg", "#232323"),
    ("tool_success_bg", "#1e2a1e"),
    ("tool_error_bg", "#2a1e1e"),
    ("usage_bar_bg", "#1d3b78"),
    ("usage_bar_fg", "#ffffff"),
    ("usage_bar_label", "#a9c7ff"),
    ("thinking_off", "#9a9a9a"),
    ("thinking_low", "#a9c7ff"),
    ("thinking_medium", "#8ab4ff"),
    ("thinking_high", "#c79bff"),
    ("thinking_text", "#9a9a9a"),
];

const LIGHT: &[(&str, &str)] = &[
    ("background", "#ffffff"),
    ("sidebar", "#f6f6f8"),
    ("panel", "#ffffff"),
    ("panel_2", "#f0f0f3"),
    ("panel_3", "#e6e6ea"),
    ("border", "#dcdce1"),
    ("text", "#1b1b1f"),
    ("dim", "#5f5f68"),
    ("faint", "#9a9aa4"),
    ("accent", "#1a6fd4"),
    ("user", "#1a6fd4"),
    ("assistant", "#1b1b1f"),
    ("success", "#1a7f37"),
    ("tool", "#8a5a00"),
    ("error", "#c0392b"),
    ("info", "#5f5f68"),
    ("tool_pending_bg", "#f0f0f3"),
    ("tool_success_bg", "#eaf6ec"),
    ("tool_error_bg", "#fdecec"),
    ("usage_bar_bg", "#dbe8ff"),
    ("usage_bar_fg", "#1b1b1f"),
    ("usage_bar_label", "#1a6fd4"),
    ("thinking_off", "#5f5f68"),
    ("thinking_low", "#1a6fd4"),
    ("thinking_medium", "#0b57b8"),
    ("thinking_high", "#7a3fb5"),
    ("thinking_text", "#5f5f68"),
];

/// The serialized form of a custom theme. Values are color names (`"cyan"`) or
/// `#rrggbb`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ThemeFile {
    background: Option<String>,
    sidebar: Option<String>,
    panel: Option<String>,
    panel_2: Option<String>,
    panel_3: Option<String>,
    text: Option<String>,
    faint: Option<String>,
    accent: Option<String>,
    user: Option<String>,
    assistant: Option<String>,
    success: Option<String>,
    tool: Option<String>,
    error: Option<String>,
    info: Option<String>,
    dim: Option<String>,
    border: Option<String>,
    tool_pending_bg: Option<String>,
    tool_success_bg: Option<String>,
    tool_error_bg: Option<String>,
    usage_bar_bg: Option<String>,
    usage_bar_fg: Option<String>,
    usage_bar_label: Option<String>,
    thinking_off: Option<String>,
    thinking_low: Option<String>,
    thinking_medium: Option<String>,
    thinking_high: Option<String>,
    thinking_text: Option<String>,
}

impl ThemeFile {
    fn overrides(&self) -> BTreeMap<&'static str, String> {
        let slots: [(&'static str, &Option<String>); 27] = [
            ("background", &self.background),
            ("sidebar", &self.sidebar),
            ("panel", &self.panel),
            ("panel_2", &self.panel_2),
            ("panel_3", &self.panel_3),
            ("text", &self.text),
            ("faint", &self.faint),
            ("accent", &self.accent),
            ("user", &self.user),
            ("assistant", &self.assistant),
            ("success", &self.success),
            ("tool", &self.tool),
            ("error", &self.error),
            ("info", &self.info),
            ("dim", &self.dim),
            ("border", &self.border),
            ("tool_pending_bg", &self.tool_pending_bg),
            ("tool_success_bg", &self.tool_success_bg),
            ("tool_error_bg", &self.tool_error_bg),
            ("usage_bar_bg", &self.usage_bar_bg),
            ("usage_bar_fg", &self.usage_bar_fg),
            ("usage_bar_label", &self.usage_bar_label),
            ("thinking_off", &self.thinking_off),
            ("thinking_low", &self.thinking_low),
            ("thinking_medium", &self.thinking_medium),
            ("thinking_high", &self.thinking_high),
            ("thinking_text", &self.thinking_text),
        ];
        let mut out = BTreeMap::new();
        for (slot, value) in slots {
            if let Some(color) = parse_color(value.as_deref()) {
                out.insert(slot, color);
            }
        }
        out
    }
}

fn base(name: &str) -> Vec<(&'static str, &'static str)> {
    match name.trim().to_ascii_lowercase().as_str() {
        "light" => LIGHT.to_vec(),
        _ => DARK.to_vec(),
    }
}

/// Loads a theme by name: built-ins first, then the project and global theme
/// files, matching the CLI's resolution order.
/// The built-in palette for a name (`dark` or `light`), without reading any
/// theme files. Shared with the CLI so both front-ends use identical colors.
pub fn builtin(name: &str) -> ThemeColors {
    let trimmed = name.trim();
    let canonical = if trimmed.is_empty() { "dark" } else { trimmed };
    let colors = base(canonical)
        .into_iter()
        .map(|(slot, value)| (slot.to_string(), value.to_string()))
        .collect();
    ThemeColors {
        name: canonical.to_string(),
        colors,
    }
}

pub fn load(cwd: &Path, name: &str) -> ThemeColors {
    let trimmed = name.trim();
    let canonical = if trimmed.is_empty() { "dark" } else { trimmed };
    let mut theme = builtin(canonical);

    for path in [project_theme(cwd, canonical), global_theme(canonical)]
        .into_iter()
        .flatten()
    {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(file) = serde_json::from_str::<ThemeFile>(&text) else {
            continue;
        };
        for (slot, value) in file.overrides() {
            theme.colors.insert(slot.to_string(), value);
        }
    }

    theme
}

/// Theme names available for a picker: built-ins plus discovered files.
pub fn names(cwd: &Path) -> Vec<String> {
    let mut names = vec!["dark".to_string(), "light".to_string()];
    for dir in theme_dirs(cwd) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    if !names.iter().any(|name| name == stem) {
                        names.push(stem.to_string());
                    }
                }
            }
        }
    }
    names
}

fn theme_dirs(cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(root) = crate::ecosystem::project_root(cwd) {
        dirs.push(root.join(".oxide/themes"));
    }
    if let Some(config) = dirs::config_dir() {
        dirs.push(config.join("oxide/themes"));
    }
    dirs
}

fn project_theme(cwd: &Path, name: &str) -> Option<PathBuf> {
    let root = crate::ecosystem::project_root(cwd)?;
    let path = root.join(".oxide/themes").join(format!("{name}.json"));
    path.is_file().then_some(path)
}

fn global_theme(name: &str) -> Option<PathBuf> {
    let path = dirs::config_dir()?
        .join("oxide/themes")
        .join(format!("{name}.json"));
    path.is_file().then_some(path)
}

/// Parses a color name or `#rrggbb` value into a lowercase `#rrggbb` string.
pub fn parse_color(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    if let Some(hex) = value.strip_prefix('#') {
        if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Some(format!("#{}", hex.to_ascii_lowercase()));
        }
        return None;
    }
    let normalized = value.to_ascii_lowercase().replace(['-', '_'], "");
    let hex = match normalized.as_str() {
        "black" => "#000000",
        "red" => "#cd0000",
        "green" => "#00cd00",
        "yellow" => "#cdcd00",
        "blue" => "#0000ee",
        "magenta" | "purple" => "#cd00cd",
        "cyan" => "#00cdcd",
        "gray" | "grey" => "#e5e5e5",
        "darkgray" | "darkgrey" => "#7f7f7f",
        "lightred" => "#ff0000",
        "lightgreen" => "#00ff00",
        "lightyellow" => "#ffff00",
        "lightblue" => "#5c5cff",
        "lightmagenta" | "lightpurple" => "#ff00ff",
        "lightcyan" => "#00ffff",
        "white" => "#ffffff",
        _ => return None,
    };
    Some(hex.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_themes_resolve_and_stay_in_sync_slots() {
        let dark = load(Path::new("."), "dark");
        assert_eq!(dark.name, "dark");
        assert_eq!(
            dark.colors.get("accent").map(String::as_str),
            Some("#a9c7ff")
        );
        let light = load(Path::new("."), "light");
        assert_eq!(
            light.colors.get("accent").map(String::as_str),
            Some("#1a6fd4")
        );
        // Every slot present in both palettes.
        assert_eq!(dark.colors.len(), light.colors.len());
        // Both palettes define the surface slots, and they actually differ, so
        // switching themes changes the window background rather than only text.
        for slot in [
            "background",
            "sidebar",
            "panel",
            "panel_2",
            "text",
            "border",
        ] {
            assert!(dark.colors.contains_key(slot), "dark missing {slot}");
            assert!(light.colors.contains_key(slot), "light missing {slot}");
            assert_ne!(dark.colors[slot], light.colors[slot], "{slot} identical");
        }
        assert_eq!(
            light.colors.get("background").map(String::as_str),
            Some("#ffffff")
        );
        assert!(names(Path::new(".")).contains(&"dark".to_string()));
    }

    #[test]
    fn custom_theme_overrides_only_set_slots() {
        let dir = std::env::temp_dir().join(format!("oxide_theme_view_{}", std::process::id()));
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::create_dir_all(dir.join(".oxide/themes")).unwrap();
        std::fs::write(
            dir.join(".oxide/themes/ocean.json"),
            r##"{"accent":"#5fd7ff","success":"green"}"##,
        )
        .unwrap();
        let theme = load(&dir, "ocean");
        assert_eq!(theme.name, "ocean");
        assert_eq!(
            theme.colors.get("accent").map(String::as_str),
            Some("#5fd7ff")
        );
        assert_eq!(
            theme.colors.get("success").map(String::as_str),
            Some("#00cd00")
        );
        assert!(names(&dir).iter().any(|name| name == "ocean"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parses_named_and_hex_colors() {
        assert_eq!(parse_color(Some("cyan")).as_deref(), Some("#00cdcd"));
        assert_eq!(parse_color(Some("#FF8800")).as_deref(), Some("#ff8800"));
        assert_eq!(parse_color(Some("nonsense")), None);
        assert_eq!(parse_color(Some("#12345")), None);
    }
}
