//! The composer's text field.
//!
//! GPUI ships the platform input plumbing but no editable field, so this is the
//! one the composer uses: a multi-line buffer shaped with the text system,
//! painted with a caret and a selection, and wired to `EntityInputHandler` so
//! the platform's own typing, pasting and input methods land in it. The keys
//! are the terminal's and the page's: `Enter` sends (the composer's binding),
//! `Shift+Enter` opens a line, the arrows move, and `Cmd`/`Ctrl` with an arrow,
//! `Backspace` or `A` reaches the ends of the buffer.

use gpui::prelude::*;
use gpui::{
    actions, canvas, div, fill, point, px, size, transparent_black, App, Bounds, ClipboardEntry,
    ClipboardItem, Context, ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, Font, FontStyle, FontWeight, ImageFormat, KeyBinding, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollDelta, ScrollWheelEvent,
    SharedString, TextAlign, TextRun, UTF16Selection, Window,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

actions!(
    keys,
    [
        Backspace,
        Delete,
        Left,
        Right,
        Up,
        Down,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        WordLeft,
        WordRight,
        SelectWordLeft,
        SelectWordRight,
        Home,
        End,
        SelectHome,
        SelectEnd,
        SelectAll,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        InsertNewline,
        DeleteToLineStart,
        DeleteWordBackward,
    ]
);

/// The keys every editor in the window answers, bound once at launch.
pub fn key_bindings() -> Vec<KeyBinding> {
    vec![
        KeyBinding::new("backspace", Backspace, Some("Editor")),
        KeyBinding::new("delete", Delete, Some("Editor")),
        KeyBinding::new("left", Left, Some("Editor")),
        KeyBinding::new("right", Right, Some("Editor")),
        KeyBinding::new("up", Up, Some("Editor")),
        KeyBinding::new("down", Down, Some("Editor")),
        KeyBinding::new("shift-left", SelectLeft, Some("Editor")),
        KeyBinding::new("shift-right", SelectRight, Some("Editor")),
        KeyBinding::new("shift-up", SelectUp, Some("Editor")),
        KeyBinding::new("shift-down", SelectDown, Some("Editor")),
        KeyBinding::new("alt-left", WordLeft, Some("Editor")),
        KeyBinding::new("alt-right", WordRight, Some("Editor")),
        KeyBinding::new("alt-shift-left", SelectWordLeft, Some("Editor")),
        KeyBinding::new("alt-shift-right", SelectWordRight, Some("Editor")),
        KeyBinding::new("cmd-left", Home, Some("Editor")),
        KeyBinding::new("ctrl-left", Home, Some("Editor")),
        KeyBinding::new("cmd-right", End, Some("Editor")),
        KeyBinding::new("ctrl-right", End, Some("Editor")),
        KeyBinding::new("home", Home, Some("Editor")),
        KeyBinding::new("end", End, Some("Editor")),
        KeyBinding::new("shift-home", SelectHome, Some("Editor")),
        KeyBinding::new("shift-end", SelectEnd, Some("Editor")),
        KeyBinding::new("cmd-shift-left", SelectHome, Some("Editor")),
        KeyBinding::new("ctrl-shift-left", SelectHome, Some("Editor")),
        KeyBinding::new("cmd-shift-right", SelectEnd, Some("Editor")),
        KeyBinding::new("ctrl-shift-right", SelectEnd, Some("Editor")),
        KeyBinding::new("cmd-a", SelectAll, Some("Editor")),
        KeyBinding::new("ctrl-a", SelectAll, Some("Editor")),
        KeyBinding::new("cmd-z", Undo, Some("Editor")),
        KeyBinding::new("ctrl-z", Undo, Some("Editor")),
        KeyBinding::new("cmd-shift-z", Redo, Some("Editor")),
        KeyBinding::new("ctrl-shift-z", Redo, Some("Editor")),
        KeyBinding::new("ctrl-y", Redo, Some("Editor")),
        KeyBinding::new("cmd-v", Paste, Some("Editor")),
        KeyBinding::new("ctrl-v", Paste, Some("Editor")),
        KeyBinding::new("cmd-c", Copy, Some("Editor")),
        KeyBinding::new("ctrl-c", Copy, Some("Editor")),
        KeyBinding::new("cmd-x", Cut, Some("Editor")),
        KeyBinding::new("ctrl-x", Cut, Some("Editor")),
        KeyBinding::new("shift-enter", InsertNewline, Some("Editor")),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, Some("Editor")),
        KeyBinding::new("alt-backspace", DeleteWordBackward, Some("Editor")),
    ]
}

#[derive(Clone, Debug)]
pub enum EditorEvent {
    Changed,
    AttachmentPasted { name: String, data_url: String },
}

impl EventEmitter<EditorEvent> for Editor {}

pub const FONT_SIZE: f32 = 13.5;
pub const LINE_HEIGHT: f32 = 20.0;
const MAX_HISTORY: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunStyle {
    Normal,
    Bold,
    Italic,
    Quote,
    Code,
    Syntax,
    Decoration,
}

struct ShapedLine {
    start: usize,
    end: usize,
    display_to_source: Vec<usize>,
    line: gpui::WrappedLine,
}

#[derive(Clone, Debug)]
struct Snapshot {
    content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
}

