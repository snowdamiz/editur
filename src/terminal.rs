use std::{
    collections::HashMap,
    io::{Read, Write},
    path::Path,
    sync::mpsc::{self, Receiver},
};

use egui::{
    Align2, Color32, CursorIcon, Event, EventFilter, FontId, Id, Key, Layout, Modifiers, RichText,
    Sense, TextFormat, UiBuilder, text::LayoutJob,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::app::{
    DropZone, PaneId, PaneLayout, SplitAxis, TabDrop, resize_divider_stroke, stable_tab_drop_zone,
    tab_drop_preview,
};
use crate::theme;

const HEADER_HEIGHT: f32 = 30.0;
const TAB_WIDTH: f32 = 154.0;
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 18.0;
const CONTENT_PADDING: f32 = 8.0;
const SCROLLBACK_ROWS: usize = 2_000;

pub(crate) struct TerminalPanel {
    sessions: Vec<TerminalSession>,
    active: usize,
    active_pane: PaneId,
    pane_active_tabs: HashMap<PaneId, u64>,
    pane_layout: PaneLayout,
    next_id: u64,
    focus_active: bool,
    tab_drag: Option<u64>,
    tab_drop: Option<TabDrop>,
    tab_drop_split: Option<u64>,
}

pub(crate) struct TerminalOutput {
    pub(crate) empty: bool,
    pub(crate) error: Option<String>,
}

struct TerminalSession {
    id: u64,
    title: String,
    pane: PaneId,
    parser: vt100::Parser,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn Child + Send + Sync>,
    output: Receiver<Vec<u8>>,
    size: (u16, u16),
    exit_reported: bool,
}

impl Default for TerminalPanel {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            active: 0,
            active_pane: PaneId(0),
            pane_active_tabs: HashMap::new(),
            pane_layout: PaneLayout::default(),
            next_id: 1,
            focus_active: false,
            tab_drag: None,
            tab_drop: None,
            tab_drop_split: None,
        }
    }
}

impl TerminalPanel {
    pub(crate) fn is_empty(&self) -> bool {
        self.sessions.is_empty()
    }

    pub(crate) fn open(&mut self, root: &Path, ctx: &egui::Context) -> Result<(), String> {
        if self.sessions.is_empty() {
            self.add(root, ctx)?;
        }
        self.focus_active = true;
        Ok(())
    }

    pub(crate) fn open_at(&mut self, directory: &Path, ctx: &egui::Context) -> Result<(), String> {
        self.add(directory, ctx)?;
        self.focus_active = true;
        Ok(())
    }

    pub(crate) fn focused(&self, ctx: &egui::Context) -> bool {
        self.sessions.iter().any(|session| {
            ctx.memory(|memory| memory.has_focus(Id::new(("terminal_surface", session.id))))
        })
    }

    pub(crate) fn close_active(&mut self) {
        self.close_tab(self.active);
    }

    pub(crate) fn blur(&self, ctx: &egui::Context) {
        for session in &self.sessions {
            ctx.memory_mut(|memory| {
                memory.surrender_focus(Id::new(("terminal_surface", session.id)));
            });
        }
    }

