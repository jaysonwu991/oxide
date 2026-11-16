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
    MouseMoveEvent, MouseUpEvent, PaintQuad, Pixels, Point, ScrollDelta, ScrollEvent, SharedString,
    StrikethroughStyle, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use std::ops::Range;
use std::path::Path;
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
const MONO_FONT: &str = "Menlo";
const MAX_HISTORY: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunStyle {
    Normal,
    Bold,
    BoldItalic,
    Italic,
    Quote,
    Code,
    Strike,
    Link,
    Task,
    Syntax,
    Decoration,
}

struct ShapedLine {
    start: usize,
    end: usize,
    line_height: Pixels,
    display_to_source: Vec<usize>,
    line: gpui::WrappedLine,
}

impl ShapedLine {
    fn height(&self) -> Pixels {
        self.line.size(self.line_height).height
    }
}

/// One composer line painted: the text the reader sees, the byte of the source
/// each of its bytes stands for, the runs that style it and the size a heading
/// gives it. A heading is drawn larger than the line it is written in, so the
/// size travels with the line rather than being the editor's one constant.
struct RenderedLine {
    display: String,
    display_to_source: Vec<usize>,
    runs: Vec<TextRun>,
    font_size: Pixels,
    line_height: Pixels,
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
    muted_color: gpui::Hsla,
    placeholder_color: gpui::Hsla,
    caret_color: gpui::Hsla,
    selection_color: gpui::Hsla,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    single_line: bool,
    scroll_offset: f32,
    viewport_height: Pixels,
}

