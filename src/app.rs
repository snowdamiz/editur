use std::{
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

use egui::{
    Align2, Color32, FontId, Id, Key, Label, Layout, RichText, ScrollArea, Sense, TextEdit,
    TextFormat, UiBuilder, ViewportId, text::LayoutJob,
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{Icon, Window, WindowId},
};

use crate::{
    buffer::Buffer,
    editor_surface::{EDITOR_BACKGROUND, EditorSurface},
    file_io::{OpenTarget, SaveError, load_buffer, safe_save},
    instance::{Claim, InstanceEvent, claim, open_running, spawn_listener},
    renderer::Renderer,
    search::{SearchController, SearchHit, SearchResults},
    syntax::{Highlighter, IncrementalHighlightCache, SyntaxManager, data_dir},
    tree::{TreeEntry, read_directory},
    tree_surface::{TreeRow, TreeSurface},
};

#[derive(Clone)]
enum PendingAction {
    Open(PathBuf),
    OpenTarget(OpenTarget),
    Close,
}

#[derive(Clone, Copy)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum WindowAction {
    Close,
    Minimize,
    ToggleMaximize,
    Drag,
}

struct TreeState {
    root: PathBuf,
    children: HashMap<PathBuf, Vec<TreeEntry>>,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    visible: Vec<TreeRow>,
    selected_index: Option<usize>,
}

impl TreeState {
    fn new(root: PathBuf, selected: Option<PathBuf>) -> Result<Self, String> {
        let root_entries = read_directory(&root)?;
        let mut state = Self {
            children: HashMap::from([(root.clone(), root_entries)]),
            root,
            expanded: HashSet::new(),
            selected,
            visible: Vec::new(),
            selected_index: None,
        };
        state.refresh_visible();
        Ok(state)
    }

    fn refresh_visible(&mut self) {
        self.visible.clear();
        Self::append_visible(
            &self.children,
            &self.expanded,
            &self.root,
            0,
            &mut self.visible,
        );
        self.selected_index = self.selected.as_ref().and_then(|selected| {
            self.visible
                .iter()
                .position(|row| &row.entry.path == selected)
        });
    }

    fn append_visible(
        children: &HashMap<PathBuf, Vec<TreeEntry>>,
        expanded: &HashSet<PathBuf>,
        directory: &Path,
        depth: usize,
        result: &mut Vec<TreeRow>,
    ) {
        let Some(entries) = children.get(directory) else {
            return;
        };
        for entry in entries {
            let mut hasher = DefaultHasher::new();
            entry.path.hash(&mut hasher);
            entry.name.hash(&mut hasher);
            result.push(TreeRow {
                entry: entry.clone(),
                label: entry.name.to_string_lossy().into_owned(),
                depth,
                directory: entry.is_dir,
                expanded: expanded.contains(&entry.path),
                revision: hasher.finish(),
            });
            if entry.is_dir && expanded.contains(&entry.path) {
                Self::append_visible(children, expanded, &entry.path, depth + 1, result);
            }
        }
    }

    fn toggle(&mut self, path: &Path) -> Result<(), String> {
        if self.expanded.remove(path) {
            self.refresh_visible();
            return Ok(());
        }
        if !self.children.contains_key(path) {
            self.children
                .insert(path.to_path_buf(), read_directory(path)?);
        }
        self.expanded.insert(path.to_path_buf());
        self.refresh_visible();
        Ok(())
    }

    fn collapse(&mut self, path: &Path) {
        if self.expanded.remove(path) {
            self.refresh_visible();
        }
    }

    fn select(&mut self, path: Option<PathBuf>) {
        self.selected_index = path
            .as_ref()
            .and_then(|path| self.visible.iter().position(|row| row.entry.path == *path));
        self.selected = path;
    }
}

#[derive(Default)]
struct HighlightCache {
    revision: u64,
    syntax: String,
    job: LayoutJob,
    valid: bool,
    incremental: IncrementalHighlightCache,
    find_revision: u64,
    find_query: String,
    find_selected: usize,
    find_job: LayoutJob,
    find_valid: bool,
    galley_key: Option<GalleyKey>,
    presentation_revision: u64,
}

#[derive(Clone, PartialEq)]
struct GalleyKey {
    revision: u64,
    syntax: String,
    wrap_width: u32,
    find: Option<(String, usize)>,
    bracket_pair: Option<(std::ops::Range<usize>, std::ops::Range<usize>)>,
}

pub struct EditorApp {
    buffer: Option<Buffer>,
    tree: TreeState,
    tree_surface: TreeSurface,
    syntaxes: SyntaxManager,
    highlighter: Highlighter,
    highlight_cache: HighlightCache,
    editor_surface: EditorSurface,
    search: SearchController,
    search_open: bool,
    search_query: String,
    search_selected: usize,
    focus_search: bool,
    find_open: bool,
    find_query: String,
    focus_find: bool,
    find_matches: Vec<std::ops::Range<usize>>,
    find_match_revision: u64,
    find_match_query: String,
    find_selected: usize,
    scroll_to_find_match: bool,
    bracket_pair: Option<(std::ops::Range<usize>, std::ops::Range<usize>)>,
    sidebar: bool,
    sidebar_width: f32,
    focus_editor: bool,
    tree_focused: bool,
    cursor: (usize, usize),
    pending: Option<PendingAction>,
    conflict: bool,
    save_as: Option<String>,
    error: Option<String>,
    should_close: bool,
    window_action: Option<WindowAction>,
}

impl EditorApp {
    pub fn new(target: OpenTarget) -> Result<Self, String> {
        let buffer = target.file.as_deref().map_or(Ok(None), |path| {
            if target.create {
                Ok(Some(Buffer::new(path.to_path_buf())))
            } else {
                load_buffer(path).map(Some)
            }
        })?;
        let selected = buffer.as_ref().map(|buffer| buffer.path.clone());
        let syntaxes = data_dir()
            .and_then(|directory| SyntaxManager::load(&directory))
            .or_else(|_| SyntaxManager::built_in())?;
        let search = SearchController::new(target.root.clone())?;
        Ok(Self {
            buffer,
            tree: TreeState::new(target.root, selected)?,
            tree_surface: TreeSurface::default(),
            syntaxes,
            highlighter: Highlighter::new()?,
            highlight_cache: HighlightCache::default(),
            editor_surface: EditorSurface::default(),
            search,
            search_open: false,
            search_query: String::new(),
            search_selected: 0,
            focus_search: false,
            find_open: false,
            find_query: String::new(),
            focus_find: false,
            find_matches: Vec::new(),
            find_match_revision: u64::MAX,
            find_match_query: String::new(),
            find_selected: 0,
            scroll_to_find_match: false,
            bracket_pair: None,
            sidebar: true,
            sidebar_width: 220.0,
            focus_editor: target.file.is_some(),
            tree_focused: target.file.is_none(),
            cursor: (1, 1),
            pending: None,
            conflict: false,
            save_as: None,
            error: None,
            should_close: false,
            window_action: None,
        })
    }

    pub fn request_close(&mut self) {
        self.request(PendingAction::Close);
    }

    fn request_target(&mut self, target: OpenTarget) {
        self.request(PendingAction::OpenTarget(target));
    }

    fn request(&mut self, action: PendingAction) {
        if self.buffer.as_ref().is_some_and(|buffer| buffer.dirty) {
            self.pending = Some(action);
        } else {
            self.perform(action);
        }
    }

    fn perform(&mut self, action: PendingAction) {
        match action {
            PendingAction::Open(path) => match load_buffer(&path) {
                Ok(buffer) => {
                    self.tree.select(Some(path));
                    self.buffer = Some(buffer);
                    self.editor_surface = EditorSurface::default();
                    self.highlight_cache.valid = false;
                    self.find_match_revision = u64::MAX;
                    self.scroll_to_find_match = self.find_open;
                    self.bracket_pair = None;
                    self.focus_editor = true;
                    self.tree_focused = false;
                }
                Err(error) => self.show_error(error),
            },
            PendingAction::OpenTarget(target) => match Self::new(target) {
                Ok(editor) => *self = editor,
                Err(error) => self.show_error(error),
            },
            PendingAction::Close => self.should_close = true,
        }
    }

