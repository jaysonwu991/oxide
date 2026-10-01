//! Out-of-band notices from the core to whatever front-end is running.
//!
//! Some core work has something to say that belongs to no tool call and no
//! agent event: an MCP server's OAuth flow announcing the URL it just opened, a
//! plugin runtime that could not start, an approval rule that could not be
//! saved. Printing those to stderr is right for a non-interactive run, but the
//! TUI owns the alternate screen — ratatui only repaints cells whose style
//! changed, so a raw write lands as garbled text that survives until the
//! terminal is redrawn. A front-end installs a sink instead and draws the
//! notice in its own transcript.

use std::sync::{Arc, OnceLock};

/// How loudly a notice should be shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
}

/// One thing the core has to say, with the level a front-end tones it by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub level: Level,
    pub text: String,
}

pub type NoticeSink = Arc<dyn Fn(Notice) + Send + Sync>;

static SINK: OnceLock<NoticeSink> = OnceLock::new();

/// Routes notices to `sink` for the rest of the process. Returns `false` when a
/// sink is already installed, since the first front-end to claim the screen owns
/// it.
pub fn set_sink(sink: NoticeSink) -> bool {
    SINK.set(sink).is_ok()
}

pub fn info(text: impl Into<String>) {
    emit(Level::Info, text.into());
}

pub fn warn(text: impl Into<String>) {
    emit(Level::Warn, text.into());
}

fn emit(level: Level, text: String) {
    match SINK.get() {
        Some(sink) => sink(Notice { level, text }),
        // No front-end claimed the screen: a print, JSON or RPC run keeps its
        // stdout for frames, so a notice belongs on stderr.
        None => eprintln!("{text}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn a_sink_receives_notices_and_only_one_can_be_installed() {
        static SEEN: Mutex<Vec<Notice>> = Mutex::new(Vec::new());
        assert!(set_sink(Arc::new(|notice| {
            SEEN.lock().expect("notice log").push(notice);
        })));
        assert!(
            !set_sink(Arc::new(|_| {})),
            "the first front-end to claim the screen keeps it"
        );

        info("mcp browser opened");
        warn("plugin host failed");

        let seen = SEEN.lock().expect("notice log").clone();
        assert!(seen.contains(&Notice {
            level: Level::Info,
            text: "mcp browser opened".to_string(),
        }));
        assert!(seen.contains(&Notice {
            level: Level::Warn,
            text: "plugin host failed".to_string(),
        }));
    }
}
