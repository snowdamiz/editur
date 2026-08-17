use super::*;

impl EditorApp {
    pub(super) fn server_launch(&self, preset: PresetId) -> ServerLaunch {
        if !self.settings.language_servers.enabled {
            return ServerLaunch::Off;
        }
        match self.settings.language_servers.servers.get(preset.as_str()) {
            None => ServerLaunch::Auto,
            Some(override_) if override_.mode == ServerMode::Off => ServerLaunch::Off,
            Some(override_) if override_.mode == ServerMode::Custom => ServerLaunch::Custom {
                command: override_.command.clone().unwrap_or_default(),
                args: override_.args.clone(),
            },
            Some(_) => ServerLaunch::Auto,
        }
    }

    #[cfg(test)]
    pub(super) fn ensure_lsp_controller(&mut self, _preset: PresetId, _ctx: &egui::Context) {}

    #[cfg(not(test))]
    pub(super) fn ensure_lsp_controller(&mut self, preset: PresetId, ctx: &egui::Context) {
        if self.lsp_controllers.contains_key(&preset) {
            return;
        }
        let descriptor = lsp_catalog()
            .iter()
            .find(|candidate| candidate.id == preset)
            .unwrap();
        let launch = self.server_launch(preset);
        let wake = ctx.clone();
        self.lsp_controllers.insert(
            preset,
            LspController::start(
                self.tree.root.clone(),
                descriptor,
                launch,
                Arc::new(move || wake.request_repaint()),
            ),
        );
    }

    pub(super) fn restart_lsp(&mut self, preset: PresetId) {
        self.send_lsp_control(preset, LspCommand::Restart(self.server_launch(preset)));
    }

    pub(super) fn send_lsp_control(&mut self, preset: PresetId, command: LspCommand) {
        let sent = self
            .lsp_controllers
            .get(&preset)
            .is_none_or(|controller| controller.send(command.clone()).is_ok());
        if sent {
            self.lsp_pending_controls.remove(&preset);
        } else {
            self.lsp_pending_controls.insert(preset, command);
            self.lsp_sync_needed = true;
        }
    }