    fn save(&mut self, destination: Option<PathBuf>) -> bool {
        let Some(buffer) = self.buffer.as_mut() else {
            return true;
        };
        let save_as = destination.is_some();
        let path = destination.unwrap_or_else(|| buffer.path.clone());
        match safe_save(buffer, &path) {
            Ok(()) => true,
            Err(SaveError::Conflict) => {
                if save_as {
                    self.show_error(format!(
                        "cannot save as {} because it already exists or changed",
                        path.display()
                    ));
                } else {
                    self.conflict = true;
                }
                false
            }
            Err(error) => {
                self.show_error(format!("cannot save {}: {error}", path.display()));
                false
            }
        }
    }

    fn finish_pending(&mut self) {
        if let Some(action) = self.pending.take() {
            self.perform(action);
        }
    }

    fn show_error(&mut self, error: String) {
        eprintln!("editur: {error}");
        self.error = Some(error);
    }

    pub fn ui(&mut self, root: &mut egui::Ui) {
        let ctx = root.ctx().clone();
        self.shortcuts(&ctx);
        if self.find_open {
            self.refresh_find_matches();
        }
        if self.search_open {
            self.search.poll(&self.search_query);
            ctx.request_repaint_after(Duration::from_millis(16));
        }

        let mut content = root.max_rect();
        #[cfg(target_os = "macos")]
        {
            let titlebar = content.with_max_y(content.top() + 34.0);
            self.draw_titlebar(root, titlebar);
            content.min.y = titlebar.bottom();
        }
        let status = content.with_min_y((content.bottom() - 25.0).max(content.top()));
        content.max.y = status.top();
        self.draw_statusbar(root, status);

        if self.sidebar {
            self.sidebar_width = self.sidebar_width.clamp(140.0, content.width().min(500.0));
            let sidebar = content.with_max_x(content.left() + self.sidebar_width);
            let divider = egui::Rect::from_center_size(
                egui::pos2(sidebar.right(), sidebar.center().y),
                egui::vec2(5.0, sidebar.height()),
            );
            let resize = root.interact(divider, Id::new("sidebar_resize"), Sense::drag());
            if resize.dragged() {
                self.sidebar_width =
                    (self.sidebar_width + resize.drag_delta().x).clamp(140.0, 500.0);
                ctx.request_repaint();
            }
            root.scope_builder(
                UiBuilder::new().id_salt("sidebar").max_rect(sidebar),
                |ui| self.draw_sidebar(ui),
            );
            root.painter().line_segment(
                [sidebar.right_top(), sidebar.right_bottom()],
                egui::Stroke::new(1.0, Color32::from_rgb(53, 55, 64)),
            );
            content.min.x = sidebar.right() + 1.0;
        }
        root.scope_builder(
            UiBuilder::new().id_salt("editor_surface").max_rect(content),
            |ui| self.draw_editor(ui),
        );
        self.draw_search(&ctx);
        self.draw_dialogs(&ctx);
        self.draw_error(&ctx);
    }

