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
    actions, canvas, div, fill, point, px, size, App, Bounds, ClipboardItem, Context,
    ElementInputHandler, EntityInputHandler, EventEmitter, FocusHandle, Focusable, Font, FontStyle,
    FontWeight, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, SharedString, TextAlign, TextRun, UTF16Selection, Window,
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
        Home,
        End,
        SelectAll,
        Paste,
        Cut,
        Copy,
        InsertNewline,
        DeleteToLineStart,
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
        KeyBinding::new("alt-left", Left, Some("Editor")),
        KeyBinding::new("alt-right", Right, Some("Editor")),
        KeyBinding::new("home", Home, Some("Editor")),
        KeyBinding::new("end", End, Some("Editor")),
        KeyBinding::new("shift-home", SelectAll, Some("Editor")),
        KeyBinding::new("cmd-a", SelectAll, Some("Editor")),
        KeyBinding::new("ctrl-a", SelectAll, Some("Editor")),
        KeyBinding::new("cmd-v", Paste, Some("Editor")),
        KeyBinding::new("ctrl-v", Paste, Some("Editor")),
        KeyBinding::new("cmd-c", Copy, Some("Editor")),
        KeyBinding::new("ctrl-c", Copy, Some("Editor")),
        KeyBinding::new("cmd-x", Cut, Some("Editor")),
        KeyBinding::new("ctrl-x", Cut, Some("Editor")),
        KeyBinding::new("shift-enter", InsertNewline, Some("Editor")),
        KeyBinding::new("cmd-backspace", DeleteToLineStart, Some("Editor")),
    ]
}

#[derive(Clone, Debug)]
pub enum EditorEvent {
    Changed,
}

impl EventEmitter<EditorEvent> for Editor {}

pub const FONT_SIZE: f32 = 13.5;
pub const LINE_HEIGHT: f32 = 20.0;

struct ShapedLine {
    start: usize,
    line: gpui::WrappedLine,
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
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn height(&self) -> Pixels {
        self.measured_height
    }

