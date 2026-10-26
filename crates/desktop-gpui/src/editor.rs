//! The composer's text field.
//!
//! A single entity holds the text, the selection and the shaped layout, and
//! paints itself as one element: soft-wrapped lines, a caret, a selection and
//! the platform's text input handler, so typing, IME composition and the
//! keyboard are the window's own rather than a hand-rolled approximation of
//! them. Keys that are not text — Enter, the arrows, Escape — stay the view's,
//! since what they mean belongs to the conversation rather than to the field.

use gpui::{
    fill, point, px, relative, size, App, Bounds, ClipboardItem, Context, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, LayoutId, PaintQuad, Pixels, Point,
    SharedString, Style, TextAlign, TextRun, UTF16Selection, UnderlineStyle, Window, WrappedLine,
};
use std::ops::Range;
use unicode_segmentation::UnicodeSegmentation;

/// What the field tells the view without the view having to poll it.
#[derive(Debug, Clone)]
pub enum EditorEvent {
    /// The text changed, so anything derived from it — the `@` and `/` rows —
    /// is now out of date.
    Changed,
}

pub struct Editor {
    focus_handle: FocusHandle,
    pub content: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    placeholder: SharedString,
    is_selecting: bool,
    last_bounds: Option<Bounds<Pixels>>,
    last_lines: Vec<WrappedLine>,
    last_line_starts: Vec<usize>,
    last_line_height: Pixels,
}

impl EventEmitter<EditorEvent> for Editor {}