    #[cfg(target_os = "macos")]
    fn draw_titlebar(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let pointer = ui.ctx().pointer_hover_pos();
        let button_centers =
            [17.0, 37.0, 57.0].map(|x| egui::pos2(rect.left() + x, rect.center().y));
        let hovered = button_centers
            .iter()
            .position(|center| pointer.is_some_and(|pointer| pointer.distance(*center) <= 10.0));
        crate::renderer::mark_retained(
            ui.painter(),
            rect,
            0x6000_0000_0000_0000,
            u64::from(rect.width().to_bits()) ^ ((hovered.unwrap_or(3) as u64) << 32),
        );
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(27, 27, 36));
        let drag = ui.interact(rect, Id::new("titlebar_drag"), Sense::click_and_drag());
        if drag.drag_started() {
            self.window_action = Some(WindowAction::Drag);
        } else if drag.double_clicked() {
            self.window_action = Some(WindowAction::ToggleMaximize);
        }
        let actions = [
            (WindowAction::Close, Color32::from_rgb(255, 95, 87), "×"),
            (WindowAction::Minimize, Color32::from_rgb(254, 188, 46), "−"),
            (
                WindowAction::ToggleMaximize,
                Color32::from_rgb(40, 200, 64),
                "+",
            ),
        ];
        for (index, ((action, color, symbol), center)) in
            actions.into_iter().zip(button_centers).enumerate()
        {
            let button = egui::Rect::from_center_size(center, egui::vec2(18.0, 24.0));
            if ui
                .interact(button, Id::new(("titlebar_button", index)), Sense::click())
                .clicked()
            {
                self.window_action = Some(action);
            }
            ui.painter().circle_filled(center, 6.0, color);
            if hovered == Some(index) {
                ui.painter().text(
                    center,
                    Align2::CENTER_CENTER,
                    symbol,
                    FontId::proportional(10.0),
                    Color32::from_black_alpha(150),
                );
            }
        }
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            "Editur",
            FontId::proportional(14.0),
            Color32::from_rgb(188, 188, 198),
        );
    }

    fn draw_statusbar(&self, ui: &egui::Ui, rect: egui::Rect) {
        let painter = ui.painter_at(rect);
        let mut hasher = DefaultHasher::new();
        self.cursor.hash(&mut hasher);
        let status = self.buffer.as_ref().map(|buffer| {
            let syntax = self
                .syntaxes
                .detect(&buffer.path, buffer.large_file_warning)
                .name
                .clone();
            (buffer.path.display().to_string(), buffer.dirty, syntax)
        });
        status.hash(&mut hasher);
        crate::renderer::mark_retained(
            &painter,
            rect,
            0x7000_0000_0000_0000,
            hasher.finish() ^ u64::from(rect.width().to_bits()),
        );
        painter.rect_filled(rect, 0.0, Color32::from_rgb(24, 25, 30));
        painter.line_segment(
            [rect.left_top(), rect.right_top()],
            egui::Stroke::new(1.0, Color32::from_rgb(53, 55, 64)),
        );
        let color = Color32::from_rgb(151, 155, 166);
        let font = FontId::proportional(12.0);
        let mut x = rect.left() + 8.0;
        if let Some((path, dirty, syntax)) = status {
            for label in [
                path,
                if dirty { "Modified" } else { "Saved" }.into(),
                syntax,
            ] {
                let painted = painter.text(
                    egui::pos2(x, rect.center().y),
                    Align2::LEFT_CENTER,
                    label,
                    font.clone(),
                    color,
                );
                x = painted.right() + 9.0;
                painter.line_segment(
                    [
                        egui::pos2(x - 4.5, rect.top() + 6.0),
                        egui::pos2(x - 4.5, rect.bottom() - 6.0),
                    ],
                    egui::Stroke::new(1.0, Color32::from_rgb(60, 62, 70)),
                );
            }
            painter.text(
                egui::pos2(rect.right() - 8.0, rect.center().y),
                Align2::RIGHT_CENTER,
                format!("Ln {}, Col {}", self.cursor.0, self.cursor.1),
                font,
                color,
            );
        } else {
            painter.text(
                egui::pos2(x, rect.center().y),
                Align2::LEFT_CENTER,
                self.tree.root.display().to_string(),
                font,
                color,
            );
        }
    }

    fn draw_error(&mut self, ctx: &egui::Context) {
        let Some(error) = self.error.clone() else {
            return;
        };
        egui::Window::new("Error")
            .id(Id::new("error_banner"))
            .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 42.0))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.colored_label(Color32::from_rgb(255, 125, 125), error);
                if ui.button("Dismiss").clicked() {
                    self.error = None;
                }
            });
    }

    fn take_window_action(&mut self) -> Option<WindowAction> {
        self.window_action.take()
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (save, project_search, find, sidebar, tree, editor, close) = ctx.input(|input| {
            let command = input.modifiers.command;
            (
                command && input.key_pressed(Key::S),
                command && input.modifiers.shift && input.key_pressed(Key::F),
                command && !input.modifiers.shift && input.key_pressed(Key::F),
                command && input.key_pressed(Key::B),
                command && input.key_pressed(Key::Num1),
                command && input.key_pressed(Key::Num2),
                command && input.key_pressed(Key::W),
            )
        });
        if save {
            self.save(None);
        }
        if project_search {
            self.search_open = true;
            self.find_open = false;
            self.focus_search = true;
            self.focus_editor = false;
            self.tree_focused = false;
            ctx.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
        }
        if find {
            self.find_open = true;
            self.search_open = false;
            self.sidebar = true;
            self.focus_find = true;
            self.scroll_to_find_match = !self.find_matches.is_empty();
            self.focus_editor = false;
            self.tree_focused = false;
            ctx.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
        }
        if sidebar {
            self.sidebar = !self.sidebar;
        }
        if tree {
            self.sidebar = true;
            self.focus_editor = false;
            self.tree_focused = true;
            ctx.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
        }
        if editor {
            self.focus_editor = true;
            self.tree_focused = false;
        }
        if close {
            self.request_close();
        }
    }

    fn refresh_find_matches(&mut self) {
        let Some(buffer) = self.buffer.as_ref() else {
            self.find_matches.clear();
            self.find_selected = 0;
            return;
        };
        if self.find_match_revision == buffer.revision && self.find_match_query == self.find_query {
            return;
        }
        self.find_matches = match_spans(&buffer.text, &self.find_query);
        self.find_match_revision = buffer.revision;
        self.find_match_query.clone_from(&self.find_query);
        self.find_selected = self
            .find_selected
            .min(self.find_matches.len().saturating_sub(1));
        self.highlight_cache.find_valid = false;
    }

    fn draw_sidebar(&mut self, ui: &mut egui::Ui) {
        if self.find_open {
            self.draw_find(ui);
            ui.separator();
        }
        self.draw_tree(ui);
    }

    fn draw_find(&mut self, ui: &mut egui::Ui) {
        if !self.find_open {
            return;
        }
        let ctx = ui.ctx().clone();
        let query_focused = ctx.memory(|memory| memory.has_focus(Id::new("file_search_query")));
        let (enter, backwards, mut close) = ctx.input(|input| {
            (
                query_focused && input.key_pressed(Key::Enter),
                input.modifiers.shift,
                input.key_pressed(Key::Escape),
            )
        });
        let mut query_changed = false;
        let mut previous = false;
        let mut next = false;
        let count = if self.find_matches.is_empty() {
            "0 / 0".to_owned()
        } else {
            format!("{} / {}", self.find_selected + 1, self.find_matches.len())
        };
        egui::Frame::new()
            .fill(Color32::from_rgb(24, 25, 30))
            .inner_margin(egui::Margin::symmetric(8, 8))
            .corner_radius(8)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Find in file").strong());
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        close |= close_icon_button(ui).clicked();
                    });
                });
                ui.add_space(4.0);
                let response = egui::Frame::new()
                    .fill(Color32::from_rgb(33, 35, 42))
                    .inner_margin(egui::Margin::symmetric(7, 4))
                    .corner_radius(6)
                    .show(ui, |ui| {
                        ui.add_sized(
                            egui::vec2(ui.available_width(), 24.0),
                            TextEdit::singleline(&mut self.find_query)
                                .id(Id::new("file_search_query"))
                                .hint_text("Search current file…")
                                .frame(egui::Frame::NONE),
                        )
                    })
                    .inner;
                if self.focus_find {
                    response.request_focus();
                    self.focus_find = false;
                }
                query_changed = response.changed();
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.add(Label::new(RichText::new(&count).small().weak()));
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        next = chevron_icon_button(ui, false, "Next match (Enter)").clicked();
                        previous =
                            chevron_icon_button(ui, true, "Previous match (Shift+Enter)").clicked();
                    });
                });
            });
        if query_changed {
            self.find_selected = 0;
            self.find_match_revision = u64::MAX;
            self.refresh_find_matches();
            self.scroll_to_find_match = !self.find_matches.is_empty();
            ctx.request_repaint();
        } else if (enter || previous || next) && !self.find_matches.is_empty() {
            self.find_selected = next_find_match(
                self.find_selected,
                self.find_matches.len(),
                previous || (enter && backwards),
            );
            self.highlight_cache.find_valid = false;
            self.scroll_to_find_match = true;
            ctx.request_repaint();
        }
        if close {
            self.find_open = false;
            self.focus_editor = self.buffer.is_some();
        }
    }

    fn draw_search(&mut self, ctx: &egui::Context) {
        if !self.search_open {
            return;
        }
        let results = self.search.results();
        let hit_count = results.files.len() + results.contents.len();
        if hit_count == 0 {
            self.search_selected = 0;
        } else {
            self.search_selected = self.search_selected.min(hit_count - 1);
        }

        let (down, up, enter, escape) = ctx.input(|input| {
            (
                input.key_pressed(Key::ArrowDown),
                input.key_pressed(Key::ArrowUp),
                input.key_pressed(Key::Enter),
                input.key_pressed(Key::Escape),
            )
        });
        let (selection, scroll_to_selection) =
            search_selection_after_navigation(self.search_selected, hit_count, down, up);
        self.search_selected = selection;

        let mut query_changed = false;
        let mut selected_path = enter
            .then(|| search_hit(results, self.search_selected).map(|hit| hit.path.clone()))
            .flatten();
        let empty_query = self.search_query.trim().is_empty();
        let palette_frame = egui::Frame::window(&ctx.style_of(ctx.theme()))
            .fill(Color32::from_rgb(24, 25, 30))
            .stroke(egui::Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 22),
            ))
            .inner_margin(14)
            .corner_radius(12)
            .shadow(egui::Shadow {
                offset: [0, 8],
                blur: 28,
                spread: 2,
                color: Color32::from_black_alpha(150),
            });
        egui::Window::new("Project search")
            .id(Id::new("project_search"))
            .title_bar(false)
            .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 48.0))
            .fixed_size(egui::vec2(680.0, if empty_query { 185.0 } else { 430.0 }))
            .resizable(false)
            .collapsible(false)
            .frame(palette_frame)
            .show(ctx, |ui| {
                ui.visuals_mut().selection.bg_fill = Color32::from_rgb(30, 83, 94);
                ui.visuals_mut().selection.stroke.color = Color32::from_rgb(126, 228, 239);
                ui.visuals_mut().widgets.hovered.weak_bg_fill = Color32::from_rgb(35, 37, 44);
                ui.visuals_mut().widgets.hovered.bg_fill = Color32::from_rgb(35, 37, 44);
                ui.spacing_mut().item_spacing.y = 4.0;

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Search project").size(15.0).strong());
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("Up/Down navigate   Enter open   Esc close")
                                .small()
                                .weak(),
                        );
                    });
                });
                ui.add_space(8.0);
                let response = egui::Frame::new()
                    .fill(Color32::from_rgb(33, 35, 42))
                    .inner_margin(egui::Margin::symmetric(10, 7))
                    .corner_radius(7)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                egui::vec2(ui.available_width(), 26.0),
                                TextEdit::singleline(&mut self.search_query)
                                    .id(Id::new("project_search_query"))
                                    .hint_text("Search files and contents…")
                                    .desired_width(f32::INFINITY)
                                    .frame(egui::Frame::NONE),
                            )
                        })
                        .inner
                    })
                    .inner;
                if self.focus_search {
                    response.request_focus();
                    self.focus_search = false;
                }
                query_changed = response.changed();
                ui.add_space(8.0);

                ScrollArea::vertical()
                    .id_salt("project_search_results")
                    .auto_shrink([false, false])
                    .max_height(if empty_query { 54.0 } else { 300.0 })
                    .show(ui, |ui| {
                        if self.search_query.trim().is_empty() {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new("Find anything in this project").strong());
                                ui.label(
                                    RichText::new("Type a filename or text from a file")
                                        .small()
                                        .weak(),
                                );
                            });
                            return;
                        }
                        if results.query != self.search_query.trim() {
                            ui.label(RichText::new("Searching…").weak());
                            return;
                        }

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("FILES")
                                    .size(11.0)
                                    .strong()
                                    .color(Color32::from_rgb(145, 153, 169)),
                            );
                            ui.label(
                                RichText::new(results.files.len().to_string())
                                    .small()
                                    .weak(),
                            );
                        });
                        for (index, hit) in results.files.iter().enumerate() {
                            let mut job = LayoutJob::default();
                            job.wrap.max_width = ui.available_width() - 18.0;
                            job.wrap.break_anywhere = true;
                            job.append(
                                &format!("  {}", hit.relative),
                                0.0,
                                TextFormat {
                                    font_id: FontId::monospace(13.5),
                                    color: Color32::from_rgb(205, 210, 220),
                                    ..TextFormat::default()
                                },
                            );
                            let response =
                                search_result_row(ui, self.search_selected == index, job, 30.0);
                            if scroll_to_selection && self.search_selected == index {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                selected_path = Some(hit.path.clone());
                            }
                        }

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("FILE CONTENT")
                                    .size(11.0)
                                    .strong()
                                    .color(Color32::from_rgb(145, 153, 169)),
                            );
                            ui.label(
                                RichText::new(results.contents.len().to_string())
                                    .small()
                                    .weak(),
                            );
                        });
                        for (offset, hit) in results.contents.iter().enumerate() {
                            let index = results.files.len() + offset;
                            let response = search_result_row(
                                ui,
                                self.search_selected == index,
                                content_result_job(
                                    hit,
                                    self.search_query.trim(),
                                    ui.available_width() - 18.0,
                                ),
                                44.0,
                            );
                            if scroll_to_selection && self.search_selected == index {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                selected_path = Some(hit.path.clone());
                            }
                        }
                        if hit_count == 0 && results.complete {
                            ui.label(RichText::new("No matches").weak());
                        }
                    });
                ui.add_space(4.0);
                let status = if results.complete {
                    format!("{} files indexed", results.indexed_files)
                } else {
                    format!("Indexing… {} files ready", results.indexed_files)
                };
                ui.label(RichText::new(status).small().weak());
            });

        if query_changed {
            self.search_selected = 0;
            if let Err(error) = self.search.set_query(&self.search_query) {
                self.show_error(error);
            }
        }
        if escape {
            self.search_open = false;
            self.focus_editor = self.buffer.is_some();
        } else if let Some(path) = selected_path {
            self.search_open = false;
            self.request(PendingAction::Open(path));
        }
    }

    fn draw_tree(&mut self, ui: &mut egui::Ui) {
        let scroll_to_selected =
            self.tree_focused && self.pending.is_none() && self.tree_keyboard(ui);
        let output = self.tree_surface.show(
            ui,
            &self.tree.visible,
            self.tree.selected_index,
            scroll_to_selected,
        );
        if output.response.clicked() {
            self.tree_focused = true;
        }
        if let Some(index) = output.clicked.filter(|_| self.pending.is_none()) {
            let entry = self.tree.visible[index].entry.clone();
            self.tree_focused = true;
            self.focus_editor = false;
            ui.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
            self.tree.select(Some(entry.path.clone()));
            if entry.is_dir {
                if let Err(error) = self.tree.toggle(&entry.path) {
                    self.show_error(error);
                }
            } else {
                self.request(PendingAction::Open(entry.path));
            }
        }
    }

    fn tree_keyboard(&mut self, ui: &egui::Ui) -> bool {
        let entry_count = self.tree.visible.len();
        if entry_count == 0 {
            return false;
        }
        let (down, up, right, left, enter) = ui.input(|input| {
            (
                input.key_pressed(Key::ArrowDown),
                input.key_pressed(Key::ArrowUp),
                input.key_pressed(Key::ArrowRight),
                input.key_pressed(Key::ArrowLeft),
                input.key_pressed(Key::Enter),
            )
        });
        if !(down || up || right || left || enter) {
            return false;
        }
        let current = self.tree.selected_index.unwrap_or(0).min(entry_count - 1);
        let next = if down {
            Some((current + 1).min(entry_count - 1))
        } else if up {
            Some(current.saturating_sub(1))
        } else {
            None
        };
        if let Some(next) = next {
            let path = self.tree.visible[next].entry.path.clone();
            self.tree.select(Some(path));
        }
        let entry = self.tree.visible[next.unwrap_or(current)].entry.clone();
        if right && entry.is_dir {
            if !self.tree.expanded.contains(&entry.path)
                && let Err(error) = self.tree.toggle(&entry.path)
            {
                self.show_error(error);
            }
        } else if left && entry.is_dir {
            self.tree.collapse(&entry.path);
        } else if enter {
            if entry.is_dir {
                if let Err(error) = self.tree.toggle(&entry.path) {
                    self.show_error(error);
                }
            } else {
                self.request(PendingAction::Open(entry.path.clone()));
            }
        }
        next.is_some()
    }

    fn draw_editor(&mut self, ui: &mut egui::Ui) {
        let find_open = self.find_open;
        let find_query = &self.find_query;
        let find_matches = &self.find_matches;
        let find_selected = self.find_selected;
        let bracket_pair = self.bracket_pair.clone();
        let scroll_character = self
            .scroll_to_find_match
            .then(|| find_matches.get(find_selected).cloned())
            .flatten()
            .map(|span| {
                self.buffer
                    .as_ref()
                    .map_or(0, |buffer| buffer.text[..span.start].chars().count())
            });
        let Some(buffer) = self.buffer.as_mut() else {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, EDITOR_BACKGROUND);
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Select a file to begin editing").weak());
            });
            return;
        };
        if buffer.large_file_warning {
            ui.colored_label(
                Color32::YELLOW,
                "Large file: syntax highlighting is disabled above 5 MiB.",
            );
        }

        let syntax = self
            .syntaxes
            .detect(&buffer.path, buffer.large_file_warning);
        let revision = buffer.revision;
        let syntax_name = syntax.name.clone();
        let cache = &mut self.highlight_cache;
        let highlighter = &self.highlighter;
        let syntaxes = &self.syntaxes;
        let large_file = buffer.large_file_warning;
        let mut highlight_error = None;
        let wrap_width = ui.available_width().max(1.0);
        if !cache.valid || cache.revision != revision || cache.syntax != syntax_name {
            if large_file {
                cache.job = plain_text_job(&buffer.text, wrap_width);
                cache.incremental = IncrementalHighlightCache::default();
            } else {
                match highlighter.highlight_job_incremental(
                    &buffer.text,
                    syntax,
                    syntaxes.set(),
                    wrap_width,
                    &mut cache.incremental,
                ) {
                    Ok(job) => cache.job = job,
                    Err(error) => {
                        highlight_error = Some(error);
                        cache.job = plain_text_job(&buffer.text, wrap_width);
                    }
                }
            }
            cache.revision = revision;
            cache.syntax.clone_from(&syntax_name);
            cache.valid = true;
            cache.find_valid = false;
        }
        let galley_key = GalleyKey {
            revision,
            syntax: syntax_name,
            wrap_width: wrap_width.round().to_bits(),
            find: (find_open && !find_matches.is_empty())
                .then(|| (find_query.clone(), find_selected)),
            bracket_pair: bracket_pair.clone(),
        };
        if cache.galley_key.as_ref() != Some(&galley_key) {
            cache.galley_key = Some(galley_key);
            cache.presentation_revision = cache.presentation_revision.wrapping_add(1);
        }
        let mut job = if find_open && !find_matches.is_empty() {
            if !cache.find_valid
                || cache.find_revision != revision
                || cache.find_query != *find_query
                || cache.find_selected != find_selected
            {
                cache.find_job = find_highlighted_job(&cache.job, find_matches, find_selected);
                cache.find_revision = revision;
                cache.find_query.clone_from(find_query);
                cache.find_selected = find_selected;
                cache.find_valid = true;
            }
            cache.find_job.clone()
        } else {
            cache.job.clone()
        };
        if let Some(pair) = &bracket_pair {
            job = bracket_highlighted_job(&job, pair);
        }
        let output = self.editor_surface.show(
            ui,
            &mut buffer.text,
            &job,
            cache.presentation_revision,
            self.focus_editor,
            scroll_character,
        );
        if self.scroll_to_find_match {
            self.scroll_to_find_match = false;
        }
        self.focus_editor = false;
        if output.response.has_focus() {
            self.tree_focused = false;
        }
        if output.changed {
            buffer.mark_changed();
            self.highlight_cache.valid = false;
        }
        self.cursor = buffer.line_column(output.cursor);
        let pair = (!buffer.large_file_warning)
            .then(|| match_bracket_pair(&buffer.text, output.cursor))
            .flatten();
        if self.bracket_pair != pair {
            self.bracket_pair = pair;
            ui.ctx().request_repaint();
        }
        if let Some(error) = highlight_error {
            self.show_error(error);
        }
    }

    fn draw_dialogs(&mut self, ctx: &egui::Context) {
        if self.pending.is_some() && !self.conflict && self.save_as.is_none() {
            egui::Window::new("Unsaved changes")
                .id(Id::new("unsaved_dialog"))
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Save changes before continuing?");
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() && self.save(None) {
                            self.finish_pending();
                        }
                        if ui.button("Discard").clicked() {
                            self.finish_pending();
                        }
                        if ui.button("Cancel").clicked() {
                            self.pending = None;
                            self.tree
                                .select(self.buffer.as_ref().map(|buffer| buffer.path.clone()));
                        }
                    });
                });
        }
        if self.conflict {
            egui::Window::new("File changed on disk")
                .id(Id::new("conflict_dialog"))
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("The file changed outside Editur. It was not overwritten.");
                    ui.horizontal(|ui| {
                        if ui.button("Reload").clicked()
                            && let Some(path) =
                                self.buffer.as_ref().map(|buffer| buffer.path.clone())
                        {
                            match load_buffer(&path) {
                                Ok(buffer) => {
                                    self.buffer = Some(buffer);
                                    self.editor_surface = EditorSurface::default();
                                    self.highlight_cache.valid = false;
                                    self.conflict = false;
                                    if self.pending.is_some() {
                                        self.finish_pending();
                                    }
                                }
                                Err(error) => self.show_error(error),
                            }
                        }
                        if ui.button("Save As…").clicked() {
                            let suggestion =
                                self.buffer.as_ref().map_or_else(String::new, |buffer| {
                                    format!("{}.editur-copy", buffer.path.display())
                                });
                            self.save_as = Some(suggestion);
                            self.conflict = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.conflict = false;
                            self.pending = None;
                        }
                    });
                });
        }
        if self.save_as.is_some() {
            let mut save_clicked = false;
            let mut cancel_clicked = false;
            egui::Window::new("Save As")
                .id(Id::new("save_as_dialog"))
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Destination path");
                    if let Some(path) = self.save_as.as_mut() {
                        ui.text_edit_singleline(path);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            save_clicked = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel_clicked = true;
                        }
                    });
                });
            if save_clicked {
                if let Some(path) = self.save_as.clone()
                    && self.save(Some(PathBuf::from(path)))
                {
                    self.save_as = None;
                    self.finish_pending();
                }
            } else if cancel_clicked {
                self.save_as = None;
                self.conflict = true;
            }
        }
    }
}

