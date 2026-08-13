use egui::{
    Color32, CursorIcon, Event, EventFilter, Id, Key, Modifiers, OutputCommand, Pos2, Rect,
    Response, Sense, Stroke, TextFormat, Ui, Vec2,
    epaint::text::{Galley, LayoutJob},
    text::{ByteIndex, CCursor, CCursorRange, LayoutSection},
};
use std::{ops::Range, sync::Arc, time::Duration};

use crate::{keybindings::Command, renderer::mark_retained, theme};

const TEXT_LEFT_PADDING: f32 = 8.0;
const TEXT_TOP_PADDING: f32 = 6.0;
const CARET_BLINK_INTERVAL: f64 = 0.7;
/// One tab stop, in characters. Indent guides land on these.
const INDENT_WIDTH: usize = 4;
/// A wrapped continuation is pushed past its own indent by this much, so the
/// eye can tell a soft wrap from a new statement.
const WRAP_INDENT_CHARS: f32 = 2.0;

/// The editor's line box, which the Appearance setting drives.
fn line_height() -> f32 {
    theme::typography::code_line()
}

pub(crate) fn editor_background() -> Color32 {
    theme::surface().editor
}

struct RetainedLine {
    job: LayoutJob,
    char_start: usize,
    character_len: usize,
    /// Leading whitespace, in characters, which sets both the indent guides and
    /// where a wrapped continuation resumes.
    indent: usize,
    height: f32,
    galley: Option<Arc<Galley>>,
    revision: u64,
}

#[derive(Clone)]
struct Edit {
    start: usize,
    deleted: String,
    inserted: String,
    before: (usize, usize),
    after: (usize, usize),
}

#[derive(Default)]
pub struct EditorSurface {
    anchor: usize,
    cursor: usize,
    h_pos: Option<f32>,
    scroll_x: f32,
    scroll_y: f32,
    horizontal_scrollbar: crate::scrollbar::State,
    scrollbar: crate::scrollbar::State,
    undo: Vec<Vec<Edit>>,
    redo: Vec<Vec<Edit>>,
    transaction: Option<Vec<Edit>>,
    lines: Vec<RetainedLine>,
    line_numbers: Vec<Option<Arc<Galley>>>,
    offsets: Vec<f32>,
    visual_revision: Option<u64>,
    wrap_width: u32,
    appearance: u64,
    caret_blink_started: f64,
    caret_was_focused: bool,
}

pub struct EditorOutput {
    pub response: Response,
    pub cursor: usize,
    pub changed: bool,
    pub caret_rect: Option<Rect>,
    pub hovered_character: Option<usize>,
    pub last_inserted: Option<char>,
    pub inserted_text: String,
    pub scrolled: bool,
}

pub(crate) struct EditorShowOptions<'a> {
    pub request_focus: bool,
    pub scroll_to_character: Option<usize>,
    pub id: Id,
    pub line_markers: &'a [(usize, Color32)],
    pub text_input: TextInputMode,
    pub native_keybindings: bool,
    pub block_caret: bool,
    pub wrap: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextInputMode {
    Standard,
    Disabled,
    Insert,
    Replace,
}

#[derive(Clone, Copy)]
pub(crate) struct DocumentMetrics {
    pub revision: u64,
    pub line_count: usize,
    pub character_len: usize,
}

impl EditorSurface {
    pub fn set_selection(&mut self, anchor: usize, cursor: usize) {
        self.anchor = anchor;
        self.cursor = cursor;
    }

