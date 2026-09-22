//! Desktop toast notifications for the TUI.
//!
//! Best-effort and dependency-free: each platform's native notifier is invoked
//! directly, and any failure is ignored so a missing helper (or a headless
//! session) never affects the agent. `notifyOnComplete` and `notifySound` in
//! `settings.json` control whether a finished turn raises a toast and whether it
//! plays the system alert sound.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Notification settings resolved from `settings.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotifyConfig {
    /// Raise a toast when an agent turn finishes.
    pub on_complete: bool,
    /// Play the system alert sound with the toast.
    pub sound: bool,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            on_complete: true,
            sound: true,
        }
    }
}

/// Raises a system notification without blocking the caller. The helper runs on
/// a detached thread so a slow notifier cannot stall the TUI event loop.
pub fn send(title: &str, body: &str, sound: bool) {
    let title = title.to_string();
    let body = body.to_string();
    std::thread::spawn(move || {
        let _ = dispatch(&title, &body, sound);
    });
}

#[cfg(target_os = "macos")]
fn dispatch(title: &str, body: &str, sound: bool) -> bool {
    // The strings travel as argv, so no shell or AppleScript quoting is needed.
    let script = if sound {
        "on run argv\n\
         display notification (item 2 of argv) with title (item 1 of argv) sound name \"Glass\"\n\
         end run"
    } else {
        "on run argv\n\
         display notification (item 2 of argv) with title (item 1 of argv)\n\
         end run"
    };
    run("osascript", &["-e", script, title, body])
}

#[cfg(target_os = "linux")]
fn dispatch(title: &str, body: &str, sound: bool) -> bool {
    let posted = run("notify-send", &["-a", "oxide", title, body]);
    if sound {
        play_linux_sound();
    }
    posted
}

/// Plays the freedesktop alert sound, falling back through the players that are
/// commonly installed. Silent when no player or sound file is available.
#[cfg(target_os = "linux")]
fn play_linux_sound() {
    if run("canberra-gtk-play", &["-i", "complete"]) {
        return;
    }
    const FILES: [&str; 4] = [
        "/usr/share/sounds/freedesktop/stereo/complete.oga",
        "/usr/share/sounds/freedesktop/stereo/bell.oga",
        "/usr/share/sounds/gnome/default/alerts/glass.ogg",
        "/usr/share/sounds/ubuntu/stereo/message.ogg",
    ];
    for file in FILES {
        if !Path::new(file).exists() {
            continue;
        }
        if run("paplay", &[file])
            || run("aplay", &[file])
            || run("ffplay", &["-nodisp", "-autoexit", file])
        {
            return;
        }
    }
}

#[cfg(target_os = "windows")]
fn dispatch(title: &str, body: &str, sound: bool) -> bool {
    // Build the toast from the WinRT template DOM so `CreateTextNode` handles
    // escaping, and append the `audio` element to play the alert sound.
    const SCRIPT: &str = "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] > $null\n\
        $template = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)\n\
        $text = $template.GetElementsByTagName('text')\n\
        $text.Item(0).AppendChild($template.CreateTextNode($env:OXIDE_NOTIFY_TITLE)) > $null\n\
        $text.Item(1).AppendChild($template.CreateTextNode($env:OXIDE_NOTIFY_BODY)) > $null\n\
        if ($env:OXIDE_NOTIFY_SOUND -eq '1') {\n\
        $audio = $template.CreateElement('audio')\n\
        $audio.SetAttribute('src', 'ms-winsoundevent:Notification.Default')\n\
        $template.DocumentElement.AppendChild($audio) > $null\n\
        }\n\
        $toast = [Windows.UI.Notifications.ToastNotification]::new($template)\n\
        [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier('oxide').Show($toast)";
    Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", SCRIPT])
        .env("OXIDE_NOTIFY_TITLE", title)
        .env("OXIDE_NOTIFY_BODY", body)
        .env("OXIDE_NOTIFY_SOUND", if sound { "1" } else { "0" })
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn dispatch(_title: &str, _body: &str, _sound: bool) -> bool {
    false
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn run(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Resolves the notification settings. Reads `notifyOnComplete` and
/// `notifySound` from the global `settings.json` and the project
/// `.oxide/settings.json` (project wins per key), overridable with
/// `OXIDE_NOTIFY_ON_COMPLETE` and `OXIDE_NOTIFY_SOUND`. Both default to on.
pub fn load_config(cwd: &Path) -> NotifyConfig {
    let mut on_complete = None;
    let mut sound = None;
    for path in config_paths(cwd).into_iter().rev() {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            continue;
        };
        if on_complete.is_none() {
            on_complete = value
                .get("notifyOnComplete")
                .and_then(|value| value.as_bool());
        }
        if sound.is_none() {
            sound = value.get("notifySound").and_then(|value| value.as_bool());
        }
        if on_complete.is_some() && sound.is_some() {
            break;
        }
    }
    let mut config = NotifyConfig {
        on_complete: on_complete.unwrap_or(true),
        sound: sound.unwrap_or(true),
    };
    if let Some(value) = env_bool("OXIDE_NOTIFY_ON_COMPLETE") {
        config.on_complete = value;
    }
    if let Some(value) = env_bool("OXIDE_NOTIFY_SOUND") {
        config.sound = value;
    }
    config
}

fn env_bool(name: &str) -> Option<bool> {
    std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<bool>().ok())
}

/// A single notification setting, so `/notify` persists only the key it changed
/// and leaves the other key's stored value (and any project or env override)
/// alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyKey {
    OnComplete,
    Sound,
}