pub fn launch(target: OpenTarget) -> Result<(), String> {
    if open_running(&target)? {
        return Ok(());
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate the Editur executable: {error}"))?;
    let path = target.file.as_ref().unwrap_or(&target.root);
    let mut command = Command::new(executable);
    command
        .arg("--resident")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot start the editor resident: {error}"))
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

pub fn run(target: OpenTarget, started: Instant) -> Result<(), String> {
    let listener = match claim(&target)? {
        Claim::Primary(listener) => listener,
        Claim::Forwarded => return Ok(()),
    };
    let event_loop = EventLoop::<InstanceEvent>::with_user_event()
        .build()
        .map_err(|error| format!("cannot create event loop: {error}"))?;
    spawn_listener(listener, event_loop.create_proxy())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let editor_started = Instant::now();
    let editor = EditorApp::new(target)?;
    if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
        eprintln!(
            "editur: editor state initialized in {:.2?}",
            editor_started.elapsed()
        );
    }
    let mut shell = Shell::new(editor, started);
    event_loop
        .run_app(&mut shell)
        .map_err(|error| format!("window event loop failed: {error}"))?;
    shell.fatal.map_or(Ok(()), Err)
}

struct Shell {
    editor: EditorApp,
    window: Option<Window>,
    renderer: Option<Renderer>,
    egui: Option<egui_winit::State>,
    repaint_at: Option<Instant>,
    fatal: Option<String>,
    clipboard: Option<arboard::Clipboard>,
    modifiers: ModifiersState,
    started: Instant,
    first_frame_logged: bool,
}

fn system_clipboard(
    clipboard: &mut Option<arboard::Clipboard>,
) -> Result<&mut arboard::Clipboard, String> {
    if clipboard.is_none() {
        *clipboard = Some(
            arboard::Clipboard::new()
                .map_err(|error| format!("system clipboard is unavailable: {error}"))?,
        );
    }
    Ok(clipboard.as_mut().expect("clipboard was initialized"))
}

impl Shell {
    fn new(editor: EditorApp, started: Instant) -> Self {
        Self {
            editor,
            window: None,
            renderer: None,
            egui: None,
            repaint_at: None,
            fatal: None,
            clipboard: None,
            modifiers: ModifiersState::default(),
            started,
            first_frame_logged: false,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        self.fatal = Some(error);
        event_loop.exit();
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer), Some(state)) = (
            self.window.as_ref(),
            self.renderer.as_mut(),
            self.egui.as_mut(),
        ) else {
            return;
        };
        let input = state.take_egui_input(window);
        let context = state.egui_ctx().clone();
        let output = context.run_ui(input, |root| self.editor.ui(root));
        if let Some(action) = self.editor.take_window_action() {
            match action {
                WindowAction::Close => self.editor.request_close(),
                WindowAction::Minimize => window.set_minimized(true),
                WindowAction::ToggleMaximize => window.set_maximized(!window.is_maximized()),
                WindowAction::Drag => {
                    if let Err(error) = window.drag_window() {
                        self.editor
                            .show_error(format!("cannot drag window: {error}"));
                    }
                }
            }
        }
        for command in &output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command
                && let Err(error) = system_clipboard(&mut self.clipboard).and_then(|clipboard| {
                    clipboard.set_text(text).map_err(|error| error.to_string())
                })
            {
                self.editor
                    .show_error(format!("cannot copy to system clipboard: {error}"));
            }
        }
        state.handle_platform_output_with_event_loop(window, event_loop, output.platform_output);
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        #[cfg(target_os = "linux")]
        window.pre_present_notify();
        if let Err(error) =
            renderer.render(output.pixels_per_point, &primitives, &output.textures_delta)
        {
            self.fail(event_loop, error);
            return;
        }
        if !self.first_frame_logged {
            window.focus_window();
            if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
                eprintln!(
                    "editur: first editable frame in {:.2?}",
                    self.started.elapsed()
                );
            }
            self.first_frame_logged = true;
            #[cfg(target_os = "macos")]
            set_macos_application_icon();
        }
        if self.editor.should_close {
            event_loop.exit();
            return;
        }
        let delay = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .map_or(Duration::MAX, |output| output.repaint_delay);
        if delay.is_zero() {
            window.request_redraw();
        } else if delay == Duration::MAX {
            self.repaint_at = None;
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            let repaint_at = Instant::now() + delay;
            self.repaint_at = Some(repaint_at);
            event_loop.set_control_flow(ControlFlow::WaitUntil(repaint_at));
        }
    }
}