pub struct Editor {
    focus_handle: FocusHandle,
    content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    placeholder: SharedString,
    lines: Vec<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    measured_height: Pixels,
    selecting: bool,
    color: gpui::Hsla,
    placeholder_color: gpui::Hsla,
    caret_color: gpui::Hsla,
    selection_color: gpui::Hsla,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    scroll_offset: f32,
    viewport_height: Pixels,
}

impl Editor {
    pub fn new(
        placeholder: impl Into<SharedString>,
        color: gpui::Hsla,
        placeholder_color: gpui::Hsla,
        caret_color: gpui::Hsla,
        selection_color: gpui::Hsla,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            placeholder: placeholder.into(),
            lines: Vec::new(),
            last_bounds: None,
            measured_height: px(LINE_HEIGHT + 8.0),
            selecting: false,
            color,
            placeholder_color,
            caret_color,
            selection_color,
            undo: Vec::new(),
            redo: Vec::new(),
            scroll_offset: 0.0,
            viewport_height: px(LINE_HEIGHT + 8.0),
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn height(&self) -> Pixels {
        let columns = self
            .last_bounds
            .map(|bounds| (f32::from(bounds.size.width) / (FONT_SIZE * 0.56)).max(8.0) as usize)
            .unwrap_or(72);
        let estimated = self
            .content
            .split('\n')
            .map(|line| line.chars().count().max(1).div_ceil(columns))
            .sum::<usize>()
            .max(1) as f32
            * LINE_HEIGHT;
        self.measured_height.max(px(estimated))
    }

    pub fn cursor_utf16(&self) -> usize {
        self.offset_to_utf16(self.cursor_offset())
    }

    pub fn replace_utf16_range(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        self.remember();
        let range = self.range_from_utf16(&range);
        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            text,
            &self.content[range.end..]
        )
        .into();
        let caret = range.start + text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
        cx.emit(EditorEvent::Changed);
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
        self.undo.clear();
        self.redo.clear();
        self.scroll_offset = 0.0;
        cx.notify();
        cx.emit(EditorEvent::Changed);
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_text("", cx);
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn snapshot(&self) -> Snapshot {
        Snapshot {
            content: self.content.clone(),
            selected_range: self.selected_range.clone(),
            selection_reversed: self.selection_reversed,
        }
    }

    fn remember(&mut self) {
        let snapshot = self.snapshot();
        if self
            .undo
            .last()
            .is_some_and(|last| last.content == snapshot.content)
        {
            self.undo.pop();
        }
        self.undo.push(snapshot);
        if self.undo.len() > MAX_HISTORY {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn restore(&mut self, snapshot: Snapshot, cx: &mut Context<Self>) {
        self.content = snapshot.content;
        self.selected_range = snapshot.selected_range;
        self.selection_reversed = snapshot.selection_reversed;
        self.marked_range = None;
        self.ensure_caret_visible();
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn undo(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.undo.pop() else {
            return;
        };
        self.redo.push(self.snapshot());
        self.restore(snapshot, cx);
    }

    fn redo(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.redo.pop() else {
            return;
        };
        self.undo.push(self.snapshot());
        self.restore(snapshot, cx);
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset;
        } else {
            self.selected_range.end = offset;
        }
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        cx.notify();
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(index, _)| (index < offset).then_some(index))
            .unwrap_or(0)
    }

    fn previous_word_boundary(&self, offset: usize) -> usize {
        let mut seen_word = false;
        for (index, grapheme) in self.content.grapheme_indices(true).rev() {
            if index >= offset {
                continue;
            }
            let is_word = grapheme.chars().any(|ch| ch.is_alphanumeric() || ch == '_');
            if seen_word && !is_word {
                return index + grapheme.len();
            }
            if is_word {
                seen_word = true;
            } else if !seen_word && !grapheme.chars().all(char::is_whitespace) {
                return index;
            }
        }
        0
    }

    fn next_word_boundary(&self, offset: usize) -> usize {
        let mut seen_word = false;
        let mut previous = None;
        for (index, grapheme) in self.content.grapheme_indices(true) {
            if index <= offset {
                previous = Some((index, grapheme));
                continue;
            }
            let is_word = grapheme.chars().any(|ch| ch.is_alphanumeric() || ch == '_');
            if seen_word && !is_word {
                return index;
            }
            if is_word {
                seen_word = true;
            } else if !seen_word
                && previous.is_some_and(|(_, previous): (usize, &str)| {
                    !previous.chars().all(char::is_whitespace)
                })
            {
                return index;
            }
            previous = Some((index, grapheme));
        }
        self.content.len()
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(index, _)| (index > offset).then_some(index))
            .unwrap_or(self.content.len())
    }

    fn line_bounds(&self) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        let mut start = 0;
        for (index, ch) in self.content.char_indices() {
            if ch == '\n' {
                ranges.push(start..index);
                start = index + 1;
            }
        }
        ranges.push(start..self.content.len());
        ranges
    }

    fn line_of(&self, offset: usize) -> (usize, usize) {
        let lines = self.line_bounds();
        for (index, range) in lines.iter().enumerate() {
            if offset <= range.end {
                return (index, offset.saturating_sub(range.start));
            }
        }
        let last = lines.len().saturating_sub(1);
        (last, lines.get(last).map(|range| range.len()).unwrap_or(0))
    }

