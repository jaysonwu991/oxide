//! The terminal state the dock paints.
//!
//! The engine owns the shell and the pseudoterminal. This module only feeds the
//! bytes it announces into a terminal parser, so cursor movement, colours,
//! erasing, alternate-screen programs and scrollback are interpreted by the same
//! kind of state a terminal emulator uses rather than by a line buffer.

use std::num::NonZeroU16;

pub struct TerminalPane {
    pub id: Option<u64>,
    pub cwd: String,
    pub cols: u16,
    pub rows: u16,
    pub exited: bool,
    pub error: Option<String>,
    parser: Option<vt100::Parser>,
}

impl Default for TerminalPane {
    fn default() -> Self {
        Self {
            id: None,
            cwd: String::new(),
            cols: 100,
            rows: 24,
            exited: false,
            error: None,
            parser: None,
        }
    }
}

impl TerminalPane {
    pub fn start(&mut self, id: u64, cwd: String) {
        self.id = Some(id);
        self.cwd = cwd;
        self.exited = false;
        self.error = None;
        self.parser = Some(vt100::Parser::new(
            NonZeroU16::new(self.rows.max(4)).unwrap(),
            NonZeroU16::new(self.cols.max(20)).unwrap(),
            2_000,
        ));
    }

    pub fn stop(&mut self) {
        self.id = None;
        self.cwd.clear();
        self.exited = false;
        self.error = None;
        self.parser = None;
    }

    pub fn feed(&mut self, id: u64, bytes: &[u8]) {
        if self.id != Some(id) {
            return;
        }
        if let Some(parser) = self.parser.as_mut() {
            parser.process(bytes);
            self.exited = false;
        }
    }

    pub fn exit(&mut self, id: u64) {
        if self.id == Some(id) {
            self.exited = true;
        }
    }

    pub fn contents(&self) -> String {
        self.parser
            .as_ref()
            .map(|parser| parser.screen().contents())
            .unwrap_or_default()
    }

    pub fn application_cursor(&self) -> bool {
        self.parser
            .as_ref()
            .is_some_and(|parser| parser.screen().application_cursor())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_output_is_interpreted_as_a_screen() {
        let mut pane = TerminalPane::default();
        pane.start(7, "/tmp".to_string());
        pane.feed(7, b"one\r\ntwo\x1b[2Jthree");
        assert!(pane.contents().contains("three"));
        assert!(!pane.contents().contains("one"));

        pane.feed(7, b"\r\nfour");
        assert!(pane.contents().contains("three"));
        assert!(pane.contents().contains("four"));
    }

    #[test]
    fn output_from_an_old_terminal_is_ignored() {
        let mut pane = TerminalPane::default();
        pane.start(8, "/tmp".to_string());
        pane.feed(7, b"stale");
        assert!(!pane.contents().contains("stale"));
        pane.feed(8, b"current");
        assert!(pane.contents().contains("current"));
    }

    #[test]
    fn stopping_forgets_the_shell_and_its_screen() {
        let mut pane = TerminalPane::default();
        pane.start(9, "/tmp".to_string());
        pane.feed(9, b"output");
        pane.stop();
        assert!(pane.id.is_none());
        assert!(pane.contents().is_empty());
    }
}