impl ApplicationHandler<InstanceEvent> for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let icon = match Icon::from_rgba(
            include_bytes!("../assets/icons/editur-64.rgba").to_vec(),
            64,
            64,
        ) {
            Ok(icon) => icon,
            Err(error) => {
                self.fail(event_loop, format!("cannot load application icon: {error}"));
                return;
            }
        };
        let attributes = Window::default_attributes()
            .with_title("Editur")
            .with_inner_size(LogicalSize::new(1000, 700))
            .with_min_inner_size(LogicalSize::new(520, 320))
            .with_window_icon(Some(icon));
        #[cfg(target_os = "macos")]
        let attributes = attributes.with_decorations(false).with_transparent(true);
        let window_started = Instant::now();
        #[cfg(target_os = "macos")]
        let window = create_macos_window_without_native_title(event_loop, attributes);
        #[cfg(not(target_os = "macos"))]
        let window = event_loop.create_window(attributes);
        let window = match window {
            Ok(window) => window,
            Err(error) => {
                self.fail(event_loop, format!("cannot create window: {error}"));
                return;
            }
        };
        let window_time = window_started.elapsed();
        let renderer_started = Instant::now();
        let renderer = match Renderer::new(&window) {
            Ok(renderer) => renderer,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        let renderer_time = renderer_started.elapsed();
        if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
            eprintln!(
                "editur: {} adapter: {}",
                renderer.backend_name(),
                renderer.adapter_name()
            );
            eprintln!("editur: window created in {window_time:.2?}");
            eprintln!("editur: renderer initialized in {renderer_time:.2?}");
        }
        let context = egui::Context::default();
        let state = egui_winit::State::new(
            context,
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        #[cfg(target_os = "macos")]
        {
            activate_macos_application();
            window.focus_window();
        }
        self.egui = Some(state);
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.redraw(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if window.id() != window_id {
            return;
        }
        if let WindowEvent::ModifiersChanged(modifiers) = &event {
            self.modifiers = modifiers.state();
        }
        if let WindowEvent::KeyboardInput { event: key, .. } = &event
            && key.state.is_pressed()
            && key.physical_key == PhysicalKey::Code(KeyCode::KeyV)
            && (self.modifiers.control_key() || self.modifiers.super_key())
        {
            match system_clipboard(&mut self.clipboard)
                .and_then(|clipboard| clipboard.get_text().map_err(|error| error.to_string()))
            {
                Ok(text) => {
                    if let Some(state) = &mut self.egui {
                        state.set_clipboard_text(text);
                    }
                }
                Err(error) => self
                    .editor
                    .show_error(format!("cannot paste from system clipboard: {error}")),
            }
        }
        if !matches!(event, WindowEvent::RedrawRequested)
            && self
                .egui
                .as_mut()
                .is_some_and(|state| state.on_window_event(window, &event).repaint)
        {
            window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => {
                self.editor.request_close();
                if self.editor.should_close {
                    event_loop.exit();
                } else {
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size);
                }
                self.redraw(event_loop);
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if self
            .repaint_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.repaint_at = None;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: InstanceEvent) {
        match event {
            InstanceEvent::Open(target, reply) => {
                self.editor.request_target(target);
                if let Some(window) = &self.window {
                    window.set_visible(true);
                    #[cfg(target_os = "macos")]
                    activate_macos_application();
                    window.focus_window();
                    window.request_redraw();
                }
                let _ = reply.send(true);
            }
            InstanceEvent::Quit(reply) => {
                let clean = self
                    .editor
                    .buffer
                    .as_ref()
                    .is_none_or(|buffer| !buffer.dirty);
                let _ = reply.send(clean);
                if clean {
                    event_loop.exit();
                } else {
                    self.editor
                        .show_error("Save or discard changes before updating Editur.".into());
                    if let Some(window) = &self.window {
                        window.set_visible(true);
                        window.focus_window();
                        window.request_redraw();
                    }
                }
            }
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn create_macos_window_without_native_title(
    event_loop: &ActiveEventLoop,
    attributes: winit::window::WindowAttributes,
) -> Result<Window, winit::error::OsError> {
    use objc::{
        class,
        runtime::{self, Imp, Method, Object, Sel},
        sel, sel_impl,
    };

    unsafe extern "C" fn ignore_title(_: *mut Object, _: Sel, _: *mut Object) {}

    struct RestoreMethod {
        method: *mut Method,
        implementation: Imp,
    }

    impl Drop for RestoreMethod {
        fn drop(&mut self) {
            unsafe {
                runtime::method_setImplementation(self.method, self.implementation);
            }
        }
    }

    unsafe {
        // macOS 26 blocks for roughly two seconds when winit sets a title on a borderless window.
        // Editur draws its own titlebar, so suppress only that synchronous creation-time call.
        let method = runtime::class_getInstanceMethod(class!(NSWindow), sel!(setTitle:));
        if method.is_null() {
            return event_loop.create_window(attributes);
        }
        let replacement: Imp = std::mem::transmute(
            ignore_title as unsafe extern "C" fn(*mut Object, Sel, *mut Object),
        );
        let restore = RestoreMethod {
            method: method.cast_mut(),
            implementation: runtime::method_setImplementation(method.cast_mut(), replacement),
        };
        let window = event_loop.create_window(attributes);
        drop(restore);
        window
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn activate_macos_application() {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};

    unsafe {
        let application: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let modern: objc::runtime::BOOL =
            msg_send![application, respondsToSelector: sel!(activate)];
        if modern == objc::runtime::YES {
            let _: () = msg_send![application, activate];
        } else {
            let _: () = msg_send![application, activateIgnoringOtherApps: objc::runtime::YES];
        }
    }
}

#[cfg(target_os = "macos")]
fn macos_icon_path(executable: &Path) -> Option<PathBuf> {
    let macos = executable.parent()?;
    let contents = macos.parent()?;
    let bundle = contents.parent()?;
    (macos.file_name()? == "MacOS"
        && contents.file_name()? == "Contents"
        && bundle.extension()? == "app")
        .then(|| contents.join("Resources/Editur.icns"))
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn set_macos_application_icon() {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    use std::ffi::CString;

    let Some(path) = std::env::current_exe()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok())
        .and_then(|path| macos_icon_path(&path))
    else {
        return;
    };
    let Ok(path) = CString::new(path.to_string_lossy().as_bytes()) else {
        return;
    };
    unsafe {
        let path: *mut Object = msg_send![
            class!(NSString),
            stringWithUTF8String: path.as_ptr()
        ];
        let icon: *mut Object = msg_send![class!(NSImage), alloc];
        let icon: *mut Object = msg_send![icon, initWithContentsOfFile: path];
        if icon.is_null() {
            return;
        }
        let application: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![application, setApplicationIconImage: icon];
        let _: () = msg_send![icon, release];
    }
}

fn search_result_row(
    ui: &mut egui::Ui,
    selected: bool,
    job: LayoutJob,
    min_height: f32,
) -> egui::Response {
    let mut row = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(9, 6))
        .corner_radius(6)
        .begin(ui);
    row.content_ui
        .set_min_width(row.content_ui.available_width());
    row.content_ui.set_min_height(min_height);
    row.content_ui.add(Label::new(job).wrap());
    let response = row.allocate_space(ui).interact(Sense::click());
    row.frame.fill = if selected {
        Color32::from_rgb(30, 74, 84)
    } else if response.hovered() {
        Color32::from_rgb(35, 37, 44)
    } else {
        Color32::TRANSPARENT
    };
    row.paint(ui);
    response
}

fn search_hit(results: &SearchResults, index: usize) -> Option<&SearchHit> {
    results.files.get(index).or_else(|| {
        results
            .contents
            .get(index.saturating_sub(results.files.len()))
    })
}

fn chevron_icon_button(ui: &mut egui::Ui, upward: bool, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 5.0, Color32::from_white_alpha(14));
    }
    let center = rect.center();
    let direction = if upward { -1.0 } else { 1.0 };
    let tip = center + egui::vec2(0.0, 3.0 * direction);
    let stroke = egui::Stroke::new(1.5, Color32::from_rgb(174, 181, 197));
    ui.painter()
        .line_segment([center + egui::vec2(-4.0, -2.0 * direction), tip], stroke);
    ui.painter()
        .line_segment([tip, center + egui::vec2(4.0, -2.0 * direction)], stroke);
    response.on_hover_text(label)
}

fn close_icon_button(ui: &mut egui::Ui) -> egui::Response {
    let label = "Close (Esc)";
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 5.0, Color32::from_white_alpha(14));
    }
    let center = rect.center();
    let stroke = egui::Stroke::new(1.5, Color32::from_rgb(174, 181, 197));
    ui.painter().line_segment(
        [
            center + egui::vec2(-3.5, -3.5),
            center + egui::vec2(3.5, 3.5),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            center + egui::vec2(-3.5, 3.5),
            center + egui::vec2(3.5, -3.5),
        ],
        stroke,
    );
    response.on_hover_text(label)
}