impl NotifyKey {
    fn setting_name(self) -> &'static str {
        match self {
            NotifyKey::OnComplete => "notifyOnComplete",
            NotifyKey::Sound => "notifySound",
        }
    }
}

/// Persists one flag into the global `settings.json`, preserving any other keys,
/// and returns the file written. A project `.oxide/settings.json` can still
/// override it.
pub fn save(key: NotifyKey, enabled: bool) -> Result<PathBuf> {
    let path = settings_path();
    save_to(&path, key, enabled)?;
    Ok(path)
}

fn settings_path() -> PathBuf {
    if let Some(path) = std::env::var_os("OXIDE_SETTINGS_FILE") {
        return PathBuf::from(path);
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("oxide")
        .join("settings.json")
}

fn save_to(path: &Path, key: NotifyKey, enabled: bool) -> Result<()> {
    let mut value = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    if !value.is_object() {
        value = serde_json::json!({});
    }
    let object = value.as_object_mut().expect("object");
    object.insert(
        key.setting_name().to_string(),
        serde_json::Value::Bool(enabled),
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&value).context("serializing settings")?;
    std::fs::write(path, format!("{text}\n"))
        .with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn config_paths(cwd: &Path) -> Vec<PathBuf> {
    let mut paths = vec![settings_path()];
    if let Some(root) = crate::ecosystem::project_root(cwd) {
        paths.push(root.join(".oxide").join("settings.json"));
    }
    paths
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_default_to_enabled() {
        assert_eq!(
            NotifyConfig::default(),
            NotifyConfig {
                on_complete: true,
                sound: true
            }
        );
    }

    #[test]
    fn project_settings_disable_notifications() {
        if std::env::var_os("OXIDE_NOTIFY_ON_COMPLETE").is_some()
            || std::env::var_os("OXIDE_NOTIFY_SOUND").is_some()
        {
            return;
        }
        let root =
            std::env::temp_dir().join(format!("oxide-notify-project-{}", std::process::id()));
        let oxide = root.join(".oxide");
        std::fs::create_dir_all(&oxide).unwrap();
        std::fs::write(
            oxide.join("settings.json"),
            r#"{"notifyOnComplete": false, "notifySound": false}"#,
        )
        .unwrap();
        let config = load_config(&root);
        assert!(!config.on_complete);
        assert!(!config.sound);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn save_updates_only_the_changed_key() {
        let dir = std::env::temp_dir().join(format!("oxide-notify-save-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        std::fs::write(
            &path,
            r#"{"hideThinkingBlock": true, "notifyOnComplete": true}"#,
        )
        .unwrap();
        save_to(&path, NotifyKey::Sound, false).unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(value["hideThinkingBlock"], serde_json::json!(true));
        assert_eq!(
            value["notifyOnComplete"],
            serde_json::json!(true),
            "the untouched key keeps its stored value"
        );
        assert_eq!(value["notifySound"], serde_json::json!(false));
        std::fs::remove_dir_all(&dir).ok();
    }
}