    pub fn selection(&self) -> Range<usize> {
        self.anchor.min(self.cursor)..self.anchor.max(self.cursor)
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn replace_selection(&mut self, text: &mut String, replacement: &str) -> bool {
        let range = self.selection();
        let character_len = text.chars().count();
        let range = range.start.min(character_len)..range.end.min(character_len);
        if range.is_empty() && replacement.is_empty() {
            return false;
        }
        let deleted = char_slice(text, range.clone()).to_owned();
        let before = (self.anchor, self.cursor);
        replace_chars(text, range.clone(), replacement);
        let cursor = range.start + replacement.chars().count();
        self.anchor = cursor;
        self.cursor = cursor;
        self.record_edit(Edit {
            start: range.start,
            deleted,
            inserted: replacement.to_owned(),
            before,
            after: (cursor, cursor),
        });
        true
    }

    fn record_edit(&mut self, edit: Edit) {
        if let Some(transaction) = &mut self.transaction {
            transaction.push(edit);
        } else {
            self.undo.push(vec![edit]);
            self.redo.clear();
        }
    }

    pub fn begin_transaction(&mut self) {
        if self.transaction.is_none() {
            self.transaction = Some(Vec::new());
        }
    }

    pub fn end_transaction(&mut self) {
        let Some(transaction) = self.transaction.take() else {
            return;
        };
        if !transaction.is_empty() {
            self.undo.push(transaction);
            self.redo.clear();
        }
    }

    pub fn undo(&mut self, text: &mut String) -> bool {
        self.end_transaction();
        let Some(edits) = self.undo.pop() else {
            return false;
        };
        for edit in edits.iter().rev() {
            let end = edit.start + edit.inserted.chars().count();
            replace_chars(text, edit.start..end, &edit.deleted);
        }
        (self.anchor, self.cursor) = edits
            .first()
            .map_or((self.anchor, self.cursor), |edit| edit.before);
        self.redo.push(edits);
        true
    }

    pub fn redo(&mut self, text: &mut String) -> bool {
        self.end_transaction();
        let Some(edits) = self.redo.pop() else {
            return false;
        };
        for edit in &edits {
            let end = edit.start + edit.deleted.chars().count();
            replace_chars(text, edit.start..end, &edit.inserted);
        }
        (self.anchor, self.cursor) = edits
            .last()
            .map_or((self.anchor, self.cursor), |edit| edit.after);
        self.undo.push(edits);
        true
    }

    pub fn execute_command(
        &mut self,
        ctx: &egui::Context,
        text: &mut String,
        command: Command,
        paste: Option<&str>,
    ) -> bool {
        let movement = |command| {
            matches!(
                command,
                Command::EditorSelectLeft
                    | Command::EditorSelectRight
                    | Command::EditorSelectUp
                    | Command::EditorSelectDown
                    | Command::EditorSelectWordLeft
                    | Command::EditorSelectWordRight
                    | Command::EditorSelectLineStart
                    | Command::EditorSelectLineEnd
                    | Command::EditorSelectPageUp
                    | Command::EditorSelectPageDown
                    | Command::EditorSelectDocumentStart
                    | Command::EditorSelectDocumentEnd
            )
        };
        match command {
            Command::EditorCopy => {
                self.copy(ctx, text);
                false
            }
            Command::EditorCut => {
                self.copy(ctx, text);
                self.replace_selection(text, "")
            }
            Command::EditorPaste => paste.is_some_and(|value| self.replace_selection(text, value)),
            Command::EditorUndo => self.undo(text),
            Command::EditorRedo => self.redo(text),
            Command::EditorSelectAll => {
                self.anchor = 0;
                self.cursor = text.chars().count();
                false
            }
            Command::EditorDeleteLeft => {
                if self.selection().is_empty() && self.cursor > 0 {
                    self.anchor = self.cursor - 1;
                }
                self.replace_selection(text, "")
            }
            Command::EditorDeleteRight => {
                if self.selection().is_empty() && self.cursor < text.chars().count() {
                    self.cursor += 1;
                }
                self.replace_selection(text, "")
            }
            Command::EditorInsertLineBreak => self.replace_selection(text, "\n"),
            Command::EditorIndent => self.replace_selection(text, "    "),
            Command::EditorOutdent => self.decrease_indent(text),
            Command::EditorCursorLeft | Command::EditorSelectLeft => {
                let target = if command == Command::EditorCursorLeft && !self.selection().is_empty()
                {
                    self.selection().start
                } else {
                    self.cursor.saturating_sub(1)
                };
                self.move_cursor(target, movement(command));
                self.h_pos = None;
                false
            }
            Command::EditorCursorRight | Command::EditorSelectRight => {
                let target =
                    if command == Command::EditorCursorRight && !self.selection().is_empty() {
                        self.selection().end
                    } else {
                        (self.cursor + 1).min(text.chars().count())
                    };
                self.move_cursor(target, movement(command));
                self.h_pos = None;
                false
            }
            Command::EditorCursorWordLeft | Command::EditorSelectWordLeft => {
                self.move_cursor(previous_word(text, self.cursor), movement(command));
                self.h_pos = None;
                false
            }
            Command::EditorCursorWordRight | Command::EditorSelectWordRight => {
                self.move_cursor(next_word(text, self.cursor), movement(command));
                self.h_pos = None;
                false
            }
            Command::EditorCursorUp | Command::EditorSelectUp => {
                self.move_vertical(-1, movement(command));
                false
            }
            Command::EditorCursorDown | Command::EditorSelectDown => {
                self.move_vertical(1, movement(command));
                false
            }
            Command::EditorCursorLineStart | Command::EditorSelectLineStart => {
                self.move_cursor(text_line_start(text, self.cursor), movement(command));
                self.h_pos = None;
                false
            }
            Command::EditorCursorLineEnd | Command::EditorSelectLineEnd => {
                self.move_cursor(text_line_end(text, self.cursor), movement(command));
                self.h_pos = None;
                false
            }
            Command::EditorCursorPageUp | Command::EditorSelectPageUp => {
                for _ in 0..20 {
                    self.move_vertical(-1, movement(command));
                }
                false
            }
            Command::EditorCursorPageDown | Command::EditorSelectPageDown => {
                for _ in 0..20 {
                    self.move_vertical(1, movement(command));
                }
                false
            }
            Command::EditorCursorDocumentStart | Command::EditorSelectDocumentStart => {
                self.move_cursor(0, movement(command));
                self.h_pos = None;
                false
            }
            Command::EditorCursorDocumentEnd | Command::EditorSelectDocumentEnd => {
                self.move_cursor(text.chars().count(), movement(command));
                self.h_pos = None;
                false
            }
            _ => false,
        }
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        text: &mut String,
        highlighted: &LayoutJob,
        visual_revision: u64,
        request_focus: bool,
        scroll_to_character: Option<usize>,
    ) -> EditorOutput {
        let document = DocumentMetrics {
            revision: visual_revision,
            line_count: line_count(text),
            character_len: text.chars().count(),
        };
        self.show_document(
            ui,
            text,
            highlighted,
            document,
            request_focus,
            scroll_to_character,
        )
    }

    pub(crate) fn show_document(
        &mut self,
        ui: &mut Ui,
        text: &mut String,
        highlighted: &LayoutJob,
        document: DocumentMetrics,
        request_focus: bool,
        scroll_to_character: Option<usize>,
    ) -> EditorOutput {
        self.show_document_with_options(
            ui,
            text,
            highlighted,
            document,
            EditorShowOptions {
                request_focus,
                scroll_to_character,
                id: Id::new("editor"),
                line_markers: &[],
                text_input: TextInputMode::Standard,
                native_keybindings: true,
                block_caret: false,
                wrap: false,
            },
        )
    }

    pub(crate) fn show_document_with_options(
        &mut self,
        ui: &mut Ui,
        text: &mut String,
        highlighted: &LayoutJob,
        document: DocumentMetrics,
        options: EditorShowOptions,
    ) -> EditorOutput {
        let EditorShowOptions {
            request_focus,
            scroll_to_character,
            id: editor_id,
            line_markers,
            text_input,
            native_keybindings,
            block_caret,
            wrap,
        } = options;
        let desired = ui.available_size();
        let (_, rect) = ui.allocate_space(desired);
        let editor_rect = rect;
        let mut response = ui.interact(editor_rect, editor_id, Sense::click_and_drag());
        if ui.input(|input| {
            input
                .pointer
                .hover_pos()
                .is_some_and(|pointer| editor_rect.contains(pointer))
        }) {
            ui.output_mut(|output| output.cursor_icon = CursorIcon::Text);
        }
        let advance = digit_advance(ui);
        let gutter_width = gutter_width(document.line_count, advance);
        let content = Rect::from_min_max(
            egui::pos2(
                rect.left() + gutter_width,
                (rect.top() + TEXT_TOP_PADDING).min(rect.bottom()),
            ),
            editor_rect.right_bottom(),
        );
        let wrap_width = if wrap {
            (content.width() - TEXT_LEFT_PADDING).max(1.0)
        } else {
            f32::INFINITY
        };
        self.sync_lines(highlighted, document.revision, wrap_width, advance);
        self.clamp_selection(document.character_len);
        let cursor_before_input = self.cursor;

        if request_focus {
            response.request_focus();
        }
        let pointer_over_editor = ui.input(|input| {
            input
                .pointer
                .hover_pos()
                .is_some_and(|pointer| rect.contains(pointer))
        });
        let (scroll_delta, shift) =
            ui.input(|input| (input.smooth_scroll_delta, input.modifiers.shift));
        let horizontal_delta = if scroll_delta.x != 0.0 {
            scroll_delta.x
        } else if !wrap && shift {
            scroll_delta.y
        } else {
            0.0
        };
        let horizontal_scrolling = pointer_over_editor && !wrap && horizontal_delta != 0.0;
        let scrolling = pointer_over_editor
            && scroll_delta.y != 0.0
            && !(horizontal_scrolling && scroll_delta.x == 0.0);
        if scrolling {
            self.scroll_y -= scroll_delta.y;
        }
        if horizontal_scrolling {
            self.scroll_x -= horizontal_delta;
        }
        if wrap {
            self.scroll_x = 0.0;
        }
        self.clamp_scroll(content.height());

        self.layout_visible_lines(ui, content);
        let mut document_width = self.document_width(advance).max(content.width());
        self.clamp_horizontal_scroll(content.width(), document_width);
        let mut ensure_cursor_visible = request_focus;
        if let Some(character) = scroll_to_character {
            self.scroll_character_into_view(character, content.height());
            self.layout_visible_lines(ui, content);
            document_width = self.document_width(advance).max(content.width());
            if !wrap {
                self.scroll_character_horizontally_into_view(character, content.width());
            }
            self.clamp_horizontal_scroll(content.width(), document_width);
        }

        if (response.double_clicked() || response.triple_clicked())
            && let Some(pointer) = response.interact_pointer_pos()
        {
            response.request_focus();
            let character = self.character_at(pointer, content);
            let range = if response.triple_clicked() {
                text_line_range(text, character)
            } else {
                text_word_range(text, character)
            };
            self.set_selection(range.start, range.end);
            ensure_cursor_visible = true;
        } else if response.clicked() || response.drag_started() {
            response.request_focus();
            if let Some(pointer) = response.interact_pointer_pos() {
                let character = self.character_at(pointer, content);
                let extend = ui.input(|input| input.modifiers.shift);
                self.move_cursor(character, extend);
                ensure_cursor_visible = true;
            }
        } else if response.dragged()
            && let Some(pointer) = response.interact_pointer_pos()
        {
            self.cursor = self.character_at(pointer, content);
            let delta = selection_drag_scroll_delta(
                pointer.y,
                content.top(),
                content.bottom(),
                ui.input(|input| input.stable_dt),
            );
            if delta != 0.0 {
                self.scroll_y += delta;
                self.clamp_scroll(content.height());
                ui.ctx().request_repaint();
            }
            if !wrap {
                let delta = selection_drag_scroll_delta(
                    pointer.x,
                    content.left(),
                    content.right(),
                    ui.input(|input| input.stable_dt),
                );
                if delta != 0.0 {
                    self.scroll_x += delta;
                    self.clamp_horizontal_scroll(content.width(), document_width);
                    ui.ctx().request_repaint();
                }
            }
        }

        let mut changed = false;
        let mut last_inserted = None;
        let mut inserted_text = String::new();
        let cursor_before_events = self.cursor;
        if response.has_focus() {
            ui.memory_mut(|memory| {
                memory.set_focus_lock_filter(
                    response.id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                );
            });
            (changed, last_inserted, inserted_text) =
                self.handle_events(ui, text, editor_id, text_input, native_keybindings);
            if changed {
                response.mark_changed();
                ui.ctx().request_repaint();
            }
        }
        ensure_cursor_visible |= self.cursor != cursor_before_events && !response.dragged();

        self.clamp_selection(document.character_len);
        if ensure_cursor_visible {
            self.scroll_character_into_view(self.cursor, content.height());
            if !wrap {
                self.scroll_character_horizontally_into_view(self.cursor, content.width());
                self.clamp_horizontal_scroll(content.width(), document_width);
            }
        }
        let focused = response.has_focus();
        let time = ui.input(|input| input.time);
        if focused
            && (!self.caret_was_focused
                || changed
                || self.cursor != cursor_before_input
                || response.clicked()
                || response.dragged())
        {
            self.caret_blink_started = time;
        }
        self.caret_was_focused = focused;
        if focused {
            let elapsed = (time - self.caret_blink_started).max(0.0);
            let until_next = CARET_BLINK_INTERVAL - elapsed.rem_euclid(CARET_BLINK_INTERVAL);
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(until_next));
        }
        self.paint(ui, rect, content, focused, block_caret, line_markers);
        if focused {
            self.update_ime(ui, rect, content);
        }
        if crate::scrollbar::show(
            ui,
            editor_id.with("scrollbar"),
            rect,
            self.offsets.last().copied().unwrap_or(0.0),
            &mut self.scroll_y,
            &mut self.scrollbar,
            scrolling,
        ) {
            self.clamp_scroll(content.height());
            ui.ctx().request_repaint();
        }
        if !wrap
            && crate::scrollbar::show_horizontal(
                ui,
                editor_id.with("horizontal_scrollbar"),
                content,
                document_width,
                &mut self.scroll_x,
                &mut self.horizontal_scrollbar,
                horizontal_scrolling,
            )
        {
            self.clamp_horizontal_scroll(content.width(), document_width);
            ui.ctx().request_repaint();
        }

        EditorOutput {
            response,
            cursor: self.cursor,
            changed,
            caret_rect: self.cursor_rect(content),
            hovered_character: ui
                .input(|input| input.pointer.hover_pos())
                .filter(|pointer| editor_rect.contains(*pointer))
                .map(|pointer| self.character_at(pointer, content)),
            last_inserted,
            inserted_text,
            scrolled: scrolling || horizontal_scrolling,
        }
    }

