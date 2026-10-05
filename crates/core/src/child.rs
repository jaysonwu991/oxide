//! Children oxide starts.
//!
//! A tool child is given pipes and no stdin, but it still inherits oxide's
//! session — and with the session, the terminal the front-end is drawing on. A
//! program that opens `/dev/tty` for an answer of its own (zsh's `compinit`
//! asking whether to keep an insecure directory, a credential or passphrase
//! prompt, a pager) then writes outside the frame the TUI is composing: those
//! cells are not in the frame's buffer, so ratatui never repaints them and the
//! prompt stays over the conversation, the composer and the message being typed
//! in it — while the keys typed there answer the child instead. The children a
//! run starts over pipes — a shell command, an LSP server, an MCP server, the
//! plugin host — get a session of their own instead, so they have no controlling
//! terminal to reach for; a program that needs one fails with the reason in its
//! own captured output rather than painting over the interface that started it.

use tokio::process::Command;

/// Runs a child over pipes in a session of its own, so it has no controlling
/// terminal: what it would have asked the reader is reported in its output.
#[cfg(unix)]
pub(crate) fn detach_terminal(cmd: &mut Command) {
    // SAFETY: only `setsid`, a raw syscall that reads no shared state, runs in
    // the child between fork and exec.
    unsafe {
        cmd.pre_exec(|| match libc::setsid() {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
}

/// A child on Windows reaches no terminal oxide draws on: its handles are
/// pipes, and a console of its own is not the one the front-end owns.
#[cfg(not(unix))]
pub(crate) fn detach_terminal(_cmd: &mut Command) {}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// Whether this process has a terminal for a child to inherit at all: a
    /// cargo run from a pipe can only prove the detached half.
    fn terminal_available() -> bool {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .is_ok()
    }

    async fn opens_the_terminal(detached: bool) -> bool {
        let mut cmd = Command::new("sh");
        cmd.arg("-c").arg("exec 3<>/dev/tty");
        if detached {
            detach_terminal(&mut cmd);
        }
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .expect("running sh")
            .success()
    }

    #[tokio::test]
    async fn a_child_over_pipes_cannot_reach_the_terminal() {
        assert!(
            !opens_the_terminal(true).await,
            "a detached child reached the terminal"
        );
        if terminal_available() {
            assert!(
                opens_the_terminal(false).await,
                "a child with no session of its own inherits the reader's terminal"
            );
        }
    }
}
