use std::{
    io::{Read, Write},
    path::Path,
    sync::mpsc::{self, Receiver},
};

use egui::{
    Align2, Color32, Event, EventFilter, FontId, Id, Key, Layout, Modifiers, RichText, Sense,
    TextFormat, UiBuilder, text::LayoutJob,
};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

use crate::theme::{
    ACCENT, CANVAS, SURFACE, SURFACE_HOVER, SURFACE_INPUT, TEXT_MUTED, TEXT_PRIMARY, TEXT_SECONDARY,
};

const HEADER_HEIGHT: f32 = 30.0;
const TAB_WIDTH: f32 = 154.0;
const FONT_SIZE: f32 = 13.0;
const LINE_HEIGHT: f32 = 18.0;
const CONTENT_PADDING: f32 = 8.0;
const SCROLLBACK_ROWS: usize = 2_000;

pub(crate) struct TerminalPanel {
    sessions: Vec<TerminalSession>,
    active: usize,
    next_id: u64,
    focus_active: bool,
}

pub(crate) struct TerminalOutput {
    pub(crate) empty: bool,
    pub(crate) error: Option<String>,
}

struct TerminalSession {
    id: u64,
    title: String,
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
            next_id: 1,
            focus_active: false,
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

    pub(crate) fn focused(&self, ctx: &egui::Context) -> bool {
        self.sessions.get(self.active).is_some_and(|session| {
            ctx.memory(|memory| memory.has_focus(Id::new(("terminal_surface", session.id))))
        })
    }

    pub(crate) fn close_active(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.sessions.remove(self.active);
        self.active = self.active.min(self.sessions.len().saturating_sub(1));
        self.focus_active = !self.sessions.is_empty();
    }