    fn sync_lines(
        &mut self,
        highlighted: &LayoutJob,
        revision: u64,
        wrap_width: f32,
        advance: f32,
    ) {
        let width = wrap_width.round().to_bits();
        let appearance = theme::appearance();
        if self.appearance != appearance {
            // Font size, line height, and palette are all baked into a galley.
            self.appearance = appearance;
            self.visual_revision = None;
            self.line_numbers.clear();
            for line in &mut self.lines {
                line.galley = None;
                line.height = estimated_height(line.character_len, wrap_width, advance);
            }
            self.rebuild_offsets();
        }
        if self.visual_revision == Some(revision) {
            if self.wrap_width == width {
                return;
            }
            for line in &mut self.lines {
                line.job.wrap.max_width = wrap_line_width(wrap_width, line.indent, advance);
                line.height = estimated_height(line.character_len, wrap_width, advance);
                line.galley = None;
            }
            self.wrap_width = width;
            self.rebuild_offsets();
            return;
        }
        let specs = split_layout_job(highlighted, wrap_width);
        let old = std::mem::take(&mut self.lines);
        let old_len = old.len();
        let new_len = specs.len();
        let mut old: Vec<_> = old.into_iter().map(Some).collect();
        let prefix = old
            .iter()
            .zip(&specs)
            .take_while(|(old, spec)| old.as_ref().is_some_and(|line| line.job == spec.job))
            .count();
        let mut suffix = 0;
        while suffix < old_len.saturating_sub(prefix)
            && suffix < new_len.saturating_sub(prefix)
            && old[old_len - suffix - 1]
                .as_ref()
                .is_some_and(|line| line.job == specs[new_len - suffix - 1].job)
        {
            suffix += 1;
        }
        self.lines = specs
            .into_iter()
            .enumerate()
            .map(|(index, spec)| {
                let old_index = if index < prefix {
                    Some(index)
                } else if index >= new_len - suffix {
                    Some(old_len - (new_len - index))
                } else {
                    None
                };
                if let Some(mut line) = old_index.and_then(|index| old[index].take()) {
                    line.char_start = spec.char_start;
                    line
                } else {
                    let mut job = spec.job;
                    job.wrap.max_width = wrap_line_width(wrap_width, spec.indent, advance);
                    RetainedLine {
                        height: estimated_height(spec.character_len, wrap_width, advance),
                        job,
                        char_start: spec.char_start,
                        character_len: spec.character_len,
                        indent: spec.indent,
                        galley: None,
                        revision,
                    }
                }
            })
            .collect();
        self.visual_revision = Some(revision);
        self.wrap_width = width;
        self.rebuild_offsets();
    }

    fn rebuild_offsets(&mut self) {
        self.offsets.clear();
        self.offsets.reserve(self.lines.len() + 1);
        let mut y = 0.0;
        self.offsets.push(y);
        for line in &self.lines {
            y += line.height.max(line_height());
            self.offsets.push(y);
        }
    }

    fn visible_lines(&self, viewport_height: f32) -> Range<usize> {
        let start = self
            .offsets
            .partition_point(|offset| *offset <= self.scroll_y)
            .saturating_sub(1)
            .min(self.lines.len());
        let end = self
            .offsets
            .partition_point(|offset| *offset < self.scroll_y + viewport_height)
            .saturating_add(1)
            .min(self.lines.len());
        start.saturating_sub(1)..end
    }

    fn layout_visible_lines(&mut self, ui: &Ui, content: Rect) {
        let range = self.visible_lines(content.height());
        let cursor_line = self.line_for_character(self.cursor);
        let mut indexes: Vec<_> = range.collect();
        if !indexes.contains(&cursor_line) && cursor_line < self.lines.len() {
            indexes.push(cursor_line);
        }
        let advance = digit_advance(ui);
        let mut changed_height = false;
        for index in indexes {
            let line = &mut self.lines[index];
            if line.galley.is_none() {
                let mut galley = ui.fonts_mut(|fonts| fonts.layout_job(line.job.clone()));
                indent_wrapped_rows(&mut galley, line.indent, advance);
                let height = galley.size().y.max(line_height());
                changed_height |= (height - line.height).abs() > f32::EPSILON;
                line.height = height;
                line.galley = Some(galley);
            }
            if self.line_numbers.len() <= index {
                self.line_numbers.resize(index + 1, None);
            }
            if self.line_numbers[index].is_none() {
                // Laid out uncolored so one cached galley can serve both the
                // active line and every other one.
                self.line_numbers[index] = Some(ui.fonts_mut(|fonts| {
                    fonts.layout_no_wrap(
                        (index + 1).to_string(),
                        theme::typography::code_small(),
                        Color32::PLACEHOLDER,
                    )
                }));
            }
        }
        if changed_height {
            self.rebuild_offsets();
            self.clamp_scroll(content.height());
        }
    }

    fn paint(
        &self,
        ui: &Ui,
        rect: Rect,
        content: Rect,
        focused: bool,
        block_caret: bool,
        line_markers: &[(usize, Color32)],
    ) {
        let painter = ui.painter_at(rect);
        let text_painter = painter.with_clip_rect(content);
        let horizontal_geometry = u64::from(content.left().to_bits())
            ^ u64::from(content.width().to_bits()).rotate_left(32)
            ^ u64::from(self.scroll_x.to_bits()).rotate_left(16);
        mark_retained(
            &painter,
            rect,
            0x1000_0000_0000_0000,
            u64::from(rect.width().to_bits()) << 32 | u64::from(rect.height().to_bits()),
        );
        painter.rect_filled(rect, 0.0, editor_background());
        let selection = self.selection();
        let has_selection = !selection.is_empty();
        let cursor_line = self.line_for_character(self.cursor);
        let advance =
            ui.fonts_mut(|fonts| fonts.glyph_width(&theme::typography::code_editor(), '0'));
        let text_left = content.left() + TEXT_LEFT_PADDING - self.scroll_x;
        let line_height = line_height();
        // The block that encloses the caret owns the one guide that is allowed
        // to be an accent.
        let active_guide = self
            .lines
            .get(cursor_line)
            .map(|line| line.indent.saturating_sub(INDENT_WIDTH) / INDENT_WIDTH * INDENT_WIDTH)
            .filter(|_| {
                self.lines
                    .get(cursor_line)
                    .is_some_and(|line| line.indent > 0)
            });
        let mut inherited_indent = 0;
        for index in self.visible_lines(content.height()) {
            let line = &self.lines[index];
            let Some(base_galley) = &line.galley else {
                continue;
            };
            let y = content.top() + self.offsets[index] - self.scroll_y;
            let line_end = line.char_start + line.character_len;
            let selected = selection.start.max(line.char_start)..selection.end.min(line_end);
            let selection_state = (selected.start < selected.end).then_some(
                (selected.start as u64).rotate_left(17) ^ (selected.end as u64).rotate_left(31),
            );
            let state = horizontal_geometry
                ^ u64::from(y.to_bits())
                ^ selection_state.unwrap_or(0)
                ^ u64::from(index == cursor_line && focused)
                ^ (has_selection as u64) << 1
                ^ (line.indent as u64).rotate_left(9);
            mark_retained(
                &painter,
                rect,
                0x2000_0000_0000_0000 | index as u64,
                line.revision ^ state,
            );
            let is_cursor_line = index == cursor_line && focused;
            if is_cursor_line && !has_selection {
                painter.rect_filled(
                    Rect::from_min_size(
                        egui::pos2(content.left(), y),
                        egui::vec2(content.width(), line.height),
                    ),
                    0.0,
                    theme::editor::line_active(),
                );
            }
            // A blank line belongs to the block around it, so it borrows that
            // indent rather than dropping every guide for one row.
            let indent = if line.character_len == 0 {
                inherited_indent
            } else {
                inherited_indent = line.indent;
                line.indent
            };
            let mut column = 0;
            while column + INDENT_WIDTH <= indent {
                let active = active_guide == Some(column) && is_cursor_line;
                text_painter.vline(
                    text_left + column as f32 * advance,
                    y..=(y + line.height),
                    Stroke::new(
                        theme::stroke::DIVIDER,
                        if active {
                            theme::editor::indent_guide_active()
                        } else {
                            theme::editor::indent_guide()
                        },
                    ),
                );
                column += INDENT_WIDTH;
            }
            if let Some(number) = self.line_numbers.get(index).and_then(Option::as_ref) {
                painter.galley(
                    egui::pos2(
                        content.left() - theme::space::SNUG - number.size().x,
                        y + (line_height - number.size().y) * 0.5,
                    ),
                    Arc::clone(number),
                    if is_cursor_line {
                        theme::text().primary
                    } else {
                        theme::text().muted
                    },
                );
            }
            if let Some((_, color)) = line_markers.iter().find(|(line, _)| *line == index) {
                painter.rect_filled(
                    Rect::from_min_size(
                        egui::pos2(content.left() - 3.0, y + (line_height - 6.0) * 0.5),
                        egui::vec2(3.0, 6.0),
                    ),
                    theme::corner(2),
                    *color,
                );
            }
            let mut galley = Arc::clone(base_galley);
            if selected.start < selected.end {
                let relative = CCursorRange::two(
                    CCursor::new(selected.start - line.char_start),
                    CCursor::new(selected.end - line.char_start),
                );
                let mut visuals = ui.visuals().clone();
                if !focused {
                    visuals.selection.bg_fill = theme::editor::selection_inactive();
                }
                egui::text_selection::visuals::paint_text_selection(
                    &mut galley,
                    &visuals,
                    &relative,
                    None,
                );
            }
            text_painter.galley(egui::pos2(text_left, y), galley, theme::syntax().foreground);
        }
        let caret_visible = focused
            && (((ui.input(|input| input.time) - self.caret_blink_started).max(0.0)
                / CARET_BLINK_INTERVAL) as u64)
                .is_multiple_of(2);
        if caret_visible {
            mark_retained(
                &painter,
                rect,
                0x3000_0000_0000_0000,
                self.cursor as u64 ^ u64::from(self.scroll_y.to_bits()) ^ horizontal_geometry,
            );
            if let Some(caret) = self.cursor_rect(content) {
                if block_caret {
                    text_painter.rect_filled(
                        Rect::from_min_size(caret.left_top(), egui::vec2(8.0, caret.height())),
                        0.0,
                        theme::accent().gamma_multiply(0.65),
                    );
                } else {
                    text_painter.line_segment(
                        [caret.left_top(), caret.left_bottom()],
                        Stroke::new(1.5, theme::accent()),
                    );
                }
            }
        }
    }