fn plain_text_job(text: &str, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: FontId::monospace(14.0),
            color: Color32::LIGHT_GRAY,
            ..TextFormat::default()
        },
    );
    job
}

fn search_selection_after_navigation(
    selected: usize,
    hit_count: usize,
    down: bool,
    up: bool,
) -> (usize, bool) {
    if hit_count == 0 {
        return (0, false);
    }
    let next = if down {
        (selected + 1).min(hit_count - 1)
    } else if up {
        selected.saturating_sub(1)
    } else {
        selected.min(hit_count - 1)
    };
    (next, next != selected)
}

fn next_find_match(selected: usize, match_count: usize, backwards: bool) -> usize {
    if match_count == 0 {
        0
    } else if backwards {
        selected.checked_sub(1).unwrap_or(match_count - 1)
    } else {
        (selected + 1) % match_count
    }
}

fn match_bracket_pair(
    text: &str,
    cursor_character: usize,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let (byte, bracket) = cursor_character
        .checked_sub(1)
        .and_then(|index| text.char_indices().nth(index))
        .filter(|(_, character)| is_bracket(*character))
        .or_else(|| {
            text.char_indices()
                .nth(cursor_character)
                .filter(|(_, character)| is_bracket(*character))
        })?;
    let bracket_range = byte..byte + bracket.len_utf8();

    if is_opening_bracket(bracket) {
        let mut stack = vec![bracket];
        let rest = byte + bracket.len_utf8();
        for (offset, candidate) in text[rest..].char_indices() {
            let candidate_byte = rest + offset;
            if is_opening_bracket(candidate) {
                stack.push(candidate);
            } else if is_closing_bracket(candidate) {
                if matching_bracket(*stack.last()?) != candidate {
                    return None;
                }
                stack.pop();
                if stack.is_empty() {
                    return Some((
                        bracket_range,
                        candidate_byte..candidate_byte + candidate.len_utf8(),
                    ));
                }
            }
        }
    } else {
        let mut stack = vec![bracket];
        for (candidate_byte, candidate) in text[..byte].char_indices().rev() {
            if is_closing_bracket(candidate) {
                stack.push(candidate);
            } else if is_opening_bracket(candidate) {
                if matching_bracket(candidate) != *stack.last()? {
                    return None;
                }
                stack.pop();
                if stack.is_empty() {
                    return Some((
                        candidate_byte..candidate_byte + candidate.len_utf8(),
                        bracket_range,
                    ));
                }
            }
        }
    }
    None
}