impl Editor {
    pub fn new(placeholder: impl Into<SharedString>, cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            content: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            placeholder: placeholder.into(),
            is_selecting: false,
            last_bounds: None,
            last_lines: Vec::new(),
            last_line_starts: Vec::new(),
            last_line_height: px(20.),
        }
    }

    pub fn focus_handle(&self) -> FocusHandle {
        self.focus_handle.clone()
    }

    pub fn content(&self) -> String {
        self.content.to_string()
    }

    pub fn cursor(&self) -> usize {
        self.cursor_offset()
    }

    pub fn is_empty(&self) -> bool {
        self.content.trim().is_empty()
    }

    pub fn set_content(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        self.selected_range = self.content.len()..self.content.len();
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
        cx.emit(EditorEvent::Changed);
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_content("", cx);
    }

    /// Inserts at the caret, replacing the selection.
    pub fn insert(&mut self, text: &str, cx: &mut Context<Self>) {
        let range = self.selected_range.clone();
        self.replace_range(range, text, cx);
    }

    pub fn replace_range(&mut self, range: Range<usize>, text: &str, cx: &mut Context<Self>) {
        let start = range.start.min(self.content.len());
        let end = range.end.min(self.content.len());
        let mut value = self.content.to_string();
        value.replace_range(start..end, text);
        self.content = value.into();
        let cursor = start + text.len();
        self.selected_range = cursor..cursor;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
        cx.emit(EditorEvent::Changed);
    }

    pub fn selected_text(&self) -> Option<String> {
        if self.selected_range.is_empty() {
            return None;
        }
        Some(self.content[self.selected_range.clone()].to_string())
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn set_cursor(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn delete_backward(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let start = self.previous_boundary(self.cursor_offset());
            self.selected_range = start..self.cursor_offset();
        }
        self.replace_range(self.selected_range.clone(), "", cx);
    }

    pub fn delete_forward(&mut self, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let end = self.next_boundary(self.cursor_offset());
            self.selected_range = self.cursor_offset()..end;
        }
        self.replace_range(self.selected_range.clone(), "", cx);
    }

    /// Left or right by one grapheme, or to the edge of the selection.
    pub fn move_horizontal(&mut self, right: bool, extend: bool, cx: &mut Context<Self>) {
        let target = if !extend && !self.selected_range.is_empty() {
            if right {
                self.selected_range.end
            } else {
                self.selected_range.start
            }
        } else if right {
            self.next_boundary(self.cursor_offset())
        } else {
            self.previous_boundary(self.cursor_offset())
        };
        if extend {
            self.select_to(target, cx);
        } else {
            self.set_cursor(target, cx);
        }
    }

    /// Up or down a line of the text, keeping the column where it can.
    pub fn move_vertical(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        let cursor = self.cursor_offset();
        let (line_start, line_end) = self.hard_line(cursor);
        let column = cursor - line_start;
        let target = if down {
            if line_end >= self.content.len() {
                self.content.len()
            } else {
                let next_start = line_end + 1;
                let (_, next_end) = self.hard_line(next_start);
                next_start + column.min(next_end - next_start)
            }
        } else if line_start == 0 {
            0
        } else {
            let previous_end = line_start - 1;
            let (previous_start, _) = self.hard_line(previous_end);
            previous_start + column.min(previous_end - previous_start)
        };
        if extend {
            self.select_to(target, cx);
        } else {
            self.set_cursor(target, cx);
        }
    }

    pub fn move_line_edge(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        let target = if end {
            self.hard_line(self.cursor_offset()).1
        } else {
            self.hard_line(self.cursor_offset()).0
        };
        if extend {
            self.select_to(target, cx);
        } else {
            self.set_cursor(target, cx);
        }
    }

    /// The hard line the offset sits on, as the range its text covers.
    fn hard_line(&self, offset: usize) -> (usize, usize) {
        let text = self.content.as_ref();
        let start = text[..offset.min(text.len())]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let end = text[start..]
            .find('\n')
            .map_or(text.len(), |index| start + index);
        (start, end)
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
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

    // ---------- the mouse ----------

    pub fn begin_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        self.is_selecting = true;
        let index = self.index_for_position(position);
        self.selected_range = index..index;
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        if !self.is_selecting {
            return;
        }
        self.select_to(self.index_for_position(position), cx);
    }

    pub fn end_selection(&mut self, cx: &mut Context<Self>) {
        self.is_selecting = false;
        cx.notify();
    }

    /// The byte the pointer is over, read out of the layout the last frame
    /// painted: the wrapped row finds the line, then the line finds the index.
    pub fn index_for_position(&self, position: Point<Pixels>) -> usize {
        let (Some(bounds), false) = (self.last_bounds, self.last_lines.is_empty()) else {
            return self.content.len();
        };
        if position.y < bounds.top() {
            return 0;
        }
        if position.y > bounds.bottom() {
            return self.content.len();
        }
        let mut top = bounds.top();
        for (index, line) in self.last_lines.iter().enumerate() {
            let height = line.size(self.last_line_height).height;
            if position.y <= top + height || index == self.last_lines.len() - 1 {
                let mut y = position.y - top;
                if y < Pixels::ZERO {
                    y = Pixels::ZERO;
                }
                let last = height - px(1.);
                if y > last {
                    y = last;
                }
                let x = (position.x - bounds.left()).max(Pixels::ZERO);
                let hit = line.closest_index_for_position(point(x, y), self.last_line_height);
                let local = match hit {
                    Ok(index) | Err(index) => index,
                };
                let start = self.last_line_starts.get(index).copied().unwrap_or(0);
                return start + local.min(line.text.len());
            }
            top += height;
        }
        self.content.len()
    }

    // ---------- the clipboard ----------

    pub fn copy(&self, cx: &mut App) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub fn cut(&mut self, cx: &mut Context<Self>) {
        if self.selected_text().is_some() {
            let text = self.selected_text().unwrap_or_default();
            cx.write_to_clipboard(ClipboardItem::new_string(text));
            self.replace_range(self.selected_range.clone(), "", cx);
        }
    }

    pub fn paste(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.insert(&text, cx);
        }
    }

    // ---------- the platform's own conversions ----------

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
}

impl Focusable for Editor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
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
        let range = range.start.min(self.content.len())..range.end.min(self.content.len());
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
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.replace_range(range, new_text, cx);
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
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());
        self.replace_range(range.clone(), new_text, cx);
        if !new_text.is_empty() {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }
        if let Some(selected) = new_selected_range_utf16 {
            let selected = self.range_from_utf16(&selected);
            let start = range.start + selected.start;
            let end = range.start + selected.end;
            self.selected_range = start.min(self.content.len())..end.min(self.content.len());
            cx.notify();
        }
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let range = self.range_from_utf16(&range_utf16);
        let (line_start, _, origin) = self.position_of(range.start)?;
        let start = position_of_in(
            &self.last_lines,
            &self.last_line_starts,
            line_start,
            range.start,
        );
        let end = position_of_in(
            &self.last_lines,
            &self.last_line_starts,
            line_start,
            range.end,
        );
        Some(Bounds::from_corners(
            point(bounds.left() + start.x, bounds.top() + origin + start.y),
            point(bounds.left() + end.x, bounds.top() + origin + end.y),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let index = self.index_for_position(point);
        Some(self.offset_to_utf16(index))
    }
}