    fn handle_events(
        &mut self,
        ui: &Ui,
        text: &mut String,
        editor_id: Id,
        text_input: TextInputMode,
        native_keybindings: bool,
    ) -> (bool, Option<char>, String) {
        let events = ui.input(|input| input.events.clone());
        let mut changed = false;
        let mut last_inserted = None;
        let mut inserted_text = String::new();
        for event in events {
            match event {
                Event::Copy if text_input != TextInputMode::Disabled => self.copy(ui.ctx(), text),
                Event::Cut if text_input != TextInputMode::Disabled => {
                    self.copy(ui.ctx(), text);
                    changed |= self.replace_selection(text, "");
                }
                Event::Paste(value) | Event::Text(value)
                    if text_input != TextInputMode::Disabled && !value.is_empty() =>
                {
                    if text_input == TextInputMode::Replace {
                        let end = (self.cursor + value.chars().count())
                            .min(text_line_end(text, self.cursor));
                        self.set_selection(self.cursor, end);
                    }
                    changed |= self.replace_selection(text, &value);
                    last_inserted = value.chars().last();
                    inserted_text.push_str(&value);
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if native_keybindings => {
                    changed |= self.handle_key(ui, text, key, modifiers, editor_id);
                }
                Event::Ime(egui::ImeEvent::Commit(value))
                    if text_input != TextInputMode::Disabled && !value.is_empty() =>
                {
                    if text_input == TextInputMode::Replace {
                        let end = (self.cursor + value.chars().count())
                            .min(text_line_end(text, self.cursor));
                        self.set_selection(self.cursor, end);
                    }
                    changed |= self.replace_selection(text, &value);
                    last_inserted = value.chars().last();
                    inserted_text.push_str(&value);
                }
                _ => {}
            }
        }
        (changed, last_inserted, inserted_text)
    }

    fn handle_key(
        &mut self,
        ui: &Ui,
        text: &mut String,
        key: Key,
        modifiers: Modifiers,
        editor_id: Id,
    ) -> bool {
        let command = if modifiers.command {
            match (key, modifiers.shift) {
                (Key::A, _) => Some(Command::EditorSelectAll),
                (Key::Z, true) => Some(Command::EditorRedo),
                (Key::Z, false) => Some(Command::EditorUndo),
                (Key::Y, _) => Some(Command::EditorRedo),
                _ => None,
            }
        } else {
            match (key, modifiers.alt, modifiers.shift) {
                (Key::Backspace, _, _) => Some(Command::EditorDeleteLeft),
                (Key::Delete, _, _) => Some(Command::EditorDeleteRight),
                (Key::Enter, _, _) => Some(Command::EditorInsertLineBreak),
                (Key::Tab, _, true) => Some(Command::EditorOutdent),
                (Key::Tab, _, false) => Some(Command::EditorIndent),
                (Key::ArrowLeft, true, true) => Some(Command::EditorSelectWordLeft),
                (Key::ArrowLeft, true, false) => Some(Command::EditorCursorWordLeft),
                (Key::ArrowRight, true, true) => Some(Command::EditorSelectWordRight),
                (Key::ArrowRight, true, false) => Some(Command::EditorCursorWordRight),
                (Key::ArrowLeft, false, true) => Some(Command::EditorSelectLeft),
                (Key::ArrowLeft, false, false) => Some(Command::EditorCursorLeft),
                (Key::ArrowRight, false, true) => Some(Command::EditorSelectRight),
                (Key::ArrowRight, false, false) => Some(Command::EditorCursorRight),
                (Key::ArrowUp, _, true) => Some(Command::EditorSelectUp),
                (Key::ArrowUp, _, false) => Some(Command::EditorCursorUp),
                (Key::ArrowDown, _, true) => Some(Command::EditorSelectDown),
                (Key::ArrowDown, _, false) => Some(Command::EditorCursorDown),
                (Key::Home, _, true) => Some(Command::EditorSelectLineStart),
                (Key::Home, _, false) => Some(Command::EditorCursorLineStart),
                (Key::End, _, true) => Some(Command::EditorSelectLineEnd),
                (Key::End, _, false) => Some(Command::EditorCursorLineEnd),
                _ => None,
            }
        };
        if let Some(command) = command {
            return self.execute_command(ui.ctx(), text, command, None);
        }
        match key {
            Key::Escape => {
                ui.memory_mut(|memory| memory.surrender_focus(editor_id));
                false
            }
            _ => false,
        }
    }

    fn decrease_indent(&mut self, text: &mut String) -> bool {
        let line = self.line_for_character(self.cursor);
        let start = self.lines.get(line).map_or(0, |line| line.char_start);
        let remove = text
            .chars()
            .skip(start)
            .take(4)
            .take_while(|character| *character == ' ')
            .count();
        if remove == 0 {
            return false;
        }
        self.set_selection(start, start + remove);
        self.replace_selection(text, "")
    }

    fn copy(&self, ctx: &egui::Context, text: &str) {
        let selection = self.selection();
        if !selection.is_empty() {
            ctx.output_mut(|output| {
                output.commands.push(OutputCommand::CopyText(
                    char_slice(text, selection).to_owned(),
                ));
            });
        }
    }

    fn move_cursor(&mut self, cursor: usize, extend: bool) {
        self.cursor = cursor;
        if !extend {
            self.anchor = cursor;
        }
    }

    fn move_vertical(&mut self, direction: i8, extend: bool) {
        let line_index = self.line_for_character(self.cursor);
        let Some(line) = self.lines.get(line_index) else {
            return;
        };
        let Some(galley) = &line.galley else {
            return;
        };
        let relative = CCursor::new(self.cursor.saturating_sub(line.char_start));
        let row = galley.layout_from_cursor(relative).row;
        let h_pos = self
            .h_pos
            .unwrap_or_else(|| galley.pos_from_cursor(relative).left());
        let at_edge = if direction < 0 {
            row == 0 && line_index > 0
        } else {
            row + 1 == galley.rows.len() && line_index + 1 < self.lines.len()
        };
        let cursor = if at_edge {
            let target = if direction < 0 {
                line_index - 1
            } else {
                line_index + 1
            };
            let target_line = &self.lines[target];
            let Some(target_galley) = &target_line.galley else {
                return;
            };
            let y = if direction < 0 {
                target_galley.size().y
            } else {
                0.0
            };
            target_line.char_start + target_galley.cursor_from_pos(Vec2::new(h_pos, y)).index.0
        } else {
            let (within, _) = if direction < 0 {
                galley.cursor_up_one_row(&relative, Some(h_pos))
            } else {
                galley.cursor_down_one_row(&relative, Some(h_pos))
            };
            line.char_start + within.index.0
        };
        self.h_pos = Some(h_pos);
        self.move_cursor(cursor, extend);
    }

    fn line_for_character(&self, character: usize) -> usize {
        self.lines
            .partition_point(|line| line.char_start <= character)
            .saturating_sub(1)
            .min(self.lines.len().saturating_sub(1))
    }

    fn character_at(&self, pointer: Pos2, content: Rect) -> usize {
        if self.lines.is_empty() {
            return 0;
        }
        let document_y = (pointer.y - content.top() + self.scroll_y).max(0.0);
        let line_index = self
            .offsets
            .partition_point(|offset| *offset <= document_y)
            .saturating_sub(1)
            .min(self.lines.len() - 1);
        let line = &self.lines[line_index];
        let Some(galley) = &line.galley else {
            return line.char_start;
        };
        let local = egui::vec2(
            pointer.x - content.left() - TEXT_LEFT_PADDING + self.scroll_x,
            document_y - self.offsets[line_index],
        );
        line.char_start + galley.cursor_from_pos(local).index.0
    }

    fn cursor_rect(&self, content: Rect) -> Option<Rect> {
        let line_index = self.line_for_character(self.cursor);
        let line = self.lines.get(line_index)?;
        let galley = line.galley.as_ref()?;
        let relative = CCursor::new(self.cursor.saturating_sub(line.char_start));
        let local = galley.pos_from_cursor(relative);
        let translated = local.translate(egui::vec2(
            content.left() + TEXT_LEFT_PADDING - self.scroll_x,
            content.top() + self.offsets[line_index] - self.scroll_y,
        ));
        Some(Rect::from_min_size(
            translated.min,
            egui::vec2(translated.width(), translated.height().max(line_height())),
        ))
    }

    fn scroll_character_into_view(&mut self, character: usize, viewport_height: f32) {
        let index = self.line_for_character(character);
        let Some(line) = self.lines.get(index) else {
            return;
        };
        let top = self.offsets[index];
        let bottom = top + line.height;
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + viewport_height {
            self.scroll_y = bottom - viewport_height;
        }
        self.clamp_scroll(viewport_height);
    }

    fn scroll_character_horizontally_into_view(&mut self, character: usize, viewport_width: f32) {
        let index = self.line_for_character(character);
        let Some(line) = self.lines.get(index) else {
            return;
        };
        let Some(galley) = &line.galley else {
            return;
        };
        let relative = CCursor::new(character.saturating_sub(line.char_start));
        let cursor = galley.pos_from_cursor(relative);
        let visible_width = (viewport_width - TEXT_LEFT_PADDING).max(1.0);
        if cursor.left() < self.scroll_x {
            self.scroll_x = cursor.left();
        } else if cursor.right() > self.scroll_x + visible_width {
            self.scroll_x = cursor.right() - visible_width;
        }
    }

    fn clamp_selection(&mut self, character_len: usize) {
        self.anchor = self.anchor.min(character_len);
        self.cursor = self.cursor.min(character_len);
    }

    fn clamp_scroll(&mut self, viewport_height: f32) {
        let total = self.offsets.last().copied().unwrap_or(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, (total - viewport_height).max(0.0));
    }

    fn document_width(&self, advance: f32) -> f32 {
        self.lines
            .iter()
            .map(|line| {
                line.galley
                    .as_ref()
                    .map_or(line.character_len as f32 * advance, |galley| {
                        galley.size().x
                    })
            })
            .fold(0.0, f32::max)
            + TEXT_LEFT_PADDING
    }

    fn clamp_horizontal_scroll(&mut self, viewport_width: f32, document_width: f32) {
        self.scroll_x = self
            .scroll_x
            .clamp(0.0, (document_width - viewport_width).max(0.0));
    }

    fn update_ime(&self, ui: &Ui, rect: Rect, content: Rect) {
        if let Some(cursor_rect) = self.cursor_rect(content) {
            ui.output_mut(|output| {
                output.ime = Some(egui::output::IMEOutput {
                    rect,
                    cursor_rect,
                    should_interrupt_composition: false,
                });
            });
        }
    }
}

struct LineSpec {
    job: LayoutJob,
    char_start: usize,
    character_len: usize,
    indent: usize,
}

/// Leading whitespace in characters, with a tab counted as one tab stop.
fn indent_of(text: &str) -> usize {
    text.chars()
        .take_while(|character| *character == ' ' || *character == '\t')
        .map(|character| if character == '\t' { INDENT_WIDTH } else { 1 })
        .sum()
}

fn split_layout_job(highlighted: &LayoutJob, wrap_width: f32) -> Vec<LineSpec> {
    let mut char_start = 0;
    let mut range_start = 0;
    let mut section_start = 0;
    highlighted
        .text
        .split('\n')
        .map(|text| {
            let range = range_start..range_start + text.len();
            while highlighted
                .sections
                .get(section_start)
                .is_some_and(|section| section.byte_range.end.0 <= range.start)
            {
                section_start += 1;
            }
            let mut sections = Vec::new();
            for section in &highlighted.sections[section_start..] {
                if section.byte_range.start.0 >= range.end {
                    break;
                }
                let start = section.byte_range.start.0.max(range.start);
                let end = section.byte_range.end.0.min(range.end);
                if start < end {
                    let mut format = section.format.clone();
                    format.font_id = theme::typography::code_editor();
                    sections.push(LayoutSection {
                        leading_space: section.leading_space,
                        byte_range: ByteIndex(start - range.start)..ByteIndex(end - range.start),
                        format,
                    });
                }
            }
            if sections.is_empty() && !text.is_empty() {
                sections.push(LayoutSection {
                    leading_space: 0.0,
                    byte_range: ByteIndex(0)..ByteIndex(text.len()),
                    format: TextFormat {
                        font_id: theme::typography::code_editor(),
                        color: theme::syntax().foreground,
                        ..TextFormat::default()
                    },
                });
            }
            let line_start = char_start;
            let character_len = text.chars().count();
            char_start += character_len + 1;
            range_start = range.end.saturating_add(1);
            LineSpec {
                job: LayoutJob {
                    text: text.to_owned(),
                    sections,
                    wrap: egui::text::TextWrapping {
                        max_width: wrap_width,
                        break_anywhere: true,
                        ..Default::default()
                    },
                    ..Default::default()
                },
                char_start: line_start,
                character_len,
                indent: indent_of(text),
            }
        })
        .collect()
}

fn estimated_height(character_len: usize, wrap_width: f32, advance: f32) -> f32 {
    let width = character_len as f32 * advance;
    line_height() * (width / wrap_width.max(1.0)).ceil().max(1.0)
}

fn line_count(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count() + 1
}

/// Measured from the code face's own digit advance, so the gutter stays correct
/// at every font size rather than at the one it was tuned for.
fn gutter_width(lines: usize, advance: f32) -> f32 {
    let digits = lines.max(1).ilog10() + 1;
    digits as f32 * advance + theme::space::MEDIUM
}

fn digit_advance(ui: &Ui) -> f32 {
    ui.fonts_mut(|fonts| fonts.glyph_width(&theme::typography::code_editor(), '0'))
}

/// How much of a line's own indent a wrapped continuation keeps.
fn wrap_indent(indent: usize, advance: f32) -> f32 {
    if indent == 0 {
        0.0
    } else {
        (indent as f32 + WRAP_INDENT_CHARS) * advance
    }
}

/// A line that will be pushed right when it wraps has to wrap earlier, or the
/// continuation runs off the edge it was measured against.
fn wrap_line_width(wrap_width: f32, indent: usize, advance: f32) -> f32 {
    (wrap_width - wrap_indent(indent, advance)).max(advance * 4.0)
}

/// Moves every continuation row of a soft-wrapped line under its own indent.
/// egui lays a wrapped line out flush, and `Galley` hit-testing reads the same
/// row origins that painting does, so shifting them keeps the caret honest.
fn indent_wrapped_rows(galley: &mut Arc<Galley>, indent: usize, advance: f32) {
    let offset = wrap_indent(indent, advance);
    if offset <= 0.0 || galley.rows.len() < 2 {
        return;
    }
    let galley = Arc::make_mut(galley);
    for row in galley.rows.iter_mut().skip(1) {
        row.pos.x += offset;
    }
    galley.rect.max.x += offset;
    galley.mesh_bounds.max.x += offset;
}

fn text_line_start(text: &str, cursor: usize) -> usize {
    text.chars()
        .take(cursor)
        .collect::<Vec<_>>()
        .iter()
        .rposition(|character| *character == '\n')
        .map_or(0, |index| index + 1)
}

fn text_line_end(text: &str, cursor: usize) -> usize {
    let characters = text.chars().collect::<Vec<_>>();
    characters[cursor.min(characters.len())..]
        .iter()
        .position(|character| *character == '\n')
        .map_or(characters.len(), |offset| {
            cursor.min(characters.len()) + offset
        })
}

fn text_line_range(text: &str, cursor: usize) -> Range<usize> {
    let start = text_line_start(text, cursor);
    let end = text_line_end(text, cursor);
    start..end + usize::from(end < text.chars().count())
}

fn text_word_range(text: &str, cursor: usize) -> Range<usize> {
    let line_start = text_line_start(text, cursor);
    let line_end = text_line_end(text, cursor);
    let line = char_slice(text, line_start..line_end);
    let cursor = CCursor::new(cursor.saturating_sub(line_start).min(line.chars().count()));
    let start = egui::text_selection::text_cursor_state::ccursor_previous_word(line, cursor)
        .index
        .0;
    let end = egui::text_selection::text_cursor_state::ccursor_next_word(line, cursor)
        .index
        .0;
    line_start + start..line_start + end
}

fn previous_word(text: &str, cursor: usize) -> usize {
    egui::text_selection::text_cursor_state::ccursor_previous_word(
        text,
        CCursor::new(cursor.min(text.chars().count())),
    )
    .index
    .0
}

fn next_word(text: &str, cursor: usize) -> usize {
    egui::text_selection::text_cursor_state::ccursor_next_word(
        text,
        CCursor::new(cursor.min(text.chars().count())),
    )
    .index
    .0
}

fn selection_drag_scroll_delta(pointer_y: f32, top: f32, bottom: f32, dt: f32) -> f32 {
    let distance = if pointer_y < top {
        pointer_y - top
    } else if pointer_y > bottom {
        pointer_y - bottom
    } else {
        return 0.0;
    };
    distance.signum() * (distance.abs() * 12.0).clamp(100.0, 1_200.0) * dt
}

fn char_slice(text: &str, range: Range<usize>) -> &str {
    &text[byte_index(text, range.start)..byte_index(text, range.end)]
}

fn replace_chars(text: &mut String, range: Range<usize>, replacement: &str) {
    let bytes = byte_index(text, range.start)..byte_index(text, range.end);
    text.replace_range(bytes, replacement);
}

fn byte_index(text: &str, character: usize) -> usize {
    text.char_indices()
        .nth(character)
        .map_or(text.len(), |(index, _)| index)
}

#[cfg(test)]
mod tests {
    use super::{
        EditorSurface, gutter_width, selection_drag_scroll_delta, split_layout_job, theme,
    };
    use egui::{
        Color32, CursorIcon, Event, Id, Key, Modifiers, MouseWheelUnit, PointerButton, RawInput,
        Rect, TextFormat, TouchPhase, Vec2, pos2, text::LayoutJob,
    };
    use std::{
        ops::Range,
        time::{Duration, Instant},
    };

