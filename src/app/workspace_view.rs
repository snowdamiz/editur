use super::*;

impl EditorApp {
    pub(super) fn draw_sidebar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter()
            .rect_filled(rect, 0.0, theme::state::sidebar_material());
        let settings = sidebar_settings_rect(rect);
        let tree = rect.with_max_y(settings.top());
        #[cfg(target_os = "macos")]
        let tree = tree.with_min_y((tree.top() + TITLEBAR_HEIGHT).min(tree.bottom()));
        ui.scope_builder(
            UiBuilder::new().id_salt("sidebar_tree").max_rect(tree),
            |ui| self.draw_tree(ui),
        );
        let (open_settings, update) = self.draw_settings_row(ui, settings);
        if open_settings {
            self.execute_keybinding(KeybindingCommand::AppOpenSettings, None, ui.ctx());
        }
        if update {
            self.start_update(ui.ctx());
        }
    }

    pub(super) fn draw_find(&mut self, ui: &mut egui::Ui, pane: PaneId) {
        if !self.pane_find.get(&pane).is_some_and(|find| find.open) {
            return;
        }
        let ctx = ui.ctx().clone();
        let query_id = Id::new(("file_search_query", pane.0));
        let query_focused = ctx.memory(|memory| memory.has_focus(query_id));
        let (enter, backwards, mut close) = ctx.input(|input| {
            (
                query_focused && input.key_pressed(Key::Enter),
                input.modifiers.shift,
                pane == self.active_pane && input.key_pressed(Key::Escape),
            )
        });
        let mut query_changed = false;
        let mut previous = false;
        let mut next = false;
        let count = match self.pane_find.get(&pane) {
            Some(find) if !find.matches.is_empty() => {
                format!("{} / {}", find.selected + 1, find.matches.len())
            }
            _ => "0 / 0".to_owned(),
        };
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, theme::surface().chrome);
        ui.painter().hline(
            rect.x_range(),
            rect.top(),
            egui::Stroke::new(1.0, theme::border::strong_color()),
        );
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(8.0, 6.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let input_width = (ui.available_width() - 142.0).max(40.0);
                let response = egui::Frame::new()
                    .fill(theme::surface().input)
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .corner_radius(4)
                    .show(ui, |ui| {
                        ui.set_width((input_width - 12.0).max(28.0));
                        let find = self.pane_find.get_mut(&pane).expect("open pane find");
                        ui.add_sized(
                            egui::vec2(ui.available_width(), 20.0),
                            TextEdit::singleline(&mut find.query)
                                .id(query_id)
                                .font(theme::typography::body())
                                .hint_text("Find in current file…")
                                .frame(egui::Frame::NONE),
                        )
                    })
                    .inner;
                ui.add(Label::new(
                    RichText::new(&count)
                        .monospace()
                        .size(theme::typography::MICRO_SIZE)
                        .weak(),
                ));
                previous = chevron_icon_button(ui, true, "Previous match (Shift+Enter)").clicked();
                next = chevron_icon_button(ui, false, "Next match (Enter)").clicked();
                close |= close_icon_button(ui).clicked();
                let find = self.pane_find.get_mut(&pane).expect("open pane find");
                if find.focus {
                    response.request_focus();
                    find.focus = false;
                }
                query_changed = response.changed();
            },
        );
        if query_changed {
            let find = self.pane_find.get_mut(&pane).expect("open pane find");
            find.selected = 0;
            find.match_revision = u64::MAX;
            self.refresh_find_matches(pane);
            let find = self.pane_find.get_mut(&pane).expect("open pane find");
            find.scroll_to_match = !find.matches.is_empty();
            ctx.request_repaint();
        } else if (enter || previous || next)
            && self
                .pane_find
                .get(&pane)
                .is_some_and(|find| !find.matches.is_empty())
        {
            let find = self.pane_find.get_mut(&pane).expect("open pane find");
            find.selected = next_find_match(
                find.selected,
                find.matches.len(),
                previous || (enter && backwards),
            );
            find.scroll_to_match = true;
            if let Some(index) = self
                .pane_active_tabs
                .get(&pane)
                .and_then(|path| self.tabs.iter().position(|tab| &tab.buffer.path == path))
            {
                self.tabs[index].highlight_cache.find_valid = false;
            }
            ctx.request_repaint();
        }
        if close {
            self.pane_find.get_mut(&pane).expect("open pane find").open = false;
            if pane == self.active_pane {
                self.focus_editor = self.active_tab.is_some();
            }
        }
    }

    pub(super) fn draw_vim_overlay(&mut self, ctx: &egui::Context) {
        let Some(overlay) = &mut self.vim_overlay else {
            return;
        };
        let escape = ctx.input(|input| input.key_pressed(Key::Escape));
        let enter = ctx.input(|input| input.key_pressed(Key::Enter));
        let screen = ctx.content_rect();
        let width = screen.width().min(640.0);
        egui::Area::new(Id::new("vim_command_overlay"))
            .order(egui::Order::Foreground)
            .fixed_pos(egui::pos2(
                screen.center().x - width * 0.5,
                screen.bottom() - 48.0,
            ))
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::surface().raised)
                    .stroke(egui::Stroke::new(1.0, theme::border::strong_color()))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(10, 7))
                    .show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        ui.horizontal(|ui| {
                            let prefix = match overlay.kind {
                                VimOverlayKind::Search(VimSearchDirection::Forward) => "/",
                                VimOverlayKind::Search(VimSearchDirection::Backward) => "?",
                                VimOverlayKind::Ex => ":",
                            };
                            ui.label(RichText::new(prefix).monospace().color(theme::accent()));
                            let response = ui.add_sized(
                                egui::vec2(ui.available_width(), 24.0),
                                TextEdit::singleline(&mut overlay.input)
                                    .id(Id::new("vim_command_input"))
                                    .hint_text(match overlay.kind {
                                        VimOverlayKind::Ex => "Vim Ex command",
                                        VimOverlayKind::Search(_) => "Vim search",
                                    })
                                    .frame(egui::Frame::NONE),
                            );
                            if overlay.focus {
                                response.request_focus();
                                overlay.focus = false;
                            }
                        });
                        if let Some(error) = &overlay.error {
                            ui.colored_label(theme::ink(theme::semantic().danger), error);
                        }
                    });
            });
        if escape {
            self.vim_overlay = None;
        } else if enter {
            let overlay = self.vim_overlay.take().expect("overlay exists");
            match overlay.kind {
                VimOverlayKind::Search(direction) => {
                    self.apply_vim_search(direction, overlay.input);
                }
                VimOverlayKind::Ex => {
                    match parse_ex(&overlay.input).and_then(|command| self.execute_ex(command)) {
                        Ok(()) => {}
                        Err(error) => {
                            self.vim_overlay = Some(VimOverlay {
                                kind: VimOverlayKind::Ex,
                                input: overlay.input,
                                error: Some(error),
                                focus: true,
                            });
                        }
                    }
                }
            }
        }
    }

    pub(super) fn draw_search(&mut self, root: &mut egui::Ui) {
        if !self.search_open {
            return;
        }
        let ctx = root.ctx().clone();
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
        let palette_size = egui::vec2(680.0, if empty_query { 185.0 } else { 430.0 });
        let screen = ctx.content_rect();
        let palette_rect = egui::Rect::from_min_size(
            egui::pos2(
                screen.center().x - palette_size.x / 2.0,
                screen.top() + 48.0,
            ),
            palette_size,
        );
        let palette_frame = egui::Frame::window(&ctx.style_of(ctx.theme()))
            .fill(theme::surface().raised)
            .stroke(theme::border::strong())
            .inner_margin(theme::space::WIDE as i8)
            .corner_radius(theme::corner(theme::radius::DIALOG))
            .shadow(theme::shadow::dialog());
        let mut palette = root.new_child(
            UiBuilder::new()
                .id_salt("project_search")
                .layer_id(egui::LayerId::new(
                    egui::Order::Foreground,
                    Id::new("project_search"),
                ))
                .max_rect(palette_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        palette.set_clip_rect(screen);
        palette.interact(
            palette_rect,
            Id::new("project_search_surface"),
            Sense::click(),
        );
        palette
            .painter()
            .add(palette_frame.paint(palette_rect.shrink(15.0)));
        palette.scope_builder(
            UiBuilder::new()
                .max_rect(palette_rect.shrink(15.0))
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;

                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Search project")
                            .size(theme::typography::TITLE_SIZE)
                            .strong(),
                    );
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
                    .fill(theme::surface().input)
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

                        if !results.files.is_empty() {
                            search_group_header(ui, "FILES", results.files.len());
                        }
                        for (index, hit) in results.files.iter().enumerate() {
                            let response = search_result_row(
                                ui,
                                self.search_selected == index,
                                file_result_job(
                                    &hit.relative,
                                    self.search_query.trim(),
                                    ui.available_width() - 18.0,
                                ),
                                30.0,
                            );
                            if scroll_to_selection && self.search_selected == index {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                selected_path = Some(hit.path.clone());
                            }
                        }

                        if !results.contents.is_empty() {
                            ui.add_space(theme::space::SMALL);
                            search_group_header(ui, "FILE CONTENT", results.contents.len());
                        }
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
            },
        );

        if query_changed {
            self.search_selected = 0;
            if let Err(error) = self.search.set_query(&self.search_query) {
                self.show_error(error);
            }
        }
        if escape {
            self.search_open = false;
            self.focus_editor = self.active_tab.is_some();
        } else if let Some(path) = selected_path {
            self.search_open = false;
            self.request(PendingAction::Open(path));
        }
    }

    /// The project switcher that opens under the tree's root header: recent
    /// roots plus a folder chooser for anything not in the list.
    pub(super) fn draw_project_menu(&mut self, tree_ui: &mut egui::Ui, opened_this_frame: bool) {
        let sidebar = tree_ui.max_rect();
        let position = egui::pos2(
            sidebar.left() + theme::space::SMALL,
            sidebar.top() + theme::control::ROW + 4.0,
        );
        let width = (sidebar.width() - theme::space::SMALL * 2.0).max(180.0);
        let mut switch_to = None;
        let mut browse = false;
        let response = egui::Area::new(Id::new("project_switcher"))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .show(tree_ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::surface().raised)
                    .stroke(egui::Stroke::new(1.0, theme::border::strong_color()))
                    .corner_radius(7)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(width - 12.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        if agentic_project_row(
                            ui,
                            &self.tree.root,
                            true,
                            self.git_workspace_status.as_ref(),
                        ) {
                            switch_to = Some(self.tree.root.clone());
                        }
                        for project in &self.recent_projects {
                            if project == &self.tree.root {
                                continue;
                            }
                            if agentic_project_row(ui, project, false, None) {
                                switch_to = Some(project.clone());
                            }
                        }
                        ui.add_space(theme::space::TIGHT);
                        let (line, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 1.0),
                            Sense::hover(),
                        );
                        ui.painter().hline(
                            line.x_range(),
                            line.center().y,
                            theme::border::hairline(),
                        );
                        ui.add_space(theme::space::TIGHT);
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 30.0),
                            Sense::hover(),
                        );
                        let open = ui
                            .interact(rect, Id::new("project_switcher_browse"), Sense::click())
                            .on_hover_text("Choose a project folder");
                        open.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                ui.is_enabled(),
                                "Open Folder…",
                            )
                        });
                        if open.hovered() {
                            ui.painter().rect_filled(rect, 6.0, theme::state::hover());
                        }
                        ui.painter().text(
                            egui::pos2(rect.left() + 8.0, rect.center().y),
                            Align2::LEFT_CENTER,
                            "Open Folder…",
                            theme::typography::small(),
                            if open.hovered() {
                                theme::text().primary
                            } else {
                                theme::text().secondary
                            },
                        );
                        browse = open.clicked();
                    });
            })
            .response;
        if tree_ui.input(|input| input.key_pressed(egui::Key::Escape))
            || (!opened_this_frame && response.clicked_elsewhere())
        {
            self.project_menu = false;
        }
        if let Some(project) = switch_to {
            self.switch_project(project);
        } else if browse {
            self.add_project_via_dialog();
        }
    }

    pub(super) fn draw_tree(&mut self, ui: &mut egui::Ui) {
        let scroll_to_selected = self.tree_focused
            && !ui.ctx().egui_wants_keyboard_input()
            && self.pending.is_none()
            && self.tree_keyboard(ui);
        let output = self.tree_surface.show(
            ui,
            &self.tree.root,
            &self.tree.visible,
            self.tree.selected_index,
            scroll_to_selected,
        );
        if output.response.clicked() {
            self.tree_focused = true;
        }
        if output.root_clicked {
            self.project_menu = !self.project_menu;
        }
        if self.project_menu {
            self.draw_project_menu(ui, output.root_clicked);
        }
        if let Some(index) = output.context_requested {
            let path = self.tree.visible[index].entry.path.clone();
            self.tree.select(Some(path));
            self.tree_focused = true;
            self.focus_editor = false;
            ui.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
        }
        let entry = self.tree.selected.as_ref().and_then(|path| {
            self.tree
                .visible
                .iter()
                .find(|row| &row.entry.path == path)
                .map(|row| row.entry.clone())
        });
        let mut context_action = None;
        if entry.is_some()
            && (output.context_requested.is_some() || output.response.context_menu_opened())
        {
            let can_paste = self.tree_clipboard.is_some();
            output.response.context_menu(|ui| {
                ui.set_min_width(220.0);
                if ui.button("Open").clicked() {
                    context_action = Some(TreeContextAction::Open);
                    ui.close();
                }
                ui.separator();
                if ui.button("New File…").clicked() {
                    context_action = Some(TreeContextAction::NewFile);
                    ui.close();
                }
                if ui.button("New Folder…").clicked() {
                    context_action = Some(TreeContextAction::NewFolder);
                    ui.close();
                }
                ui.separator();
                if ui.button("Cut").clicked() {
                    context_action = Some(TreeContextAction::Cut);
                    ui.close();
                }
                if ui.button("Copy").clicked() {
                    context_action = Some(TreeContextAction::Copy);
                    ui.close();
                }
                if ui
                    .add_enabled(can_paste, egui::Button::new("Paste"))
                    .clicked()
                {
                    context_action = Some(TreeContextAction::Paste);
                    ui.close();
                }
                if ui.button("Duplicate").clicked() {
                    context_action = Some(TreeContextAction::Duplicate);
                    ui.close();
                }
                ui.separator();
                if ui.button("Rename…").clicked() {
                    context_action = Some(TreeContextAction::Rename);
                    ui.close();
                }
                if ui.button("Delete…").clicked() {
                    context_action = Some(TreeContextAction::Delete);
                    ui.close();
                }
                ui.separator();
                if ui.button("Copy Path").clicked() {
                    context_action = Some(TreeContextAction::CopyPath);
                    ui.close();
                }
                if ui.button("Copy Relative Path").clicked() {
                    context_action = Some(TreeContextAction::CopyRelativePath);
                    ui.close();
                }
                if ui.button("Reveal in File Manager").clicked() {
                    context_action = Some(TreeContextAction::Reveal);
                    ui.close();
                }
                if ui.button("Open in Integrated Terminal").clicked() {
                    context_action = Some(TreeContextAction::OpenTerminal);
                    ui.close();
                }
                ui.separator();
                if ui.button("Refresh").clicked() {
                    context_action = Some(TreeContextAction::Refresh);
                    ui.close();
                }
            });
        }
        if let (Some(action), Some(entry)) = (context_action, entry) {
            self.execute_tree_context_action(action, entry, ui.ctx());
        }
        if let Some(index) = output.drag_started {
            let entry = &self.tree.visible[index].entry;
            self.tab_drag = Some(entry.path.clone());
            self.tree.select(Some(entry.path.clone()));
            ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
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

    pub(super) fn execute_tree_context_action(
        &mut self,
        action: TreeContextAction,
        entry: TreeEntry,
        ctx: &egui::Context,
    ) {
        let directory = if entry.is_dir {
            entry.path.clone()
        } else {
            entry.path.parent().unwrap_or(&self.tree.root).to_path_buf()
        };
        match action {
            TreeContextAction::Open if entry.is_dir => {
                if let Err(error) = self.tree.toggle(&entry.path) {
                    self.show_error(error);
                }
            }
            TreeContextAction::Open => self.request(PendingAction::Open(entry.path)),
            TreeContextAction::NewFile | TreeContextAction::NewFolder => {
                self.tree_prompt = Some(TreePrompt {
                    action: if matches!(action, TreeContextAction::NewFile) {
                        TreePromptAction::NewFile
                    } else {
                        TreePromptAction::NewFolder
                    },
                    directory,
                    original: None,
                    name: String::new(),
                    focus: true,
                });
            }
            TreeContextAction::Rename => {
                self.tree_prompt = Some(TreePrompt {
                    action: TreePromptAction::Rename,
                    directory,
                    original: Some(entry.path.clone()),
                    name: entry.name.to_string_lossy().into_owned(),
                    focus: true,
                });
            }
            TreeContextAction::Cut | TreeContextAction::Copy => {
                self.tree_clipboard = Some(TreeClipboard {
                    path: entry.path,
                    cut: matches!(action, TreeContextAction::Cut),
                });
            }
            TreeContextAction::Paste => match self.paste_tree_entry(&directory) {
                Ok(path) => self.refresh_tree(Some(path)),
                Err(error) => self.show_error(error),
            },
            TreeContextAction::Duplicate => {
                let result = entry
                    .path
                    .parent()
                    .ok_or_else(|| format!("{} has no parent", entry.path.display()))
                    .and_then(|parent| unique_copy_path(&entry.path, parent))
                    .and_then(|destination| {
                        copy_tree_entry(&entry.path, &destination).map(|()| destination)
                    });
                match result {
                    Ok(path) => self.refresh_tree(Some(path)),
                    Err(error) => self.show_error(error),
                }
            }
            TreeContextAction::Delete => {
                if self
                    .tabs
                    .iter()
                    .any(|tab| tab.buffer.dirty && tab.buffer.path.starts_with(&entry.path))
                {
                    self.show_error("save or close modified files before deleting them".into());
                } else {
                    self.tree_delete = Some(entry.path);
                }
            }
            TreeContextAction::CopyPath => {
                ctx.copy_text(entry.path.to_string_lossy().into_owned());
            }
            TreeContextAction::CopyRelativePath => {
                let path = entry
                    .path
                    .strip_prefix(&self.tree.root)
                    .unwrap_or(&entry.path);
                ctx.copy_text(path.to_string_lossy().into_owned());
            }
            TreeContextAction::Reveal => {
                if let Err(error) = reveal_in_file_manager(&entry.path) {
                    self.show_error(error);
                }
            }
            TreeContextAction::OpenTerminal => match self.terminal.open_at(&directory, ctx) {
                Ok(()) => self.terminal_open = true,
                Err(error) => self.show_error(error),
            },
            TreeContextAction::Refresh => self.refresh_tree(Some(entry.path)),
        }
        ctx.request_repaint();
    }

    pub(super) fn paste_tree_entry(&mut self, directory: &Path) -> Result<PathBuf, String> {
        let clipboard = self
            .tree_clipboard
            .clone()
            .ok_or_else(|| "nothing has been copied or cut".to_owned())?;
        let source = clipboard
            .path
            .canonicalize()
            .map_err(|error| format!("cannot access {}: {error}", clipboard.path.display()))?;
        let directory = directory
            .canonicalize()
            .map_err(|error| format!("cannot access {}: {error}", directory.display()))?;
        if directory.starts_with(&source) {
            return Err("cannot paste a folder inside itself".into());
        }
        if clipboard.cut && source.parent() == Some(directory.as_path()) {
            self.tree_clipboard = None;
            return Ok(source);
        }
        let name = source
            .file_name()
            .ok_or_else(|| format!("{} has no file name", source.display()))?;
        let candidate = directory.join(name);
        let destination = if candidate.exists() {
            unique_copy_path(&source, &directory)?
        } else {
            candidate
        };
        if clipboard.cut {
            fs::rename(&source, &destination)
                .map_err(|error| format!("cannot move {}: {error}", source.display()))?;
            self.rebase_open_paths(&source, &destination);
            self.tree_clipboard = None;
        } else {
            copy_tree_entry(&source, &destination)?;
        }
        Ok(destination)
    }

    pub(super) fn refresh_tree(&mut self, selected: Option<PathBuf>) {
        match self.tree.reload() {
            Ok(()) => self.tree.select(selected),
            Err(error) => self.show_error(error),
        }
    }

    pub(super) fn rebase_open_paths(&mut self, old: &Path, new: &Path) {
        for tab in &mut self.tabs {
            if let Ok(relative) = tab.buffer.path.strip_prefix(old) {
                tab.buffer.path = new.join(relative);
                tab.highlight_cache.valid = false;
            }
        }
        for path in self.pane_active_tabs.values_mut() {
            if let Ok(relative) = path.strip_prefix(old) {
                *path = new.join(relative);
            }
        }
        self.lsp_sync_needed = true;
    }

    pub(super) fn delete_tree_entry(&mut self, path: &Path) -> Result<(), String> {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if metadata.is_dir() {
            fs::remove_dir_all(path)
        } else {
            fs::remove_file(path)
        }
        .map_err(|error| format!("cannot delete {}: {error}", path.display()))?;
        let mut tabs = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| tab.buffer.path.starts_with(path).then_some(index))
            .collect::<Vec<_>>();
        tabs.reverse();
        for index in tabs {
            self.close_tab(index);
        }
        self.refresh_tree(None);
        Ok(())
    }

    pub(super) fn finish_tree_prompt(&mut self) -> Result<(PathBuf, TreePromptAction), String> {
        let prompt = self
            .tree_prompt
            .clone()
            .ok_or_else(|| "no file operation is pending".to_owned())?;
        let destination = child_path(&prompt.directory, prompt.name.trim())?;
        match prompt.action {
            TreePromptAction::NewFile => {
                fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&destination)
                    .map_err(|error| format!("cannot create {}: {error}", destination.display()))?;
            }
            TreePromptAction::NewFolder => fs::create_dir(&destination)
                .map_err(|error| format!("cannot create {}: {error}", destination.display()))?,
            TreePromptAction::Rename => {
                let original = prompt
                    .original
                    .as_ref()
                    .ok_or_else(|| "the original path for this rename is unavailable".to_owned())?;
                if destination != *original {
                    if destination.exists() {
                        return Err(format!("{} already exists", destination.display()));
                    }
                    fs::rename(original, &destination).map_err(|error| {
                        format!("cannot rename {}: {error}", original.display())
                    })?;
                    self.rebase_open_paths(original, &destination);
                }
            }
        }
        self.tree_prompt = None;
        Ok((destination, prompt.action))
    }

    pub(super) fn tree_keyboard(&mut self, ui: &egui::Ui) -> bool {
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

    pub(super) fn draw_editor(
        &mut self,
        ui: &mut egui::Ui,
        pane: PaneId,
        single_pane: bool,
        path_override: Option<&Path>,
        preview: bool,
    ) {
        let active_tab = path_override
            .and_then(|path| self.tabs.iter().position(|tab| tab.buffer.path == path))
            .or_else(|| {
                self.pane_active_tabs
                    .get(&pane)
                    .and_then(|path| self.tabs.iter().position(|tab| &tab.buffer.path == path))
            })
            .or_else(|| self.tabs.iter().position(|tab| tab.pane == pane));
        let active_pane = !preview && path_override.is_none() && pane == self.active_pane;
        if let Some(index) = active_tab
            && self.tabs[index].agent_diff.is_some()
        {
            let tab = &self.tabs[index];
            let baseline = tab.agent_diff.as_ref().expect("checked above");
            let dismissed = draw_agent_diff_view(
                ui,
                Id::new(("tab_agent_diff", &tab.buffer.path)),
                &self.tree.root,
                &tab.buffer.path,
                baseline.as_deref(),
                &tab.buffer.text,
                "Edit file",
                &self.highlighter,
                &self.syntaxes,
            );
            if dismissed {
                self.tabs[index].agent_diff = None;
            }
            return;
        }
        let markdown =
            active_tab.is_some_and(|index| markdown::is_markdown(&self.tabs[index].buffer.path));
        if markdown && active_tab.is_some_and(|index| self.tabs[index].markdown_preview) {
            let tab = &mut self.tabs[active_tab.expect("checked above")];
            draw_markdown_preview(
                ui,
                &tab.buffer.text,
                tab.buffer.revision,
                &mut tab.markdown_layout,
                pane,
            );
            return;
        }
        let find = self.pane_find.get(&pane).filter(|find| find.open);
        let find_open = find.is_some();
        let find_query = find.map_or("", |find| find.query.as_str());
        let find_matches = find.map_or(&[][..], |find| find.matches.as_slice());
        let find_selected = find.map_or(0, |find| find.selected);
        let scroll_to_find_match = find.is_some_and(|find| find.scroll_to_match);
        let bracket_pair = active_pane.then(|| self.bracket_pair.clone()).flatten();
        let scroll_character = scroll_to_find_match
            .then(|| find_matches.get(find_selected).cloned())
            .flatten()
            .map(|span| {
                active_tab.map_or(0, |index| {
                    self.tabs[index].buffer.text[..span.start].chars().count()
                })
            })
            .or_else(|| {
                let (path, character) = self.lsp_scroll_to.as_ref()?;
                let index = active_tab?;
                (active_pane && self.tabs[index].buffer.path == *path).then_some(*character)
            });
        let Some(index) = active_tab else {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, editor_background());
            draw_editor_empty_state(ui);
            return;
        };
        let diagnostics = self
            .lsp_diagnostics
            .get(&self.tabs[index].buffer.path)
            .filter(|diagnostics| diagnostics.revision == self.tabs[index].buffer.revision);
        let line_markers = diagnostics
            .map(|state| {
                state
                    .diagnostics
                    .iter()
                    .filter(|diagnostic| diagnostic.range.is_empty())
                    .map(|diagnostic| {
                        let color = diagnostic_color(diagnostic.severity);
                        (
                            diagnostic.line as usize,
                            if state.stale {
                                color.gamma_multiply(0.55)
                            } else {
                                color
                            },
                        )
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let FileTab {
            buffer,
            editor_surface,
            highlight_cache: cache,
            vim,
            ..
        } = &mut self.tabs[index];
        let vim_enabled = self.settings.keybindings.active_behavior() == KeybindingBehavior::Vim;
        let text_input = if vim_enabled {
            match vim.mode() {
                VimMode::Insert => TextInputMode::Insert,
                VimMode::Replace => TextInputMode::Replace,
                VimMode::Normal | VimMode::VisualCharacter | VimMode::VisualLine => {
                    TextInputMode::Disabled
                }
            }
        } else {
            TextInputMode::Standard
        };
        if buffer.large_file_warning {
            ui.colored_label(
                theme::ink(theme::semantic().warning),
                "Large file: syntax highlighting is disabled above 5 MiB.",
            );
        }

        let revision = buffer.revision;
        let highlighter = &self.highlighter;
        let syntaxes = &self.syntaxes;
        let large_file = buffer.large_file_warning;
        let mut highlight_error = None;
        let wrap_width = ui.available_width().max(1.0);
        let appearance = theme::appearance();
        if !cache.valid || cache.revision != revision || cache.appearance != appearance {
            let syntax = syntaxes.detect(&buffer.path, large_file);
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
            cache.appearance = appearance;
            cache.revision = revision;
            cache.syntax.clone_from(&syntax.name);
            cache.valid = true;
            cache.find_valid = false;
        }
        let syntax_name = cache.syntax.clone();
        let job = if find_open && !find_matches.is_empty() {
            if !cache.find_valid
                || cache.find_revision != revision
                || cache.find_query != find_query
                || cache.find_selected != find_selected
            {
                cache.find_job = find_highlighted_job(&cache.job, find_matches, find_selected);
                cache.find_revision = revision;
                cache.find_query = find_query.to_owned();
                cache.find_selected = find_selected;
                cache.find_valid = true;
            }
            &cache.find_job
        } else {
            &cache.job
        };
        let galley_key = GalleyKey {
            appearance,
            revision,
            syntax: syntax_name,
            find: (find_open && !find_matches.is_empty())
                .then(|| (find_query.to_owned(), find_selected)),
            bracket_pair: bracket_pair.clone(),
        };
        if cache.galley_key.as_ref() != Some(&galley_key) {
            cache.galley_key = Some(galley_key);
            cache.presentation_revision = cache.presentation_revision.wrapping_add(1);
            cache.bracket_job = bracket_pair
                .as_ref()
                .map(|pair| bracket_highlighted_job(job, pair));
        }
        let job = presentation_job(job, cache.bracket_job.as_ref());
        let job = if let Some(diagnostics) = diagnostics {
            let key = (cache.presentation_revision, diagnostics.generation);
            if cache.lsp_key != Some(key) {
                cache.lsp_job =
                    diagnostic_highlighted_job(job, &diagnostics.diagnostics, diagnostics.stale);
                cache.lsp_key = Some(key);
            }
            &cache.lsp_job
        } else {
            job
        };
        let document = DocumentMetrics {
            revision: cache.presentation_revision,
            line_count: buffer.line_count(),
            character_len: buffer.character_len(),
        };
        let editor_id = if preview {
            Id::new(("editor_preview", pane.0))
        } else if single_pane {
            Id::new("editor")
        } else {
            Id::new(("editor", pane.0))
        };
        let output = editor_surface.show_document_with_options(
            ui,
            &mut buffer.text,
            job,
            document,
            EditorShowOptions {
                request_focus: active_pane && self.focus_editor,
                scroll_to_character: scroll_character,
                id: editor_id,
                line_markers: &line_markers,
                text_input,
                native_keybindings: false,
                block_caret: vim_enabled && !vim.text_input_enabled(),
                wrap: self.settings.appearance.line_wrap == LineWrapPreference::Wrap,
            },
        );
        if vim_enabled && (output.response.clicked() || output.response.dragged()) {
            vim.sync_pointer_selection(editor_surface, output.response.triple_clicked());
        }
        if vim_enabled && !output.inserted_text.is_empty() {
            vim.record_insert_text(&output.inserted_text);
        }
        let mut diagnostic_at_pointer = false;
        if let Some(character) = output.hovered_character
            && let Some(state) = diagnostics
        {
            let byte = buffer.byte_index(character);
            if let Some(diagnostic) = state.diagnostics.iter().find(|diagnostic| {
                if diagnostic.range.is_empty() {
                    diagnostic.range.start == byte
                } else {
                    diagnostic.range.contains(&byte)
                }
            }) && let Some(pointer) = ui.ctx().pointer_hover_pos()
            {
                diagnostic_at_pointer = true;
                let severity = match diagnostic.severity {
                    crate::lsp::DiagnosticSeverity::Error => "Error",
                    crate::lsp::DiagnosticSeverity::Warning => "Warning",
                    crate::lsp::DiagnosticSeverity::Information => "Information",
                    crate::lsp::DiagnosticSeverity::Hint => "Hint",
                };
                let details = [diagnostic.source.as_deref(), diagnostic.code.as_deref()]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                    .join(" · ");
                egui::Area::new(Id::new(("diagnostic_hover", &buffer.path)))
                    .order(egui::Order::Foreground)
                    .fixed_pos(pointer + egui::vec2(12.0, 16.0))
                    .show(ui.ctx(), |ui| {
                        egui::Frame::new()
                            .fill(theme::surface().raised)
                            .stroke(egui::Stroke::new(1.0, theme::border::strong_color()))
                            .corner_radius(7)
                            .inner_margin(egui::Margin::same(10))
                            .show(ui, |ui| {
                                ui.set_max_width(420.0);
                                ui.label(
                                    RichText::new(if details.is_empty() {
                                        severity.to_owned()
                                    } else {
                                        format!("{severity} · {details}")
                                    })
                                    .strong()
                                    .color(diagnostic_color(diagnostic.severity)),
                                );
                                ui.label(&diagnostic.message);
                            });
                    });
            }
        }
        let activate_pane = !active_pane
            && (output.response.has_focus()
                || output.response.clicked()
                || output.response.drag_started());
        if scroll_to_find_match && let Some(find) = self.pane_find.get_mut(&pane) {
            find.scroll_to_match = false;
        }
        if active_pane
            && self
                .lsp_scroll_to
                .as_ref()
                .is_some_and(|(path, _)| *path == buffer.path)
        {
            self.lsp_scroll_to = None;
        }
        if active_pane {
            self.focus_editor = false;
        }
        if output.response.has_focus() {
            self.tree_focused = false;
        }
        if output.changed {
            buffer.mark_changed();
            cache.valid = false;
            self.lsp_sync_needed = true;
            self.lsp_completion = None;
            self.lsp_hover = None;
            self.lsp_hover_probe = None;
            self.lsp_pending_hover = None;
            if let Some(trigger) = output.last_inserted.map(|character| character.to_string())
                && preset_for_path(&buffer.path).is_some_and(|(preset, _)| {
                    matches!(
                        self.lsp_status.get(&preset.id),
                        Some(ServerStatus::Ready(capabilities))
                            if capabilities.completion
                                && capabilities.completion_triggers.contains(&trigger)
                    )
                })
            {
                self.lsp_pending_completion = Some((
                    RequestTag {
                        path: buffer.path.clone(),
                        revision: buffer.revision,
                        cursor: output.cursor,
                    },
                    Some(trigger),
                ));
            }
        }
        if active_pane {
            if let Some(rect) = output.caret_rect {
                let caret = LspCaret {
                    tag: RequestTag {
                        path: buffer.path.clone(),
                        revision: buffer.revision,
                        cursor: output.cursor,
                    },
                    rect,
                    bounds: ui.max_rect(),
                };
                if self
                    .lsp_completion
                    .as_ref()
                    .is_some_and(|popup| popup.tag != caret.tag)
                {
                    self.lsp_completion = None;
                }
                self.lsp_caret = Some(caret);
            }
            if output.response.clicked() && ui.input(|input| input.modifiers.command) {
                self.lsp_pending_definition = Some(RequestTag {
                    path: buffer.path.clone(),
                    revision: buffer.revision,
                    cursor: output.cursor,
                });
                self.lsp_definitions = None;
                self.lsp_sync_needed = true;
            }
            if output.response.clicked() {
                self.lsp_completion = None;
            }
            let pointer_interrupted = output.changed
                || output.scrolled
                || output.response.clicked()
                || output.response.dragged()
                || ui.input(|input| {
                    input.pointer.any_click()
                        || input
                            .events
                            .iter()
                            .any(|event| matches!(event, egui::Event::Key { pressed: true, .. }))
                });
            let hover_supported = preset_for_path(&buffer.path).is_some_and(|(preset, _)| {
                matches!(
                    self.lsp_status.get(&preset.id),
                    Some(ServerStatus::Ready(capabilities)) if capabilities.hover
                )
            });
            if pointer_interrupted || !hover_supported || diagnostic_at_pointer {
                self.lsp_hover_probe = None;
                self.lsp_hover = None;
                self.lsp_pending_hover = None;
            } else if let (Some(character), Some(pointer)) =
                (output.hovered_character, ui.ctx().pointer_hover_pos())
            {
                let tag = RequestTag {
                    path: buffer.path.clone(),
                    revision: buffer.revision,
                    cursor: character,
                };
                let same = self.lsp_hover_probe.as_ref().is_some_and(|probe| {
                    probe.tag == tag && probe.pointer.distance(pointer) <= 0.5
                });
                if !same {
                    self.lsp_hover_probe = Some(HoverProbe {
                        tag,
                        pointer,
                        bounds: ui.max_rect(),
                        started: Instant::now(),
                        requested: false,
                    });
                    self.lsp_hover = None;
                    self.lsp_pending_hover = None;
                    ui.ctx().request_repaint_after(Duration::from_millis(400));
                } else if let Some(probe) = self.lsp_hover_probe.as_mut()
                    && !probe.requested
                {
                    let elapsed = probe.started.elapsed();
                    if elapsed >= Duration::from_millis(400) {
                        probe.requested = true;
                        self.lsp_pending_hover = Some(probe.tag.clone());
                        self.lsp_sync_needed = true;
                        ui.ctx().request_repaint();
                    } else {
                        ui.ctx()
                            .request_repaint_after(Duration::from_millis(400) - elapsed);
                    }
                }
            } else {
                self.lsp_hover_probe = None;
                self.lsp_hover = None;
                self.lsp_pending_hover = None;
            }
            self.cursor = buffer.line_column(output.cursor);
            let bracket_pair_key = (buffer.revision, output.cursor);
            if self.bracket_pair_key != Some(bracket_pair_key) {
                let pair = (!buffer.large_file_warning)
                    .then(|| match_bracket_pair(buffer, output.cursor))
                    .flatten();
                self.bracket_pair_key = Some(bracket_pair_key);
                if self.bracket_pair != pair {
                    self.bracket_pair = pair;
                    ui.ctx().request_repaint();
                }
            }
        }
        if let Some(error) = highlight_error {
            self.show_error(error);
        }
        if activate_pane {
            self.activate_tab(index);
        }
    }

    pub(super) fn draw_agent_file_picker(&mut self, ctx: &egui::Context) {
        if self.agent_file_picker.is_none() {
            return;
        }
        let project_root = self.tree.root.clone();
        let attached_count = self.agent_attachments.len();
        let Some(picker) = self.agent_file_picker.as_mut() else {
            return;
        };
        match Self::file_picker_dialog(ctx, picker, Some(&project_root), attached_count) {
            Some(FilePickerOutcome::Dismissed) => self.agent_file_picker = None,
            Some(FilePickerOutcome::AttachFiles(paths)) => {
                self.agent_file_picker = None;
                self.attach_agent_files(ctx, paths);
                ctx.request_repaint();
            }
            Some(FilePickerOutcome::OpenDirectory(_)) | None => {}
        }
    }

    pub(super) fn draw_project_folder_picker(&mut self, ctx: &egui::Context) {
        if self.project_folder_picker.is_none() {
            return;
        }
        let project_root = self.tree.root.clone();
        let Some(picker) = self.project_folder_picker.as_mut() else {
            return;
        };
        match Self::file_picker_dialog(ctx, picker, Some(&project_root), 0) {
            Some(FilePickerOutcome::Dismissed) => self.project_folder_picker = None,
            Some(FilePickerOutcome::OpenDirectory(path)) => {
                self.project_folder_picker = None;
                self.switch_project(path);
                ctx.request_repaint();
            }
            Some(FilePickerOutcome::AttachFiles(_)) | None => {}
        }
    }

    /// The one file browser in the product. It reads a single directory per
    /// frame and renders from memory, which is why it opens instantly where a
    /// native dialog stalls. Returns what the user resolved this frame.
    pub(super) fn file_picker_dialog(
        ctx: &egui::Context,
        picker: &mut AgentFilePicker,
        project_root: Option<&Path>,
        attached_count: usize,
    ) -> Option<FilePickerOutcome> {
        let purpose = picker.purpose;
        // The rail offers the same places the operating system's own dialogs
        // do; only the ones that exist on this machine make the list.
        let user_dirs = directories::UserDirs::new();
        let mut places: Vec<(Icon, &str, PathBuf)> = Vec::new();
        if let Some(project_root) = project_root {
            places.push((Icon::Folder, "Project", project_root.to_path_buf()));
        }
        if let Some(dirs) = &user_dirs {
            places.push((Icon::Home, "Home", dirs.home_dir().to_path_buf()));
            let standard = [
                ("Desktop", dirs.desktop_dir()),
                ("Documents", dirs.document_dir()),
                ("Downloads", dirs.download_dir()),
            ];
            for (label, path) in standard {
                if let Some(path) = path.filter(|path| path.is_dir()) {
                    places.push((Icon::Folder, label, path.to_path_buf()));
                }
            }
        }
        // The place whose path sits deepest above the current directory wins
        // the highlight, so Home yields to Documents inside ~/Documents.
        let active_place = places
            .iter()
            .enumerate()
            .filter(|(_, (_, _, path))| picker.directory.starts_with(path))
            .max_by_key(|(_, (_, _, path))| path.components().count())
            .map(|(index, _)| index);
        let screen = ctx.content_rect();
        let size = egui::vec2(
            (screen.width() - 48.0).clamp(560.0, 760.0),
            (screen.height() - 48.0).clamp(360.0, 560.0),
        );

        // The keyboard works against the same filtered list the rows render.
        let entries = picker.visible_entries();
        let search_id = Id::new("agent_file_picker_search");
        // History rides on Finder's ⌘[ and ⌘], which never collide with the
        // caret keys the focused filter field owns.
        let (escape, enter, cursor_up, cursor_down, to_parent, mut go_back, mut go_forward) = ctx
            .input(|input| {
                let jump = input.modifiers.command || input.modifiers.alt;
                (
                    input.key_pressed(Key::Escape),
                    input.key_pressed(Key::Enter),
                    input.key_pressed(Key::ArrowUp) && !jump,
                    input.key_pressed(Key::ArrowDown) && !jump,
                    input.key_pressed(Key::ArrowUp) && jump,
                    input.key_pressed(Key::OpenBracket) && jump,
                    input.key_pressed(Key::CloseBracket) && jump,
                )
            });
        let mut close = escape;
        picker.cursor = picker.cursor.filter(|cursor| *cursor < entries.len());
        if cursor_down && !entries.is_empty() {
            picker.cursor = Some(
                picker
                    .cursor
                    .map_or(0, |cursor| (cursor + 1).min(entries.len() - 1)),
            );
        }
        if cursor_up {
            picker.cursor = picker.cursor.and_then(|cursor| cursor.checked_sub(1));
        }
        let cursor_moved = cursor_up || cursor_down;
        let cursor_entry = if enter {
            picker
                .cursor
                .and_then(|cursor| entries.get(cursor))
                .cloned()
        } else {
            None
        };
        let confirm = enter && cursor_entry.is_none();
        let mut attach =
            purpose == FilePickerPurpose::AttachFiles && confirm && !picker.selected.is_empty();
        let mut open_directory = purpose == FilePickerPurpose::OpenProject && confirm;
        let mut navigate = None;
        let mut toggle_file = None;
        if to_parent {
            navigate = picker.directory.parent().map(Path::to_path_buf);
        }
        match cursor_entry {
            Some(entry) if entry.is_dir => navigate = Some(entry.path),
            Some(entry) => toggle_file = Some(entry.path),
            None => {}
        }
        let mut refresh = false;
        let mut query_changed = false;
        let remaining = MAX_PROMPT_ATTACHMENTS
            .saturating_sub(attached_count.saturating_add(picker.selected.len()));
        let (title, window_id) = match purpose {
            FilePickerPurpose::AttachFiles => ("Add context", "agent_file_picker"),
            FilePickerPurpose::OpenProject => ("Open project", "project_folder_picker"),
        };
        let frame = egui::Frame::new()
            .fill(theme::surface().input)
            .stroke(theme::border::strong())
            .corner_radius(theme::corner(theme::radius::DIALOG))
            .shadow(theme::shadow::dialog());
        let appear = theme::motion::animate(
            ctx,
            Id::new((window_id, "appear")),
            true,
            theme::motion::BASE,
        );
        let modal = egui::Modal::new(Id::new(window_id))
            .backdrop_color(theme::motion::fade(theme::state::scrim(), appear))
            .frame(frame)
            .show(ctx, |ui| {
                let (_, full) = ui.allocate_space(size);
                let divider = theme::border::hairline();
                let header = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), 44.0));
                let toolbar =
                    egui::Rect::from_min_size(header.left_bottom(), egui::vec2(full.width(), 46.0));
                let footer = egui::Rect::from_min_max(
                    egui::pos2(full.left(), full.bottom() - 52.0),
                    full.right_bottom(),
                );
                let body = egui::Rect::from_min_max(toolbar.left_bottom(), footer.right_top());
                let rail_width = (full.width() * 0.26).clamp(150.0, 190.0);
                let rail = body.with_max_x(body.left() + rail_width);
                let list = egui::Rect::from_min_max(rail.right_top(), body.right_bottom());

                // One dialog surface: places, list, and footer share `input`.
                // Structure comes from hairlines, not nested chrome fills.
                ui.painter()
                    .hline(toolbar.x_range(), toolbar.bottom(), divider);
                ui.painter().vline(rail.right(), rail.y_range(), divider);
                ui.painter().hline(footer.x_range(), footer.top(), divider);

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(header.shrink2(egui::vec2(theme::space::LARGE, 0.0)))
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        ui.label(
                            RichText::new(title)
                                .size(theme::typography::TITLE_SIZE)
                                .strong()
                                .color(theme::text().primary),
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            close |= icon_button(
                                ui,
                                "Close (Esc)",
                                egui::vec2(30.0, 30.0),
                                theme::text().muted,
                                |painter, rect, color| {
                                    icons::paint(
                                        painter,
                                        Icon::Close,
                                        egui::Rect::from_center_size(
                                            rect.center(),
                                            egui::Vec2::splat(icons::GRID * 0.85),
                                        ),
                                        color,
                                    );
                                },
                            )
                            .clicked();
                        });
                    },
                );

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(toolbar.shrink2(egui::vec2(theme::space::LARGE, 8.0)))
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = theme::space::TIGHT;
                        let button = egui::Vec2::splat(theme::control::ROW);
                        ui.add_enabled_ui(picker.can_go_back(), |ui| {
                            go_back |= icons::button_with_id(
                                ui,
                                Some(Id::new("picker_nav_back")),
                                Icon::ChevronLeft,
                                "Back",
                                theme::text().secondary,
                                button,
                            )
                            .clicked();
                        });
                        ui.add_enabled_ui(picker.can_go_forward(), |ui| {
                            go_forward |= icons::button_with_id(
                                ui,
                                Some(Id::new("picker_nav_forward")),
                                Icon::ChevronRight,
                                "Forward",
                                theme::text().secondary,
                                button,
                            )
                            .clicked();
                        });
                        let parent = picker.directory.parent().map(Path::to_path_buf);
                        ui.add_enabled_ui(parent.is_some(), |ui| {
                            if icons::button_with_id(
                                ui,
                                Some(Id::new("picker_nav_up")),
                                Icon::ArrowUp,
                                "Enclosing folder",
                                theme::text().secondary,
                                button,
                            )
                            .clicked()
                            {
                                navigate = parent;
                            }
                        });
                        ui.add_space(theme::space::TIGHT);

                        let search_width = 170.0;
                        let address_width = (ui.available_width()
                            - search_width
                            - button.x
                            - theme::space::TIGHT * 2.0)
                            .max(96.0);
                        let (address_rect, _) =
                            ui.allocate_exact_size(egui::vec2(address_width, 30.0), Sense::hover());
                        ui.painter().rect_filled(
                            address_rect,
                            theme::corner(theme::radius::CONTROL),
                            theme::surface().raised,
                        );
                        ui.painter().rect_stroke(
                            address_rect,
                            theme::corner(theme::radius::CONTROL),
                            theme::border::hairline(),
                            egui::StrokeKind::Inside,
                        );
                        ui.scope_builder(
                            UiBuilder::new()
                                .max_rect(
                                    address_rect.shrink2(egui::vec2(theme::space::TIGHT, 4.0)),
                                )
                                .layout(Layout::left_to_right(Align::Center)),
                            |ui| {
                                ui.shrink_clip_rect(address_rect);
                                if let Some(target) = file_picker_breadcrumbs(ui, &picker.directory)
                                {
                                    navigate = Some(target);
                                }
                            },
                        );

                        refresh |= icons::button_with_id(
                            ui,
                            Some(Id::new("picker_nav_refresh")),
                            Icon::Refresh,
                            "Refresh folder",
                            theme::text().secondary,
                            button,
                        )
                        .clicked();

                        let (search_rect, _) =
                            ui.allocate_exact_size(egui::vec2(search_width, 30.0), Sense::hover());
                        ui.painter().rect_filled(
                            search_rect,
                            theme::corner(theme::radius::CONTROL),
                            theme::surface().raised,
                        );
                        icons::paint(
                            ui.painter(),
                            Icon::Search,
                            egui::Rect::from_center_size(
                                egui::pos2(search_rect.left() + 14.0, search_rect.center().y),
                                egui::Vec2::splat(icons::GRID * 0.75),
                            ),
                            theme::text().muted,
                        );
                        let search_response = ui
                            .scope_builder(
                                UiBuilder::new()
                                    .max_rect(egui::Rect::from_min_max(
                                        egui::pos2(
                                            search_rect.left() + 24.0,
                                            search_rect.top() + 4.0,
                                        ),
                                        egui::pos2(
                                            search_rect.right() - theme::space::SNUG,
                                            search_rect.bottom() - 4.0,
                                        ),
                                    ))
                                    .layout(Layout::left_to_right(Align::Center)),
                                |ui| {
                                    ui.add_sized(
                                        ui.available_size(),
                                        TextEdit::singleline(&mut picker.query)
                                            .id(search_id)
                                            .hint_text(match purpose {
                                                FilePickerPurpose::AttachFiles => "Filter files",
                                                FilePickerPurpose::OpenProject => "Filter folders",
                                            })
                                            .font(theme::typography::small())
                                            .frame(egui::Frame::NONE),
                                    )
                                },
                            )
                            .inner;
                        query_changed = search_response.changed();
                        ui.painter().rect_stroke(
                            search_rect,
                            theme::corner(theme::radius::CONTROL),
                            egui::Stroke::new(
                                theme::stroke::DIVIDER,
                                if search_response.has_focus() {
                                    theme::accent()
                                } else {
                                    theme::border::hairline_color()
                                },
                            ),
                            egui::StrokeKind::Inside,
                        );
                        if picker.focus_search {
                            search_response.request_focus();
                            picker.focus_search = false;
                        }
                    },
                );

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(rail.shrink2(egui::vec2(10.0, theme::space::MEDIUM)))
                        .layout(Layout::top_down(Align::LEFT)),
                    |ui| {
                        ui.spacing_mut().item_spacing.y = theme::space::HAIR;
                        ui.label(
                            RichText::new("PLACES")
                                .size(theme::typography::MICRO_SIZE)
                                .strong()
                                .color(theme::text().muted),
                        );
                        ui.add_space(theme::space::TIGHT);
                        for (index, (icon, label, path)) in places.iter().enumerate() {
                            if agent_file_picker_location_row(
                                ui,
                                *icon,
                                label,
                                active_place == Some(index),
                            )
                            .on_hover_text(path.display().to_string())
                            .clicked()
                            {
                                navigate = Some(path.clone());
                            }
                        }
                        if !picker.recent.is_empty() {
                            ui.add_space(theme::space::MEDIUM);
                            ui.label(
                                RichText::new("RECENT")
                                    .size(theme::typography::MICRO_SIZE)
                                    .strong()
                                    .color(theme::text().muted),
                            );
                            ui.add_space(theme::space::TIGHT);
                            for path in &picker.recent {
                                let name = path.file_name().map_or_else(
                                    || path.display().to_string(),
                                    |name| name.to_string_lossy().into_owned(),
                                );
                                if agent_file_picker_location_row(
                                    ui,
                                    Icon::History,
                                    &name,
                                    picker.directory == *path,
                                )
                                .on_hover_text(path.display().to_string())
                                .clicked()
                                {
                                    navigate = Some(path.clone());
                                }
                            }
                        }
                    },
                );

                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(list.shrink2(egui::vec2(theme::space::SNUG, 0.0)))
                        .layout(Layout::top_down(Align::LEFT)),
                    |ui| {
                        ScrollArea::vertical()
                            .id_salt("agent_file_picker_entries")
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.spacing_mut().item_spacing.y = theme::space::HAIR;
                                ui.add_space(theme::space::SNUG);
                                if entries.is_empty() {
                                    ui.add_space(theme::space::WIDE);
                                    ui.centered_and_justified(|ui| {
                                        let filtered = !picker.query.trim().is_empty();
                                        let message = if filtered {
                                            match purpose {
                                                FilePickerPurpose::AttachFiles => {
                                                    "No matching files"
                                                }
                                                FilePickerPurpose::OpenProject => {
                                                    "No matching folders"
                                                }
                                            }
                                        } else if picker.hidden_entries() > 0 {
                                            "Only hidden items here"
                                        } else {
                                            match purpose {
                                                FilePickerPurpose::AttachFiles => {
                                                    "This folder is empty"
                                                }
                                                FilePickerPurpose::OpenProject => "No folders here",
                                            }
                                        };
                                        ui.label(
                                            RichText::new(message)
                                                .size(theme::typography::SMALL_SIZE)
                                                .color(theme::text_disabled()),
                                        );
                                    });
                                }
                                for (index, entry) in entries.iter().enumerate() {
                                    let selected = picker.selected.contains(&entry.path);
                                    let highlighted = picker.cursor == Some(index);
                                    let response =
                                        agent_file_picker_row(ui, entry, selected, highlighted);
                                    if highlighted && cursor_moved {
                                        response.scroll_to_me(None);
                                    }
                                    if response.clicked() {
                                        if entry.is_dir {
                                            navigate = Some(entry.path.clone());
                                        } else {
                                            toggle_file = Some(entry.path.clone());
                                        }
                                    }
                                }
                                ui.add_space(theme::space::SNUG);
                            });
                    },
                );

                // The address bar already shows where you are, so the footer
                // only speaks up for errors and attach counts.
                let status = picker.error.clone().or_else(|| match purpose {
                    FilePickerPurpose::AttachFiles => Some(format!(
                        "{} selected  ·  {remaining} available",
                        picker.selected.len()
                    )),
                    FilePickerPurpose::OpenProject => None,
                });
                let status_color = if picker.error.is_some() {
                    theme::ink(theme::semantic().danger)
                } else {
                    theme::text().muted
                };
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(
                            footer
                                .shrink2(egui::vec2(theme::space::LARGE, 10.0))
                                .with_max_x(footer.right() - 240.0),
                        )
                        .layout(Layout::left_to_right(Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = theme::space::MEDIUM;
                        ui.shrink_clip_rect(ui.max_rect());
                        if file_picker_check_row(ui, "Hidden files", picker.show_hidden)
                            .on_hover_text("Show entries that start with a dot")
                            .clicked()
                        {
                            picker.show_hidden = !picker.show_hidden;
                            picker.cursor = None;
                        }
                        if let Some(status) = &status {
                            ui.label(
                                RichText::new(status)
                                    .size(theme::typography::MICRO_SIZE)
                                    .color(status_color),
                            );
                        }
                    },
                );
                ui.scope_builder(
                    UiBuilder::new()
                        .max_rect(footer.shrink2(egui::vec2(theme::space::MEDIUM, 10.0)))
                        .layout(Layout::right_to_left(Align::Center)),
                    |ui| {
                        ui.spacing_mut().item_spacing.x = theme::space::SMALL;
                        let (label, enabled) = match purpose {
                            FilePickerPurpose::AttachFiles => (
                                match picker.selected.len() {
                                    1 => "Add 1 file".to_owned(),
                                    count => format!("Add {count} files"),
                                },
                                !picker.selected.is_empty(),
                            ),
                            FilePickerPurpose::OpenProject => (
                                picker.directory.file_name().map_or_else(
                                    || "Open folder".to_owned(),
                                    |name| {
                                        format!(
                                            "Open {}",
                                            crate::dialog::middle_truncate(
                                                &name.to_string_lossy(),
                                                18
                                            )
                                        )
                                    },
                                ),
                                true,
                            ),
                        };
                        let confirmed = ui
                            .add_enabled(
                                enabled,
                                egui::Button::new(
                                    RichText::new(label).strong().color(theme::text().on_accent),
                                )
                                .fill(theme::accent())
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(theme::corner(theme::radius::CONTROL))
                                .min_size(egui::vec2(112.0, theme::control::STANDARD)),
                            )
                            .clicked();
                        match purpose {
                            FilePickerPurpose::AttachFiles => attach |= confirmed,
                            FilePickerPurpose::OpenProject => open_directory |= confirmed,
                        }
                        close |= ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Cancel").color(theme::text().primary),
                                )
                                .fill(theme::composite(
                                    theme::state::hover(),
                                    theme::surface().raised,
                                ))
                                .stroke(theme::border::hairline())
                                .corner_radius(theme::corner(theme::radius::CONTROL))
                                .min_size(egui::vec2(84.0, theme::control::STANDARD)),
                            )
                            .clicked();
                    },
                );
            });
        if modal.backdrop_response.clicked() {
            close = true;
        }

        if let Some(directory) = navigate {
            let _ = picker.navigate(directory);
        } else if go_back {
            picker.go_back();
        } else if go_forward {
            picker.go_forward();
        } else if refresh {
            picker.reload();
        }
        if query_changed {
            picker.cursor = None;
        }
        if let Some(path) = toggle_file {
            if picker.selected.contains(&path)
                || picker.selected.len() + attached_count < MAX_PROMPT_ATTACHMENTS
            {
                picker.toggle(path);
            } else {
                picker.error = Some(format!("Attach at most {MAX_PROMPT_ATTACHMENTS} files"));
            }
        }
        if attach {
            let mut paths = picker.selected.iter().cloned().collect::<Vec<_>>();
            paths.sort();
            return Some(FilePickerOutcome::AttachFiles(paths));
        }
        if open_directory {
            return Some(FilePickerOutcome::OpenDirectory(picker.directory.clone()));
        }
        if close {
            return Some(FilePickerOutcome::Dismissed);
        }
        None
    }
}