impl Editor {
    pub fn new(
        placeholder: impl Into<SharedString>,
        color: gpui::Hsla,
        muted_color: gpui::Hsla,
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
            muted_color,
            placeholder_color,
            caret_color,
            selection_color,
            undo: Vec::new(),
            redo: Vec::new(),
            single_line: false,
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

    pub fn single_line(mut self) -> Self {
        self.single_line = true;
        self
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
        // The rows of the rendered lines, so the caret steps one painted row at
        // a time: a heading's row is taller than plain text's, and a wrapped
        // line holds several.
        let mut rows: Vec<(f32, f32)> = Vec::new();
        let mut row_top = 0.0;
        for shaped in &self.lines {
            let row_height = f32::from(shaped.line_height).max(1.0);
            let count = ((f32::from(shaped.height()) / row_height).round() as usize).max(1);
            for _ in 0..count {
                rows.push((row_top, row_height));
                row_top += row_height;
            }
        }
        let Some(current) = rows
            .iter()
            .position(|(top, height)| f32::from(caret.y) < top + height)
        else {
            return;
        };
        let target = if down {
            current + 1
        } else {
            match current.checked_sub(1) {
                Some(target) => target,
                None => return,
            }
        };
        let y = rows.get(target).map_or(row_top + 0.5, |(top, _)| top + 0.5);
        let target = self.offset_for_point(point(caret.x, px(y)));
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
        // Two reads answer a paste. The window's own read is in process and
        // knows the text and the picture at once; the shared media reader is
        // the one that names the file a Finder copy carries, and it asks the
        // pasteboard through a subprocess. Text needs only the first, so it is
        // inserted now. A picture waits for the reader on the app's own task,
        // since a read this slow would otherwise hold the window still.
        let clipboard = cx.read_from_clipboard();
        let picture = clipboard.as_ref().and_then(clipboard_image_attachment);
        let text = clipboard.and_then(|item| item.text());
        if picture.is_none() {
            if let Some(text) = text.as_ref().filter(|text| !clipboard_names_file(text)) {
                self.insert(text, cx);
                return;
            }
        }
        let pasted = cx
            .background_executor()
            .spawn(async move { oxide_core::media::clipboard_media().or(picture) });
        cx.spawn(async move |this, cx| {
            let Some((name, data_url)) = pasted.await else {
                if let Some(text) = text {
                    let _ = this.update(cx, |editor, cx| editor.insert(&text, cx));
                }
                return;
            };
            let _ = this.update(cx, |_editor, cx| {
                cx.emit(EditorEvent::AttachmentPasted { name, data_url });
            });
        })
        .detach();
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

    fn toggle_task_at_point(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) -> bool {
        let offset = self.offset_for_point(position);
        let (line, _) = self.line_of(offset);
        let range = self.line_bounds()[line].clone();
        let Some((marker, checked)) = task_marker(&self.content[range.clone()]) else {
            return false;
        };
        let indent =
            self.content[range.clone()].len() - self.content[range.clone()].trim_start().len();
        if f32::from(position.x) > (indent + 2) as f32 * 8.0 + 12.0 {
            return false;
        }
        self.remember();
        let marker = range.start + marker;
        let replacement = if checked { " " } else { "x" };
        self.content = format!(
            "{}{}{}",
            &self.content[..marker],
            replacement,
            &self.content[marker + 1..]
        )
        .into();
        self.selected_range = marker + 1..marker + 1;
        self.selection_reversed = false;
        cx.emit(EditorEvent::Changed);
        cx.notify();
        true
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
                if let Some(position) = shaped.line.position_for_index(display, shaped.line_height)
                {
                    return point(position.x, y + position.y);
                }
            }
            y += shaped.height();
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
            let height = shaped.height();
            if position.y <= y + height {
                let within = point(position.x, position.y - y);
                return match shaped.line.index_for_position(within, shaped.line_height) {
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
            .map(ShapedLine::height)
            .fold(px(0.0), |sum, height| sum + height)
            .max(px(LINE_HEIGHT))
    }

    /// The height of one painted row of the line holding `offset`, so a caret in
    /// a heading is as tall as the heading's row rather than as tall as plain
    /// text, and a wrapped line's caret is one row rather than the paragraph.
    fn row_height_at(&self, offset: usize) -> Pixels {
        self.lines
            .iter()
            .find(|shaped| offset <= shaped.end)
            .map(|shaped| shaped.line_height)
            .unwrap_or(px(LINE_HEIGHT))
    }

    fn max_scroll(&self) -> f32 {
        (f32::from(self.total_height()) - f32::from(self.viewport_height)).max(0.0)
    }

    fn ensure_caret_visible(&mut self) {
        let caret = self.point_for_offset(self.cursor_offset());
        let top = f32::from(caret.y);
        let bottom = top + f32::from(self.row_height_at(self.cursor_offset()));
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

    fn mono_font(&self) -> Font {
        Font {
            family: MONO_FONT.into(),
            features: Default::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        }
    }

    fn push_run(&self, runs: &mut Vec<TextRun>, text: &str, style: RunStyle) {
        if text.is_empty() {
            return;
        }
        let mut run = TextRun {
            len: text.len(),
            font: self.editor_font(FontWeight::NORMAL, FontStyle::Normal),
            color: self.color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        match style {
            RunStyle::Normal => {}
            RunStyle::Bold => run.font.weight = FontWeight::SEMIBOLD,
            RunStyle::BoldItalic => {
                run.font.weight = FontWeight::SEMIBOLD;
                run.font.style = FontStyle::Italic;
            }
            RunStyle::Italic => run.font.style = FontStyle::Italic,
            RunStyle::Quote => run.color = self.muted_color,
            RunStyle::Code => {
                run.font = self.mono_font();
                let mut background = self.color;
                background.a = 0.12;
                run.background_color = Some(background);
            }
            RunStyle::Strike => {
                run.strikethrough = Some(StrikethroughStyle {
                    thickness: px(1.0),
                    color: Some(self.color),
                });
            }
            RunStyle::Link => {
                run.color = self.caret_color;
                run.underline = Some(UnderlineStyle {
                    thickness: px(1.0),
                    color: Some(self.caret_color),
                    wavy: false,
                });
            }
            RunStyle::Task => run.color = self.caret_color,
            RunStyle::Syntax => run.color = transparent_black(),
            RunStyle::Decoration => {}
        }
        runs.push(run);
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

    fn render_line(&self, text: &str, fenced: bool) -> RenderedLine {
        let mut display = String::new();
        let mut display_to_source = vec![0];
        let mut runs = Vec::new();
        if fenced {
            self.append_visible(
                &mut display,
                &mut display_to_source,
                &mut runs,
                0,
                text,
                RunStyle::Code,
            );
            if let Some(last) = display_to_source.last_mut() {
                *last = text.len();
            }
            return RenderedLine {
                display,
                display_to_source,
                runs,
                font_size: px(FONT_SIZE),
                line_height: px(LINE_HEIGHT),
            };
        }
        let (prefix_end, _prefix_style, content_style) = line_marker(text);
        let scale = if self.single_line {
            1.0
        } else {
            heading_scale(text)
        };
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
                Some(("│ ".to_string(), RunStyle::Quote))
            } else if trimmed.starts_with("- [ ] ")
                || trimmed.starts_with("* [ ] ")
                || trimmed.starts_with("+ [ ] ")
            {
                Some(("☐ ".to_string(), RunStyle::Task))
            } else if trimmed.starts_with("- [x] ")
                || trimmed.starts_with("* [x] ")
                || trimmed.starts_with("+ [x] ")
                || trimmed.starts_with("- [X] ")
                || trimmed.starts_with("* [X] ")
                || trimmed.starts_with("+ [X] ")
            {
                Some(("☑ ".to_string(), RunStyle::Task))
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
            if let Some((entity, next)) = entity_marker(content, index) {
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
                Self::append_hidden(&mut display_to_source, content_start + index);
                self.append_visible(
                    &mut display,
                    &mut display_to_source,
                    &mut runs,
                    content_start + index,
                    entity.encode_utf8(&mut [0; 4]),
                    content_style,
                );
                Self::append_hidden(&mut display_to_source, content_start + next);
                index = next;
                plain_start = index;
                continue;
            }
            if content[index..].starts_with('\\') {
                let next = content[index + 1..].chars().next();
                if next.is_some_and(is_markdown_punctuation) {
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
                    let next_end = index + 1 + next.map_or(0, char::len_utf8);
                    Self::append_hidden(&mut display_to_source, content_start + index + 1);
                    self.append_visible(
                        &mut display,
                        &mut display_to_source,
                        &mut runs,
                        content_start + index + 1,
                        &content[index + 1..next_end],
                        content_style,
                    );
                    index = next_end;
                    plain_start = index;
                    continue;
                }
            }
            if let Some(link) = autolink_marker(content, index) {
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
                if link.angle {
                    Self::append_hidden(&mut display_to_source, content_start + link.text_start);
                }
                self.append_visible(
                    &mut display,
                    &mut display_to_source,
                    &mut runs,
                    content_start + link.text_start,
                    &content[link.text_start..link.text_end],
                    RunStyle::Link,
                );
                Self::append_hidden(&mut display_to_source, content_start + link.next);
                index = link.next;
                plain_start = index;
                continue;
            }
            if let Some(link) = link_marker(content, index) {
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
                Self::append_hidden(&mut display_to_source, content_start + link.text_start);
                self.append_visible(
                    &mut display,
                    &mut display_to_source,
                    &mut runs,
                    content_start + link.text_start,
                    &content[link.text_start..link.text_end],
                    RunStyle::Link,
                );
                Self::append_hidden(&mut display_to_source, content_start + link.next);
                index = link.next;
                plain_start = index;
                continue;
            }
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
        RenderedLine {
            display,
            display_to_source,
            runs,
            font_size: px(FONT_SIZE * scale),
            line_height: px(LINE_HEIGHT * scale),
        }
    }

    fn shape(&mut self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        self.last_bounds = Some(bounds);
        self.viewport_height = bounds.size.height;
        let width = bounds.size.width.max(px(16.0));
        let mut lines = Vec::new();
        let mut start = 0;
        let mut fence = None;
        for text in self.content.split('\n') {
            let current_fence = fence;
            if let Some(marker) = current_fence {
                if fence_closes(text, marker) {
                    fence = None;
                }
            } else if let Some(marker) = fence_opens(text) {
                fence = Some(marker);
            }
            let rendered = self.render_line(
                text,
                current_fence.is_some_and(|marker| !fence_closes(text, marker)),
            );
            let shaped = window
                .text_system()
                .shape_text(
                    SharedString::from(rendered.display),
                    rendered.font_size,
                    &rendered.runs,
                    if self.single_line { None } else { Some(width) },
                    None,
                )
                .ok()
                .and_then(|mut lines| lines.pop());
            if let Some(line) = shaped {
                lines.push(ShapedLine {
                    start,
                    end: start + text.len(),
                    line_height: rendered.line_height,
                    display_to_source: rendered.display_to_source,
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
        let vertical_padding = if self.single_line {
            let line_height = self
                .lines
                .first()
                .map_or(px(LINE_HEIGHT), ShapedLine::height);
            (f32::from(bounds.size.height) - f32::from(line_height)).max(0.0) / 2.0
        } else {
            0.0
        };
        let text_origin = point(
            bounds.origin.x,
            bounds.origin.y + px(vertical_padding) - px(scroll_offset),
        );
        let mut y = text_origin.y;
        for shaped in &self.lines {
            let height = shaped.height();
            let row_height = f32::from(shaped.line_height);
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
                        shaped
                            .line
                            .position_for_index(from_index, shaped.line_height),
                        shaped.line.position_for_index(to_index, shaped.line_height),
                    ) {
                        let first_row = (f32::from(from.y) / row_height) as usize;
                        let last_row = (f32::from(to.y) / row_height) as usize;
                        for row in first_row..=last_row {
                            let row_top = y + px(row as f32 * row_height);
                            let left = if row == first_row { from.x } else { px(0.0) };
                            let right = if row == last_row {
                                to.x
                            } else {
                                bounds.size.width
                            };
                            window.paint_quad(fill(
                                Bounds::new(
                                    point(bounds.origin.x + left, row_top),
                                    size((right - left).max(px(1.0)), shaped.line_height),
                                ),
                                self.selection_color,
                            ));
                        }
                    }
                }
            }

            let _ = shaped.line.paint(
                origin,
                shaped.line_height,
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
                    point(text_origin.x + caret.x, text_origin.y + caret.y),
                    size(px(1.5), self.row_height_at(self.cursor_offset())),
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
                        text_origin,
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
                bounds.origin.y + to.y + self.row_height_at(range.end),
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
        let editor_id = ("oxide-editor", cx.entity_id());

        div()
            .id(editor_id)
            .size_full()
            .flex_1()
            .min_w(px(0.0))
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
                    this.focus_handle.focus(window, cx);
                    let Some(bounds) = this.last_bounds else {
                        return;
                    };
                    let local = event.position - bounds.origin;
                    if this.toggle_task_at_point(local, cx) {
                        this.selecting = false;
                        return;
                    }
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
            .on_scroll(cx.listener(|this, event: &ScrollEvent, _window, cx| {
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
        ClipboardEntry::String(_) | ClipboardEntry::ExternalPaths(_) => None,
    })?;
    let extension = match image.format {
        ImageFormat::Png => "png",
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        ImageFormat::Ico => "ico",
        ImageFormat::Pnm => "pnm",
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

fn clipboard_names_file(text: &str) -> bool {
    let text = text.trim();
    text.starts_with("file://") || (!text.contains('\n') && Path::new(text).exists())
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

fn fence_opens(text: &str) -> Option<char> {
    let trimmed = text.trim_start();
    let marker = *trimmed.as_bytes().first()? as char;
    if !matches!(marker, '`' | '~') {
        return None;
    }
    let count = trimmed.chars().take_while(|ch| *ch == marker).count();
    (count >= 3).then_some(marker)
}

fn fence_closes(text: &str, marker: char) -> bool {
    let trimmed = text.trim();
    let count = trimmed.chars().take_while(|ch| *ch == marker).count();
    count >= 3 && trimmed.chars().all(|ch| ch == marker)
}

/// The size a heading line is drawn at, as the page's composer grows one: two
/// times the composer's text at the first level, then the levels the page's own
/// headings scale by. A line that is not a heading keeps the composer's size.
fn heading_scale(text: &str) -> f32 {
    let indent = text.len() - text.trim_start().len();
    let trimmed = &text[indent..];
    let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&hashes) || trimmed.as_bytes().get(hashes) != Some(&b' ') {
        return 1.0;
    }
    match hashes {
        1 => 2.0,
        2 => 1.5,
        3 => 1.17,
        4 => 1.0,
        5 => 0.83,
        _ => 0.67,
    }
}

/// The `[text](target)` a composer line holds: where its text starts and ends
/// and where the line carries on afterwards. Only the text is drawn; the
/// brackets and the target stay in the source, the way the page renders a link.
struct LinkMarker {
    text_start: usize,
    text_end: usize,
    next: usize,
}

struct AutoLinkMarker {
    text_start: usize,
    text_end: usize,
    next: usize,
    angle: bool,
}

fn autolink_marker(text: &str, index: usize) -> Option<AutoLinkMarker> {
    if text[index..].starts_with('<') {
        let close = text[index + 1..].find('>')? + index + 1;
        let target = &text[index + 1..close];
        if !(target.starts_with("http://") || target.starts_with("https://")) {
            return None;
        }
        return Some(AutoLinkMarker {
            text_start: index + 1,
            text_end: close,
            next: close + 1,
            angle: true,
        });
    }
    if !(text[index..].starts_with("http://") || text[index..].starts_with("https://")) {
        return None;
    }
    let mut end = text[index..]
        .char_indices()
        .find_map(|(offset, ch)| {
            (ch.is_whitespace() || matches!(ch, '<' | '>' | '`')).then_some(index + offset)
        })
        .unwrap_or(text.len());
    while end > index
        && text[..end]
            .chars()
            .last()
            .is_some_and(|ch| matches!(ch, '.' | ',' | ';' | ':' | '!' | '?'))
    {
        end -= text[..end].chars().last().map_or(1, char::len_utf8);
    }
    while text[index..end].ends_with(')')
        && text[index..end].matches(')').count() > text[index..end].matches('(').count()
    {
        end -= 1;
    }
    Some(AutoLinkMarker {
        text_start: index,
        text_end: end,
        next: end,
        angle: false,
    })
}

fn link_marker(text: &str, index: usize) -> Option<LinkMarker> {
    let body = text[index..].strip_prefix('[')?;
    let mut label_depth = 1usize;
    let mut close = None;
    let mut characters = body.char_indices().peekable();
    while let Some((offset, character)) = characters.next() {
        match character {
            '\\' => {
                characters.next();
            }
            '[' => label_depth += 1,
            ']' => {
                label_depth -= 1;
                if label_depth == 0 {
                    close = Some(offset);
                    break;
                }
            }
            _ => {}
        }
    }
    let close = close?;
    if close == 0 {
        return None;
    }
    let after = body[close + 1..].strip_prefix('(')?;
    let mut cursor = 0usize;
    while after[cursor..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
    {
        cursor += after[cursor..].chars().next()?.len_utf8();
    }
    let target_start = cursor;
    if after[cursor..].starts_with('<') {
        let angle = after[cursor + 1..].find('>')?;
        cursor += angle + 2;
    } else {
        let mut depth = 0usize;
        for (offset, character) in after[target_start..].char_indices() {
            match character {
                '(' => depth += 1,
                ')' if depth == 0 => break,
                ')' => depth -= 1,
                character if depth == 0 && character.is_whitespace() => break,
                _ => {}
            }
            cursor = target_start + offset + character.len_utf8();
        }
    }
    if cursor == target_start {
        return None;
    }
    while after[cursor..]
        .chars()
        .next()
        .is_some_and(char::is_whitespace)
    {
        cursor += after[cursor..].chars().next()?.len_utf8();
    }
    if let Some(quote @ ('"' | '\'')) = after[cursor..].chars().next() {
        let title = &after[cursor + quote.len_utf8()..];
        let end = title.find(quote)?;
        cursor += quote.len_utf8() + end + quote.len_utf8();
        while after[cursor..]
            .chars()
            .next()
            .is_some_and(char::is_whitespace)
        {
            cursor += after[cursor..].chars().next()?.len_utf8();
        }
    }
    if !after[cursor..].starts_with(')') {
        return None;
    }
    let text_start = index + 1;
    let text_end = text_start + close;
    Some(LinkMarker {
        text_start,
        text_end,
        next: index + close + cursor + 4,
    })
}

fn is_markdown_punctuation(ch: char) -> bool {
    "\\`*_{}[]()#+-.!>~|".contains(ch)
}

fn entity_marker(text: &str, index: usize) -> Option<(char, usize)> {
    let rest = text[index..].strip_prefix('&')?;
    let end = rest.find(';')?;
    if end == 0 || end > 12 {
        return None;
    }
    let name = &rest[..end];
    let character = match name.to_ascii_lowercase().as_str() {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => '\u{a0}',
        name if name.starts_with("#x") => u32::from_str_radix(&name[2..], 16)
            .ok()
            .and_then(char::from_u32)?,
        name if name.starts_with('#') => name[1..].parse().ok().and_then(char::from_u32)?,
        _ => return None,
    };
    Some((character, index + end + 2))
}

fn inline_marker(text: &str, index: usize) -> Option<(usize, usize, RunStyle)> {
    let rest = &text[index..];
    let (marker, style) = if rest.starts_with("***") || rest.starts_with("___") {
        (3, RunStyle::BoldItalic)
    } else if rest.starts_with("**") || rest.starts_with("__") {
        (2, RunStyle::Bold)
    } else if rest.starts_with("~~") {
        (2, RunStyle::Strike)
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

fn task_marker(text: &str) -> Option<(usize, bool)> {
    let indent = text.len() - text.trim_start().len();
    let trimmed = &text[indent..];
    for marker in ["- ", "* ", "+ "] {
        let Some(rest) = trimmed.strip_prefix(marker) else {
            continue;
        };
        let checked = match rest.as_bytes().get(1) {
            Some(b' ') => false,
            Some(b'x' | b'X') => true,
            _ => return None,
        };
        if rest.as_bytes().first() == Some(&b'[')
            && rest.as_bytes().get(2) == Some(&b']')
            && rest.as_bytes().get(3) == Some(&b' ')
        {
            return Some((indent + marker.len() + 1, checked));
        }
        return None;
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
        autolink_marker, clipboard_image_attachment, clipboard_names_file, continuation_prefix,
        entity_marker, fence_closes, fence_opens, heading_scale, inline_marker, line_marker,
        link_marker, marker_content_start, task_marker, RunStyle,
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
    fn a_heading_is_drawn_at_the_size_its_level_names() {
        assert_eq!(heading_scale("# Plan"), 2.0);
        assert_eq!(heading_scale("## Plan"), 1.5);
        assert_eq!(heading_scale("### Plan"), 1.17);
        assert_eq!(heading_scale("#### Plan"), 1.0);
        assert_eq!(heading_scale("##### Plan"), 0.83);
        assert_eq!(heading_scale("###### Plan"), 0.67);
        assert_eq!(heading_scale("#Plan"), 1.0);
        assert_eq!(heading_scale("- item"), 1.0);
    }

    #[test]
    fn a_link_keeps_its_text_and_hides_its_target() {
        let link = link_marker("[Oxide](https://example.com) here", 0).unwrap();
        assert_eq!(link.text_start, 1);
        assert_eq!(link.text_end, 6);
        assert_eq!(link.next, 28);
        assert!(link_marker("array[0] = 1", 5).is_none());
        assert!(link_marker("[](empty)", 0).is_none());
        let nested = link_marker("[wiki](https://example.com/a_(b)) after", 0).unwrap();
        assert_eq!(nested.next, 33);
    }

    #[test]
    fn bare_and_angle_links_stop_before_sentence_punctuation() {
        let bare = autolink_marker("see https://example.com/oxide. next", 4).unwrap();
        assert_eq!(bare.text_start, 4);
        assert_eq!(bare.text_end, 29);
        assert!(!bare.angle);

        let angle = autolink_marker("<https://example.com>", 0).unwrap();
        assert_eq!((angle.text_start, angle.text_end, angle.next), (1, 20, 21));
        assert!(angle.angle);
    }

    #[test]
    fn markdown_entities_render_as_the_character_they_name() {
        assert_eq!(entity_marker("a &amp; b", 2), Some(('&', 7)));
        assert_eq!(entity_marker("&#x2192;", 0), Some(('\u{2192}', 8)));
        assert_eq!(entity_marker("&unknown;", 0), None);
    }

    #[test]
    fn strikethrough_is_its_own_style() {
        assert_eq!(inline_marker("~~gone~~", 0), Some((2, 6, RunStyle::Strike)));
        assert_eq!(inline_marker("**bold**", 0), Some((2, 6, RunStyle::Bold)));
        assert_eq!(
            inline_marker("***both***", 0),
            Some((3, 7, RunStyle::BoldItalic))
        );
    }

    #[test]
    fn a_code_fence_keeps_its_language_and_closes_on_its_own_marker() {
        assert_eq!(fence_opens("```rust"), Some('`'));
        assert_eq!(fence_opens("  ~~~~"), Some('~'));
        assert!(fence_closes("```", '`'));
        assert!(!fence_closes("```rust", '`'));
        assert!(!fence_closes("~~~", '`'));
    }

    #[test]
    fn a_task_marker_points_at_the_checkbox_source_character() {
        assert_eq!(task_marker("- [ ] todo"), Some((3, false)));
        assert_eq!(task_marker("  * [X] done"), Some((5, true)));
        assert_eq!(task_marker("+ [x] done"), Some((3, true)));
        assert_eq!(task_marker("plain"), None);
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

    #[test]
    fn a_clipboard_file_is_not_inserted_as_ordinary_text_first() {
        assert!(clipboard_names_file("file:///tmp/shot.png"));
        assert!(!clipboard_names_file("ordinary clipboard text"));
        assert!(!clipboard_names_file("two\nlines"));
    }
}