    fn delete_selection(&mut self, cx: &mut Context<Self>) -> bool {
        if self.selected_range.is_empty() {
            return false;
        }
        self.remember();
        let range = self.selected_range.clone();
        self.content = format!(
            "{}{}",
            &self.content[..range.start],
            &self.content[range.end..]
        )
        .into();
        self.selected_range = range.start..range.start;
        self.marked_range = None;
        cx.emit(EditorEvent::Changed);
        cx.notify();
        true
    }

    fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        self.remember();
        let range = self.selected_range.clone();
        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            text,
            &self.content[range.end..]
        )
        .into();
        let caret = range.start + text.len();
        self.selected_range = caret..caret;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn backspace(&mut self, cx: &mut Context<Self>) {
        if self.delete_selection(cx) {
            return;
        }
        let cursor = self.cursor_offset();
        if cursor == 0 {
            return;
        }
        let (line, column) = self.line_of(cursor);
        let bounds = self.line_bounds();
        let range = &bounds[line];
        let before = &self.content[range.start..cursor];
        if column > 0 && marker_content_start(before).is_some_and(|start| start == before.len()) {
            self.remember();
            self.content = format!(
                "{}{}",
                &self.content[..range.start],
                &self.content[cursor..]
            )
            .into();
            self.selected_range = range.start..range.start;
            self.selection_reversed = false;
            cx.emit(EditorEvent::Changed);
            cx.notify();
            return;
        }
        self.remember();
        let previous = self.previous_boundary(cursor);
        self.content = format!("{}{}", &self.content[..previous], &self.content[cursor..]).into();
        self.selected_range = previous..previous;
        self.selection_reversed = false;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn delete_forward(&mut self, cx: &mut Context<Self>) {
        if self.delete_selection(cx) {
            return;
        }
        let cursor = self.cursor_offset();
        if cursor >= self.content.len() {
            return;
        }
        self.remember();
        let next = self.next_boundary(cursor);
        self.content = format!("{}{}", &self.content[..cursor], &self.content[next..]).into();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn move_left(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selected_range.is_empty() {
            self.move_to(self.selected_range.start, cx);
            return;
        }
        let target = self.previous_boundary(self.cursor_offset());
        if select {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn move_right(&mut self, select: bool, cx: &mut Context<Self>) {
        if !select && !self.selected_range.is_empty() {
            self.move_to(self.selected_range.end, cx);
            return;
        }
        let target = self.next_boundary(self.cursor_offset());
        if select {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn move_word(&mut self, left: bool, select: bool, cx: &mut Context<Self>) {
        let target = if left {
            self.previous_word_boundary(self.cursor_offset())
        } else {
            self.next_word_boundary(self.cursor_offset())
        };
        if select {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn move_vertical(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let caret = self.point_for_offset(cursor);
        let target_y = if down {
            caret.y + px(LINE_HEIGHT + 0.5)
        } else {
            (caret.y - px(LINE_HEIGHT)).max(px(0.0)) + px(0.5)
        };
        let target = self.offset_for_point(point(caret.x, target_y));
        if select {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn home(&mut self, select: bool, cx: &mut Context<Self>) {
        let caret = self.point_for_offset(self.cursor_offset());
        let target = self.offset_for_point(point(px(0.0), caret.y + px(0.5)));
        if select {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn end(&mut self, select: bool, cx: &mut Context<Self>) {
        let caret = self.point_for_offset(self.cursor_offset());
        let right = self
            .last_bounds
            .map(|bounds| bounds.size.width)
            .unwrap_or(px(10_000.0));
        let target = self.offset_for_point(point(right, caret.y + px(0.5)));
        if select {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }
        let text = self.content[self.selected_range.clone()].to_string();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn cut(&mut self, cx: &mut Context<Self>) {
        self.copy(cx);
        self.delete_selection(cx);
    }

    fn paste(&mut self, cx: &mut Context<Self>) {
        // Read through the shared media reader first, so a Finder copy attaches
        // the copied file rather than the icon the pasteboard also carries.
        if let Some((name, data_url)) = oxide_core::media::clipboard_media() {
            cx.emit(EditorEvent::AttachmentPasted { name, data_url });
            return;
        }

        let clipboard = cx.read_from_clipboard();
        if let Some((name, data_url)) = clipboard.as_ref().and_then(clipboard_image_attachment) {
            cx.emit(EditorEvent::AttachmentPasted { name, data_url });
            return;
        }
        if let Some(text) = clipboard.and_then(|item| item.text()) {
            self.insert(&text, cx);
        }
    }

    fn delete_to_line_start(&mut self, cx: &mut Context<Self>) {
        if self.delete_selection(cx) {
            return;
        }
        let cursor = self.cursor_offset();
        let (line, _) = self.line_of(cursor);
        let start = self.line_bounds()[line].start;
        if start == cursor {
            return;
        }
        self.remember();
        self.content = format!("{}{}", &self.content[..start], &self.content[cursor..]).into();
        self.selected_range = start..start;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn delete_word_backward(&mut self, cx: &mut Context<Self>) {
        if self.delete_selection(cx) {
            return;
        }
        let cursor = self.cursor_offset();
        let start = self.previous_word_boundary(cursor);
        if start == cursor {
            return;
        }
        self.remember();
        self.content = format!("{}{}", &self.content[..start], &self.content[cursor..]).into();
        self.selected_range = start..start;
        self.selection_reversed = false;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn insert_newline(&mut self, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let (line, column) = self.line_of(cursor);
        let bounds = self.line_bounds();
        let range = &bounds[line];
        let before = &self.content[range.start..cursor];
        let insertion = match continuation_prefix(before) {
            Some(prefix) => format!("\n{prefix}"),
            None if column > 0
                && marker_content_start(before).is_some_and(|start| start == before.len()) =>
            {
                "\n".to_string()
            }
            None => "\n".to_string(),
        };
        self.insert(&insertion, cx);
    }

    /// The point a byte offset is painted at, in the editor's own coordinates.
    fn point_for_offset(&self, offset: usize) -> Point<Pixels> {
        let mut y = px(0.0);
        for shaped in &self.lines {
            let start = shaped.start;
            if offset <= shaped.end || shaped.end == self.content.len() {
                if offset < start && shaped.start != 0 {
                    continue;
                }
                let source = offset.min(shaped.end).saturating_sub(start);
                let display = shaped
                    .display_to_source
                    .iter()
                    .rposition(|position| *position <= source)
                    .unwrap_or(0);
                if let Some(position) = shaped.line.position_for_index(display, px(LINE_HEIGHT)) {
                    return point(position.x, y + position.y);
                }
            }
            y += shaped.line.size(px(LINE_HEIGHT)).height;
        }
        point(px(0.0), y)
    }

    fn offset_for_point(&self, position: Point<Pixels>) -> usize {
        let viewport = self
            .last_bounds
            .map(|bounds| bounds.size.height)
            .unwrap_or(self.viewport_height);
        let scroll_offset = self
            .scroll_offset
            .min((f32::from(self.total_height()) - f32::from(viewport)).max(0.0));
        let position = point(position.x, position.y + px(scroll_offset));
        let mut y = px(0.0);
        for shaped in &self.lines {
            let height = shaped.line.size(px(LINE_HEIGHT)).height;
            if position.y <= y + height {
                let within = point(position.x, position.y - y);
                return match shaped.line.index_for_position(within, px(LINE_HEIGHT)) {
                    Ok(index) | Err(index) => {
                        shaped.start
                            + shaped
                                .display_to_source
                                .get(index)
                                .copied()
                                .unwrap_or(shaped.end - shaped.start)
                    }
                };
            }
            y += height;
        }
        self.content.len()
    }

    fn total_height(&self) -> Pixels {
        self.lines
            .iter()
            .map(|shaped| shaped.line.size(px(LINE_HEIGHT)).height)
            .fold(px(0.0), |sum, height| sum + height)
            .max(px(LINE_HEIGHT))
    }

    fn max_scroll(&self) -> f32 {
        (f32::from(self.total_height()) - f32::from(self.viewport_height)).max(0.0)
    }

    fn ensure_caret_visible(&mut self) {
        let caret = self.point_for_offset(self.cursor_offset());
        let top = f32::from(caret.y);
        let bottom = top + LINE_HEIGHT;
        let visible = f32::from(self.viewport_height);
        if top < self.scroll_offset + 4.0 {
            self.scroll_offset = (top - 4.0).max(0.0);
        } else if bottom > self.scroll_offset + visible - 4.0 {
            self.scroll_offset = bottom - visible + 4.0;
        }
        self.scroll_offset = self.scroll_offset.clamp(0.0, self.max_scroll());
    }

    fn scroll(&mut self, delta: Pixels, cx: &mut Context<Self>) {
        if self.max_scroll() <= 0.0 {
            return;
        }
        self.scroll_offset = (self.scroll_offset + f32::from(delta)).clamp(0.0, self.max_scroll());
        cx.notify();
    }

    fn editor_font(&self, weight: FontWeight, style: FontStyle) -> Font {
        Font {
            family: ".SystemUIFont".into(),
            features: Default::default(),
            fallbacks: None,
            weight,
            style,
        }
    }

    fn push_run(&self, runs: &mut Vec<TextRun>, text: &str, style: RunStyle) {
        if text.is_empty() {
            return;
        }
        let (weight, font_style, color, background_color) = match style {
            RunStyle::Normal => (FontWeight::NORMAL, FontStyle::Normal, self.color, None),
            RunStyle::Bold => (FontWeight::SEMIBOLD, FontStyle::Normal, self.color, None),
            RunStyle::Italic => (FontWeight::NORMAL, FontStyle::Italic, self.color, None),
            RunStyle::Quote => (FontWeight::NORMAL, FontStyle::Italic, self.color, None),
            RunStyle::Code => (
                FontWeight::NORMAL,
                FontStyle::Normal,
                self.caret_color,
                Some(self.selection_color),
            ),
            RunStyle::Syntax => (
                FontWeight::NORMAL,
                FontStyle::Normal,
                transparent_black(),
                None,
            ),
            RunStyle::Decoration => (
                FontWeight::NORMAL,
                FontStyle::Normal,
                self.placeholder_color,
                None,
            ),
        };
        runs.push(TextRun {
            len: text.len(),
            font: self.editor_font(weight, font_style),
            color,
            background_color,
            underline: None,
            strikethrough: None,
        });
    }

    fn append_visible(
        &self,
        display: &mut String,
        display_to_source: &mut Vec<usize>,
        runs: &mut Vec<TextRun>,
        source_start: usize,
        text: &str,
        style: RunStyle,
    ) {
        if text.is_empty() {
            return;
        }
        self.push_run(runs, text, style);
        for (offset, character) in text.char_indices() {
            display.push(character);
            let bytes = character.len_utf8();
            for byte in 1..=bytes {
                display_to_source.push(source_start + offset + byte);
            }
        }
    }

    fn append_hidden(display_to_source: &mut [usize], source_end: usize) {
        if let Some(last) = display_to_source.last_mut() {
            *last = source_end;
        }
    }

    fn append_decoration(
        &self,
        display: &mut String,
        display_to_source: &mut Vec<usize>,
        runs: &mut Vec<TextRun>,
        source: usize,
        text: &str,
        style: RunStyle,
    ) {
        if text.is_empty() {
            return;
        }
        self.push_run(runs, text, style);
        display.push_str(text);
        for _ in text.bytes() {
            display_to_source.push(source);
        }
    }

    fn render_line(&self, text: &str) -> (String, Vec<usize>, Vec<TextRun>) {
        let mut display = String::new();
        let mut display_to_source = vec![0];
        let mut runs = Vec::new();
        let (prefix_end, _prefix_style, content_style) = line_marker(text);
        let indent = text.len() - text.trim_start().len();
        if indent > 0 {
            self.append_visible(
                &mut display,
                &mut display_to_source,
                &mut runs,
                0,
                &text[..indent],
                RunStyle::Normal,
            );
        }
        let syntax = &text[indent..prefix_end];
        if !syntax.is_empty() {
            Self::append_hidden(&mut display_to_source, prefix_end);
        }
        let trimmed = &text[indent..];
        let decoration =
            if trimmed.starts_with("```") || trimmed.starts_with("~~~") || trimmed.starts_with('#')
            {
                None
            } else if let Some(quoted) = trimmed.strip_prefix("> ") {
                let _ = quoted;
                Some(("│ ".to_string(), RunStyle::Decoration))
            } else if trimmed.starts_with("- [ ] ") || trimmed.starts_with("* [ ] ") {
                Some(("☐ ".to_string(), RunStyle::Decoration))
            } else if trimmed.starts_with("- [x] ")
                || trimmed.starts_with("* [x] ")
                || trimmed.starts_with("- [X] ")
                || trimmed.starts_with("* [X] ")
            {
                Some(("☑ ".to_string(), RunStyle::Decoration))
            } else if trimmed.starts_with("- ")
                || trimmed.starts_with("* ")
                || trimmed.starts_with("+ ")
            {
                Some(("• ".to_string(), RunStyle::Decoration))
            } else {
                let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
                (digits > 0
                    && matches!(trimmed.as_bytes().get(digits), Some(b'.' | b')'))
                    && trimmed.as_bytes().get(digits + 1) == Some(&b' '))
                .then(|| (trimmed[..digits + 2].to_string(), RunStyle::Decoration))
            };
        if let Some((decoration, style)) = decoration {
            self.append_decoration(
                &mut display,
                &mut display_to_source,
                &mut runs,
                prefix_end,
                &decoration,
                style,
            );
        }
        let content_source = if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            text.len()
        } else {
            prefix_end
        };
        let content_start = prefix_end;
        let content = &text[content_source..];
        let mut plain_start = 0;
        let mut index = 0;
        while index < content.len() {
            let Some((marker, closer, style)) = inline_marker(content, index) else {
                index += content[index..].chars().next().map_or(1, char::len_utf8);
                continue;
            };
            if plain_start < index {
                self.append_visible(
                    &mut display,
                    &mut display_to_source,
                    &mut runs,
                    content_start + plain_start,
                    &content[plain_start..index],
                    content_style,
                );
            }
            Self::append_hidden(&mut display_to_source, content_start + index + marker);
            if index + marker < closer {
                self.append_visible(
                    &mut display,
                    &mut display_to_source,
                    &mut runs,
                    content_start + index + marker,
                    &content[index + marker..closer],
                    style,
                );
            }
            Self::append_hidden(&mut display_to_source, content_start + closer + marker);
            index = closer + marker;
            plain_start = index;
        }
        if plain_start < content.len() {
            self.append_visible(
                &mut display,
                &mut display_to_source,
                &mut runs,
                content_start + plain_start,
                &content[plain_start..],
                content_style,
            );
        }
        if let Some(last) = display_to_source.last_mut() {
            *last = text.len();
        }
        (display, display_to_source, runs)
    }

    fn shape(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.last_bounds = Some(bounds);
        self.viewport_height = bounds.size.height;
        let width = bounds.size.width.max(px(16.0));
        let mut lines = Vec::new();
        let mut start = 0;
        for text in self.content.split('\n') {
            let (display, display_to_source, runs) = self.render_line(text);
            let shaped = window
                .text_system()
                .shape_text(
                    SharedString::from(display),
                    px(FONT_SIZE),
                    &runs,
                    Some(width),
                    None,
                )
                .ok()
                .and_then(|mut lines| lines.pop());
            if let Some(line) = shaped {
                lines.push(ShapedLine {
                    start,
                    end: start + text.len(),
                    display_to_source,
                    line,
                });
            }
            start += text.len() + 1;
        }
        self.lines = lines;
        let measured_height = self.total_height();
        if self.measured_height != measured_height {
            self.measured_height = measured_height;
            cx.notify();
        }
        self.ensure_caret_visible();
    }

    fn paint_lines(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let focused = self.focus_handle.is_focused(window);
        let scroll_offset = self
            .scroll_offset
            .min((f32::from(self.total_height()) - f32::from(bounds.size.height)).max(0.0));
        let mut y = bounds.origin.y - px(scroll_offset);
        for shaped in &self.lines {
            let height = shaped.line.size(px(LINE_HEIGHT)).height;
            let origin = point(bounds.origin.x, y);
            let line_bounds = Bounds::new(origin, size(bounds.size.width, height));

            if focused && !self.selected_range.is_empty() {
                let start = shaped.start;
                let end = shaped.end;
                let selected =
                    self.selected_range.start.max(start)..self.selected_range.end.min(end);
                if selected.start < selected.end {
                    let from_source = selected.start.saturating_sub(start);
                    let to_source = selected.end.saturating_sub(start);
                    let from_index = shaped
                        .display_to_source
                        .iter()
                        .rposition(|position| *position <= from_source)
                        .unwrap_or(0);
                    let to_index = shaped
                        .display_to_source
                        .iter()
                        .rposition(|position| *position <= to_source)
                        .unwrap_or(from_index);
                    if let (Some(from), Some(to)) = (
                        shaped.line.position_for_index(from_index, px(LINE_HEIGHT)),
                        shaped.line.position_for_index(to_index, px(LINE_HEIGHT)),
                    ) {
                        let first_row = (f32::from(from.y) / LINE_HEIGHT) as usize;
                        let last_row = (f32::from(to.y) / LINE_HEIGHT) as usize;
                        for row in first_row..=last_row {
                            let row_top = y + px(row as f32 * LINE_HEIGHT);
                            let left = if row == first_row { from.x } else { px(0.0) };
                            let right = if row == last_row {
                                to.x
                            } else {
                                bounds.size.width
                            };
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(bounds.origin.x + left, row_top),
                                    size((right - left).max(px(1.0)), px(LINE_HEIGHT)),
                                ),
                                self.selection_color,
                            ));
                        }
                    }
                }
            }

            let _ = shaped.line.paint(
                origin,
                px(LINE_HEIGHT),
                TextAlign::Left,
                Some(line_bounds),
                window,
                cx,
            );
            y += height;
        }

        if focused {
            let caret = self.point_for_offset(self.cursor_offset());
            let cursor: PaintQuad = fill(
                Bounds::new(
                    point(
                        bounds.origin.x + caret.x,
                        bounds.origin.y + caret.y - px(scroll_offset),
                    ),
                    size(px(1.5), px(LINE_HEIGHT)),
                ),
                self.caret_color,
            );
            window.paint_quad(cursor);
        }

        if self.content.is_empty() {
            let placeholder = TextRun {
                len: self.placeholder.len(),
                font: Font {
                    family: ".SystemUIFont".into(),
                    features: Default::default(),
                    fallbacks: None,
                    weight: FontWeight::NORMAL,
                    style: FontStyle::Normal,
                },
                color: self.placeholder_color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            if let Ok(mut shaped) = window.text_system().shape_text(
                self.placeholder.clone(),
                px(FONT_SIZE),
                std::slice::from_ref(&placeholder),
                Some(bounds.size.width),
                None,
            ) {
                if let Some(line) = shaped.pop() {
                    let _ = line.paint(
                        bounds.origin,
                        px(LINE_HEIGHT),
                        TextAlign::Left,
                        Some(bounds),
                        window,
                        cx,
                    );
                }
            }
        }
    }
}

impl Focusable for Editor {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EntityInputHandler for Editor {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.remember();
        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            new_text,
            &self.content[range.end..]
        )
        .into();
        let caret = range.start + new_text.len();
        self.selected_range = caret..caret;
        self.marked_range.take();
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range_utf16| self.range_from_utf16(range_utf16))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());

        self.remember();
        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            new_text,
            &self.content[range.end..]
        )
        .into();
        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range_utf16| {
                let selected = self.range_from_utf16(range_utf16);
                range.start + selected.start..range.start + selected.end
            })
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let from = self.point_for_offset(range.start);
        let to = self.point_for_offset(range.end);
        Some(Bounds::from_corners(
            point(bounds.origin.x + from.x, bounds.origin.y + from.y),
            point(
                bounds.origin.x + to.x,
                bounds.origin.y + to.y + px(LINE_HEIGHT),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        position: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let local = position - bounds.origin;
        let offset = self.offset_for_point(local);
        Some(self.offset_to_utf16(offset))
    }
}

impl Render for Editor {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let entity = cx.entity();
        let focus = self.focus_handle.clone();
        let paint_entity = entity.clone();
        let paint_focus = focus.clone();
        let color = self.color;

        div()
            .id("oxide-editor")
            .size_full()
            .key_context("Editor")
            .track_focus(&focus)
            .cursor(gpui::CursorStyle::IBeam)
            .overflow_hidden()
            .text_color(color)
            .child(
                canvas(
                    move |bounds, window, cx| {
                        entity.update(cx, |editor, cx| editor.shape(bounds, window, cx));
                    },
                    move |bounds, _, window, cx| {
                        window.handle_input(
                            &paint_focus,
                            ElementInputHandler::new(bounds, paint_entity.clone()),
                            cx,
                        );
                        paint_entity
                            .update(cx, |editor, cx| editor.paint_lines(bounds, window, cx));
                    },
                )
                .size_full(),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, window, cx| {
                    this.focus_handle.focus(window);
                    let Some(bounds) = this.last_bounds else {
                        return;
                    };
                    let local = event.position - bounds.origin;
                    let offset = this.offset_for_point(local);
                    if event.modifiers.shift {
                        this.select_to(offset, cx);
                    } else if event.click_count == 2 {
                        let start = this.previous_word_boundary(offset);
                        let end = this.next_word_boundary(offset);
                        this.selected_range = start..end;
                        this.selection_reversed = false;
                        cx.notify();
                    } else if event.click_count >= 3 {
                        let (line, _) = this.line_of(offset);
                        let range = this.line_bounds()[line].clone();
                        this.selected_range = range;
                        this.selection_reversed = false;
                        cx.notify();
                    } else {
                        this.move_to(offset, cx);
                    }
                    this.selecting = true;
                    cx.notify();
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _window, cx| {
                if !this.selecting {
                    return;
                }
                let Some(bounds) = this.last_bounds else {
                    return;
                };
                let local = event.position - bounds.origin;
                let offset = this.offset_for_point(local);
                this.select_to(offset, cx);
            }))
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _: &MouseUpEvent, _window, cx| {
                    this.selecting = false;
                    cx.notify();
                }),
            )
            .on_action(cx.listener(|this, _: &Backspace, _window, cx| this.backspace(cx)))
            .on_action(cx.listener(|this, _: &Delete, _window, cx| this.delete_forward(cx)))
            .on_action(cx.listener(|this, _: &Left, _window, cx| this.move_left(false, cx)))
            .on_action(cx.listener(|this, _: &Right, _window, cx| this.move_right(false, cx)))
            .on_action(
                cx.listener(|this, _: &Up, _window, cx| this.move_vertical(false, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &Down, _window, cx| this.move_vertical(true, false, cx)),
            )
            .on_action(cx.listener(|this, _: &SelectLeft, _window, cx| this.move_left(true, cx)))
            .on_action(cx.listener(|this, _: &SelectRight, _window, cx| this.move_right(true, cx)))
            .on_action(
                cx.listener(|this, _: &WordLeft, _window, cx| this.move_word(true, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &WordRight, _window, cx| this.move_word(false, false, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectWordLeft, _window, cx| this.move_word(true, true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectWordRight, _window, cx| {
                    this.move_word(false, true, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &SelectUp, _window, cx| this.move_vertical(false, true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectDown, _window, cx| this.move_vertical(true, true, cx)),
            )
            .on_action(cx.listener(|this, _: &Home, _window, cx| this.home(false, cx)))
            .on_action(cx.listener(|this, _: &End, _window, cx| this.end(false, cx)))
            .on_action(cx.listener(|this, _: &SelectHome, _window, cx| this.home(true, cx)))
            .on_action(cx.listener(|this, _: &SelectEnd, _window, cx| this.end(true, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _window, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &Copy, _window, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Cut, _window, cx| this.cut(cx)))
            .on_action(cx.listener(|this, _: &Paste, _window, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &Undo, _window, cx| this.undo(cx)))
            .on_action(cx.listener(|this, _: &Redo, _window, cx| this.redo(cx)))
            .on_action(cx.listener(|this, _: &InsertNewline, _window, cx| this.insert_newline(cx)))
            .on_action(
                cx.listener(|this, _: &DeleteToLineStart, _window, cx| {
                    this.delete_to_line_start(cx)
                }),
            )
            .on_action(cx.listener(|this, _: &DeleteWordBackward, _window, cx| {
                this.delete_word_backward(cx)
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _window, cx| {
                let delta = match event.delta {
                    ScrollDelta::Pixels(delta) => delta.y,
                    ScrollDelta::Lines(delta) => px(delta.y * LINE_HEIGHT),
                };
                this.scroll(delta, cx);
            }))
    }
}

fn clipboard_image_attachment(item: &ClipboardItem) -> Option<(String, String)> {
    let image = item.entries().iter().find_map(|entry| match entry {
        ClipboardEntry::Image(image) => Some(image),
        ClipboardEntry::String(_) => None,
    })?;
    let extension = match image.format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
    };
    Some((
        format!("pasted-image.{extension}"),
        format!(
            "data:{};base64,{}",
            image.format.mime_type(),
            oxide_core::media::base64_encode(&image.bytes)
        ),
    ))
}

fn line_marker(text: &str) -> (usize, RunStyle, RunStyle) {
    let indent = text.len() - text.trim_start().len();
    let trimmed = &text[indent..];
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return (text.len(), RunStyle::Syntax, RunStyle::Syntax);
    }
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
        let spaces = trimmed[hashes..]
            .chars()
            .take_while(|ch| *ch == ' ')
            .count();
        return (indent + hashes + spaces, RunStyle::Syntax, RunStyle::Bold);
    }
    if let Some(quoted) = trimmed.strip_prefix("> ") {
        let spaces = quoted.chars().take_while(|ch| *ch == ' ').count();
        return (indent + 2 + spaces, RunStyle::Syntax, RunStyle::Quote);
    }
    for marker in ["- ", "* ", "+ "] {
        if trimmed.starts_with(marker) {
            return (indent + marker.len(), RunStyle::Syntax, RunStyle::Normal);
        }
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && matches!(trimmed.as_bytes().get(digits), Some(b'.' | b')'))
        && trimmed.as_bytes().get(digits + 1) == Some(&b' ')
    {
        return (indent + digits + 2, RunStyle::Syntax, RunStyle::Normal);
    }
    (indent, RunStyle::Syntax, RunStyle::Normal)
}

fn inline_marker(text: &str, index: usize) -> Option<(usize, usize, RunStyle)> {
    let rest = &text[index..];
    let (marker, style) = if rest.starts_with("**") || rest.starts_with("__") {
        (2, RunStyle::Bold)
    } else if rest.starts_with("~~") {
        (2, RunStyle::Italic)
    } else if rest.starts_with('`') {
        (1, RunStyle::Code)
    } else if rest.starts_with('*') || rest.starts_with('_') {
        (1, RunStyle::Italic)
    } else {
        return None;
    };
    let opener = &rest[..marker];
    let next = rest[marker..].chars().next()?;
    if next.is_whitespace() {
        return None;
    }
    let closer = index + marker + rest[marker..].find(opener)?;
    if closer <= index + marker {
        return None;
    }
    if style != RunStyle::Code {
        let after = text[closer + marker..].chars().next();
        if after.is_some_and(|ch| ch.is_alphanumeric() || ch == '_') {
            return None;
        }
    }
    Some((marker, closer, style))
}

fn marker_content_start(text: &str) -> Option<usize> {
    let indent = text.len() - text.trim_start().len();
    let trimmed = &text[indent..];
    if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
        return Some(indent + 3);
    }
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if (1..=6).contains(&hashes) && trimmed.as_bytes().get(hashes) == Some(&b' ') {
        let spaces = trimmed[hashes..]
            .chars()
            .take_while(|ch| *ch == ' ')
            .count();
        return Some(indent + hashes + spaces);
    }
    if let Some(quoted) = trimmed.strip_prefix("> ") {
        let spaces = quoted.chars().take_while(|ch| *ch == ' ').count();
        return Some(indent + 2 + spaces);
    }
    for marker in ["- ", "* ", "+ "] {
        if let Some(stripped) = trimmed.strip_prefix(marker) {
            if let Some(task) = stripped.strip_prefix("[ ] ") {
                let _ = task;
                return Some(indent + marker.len() + 4);
            }
            if let Some(task) = stripped.strip_prefix("[x] ") {
                let _ = task;
                return Some(indent + marker.len() + 4);
            }
            return Some(indent + marker.len());
        }
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits > 0
        && matches!(trimmed.as_bytes().get(digits), Some(b'.' | b')'))
        && trimmed.as_bytes().get(digits + 1) == Some(&b' ')
    {
        return Some(indent + digits + 2);
    }
    None
}

fn continuation_prefix(text: &str) -> Option<String> {
    let start = marker_content_start(text)?;
    if start >= text.len() {
        return None;
    }
    let indent = &text[..text.len() - text.trim_start().len()];
    let marked = &text[indent.len()..start];
    if marked.starts_with('>') {
        return Some(format!("{indent}> "));
    }
    if marked.starts_with('-') || marked.starts_with('*') || marked.starts_with('+') {
        if text[start..].starts_with("[ ] ") {
            return Some(format!("{indent}- [ ] "));
        }
        if text[start..].starts_with("[x] ") || text[start..].starts_with("[X] ") {
            return Some(format!("{indent}- [ ] "));
        }
        return Some(format!("{indent}{marked}"));
    }
    let digits = marked
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>()
        .parse::<u64>()
        .ok()?;
    let separator = if marked.contains(')') { ')' } else { '.' };
    Some(format!("{indent}{}{separator} ", digits + 1))
}

#[cfg(test)]
mod tests {
    use super::{
        clipboard_image_attachment, continuation_prefix, line_marker, marker_content_start,
        RunStyle,
    };
    use gpui::{ClipboardItem, Image, ImageFormat};

    #[test]
    fn composer_markers_keep_the_source_but_style_the_text() {
        assert_eq!(
            line_marker("## Plan"),
            (3, RunStyle::Syntax, RunStyle::Bold)
        );
        assert_eq!(
            line_marker("> quoted"),
            (2, RunStyle::Syntax, RunStyle::Quote)
        );
        assert_eq!(
            line_marker("  - item"),
            (4, RunStyle::Syntax, RunStyle::Normal)
        );
    }

    #[test]
    fn shift_enter_continues_a_markdown_block() {
        assert_eq!(continuation_prefix("- item"), Some("- ".to_string()));
        assert_eq!(continuation_prefix("  > quote"), Some("  > ".to_string()));
        assert_eq!(continuation_prefix("9. item"), Some("10. ".to_string()));
        assert_eq!(continuation_prefix("- "), None);
    }

    #[test]
    fn backspace_can_find_the_marker_content_boundary() {
        assert_eq!(marker_content_start("### "), Some(4));
        assert_eq!(marker_content_start("- [x] done"), Some(6));
        assert_eq!(marker_content_start("plain"), None);
    }

    #[test]
    fn a_clipboard_picture_becomes_an_image_data_url() {
        let item = ClipboardItem::new_image(&Image::from_bytes(
            ImageFormat::Png,
            b"\x89PNG\r\n\x1a\n".to_vec(),
        ));
        let (name, data_url) = clipboard_image_attachment(&item).unwrap();
        assert_eq!(name, "pasted-image.png");
        assert_eq!(data_url, "data:image/png;base64,iVBORw0KGgo=");
    }
}