    pub(crate) fn blur(&self, ctx: &egui::Context) {
        if let Some(session) = self.sessions.get(self.active) {
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
        let header = rect.with_max_y((rect.top() + HEADER_HEIGHT).min(rect.bottom()));
        let content = rect.with_min_y(header.bottom());
        ui.painter().rect_filled(rect, 0.0, CANVAS);
        ui.painter().rect_filled(header, 0.0, SURFACE);
        ui.painter().hline(
            header.x_range(),
            header.bottom() - 0.5,
            egui::Stroke::new(1.0, crate::theme::BORDER_SUBTLE),
        );

        let mut activate = None;
        let mut close = None;
        let mut add = false;
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("terminal_tabs")
                .max_rect(header)
                .layout(Layout::left_to_right(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                for (index, session) in self.sessions.iter().enumerate() {
                    let (_, tab) = ui.allocate_space(egui::vec2(TAB_WIDTH, header.height()));
                    let selected = self.active == index;
                    let response =
                        ui.interact(tab, Id::new(("terminal_tab", session.id)), Sense::click());
                    let hovered = response.hovered();
                    if selected || hovered {
                        ui.painter().rect_filled(
                            tab,
                            0.0,
                            if selected {
                                SURFACE_INPUT
                            } else {
                                SURFACE_HOVER
                            },
                        );
                    }
                    if selected {
                        ui.painter().hline(
                            tab.x_range(),
                            tab.bottom() - 1.0,
                            egui::Stroke::new(2.0, ACCENT),
                        );
                    }
                    let close_rect = egui::Rect::from_center_size(
                        egui::pos2(tab.right() - 14.0, tab.center().y),
                        egui::vec2(20.0, 20.0),
                    );
                    ui.painter().text(
                        egui::pos2(tab.left() + 12.0, tab.center().y),
                        Align2::LEFT_CENTER,
                        &session.title,
                        FontId::proportional(12.0),
                        if selected { TEXT_PRIMARY } else { TEXT_MUTED },
                    );
                    let close_response = ui
                        .interact(
                            close_rect,
                            Id::new(("terminal_tab_close", session.id)),
                            Sense::click(),
                        )
                        .on_hover_text(format!("Close {}", session.title));
                    if hovered || selected {
                        ui.painter().text(
                            close_rect.center(),
                            Align2::CENTER_CENTER,
                            "×",
                            FontId::proportional(14.0),
                            if close_response.hovered() {
                                TEXT_PRIMARY
                            } else {
                                TEXT_SECONDARY
                            },
                        );
                    }
                    if close_response.clicked() {
                        close = Some(index);
                    } else if response.clicked() {
                        activate = Some(index);
                    }
                }
                if ui
                    .add_sized(
                        egui::vec2(32.0, header.height()),
                        egui::Button::new(RichText::new("+").size(16.0).color(TEXT_SECONDARY))
                            .frame(false),
                    )
                    .on_hover_text("New Terminal")
                    .clicked()
                {
                    add = true;
                }
            },
        );

        if let Some(index) = activate {
            self.active = index;
            self.focus_active = true;
        }
        if let Some(index) = close {
            self.sessions.remove(index);
            if index < self.active {
                self.active -= 1;
            } else if index == self.active {
                self.active = self.active.min(self.sessions.len().saturating_sub(1));
            }
            self.focus_active = !self.sessions.is_empty();
        }
        let mut error = None;
        if add && let Err(add_error) = self.add(root, ui.ctx()) {
            error = Some(add_error);
        }
        let request_focus = std::mem::take(&mut self.focus_active);
        if let Some(session) = self.sessions.get_mut(self.active)
            && let Err(session_error) = session.show(ui, content, request_focus)
        {
            error = Some(session_error);
        }
        TerminalOutput {
            empty: self.sessions.is_empty(),
            error,
        }
    }

    fn add(&mut self, root: &Path, ctx: &egui::Context) -> Result<(), String> {
        let id = self.next_id;
        let mut command = CommandBuilder::new_default_prog();
        command.cwd(root);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        self.sessions.push(TerminalSession::spawn(
            id,
            format!("Terminal {id}"),
            command,
            ctx,
        )?);
        self.next_id += 1;
        self.active = self.sessions.len() - 1;
        self.focus_active = true;
        Ok(())
    }
}

impl TerminalSession {
    fn spawn(
        id: u64,
        title: String,
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
    ) -> Result<(), String> {
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
                .layout_no_wrap("M".into(), font.clone(), TEXT_PRIMARY)
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
        painter.rect_filled(rect, 0.0, CANVAS);
        for row in 0..rows {
            let mut job = LayoutJob::default();
            job.wrap.max_width = f32::INFINITY;
            for col in 0..cols {
                let cell = screen.cell(row, col);
                let mut foreground =
                    cell.map_or(TEXT_PRIMARY, |cell| terminal_color(cell.fgcolor(), true));
                let mut background = cell.map_or(Color32::TRANSPARENT, |cell| {
                    terminal_color(cell.bgcolor(), false)
                });
                if cell.is_some_and(vt100::Cell::inverse) {
                    std::mem::swap(&mut foreground, &mut background);
                    if foreground == Color32::TRANSPARENT {
                        foreground = CANVAS;
                    }
                }
                if cursor == Some((row, col)) {
                    foreground = CANVAS;
                    background = ACCENT;
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
                TEXT_PRIMARY,
            );
        }
        Ok(())
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

fn key_sequence(key: Key, modifiers: Modifiers, application_cursor: bool) -> Option<Vec<u8>> {
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
                TEXT_PRIMARY
            } else {
                Color32::TRANSPARENT
            }
        }
        vt100::Color::Rgb(red, green, blue) => Color32::from_rgb(red, green, blue),
        vt100::Color::Idx(index @ 0..=15) => ANSI_COLORS[index as usize],
        vt100::Color::Idx(index @ 16..=231) => {
            let index = index - 16;
            let component = |value| if value == 0 { 0 } else { value * 40 + 55 };
            Color32::from_rgb(
                component(index / 36),
                component((index / 6) % 6),
                component(index % 6),
            )
        }
        vt100::Color::Idx(index) => {
            let gray = 8 + (index - 232) * 10;
            Color32::from_gray(gray)
        }
    }
}

const ANSI_COLORS: [Color32; 16] = [
    Color32::from_rgb(30, 32, 36),
    Color32::from_rgb(224, 82, 82),
    Color32::from_rgb(105, 190, 112),
    Color32::from_rgb(224, 188, 87),
    Color32::from_rgb(91, 155, 213),
    Color32::from_rgb(190, 112, 198),
    Color32::from_rgb(86, 190, 190),
    Color32::from_rgb(205, 208, 214),
    Color32::from_rgb(103, 110, 122),
    Color32::from_rgb(240, 112, 112),
    Color32::from_rgb(135, 214, 141),
    Color32::from_rgb(241, 211, 119),
    Color32::from_rgb(119, 177, 231),
    Color32::from_rgb(211, 143, 218),
    Color32::from_rgb(113, 211, 211),
    Color32::from_rgb(245, 246, 248),
];

#[cfg(test)]
mod tests {
    use std::{
        io::Write as _,
        time::{Duration, Instant},
    };

    use egui::{Key, Modifiers};
    use portable_pty::CommandBuilder;

    use super::{TerminalSession, key_sequence};

    #[test]
    fn control_keys_use_ascii_control_codes() {
        assert_eq!(key_sequence(Key::C, Modifiers::CTRL, false), Some(vec![3]));
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
            CommandBuilder::new("/bin/sh"),
            &egui::Context::default(),
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