const fn is_opening_bracket(character: char) -> bool {
    matches!(character, '(' | '[' | '{')
}

const fn is_closing_bracket(character: char) -> bool {
    matches!(character, ')' | ']' | '}')
}

const fn is_bracket(character: char) -> bool {
    is_opening_bracket(character) || is_closing_bracket(character)
}

const fn matching_bracket(character: char) -> char {
    match character {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => character,
    }
}

fn content_result_job(hit: &SearchHit, query: &str, wrap_width: f32) -> LayoutJob {
    let font_id = FontId::monospace(13.0);
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.append(
        &format!("  {}:{}\n", hit.relative, hit.line.unwrap_or(1)),
        0.0,
        TextFormat {
            font_id: font_id.clone(),
            color: Color32::from_rgb(163, 172, 190),
            ..TextFormat::default()
        },
    );
    let normal = TextFormat {
        font_id: font_id.clone(),
        color: Color32::from_rgb(205, 210, 220),
        ..TextFormat::default()
    };
    let highlighted = TextFormat {
        font_id,
        color: Color32::from_rgb(128, 232, 242),
        background: Color32::from_rgb(30, 79, 89),
        ..TextFormat::default()
    };
    let mut cursor = 0;
    for span in match_spans(&hit.preview, query) {
        job.append(&hit.preview[cursor..span.start], 0.0, normal.clone());
        job.append(&hit.preview[span.clone()], 0.0, highlighted.clone());
        cursor = span.end;
    }
    job.append(&hit.preview[cursor..], 0.0, normal);
    job
}

fn find_highlighted_job(
    base: &LayoutJob,
    matches: &[std::ops::Range<usize>],
    active: usize,
) -> LayoutJob {
    if matches.is_empty() {
        return base.clone();
    }
    let mut highlighted = base.clone();
    highlighted.text.clear();
    highlighted.sections.clear();
    let mut match_index = 0;
    for section in &base.sections {
        let section_start = section.byte_range.start.0;
        let section_end = section.byte_range.end.0;
        while match_index < matches.len() && matches[match_index].end <= section_start {
            match_index += 1;
        }
        let mut current_match = match_index;
        let mut cursor = section_start;
        let mut leading_space = section.leading_space;
        while current_match < matches.len() && matches[current_match].start < section_end {
            let start = matches[current_match].start.max(section_start);
            let end = matches[current_match].end.min(section_end);
            if cursor < start {
                highlighted.append(
                    &base.text[cursor..start],
                    leading_space,
                    section.format.clone(),
                );
                leading_space = 0.0;
            }
            if start < end {
                let mut format = section.format.clone();
                format.background = if current_match == active {
                    Color32::from_rgb(42, 117, 128)
                } else {
                    Color32::from_rgb(30, 79, 89)
                };
                highlighted.append(&base.text[start..end], leading_space, format);
                leading_space = 0.0;
                cursor = end;
            }
            if matches[current_match].end <= section_end {
                current_match += 1;
            } else {
                break;
            }
        }
        if cursor < section_end {
            highlighted.append(
                &base.text[cursor..section_end],
                leading_space,
                section.format.clone(),
            );
        }
        while match_index < matches.len() && matches[match_index].end <= section_end {
            match_index += 1;
        }
    }
    highlighted
}