    pub(super) fn sync_lsp_documents(&mut self, ctx: &egui::Context) {
        self.lsp_sync_needed = false;
        for (preset, command) in self
            .lsp_pending_controls
            .iter()
            .map(|(preset, command)| (*preset, command.clone()))
            .collect::<Vec<_>>()
        {
            let sent = self
                .lsp_controllers
                .get(&preset)
                .is_some_and(|controller| controller.send(command).is_ok());
            if sent {
                self.lsp_pending_controls.remove(&preset);
            } else {
                self.lsp_sync_needed = true;
            }
        }
        let desired = self
            .tabs
            .iter()
            .enumerate()
            .filter(|tab| {
                tab.1.git_diff.is_none()
                    && !tab.1.buffer.large_file_warning
                    && tab.1.buffer.text.len() <= LARGE_FILE_BYTES
            })
            .filter_map(|(index, tab)| {
                let (preset, language_id) = preset_for_path(&tab.buffer.path)?;
                Some((
                    index,
                    tab.buffer.path.clone(),
                    preset.id,
                    language_id.to_owned(),
                    tab.buffer.revision,
                ))
            })
            .collect::<Vec<_>>();
        let desired_paths = desired
            .iter()
            .map(|(_, path, ..)| path.clone())
            .collect::<HashSet<_>>();
        let closed = self
            .lsp_open
            .keys()
            .filter(|path| !desired_paths.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        for path in closed {
            let Some((preset, _)) = self.lsp_open.get(&path).copied() else {
                continue;
            };
            if self
                .lsp_controllers
                .get(&preset)
                .is_some_and(|controller| controller.send(LspCommand::Close(path.clone())).is_ok())
            {
                self.lsp_open.remove(&path);
                self.lsp_pending_saves.remove(&path);
                self.lsp_diagnostics.remove(&path);
            } else {
                self.lsp_sync_needed = true;
            }
        }
        if !self.settings.language_servers.enabled {
            return;
        }
        for (index, path, preset, language_id, revision) in desired {
            if self.server_mode(preset) == ServerMode::Off {
                continue;
            }
            if let Some((known_preset, known_revision)) = self.lsp_open.get_mut(&path) {
                if *known_preset == preset
                    && *known_revision != revision
                    && let Some(controller) = self.lsp_controllers.get(&preset)
                {
                    if controller
                        .send(LspCommand::Change {
                            path: path.clone(),
                            text: self.tabs[index].buffer.text.clone(),
                            revision,
                        })
                        .is_ok()
                    {
                        *known_revision = revision;
                    } else {
                        self.lsp_sync_needed = true;
                    }
                }
                continue;
            }
            self.ensure_lsp_controller(preset, ctx);
            if let Some(controller) = self.lsp_controllers.get(&preset)
                && controller
                    .send(LspCommand::Open(crate::lsp::DocumentSnapshot {
                        path: path.clone(),
                        language_id,
                        text: self.tabs[index].buffer.text.clone(),
                        revision,
                    }))
                    .is_ok()
            {
                self.lsp_open.insert(path, (preset, revision));
            } else {
                self.lsp_sync_needed = true;
            }
        }
        let saves = self.lsp_pending_saves.iter().cloned().collect::<Vec<_>>();
        for path in saves {
            let sent = self.lsp_open.get(&path).is_some_and(|(preset, _)| {
                self.lsp_controllers.get(preset).is_some_and(|controller| {
                    controller.send(LspCommand::Save(path.clone())).is_ok()
                })
            });
            if sent || !desired_paths.contains(&path) {
                self.lsp_pending_saves.remove(&path);
            } else {
                self.lsp_sync_needed = true;
            }
        }
        self.send_pending_lsp_requests(ctx);
        if self.lsp_sync_needed {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
    }

    pub(super) fn send_pending_lsp_requests(&mut self, ctx: &egui::Context) {
        if let Some((tag, trigger)) = self.lsp_pending_completion.clone() {
            if !self.tag_matches_cursor(&tag) {
                self.lsp_pending_completion = None;
            } else {
                match self.send_lsp_feature(
                    &tag,
                    |capabilities| capabilities.completion,
                    LspCommand::Complete {
                        tag: tag.clone(),
                        trigger,
                    },
                ) {
                    LspFeatureSend::Sent | LspFeatureSend::Unsupported => {
                        self.lsp_pending_completion = None;
                    }
                    LspFeatureSend::Retry => {
                        self.lsp_sync_needed = true;
                        ctx.request_repaint_after(Duration::from_millis(50));
                    }
                    LspFeatureSend::Waiting => {}
                }
            }
        }
        if let Some(tag) = self.lsp_pending_hover.clone() {
            let current = self
                .lsp_hover_probe
                .as_ref()
                .is_some_and(|probe| probe.tag == tag);
            if !current {
                self.lsp_pending_hover = None;
            } else {
                match self.send_lsp_feature(
                    &tag,
                    |capabilities| capabilities.hover,
                    LspCommand::Hover(tag.clone()),
                ) {
                    LspFeatureSend::Sent | LspFeatureSend::Unsupported => {
                        self.lsp_pending_hover = None;
                    }
                    LspFeatureSend::Retry => {
                        self.lsp_sync_needed = true;
                        ctx.request_repaint_after(Duration::from_millis(50));
                    }
                    LspFeatureSend::Waiting => {}
                }
            }
        }
        if let Some(tag) = self.lsp_pending_definition.clone() {
            if !self.tag_matches_cursor(&tag) {
                self.lsp_pending_definition = None;
            } else {
                match self.send_lsp_feature(
                    &tag,
                    |capabilities| capabilities.definition,
                    LspCommand::Definition(tag.clone()),
                ) {
                    LspFeatureSend::Sent | LspFeatureSend::Unsupported => {
                        self.lsp_pending_definition = None;
                    }
                    LspFeatureSend::Retry => {
                        self.lsp_sync_needed = true;
                        ctx.request_repaint_after(Duration::from_millis(50));
                    }
                    LspFeatureSend::Waiting => {}
                }
            }
        }
    }

    pub(super) fn send_lsp_feature(
        &self,
        tag: &RequestTag,
        supported: impl FnOnce(&ServerCapabilities) -> bool,
        command: LspCommand,
    ) -> LspFeatureSend {
        let Some((preset, _)) = self.lsp_open.get(&tag.path) else {
            return LspFeatureSend::Waiting;
        };
        let Some(status) = self.lsp_status.get(preset) else {
            return LspFeatureSend::Waiting;
        };
        let ServerStatus::Ready(capabilities) = status else {
            return if matches!(status, ServerStatus::NotStarted | ServerStatus::Starting) {
                LspFeatureSend::Waiting
            } else {
                LspFeatureSend::Unsupported
            };
        };
        if !supported(capabilities) {
            return LspFeatureSend::Unsupported;
        }
        match self.lsp_controllers.get(preset) {
            Some(controller) if controller.send(command).is_ok() => LspFeatureSend::Sent,
            Some(_) => LspFeatureSend::Retry,
            None => LspFeatureSend::Waiting,
        }
    }

    pub(super) fn active_request_tag(&self) -> Option<RequestTag> {
        let tab = self.active_tab.and_then(|index| self.tabs.get(index))?;
        Some(RequestTag {
            path: tab.buffer.path.clone(),
            revision: tab.buffer.revision,
            cursor: tab.editor_surface.cursor(),
        })
    }

    pub(super) fn tag_matches_cursor(&self, tag: &RequestTag) -> bool {
        self.active_request_tag().as_ref() == Some(tag)
    }

    pub(super) fn poll_lsp(&mut self, ctx: &egui::Context) {
        let cursor_context_changed = ctx.input(|input| {
            input.pointer.any_click()
                || input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::Cut
                            | egui::Event::Paste(_)
                            | egui::Event::Text(_)
                            | egui::Event::Ime(_)
                            | egui::Event::Key { pressed: true, .. }
                    )
                })
        });
        let presets = self.lsp_controllers.keys().copied().collect::<Vec<_>>();
        for preset in presets {
            let events = self.lsp_controllers[&preset]
                .events()
                .try_iter()
                .collect::<Vec<_>>();
            for event in events {
                match event {
                    crate::lsp::Event::StateChanged(status) => {
                        self.lsp_detail.remove(&preset);
                        if matches!(status, ServerStatus::Ready(_)) {
                            self.lsp_sync_needed = true;
                        }
                        if self.lsp_pending_completion.is_some()
                            || self.lsp_pending_hover.is_some()
                            || self.lsp_pending_definition.is_some()
                        {
                            self.lsp_sync_needed = true;
                        }
                        self.lsp_status.insert(preset, status);
                    }
                    crate::lsp::Event::ServerMessage(message) => {
                        self.lsp_detail.insert(preset, message);
                    }
                    crate::lsp::Event::ProcessExited { error, stderr } => {
                        self.lsp_status.insert(preset, ServerStatus::Failed(error));
                        if !stderr.is_empty() {
                            self.lsp_detail.insert(preset, stderr);
                        }
                        for (path, (owner, _)) in &self.lsp_open {
                            if *owner == preset
                                && let Some(diagnostics) = self.lsp_diagnostics.get_mut(path)
                            {
                                diagnostics.stale = true;
                                self.lsp_generation = self.lsp_generation.wrapping_add(1);
                                diagnostics.generation = self.lsp_generation;
                            }
                        }
                        if self.lsp_completion.as_ref().is_some_and(|popup| {
                            preset_for_path(&popup.tag.path)
                                .is_some_and(|(owner, _)| owner.id == preset)
                        }) {
                            self.lsp_completion = None;
                        }
                        if self.lsp_hover.as_ref().is_some_and(|popup| {
                            preset_for_path(&popup.tag.path)
                                .is_some_and(|(owner, _)| owner.id == preset)
                        }) {
                            self.lsp_hover = None;
                            self.lsp_hover_probe = None;
                        }
                    }
                    crate::lsp::Event::Diagnostics {
                        path,
                        revision,
                        diagnostics,
                        ..
                    } => {
                        if self
                            .tabs
                            .iter()
                            .any(|tab| tab.buffer.path == path && tab.buffer.revision == revision)
                        {
                            let mut line_markers = HashMap::new();
                            for diagnostic in &diagnostics {
                                if diagnostic.range.is_empty() {
                                    line_markers
                                        .entry(diagnostic.line as usize)
                                        .or_insert(diagnostic.severity);
                                }
                            }
                            self.lsp_generation = self.lsp_generation.wrapping_add(1);
                            self.lsp_diagnostics.insert(
                                path,
                                LspDiagnosticsState {
                                    revision,
                                    stale: false,
                                    generation: self.lsp_generation,
                                    diagnostics,
                                    line_markers,
                                },
                            );
                        }
                    }
                    crate::lsp::Event::DiagnosticsStale(path) => {
                        if let Some(diagnostics) = self.lsp_diagnostics.get_mut(&path) {
                            diagnostics.stale = true;
                            self.lsp_generation = self.lsp_generation.wrapping_add(1);
                            diagnostics.generation = self.lsp_generation;
                        }
                    }
                    crate::lsp::Event::Completion {
                        tag,
                        items,
                        truncated,
                    } => {
                        if !cursor_context_changed
                            && self.tag_matches_cursor(&tag)
                            && let Some(caret) =
                                self.lsp_caret.as_ref().filter(|caret| caret.tag == tag)
                        {
                            if truncated {
                                self.lsp_detail.insert(
                                    preset,
                                    "Completion results were truncated to 500 items".into(),
                                );
                            }
                            self.lsp_completion = (!items.is_empty()).then_some(CompletionPopup {
                                tag,
                                items,
                                selected: 0,
                                anchor: caret.rect,
                                bounds: caret.bounds,
                            });
                        }
                    }
                    crate::lsp::Event::Hover { tag, content } => {
                        if !cursor_context_changed
                            && self
                                .lsp_hover_probe
                                .as_ref()
                                .is_some_and(|probe| probe.tag == tag)
                            && let Some(content) = content
                        {
                            let probe = self.lsp_hover_probe.as_ref().unwrap();
                            let markdown = content.markdown.then(|| {
                                markdown::compact_layout(&content.text, 400.0, |_, _| None)
                            });
                            self.lsp_hover = Some(HoverPopup {
                                tag,
                                pointer: probe.pointer,
                                bounds: probe.bounds,
                                content,
                                markdown,
                            });
                        }
                    }
                    crate::lsp::Event::Definitions {
                        tag,
                        locations,
                        truncated,
                    } => {
                        if !cursor_context_changed && self.tag_matches_cursor(&tag) {
                            if truncated {
                                self.lsp_detail.insert(
                                    preset,
                                    "Definition results were truncated to 200 locations".into(),
                                );
                            }
                            match locations.len() {
                                0 => {
                                    self.show_error("No valid local definition was returned".into())
                                }
                                1 => self
                                    .navigate_to_definition(locations.into_iter().next().unwrap()),
                                _ => {
                                    self.lsp_definitions = Some(DefinitionChooser {
                                        locations,
                                        selected: 0,
                                    })
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    pub(super) fn draw_lsp_popups(&mut self, root: &mut egui::Ui) {
        self.draw_lsp_hover(root);
        self.draw_lsp_completion(root);
        self.draw_lsp_definitions(root);
    }

    pub(super) fn draw_lsp_completion(&mut self, root: &mut egui::Ui) {
        let Some(popup) = self.lsp_completion.as_ref() else {
            return;
        };
        let row_count = popup.items.len().min(12);
        let start = popup
            .selected
            .saturating_sub(11)
            .min(popup.items.len().saturating_sub(row_count));
        let rows = popup.items[start..start + row_count]
            .iter()
            .enumerate()
            .map(|(offset, item)| {
                (
                    start + offset,
                    item.label.clone(),
                    completion_kind_label(item.kind),
                    item.detail.clone(),
                )
            })
            .collect::<Vec<_>>();
        let selected = popup.selected;
        let width = 480.0_f32.min(popup.bounds.width().max(1.0));
        let position = popup_position(
            popup.anchor.left_bottom() + egui::vec2(0.0, 4.0),
            egui::vec2(width, row_count as f32 * 34.0 + 12.0),
            popup.bounds,
        );
        let mut clicked = None;
        egui::Area::new(Id::new("lsp_completion"))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::surface().raised)
                    .stroke(egui::Stroke::new(1.0, theme::border::strong_color()))
                    .corner_radius(7)
                    .inner_margin(egui::Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(width - 12.0);
                        for (index, label, kind, detail) in &rows {
                            let response =
                                selectable_content_row(ui, *index == selected, 22.0, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new(label)
                                                .monospace()
                                                .color(theme::text().primary),
                                        );
                                        if let Some(kind) = kind {
                                            ui.label(
                                                RichText::new(*kind)
                                                    .small()
                                                    .color(theme::text().muted),
                                            );
                                        }
                                        if let Some(detail) = detail {
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    ui.label(
                                                        RichText::new(detail)
                                                            .small()
                                                            .color(theme::text().secondary),
                                                    );
                                                },
                                            );
                                        }
                                    });
                                });
                            if response.clicked() {
                                clicked = Some(*index);
                            }
                        }
                    });
            });
        if let Some(index) = clicked {
            if let Some(popup) = self.lsp_completion.as_mut() {
                popup.selected = index;
            }
            self.accept_completion();
        }
    }

    pub(super) fn draw_lsp_hover(&self, root: &mut egui::Ui) {
        if self.lsp_completion.is_some() {
            return;
        }
        let Some(popup) = self.lsp_hover.as_ref() else {
            return;
        };
        let width = 420.0_f32.min(popup.bounds.width().max(1.0));
        let position = popup_position(
            popup.pointer + egui::vec2(12.0, 16.0),
            egui::vec2(width, 300.0_f32.min(popup.bounds.height())),
            popup.bounds,
        );
        egui::Area::new(Id::new("lsp_hover"))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::surface().raised)
                    .stroke(egui::Stroke::new(1.0, theme::border::strong_color()))
                    .corner_radius(7)
                    .inner_margin(egui::Margin::same(10))
                    .show(ui, |ui| {
                        ui.set_width(width - 20.0);
                        ScrollArea::vertical().max_height(280.0).show(ui, |ui| {
                            if let Some(job) = &popup.markdown {
                                ui.add(Label::new(job.clone()).wrap());
                            } else {
                                ui.label(&popup.content.text);
                            }
                        });
                    });
            });
    }

    pub(super) fn draw_lsp_definitions(&mut self, root: &mut egui::Ui) {
        let Some(chooser) = self.lsp_definitions.as_ref() else {
            return;
        };
        let selected = chooser.selected;
        let locations = chooser.locations.clone();
        let screen = root.ctx().content_rect();
        let size = egui::vec2(
            620.0_f32.min(screen.width()),
            420.0_f32.min(screen.height()),
        );
        let position = egui::pos2(screen.center().x - size.x / 2.0, screen.top() + 48.0);
        let mut clicked = None;
        egui::Area::new(Id::new("lsp_definitions"))
            .order(egui::Order::Foreground)
            .fixed_pos(position)
            .show(root.ctx(), |ui| {
                egui::Frame::new()
                    .fill(theme::surface().raised)
                    .stroke(egui::Stroke::new(1.0, theme::border::strong_color()))
                    .corner_radius(10)
                    .inner_margin(egui::Margin::same(12))
                    .show(ui, |ui| {
                        ui.set_width(size.x - 24.0);
                        ui.label(
                            RichText::new("Go to definition")
                                .size(theme::typography::TITLE_SIZE)
                                .strong(),
                        );
                        ui.label(
                            RichText::new("Up/Down navigate   Enter open   Esc close")
                                .small()
                                .color(theme::text().muted),
                        );
                        ui.add_space(8.0);
                        ScrollArea::vertical()
                            .max_height(size.y - 72.0)
                            .show(ui, |ui| {
                                for (index, location) in locations.iter().enumerate() {
                                    let label = format!(
                                        "{}:{}:{}",
                                        location.path.display(),
                                        location.line + 1,
                                        location.character + 1
                                    );
                                    let response =
                                        selectable_content_row(ui, index == selected, 24.0, |ui| {
                                            ui.label(
                                                RichText::new(&label)
                                                    .monospace()
                                                    .color(theme::text().primary),
                                            );
                                        });
                                    if response.clicked() {
                                        clicked = Some(index);
                                    }
                                }
                            });
                    });
            });
        if let Some(index) = clicked {
            self.lsp_definitions = None;
            self.navigate_to_definition(locations[index].clone());
        }
    }
}