    fn painted_caret(primitives: &[egui::ClippedPrimitive]) -> bool {
        primitives
            .iter()
            .any(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) => mesh
                    .vertices
                    .iter()
                    .any(|vertex| vertex.color == theme::accent()),
                egui::epaint::Primitive::Callback(_) => false,
            })
    }

    fn multi_click_selection(text: &str, cursor: usize, clicks: usize) -> Range<usize> {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = text.to_owned();
        editor.set_selection(cursor, cursor);
        let job = LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            Color32::WHITE,
            400.0,
        );
        let screen = Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(400.0, 160.0)));
        let mut caret = None;
        let _ = context.run_ui(
            RawInput {
                screen_rect: screen,
                ..RawInput::default()
            },
            |ui| caret = editor.show(ui, &mut text, &job, 1, true, None).caret_rect,
        );
        let pointer = caret.unwrap().center() - Vec2::new(2.0, 0.0);
        for click in 0..clicks {
            for pressed in [true, false] {
                let _ = context.run_ui(
                    RawInput {
                        screen_rect: screen,
                        time: Some(click as f64 * 0.1 + if pressed { 0.01 } else { 0.02 }),
                        events: vec![
                            Event::PointerMoved(pointer),
                            Event::PointerButton {
                                pos: pointer,
                                button: PointerButton::Primary,
                                pressed,
                                modifiers: Modifiers::NONE,
                            },
                        ],
                        ..RawInput::default()
                    },
                    |ui| {
                        editor.show(ui, &mut text, &job, 1, false, None);
                    },
                );
            }
        }
        editor.selection()
    }

    #[test]
    fn double_click_selects_the_complete_identifier() {
        assert_eq!(
            multi_click_selection("let my_value = calculate();", 8, 2),
            4..12
        );
    }

    #[test]
    fn triple_click_selects_the_complete_line_including_its_break() {
        assert_eq!(
            multi_click_selection("first\nlet my_value = calculate();\nthird", 14, 3),
            6..34
        );
    }

    #[test]
    fn horizontal_arrows_collapse_a_selection_toward_their_direction() {
        let context = theme::test_context();
        let mut text = "abcdef".to_owned();
        for (command, expected) in [
            (crate::keybindings::Command::EditorCursorLeft, 2..2),
            (crate::keybindings::Command::EditorCursorRight, 5..5),
        ] {
            let mut editor = EditorSurface::default();
            editor.set_selection(2, 5);
            editor.execute_command(&context, &mut text, command, None);
            assert_eq!(editor.selection(), expected, "command {command:?}");
        }
    }

    #[test]
    fn word_navigation_stops_at_code_punctuation() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "foo.bar".to_owned();
        editor.set_selection(text.chars().count(), text.chars().count());

        editor.execute_command(
            &context,
            &mut text,
            crate::keybindings::Command::EditorCursorWordLeft,
            None,
        );

        assert_eq!(editor.selection(), 4..4);
    }

    #[test]
    fn retained_editor_replaces_selections_and_replays_delta_history() {
        let mut editor = EditorSurface::default();
        let mut text = "hello world".to_owned();
        editor.set_selection(6, 11);

        assert!(editor.replace_selection(&mut text, "Editur"));
        assert_eq!(text, "hello Editur");
        assert_eq!(editor.selection(), 12..12);

        assert!(editor.undo(&mut text));
        assert_eq!(text, "hello world");
        assert_eq!(editor.selection(), 6..11);

        assert!(editor.redo(&mut text));
        assert_eq!(text, "hello Editur");
        assert_eq!(editor.selection(), 12..12);
    }

    #[test]
    fn highlighted_large_file_splits_into_lines_without_quadratic_delay() {
        let mut job = LayoutJob::default();
        for _ in 0..3_000 {
            for (index, token) in ["pub ", "value", " = ", "1;\n"].into_iter().enumerate() {
                job.append(
                    token,
                    0.0,
                    TextFormat {
                        font_id: theme::typography::code_editor(),
                        color: theme::mix(
                            theme::text().primary,
                            theme::syntax().keyword,
                            index as f32 * 0.25,
                        ),
                        ..TextFormat::default()
                    },
                );
            }
        }

        let started = Instant::now();
        let lines = split_layout_job(&job, 800.0);

        assert_eq!(lines.len(), 3_001);
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "retained-line split took {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn width_only_resize_reuses_retained_line_storage() {
        let job = LayoutJob::simple(
            "first\nsecond\nthird".to_owned(),
            theme::typography::code_editor(),
            Color32::WHITE,
            400.0,
        );
        let mut editor = EditorSurface::default();
        editor.sync_lines(&job, 1, 400.0, 8.0);
        let allocations = editor
            .lines
            .iter()
            .map(|line| line.job.text.as_ptr())
            .collect::<Vec<_>>();

        editor.sync_lines(&job, 1, 800.0, 8.0);

        assert_eq!(
            allocations,
            editor
                .lines
                .iter()
                .map(|line| line.job.text.as_ptr())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn the_gutter_is_measured_from_the_code_face_rather_than_guessed() {
        let context = theme::test_context();
        let mut advance = 0.0;
        let _ = context.run_ui(RawInput::default(), |ui| advance = super::digit_advance(ui));

        assert!(advance > 0.0, "the code face has to report a digit advance");
        // Every extra digit costs exactly one advance, whatever the font size.
        assert!((gutter_width(999, advance) - gutter_width(99, advance) - advance).abs() < 0.01);
        for digits in [1usize, 2, 3, 5] {
            let lines = 10usize.pow(digits as u32 - 1);
            let width = gutter_width(lines, advance);
            assert!(
                width >= digits as f32 * advance,
                "{digits} digits do not fit in {width}"
            );
        }
    }

    #[test]
    fn requested_character_stays_in_view_when_the_caret_is_elsewhere() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = (0..40)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let target = text.chars().count() - 2;
        let job = LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            Color32::WHITE,
            180.0,
        );

        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(200.0, 90.0))),
                ..RawInput::default()
            },
            |ui| {
                editor.show(ui, &mut text, &job, 1, false, Some(target));
            },
        );

        assert!(editor.scroll_y > 0.0);
    }

    #[test]
    fn first_code_line_is_inset_from_the_editor_top() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            Color32::WHITE,
            200.0,
        );
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
                ..RawInput::default()
            },
            |ui| {
                editor.show(ui, &mut text, &job, 1, false, None);
            },
        );
        fn code_y(shape: &egui::epaint::Shape) -> Option<f32> {
            match shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "text" => Some(text.pos.y),
                egui::epaint::Shape::Vec(shapes) => shapes.iter().find_map(code_y),
                _ => None,
            }
        }
        let y = output
            .shapes
            .iter()
            .find_map(|shape| code_y(&shape.shape))
            .expect("painted code line");

        assert_eq!(y, 6.0);
    }

    #[test]
    fn configured_font_size_overrides_a_stale_highlight_job() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let configured = theme::typography::code_size();
        let stale = LayoutJob::simple(
            text.clone(),
            egui::FontId::monospace(configured - 2.0),
            Color32::WHITE,
            200.0,
        );
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
                ..RawInput::default()
            },
            |ui| {
                editor.show(ui, &mut text, &stale, 1, false, None);
            },
        );
        let painted = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "text" => {
                    Some(text.galley.job.sections[0].format.font_id.size)
                }
                _ => None,
            })
            .expect("painted code line");

        assert_eq!(painted, configured);
    }

    #[test]
    fn appearance_change_rebuilds_retained_font_metrics() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let configured = theme::typography::code_size();
        let job = LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            Color32::WHITE,
            200.0,
        );
        let input = || RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
            ..RawInput::default()
        };
        let _ = context.run_ui(input(), |ui| {
            editor.show(ui, &mut text, &job, 1, false, None);
        });
        editor.lines[0].job.sections[0].format.font_id.size = configured - 2.0;
        editor.appearance ^= 1;

        let output = context.run_ui(input(), |ui| {
            editor.show(ui, &mut text, &job, 1, false, None);
        });
        let painted = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::epaint::Shape::Text(text) if text.galley.text() == "text" => {
                    Some(text.galley.job.sections[0].format.font_id.size)
                }
                _ => None,
            })
            .expect("painted code line");

        assert_eq!(painted, configured);
    }

    /// One document, painted twice so the second frame has laid-out lines, which
    /// is what every reading-affordance test below looks at.
    fn paint_document(
        text: &str,
        selection: Option<(usize, usize)>,
    ) -> (Vec<egui::epaint::ClippedShape>, Vec<egui::ClippedPrimitive>) {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = text.to_owned();
        let job = LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            theme::syntax().foreground,
            600.0,
        );
        if let Some((anchor, cursor)) = selection {
            editor.set_selection(anchor, cursor);
        }
        let mut shapes = Vec::new();
        let mut primitives = Vec::new();
        for _ in 0..2 {
            let output = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(640.0, 300.0))),
                    ..RawInput::default()
                },
                |ui| {
                    editor.show(ui, &mut text, &job, 1, true, None);
                },
            );
            shapes = output.shapes.clone();
            primitives = context.tessellate(output.shapes, output.pixels_per_point);
        }
        (shapes, primitives)
    }

    fn painted(primitives: &[egui::ClippedPrimitive], color: Color32) -> bool {
        primitives
            .iter()
            .any(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) => {
                    mesh.vertices.iter().any(|vertex| vertex.color == color)
                }
                egui::epaint::Primitive::Callback(_) => false,
            })
    }

    fn fills(shapes: &[egui::epaint::ClippedShape], color: Color32) -> Vec<Rect> {
        shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Rect(rect) if rect.fill == color => Some(rect.rect),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_selection_is_unmistakable_against_the_document_and_still_readable() {
        let editor = theme::surface().editor;
        let selection = theme::composite(theme::editor::selection(), editor);

        assert!(
            theme::contrast_ratio(selection, editor) >= 1.5,
            "the 1.1:1 selection that made a dragged paragraph invisible is back"
        );
        assert!(theme::contrast_ratio(theme::text().primary, selection) >= 4.5);

        let (_, selected) = paint_document("first line\nsecond line", Some((0, 8)));
        assert!(
            painted(&selected, theme::editor::selection()),
            "a selected range has to paint something behind the glyphs"
        );
        let (_, unselected) = paint_document("first line\nsecond line", Some((0, 0)));
        assert!(!painted(&unselected, theme::editor::selection()));
    }

    #[test]
    fn the_active_line_is_visible_and_yields_to_a_selection() {
        let (with_caret, _) = paint_document("first line\nsecond line", Some((3, 3)));
        let active = theme::editor::line_active();

        assert_eq!(
            fills(&with_caret, active).len(),
            1,
            "exactly the caret's line gets the active fill"
        );
        assert!(
            theme::composite(active, theme::surface().editor).r()
                >= theme::surface().editor.r() + 8,
            "the active line is back to being invisible"
        );

        let (with_selection, _) = paint_document("first line\nsecond line", Some((0, 8)));
        assert!(
            fills(&with_selection, active).is_empty(),
            "the active line must step aside while a selection is showing"
        );
    }

    #[test]
    fn the_caret_line_number_is_the_only_emphasized_one() {
        let (shapes, _) = paint_document("one\ntwo\nthree", Some((5, 5)));
        let numbers: Vec<_> = shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::Text(text)
                    if text.galley.text().chars().all(char::is_numeric)
                        && !text.galley.text().is_empty() =>
                {
                    Some((text.galley.text().to_owned(), text.fallback_color))
                }
                _ => None,
            })
            .collect();

        assert_eq!(numbers.len(), 3, "one number per line: {numbers:?}");
        assert_eq!(
            numbers
                .iter()
                .filter(|(_, color)| *color == theme::text().primary)
                .count(),
            1
        );
        assert_eq!(
            numbers
                .iter()
                .find(|(_, color)| *color == theme::text().primary)
                .map(|(label, _): &(String, Color32)| label.as_str()),
            Some("2"),
            "the emphasized number has to be the caret's own"
        );
    }

    #[test]
    fn no_rule_divides_the_gutter_from_the_text_anymore() {
        let (shapes, _) = paint_document("fn main() {\n    let value = 1;\n}", None);
        let full_height_rules = shapes
            .iter()
            .filter(|clipped| match &clipped.shape {
                egui::epaint::Shape::LineSegment { points, .. } => {
                    (points[0].x - points[1].x).abs() < 0.01 && (points[1].y - points[0].y) > 200.0
                }
                _ => false,
            })
            .count();

        assert_eq!(
            full_height_rules, 0,
            "the gutter rule is still splitting the reading surface"
        );
    }

    #[test]
    fn indent_guides_appear_only_where_a_line_is_actually_indented() {
        let (shapes, _) = paint_document("fn main() {\n    let value = 1;\n}", None);
        let guides: Vec<_> = shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::epaint::Shape::LineSegment { points, stroke }
                    if stroke.color == theme::editor::indent_guide() =>
                {
                    Some(points[0].x)
                }
                _ => None,
            })
            .collect();

        assert_eq!(
            guides.len(),
            1,
            "only the one indented line owns a guide: {guides:?}"
        );
    }

    #[test]
    fn long_lines_do_not_wrap_by_default() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "word ".repeat(60);
        let job = LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            theme::syntax().foreground,
            240.0,
        );
        for _ in 0..2 {
            let _ = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(240.0, 300.0))),
                    ..RawInput::default()
                },
                |ui| {
                    editor.show(ui, &mut text, &job, 1, true, None);
                },
            );
        }

        assert_eq!(editor.lines[0].galley.as_ref().unwrap().rows.len(), 1);
    }

    #[test]
    fn long_lines_scroll_horizontally() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "word ".repeat(60);
        let job = LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            theme::syntax().foreground,
            240.0,
        );
        let input = |events| RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(240.0, 300.0))),
            events,
            ..RawInput::default()
        };
        let _ = context.run_ui(input(Vec::new()), |ui| {
            editor.show(ui, &mut text, &job, 1, false, None);
        });
        let _ = context.run_ui(
            input(vec![
                Event::PointerMoved(pos2(120.0, 120.0)),
                Event::MouseWheel {
                    unit: MouseWheelUnit::Line,
                    delta: Vec2::new(-8.0, 0.0),
                    phase: TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                },
            ]),
            |ui| {
                editor.show(ui, &mut text, &job, 1, false, None);
            },
        );

        assert!(editor.scroll_x > 0.0);
    }

    #[test]
    fn a_wrapped_continuation_resumes_under_its_own_indent() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = format!("    {}", "word ".repeat(60));
        let job = LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            theme::syntax().foreground,
            240.0,
        );
        let mut rows = Vec::new();
        for _ in 0..2 {
            let _ = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(240.0, 300.0))),
                    ..RawInput::default()
                },
                |ui| {
                    let character_len = text.chars().count();
                    editor.show_document_with_options(
                        ui,
                        &mut text,
                        &job,
                        super::DocumentMetrics {
                            revision: 1,
                            line_count: 1,
                            character_len,
                        },
                        super::EditorShowOptions {
                            request_focus: true,
                            scroll_to_character: None,
                            id: Id::new("editor"),
                            line_markers: &[],
                            text_input: super::TextInputMode::Standard,
                            native_keybindings: true,
                            block_caret: false,
                            wrap: true,
                        },
                    );
                },
            );
            rows = editor.lines[0]
                .galley
                .as_ref()
                .map(|galley| galley.rows.iter().map(|row| row.pos.x).collect())
                .unwrap_or_default();
        }

        assert!(rows.len() > 1, "the line under test never wrapped");
        assert!(
            rows[1..].iter().all(|x| *x > rows[0]),
            "a soft wrap still resumes in column zero: {rows:?}"
        );
    }

    #[test]
    fn a_diagnostic_reads_as_a_bar_at_the_gutter_edge_rather_than_a_dot() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "broken line".to_owned();
        let job = LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            theme::syntax().foreground,
            400.0,
        );
        let danger = theme::semantic().danger;
        let character_len = text.chars().count();
        let mut shapes = Vec::new();
        for _ in 0..2 {
            let output = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(400.0, 200.0))),
                    ..RawInput::default()
                },
                |ui| {
                    editor.show_document_with_options(
                        ui,
                        &mut text,
                        &job,
                        super::DocumentMetrics {
                            revision: 1,
                            line_count: 1,
                            character_len,
                        },
                        super::EditorShowOptions {
                            request_focus: false,
                            scroll_to_character: None,
                            id: Id::new("editor"),
                            line_markers: &[(0, danger)],
                            text_input: super::TextInputMode::Standard,
                            native_keybindings: true,
                            block_caret: false,
                            wrap: false,
                        },
                    );
                },
            );
            shapes = output.shapes;
        }
        let marker = fills(&shapes, danger)
            .into_iter()
            .next()
            .expect("a diagnostic marker");

        assert!(
            marker.height() > marker.width(),
            "a bar is taller than it is wide"
        );
        assert!(marker.height() >= 6.0);
    }

    #[test]
    fn caret_at_line_start_is_inset_from_the_gutter_edge() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            crate::theme::typography::code_editor(),
            egui::Color32::WHITE,
            200.0,
        );
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
                ..RawInput::default()
            },
            |ui| {
                editor.show(ui, &mut text, &job, 1, true, None);
            },
        );
        let content = Rect::from_min_max(pos2(20.0, 0.0), pos2(200.0, 200.0));

        assert!(editor.cursor_rect(content).unwrap().left() >= content.left() + 8.0);
    }

    #[test]
    fn caret_on_an_empty_line_has_visible_height() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "text\n".to_owned();
        editor.set_selection(text.chars().count(), text.chars().count());
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            crate::theme::typography::code_editor(),
            egui::Color32::WHITE,
            200.0,
        );
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
                ..RawInput::default()
            },
            |ui| {
                editor.show(ui, &mut text, &job, 1, true, None);
            },
        );
        let content = Rect::from_min_max(pos2(22.0, 0.0), pos2(200.0, 200.0));

        assert!(editor.cursor_rect(content).unwrap().height() >= super::line_height());
    }

    #[test]
    fn focused_caret_is_painted_during_the_former_hidden_blink_phase() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            crate::theme::typography::code_editor(),
            egui::Color32::WHITE,
            200.0,
        );
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
                time: Some(0.75),
                ..RawInput::default()
            },
            |ui| {
                editor.show(ui, &mut text, &job, 1, true, None);
            },
        );
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);

        assert!(painted_caret(&primitives));
    }

    #[test]
    fn focused_caret_blinks_on_a_slow_cadence() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            crate::theme::typography::code_editor(),
            egui::Color32::WHITE,
            200.0,
        );
        let input = |time| RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
            time: Some(time),
            ..RawInput::default()
        };
        let first = context.run_ui(input(0.0), |ui| {
            editor.show(ui, &mut text, &job, 1, true, None);
        });
        let hidden = context.run_ui(input(0.8), |ui| {
            editor.show(ui, &mut text, &job, 1, false, None);
        });
        let visible_again = context.run_ui(input(1.5), |ui| {
            editor.show(ui, &mut text, &job, 1, false, None);
        });

        assert!(painted_caret(
            &context.tessellate(first.shapes, first.pixels_per_point)
        ));
        assert!(!painted_caret(
            &context.tessellate(hidden.shapes, hidden.pixels_per_point)
        ));
        assert!(painted_caret(&context.tessellate(
            visible_again.shapes,
            visible_again.pixels_per_point,
        )));
    }

    #[test]
    fn transaction_undoes_a_complete_insert_session_at_once() {
        let mut editor = EditorSurface::default();
        let mut text = String::new();
        editor.begin_transaction();
        assert!(editor.replace_selection(&mut text, "a"));
        assert!(editor.replace_selection(&mut text, "b"));
        editor.end_transaction();

        assert!(editor.undo(&mut text));
        assert_eq!(text, "");
    }

    #[test]
    fn arrow_navigation_paints_the_caret_at_its_new_position() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            crate::theme::typography::code_editor(),
            egui::Color32::WHITE,
            200.0,
        );
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
                ..RawInput::default()
            },
            |ui| {
                editor.show(ui, &mut text, &job, 1, true, None);
            },
        );
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
                time: Some(0.75),
                events: vec![Event::Key {
                    key: Key::ArrowRight,
                    physical_key: Some(Key::ArrowRight),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
                ..RawInput::default()
            },
            |ui| {
                editor.show(ui, &mut text, &job, 1, false, None);
            },
        );
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);

        assert_eq!(editor.cursor(), 1);
        assert!(painted_caret(&primitives));
        assert!(context.memory(|memory| memory.has_focus(Id::new("editor"))));
    }

    #[test]
    fn vertical_arrows_preserve_the_column_across_logical_lines() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        editor.set_selection(1, 1);
        let mut text = "one\ntwo".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            crate::theme::typography::code_editor(),
            egui::Color32::WHITE,
            200.0,
        );
        let raw_input = || RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
            ..RawInput::default()
        };
        let _ = context.run_ui(raw_input(), |ui| {
            editor.show(ui, &mut text, &job, 1, true, None);
        });
        let mut input = raw_input();
        input.events.push(Event::Key {
            key: Key::ArrowDown,
            physical_key: Some(Key::ArrowDown),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        let _ = context.run_ui(input, |ui| {
            editor.show(ui, &mut text, &job, 1, false, None);
        });
        assert_eq!(editor.cursor(), 5);

        let mut input = raw_input();
        input.events.push(Event::Key {
            key: Key::ArrowUp,
            physical_key: Some(Key::ArrowUp),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        });
        let _ = context.run_ui(input, |ui| {
            editor.show(ui, &mut text, &job, 1, false, None);
        });

        assert_eq!(editor.cursor(), 1);
    }

    #[test]
    fn selection_drag_scroll_moves_only_toward_an_outside_pointer() {
        let deltas = [
            selection_drag_scroll_delta(80.0, 100.0, 500.0, 1.0 / 60.0),
            selection_drag_scroll_delta(300.0, 100.0, 500.0, 1.0 / 60.0),
            selection_drag_scroll_delta(520.0, 100.0, 500.0, 1.0 / 60.0),
        ];

        assert_eq!(
            deltas.map(|delta| (delta * 60.0).round()),
            [-240.0, 0.0, 240.0]
        );
    }

    #[test]
    fn moving_the_editor_horizontally_invalidates_retained_line_geometry() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "fn main() {}".to_owned();
        let job = LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            Color32::WHITE,
            300.0,
        );
        let mut draw = |left| {
            let output = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(700.0, 300.0))),
                    ..RawInput::default()
                },
                |root| {
                    root.scope_builder(
                        egui::UiBuilder::new().max_rect(Rect::from_min_size(
                            pos2(left, 0.0),
                            Vec2::new(400.0, 300.0),
                        )),
                        |ui| {
                            editor.show(ui, &mut text, &job, 1, false, None);
                        },
                    );
                },
            );
            context
                .tessellate(output.shapes, output.pixels_per_point)
                .into_iter()
                .find_map(|primitive| {
                    crate::renderer::retained_paint(&primitive.primitive)
                        .ok()
                        .flatten()
                        .filter(|paint| paint.key == 0x2000_0000_0000_0000)
                })
                .expect("retained first line")
                .revision
        };

        let before = draw(0.0);
        let shifted = draw(120.0);

        assert_ne!(before, shifted);
    }

    #[test]
    fn hovering_the_editor_uses_the_native_text_cursor() {
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            crate::theme::typography::code_editor(),
            egui::Color32::WHITE,
            200.0,
        );
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
                events: vec![Event::PointerMoved(pos2(100.0, 100.0))],
                ..RawInput::default()
            },
            |ui| {
                editor.show(ui, &mut text, &job, 1, false, None);
            },
        );

        assert_eq!(output.platform_output.cursor_icon, CursorIcon::Text);
    }
}
