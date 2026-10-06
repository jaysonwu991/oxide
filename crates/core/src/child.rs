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
//! run starts over pipes get a terminal of their own instead — a session on
//! Unix, a console without a window on Windows — so the one the front-end draws
//! on is not theirs to reach for; a program that needs a terminal fails with the
//! reason in its own captured output rather than painting over the interface that
//! started it.

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

/// Windows keeps a child attached to the console the front-end is drawing on
/// however its standard handles are redirected, where opening `CONOUT$` by name
/// paints over the interface and `CONIN$` reads the keys typed into it, so the
/// child is given a console of its own with no window on it.
#[cfg(windows)]
pub(crate) fn detach_terminal(cmd: &mut Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn detach_terminal(_cmd: &mut Command) {}

/// The same detach for a `std::process::Command`: a browser launcher must not
/// sit on the terminal the front-end is drawing on either, even though it is
/// started with null standard handles.
#[cfg(unix)]
pub(crate) fn detach_terminal_std(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: only `setsid`, a raw syscall that reads no shared state, runs in
    // the child between fork and exec.
    unsafe {
        cmd.pre_exec(|| match libc::setsid() {
            -1 => Err(std::io::Error::last_os_error()),
            _ => Ok(()),
        });
    }
}

#[cfg(windows)]
pub(crate) fn detach_terminal_std(cmd: &mut std::process::Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn detach_terminal_std(_cmd: &mut std::process::Command) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Stdio;

    /// Whether this process has a terminal for a child to inherit at all: a
    /// cargo run from a pipe can only prove the detached half.
    #[cfg(unix)]
    fn terminal_available() -> bool {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/tty")
            .is_ok()
    }

    #[cfg(unix)]
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

    #[cfg(unix)]
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

    /// A detached child is still a child: the detach must not take its pipes or
    /// its exit status with it, on either platform.
    #[tokio::test]
    async fn a_detached_child_still_runs_and_writes_its_output() {
        let mut cmd = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
        if cfg!(windows) {
            cmd.args(["/C", "echo detached"]);
        } else {
            cmd.args(["-c", "echo detached"]);
        }
        detach_terminal(&mut cmd);
        let output = cmd
            .stdin(Stdio::null())
            .output()
            .await
            .expect("running the child");
        assert!(output.status.success(), "the child failed: {output:?}");
        assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "detached");
    }
}