impl Editor {
    /// The line the offset sits on, its index, and the y its text starts at.
    fn position_of(&self, offset: usize) -> Option<(usize, usize, Pixels)> {
        let line = self
            .last_line_starts
            .iter()
            .rposition(|start| *start <= offset)?;
        let mut top = Pixels::ZERO;
        for previous in &self.last_lines[..line] {
            top += previous.size(self.last_line_height).height;
        }
        Some((line, line, top))
    }
}

fn position_of_in(
    lines: &[WrappedLine],
    starts: &[usize],
    line: usize,
    offset: usize,
) -> Point<Pixels> {
    let Some(entry) = lines.get(line) else {
        return Point::default();
    };
    let local = offset.saturating_sub(starts.get(line).copied().unwrap_or(0));
    entry
        .position_for_index(local.min(entry.text.len()), px(0.))
        .unwrap_or_default()
}

/// The element that paints the field: the text, the selection, the caret, and
/// the platform's input handler over the box they are drawn in.
pub struct EditorElement {
    pub editor: Entity<Editor>,
    pub selection: Hsla,
    pub caret: Hsla,
    pub placeholder: Hsla,
}

pub struct PrepaintState {
    lines: Vec<WrappedLine>,
    line_height: Pixels,
    caret: Option<PaintQuad>,
    selection: Vec<PaintQuad>,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        _cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let editor = self.editor.clone();
        let placeholder = self.placeholder;
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let line_height = window.line_height();
        let layout_id =
            window.request_measured_layout(style, move |known, available, window, cx| {
                let width = match known.width {
                    Some(width) => width,
                    None => match available.width {
                        gpui::AvailableSpace::Definite(width) => width,
                        _ => px(480.),
                    },
                };
                let lines = shape(editor.read(cx), window, width, placeholder);
                let height: Pixels = lines
                    .iter()
                    .map(|line| line.size(line_height).height)
                    .fold(Pixels::ZERO, |total, line| total + line);
                size(width, height.max(line_height))
            });
        (layout_id, ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let line_height = window.line_height();
        let editor = self.editor.read(cx);
        let lines = shape(editor, window, bounds.size.width, self.placeholder);
        let mut line_starts = Vec::with_capacity(lines.len());
        let mut start = 0;
        for line in &lines {
            line_starts.push(start);
            start += line.text.len() + 1;
        }

        // The caret sits where the text it follows ends, on the wrapped row it
        // ends on.
        let mut caret = None;
        let cursor = editor.cursor();
        if let Some(line) = line_starts.iter().rposition(|start| *start <= cursor) {
            if let Some(entry) = lines.get(line) {
                let local = (cursor - line_starts[line]).min(entry.text.len());
                let origin = top_of(&lines, line, line_height);
                if let Some(position) = entry.position_for_index(local, line_height) {
                    caret = Some(fill(
                        Bounds::new(
                            point(
                                bounds.left() + position.x,
                                bounds.top() + origin + position.y,
                            ),
                            size(px(2.), line_height),
                        ),
                        self.caret,
                    ));
                }
            }
        }

        let mut selection = Vec::new();
        let selected = editor.selected_range.clone();
        if !selected.is_empty() {
            for (index, line) in lines.iter().enumerate() {
                let line_start = line_starts[index];
                let line_end = line_start + line.text.len();
                let start = selected.start.max(line_start);
                let end = selected.end.min(line_end);
                if start > end
                    || (start == end && selected.start != selected.end && line_end == start)
                {
                    continue;
                }
                let top = bounds.top() + top_of(&lines, index, line_height);
                let local_start = start - line_start;
                let local_end = end - line_start;
                let from = line
                    .position_for_index(local_start, line_height)
                    .unwrap_or_default();
                let to = line
                    .position_for_index(local_end, line_height)
                    .unwrap_or(from);
                if (to.y - from.y).abs() < px(0.5) {
                    selection.push(fill(
                        Bounds::new(
                            point(bounds.left() + from.x, top + from.y),
                            size((to.x - from.x).max(px(2.)), line_height),
                        ),
                        self.selection,
                    ));
                } else {
                    let height = line.size(line_height).height;
                    selection.push(fill(
                        Bounds::new(
                            point(bounds.left() + from.x, top + from.y),
                            size((line.width() - from.x).max(px(2.)), line_height),
                        ),
                        self.selection,
                    ));
                    if to.y > from.y + line_height {
                        selection.push(fill(
                            Bounds::new(
                                point(bounds.left(), top + from.y + line_height),
                                size(
                                    line.width(),
                                    height - from.y - line_height - (height - to.y),
                                ),
                            ),
                            self.selection,
                        ));
                    }
                    if to.x > Pixels::ZERO {
                        selection.push(fill(
                            Bounds::new(point(bounds.left(), top + to.y), size(to.x, line_height)),
                            self.selection,
                        ));
                    }
                }
            }
        }

        self.editor.update(cx, |editor, _cx| {
            editor.last_bounds = Some(bounds);
            editor.last_line_starts = line_starts.clone();
            editor.last_line_height = line_height;
            editor.last_lines = lines.clone();
        });

        PrepaintState {
            lines,
            line_height,
            caret,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.editor.read(cx).focus_handle();
        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.editor.clone()),
            cx,
        );

