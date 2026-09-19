//! System clipboard writes for the TUI.
//!
//! Copying always emits an OSC 52 escape sequence, which travels through SSH
//! and tmux, and additionally tries a native clipboard command so terminals
//! that ignore OSC 52 still receive the text.
use anyhow::Result;
use std::io::Write;

/// Copies `text` to the system clipboard.
pub fn copy(text: &str) -> Result<()> {
    let mut stdout = std::io::stdout();
    stdout.write_all(osc52(text).as_bytes())?;
    stdout.flush()?;
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    copy_native(text);
    Ok(())
}

/// OSC 52 payload, wrapped in a DCS passthrough when running under tmux or
/// screen so the multiplexer forwards the sequence to the outer terminal.
fn osc52(text: &str) -> String {
    let payload = format!(
        "\x1b]52;c;{}\x07",
        crate::media::base64_encode(text.as_bytes())
    );
    if std::env::var_os("TMUX").is_some() {
        return format!("{payload}\x1bPtmux;\x1b{payload}\x1b\\");
    }
    if std::env::var_os("STY").is_some() {
        return format!("\x1bP{payload}\x1b\\");
    }
    payload
}

#[cfg(target_os = "macos")]
fn copy_native(text: &str) -> bool {
    if pipe_into("pbcopy", &[], text) {
        return true;
    }
    let script = format!("set the clipboard to \"{}\"", escape_osascript(text));
    std::process::Command::new("osascript")
        .args(["-e", &script])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn copy_native(text: &str) -> bool {
    let wayland = std::env::var_os("WAYLAND_DISPLAY").is_some();
    if wayland && pipe_into("wl-copy", &[], text) {
        return true;
    }
    if pipe_into("xclip", &["-selection", "clipboard"], text) {
        return true;
    }
    pipe_into("xsel", &["--clipboard", "--input"], text)
}

#[cfg(any(target_os = "macos", target_os = "linux"))]
fn pipe_into(command: &str, args: &[&str], text: &str) -> bool {
    use std::process::{Command, Stdio};
    let mut child = match Command::new(command)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        Ok(child) => child,
        Err(_) => return false,
    };
    let mut stdin = child.stdin.take();
    let written = match stdin.as_mut() {
        Some(stdin) => stdin.write_all(text.as_bytes()).is_ok(),
        None => false,
    };
    drop(stdin);
    let status = child.wait();
    written && status.map(|status| status.success()).unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn escape_osascript(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc52_wraps_base64_payload() {
        // `TMUX`/`STY` are absent in the test environment, so the bare sequence
        // is expected.
        if std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some() {
            return;
        }
        assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
    }

    #[test]
    fn osc52_encodes_multibyte_text() {
        if std::env::var_os("TMUX").is_some() || std::env::var_os("STY").is_some() {
            return;
        }
        assert_eq!(
            osc52("é"),
            format!(
                "\x1b]52;c;{}\x07",
                crate::media::base64_encode("é".as_bytes())
            )
        );
    }
}