    pub fn set_text(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected_range = end..end;
        self.marked_range = None;
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

    fn move_vertical(&mut self, down: bool, select: bool, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let (line, column) = self.line_of(cursor);
        let bounds = self.line_bounds();
        let target_line = if down {
            (line + 1).min(bounds.len().saturating_sub(1))
        } else {
            line.saturating_sub(1)
        };
        let range = &bounds[target_line];
        let target = (range.start + column).min(range.end);
        if select {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn home(&mut self, select: bool, cx: &mut Context<Self>) {
        let (line, _) = self.line_of(self.cursor_offset());
        let bounds = self.line_bounds();
        let target = bounds[line].start;
        if select {
            self.select_to(target, cx);
        } else {
            self.move_to(target, cx);
        }
    }

    fn end(&mut self, select: bool, cx: &mut Context<Self>) {
        let (line, _) = self.line_of(self.cursor_offset());
        let bounds = self.line_bounds();
        let target = bounds[line].end;
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
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
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
        self.content = format!("{}{}", &self.content[..start], &self.content[cursor..]).into();
        self.selected_range = start..start;
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    /// The point a byte offset is painted at, in the editor's own coordinates.
    fn point_for_offset(&self, offset: usize) -> Point<Pixels> {
        let mut y = px(0.0);
        for shaped in &self.lines {
            let start = shaped.start;
            let end = start + shaped.line.len();
            if offset <= end || shaped.start.saturating_add(shaped.line.len()) == self.content.len()
            {
                if offset < start && shaped.start != 0 {
                    continue;
                }
                if let Some(position) = shaped
                    .line
                    .position_for_index(offset.saturating_sub(start), px(LINE_HEIGHT))
                {
                    return point(position.x, y + position.y);
                }
            }
            y += shaped.line.size(px(LINE_HEIGHT)).height;
        }
        point(px(0.0), y)
    }

    fn offset_for_point(&self, position: Point<Pixels>) -> usize {
        let mut y = px(0.0);
        for shaped in &self.lines {
            let height = shaped.line.size(px(LINE_HEIGHT)).height;
            if position.y <= y + height {
                let within = point(position.x, position.y - y);
                return match shaped.line.index_for_position(within, px(LINE_HEIGHT)) {
                    Ok(index) | Err(index) => shaped.start + index,
                };
            }
            y += height;
        }
        self.content.len()
    }

    fn shape(&mut self, bounds: Bounds<Pixels>, window: &mut Window) {
        self.last_bounds = Some(bounds);
        let width = bounds.size.width.max(px(16.0));
        let font = Font {
            family: ".SystemUIFont".into(),
            features: Default::default(),
            fallbacks: None,
            weight: FontWeight::NORMAL,
            style: FontStyle::Normal,
        };
        let mut lines = Vec::new();
        let mut start = 0;
        for text in self.content.split('\n') {
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: self.color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let shaped = window
                .text_system()
                .shape_text(
                    SharedString::from(text.to_string()),
                    px(FONT_SIZE),
                    std::slice::from_ref(&run),
                    Some(width),
                    None,
                )
                .ok()
                .and_then(|mut lines| lines.pop());
            if let Some(line) = shaped {
                lines.push(ShapedLine { start, line });
            }
            start += text.len() + 1;
        }
        let total: Pixels = lines
            .iter()
            .map(|shaped| shaped.line.size(px(LINE_HEIGHT)).height)
            .fold(px(0.0), |sum, height| sum + height);
        self.lines = lines;
        self.measured_height = total.max(px(LINE_HEIGHT));
    }

    fn paint_lines(&self, bounds: Bounds<Pixels>, window: &mut Window, cx: &mut App) {
        let focused = self.focus_handle.is_focused(window);
        let mut y = bounds.origin.y;
        for shaped in &self.lines {
            let height = shaped.line.size(px(LINE_HEIGHT)).height;
            let origin = point(bounds.origin.x, y);
            let line_bounds = Bounds::new(origin, size(bounds.size.width, height));

            if focused && !self.selected_range.is_empty() {
                let start = shaped.start;
                let end = start + shaped.line.len();
                let selected =
                    self.selected_range.start.max(start)..self.selected_range.end.min(end);
                if selected.start < selected.end {
                    if let (Some(from), Some(to)) = (
                        shaped
                            .line
                            .position_for_index(selected.start - start, px(LINE_HEIGHT)),
                        shaped
                            .line
                            .position_for_index(selected.end - start, px(LINE_HEIGHT)),
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
                    point(bounds.origin.x + caret.x, bounds.origin.y + caret.y),
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
            .key_context("Editor")
            .track_focus(&focus)
            .cursor(gpui::CursorStyle::IBeam)
            .text_color(color)
            .child(
                canvas(
                    move |bounds, window, cx| {
                        entity.update(cx, |editor, _| editor.shape(bounds, window));
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
                cx.listener(|this, _: &SelectUp, _window, cx| this.move_vertical(false, true, cx)),
            )
            .on_action(
                cx.listener(|this, _: &SelectDown, _window, cx| this.move_vertical(true, true, cx)),
            )
            .on_action(cx.listener(|this, _: &Home, _window, cx| this.home(false, cx)))
            .on_action(cx.listener(|this, _: &End, _window, cx| this.end(false, cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _window, cx| this.select_all(cx)))
            .on_action(cx.listener(|this, _: &Copy, _window, cx| this.copy(cx)))
            .on_action(cx.listener(|this, _: &Cut, _window, cx| this.cut(cx)))
            .on_action(cx.listener(|this, _: &Paste, _window, cx| this.paste(cx)))
            .on_action(cx.listener(|this, _: &InsertNewline, _window, cx| this.insert("\n", cx)))
            .on_action(
                cx.listener(|this, _: &DeleteToLineStart, _window, cx| {
                    this.delete_to_line_start(cx)
                }),
            )
    }
}