        for quad in prepaint.selection.drain(..) {
            window.paint_quad(quad);
        }

        let line_height = prepaint.line_height;
        let mut top = Pixels::ZERO;
        for line in &prepaint.lines {
            let _ = line.paint(
                point(bounds.left(), bounds.top() + top),
                line_height,
                TextAlign::Left,
                None,
                window,
                cx,
            );
            top += line.size(line_height).height;
        }

        if focus_handle.is_focused(window) {
            if let Some(caret) = prepaint.caret.take() {
                window.paint_quad(caret);
            }
        }
    }
}

fn shape(
    editor: &Editor,
    window: &mut Window,
    width: Pixels,
    placeholder: Hsla,
) -> Vec<WrappedLine> {
    let style = window.text_style();
    let font_size = style.font_size.to_pixels(window.rem_size());
    let (text, color) = if editor.content.is_empty() {
        (editor.placeholder.clone(), placeholder)
    } else {
        (editor.content.clone(), style.color)
    };
    let base = TextRun {
        len: text.len(),
        font: style.font(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let runs = match editor.marked_range.as_ref() {
        Some(marked) if !editor.content.is_empty() => vec![
            TextRun {
                len: marked.start,
                ..base.clone()
            },
            TextRun {
                len: marked.end - marked.start,
                underline: Some(UnderlineStyle {
                    color: Some(color),
                    thickness: px(1.),
                    wavy: false,
                }),
                ..base.clone()
            },
            TextRun {
                len: text.len() - marked.end,
                ..base
            },
        ]
        .into_iter()
        .filter(|run| run.len > 0)
        .collect(),
        _ => vec![base],
    };
    window
        .text_system()
        .shape_text(text, font_size, &runs, Some(width), None)
        .map(|lines| lines.into_iter().collect())
        .unwrap_or_default()
}

fn top_of(lines: &[WrappedLine], index: usize, line_height: Pixels) -> Pixels {
    lines[..index]
        .iter()
        .map(|line| line.size(line_height).height)
        .fold(Pixels::ZERO, |total, line| total + line)
}

/// The caret's own color for a theme, so an element built outside the view can
/// still be told what it paints.
pub fn caret_color(accent: impl Into<Hsla>) -> Hsla {
    accent.into()
}

/// A selection color: the accent at a quarter, which reads on both built-in
/// palettes.
pub fn selection_color(accent: impl Into<Hsla>) -> Hsla {
    let accent = accent.into();
    Hsla { a: 0.25, ..accent }
}

/// A placeholder color: what the theme dims its supporting text with.
pub fn placeholder_color(dim: impl Into<Hsla>) -> Hsla {
    let dim = dim.into();
    Hsla { a: 0.7, ..dim }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The editor's own state is exercised through the window's view; what can be
    // checked without one is the arithmetic its caret and selection read.
    #[test]
    fn a_selection_color_is_the_accent_at_a_quarter() {
        let color = selection_color(gpui::rgba(0x112233ff));
        assert!((color.a - 0.25).abs() < 1e-6);
    }

    #[test]
    fn a_placeholder_is_dimmer_than_the_text_it_stands_in_for() {
        let color = placeholder_color(gpui::rgba(0xffffff80));
        assert!(color.a < 1.0);
    }
}
