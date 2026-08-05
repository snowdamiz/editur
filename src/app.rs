use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use egui::{
    Align2, Color32, FontId, Id, Key, Label, Layout, RichText, ScrollArea, Sense, TextEdit,
    TextFormat, ViewportId, text::LayoutJob,
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
    file_io::{OpenTarget, SaveError, load_buffer, safe_save},
    instance::{Claim, InstanceEvent, claim, spawn_listener},
    renderer::Renderer,
    search::{SearchController, SearchHit, SearchResults},
    syntax::{Highlighter, IncrementalHighlightCache, SyntaxManager, data_dir},
    tree::{TreeEntry, read_directory},
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

#[derive(Clone)]
struct VisibleEntry {
    entry: TreeEntry,
    depth: usize,
}

struct TreeState {
    root: PathBuf,
    children: HashMap<PathBuf, Vec<TreeEntry>>,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    visible: Vec<VisibleEntry>,
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
    }

    fn append_visible(
        children: &HashMap<PathBuf, Vec<TreeEntry>>,
        expanded: &HashSet<PathBuf>,
        directory: &Path,
        depth: usize,
        result: &mut Vec<VisibleEntry>,
    ) {
        let Some(entries) = children.get(directory) else {
            return;
        };
        for entry in entries {
            result.push(VisibleEntry {
                entry: entry.clone(),
                depth,
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
}

pub struct EditorApp {
    buffer: Option<Buffer>,
    tree: TreeState,
    syntaxes: SyntaxManager,
    highlighter: Highlighter,
    highlight_cache: HighlightCache,
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
    sidebar: bool,
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
            syntaxes,
            highlighter: Highlighter::new()?,
            highlight_cache: HighlightCache::default(),
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
            sidebar: true,
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
                    self.tree.selected = Some(path);
                    self.buffer = Some(buffer);
                    self.highlight_cache.valid = false;
                    self.find_match_revision = u64::MAX;
                    self.scroll_to_find_match = self.find_open;
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

        if let Some(error) = self.error.clone() {
            egui::Panel::top("error_banner").show(root, |ui| {
                ui.horizontal(|ui| {
                    ui.colored_label(Color32::from_rgb(255, 125, 125), error);
                    if ui.button("Dismiss").clicked() {
                        self.error = None;
                    }
                });
            });
        }

        #[cfg(target_os = "macos")]
        self.draw_titlebar(root);

        egui::Panel::bottom("status")
            .exact_size(25.0)
            .show(root, |ui| {
                ui.with_layout(Layout::left_to_right(egui::Align::Center), |ui| {
                    if let Some(buffer) = &self.buffer {
                        let syntax = self
                            .syntaxes
                            .detect(&buffer.path, buffer.large_file_warning)
                            .name
                            .clone();
                        ui.label(buffer.path.display().to_string());
                        ui.separator();
                        ui.label(if buffer.dirty { "Modified" } else { "Saved" });
                        ui.separator();
                        ui.label(syntax);
                        ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.label(format!("Ln {}, Col {}", self.cursor.0, self.cursor.1));
                        });
                    } else {
                        ui.label(self.tree.root.display().to_string());
                    }
                });
            });

        if self.sidebar {
            egui::Panel::left("files")
                .resizable(true)
                .default_size(220.0)
                .size_range(140.0..=500.0)
                .show(root, |ui| self.draw_tree(ui));
        }

        egui::CentralPanel::default().show(root, |ui| self.draw_editor(ui));
        self.draw_find(&ctx);
        self.draw_search(&ctx);
        self.draw_dialogs(&ctx);
    }

    #[cfg(target_os = "macos")]
    fn draw_titlebar(&mut self, root: &mut egui::Ui) {
        egui::Panel::top("titlebar")
            .exact_size(42.0)
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(27, 27, 36))
                    .inner_margin(egui::Margin::ZERO),
            )
            .show(root, |ui| {
                let rect = ui.max_rect();
                let drag = ui.interact(rect, Id::new("titlebar_drag"), Sense::click_and_drag());
                if drag.drag_started() {
                    self.window_action = Some(WindowAction::Drag);
                } else if drag.double_clicked() {
                    self.window_action = Some(WindowAction::ToggleMaximize);
                }

                ui.horizontal(|ui| {
                    if titlebar_button(ui, Color32::from_rgb(255, 95, 87), "×", "Close").clicked()
                    {
                        self.window_action = Some(WindowAction::Close);
                    }
                    if titlebar_button(ui, Color32::from_rgb(254, 188, 46), "−", "Minimize")
                        .clicked()
                    {
                        self.window_action = Some(WindowAction::Minimize);
                    }
                    if titlebar_button(ui, Color32::from_rgb(40, 200, 64), "+", "Zoom").clicked() {
                        self.window_action = Some(WindowAction::ToggleMaximize);
                    }
                });
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    "Editur",
                    FontId::proportional(14.0),
                    Color32::from_rgb(188, 188, 198),
                );
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

    fn draw_find(&mut self, ctx: &egui::Context) {
        if !self.find_open {
            return;
        }
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
        let frame = egui::Frame::window(&ctx.style_of(ctx.theme()))
            .fill(Color32::from_rgb(24, 25, 30))
            .stroke(egui::Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 22),
            ))
            .inner_margin(10)
            .corner_radius(10)
            .shadow(egui::Shadow {
                offset: [0, 6],
                blur: 20,
                spread: 1,
                color: Color32::from_black_alpha(130),
            });
        egui::Window::new("Find in file")
            .id(Id::new("file_search"))
            .title_bar(false)
            .anchor(Align2::RIGHT_TOP, egui::vec2(-16.0, 48.0))
            .fixed_size(egui::vec2(470.0, 52.0))
            .resizable(false)
            .collapsible(false)
            .frame(frame)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.horizontal(|ui| {
                    let response = egui::Frame::new()
                        .fill(Color32::from_rgb(33, 35, 42))
                        .inner_margin(egui::Margin::symmetric(8, 4))
                        .corner_radius(6)
                        .show(ui, |ui| {
                            ui.add_sized(
                                egui::vec2(240.0, 24.0),
                                TextEdit::singleline(&mut self.find_query)
                                    .id(Id::new("file_search_query"))
                                    .hint_text("Find in current file…")
                                    .frame(egui::Frame::NONE),
                            )
                        })
                        .inner;
                    if self.focus_find {
                        response.request_focus();
                        self.focus_find = false;
                    }
                    query_changed = response.changed();
                    ui.add_sized(
                        egui::vec2(62.0, 30.0),
                        Label::new(RichText::new(&count).small().weak()),
                    );
                    previous = ui
                        .add_sized(egui::vec2(36.0, 30.0), egui::Button::new("↑"))
                        .on_hover_text("Previous match (Shift+Enter)")
                        .clicked();
                    next = ui
                        .add_sized(egui::vec2(36.0, 30.0), egui::Button::new("↓"))
                        .on_hover_text("Next match (Enter)")
                        .clicked();
                    close |= ui
                        .add_sized(egui::vec2(36.0, 30.0), egui::Button::new("×"))
                        .on_hover_text("Close (Esc)")
                        .clicked();
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
        let mut clicked = None;
        ScrollArea::vertical()
            .id_salt("file_tree_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for visible in &self.tree.visible {
                    ui.horizontal(|ui| {
                        ui.add_space((visible.depth * 14) as f32);
                        let expanded = self.tree.expanded.contains(&visible.entry.path);
                        let marker = if visible.entry.is_dir {
                            if expanded { "▾" } else { "▸" }
                        } else {
                            " "
                        };
                        let label = format!("{marker} {}", visible.entry.name.to_string_lossy());
                        let selected = self.tree.selected.as_ref() == Some(&visible.entry.path);
                        let response = ui.selectable_label(selected, label);
                        if selected && scroll_to_selected {
                            response.scroll_to_me(None);
                        }
                        if response.clicked() && self.pending.is_none() {
                            clicked = Some(visible.entry.clone());
                        }
                    });
                }
            });
        if let Some(entry) = clicked {
            self.tree_focused = true;
            self.focus_editor = false;
            ui.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
            self.tree.selected = Some(entry.path.clone());
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
        let entries = &self.tree.visible;
        if entries.is_empty() {
            return false;
        }
        let current = self
            .tree
            .selected
            .as_ref()
            .and_then(|path| entries.iter().position(|entry| &entry.entry.path == path))
            .unwrap_or(0);
        let next = ui.input(|input| {
            if input.key_pressed(Key::ArrowDown) {
                Some((current + 1).min(entries.len() - 1))
            } else if input.key_pressed(Key::ArrowUp) {
                Some(current.saturating_sub(1))
            } else {
                None
            }
        });
        if let Some(next) = next {
            self.tree.selected = Some(entries[next].entry.path.clone());
        }
        let entry = entries[next.unwrap_or(current)].entry.clone();
        if ui.input(|input| input.key_pressed(Key::ArrowRight)) && entry.is_dir {
            if !self.tree.expanded.contains(&entry.path)
                && let Err(error) = self.tree.toggle(&entry.path)
            {
                self.show_error(error);
            }
        } else if ui.input(|input| input.key_pressed(Key::ArrowLeft)) && entry.is_dir {
            self.tree.collapse(&entry.path);
        } else if ui.input(|input| input.key_pressed(Key::Enter)) {
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
        let scroll_match = self
            .scroll_to_find_match
            .then(|| find_matches.get(find_selected).cloned())
            .flatten();
        let Some(buffer) = self.buffer.as_mut() else {
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
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap_width: f32| {
            if !cache.valid || cache.revision != revision || cache.syntax != syntax_name {
                if large_file {
                    cache.job = plain_text_job(text.as_str(), wrap_width);
                    cache.incremental = IncrementalHighlightCache::default();
                } else {
                    match highlighter.highlight_job_incremental(
                        text.as_str(),
                        syntax,
                        syntaxes.set(),
                        wrap_width,
                        &mut cache.incremental,
                    ) {
                        Ok(job) => cache.job = job,
                        Err(error) => {
                            highlight_error = Some(error);
                            cache.job = plain_text_job(text.as_str(), wrap_width);
                        }
                    }
                }
                cache.revision = revision;
                cache.syntax.clone_from(&syntax_name);
                cache.valid = true;
                cache.find_valid = false;
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
            job.wrap.max_width = wrap_width;
            job.wrap.break_anywhere = true;
            ui.fonts_mut(|fonts| fonts.layout_job(job))
        };
        let output = ScrollArea::vertical()
            .id_salt("editor_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                let output = TextEdit::multiline(&mut buffer.text)
                    .id(Id::new("editor"))
                    .code_editor()
                    .desired_width(ui.available_width())
                    .desired_rows(30)
                    .frame(egui::Frame::NONE)
                    .layouter(&mut layouter)
                    .show(ui);
                if let Some(span) = &scroll_match {
                    let character = buffer.text[..span.start].chars().count();
                    let cursor = output
                        .galley
                        .pos_from_cursor(egui::text::CCursor::new(character))
                        .translate(
                            output.galley_pos.to_vec2()
                                - egui::vec2(output.galley.rect.left(), 0.0),
                        );
                    ui.scroll_to_rect(cursor, Some(egui::Align::Center));
                }
                output
            })
            .inner;
        if self.scroll_to_find_match {
            self.scroll_to_find_match = false;
        }
        if self.focus_editor {
            output.response.request_focus();
            self.focus_editor = false;
        }
        if output.response.has_focus() {
            self.tree_focused = false;
        }
        if output.response.changed() {
            buffer.mark_changed();
            self.highlight_cache.valid = false;
        }
        if let Some(range) = output.cursor_range {
            self.cursor = buffer.line_column(range.primary.index.into());
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
                            self.tree.selected =
                                self.buffer.as_ref().map(|buffer| buffer.path.clone());
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
    let mut shell = Shell::new(EditorApp::new(target)?, started);
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

impl Shell {
    fn new(editor: EditorApp, started: Instant) -> Self {
        let clipboard = match arboard::Clipboard::new() {
            Ok(clipboard) => Some(clipboard),
            Err(error) => {
                eprintln!("editur: system clipboard is unavailable: {error}");
                None
            }
        };
        Self {
            editor,
            window: None,
            renderer: None,
            egui: None,
            repaint_at: None,
            fatal: None,
            clipboard,
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
        if let Some(clipboard) = &mut self.clipboard {
            for command in &output.platform_output.commands {
                if let egui::OutputCommand::CopyText(text) = command
                    && let Err(error) = clipboard.set_text(text)
                {
                    self.editor
                        .show_error(format!("cannot copy to system clipboard: {error}"));
                }
            }
        }
        state.handle_platform_output_with_event_loop(window, event_loop, output.platform_output);
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        if let Err(error) =
            renderer.render(output.pixels_per_point, &primitives, &output.textures_delta)
        {
            self.fail(event_loop, error);
            return;
        }
        if !self.first_frame_logged {
            if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
                eprintln!(
                    "editur: first editable frame in {:.2?}",
                    self.started.elapsed()
                );
            }
            self.first_frame_logged = true;
        }
        if self.editor.should_close {
            window.set_visible(false);
            self.editor.should_close = false;
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
        let attributes = attributes.with_decorations(false);
        let window_started = Instant::now();
        let window = match event_loop.create_window(attributes) {
            Ok(window) => window,
            Err(error) => {
                self.fail(event_loop, format!("cannot create window: {error}"));
                return;
            }
        };
        let window_time = window_started.elapsed();
        #[cfg(target_os = "macos")]
        if let Err(error) = set_macos_application_icon() {
            self.fail(event_loop, error);
            return;
        }
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
        window.request_redraw();
        self.egui = Some(state);
        self.renderer = Some(renderer);
        self.window = Some(window);
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
            && let (Some(clipboard), Some(state)) = (&mut self.clipboard, &mut self.egui)
        {
            match clipboard.get_text() {
                Ok(text) => state.set_clipboard_text(text),
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
                    window.set_visible(false);
                    self.editor.should_close = false;
                } else {
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(renderer) = self.renderer.as_mut() {
                    renderer.resize(size);
                }
                window.request_redraw();
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
fn set_macos_application_icon() -> Result<(), String> {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};

    let png = include_bytes!("../assets/icons/editur.png");
    unsafe {
        let data: *mut Object =
            msg_send![class!(NSData), dataWithBytes: png.as_ptr() length: png.len()];
        let image: *mut Object = msg_send![class!(NSImage), alloc];
        let image: *mut Object = msg_send![image, initWithData: data];
        if image.is_null() {
            return Err("cannot decode macOS application icon".into());
        }
        let application: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: () = msg_send![application, setApplicationIconImage: image];
        let _: () = msg_send![image, release];
    }
    Ok(())
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

#[cfg(target_os = "macos")]
fn titlebar_button(ui: &mut egui::Ui, color: Color32, symbol: &str, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let fill = if response.hovered() {
        color.gamma_multiply(1.12)
    } else {
        color
    };
    ui.painter().circle_filled(rect.center(), 6.5, fill);
    if response.hovered() {
        ui.painter().text(
            rect.center(),
            Align2::CENTER_CENTER,
            symbol,
            FontId::proportional(10.0),
            Color32::from_black_alpha(150),
        );
    }
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
        EditorApp, TreeState, find_highlighted_job, match_spans, next_find_match, plain_text_job,
        search_selection_after_navigation,
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
}
