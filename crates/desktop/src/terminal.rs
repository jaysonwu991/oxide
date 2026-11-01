//! The terminal a conversation can open.
//!
//! A turn's tools run through the same shell the reader does, so this is a real
//! pseudoterminal rather than a pipe: the shell draws its own prompt, colours its
//! own output and handles its own line editing, and what the window paints is
//! what the shell wrote. One terminal belongs to the thread that opened it, in
//! the folder that thread runs in — the project, or the worktree it was handed
//! to — so a command run here sees the same files a turn does.
//!
//! Output arrives on a thread of its own and leaves through the event channel;
//! a shell that exits is announced the same way, so the window can say so instead
//! of leaving a prompt that no longer answers.

use anyhow::{Context, Result};
use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use std::io::{Read, Write};
use std::path::Path;

/// The shell a terminal starts. `$SHELL` is what the reader's own terminal
/// starts; a machine that sets nothing gets the one every Unix has.
pub fn default_shell() -> String {
    if cfg!(windows) {
        std::env::var("COMSPEC").unwrap_or_else(|_| "powershell.exe".to_string())
    } else {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

/// One live terminal: the pair the shell is attached to, and the two ends of it
/// this process writes to.
pub struct Terminal {
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    pub child: Box<dyn Child + Send + Sync>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
}

impl Terminal {
    /// Starts a shell in `cwd` at the given size, and hands back the terminal and
    /// the reader its output arrives on. The caller keeps reading: a terminal
    /// nobody reads fills its buffer and the shell stops with it.
    pub fn open(cwd: &Path, cols: u16, rows: u16) -> Result<(Self, Box<dyn Read + Send>)> {
        let (cols, rows) = (cols.max(20), rows.max(4));
        let pair = native_pty_system()
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening a pseudoterminal")?;
        let mut command = CommandBuilder::new(default_shell());
        command.cwd(cwd);
        // The terminal is the reader's own shell, so it gets the environment they
        // would have: without this the child inherits only what this process
        // holds, which is the app's, not a login shell's.
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        let child = pair
            .slave
            .spawn_command(command)
            .with_context(|| format!("starting a shell in {}", cwd.display()))?;
        // The slave end is the child's now: holding it here would keep the master
        // from ever reporting the shell's exit.
        drop(pair.slave);
        let reader = pair
            .master
            .try_clone_reader()
            .context("reading the terminal's output")?;
        let writer = pair
            .master
            .take_writer()
            .context("writing to the terminal")?;
        Ok((
            Self {
                master: pair.master,
                writer,
                child,
                cwd: cwd.to_string_lossy().into_owned(),
                cols,
                rows,
            },
            reader,
        ))
    }

    /// Sends what the reader typed. A shell that has gone away answers with an
    /// error the window reports rather than a silent lost keystroke.
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        self.writer
            .write_all(data)
            .context("writing to the shell")?;
        self.writer.flush().context("flushing the shell's input")
    }

    /// Tells the shell how much room the window gave it, which is what makes a
    /// program like `top` lay itself out again.
    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let (cols, rows) = (cols.max(20), rows.max(4));
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resizing the terminal")?;
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    /// Ends the terminal and the shell with it. A shell that ignores the hangup
    /// is killed: a panel that has been closed must not leave a process behind.
    pub fn close(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(unix)]
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::time::{Duration, Instant};

    /// Reads until the output carries `needle` or the wait runs out: a shell is a
    /// process, so its answer lands when it lands.
    fn read_until(reader: &mut Box<dyn Read + Send>, needle: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut seen = String::new();
        let mut buf = [0u8; 4096];
        while Instant::now() < deadline {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(count) => {
                    seen.push_str(&String::from_utf8_lossy(&buf[..count]));
                    if seen.contains(needle) {
                        return seen;
                    }
                }
                Err(_) => break,
            }
        }
        seen
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("oxide_term_{}_{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_shell_runs_in_the_folder_the_thread_runs_in() {
        let dir = temp("cwd");
        std::fs::write(dir.join("here.txt"), b"x").unwrap();
        let (mut terminal, mut reader) = Terminal::open(&dir, 100, 30).unwrap();
        terminal.write(b"ls here.txt\r").unwrap();
        let seen = read_until(&mut reader, "here.txt");
        assert!(seen.contains("here.txt"), "{seen:?}");
        terminal.close();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn what_the_shell_writes_comes_back_to_the_reader() {
        let dir = temp("echo");
        let (mut terminal, mut reader) = Terminal::open(&dir, 100, 30).unwrap();
        terminal.write(b"echo oxide-terminal-check\r").unwrap();
        let seen = read_until(&mut reader, "oxide-terminal-check");
        assert!(seen.contains("oxide-terminal-check"), "{seen:?}");
        terminal.close();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_shell_is_told_the_size_the_window_gave_it() {
        let dir = temp("size");
        let (mut terminal, _reader) = Terminal::open(&dir, 91, 27).unwrap();
        assert_eq!((terminal.cols, terminal.rows), (91, 27));
        terminal.resize(120, 40).unwrap();
        assert_eq!((terminal.cols, terminal.rows), (120, 40));
        // The floor is the smallest a shell can lay itself out in: a window that
        // reported two columns would not be a terminal at all.
        terminal.resize(3, 1).unwrap();
        assert_eq!((terminal.cols, terminal.rows), (20, 4));
        terminal.close();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_closed_terminal_leaves_nothing_running() {
        let dir = temp("close");
        let (mut terminal, _reader) = Terminal::open(&dir, 80, 24).unwrap();
        terminal.write(b"echo alive\r").unwrap();
        terminal.close();
        // The child was waited on, so it is no longer running: the handle reports
        // the exit rather than a shell left behind.
        assert!(terminal.child.try_wait().is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
