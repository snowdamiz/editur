use egui::{
    Color32, CursorIcon, Event, EventFilter, FontId, Id, Key, Modifiers, OutputCommand, Pos2, Rect,
    Response, Sense, Stroke, TextFormat, Ui, Vec2,
    epaint::text::{Galley, LayoutJob},
    text::{ByteIndex, CCursor, CCursorRange, LayoutSection},
};
use std::{ops::Range, sync::Arc, time::Duration};

use crate::renderer::mark_retained;

const LINE_HEIGHT: f32 = 18.0;
const TEXT_LEFT_PADDING: f32 = 8.0;
const CARET_BLINK_INTERVAL: f64 = 0.7;
pub(crate) const EDITOR_BACKGROUND: Color32 = Color32::from_rgb(24, 24, 26);

struct RetainedLine {
    job: LayoutJob,
    char_start: usize,
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
    scroll_y: f32,
    scrollbar: crate::scrollbar::State,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
    lines: Vec<RetainedLine>,
    line_numbers: Vec<Option<Arc<Galley>>>,
    offsets: Vec<f32>,
    visual_revision: Option<u64>,
    wrap_width: u32,
    caret_blink_started: f64,
    caret_was_focused: bool,
}

pub struct EditorOutput {
    pub response: Response,
    pub cursor: usize,
    pub changed: bool,
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
        self.undo.push(Edit {
            start: range.start,
            deleted,
            inserted: replacement.to_owned(),
            before,
            after: (cursor, cursor),
        });
        self.redo.clear();
        true
    }

    pub fn undo(&mut self, text: &mut String) -> bool {
        let Some(edit) = self.undo.pop() else {
            return false;
        };
        let end = edit.start + edit.inserted.chars().count();
        replace_chars(text, edit.start..end, &edit.deleted);
        (self.anchor, self.cursor) = edit.before;
        self.redo.push(edit);
        true
    }

    pub fn redo(&mut self, text: &mut String) -> bool {
        let Some(edit) = self.redo.pop() else {
            return false;
        };
        let end = edit.start + edit.deleted.chars().count();
        replace_chars(text, edit.start..end, &edit.inserted);
        (self.anchor, self.cursor) = edit.after;
        self.undo.push(edit);
        true
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
        let desired = ui.available_size();
        let (_, rect) = ui.allocate_space(desired);
        let editor_rect = rect;
        let mut response = ui.interact(editor_rect, Id::new("editor"), Sense::click_and_drag());
        if ui.input(|input| {
            input
                .pointer
                .hover_pos()
                .is_some_and(|pointer| editor_rect.contains(pointer))
        }) {
            ui.output_mut(|output| output.cursor_icon = CursorIcon::Text);
        }
        let gutter_width = gutter_width(self.lines.len().max(line_count(text)));
        let content = Rect::from_min_max(
            egui::pos2(rect.left() + gutter_width, rect.top()),
            editor_rect.right_bottom(),
        );
        let wrap_width = (content.width() - TEXT_LEFT_PADDING).max(1.0);
        self.sync_lines(highlighted, visual_revision, wrap_width);
        self.clamp_selection(text);
        let cursor_before_input = self.cursor;

        if request_focus {
            response.request_focus();
        }
        let scrolling = ui.input(|input| {
            input
                .pointer
                .hover_pos()
                .is_some_and(|pointer| rect.contains(pointer))
        }) && ui.input(|input| input.smooth_scroll_delta.y != 0.0);
        if scrolling {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            self.scroll_y -= scroll;
        }
        self.clamp_scroll(content.height());

        self.layout_visible_lines(ui, content);
        let mut ensure_cursor_visible = request_focus;
        if let Some(character) = scroll_to_character {
            self.scroll_character_into_view(character, content.height());
            self.layout_visible_lines(ui, content);
        }

        if response.clicked() || response.drag_started() {
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
        }

        let mut changed = false;
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
            changed = self.handle_events(ui, text);
            if changed {
                response.mark_changed();
                ui.ctx().request_repaint();
            }
        }
        ensure_cursor_visible |= self.cursor != cursor_before_events && !response.dragged();

        self.clamp_selection(text);
        if ensure_cursor_visible {
            self.scroll_character_into_view(self.cursor, content.height());
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
        let caret_visible = focused
            && (((time - self.caret_blink_started).max(0.0) / CARET_BLINK_INTERVAL) as u64)
                .is_multiple_of(2);
        if focused {
            let elapsed = (time - self.caret_blink_started).max(0.0);
            let until_next = CARET_BLINK_INTERVAL - elapsed.rem_euclid(CARET_BLINK_INTERVAL);
            ui.ctx()
                .request_repaint_after(Duration::from_secs_f64(until_next));
        }
        self.paint(ui, rect, content, focused, caret_visible);
        if focused {
            self.update_ime(ui, rect, content);
        }
        if crate::scrollbar::show(
            ui,
            Id::new("editor_scrollbar"),
            rect,
            self.offsets.last().copied().unwrap_or(0.0),
            &mut self.scroll_y,
            &mut self.scrollbar,
            scrolling,
        ) {
            self.clamp_scroll(content.height());
            ui.ctx().request_repaint();
        }

        EditorOutput {
            response,
            cursor: self.cursor,
            changed,
        }
    }

    fn sync_lines(&mut self, highlighted: &LayoutJob, revision: u64, wrap_width: f32) {
        let width = wrap_width.round().to_bits();
        if self.visual_revision == Some(revision) && self.wrap_width == width {
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
                    RetainedLine {
                        height: estimated_height(&spec.job.text, wrap_width),
                        job: spec.job,
                        char_start: spec.char_start,
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
            y += line.height.max(LINE_HEIGHT);
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
        let mut changed_height = false;
        for index in indexes {
            let line = &mut self.lines[index];
            if line.galley.is_none() {
                let galley = ui.fonts_mut(|fonts| fonts.layout_job(line.job.clone()));
                let height = galley.size().y.max(LINE_HEIGHT);
                changed_height |= (height - line.height).abs() > f32::EPSILON;
                line.height = height;
                line.galley = Some(galley);
            }
            if self.line_numbers.len() <= index {
                self.line_numbers.resize(index + 1, None);
            }
            if self.line_numbers[index].is_none() {
                self.line_numbers[index] = Some(ui.fonts_mut(|fonts| {
                    fonts.layout_no_wrap(
                        (index + 1).to_string(),
                        FontId::monospace(12.0),
                        Color32::from_rgb(100, 106, 123),
                    )
                }));
            }
        }
        if changed_height {
            self.rebuild_offsets();
            self.clamp_scroll(content.height());
        }
    }

    fn paint(&self, ui: &Ui, rect: Rect, content: Rect, focused: bool, caret_visible: bool) {
        let painter = ui.painter_at(rect);
        mark_retained(
            &painter,
            rect,
            0x1000_0000_0000_0000,
            u64::from(rect.width().to_bits()) << 32 | u64::from(rect.height().to_bits()),
        );
        painter.rect_filled(rect, 0.0, EDITOR_BACKGROUND);
        painter.line_segment(
            [content.left_top(), content.left_bottom()],
            Stroke::new(1.0, Color32::from_rgb(53, 53, 59)),
        );
        let selection = self.selection();
        let cursor_line = self.line_for_character(self.cursor);
        for index in self.visible_lines(content.height()) {
            let line = &self.lines[index];
            let Some(base_galley) = &line.galley else {
                continue;
            };
            let y = content.top() + self.offsets[index] - self.scroll_y;
            let line_end = line.char_start + line.job.text.chars().count();
            let selected = selection.start.max(line.char_start)..selection.end.min(line_end);
            let selection_state = (selected.start < selected.end).then_some(
                (selected.start as u64).rotate_left(17) ^ (selected.end as u64).rotate_left(31),
            );
            let state = u64::from(y.to_bits())
                ^ selection_state.unwrap_or(0)
                ^ u64::from(index == cursor_line && focused);
            mark_retained(
                &painter,
                rect,
                0x2000_0000_0000_0000 | index as u64,
                line.revision ^ state,
            );
            if index == cursor_line && focused {
                painter.rect_filled(
                    Rect::from_min_size(
                        egui::pos2(content.left(), y),
                        egui::vec2(content.width(), line.height),
                    ),
                    0.0,
                    Color32::from_white_alpha(6),
                );
            }
            if let Some(number) = self.line_numbers.get(index).and_then(Option::as_ref) {
                painter.galley(
                    egui::pos2(
                        content.left() - 5.0 - number.size().x,
                        y + (LINE_HEIGHT - number.size().y) * 0.5,
                    ),
                    Arc::clone(number),
                    Color32::from_rgb(100, 106, 123),
                );
            }
            let mut galley = Arc::clone(base_galley);
            if selected.start < selected.end {
                let relative = CCursorRange::two(
                    CCursor::new(selected.start - line.char_start),
                    CCursor::new(selected.end - line.char_start),
                );
                egui::text_selection::visuals::paint_text_selection(
                    &mut galley,
                    &ui.visuals().clone(),
                    &relative,
                    None,
                );
            }
            painter.galley(
                egui::pos2(content.left() + TEXT_LEFT_PADDING, y),
                galley,
                Color32::LIGHT_GRAY,
            );
        }
        if caret_visible {
            mark_retained(
                &painter,
                rect,
                0x3000_0000_0000_0000,
                self.cursor as u64 ^ u64::from(self.scroll_y.to_bits()),
            );
            if let Some(caret) = self.cursor_rect(content) {
                painter.line_segment(
                    [caret.left_top(), caret.left_bottom()],
                    Stroke::new(1.5, Color32::from_rgb(185, 205, 235)),
                );
            }
        }
    }

    fn handle_events(&mut self, ui: &Ui, text: &mut String) -> bool {
        let events = ui.input(|input| input.events.clone());
        let mut changed = false;
        for event in events {
            match event {
                Event::Copy => self.copy(ui, text),
                Event::Cut => {
                    self.copy(ui, text);
                    changed |= self.replace_selection(text, "");
                }
                Event::Paste(value) | Event::Text(value) if !value.is_empty() => {
                    changed |= self.replace_selection(text, &value);
                }
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => changed |= self.handle_key(ui, text, key, modifiers),
                Event::Ime(egui::ImeEvent::Commit(value)) if !value.is_empty() => {
                    changed |= self.replace_selection(text, &value);
                }
                _ => {}
            }
        }
        changed
    }

    fn handle_key(&mut self, ui: &Ui, text: &mut String, key: Key, modifiers: Modifiers) -> bool {
        if modifiers.command {
            match key {
                Key::A => {
                    self.anchor = 0;
                    self.cursor = text.chars().count();
                    return false;
                }
                Key::Z if modifiers.shift => return self.redo(text),
                Key::Z => return self.undo(text),
                Key::Y => return self.redo(text),
                _ => {}
            }
        }
        match key {
            Key::Backspace => {
                if self.selection().is_empty() && self.cursor > 0 {
                    self.anchor = self.cursor - 1;
                }
                self.replace_selection(text, "")
            }
            Key::Delete => {
                if self.selection().is_empty() && self.cursor < text.chars().count() {
                    self.cursor += 1;
                }
                self.replace_selection(text, "")
            }
            Key::Enter => self.replace_selection(text, "\n"),
            Key::Tab if modifiers.shift => self.decrease_indent(text),
            Key::Tab => self.replace_selection(text, "    "),
            Key::ArrowLeft => {
                let next = if modifiers.alt {
                    previous_word(text, self.cursor)
                } else {
                    self.cursor.saturating_sub(1)
                };
                self.move_cursor(next, modifiers.shift);
                self.h_pos = None;
                false
            }
            Key::ArrowRight => {
                let next = if modifiers.alt {
                    next_word(text, self.cursor)
                } else {
                    (self.cursor + 1).min(text.chars().count())
                };
                self.move_cursor(next, modifiers.shift);
                self.h_pos = None;
                false
            }
            Key::ArrowUp => {
                self.move_vertical(-1, modifiers.shift);
                false
            }
            Key::ArrowDown => {
                self.move_vertical(1, modifiers.shift);
                false
            }
            Key::Home => {
                let line = self.line_for_character(self.cursor);
                let start = self.lines.get(line).map_or(0, |line| line.char_start);
                self.move_cursor(start, modifiers.shift);
                false
            }
            Key::End => {
                let line = self.line_for_character(self.cursor);
                let end = self.lines.get(line).map_or_else(
                    || text.chars().count(),
                    |line| line.char_start + line.job.text.chars().count(),
                );
                self.move_cursor(end, modifiers.shift);
                false
            }
            Key::Escape => {
                ui.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
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

    fn copy(&self, ui: &Ui, text: &str) {
        let selection = self.selection();
        if !selection.is_empty() {
            ui.output_mut(|output| {
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
            pointer.x - content.left() - TEXT_LEFT_PADDING,
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
            content.left() + TEXT_LEFT_PADDING,
            content.top() + self.offsets[line_index] - self.scroll_y,
        ));
        Some(Rect::from_min_size(
            translated.min,
            egui::vec2(translated.width(), translated.height().max(LINE_HEIGHT)),
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

    fn clamp_selection(&mut self, text: &str) {
        let len = text.chars().count();
        self.anchor = self.anchor.min(len);
        self.cursor = self.cursor.min(len);
    }

    fn clamp_scroll(&mut self, viewport_height: f32) {
        let total = self.offsets.last().copied().unwrap_or(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, (total - viewport_height).max(0.0));
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
                    sections.push(LayoutSection {
                        leading_space: section.leading_space,
                        byte_range: ByteIndex(start - range.start)..ByteIndex(end - range.start),
                        format: section.format.clone(),
                    });
                }
            }
            if sections.is_empty() && !text.is_empty() {
                sections.push(LayoutSection {
                    leading_space: 0.0,
                    byte_range: ByteIndex(0)..ByteIndex(text.len()),
                    format: TextFormat {
                        font_id: FontId::monospace(14.0),
                        color: Color32::LIGHT_GRAY,
                        ..TextFormat::default()
                    },
                });
            }
            let line_start = char_start;
            char_start += text.chars().count() + 1;
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
            }
        })
        .collect()
}

fn estimated_height(text: &str, wrap_width: f32) -> f32 {
    let width = text.chars().count() as f32 * 8.4;
    LINE_HEIGHT * (width / wrap_width.max(1.0)).ceil().max(1.0)
}

fn line_count(text: &str) -> usize {
    text.bytes().filter(|byte| *byte == b'\n').count() + 1
}

fn gutter_width(lines: usize) -> f32 {
    let digits = lines.max(1).ilog10() + 1;
    (digits as f32 * 7.0 + 11.0).max(22.0)
}

fn previous_word(text: &str, cursor: usize) -> usize {
    let characters: Vec<_> = text.chars().collect();
    let mut index = cursor.min(characters.len());
    while index > 0 && characters[index - 1].is_whitespace() {
        index -= 1;
    }
    while index > 0 && !characters[index - 1].is_whitespace() {
        index -= 1;
    }
    index
}

fn next_word(text: &str, cursor: usize) -> usize {
    let characters: Vec<_> = text.chars().collect();
    let mut index = cursor.min(characters.len());
    while index < characters.len() && !characters[index].is_whitespace() {
        index += 1;
    }
    while index < characters.len() && characters[index].is_whitespace() {
        index += 1;
    }
    index
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
    use super::{EditorSurface, gutter_width, selection_drag_scroll_delta, split_layout_job};
    use egui::{
        Color32, CursorIcon, Event, FontId, Id, Key, Modifiers, RawInput, Rect, TextFormat, Vec2,
        pos2, text::LayoutJob,
    };
    use std::time::{Duration, Instant};

    fn painted_caret(primitives: &[egui::ClippedPrimitive]) -> bool {
        primitives
            .iter()
            .any(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) => mesh
                    .vertices
                    .iter()
                    .any(|vertex| vertex.color == Color32::from_rgb(185, 205, 235)),
                egui::epaint::Primitive::Callback(_) => false,
            })
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
                        font_id: FontId::monospace(14.0),
                        color: Color32::from_gray(180 + index as u8),
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
    fn line_number_gutter_stays_compact_and_grows_with_digit_count() {
        assert_eq!(gutter_width(9), 22.0);
        assert_eq!(gutter_width(999), 32.0);
    }

    #[test]
    fn requested_character_stays_in_view_when_the_caret_is_elsewhere() {
        let context = egui::Context::default();
        let mut editor = EditorSurface::default();
        let mut text = (0..40)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        let target = text.chars().count() - 2;
        let job = LayoutJob::simple(text.clone(), FontId::monospace(14.0), Color32::WHITE, 180.0);

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
    fn caret_at_line_start_is_inset_from_the_gutter_edge() {
        let context = egui::Context::default();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            egui::FontId::monospace(14.0),
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
        let context = egui::Context::default();
        let mut editor = EditorSurface::default();
        let mut text = "text\n".to_owned();
        editor.set_selection(text.chars().count(), text.chars().count());
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            egui::FontId::monospace(14.0),
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

        assert!(editor.cursor_rect(content).unwrap().height() >= 18.0);
    }

    #[test]
    fn focused_caret_is_painted_during_the_former_hidden_blink_phase() {
        let context = egui::Context::default();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            egui::FontId::monospace(14.0),
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
        let context = egui::Context::default();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            egui::FontId::monospace(14.0),
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
    fn arrow_navigation_paints_the_caret_at_its_new_position() {
        let context = egui::Context::default();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            egui::FontId::monospace(14.0),
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
        let context = egui::Context::default();
        let mut editor = EditorSurface::default();
        editor.set_selection(1, 1);
        let mut text = "one\ntwo".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            egui::FontId::monospace(14.0),
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
    fn hovering_the_editor_uses_the_native_text_cursor() {
        let context = egui::Context::default();
        let mut editor = EditorSurface::default();
        let mut text = "text".to_owned();
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            egui::FontId::monospace(14.0),
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