fn bracket_highlighted_job(
    base: &LayoutJob,
    pair: &(std::ops::Range<usize>, std::ops::Range<usize>),
) -> LayoutJob {
    let spans = [pair.0.clone(), pair.1.clone()];
    let mut highlighted = find_highlighted_job(base, &spans, usize::MAX);
    for section in &mut highlighted.sections {
        let range = section.byte_range.start.0..section.byte_range.end.0;
        if spans
            .iter()
            .any(|span| range.start < span.end && span.start < range.end)
        {
            section.format.background = Color32::from_rgb(50, 57, 72);
            section.format.underline = egui::Stroke::new(1.0, Color32::from_rgb(86, 207, 225));
        }
    }
    highlighted
}

fn match_spans(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut text_lower = text.to_owned();
    text_lower.make_ascii_lowercase();
    let mut query_lower = query.to_owned();
    query_lower.make_ascii_lowercase();
    let mut spans = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = text_lower[cursor..].find(&query_lower) {
        let start = cursor + offset;
        let end = start + query_lower.len();
        spans.push(start..end);
        cursor = end;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::{
        EditorApp, TreeState, find_highlighted_job, match_bracket_pair, match_spans,
        next_find_match, plain_text_job, search_selection_after_navigation,
    };
    use crate::file_io::OpenTarget;
    use egui::{Event, Id, Key, Modifiers, RawInput, Rect, Vec2};
    use std::fs;

    #[test]
    fn file_tree_rebuilds_its_cached_rows_only_when_expansion_changes() {
        let temp = tempfile::tempdir().unwrap();
        let directory = temp.path().join("src");
        fs::create_dir(&directory).unwrap();
        fs::write(directory.join("main.rs"), "").unwrap();
        fs::write(temp.path().join("README.md"), "").unwrap();
        let mut tree = TreeState::new(temp.path().to_path_buf(), None).unwrap();

        assert_eq!(tree.visible.len(), 2);
        tree.toggle(&directory).unwrap();
        assert_eq!(tree.visible.len(), 3);
        tree.collapse(&directory);
        assert_eq!(tree.visible.len(), 2);
    }

    #[test]
    fn finds_every_case_insensitive_ascii_match_for_palette_highlighting() {
        assert_eq!(
            match_spans("Cargo cargo CARGO", "cargo"),
            [0..5, 6..11, 12..17]
        );
        assert!(match_spans("Cargo", "").is_empty());
    }

    #[test]
    fn large_file_plain_layout_preserves_text_without_syntax_sections() {
        let job = plain_text_job("one\ntwo", 320.0);

        assert_eq!((job.text, job.sections.len()), ("one\ntwo".into(), 1));
    }

    #[test]
    fn project_search_scrolls_only_when_keyboard_navigation_moves_selection() {
        assert_eq!(
            search_selection_after_navigation(12, 30, false, false),
            (12, false)
        );
        assert_eq!(
            search_selection_after_navigation(12, 30, true, false),
            (13, true)
        );
        assert_eq!(
            search_selection_after_navigation(12, 30, false, true),
            (11, true)
        );
        assert_eq!(
            search_selection_after_navigation(0, 30, false, true),
            (0, false)
        );
    }

    #[test]
    fn in_file_search_navigation_wraps_in_both_directions() {
        assert_eq!(next_find_match(0, 3, false), 1);
        assert_eq!(next_find_match(2, 3, false), 0);
        assert_eq!(next_find_match(0, 3, true), 2);
        assert_eq!(next_find_match(0, 0, false), 0);
    }

    #[test]
    fn bracket_pair_matching_respects_nested_pairs_on_either_side_of_the_cursor() {
        let text = "fn call(value: [u8; 2]) { values[index] }";
        let opening = text.find('[').unwrap();
        let closing = text[opening..].find(']').unwrap() + opening;

        assert_eq!(
            match_bracket_pair(text, text[..opening].chars().count()),
            Some((opening..opening + 1, closing..closing + 1))
        );
        assert_eq!(
            match_bracket_pair(text, text[..closing + 1].chars().count()),
            Some((opening..opening + 1, closing..closing + 1))
        );
    }

    #[test]
    fn in_file_search_highlights_every_match_and_distinguishes_the_active_one() {
        let text = "needle then needle";
        let base = egui::text::LayoutJob::simple(
            text.into(),
            egui::FontId::monospace(14.0),
            egui::Color32::WHITE,
            400.0,
        );
        let highlighted = find_highlighted_job(&base, &match_spans(text, "needle"), 1);
        let backgrounds: Vec<_> = highlighted
            .sections
            .iter()
            .filter_map(|section| {
                (section.format.background != egui::Color32::TRANSPARENT)
                    .then_some(section.format.background)
            })
            .collect();

        assert_eq!(highlighted.text, text);
        assert_eq!(backgrounds.len(), 2);
        assert_ne!(backgrounds[0], backgrounds[1]);
    }

    #[test]
    fn arrow_keys_navigate_only_the_focused_pane() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let first = root.join("a.rs");
        let second = root.join("b.rs");
        fs::write(&first, "one\ntwo\n").unwrap();
        fs::write(&second, "three\n").unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root,
            file: Some(first.clone()),
            create: false,
        })
        .unwrap();
        let context = egui::Context::default();
        let draw = |app: &mut EditorApp, events| {
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Default::default(),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            };
            let _ = context.run_ui(input, |root| app.ui(root));
        };
        let arrow_down = || Event::Key {
            key: Key::ArrowDown,
            physical_key: Some(Key::ArrowDown),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };

        draw(&mut app, Vec::new());
        draw(&mut app, vec![arrow_down()]);
        assert_eq!(app.tree.selected.as_ref(), Some(&first));

        app.tree_focused = true;
        context.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
        draw(&mut app, vec![arrow_down()]);
        assert_eq!(app.tree.selected.as_ref(), Some(&second));
    }

    #[test]
    fn command_f_and_command_shift_f_open_their_own_searches() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("current.rs");
        fs::write(&file, "needle here\n").unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root: temp.path().canonicalize().unwrap(),
            file: Some(file),
            create: false,
        })
        .unwrap();
        let context = egui::Context::default();
        let command = Modifiers {
            command: true,
            ..Modifiers::NONE
        };
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1000.0, 700.0),
            )),
            modifiers: command,
            events: vec![Event::Key {
                key: Key::F,
                physical_key: Some(Key::F),
                pressed: true,
                repeat: false,
                modifiers: command,
            }],
            ..RawInput::default()
        };

        let _ = context.run_ui(input, |root| app.ui(root));

        assert!(app.find_open);
        assert!(!app.search_open);
        assert!(app.sidebar);
        assert!(context.memory(|memory| memory.has_focus(Id::new("file_search_query"))));

        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1000.0, 700.0),
            )),
            events: vec![Event::Text("needle".into())],
            ..RawInput::default()
        };
        let _ = context.run_ui(input, |root| app.ui(root));
        assert_eq!(app.find_query, "needle");
        assert_eq!(app.find_matches.len(), 1);
        assert_eq!(app.find_matches[0], 0..6);

        let command_shift = Modifiers {
            command: true,
            shift: true,
            ..Modifiers::NONE
        };
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1000.0, 700.0),
            )),
            modifiers: command_shift,
            events: vec![Event::Key {
                key: Key::F,
                physical_key: Some(Key::F),
                pressed: true,
                repeat: false,
                modifiers: command_shift,
            }],
            ..RawInput::default()
        };
        let _ = context.run_ui(input, |root| app.ui(root));

        assert!(app.search_open);
        assert!(!app.find_open);
        assert!(context.memory(|memory| memory.has_focus(Id::new("project_search_query"))));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_icon_is_resolved_from_the_bundle_containing_the_executable() {
        let executable = std::path::Path::new("/Applications/Editur.app/Contents/MacOS/editur");

        assert_eq!(
            super::macos_icon_path(executable),
            Some(std::path::PathBuf::from(
                "/Applications/Editur.app/Contents/Resources/Editur.icns"
            ))
        );
        assert_eq!(
            super::macos_icon_path(std::path::Path::new("/usr/local/bin/editur")),
            None
        );
    }
}
