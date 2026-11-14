//! The desktop engine, in this process.
//!
//! The Electron app runs the engine as a child (`crates/desktop/src/main.rs`)
//! and speaks one JSON packet per line to it. This front-end links the same
//! engine instead — `oxide_desktop::commands::dispatch` is the command contract
//! the page uses, and `oxide_desktop::bridge::Host` is the pipe it answers
//! through — so the window below draws the same projects, sessions, turns,
//! approvals and configuration the Electron window does.
//!
//! A call is dispatched on the engine's own runtime and answered on a channel a
//! GPUI task awaits, and every announcement the engine emits (a streaming turn,
//! an approval, a terminal, an update) arrives on `packets` in the same packet
//! shape the page reads: `{type: "message", id: <event>, payload}`.

use anyhow::Result;
use oxide_desktop::bridge::{EventSink, Host};
use oxide_desktop::commands::{dispatch, DesktopState};
use oxide_desktop::manager::DesktopManager;
use serde_json::Value;
use std::io::Write;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// The channel the engine writes its packets into.
struct Pipe {
    buffer: Vec<u8>,
    out: mpsc::UnboundedSender<Value>,
}

impl Write for Pipe {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.buffer.extend_from_slice(bytes);
        while let Some(end) = self.buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=end).collect();
            if let Ok(packet) = serde_json::from_slice::<Value>(&line[..line.len() - 1]) {
                let _ = self.out.send(packet);
            }
        }
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub struct Engine {
    state: Arc<DesktopState>,
    host: Arc<Host>,
    runtime: tokio::runtime::Runtime,
}

impl Engine {
    /// Starts the engine and returns the stream of its announcements beside it.
    pub fn start() -> Result<(Self, mpsc::UnboundedReceiver<Value>)> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()?;
        let _entered = runtime.enter();

        let (sender, packets) = mpsc::unbounded_channel();
        let host = Arc::new(Host::new(Pipe {
            buffer: Vec::new(),
            out: sender,
        }));
        let state = Arc::new(DesktopState::new(
            DesktopManager::load_lossy(),
            EventSink::new(Arc::clone(&host)),
        ));

        sync_plugins();
        runtime.spawn(oxide_desktop::commands::auto_catalog(Arc::clone(&state)));

        Ok((
            Self {
                state,
                host,
                runtime,
            },
            packets,
        ))
    }

    /// Dispatches one command and waits for its answer, for the launch check.
    pub fn call_blocking(&self, command: &str, args: Value) -> Result<Value, String> {
        let reply = self.call(command, args);
        self.runtime
            .block_on(reply)
            .unwrap_or_else(|_| Err("the engine stopped answering".to_string()))
    }

    /// Dispatches one command through the contract the Electron window speaks.
    pub fn call(&self, command: &str, args: Value) -> oneshot::Receiver<Result<Value, String>> {
        let (reply, answer) = oneshot::channel();
        let state = Arc::clone(&self.state);
        let host = Arc::clone(&self.host);
        let command = command.to_string();
        self.runtime.spawn(async move {
            let result = dispatch(state, host.as_ref(), &command, args).await;
            let _ = reply.send(result);
        });
        answer
    }
}

/// The plugins shipped beside this binary, installed into the shared config the
/// way the Electron app installs them at launch.
///
/// The window that starts the engine may say where they are
/// (`OXIDE_DESKTOP_PLUGINS`, which the Electron window sets); a bundle that has
/// no window to ask is looked through itself — `plugins` beside the executable,
/// then the resources directory of a macOS app bundle.
fn sync_plugins() {
    let from_env = std::env::var_os("OXIDE_DESKTOP_PLUGINS").map(std::path::PathBuf::from);
    let beside = std::env::current_exe().ok().and_then(|exe| {
        let directory = exe.parent()?.to_path_buf();
        [
            directory.join("plugins"),
            directory.join("..").join("Resources").join("plugins"),
        ]
        .into_iter()
        .find(|candidate| candidate.is_dir())
    });
    let Some(root) = from_env.or(beside) else {
        return;
    };
    if let Err(error) = oxide_core::plugin_registry::sync_managed_plugins(&root) {
        eprintln!("[oxide] could not sync bundled plugins: {error:#}");
    }
}