    pub(crate) fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        root: &Path,
    ) -> TerminalOutput {
        ui.painter().rect_filled(rect, 0.0, theme::surface().chrome);
        let mut error = None;
        let mut panes = self.pane_layout.rects(rect);
        if self.update_tab_drag(ui.ctx(), rect, &panes) {
            panes = self.pane_layout.rects(rect);
        }
        let request_focus = std::mem::take(&mut self.focus_active);
        let mut clicked = None;
        for (pane, pane_rect) in panes.iter().copied() {
            let header =
                pane_rect.with_max_y((pane_rect.top() + HEADER_HEIGHT).min(pane_rect.bottom()));
            let content = pane_rect.with_min_y(header.bottom());
            self.draw_tabs(ui, header, pane, root, &mut error);
            let active = self
                .pane_active_tabs
                .get(&pane)
                .and_then(|id| self.sessions.iter().position(|session| session.id == *id))
                .or_else(|| {
                    self.sessions
                        .iter()
                        .position(|session| session.pane == pane)
                });
            if let Some(index) = active {
                ui.scope_builder(
                    UiBuilder::new()
                        .id_salt(("terminal_pane", pane.0))
                        .max_rect(content),
                    |ui| match self.sessions[index].show(
                        ui,
                        content,
                        request_focus && self.active == index,
                    ) {
                        Ok(true) => clicked = Some(index),
                        Ok(false) => {}
                        Err(session_error) => error = Some(session_error),
                    },
                );
            }
            ui.painter().vline(
                pane_rect.right() - 0.5,
                pane_rect.y_range(),
                egui::Stroke::new(1.0, theme::border::strong_color()),
            );
            ui.painter().hline(
                pane_rect.x_range(),
                pane_rect.bottom() - 0.5,
                egui::Stroke::new(1.0, theme::border::strong_color()),
            );
        }
        if let Some(index) = clicked {
            self.activate_tab(index);
        }
        if panes.len() > 1
            && self.focused(ui.ctx())
            && let Some((_, pane)) = panes.iter().find(|(pane, _)| *pane == self.active_pane)
        {
            ui.painter().rect_stroke(
                *pane,
                0.0,
                egui::Stroke::new(1.0, theme::border::focus_color()),
                egui::StrokeKind::Inside,
            );
        }
        self.draw_split_handles(ui, rect);
        if let Some(drop) = self.tab_drop {
            let preview = drop.preview.shrink(4.0);
            ui.painter()
                .rect_filled(preview, 5.0, theme::subtle(theme::accent()));
            ui.painter().rect_stroke(
                preview,
                5.0,
                egui::Stroke::new(1.5, theme::accent()),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(title) = self
            .tab_drag
            .and_then(|id| self.sessions.iter().find(|session| session.id == id))
            .map(|session| session.title.as_str())
        {
            crate::app::draw_tab_drag_ghost(ui.ctx(), title);
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        TerminalOutput {
            empty: self.sessions.is_empty(),
            error,
        }
    }

    fn draw_tabs(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        pane: PaneId,
        root: &Path,
        error: &mut Option<String>,
    ) {
        ui.painter().rect_filled(rect, 0.0, theme::surface().chrome);
        ui.painter().hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1.0, theme::border::hairline_color()),
        );
        let active = self.pane_active_tabs.get(&pane).copied();
        let tabs = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| session.pane == pane)
            .map(|(index, session)| (index, session.id, session.title.clone()))
            .collect::<Vec<_>>();
        let mut activate = None;
        let mut close = None;
        let mut reorder = None;
        let mut add = false;
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("terminal_tabs", pane.0))
                .max_rect(rect)
                .layout(Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.set_clip_rect(rect);
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                for (position, (index, id, title)) in tabs.iter().enumerate() {
                    let (_, tab) = ui.allocate_space(egui::vec2(TAB_WIDTH, rect.height()));
                    let selected = active == Some(*id);
                    let response = ui
                        .interact(tab, Id::new(("terminal_tab", id)), Sense::click_and_drag())
                        .on_hover_text(title);
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            ui.is_enabled(),
                            selected,
                            title,
                        )
                    });
                    let dragging = response.dragged();
                    if selected || response.hovered() || dragging {
                        ui.painter().rect_filled(
                            tab,
                            0.0,
                            if dragging {
                                theme::state::selected()
                            } else if selected {
                                theme::surface().editor
                            } else {
                                theme::state::hover()
                            },
                        );
                    }
                    let close_rect = egui::Rect::from_center_size(
                        egui::pos2(tab.right() - 14.0, tab.center().y),
                        egui::vec2(20.0, 20.0),
                    );
                    ui.painter().text(
                        egui::pos2(tab.left() + 12.0, tab.center().y),
                        Align2::LEFT_CENTER,
                        title,
                        if selected {
                            theme::typography::strong()
                        } else {
                            theme::typography::small()
                        },
                        if selected {
                            theme::text().primary
                        } else {
                            theme::text().muted
                        },
                    );
                    let close_response = ui
                        .interact(
                            close_rect,
                            Id::new(("terminal_tab_close", id)),
                            Sense::click(),
                        )
                        .on_hover_text(format!("Close {title}"));
                    if response.hovered() || selected {
                        ui.painter().text(
                            close_rect.center(),
                            Align2::CENTER_CENTER,
                            "×",
                            theme::typography::body(),
                            if close_response.hovered() {
                                theme::text().primary
                            } else {
                                theme::text().secondary
                            },
                        );
                    }
                    if close_response.clicked() {
                        close = Some(*index);
                    } else if response.clicked() {
                        activate = Some(*index);
                    }
                    if response.drag_started() {
                        self.tab_drag = Some(*id);
                        activate = Some(*index);
                    }
                    if dragging {
                        self.tab_drag = Some(*id);
                        if let Some(pointer) = ui.input(|input| input.pointer.interact_pos())
                            && rect.contains(pointer)
                        {
                            let first_left = tab.left() - position as f32 * TAB_WIDTH;
                            let target_position = ((pointer.x - first_left) / TAB_WIDTH)
                                .floor()
                                .clamp(0.0, (tabs.len() - 1) as f32)
                                as usize;
                            let target = tabs[target_position].0;
                            if target != *index {
                                reorder = Some((*index, target));
                            }
                        }
                    }
                }
                if ui
                    .add_sized(
                        egui::vec2(32.0, rect.height()),
                        egui::Button::new(
                            RichText::new("+")
                                .size(theme::typography::TITLE_SIZE)
                                .color(theme::text().secondary),
                        )
                        .frame(false),
                    )
                    .on_hover_text("New Terminal")
                    .clicked()
                {
                    add = true;
                }
            },
        );
        if let Some(index) = close {
            self.close_tab(index);
        } else if let Some((from, to)) = reorder {
            self.move_tab(from, to);
        } else if let Some(index) = activate {
            self.activate_tab(index);
        }
        if add && let Err(add_error) = self.add_to_pane(root, ui.ctx(), pane) {
            *error = Some(add_error);
        }
    }

    fn update_tab_drag(
        &mut self,
        ctx: &egui::Context,
        available: egui::Rect,
        panes: &[(PaneId, egui::Rect)],
    ) -> bool {
        let Some(id) = self.tab_drag else {
            self.tab_drop = None;
            self.tab_drop_split = None;
            return false;
        };
        let index = self.sessions.iter().position(|session| session.id == id);
        let previous = self.tab_drop;
        let pointer = ctx.pointer_hover_pos();
        let split = pointer.and_then(|pointer| {
            self.pane_layout
                .split_handles(available)
                .into_iter()
                .find(|handle| handle.hit_rect.expand(8.0).contains(pointer))
        });
        if let Some(handle) = split {
            self.tab_drop_split = Some(handle.id);
            self.tab_drop = Some(TabDrop {
                target: self.active_pane,
                zone: DropZone::Center,
                preview: split_insert_preview(handle.axis, handle.bounds, handle.hit_rect.center()),
            });
        } else {
            self.tab_drop_split = None;
            self.tab_drop = pointer.and_then(|pointer| {
                panes.iter().find_map(|(target, rect)| {
                    rect.contains(pointer).then(|| {
                        let previous = previous
                            .filter(|drop| drop.target == *target)
                            .map(|drop| drop.zone);
                        let zone = if pointer.y <= rect.top() + HEADER_HEIGHT
                            && previous.unwrap_or(DropZone::Center) == DropZone::Center
                        {
                            DropZone::Center
                        } else {
                            stable_tab_drop_zone(*rect, pointer, previous)
                        };
                        TabDrop {
                            target: *target,
                            zone,
                            preview: tab_drop_preview(*rect, zone),
                        }
                    })
                })
            });
        }
        if index.is_some_and(|index| {
            let source = self.sessions[index].pane;
            let only_tab = self
                .sessions
                .iter()
                .filter(|session| session.pane == source)
                .count()
                == 1;
            only_tab
                && (self.tab_drop_split.is_some()
                    || self
                        .tab_drop
                        .is_some_and(|drop| drop.zone != DropZone::Center && drop.target == source))
        }) {
            self.tab_drop = None;
            self.tab_drop_split = None;
        }
        let released = ctx.input(|input| input.pointer.primary_released());
        if released {
            let drop = self.tab_drop.take();
            let split = self.tab_drop_split.take();
            self.tab_drag = None;
            if let Some(index) = index {
                if let Some(split) = split {
                    self.drop_tab_at_split(index, split);
                    return true;
                }
                if let Some(drop) = drop {
                    self.drop_tab(index, drop.target, drop.zone);
                    return true;
                }
            }
        } else if !ctx.input(|input| input.pointer.primary_down()) {
            self.tab_drag = None;
            self.tab_drop = None;
            self.tab_drop_split = None;
        } else {
            ctx.request_repaint();
        }
        false
    }

    fn draw_split_handles(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        if self.tab_drag.is_some() {
            return;
        }
        for handle in self.pane_layout.split_handles(rect) {
            let response = ui.interact(
                handle.hit_rect,
                Id::new(("terminal_pane_divider", handle.id)),
                Sense::drag(),
            );
            let active = response.hovered() || response.dragged();
            if active {
                ui.ctx().set_cursor_icon(match handle.axis {
                    SplitAxis::Horizontal => CursorIcon::ResizeVertical,
                    SplitAxis::Vertical => CursorIcon::ResizeHorizontal,
                });
            }
            if response.dragged()
                && let Some(pointer) = ui.ctx().pointer_interact_pos()
                && self.pane_layout.resize_adjacent(handle.id, rect, pointer)
            {
                ui.ctx().request_repaint();
            }
            let center = handle.hit_rect.center();
            let line = match handle.axis {
                SplitAxis::Horizontal => [
                    egui::pos2(handle.hit_rect.left(), center.y),
                    egui::pos2(handle.hit_rect.right(), center.y),
                ],
                SplitAxis::Vertical => [
                    egui::pos2(center.x, handle.hit_rect.top()),
                    egui::pos2(center.x, handle.hit_rect.bottom()),
                ],
            };
            ui.painter()
                .line_segment(line, resize_divider_stroke(ui.ctx(), active));
        }
    }

    fn add(&mut self, root: &Path, ctx: &egui::Context) -> Result<(), String> {
        self.add_to_pane(root, ctx, self.active_pane)
    }

    fn add_to_pane(
        &mut self,
        root: &Path,
        ctx: &egui::Context,
        pane: PaneId,
    ) -> Result<(), String> {
        let id = self.next_id;
        let mut command = CommandBuilder::new_default_prog();
        command.cwd(root);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        self.sessions.push(TerminalSession::spawn(
            id,
            format!("Terminal {id}"),
            pane,
            command,
            ctx,
        )?);
        self.next_id += 1;
        self.active = self.sessions.len() - 1;
        self.active_pane = pane;
        self.pane_active_tabs.insert(pane, id);
        self.focus_active = true;
        Ok(())
    }

    fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.sessions.len() || to >= self.sessions.len() {
            return;
        }
        let session = self.sessions.remove(from);
        self.sessions.insert(to, session);
        self.active = if self.active == from {
            to
        } else if from < self.active && self.active <= to {
            self.active - 1
        } else if to <= self.active && self.active < from {
            self.active + 1
        } else {
            self.active
        };
    }

    fn activate_tab(&mut self, index: usize) {
        let Some(session) = self.sessions.get(index) else {
            return;
        };
        self.active = index;
        self.active_pane = session.pane;
        self.pane_active_tabs.insert(session.pane, session.id);
        self.focus_active = true;
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.sessions.len() {
            return;
        }
        let active_id = self.sessions.get(self.active).map(|session| session.id);
        let removed = self.sessions.remove(index);
        let next_in_pane = self
            .sessions
            .iter()
            .position(|session| session.pane == removed.pane);
        if self.pane_active_tabs.get(&removed.pane) == Some(&removed.id) {
            if let Some(next) = next_in_pane {
                self.pane_active_tabs
                    .insert(removed.pane, self.sessions[next].id);
            } else {
                self.pane_active_tabs.remove(&removed.pane);
                self.pane_layout.remove(removed.pane);
            }
        }
        let next = active_id
            .filter(|id| *id != removed.id)
            .and_then(|id| self.sessions.iter().position(|session| session.id == id))
            .or(next_in_pane)
            .or_else(|| (!self.sessions.is_empty()).then_some(0));
        if let Some(next) = next {
            self.activate_tab(next);
        } else {
            self.active = 0;
            self.active_pane = PaneId(0);
            self.pane_layout = PaneLayout::default();
            self.pane_active_tabs.clear();
            self.focus_active = false;
        }
    }

    fn drop_tab(&mut self, index: usize, target: PaneId, zone: DropZone) {
        let Some(source) = self.sessions.get(index).map(|session| session.pane) else {
            return;
        };
        if source == target
            && zone != DropZone::Center
            && self
                .sessions
                .iter()
                .filter(|session| session.pane == source)
                .count()
                == 1
        {
            return;
        }
        let destination = if zone == DropZone::Center {
            target
        } else if let Some(pane) = self.pane_layout.split(target, zone) {
            pane
        } else {
            return;
        };
        self.move_tab_to_pane(index, destination);
    }

    fn drop_tab_at_split(&mut self, index: usize, split: u64) {
        let Some(source) = self.sessions.get(index).map(|session| session.pane) else {
            return;
        };
        if self
            .sessions
            .iter()
            .filter(|session| session.pane == source)
            .count()
            == 1
        {
            return;
        }
        if let Some(destination) = self.pane_layout.insert_at_split(split) {
            self.move_tab_to_pane(index, destination);
        }
    }

    fn move_tab_to_pane(&mut self, index: usize, destination: PaneId) {
        let source = self.sessions[index].pane;
        let moved_id = self.sessions[index].id;
        self.sessions[index].pane = destination;
        self.active = index;
        self.active_pane = destination;
        self.pane_active_tabs.insert(destination, moved_id);
        if source != destination {
            if let Some(session) = self.sessions.iter().find(|session| session.pane == source) {
                if self.pane_active_tabs.get(&source) == Some(&moved_id) {
                    self.pane_active_tabs.insert(source, session.id);
                }
            } else {
                self.pane_active_tabs.remove(&source);
                self.pane_layout.remove(source);
            }
        }
        self.focus_active = true;
    }
}

impl TerminalSession {
    fn spawn(
        id: u64,
        title: String,
        pane: PaneId,
        command: CommandBuilder,
        ctx: &egui::Context,
    ) -> Result<Self, String> {
        let size = (24, 80);
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(|error| format!("cannot open terminal: {error}"))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("cannot read terminal: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("cannot write terminal: {error}"))?;
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("cannot start shell: {error}"))?;
        drop(pair.slave);

        let (sender, output) = mpsc::sync_channel(256);
        let repaint = ctx.clone();
        std::thread::spawn(move || {
            let mut buffer = [0_u8; 8 * 1024];
            while let Ok(read) = reader.read(&mut buffer) {
                if read == 0 || sender.send(buffer[..read].to_vec()).is_err() {
                    break;
                }
                repaint.request_repaint();
            }
        });

        Ok(Self {
            id,
            title,
            pane,
            parser: vt100::Parser::new(size.0, size.1, SCROLLBACK_ROWS),
            master: pair.master,
            writer,
            child,
            output,
            size,
            exit_reported: false,
        })
    }

    fn show(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        request_focus: bool,
    ) -> Result<bool, String> {
        while let Ok(bytes) = self.output.try_recv() {
            self.parser.process(&bytes);
        }
        if !self.exit_reported
            && let Some(status) = self
                .child
                .try_wait()
                .map_err(|error| format!("cannot read terminal status: {error}"))?
        {
            self.parser.process(
                format!("\r\n[Process exited with code {}]\r\n", status.exit_code()).as_bytes(),
            );
            self.exit_reported = true;
        }

        let font = FontId::monospace(FONT_SIZE);
        let cell_width = ui.fonts_mut(|fonts| {
            fonts
                .layout_no_wrap("M".into(), font.clone(), theme::text().primary)
                .size()
                .x
        });
        let cols = ((rect.width() - CONTENT_PADDING * 2.0) / cell_width.max(1.0))
            .floor()
            .clamp(2.0, u16::MAX as f32) as u16;
        let rows = ((rect.height() - CONTENT_PADDING * 2.0) / LINE_HEIGHT)
            .floor()
            .clamp(1.0, u16::MAX as f32) as u16;
        let size = (rows, cols);
        if size != self.size {
            self.master
                .resize(pty_size(size))
                .map_err(|error| format!("cannot resize terminal: {error}"))?;
            self.parser.screen_mut().set_size(rows, cols);
            self.size = size;
        }

        let response = ui.interact(rect, Id::new(("terminal_surface", self.id)), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::TextEdit, ui.is_enabled(), &self.title)
        });
        if response.clicked() {
            response.request_focus();
        }
        if request_focus {
            response.request_focus();
        }
        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll != 0.0 {
                let screen = self.parser.screen_mut();
                let rows = (scroll.abs() / LINE_HEIGHT).ceil().max(1.0) as usize;
                let target = if scroll > 0.0 {
                    screen.scrollback().saturating_add(rows)
                } else {
                    screen.scrollback().saturating_sub(rows)
                };
                screen.set_scrollback(target);
                ui.ctx().request_repaint();
            }
        }
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
            self.handle_events(ui)?;
        }

        let screen = self.parser.screen();
        let cursor =
            (screen.scrollback() == 0 && !screen.hide_cursor()).then(|| screen.cursor_position());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, theme::surface().editor);
        for row in 0..rows {
            let mut job = LayoutJob::default();
            job.wrap.max_width = f32::INFINITY;
            for col in 0..cols {
                let cell = screen.cell(row, col);
                let mut foreground = cell.map_or(theme::text().primary, |cell| {
                    terminal_color(cell.fgcolor(), true)
                });
                let mut background = cell.map_or(Color32::TRANSPARENT, |cell| {
                    terminal_color(cell.bgcolor(), false)
                });
                if cell.is_some_and(vt100::Cell::inverse) {
                    std::mem::swap(&mut foreground, &mut background);
                    if foreground == Color32::TRANSPARENT {
                        foreground = theme::surface().editor;
                    }
                }
                if cursor == Some((row, col)) {
                    foreground = theme::surface().editor;
                    background = theme::accent();
                }
                let mut format = TextFormat {
                    font_id: font.clone(),
                    color: foreground,
                    background,
                    line_height: Some(LINE_HEIGHT),
                    ..TextFormat::default()
                };
                if cell.is_some_and(vt100::Cell::underline) {
                    format.underline = egui::Stroke::new(1.0, foreground);
                }
                job.append(
                    cell.map_or(" ", |cell| {
                        let contents = cell.contents();
                        if contents.is_empty() { " " } else { contents }
                    }),
                    0.0,
                    format,
                );
            }
            let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
            painter.galley(
                egui::pos2(
                    rect.left() + CONTENT_PADDING,
                    rect.top() + CONTENT_PADDING + row as f32 * LINE_HEIGHT,
                ),
                galley,
                theme::text().primary,
            );
        }
        Ok(response.clicked())
    }

    fn handle_events(&mut self, ui: &egui::Ui) -> Result<(), String> {
        let application_cursor = self.parser.screen().application_cursor();
        for event in ui.input(|input| input.events.clone()) {
            let bytes = match event {
                Event::Paste(text) => {
                    if self.parser.screen().bracketed_paste() {
                        format!("\x1b[200~{text}\x1b[201~").into_bytes()
                    } else {
                        text.into_bytes()
                    }
                }
                Event::Text(text) if !text.is_empty() => text.into_bytes(),
                Event::Ime(egui::ImeEvent::Commit(text)) if !text.is_empty() => text.into_bytes(),
                Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => key_sequence(key, modifiers, application_cursor).unwrap_or_default(),
                _ => Vec::new(),
            };
            if !bytes.is_empty() {
                self.writer
                    .write_all(&bytes)
                    .and_then(|()| self.writer.flush())
                    .map_err(|error| format!("cannot write to terminal: {error}"))?;
                self.parser.screen_mut().set_scrollback(0);
            }
        }
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn pty_size((rows, cols): (u16, u16)) -> PtySize {
    PtySize {
        rows,
        cols,
        pixel_width: 0,
        pixel_height: 0,
    }
}

fn split_insert_preview(axis: SplitAxis, bounds: egui::Rect, center: egui::Pos2) -> egui::Rect {
    match axis {
        SplitAxis::Horizontal => {
            let half = bounds.height() / 6.0;
            bounds
                .with_min_y(center.y - half)
                .with_max_y(center.y + half)
        }
        SplitAxis::Vertical => {
            let half = bounds.width() / 6.0;
            bounds
                .with_min_x(center.x - half)
                .with_max_x(center.x + half)
        }
    }
}

fn key_sequence(key: Key, modifiers: Modifiers, application_cursor: bool) -> Option<Vec<u8>> {
    if key == Key::Backspace {
        if modifiers.mac_cmd {
            return Some(vec![21]);
        }
        if modifiers.alt || modifiers.ctrl {
            return Some(vec![23]);
        }
    }
    if modifiers.ctrl
        && !modifiers.mac_cmd
        && let Some(byte) = control_byte(key)
    {
        return Some(vec![byte]);
    }
    let bytes: &[u8] = match key {
        Key::Enter => b"\r",
        Key::Tab if modifiers.shift => b"\x1b[Z",
        Key::Tab => b"\t",
        Key::Backspace => b"\x7f",
        Key::Escape => b"\x1b",
        Key::ArrowUp if application_cursor => b"\x1bOA",
        Key::ArrowDown if application_cursor => b"\x1bOB",
        Key::ArrowRight if application_cursor => b"\x1bOC",
        Key::ArrowLeft if application_cursor => b"\x1bOD",
        Key::ArrowUp => b"\x1b[A",
        Key::ArrowDown => b"\x1b[B",
        Key::ArrowRight => b"\x1b[C",
        Key::ArrowLeft => b"\x1b[D",
        Key::Home => b"\x1b[H",
        Key::End => b"\x1b[F",
        Key::Delete => b"\x1b[3~",
        Key::Insert => b"\x1b[2~",
        Key::PageUp => b"\x1b[5~",
        Key::PageDown => b"\x1b[6~",
        _ => return None,
    };
    Some(bytes.to_vec())
}

fn control_byte(key: Key) -> Option<u8> {
    Some(match key {
        Key::A => 1,
        Key::B => 2,
        Key::C => 3,
        Key::D => 4,
        Key::E => 5,
        Key::F => 6,
        Key::G => 7,
        Key::H => 8,
        Key::I => 9,
        Key::J => 10,
        Key::K => 11,
        Key::L => 12,
        Key::M => 13,
        Key::N => 14,
        Key::O => 15,
        Key::P => 16,
        Key::Q => 17,
        Key::R => 18,
        Key::S => 19,
        Key::T => 20,
        Key::U => 21,
        Key::V => 22,
        Key::W => 23,
        Key::X => 24,
        Key::Y => 25,
        Key::Z => 26,
        _ => return None,
    })
}

fn terminal_color(color: vt100::Color, foreground: bool) -> Color32 {
    match color {
        vt100::Color::Default => {
            if foreground {
                theme::text().primary
            } else {
                Color32::TRANSPARENT
            }
        }
        vt100::Color::Rgb(red, green, blue) => theme::color::literal(red, green, blue),
        vt100::Color::Idx(index @ 0..=15) => theme::color::ansi()[index as usize],
        vt100::Color::Idx(index @ 16..=231) => {
            let index = index - 16;
            let component = |value| if value == 0 { 0 } else { value * 40 + 55 };
            theme::color::literal(
                component(index / 36),
                component((index / 6) % 6),
                component(index % 6),
            )
        }
        vt100::Color::Idx(index) => theme::color::literal_gray(8 + (index - 232) * 10),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::{
        io::Write as _,
        path::Path,
        time::{Duration, Instant},
    };

    use egui::{Key, Modifiers};
    #[cfg(unix)]
    use portable_pty::CommandBuilder;

    #[cfg(unix)]
    use crate::app::{DropZone, PaneId};
    #[cfg(unix)]
    use crate::theme;

    use super::key_sequence;
    #[cfg(unix)]
    use super::{TerminalPanel, TerminalSession};

    #[cfg(unix)]
    #[test]
    fn terminal_tabs_can_be_reordered() {
        let ctx = theme::test_context();
        let mut panel = TerminalPanel::default();
        panel.add(Path::new("."), &ctx).unwrap();
        panel.add(Path::new("."), &ctx).unwrap();

        panel.move_tab(0, 1);

        assert_eq!(
            panel
                .sessions
                .iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            vec![2, 1]
        );
        assert_eq!(panel.sessions[panel.active].id, 2);
    }

    #[cfg(unix)]
    #[test]
    fn terminal_tabs_can_split_into_panes() {
        let ctx = theme::test_context();
        let mut panel = TerminalPanel::default();
        panel.add(Path::new("."), &ctx).unwrap();
        panel.add(Path::new("."), &ctx).unwrap();

        panel.drop_tab(1, PaneId(0), DropZone::Right);

        assert_eq!(
            panel
                .pane_layout
                .rects(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 400.0)
                ))
                .len(),
            2
        );
        assert_ne!(panel.sessions[0].pane, panel.sessions[1].pane);
    }

    #[cfg(unix)]
    #[test]
    fn terminal_tab_can_be_inserted_between_two_existing_panes() {
        let ctx = theme::test_context();
        let mut panel = TerminalPanel::default();
        panel.add(Path::new("."), &ctx).unwrap();
        panel.add(Path::new("."), &ctx).unwrap();
        panel.drop_tab(1, PaneId(0), DropZone::Right);
        panel.add(Path::new("."), &ctx).unwrap();
        let available = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 400.0));
        let divider = panel.pane_layout.split_handles(available)[0];

        panel.drop_tab_at_split(2, divider.id);

        let panes = panel
            .pane_layout
            .rects(available)
            .into_iter()
            .map(|(pane, _)| {
                panel
                    .sessions
                    .iter()
                    .find(|session| session.pane == pane)
                    .unwrap()
                    .id
            })
            .collect::<Vec<_>>();
        assert_eq!(panes, [1, 3, 2]);
    }

    #[cfg(unix)]
    #[test]
    fn selected_terminal_tab_uses_the_editor_tab_face_without_an_accent_border() {
        let ctx = theme::test_context();
        let mut panel = TerminalPanel::default();
        panel.add(Path::new("."), &ctx).unwrap();

        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 400.0),
                )),
                ..egui::RawInput::default()
            },
            |ui| {
                panel.show(ui, ui.max_rect(), Path::new("."));
            },
        );
        let tab = ctx
            .read_response(egui::Id::new(("terminal_tab", 1_u64)))
            .expect("terminal tab")
            .rect;

        assert!(output.shapes.iter().any(|shape| {
            matches!(
                &shape.shape,
                egui::Shape::Rect(rect)
                    if rect.rect == tab && rect.fill == theme::surface().editor
            )
        }));
        assert!(!output.shapes.iter().any(|shape| {
            matches!(
                &shape.shape,
                egui::Shape::LineSegment { points, stroke }
                    if tab.contains(points[0])
                        && tab.contains(points[1])
                        && stroke.color == theme::accent()
            )
        }));
    }

    #[cfg(unix)]
    #[test]
    fn selected_terminal_pane_draws_the_editor_focus_outline() {
        let ctx = theme::test_context();
        let mut panel = TerminalPanel::default();
        panel.add(Path::new("."), &ctx).unwrap();
        panel.add(Path::new("."), &ctx).unwrap();
        panel.drop_tab(1, PaneId(0), DropZone::Right);

        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 400.0),
                )),
                ..egui::RawInput::default()
            },
            |ui| {
                panel.show(ui, ui.max_rect(), Path::new("."));
            },
        );

        assert!(output.shapes.iter().any(|shape| {
            matches!(
                &shape.shape,
                egui::Shape::Rect(rect) if rect.stroke.color == theme::border::focus_color()
            )
        }));
    }

    #[cfg(unix)]
    #[test]
    fn unfocused_terminal_panes_do_not_draw_a_focus_outline() {
        let ctx = theme::test_context();
        let mut panel = TerminalPanel::default();
        panel.add(Path::new("."), &ctx).unwrap();
        panel.add(Path::new("."), &ctx).unwrap();
        panel.drop_tab(1, PaneId(0), DropZone::Right);
        panel.focus_active = false;
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("editor")));

        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 400.0),
                )),
                ..egui::RawInput::default()
            },
            |ui| {
                panel.show(ui, ui.max_rect(), Path::new("."));
            },
        );

        assert!(!output.shapes.iter().any(|shape| {
            matches!(
                &shape.shape,
                egui::Shape::Rect(rect) if rect.stroke.color == theme::border::focus_color()
            )
        }));
    }

    #[cfg(unix)]
    #[test]
    fn closing_a_panes_last_terminal_collapses_the_pane() {
        let ctx = theme::test_context();
        let mut panel = TerminalPanel::default();
        panel.add(Path::new("."), &ctx).unwrap();
        panel.add(Path::new("."), &ctx).unwrap();
        panel.drop_tab(1, PaneId(0), DropZone::Right);

        panel.close_active();

        assert_eq!(panel.sessions.len(), 1);
        assert_eq!(
            panel
                .pane_layout
                .rects(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 400.0)
                ))
                .len(),
            1
        );
    }

    #[test]
    fn control_keys_use_ascii_control_codes() {
        assert_eq!(key_sequence(Key::C, Modifiers::CTRL, false), Some(vec![3]));
    }

    #[test]
    fn mac_command_backspace_deletes_to_the_prompt_start() {
        let modifiers = Modifiers {
            mac_cmd: true,
            command: true,
            ..Modifiers::NONE
        };

        assert_eq!(
            key_sequence(Key::Backspace, modifiers, false),
            Some(vec![21])
        );
    }

    #[test]
    fn option_or_control_backspace_deletes_the_previous_word() {
        assert_eq!(
            key_sequence(Key::Backspace, Modifiers::ALT, false),
            Some(vec![23])
        );
        assert_eq!(
            key_sequence(Key::Backspace, Modifiers::CTRL, false),
            Some(vec![23])
        );
    }

    #[test]
    fn arrows_follow_the_terminal_cursor_mode() {
        assert_eq!(
            key_sequence(Key::ArrowUp, Modifiers::NONE, false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            key_sequence(Key::ArrowUp, Modifiers::NONE, true),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn printable_keys_are_handled_by_text_events() {
        assert_eq!(key_sequence(Key::A, Modifiers::NONE, false), None);
    }

    #[cfg(unix)]
    #[test]
    fn terminal_session_round_trips_shell_input() {
        let mut session = TerminalSession::spawn(
            1,
            "Terminal 1".into(),
            PaneId(0),
            CommandBuilder::new("/bin/sh"),
            &theme::test_context(),
        )
        .unwrap();
        session
            .writer
            .write_all(b"printf editur-terminal-ready\\n\r")
            .unwrap();
        session.writer.flush().unwrap();

        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            while let Ok(bytes) = session.output.try_recv() {
                session.parser.process(&bytes);
            }
            if session
                .parser
                .screen()
                .contents()
                .contains("editur-terminal-ready")
            {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        panic!("shell output was {:?}", session.parser.screen().contents());
    }
}
