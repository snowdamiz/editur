use super::*;

impl EditorApp {
    pub(super) fn draw_agent_sidebar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, theme::surface().chrome);
        self.draw_agent(ui, rect);
    }

    pub(super) fn open_agent(&mut self, ctx: &egui::Context) {
        self.warm_providers(ctx);
    }

    pub(super) fn draw_agent_find(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        if self.agent_find.dirty {
            self.agent_find.matches = agent_search_matches(
                &self.agent.transcript,
                &self.agent.changed_paths,
                &self.agent_find.query,
            );
            self.agent_find.selected = self
                .agent_find
                .selected
                .min(self.agent_find.matches.len().saturating_sub(1));
            self.agent_find.dirty = false;
        }
        let count = if self.agent_find.matches.is_empty() {
            "0 / 0".to_owned()
        } else {
            format!(
                "{} / {}",
                self.agent_find.selected + 1,
                self.agent_find.matches.len()
            )
        };
        let query_id = Id::new("agent_find_query");
        let focused = ui.memory(|memory| memory.has_focus(query_id));
        let (enter, backwards, escape) = ui.input(|input| {
            (
                focused && input.key_pressed(Key::Enter),
                input.modifiers.shift,
                input.key_pressed(Key::Escape),
            )
        });
        let mut previous = false;
        let mut next = false;
        let mut close = escape;
        let mut changed = false;
        ui.painter().rect_filled(rect, 0.0, theme::surface().chrome);
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, theme::border::hairline_color()),
        );
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("agent_find")
                .max_rect(rect.shrink2(egui::vec2(8.0, 5.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                let controls = 3.0 * theme::control::STANDARD + 54.0;
                let response = ui.add_sized(
                    [
                        (ui.available_width() - controls).max(64.0),
                        theme::control::COMPACT + 2.0,
                    ],
                    TextEdit::singleline(&mut self.agent_find.query)
                        .id(query_id)
                        .hint_text("Find in conversation…")
                        .frame(egui::Frame::NONE),
                );
                changed = response.changed();
                ui.label(
                    RichText::new(count)
                        .size(theme::typography::MICRO_SIZE)
                        .color(theme::text().muted),
                );
                previous = chevron_icon_button(ui, true, "Previous match (Shift+Enter)").clicked();
                next = chevron_icon_button(ui, false, "Next match (Enter)").clicked();
                close |= close_icon_button(ui).clicked();
            },
        );
        if self.agent_find.focus {
            ui.memory_mut(|memory| memory.request_focus(query_id));
            self.agent_find.focus = false;
        }
        if changed {
            self.agent_find.matches = agent_search_matches(
                &self.agent.transcript,
                &self.agent.changed_paths,
                &self.agent_find.query,
            );
            self.agent_find.selected = 0;
            self.agent_find.scroll_to_match = !self.agent_find.matches.is_empty();
            self.agent_find.dirty = false;
        }
        if enter {
            if backwards {
                previous = true;
            } else {
                next = true;
            }
        }
        if !self.agent_find.matches.is_empty() && (previous || next) {
            self.agent_find.selected = next_find_match(
                self.agent_find.selected,
                self.agent_find.matches.len(),
                previous,
            );
            self.agent_find.scroll_to_match = true;
        }
        if close {
            self.agent_find.open = false;
            self.agent_find.focus = false;
        }
    }

    pub(super) fn ensure_provider_catalog(&mut self) {
        if !self.available_providers.is_empty() {
            return;
        }
        self.available_providers = crate::agent::provision::embedded_bundle()
            .map(|bundle| bundle.available())
            .unwrap_or_else(|_| vec![ProviderId::Cursor]);
        self.selected_provider = data_dir().map_or(ProviderId::Cursor, |directory| {
            crate::agent::provider::load_selected(&directory, &self.available_providers)
        });
    }

    pub(super) fn warm_providers(&mut self, ctx: &egui::Context) {
        self.ensure_provider_catalog();
        self.start_provider(self.selected_provider, ctx);
    }

    pub(super) fn start_provider(&mut self, provider: ProviderId, ctx: &egui::Context) {
        if self.agent_controllers.contains_key(&provider) {
            return;
        }
        let state = if provider == self.selected_provider {
            &mut self.agent
        } else {
            self.provider_agents.entry(provider).or_default()
        };
        state.session_ready = false;
        state.active = false;
        state.connection = ConnectionState::Starting;
        let preferred_session = state.session_id.clone();
        let wake = ctx.clone();
        self.agent_controllers.insert(
            provider,
            AgentController::start_with_wake(
                provider,
                self.tree.root.clone(),
                preferred_session,
                move || wake.request_repaint(),
            ),
        );
    }

    pub(super) fn reconnect_agent(&mut self, ctx: &egui::Context) {
        if let Some(controller) = self.agent_controllers.remove(&self.selected_provider) {
            drop(controller);
        }
        let provider = self.selected_provider;
        self.start_provider(provider, ctx);
    }

    pub(super) fn request_provider_switch(&mut self, target: ProviderId, ctx: &egui::Context) {
        if target == self.selected_provider || !self.available_providers.contains(&target) {
            return;
        }
        if let Some(controller) = self.agent_controllers.remove(&self.selected_provider) {
            drop(controller);
        }
        self.select_provider_state(target);
        if let Ok(directory) = data_dir()
            && let Err(error) = crate::agent::provider::save_selected(&directory, target)
        {
            self.show_error(error);
        }
        self.agent_menu = None;
        self.agent_menu_popup = None;
        self.agent_prompt_history_index = None;
        self.agent_prompt_history_draft.clear();
        self.agent_follow_transcript = true;
        self.agent_find.dirty = true;
        self.agent_file_picker = None;
        self.agent_run_everything = None;
        self.start_provider(target, ctx);
    }

    pub(super) fn select_provider_state(&mut self, target: ProviderId) {
        if target == self.selected_provider {
            return;
        }
        let draft = std::mem::take(&mut self.agent.prompt);
        let mut next = self.provider_agents.remove(&target).unwrap_or_default();
        next.prompt = draft;
        let previous = std::mem::replace(&mut self.agent, next);
        self.provider_agents
            .insert(self.selected_provider, previous);
        self.selected_provider = target;
    }

    pub(super) fn poll_agent(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        for (&provider, controller) in &self.agent_controllers {
            for _ in 0..8 {
                let Ok(event) = controller.events().try_recv() else {
                    break;
                };
                events.push((provider, event));
            }
        }
        if events.len() >= 8 {
            ctx.request_repaint();
        }
        for (provider, event) in events {
            let selected = provider == self.selected_provider;
            if selected {
                self.agent_find.dirty = true;
            }
            if selected
                && matches!(
                    event,
                    AgentEvent::SessionReady { .. }
                        | AgentEvent::SessionLoaded { .. }
                        | AgentEvent::SessionTranscriptStarted
                )
            {
                self.agent_follow_transcript = true;
                self.agent_prompt_history_index = None;
                self.agent_prompt_history_draft.clear();
                self.agent_attachments.clear();
                self.agent_file_picker = None;
            }
            if selected && matches!(event, AgentEvent::UserMessage(_)) {
                self.agent_attachments.clear();
            }
            if selected
                && self.agent.allow_run_everything
                && let AgentEvent::CommandsUpdated(commands) = &event
                && let Some(enabled) = run_everything_state(commands)
            {
                self.agent_run_everything = Some(enabled);
            }
            if selected
                && matches!(
                    &event,
                    AgentEvent::Capabilities {
                        allow_run_everything: false,
                        ..
                    }
                )
            {
                self.agent_run_everything = None;
            }
            let reconcile_path = match &event {
                AgentEvent::ToolCallUpdated(tool) => self.tabs.iter().any(|tab| {
                    tool.paths
                        .iter()
                        .any(|tool_path| tool_path.path == tab.buffer.path)
                }),
                _ => false,
            };
            let turn_finished = matches!(event, AgentEvent::TurnFinished { .. });
            let refresh_project =
                turn_finished || matches!(event, AgentEvent::ProcessExited { .. });
            if selected {
                self.agent.apply(event);
            } else {
                self.provider_agents
                    .entry(provider)
                    .or_default()
                    .apply(event);
            }
            if reconcile_path || turn_finished {
                self.reconcile_open_buffer();
                self.refresh_agentic_diff();
            }
            if refresh_project {
                self.refresh_after_agent(provider);
            }
        }
    }

    pub(super) fn reconcile_open_buffer(&mut self) {
        let mut active_reloaded = false;
        let mut conflict = None;
        let mut error = None;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            match reconcile_buffer(&mut tab.buffer) {
                Ok(ReconcileOutcome::Unchanged) => {}
                Ok(ReconcileOutcome::Reloaded) => {
                    if let Some((_, revision)) = self.lsp_open.get_mut(&tab.buffer.path) {
                        *revision = u64::MAX;
                    }
                    if let Some(diagnostics) = self.lsp_diagnostics.get_mut(&tab.buffer.path) {
                        self.lsp_generation = self.lsp_generation.wrapping_add(1);
                        diagnostics.stale = true;
                        diagnostics.generation = self.lsp_generation;
                    }
                    let cursor = tab.editor_surface.cursor();
                    tab.editor_surface = EditorSurface::default();
                    tab.editor_surface.set_selection(cursor, cursor);
                    tab.highlight_cache = HighlightCache::default();
                    tab.markdown_layout = None;
                    active_reloaded |= self.active_tab == Some(index);
                    self.lsp_sync_needed = true;
                    self.lsp_completion = None;
                    self.lsp_hover = None;
                    self.lsp_hover_probe = None;
                }
                Ok(ReconcileOutcome::Conflict) => {
                    conflict.get_or_insert(index);
                }
                Err(found) if error.is_none() => error = Some(found),
                Err(_) => {}
            };
        }
        if active_reloaded {
            self.pane_find
                .values_mut()
                .for_each(|find| find.match_revision = u64::MAX);
            self.bracket_pair = None;
            self.bracket_pair_key = None;
        }
        if let Some(index) = conflict {
            self.activate_tab(index);
            self.conflict = true;
        }
        if let Some(error) = error
            && self.error.is_none()
        {
            self.show_error(error);
        }
    }

    pub(super) fn refresh_after_agent(&mut self, provider: ProviderId) {
        if provider == self.selected_provider {
            self.git_workspace_status_started = false;
            self.git_workspace_status_rx = None;
        }
        let changed = if provider == self.selected_provider {
            std::mem::take(&mut self.agent.refresh_queue)
        } else {
            std::mem::take(
                &mut self
                    .provider_agents
                    .entry(provider)
                    .or_default()
                    .refresh_queue,
            )
        };
        let directories = if changed.is_empty() {
            self.tree.children.keys().cloned().collect::<HashSet<_>>()
        } else {
            changed
                .iter()
                .filter_map(|path| path.parent().map(Path::to_path_buf))
                .collect()
        };
        for directory in directories {
            let error = match self.tree.children.entry(directory) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    match read_directory(entry.key()) {
                        Ok(entries) => {
                            entry.insert(entries);
                            None
                        }
                        Err(error) => Some(error),
                    }
                }
                std::collections::hash_map::Entry::Vacant(_) => None,
            };
            if let Some(error) = error
                && self.error.is_none()
            {
                self.show_error(error);
            }
        }
        self.tree.refresh_visible();
        if !changed.is_empty() {
            self.agent_mentions = None;
            match SearchController::new(self.tree.root.clone()) {
                Ok(search) => {
                    self.search = search;
                    if !self.search_query.trim().is_empty() {
                        let _ = self.search.set_query(&self.search_query);
                    }
                }
                Err(error) => self.show_error(error),
            }
        }
    }

    pub(super) fn queue_agent_prompt(&mut self) {
        if self.buffer().is_some_and(|buffer| buffer.dirty) {
            self.pending_agent_prompt = true;
        } else {
            self.send_agent_prompt();
        }
    }

    pub(super) fn send_agent_prompt(&mut self) {
        let Some(controller) = self.agent_controllers.get(&self.selected_provider) else {
            return;
        };
        let prompt = self.agent.prompt.trim().to_owned();
        if (prompt.is_empty() && self.agent_attachments.is_empty())
            || self.agent.active
            || !self.agent.session_ready
        {
            return;
        }
        self.agent.active = true;
        match controller.send(AgentCommand::PromptWithAttachments {
            text: prompt,
            attachments: self
                .agent_attachments
                .iter()
                .map(|attachment| attachment.file.clone())
                .collect(),
        }) {
            Ok(()) => {
                self.agent_prompt_history_index = None;
                self.agent_prompt_history_draft.clear();
            }
            Err(error) => {
                self.agent.active = false;
                self.show_error(error);
            }
        }
    }

    pub(super) fn attach_agent_files(
        &mut self,
        ctx: &egui::Context,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        for path in paths {
            if self.agent_attachments.len() >= MAX_PROMPT_ATTACHMENTS {
                self.show_error(format!("attach at most {MAX_PROMPT_ATTACHMENTS} items"));
                break;
            }
            let attachment = match PromptAttachment::from_path(path) {
                Ok(attachment) => attachment,
                Err(error) => {
                    self.show_error(error);
                    continue;
                }
            };
            if self
                .agent_attachments
                .iter()
                .any(|attached| attached.file.path() == attachment.path())
            {
                continue;
            }
            let total = self
                .agent_attachments
                .iter()
                .map(|attached| attached.file.byte_len())
                .sum::<u64>()
                .saturating_add(attachment.byte_len());
            if total > MAX_PROMPT_ATTACHMENT_TOTAL_BYTES {
                self.show_error(format!(
                    "attached files must total no more than {} MiB",
                    MAX_PROMPT_ATTACHMENT_TOTAL_BYTES / 1024 / 1024
                ));
                break;
            }
            let thumbnail = load_agent_thumbnail(ctx, &attachment);
            self.agent_attachments.push(AgentComposerAttachment {
                file: attachment,
                thumbnail,
            });
        }
    }

    pub(super) fn open_agent_file_picker(&mut self) {
        let directory = self.tree.root.clone();
        let picker = self
            .tree
            .children
            .get(&directory)
            .cloned()
            .map(|entries| AgentFilePicker::with_entries(directory.clone(), entries))
            .map(Ok)
            .unwrap_or_else(|| AgentFilePicker::open(directory));
        match picker {
            Ok(picker) => self.agent_file_picker = Some(picker),
            Err(error) => self.show_error(error),
        }
    }

    pub(super) fn navigate_agent_prompt_history(&mut self, older: bool) -> bool {
        let history_len = self
            .agent
            .transcript
            .iter()
            .filter(|item| matches!(item, TranscriptItem::User(_)))
            .count();
        if history_len == 0 {
            return false;
        }
        let next = if older {
            if let Some(index) = self.agent_prompt_history_index {
                Some(index.saturating_sub(1))
            } else {
                self.agent_prompt_history_draft = self.agent.prompt.clone();
                Some(history_len - 1)
            }
        } else {
            match self.agent_prompt_history_index {
                Some(index) if index + 1 < history_len => Some(index + 1),
                Some(_) => None,
                None => return false,
            }
        };
        self.agent_prompt_history_index = next;
        self.agent.prompt = next.map_or_else(
            || self.agent_prompt_history_draft.clone(),
            |index| {
                self.agent
                    .transcript
                    .iter()
                    .filter_map(|item| match item {
                        TranscriptItem::User(prompt) => Some(prompt),
                        _ => None,
                    })
                    .nth(index)
                    .cloned()
                    .unwrap_or_default()
            },
        );
        true
    }

    pub(super) fn draw_agent(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Body, theme::typography::title());
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Small, theme::typography::small());
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Button, theme::typography::body());
        let font_id = egui::TextStyle::Body.resolve(ui.style());
        // The exact width the prompt is laid out at later, so the measured
        // text height matches what the composer actually shows.
        let prompt_width = if self.agentic_mode {
            (rect.width() - 2.0 * theme::space::WIDE).min(AGENTIC_CONTENT_WIDTH)
                - 2.0 * theme::space::MEDIUM
        } else {
            rect.width() - 2.0 * theme::space::MEDIUM
        };
        let (text_height, row_height) = ui.fonts_mut(|fonts| {
            let row_height = fonts.row_height(&font_id);
            let text_height = fonts
                .layout(
                    self.agent.prompt.clone(),
                    font_id,
                    Color32::WHITE,
                    prompt_width.max(24.0),
                )
                .size()
                .y;
            (text_height, row_height)
        });
        let attachment_height = if self.agent_attachments.is_empty() {
            0.0
        } else {
            AGENT_ATTACHMENT_ROW_HEIGHT
        };
        let composer_height = (agent_composer_height(text_height, row_height, rect.height())
            + attachment_height)
            .min(AGENT_COMPOSER_MAX_HEIGHT + attachment_height);
        let composer_height = if self.agentic_mode {
            composer_height + AGENTIC_COMPOSER_TOP_MARGIN + AGENTIC_COMPOSER_BOTTOM_MARGIN
        } else {
            composer_height
        };
        let (header, mut transcript, composer) = split_agent_sidebar(rect, composer_height);
        let menu_owns_wheel = self.agent_menu.is_some()
            && self.agent_menu_popup.is_some_and(|popup| {
                ui.input(|input| {
                    input
                        .pointer
                        .hover_pos()
                        .is_some_and(|pointer| popup.contains(pointer))
                })
            });
        let status = self.agent.connection.clone();
        let mut new_session = false;
        let mut session_menu_anchor = None;
        let mut session_menu_toggled = false;
        let mut provider_menu_toggled = false;
        let painter = ui.painter().clone();
        painter.rect_filled(header, 0.0, theme::surface().chrome);
        painter.hline(
            header.x_range(),
            header.bottom() - 0.5,
            egui::Stroke::new(1.0, theme::border::hairline_color()),
        );
        let project_title = self
            .tree
            .root
            .file_name()
            .unwrap_or(self.tree.root.as_os_str())
            .to_string_lossy();
        let title = self.agent.title.as_deref().unwrap_or(if self.agentic_mode {
            project_title.as_ref()
        } else {
            "Agent"
        });
        #[cfg(target_os = "macos")]
        let mut title_x = if self.agentic_mode && !self.sidebar {
            header.left() + 166.0
        } else {
            header.left() + 14.0
        };
        #[cfg(not(target_os = "macos"))]
        let mut title_x = if self.agentic_mode {
            header.left() + if self.sidebar { 76.0 } else { 90.0 }
        } else {
            header.left() + 14.0
        };
        if !self.agentic_mode && provider_selector_visible(&self.available_providers) {
            let provider_rect = egui::Rect::from_min_max(
                egui::pos2(header.left() + 10.0, header.top() + 3.0),
                egui::pos2(
                    (header.left() + 102.0).min(header.right()),
                    header.bottom() - 2.0,
                ),
            );
            ui.scope_builder(
                UiBuilder::new()
                    .id_salt("agent_provider_header")
                    .max_rect(provider_rect)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    let response =
                        draw_provider_selector_identity(ui, self.selected_provider, true);
                    self.provider_menu_anchor = Some(response.rect);
                    if response.clicked() {
                        let menu = AgentMenu::Providers;
                        self.agent_menu = (self.agent_menu.as_ref() != Some(&menu)).then_some(menu);
                        provider_menu_toggled = true;
                    }
                },
            );
            painter.vline(
                header.left() + 100.0,
                (header.center().y - 7.0)..=(header.center().y + 7.0),
                egui::Stroke::new(1.0, theme::border::strong_color()),
            );
            title_x = header.left() + 112.0;
        } else if !self.agentic_mode {
            self.provider_menu_anchor = None;
        }
        if !self.agentic_mode && self.agent.session_ready && self.agent.history_available {
            let button = agent_session_selector_rect(ui, header, title_x, title);
            let response = draw_agent_session_selector(ui, button, title);
            if response.clicked() {
                let menu = AgentMenu::Sessions;
                self.agent_menu = (self.agent_menu.as_ref() != Some(&menu)).then_some(menu.clone());
                session_menu_toggled = true;
                if self.agent_menu.is_some()
                    && let Some(controller) = self.agent_controllers.get(&self.selected_provider)
                {
                    let _ = controller.send(AgentCommand::RefreshSessions);
                }
            }
            if matches!(self.agent_menu, Some(AgentMenu::Sessions)) {
                session_menu_anchor = Some(button);
            }
        } else {
            painter.text(
                egui::pos2(title_x, header.center().y),
                Align2::LEFT_CENTER,
                title,
                theme::typography::body(),
                theme::text().primary,
            );
        }
        let agent_button = agent_toggle_rect(header);
        if !self.agentic_mode && self.draw_agent_toggle(ui, agent_button) {
            self.agent_sidebar = false;
            self.agent_sidebar_dragging = false;
            self.agent_menu = None;
            self.agent_file_picker = None;
            ui.ctx().request_repaint();
        }
        if !self.agentic_mode && self.agent.session_ready {
            let button = agent_new_session_rect(header);
            let response = ui
                .interact(button, Id::new("agent_new_session"), Sense::click())
                .on_hover_text("New Agent session");
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    "New Agent session",
                )
            });
            let icon_color = if response.hovered() {
                theme::text().primary
            } else {
                theme::text().secondary
            };
            painter.hline(
                (button.center().x - 5.0)..=(button.center().x + 5.0),
                button.center().y,
                egui::Stroke::new(1.3, icon_color),
            );
            painter.vline(
                button.center().x,
                (button.center().y - 5.0)..=(button.center().y + 5.0),
                egui::Stroke::new(1.3, icon_color),
            );
            new_session = response.clicked();
        }
        if self.agent_find.open {
            let find_bar = transcript
                .with_max_y((transcript.top() + AGENT_FIND_HEIGHT).min(transcript.bottom()));
            transcript = transcript.with_min_y(find_bar.bottom());
            self.draw_agent_find(ui, find_bar);
        }

        let mut reconnect = false;
        let mut authenticate = None;
        let mut permission_decisions = Vec::new();
        let mut interaction_responses = Vec::new();
        let mut open_path_request: Option<(PathBuf, Option<u32>)> = None;
        let mut open_diff_request: Option<PathBuf> = None;
        let mut open_image_request: Option<AgentImageSource> = None;
        let transcript_padding = if self.agentic_mode {
            ((transcript.width() - AGENTIC_CONTENT_WIDTH) * 0.5).max(28.0)
        } else {
            16.0
        };
        let transcript_content = transcript.shrink2(egui::vec2(transcript_padding, 0.0));
        let transcript_width = transcript_content.width();
        let transcript_region = if matches!(&status, ConnectionState::Ready)
            && (!self.agent.transcript.is_empty() || self.agent.active)
        {
            transcript
        } else {
            transcript_content
        };
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("agent_transcript_region")
                .max_rect(transcript_region)
                .layout(Layout::top_down(Align::LEFT)),
            |ui| match &status {
                ConnectionState::Provisioning { downloaded, total } => {
                    let downloaded_mib = downloaded / 1_048_576;
                    let detail = match total {
                        Some(total) => format!(
                            "Downloading {downloaded_mib} of {} MiB",
                            total / 1_048_576
                        ),
                        None => format!("Downloading {downloaded_mib} MiB…"),
                    };
                    draw_agent_connecting(
                        ui,
                        self.selected_provider,
                        &format!(
                            "Installing {} Agent",
                            provider_descriptor(self.selected_provider).display_name
                        ),
                        &detail,
                        total
                            .filter(|total| *total > 0)
                            .map(|total| *downloaded as f32 / total as f32),
                    );
                }
                ConnectionState::Starting => {
                    draw_agent_connecting(
                        ui,
                        self.selected_provider,
                        &format!(
                            "Starting {} Agent",
                            provider_descriptor(self.selected_provider).display_name
                        ),
                        "Connecting to this project…",
                        None,
                    );
                }
                ConnectionState::AuthenticationRequired(methods) => {
                    let environment_only = !methods.is_empty()
                        && methods.iter().all(|method| {
                            method.kind == AuthKind::Environment && !method.can_authenticate
                        });
                    egui::Frame::new()
                        .fill(theme::surface().raised)
                        .stroke(egui::Stroke::new(1.0, theme::border::hairline_color()))
                        .inner_margin(egui::Margin::same(14))
                        .corner_radius(7)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(format!(
                                    "Connect {}",
                                    provider_descriptor(self.selected_provider).display_name
                                ))
                                    .size(theme::typography::TITLE_SIZE)
                                    .strong()
                                    .color(theme::text().primary),
                            );
                            ui.add_space(3.0);
                            ui.add(
                                Label::new(
                                    RichText::new(if environment_only {
                                        "Configure API or supported commercial cloud credentials outside Editur, then retry this provider."
                                            .to_owned()
                                    } else {
                                        format!(
                                            "Sign in with your {} account to start an Agent session in this project.",
                                            provider_descriptor(self.selected_provider).display_name
                                        )
                                    })
                                    .color(theme::text().muted),
                                )
                                .wrap(),
                            );
                            ui.add_space(10.0);
                            for method in methods {
                                if method.can_authenticate {
                                    let button = egui::Button::new(
                                        RichText::new(if method.kind == AuthKind::Environment {
                                            "Use environment API key"
                                        } else {
                                            &method.name
                                        })
                                            .strong()
                                            .color(theme::text().on_accent),
                                    )
                                    .fill(theme::accent())
                                    .stroke(egui::Stroke::NONE)
                                    .corner_radius(5)
                                    .min_size(egui::vec2(ui.available_width(), 30.0));
                                    if ui.add(button).clicked() {
                                        authenticate = Some(method.id.clone());
                                    }
                                } else {
                                    ui.label(RichText::new(&method.name).strong());
                                }
                                if let Some(description) = &method.description {
                                    ui.add(
                                        Label::new(RichText::new(description).small().weak())
                                            .wrap(),
                                    );
                                }
                                let setup = match method.kind {
                                    AuthKind::Agent => None,
                                    AuthKind::Terminal => Some(
                                        "Complete the provider-owned sign-in in the terminal. Editur connects automatically when it finishes.",
                                    ),
                                    AuthKind::Environment => Some(
                                        "Set the required environment credentials before launching Editur again.",
                                    ),
                                    AuthKind::Unsupported => Some(
                                        "This authentication method is not supported in Editur.",
                                    ),
                                };
                                if let Some(setup) = setup {
                                    ui.add(Label::new(RichText::new(setup).small().weak()).wrap());
                                }
                                if let Some(details) = &method.setup {
                                    ui.add(
                                        Label::new(RichText::new(details).small().monospace().weak())
                                            .wrap(),
                                    );
                                }
                            }
                            if environment_only && ui.button("Retry").clicked() {
                                reconnect = true;
                            }
                        });
                }
                ConnectionState::Failed(error) => {
                    egui::Frame::new()
                        .fill(theme::callout(theme::semantic().danger).fill)
                        .stroke(egui::Stroke::new(1.0, theme::callout(theme::semantic().danger).border))
                        .inner_margin(egui::Margin::same(14))
                        .corner_radius(7)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new(format!(
                                    "{} Agent unavailable",
                                    provider_descriptor(self.selected_provider).display_name
                                ))
                                    .strong()
                                    .color(theme::ink(theme::semantic().danger)),
                            );
                            ui.add(Label::new(RichText::new(error).weak()).wrap());
                            if let Some(diagnostics) = &self.agent.diagnostics {
                                ui.add(
                                    Label::new(
                                        RichText::new(diagnostics).small().monospace().weak(),
                                    )
                                    .wrap(),
                                );
                            }
                            ui.add_space(8.0);
                            reconnect = ui.button("Retry").clicked();
                        });
                }
                ConnectionState::Disconnected => {
                    ui.label(
                        RichText::new(format!(
                            "{} Agent is offline.",
                            provider_descriptor(self.selected_provider).display_name
                        ))
                        .weak(),
                    );
                    reconnect = ui.button("Connect").clicked();
                }
                ConnectionState::Ready if self.agent.transcript.is_empty() && !self.agent.active => {
                    let project = self
                        .tree
                        .root
                        .file_name()
                        .unwrap_or(self.tree.root.as_os_str())
                        .to_string_lossy();
                    draw_agent_empty_state(
                        ui,
                        self.selected_provider,
                        &project,
                        self.agentic_mode,
                    );
                }
                ConnectionState::Ready => {
                    let scroll_delta = if !menu_owns_wheel && ui.rect_contains_pointer(ui.max_rect()) {
                        ui.input(|input| input.smooth_scroll_delta.y)
                    } else {
                        0.0
                    };
                    let scrolling_up = scroll_delta > 0.0;
                    let scrolling_down = scroll_delta < 0.0;
                    if scrolling_up {
                        self.agent_follow_transcript = false;
                    }
                    let find_matches = self.agent_find.matches.clone();
                    let selected_find_item = find_matches
                        .get(self.agent_find.selected)
                        .copied();
                    let selected_find_occurrence = selected_find_item.map(|item| {
                        find_matches[..self.agent_find.selected]
                            .iter()
                            .filter(|&&candidate| candidate == item)
                            .count()
                    });
                    let find_query = self.agent_find.query.clone();
                    let should_scroll_to_find = self.agent_find.scroll_to_match;
                    let mut scrolled_to_find = false;
                    // Cached heights are valid only for the layout and
                    // provider session that produced those item indexes.
                    let dense_agent = self.settings.appearance.dense_agent;
                    let mut session_hasher = DefaultHasher::new();
                    (self.selected_provider, self.agent.session_id.as_deref())
                        .hash(&mut session_hasher);
                    let heights_key = (
                        transcript_width.round().to_bits(),
                        theme::paint_appearance(ui.pixels_per_point()),
                        dense_agent,
                        self.agent.active,
                        session_hasher.finish(),
                    );
                    if self.agent_transcript_heights_key != heights_key
                        || self.agent_transcript_heights.len() > self.agent.transcript.len()
                    {
                        self.agent_transcript_heights.clear();
                        self.agent_transcript_heights_key = heights_key;
                    }
                    let mut item_heights = std::mem::take(&mut self.agent_transcript_heights);
                    let transcript_len = self.agent.transcript.len();
                    let mut user_prompt_images = vec![Vec::new(); transcript_len];
                    let mut merged_user_images = vec![false; transcript_len];
                    let mut user_prompt_index = None;
                    for (index, item) in self.agent.transcript.iter().enumerate() {
                        match item {
                            TranscriptItem::User(_) => user_prompt_index = Some(index),
                            TranscriptItem::Content {
                                role: ContentRole::User,
                                content: DisplayContent::Image {
                                    data: Some(data), ..
                                },
                            } => {
                                if let Some(prompt_index) = user_prompt_index {
                                    user_prompt_images[prompt_index].push(Arc::clone(data));
                                    merged_user_images[index] = true;
                                }
                            }
                            TranscriptItem::Content {
                                role: ContentRole::User,
                                ..
                            } => {}
                            _ => user_prompt_index = None,
                        }
                    }
                    let transcript_has_footer =
                        !self.agent.changed_paths.is_empty() || self.agent.active;
                    item_heights.resize(transcript_len, f32::NAN);
                    let mut rendered_items = 0_usize;
                    let tool_changes = &self.agent.tool_changes;
                    let active_work_start = self.agent.active.then(|| {
                        self.agent
                            .transcript
                            .iter()
                            .rposition(|item| matches!(item, TranscriptItem::User(_)))
                            .map_or(0, |index| index + 1)
                    });
                    let dense_work_clusters = dense_agent_work_clusters(
                        &self.agent.transcript,
                        tool_changes,
                        active_work_start,
                    );
                    let dense_final_responses = if dense_agent {
                        dense_agent_final_response_starts(
                            &self.agent.transcript,
                            self.agent.active,
                        )
                    } else {
                        Vec::new()
                    };
                    let dense_work_items = if dense_agent {
                        self.agent
                            .transcript
                            .iter()
                            .map(dense_agent_work_item)
                            .collect::<Vec<_>>()
                    } else {
                        Vec::new()
                    };
                    let output = ScrollArea::vertical()
                        .id_salt("agent_transcript")
                        .auto_shrink([false, false])
                        .scroll_source(egui::scroll_area::ScrollSource {
                            mouse_wheel: !menu_owns_wheel,
                            ..Default::default()
                        })
                        .content_margin(egui::Margin {
                            left: 0,
                            right: 0,
                            top: AGENT_TRANSCRIPT_TOP_PADDING,
                            bottom: theme::space::LARGE as i8,
                        })
                        .stick_to_bottom(self.agent_follow_transcript)
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_space(transcript_padding);
                                ui.vertical(|ui| {
                                    ui.set_width(transcript_width);
                                    ui.set_max_width(transcript_width);
                                    let clip = ui.clip_rect();
                                    let item_gap = if dense_agent { 8.0 } else { 16.0 };
                                    let mut dense_work_open = true;
                                    for (item_index, item) in
                                        self.agent.transcript.iter_mut().enumerate()
                                    {
                                if merged_user_images[item_index] {
                                    item_heights[item_index] = 0.0;
                                    continue;
                                }
                                let item_top = ui.cursor().top();
                                let item_is_selected = selected_find_item == Some(item_index);
                                let is_dense_work = dense_agent && dense_agent_work_item(item);
                                let dense_cluster = is_dense_work
                                    .then(|| dense_work_clusters[item_index].as_ref())
                                    .flatten();
                                if let Some(cluster) = dense_cluster {
                                    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                                            ui.ctx(),
                                            Id::new(("dense_agent_work", item_index, cluster.active)),
                                            cluster.active,
                                        );
                                    if cluster.active || !find_matches.is_empty() {
                                        state.set_open(true);
                                        state.store(ui.ctx());
                                    }
                                    dense_work_open = state.is_open();
                                } else if is_dense_work && !dense_work_open {
                                    item_heights[item_index] = 0.0;
                                    continue;
                                }
                                let gap_after_item = dense_agent_gap_after_item(
                                    item_gap,
                                    is_dense_work,
                                    dense_work_open,
                                    dense_work_items.get(item_index + 1) == Some(&true),
                                );
                                // An off-screen item with a known height only
                                // needs its space, not its widgets: laying out
                                // every item every frame is what made long
                                // transcripts crawl. The selected item stays
                                // rendered while a find-scroll is pending so
                                // its diff can steer the scroll to the exact
                                // matching row.
                                let cached_height = item_heights[item_index];
                                if cached_height.is_finite()
                                    && !(item_is_selected && should_scroll_to_find)
                                    && (item_top + cached_height < clip.top() - AGENT_CULL_MARGIN
                                        || item_top > clip.bottom() + AGENT_CULL_MARGIN)
                                {
                                    ui.add_space(cached_height);
                                    if item_index + 1 < transcript_len || transcript_has_footer {
                                        ui.add_space(gap_after_item);
                                    }
                                    continue;
                                }
                                rendered_items += 1;
                                let mut item_scrolled = false;
                                let item_matches = find_matches.binary_search(&item_index).is_ok();
                                let item_search = item_matches.then_some((
                                    find_query.as_str(),
                                    item_is_selected
                                        .then_some(selected_find_occurrence)
                                        .flatten(),
                                ));
                                if let Some(cluster) = dense_cluster {
                                    dense_work_open = agent_dense_disclosure_row(
                                        ui,
                                        Id::new(("dense_agent_work", item_index, cluster.active)),
                                        &cluster.label,
                                        cluster.change,
                                        cluster.active,
                                        cluster.active || !find_matches.is_empty(),
                                    );
                                    if !dense_work_open {
                                        item_heights[item_index] = ui.cursor().top() - item_top;
                                        if item_index + 1 < transcript_len
                                            || transcript_has_footer
                                        {
                                            ui.add_space(item_gap);
                                        }
                                        continue;
                                    }
                                }
                                match item {
                                    TranscriptItem::User(text) => {
                                        if !dense_agent {
                                            ui.label(
                                                RichText::new("YOU")
                                                    .size(theme::typography::MICRO_SIZE)
                                                    .strong()
                                                    .color(theme::text().muted),
                                            );
                                        }
                                        egui::Frame::new()
                                            .fill(theme::surface().input)
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                theme::border::strong_color(),
                                            ))
                                            .inner_margin(egui::Margin::same(12))
                                            .corner_radius(8)
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                if !text.is_empty() {
                                                    let job = agent_text_job(
                                                        text,
                                                        ui.available_width(),
                                                        theme::typography::body(),
                                                        theme::text().primary,
                                                        item_search,
                                                    );
                                                    ui.add(Label::new(job).wrap());
                                                }
                                                if !user_prompt_images[item_index].is_empty() {
                                                    if !text.is_empty() {
                                                        ui.add_space(theme::space::MEDIUM);
                                                    }
                                                    ui.horizontal_wrapped(|ui| {
                                                        for data in &user_prompt_images[item_index] {
                                                            if agent_prompt_image_preview(ui, data)
                                                                .is_some_and(|preview| {
                                                                    preview.clicked()
                                                                })
                                                            {
                                                                open_image_request = Some(
                                                                    AgentImageSource::Bytes(
                                                                        Arc::clone(data),
                                                                    ),
                                                                );
                                                            }
                                                        }
                                                    });
                                                }
                                            });
                                    }
                                    TranscriptItem::Assistant(text) => {
                                        if !dense_agent || dense_final_responses[item_index] {
                                            draw_provider_identity(ui, self.selected_provider);
                                        }
                                        let width = ui.available_width();
                                        let galley = agent_markdown_galley(
                                            ui,
                                            Id::new(("agent_markdown", item_index, dense_agent)),
                                            text,
                                            width,
                                            &self.highlighter,
                                            &self.syntaxes,
                                            dense_agent,
                                            item_search,
                                        );
                                        ui.add(Label::new(galley).wrap());
                                    }
                                    TranscriptItem::Thought(text) => {
                                        let add_thought = |ui: &mut egui::Ui| {
                                                ui.add(
                                                    Label::new(
                                                        RichText::new(text.as_str()).weak(),
                                                    )
                                                    .wrap(),
                                                );
                                            };
                                        if dense_agent {
                                            agent_dense_tool(
                                                ui,
                                                Id::new(("dense_agent_thought", item_index)),
                                                "Thought",
                                                None,
                                                None,
                                                item_search,
                                                true,
                                                add_thought,
                                            );
                                        } else {
                                            egui::CollapsingHeader::new("Thinking")
                                                .id_salt(("thought", item_index))
                                                .icon(paint_agent_disclosure)
                                                .show(ui, add_thought);
                                        }
                                    }
                                    TranscriptItem::Content { role, content } => {
                                        let image = matches!(content, DisplayContent::Image { .. });
                                        if image && !matches!(role, ContentRole::User) {
                                            if matches!(role, ContentRole::Assistant) {
                                                draw_provider_identity(ui, self.selected_provider);
                                            }
                                            egui::CollapsingHeader::new("Image")
                                                .id_salt(("agent_content_image", item_index))
                                                .default_open(false)
                                                .icon(paint_agent_disclosure)
                                                .show(ui, |ui| {
                                                    if let Some(source) = draw_agent_content(
                                                        ui,
                                                        content,
                                                        item_search,
                                                    ) {
                                                        open_image_request = Some(source);
                                                    }
                                                });
                                        } else if dense_agent
                                            && matches!(role, ContentRole::Thought)
                                        {
                                            agent_dense_tool(
                                                ui,
                                                Id::new(("dense_agent_content", item_index)),
                                                "Thought",
                                                None,
                                                None,
                                                item_search,
                                                true,
                                                |ui| {
                                                    if let Some(source) = draw_agent_content(
                                                        ui,
                                                        content,
                                                        item_search,
                                                    ) {
                                                        open_image_request = Some(source);
                                                    }
                                                },
                                            );
                                        } else {
                                            if !dense_agent
                                                || (matches!(role, ContentRole::Assistant)
                                                    && dense_final_responses[item_index])
                                            {
                                                match role {
                                                    ContentRole::Assistant => draw_provider_identity(
                                                        ui,
                                                        self.selected_provider,
                                                    ),
                                                    ContentRole::User => {
                                                        ui.label(
                                                            RichText::new("You").small().strong(),
                                                        );
                                                    }
                                                    ContentRole::Thought => {
                                                        ui.label(
                                                            RichText::new("Thinking")
                                                                .small()
                                                                .strong(),
                                                        );
                                                    }
                                                }
                                            }
                                            if let Some(source) =
                                                draw_agent_content(ui, content, item_search)
                                            {
                                                open_image_request = Some(source);
                                            }
                                        }
                                    }
                                    TranscriptItem::Plan(plan) => {
                                        let add_plan = |ui: &mut egui::Ui| {
                                                for item in plan {
                                                    agent_search_label(
                                                        ui,
                                                        &format!(
                                                            "{}  {}",
                                                            item.status, item.content
                                                        ),
                                                        theme::typography::body(),
                                                        theme::text().primary,
                                                        item_search,
                                                    );
                                                }
                                            };
                                        if dense_agent {
                                            agent_dense_tool(
                                                ui,
                                                Id::new(("dense_agent_plan", item_index)),
                                                "Plan",
                                                None,
                                                None,
                                                item_search,
                                                true,
                                                add_plan,
                                            );
                                        } else {
                                            egui::CollapsingHeader::new("Plan")
                                                .id_salt(("plan", item_index))
                                                .default_open(true)
                                                .icon(paint_agent_disclosure)
                                                .show(ui, add_plan);
                                        }
                                    }
                                    TranscriptItem::Tool(tool) => {
                                        let title = tool.display_title();
                                        let title = title.as_ref();
                                        let action = title.split_whitespace().next();
                                        let title_includes_paths = action.is_some_and(|action| {
                                            action.eq_ignore_ascii_case("Read")
                                                || action.eq_ignore_ascii_case("Edit")
                                        });
                                        let is_file_edit = action
                                            .is_some_and(|action| action.eq_ignore_ascii_case("Edit"))
                                            || tool
                                                .kind
                                                .as_deref()
                                                .is_some_and(|kind| kind.eq_ignore_ascii_case("Edit"));
                                        let contains_diff = tool_contains_diff(tool);
                                        let is_subagent = tool_is_subagent(tool);
                                        let has_body = tool.detail.is_some()
                                            || (!title_includes_paths && !tool.paths.is_empty());
                                        let change = is_file_edit
                                            .then(|| tool_changes.get(&tool.id).copied())
                                            .flatten();
                                        let add_body = |ui: &mut egui::Ui| {
                                                if is_subagent {
                                                    ui.label(
                                                        RichText::new("SUBAGENT")
                                                            .size(theme::typography::MICRO_SIZE)
                                                            .strong()
                                                            .color(theme::accent()),
                                                    );
                                                }
                                                if !title_includes_paths {
                                                    for tool_path in &tool.paths {
                                                        let label = match tool_path.line {
                                                            Some(line) => format!(
                                                                "{}:{line}",
                                                                tool_path.path.display()
                                                            ),
                                                            None => tool_path
                                                                .path
                                                                .display()
                                                                .to_string(),
                                                        };
                                                        if agent_path_link(ui, &label, item_search)
                                                            .clicked()
                                                        {
                                                            open_path_request = Some((
                                                                tool_path.path.clone(),
                                                                tool_path.line,
                                                            ));
                                                        }
                                                    }
                                                }
                                                if let Some(detail) = &tool.detail {
                                                    for (content_index, content) in
                                                        detail.content.iter().enumerate()
                                                    {
                                                        match content {
                                                            ToolOutput::Text(text) => {
                                                                if let Some(path) = tool
                                                                    .paths
                                                                    .get(content_index)
                                                                    .or_else(|| tool.paths.first())
                                                                    .map(|tool_path| {
                                                                        &tool_path.path
                                                                    })
                                                                {
                                                                    let width = ui.available_width();
                                                                    let galley = agent_code_galley(
                                                                        ui,
                                                                        Id::new((
                                                                            "agent_tool_code",
                                                                            item_index,
                                                                            content_index,
                                                                        )),
                                                                        path,
                                                                        text,
                                                                        width,
                                                                        &self.highlighter,
                                                                        &self.syntaxes,
                                                                        item_search,
                                                                    );
                                                                    ui.add(
                                                                        Label::new(galley)
                                                                            .wrap()
                                                                            .selectable(true),
                                                                    );
                                                                } else {
                                                                    agent_search_label(
                                                                        ui,
                                                                        text,
                                                                        theme::typography::body(),
                                                                        theme::text().primary,
                                                                        item_search,
                                                                    );
                                                                }
                                                            }
                                                            ToolOutput::Content(content) => {
                                                                if let Some(source) = draw_agent_content(
                                                                    ui,
                                                                    content,
                                                                    item_search,
                                                                ) {
                                                                    open_image_request = Some(source);
                                                                }
                                                            }
                                                            ToolOutput::Diff {
                                                                path,
                                                                old_text,
                                                                new_text,
                                                            } => {
                                                                let (clicked, scrolled) =
                                                                    draw_agent_diff(
                                                                        ui,
                                                                        Id::new((
                                                                            "agent_diff",
                                                                            item_index,
                                                                            content_index,
                                                                        )),
                                                                        path,
                                                                        old_text.as_deref(),
                                                                        new_text,
                                                                        &self.highlighter,
                                                                        &self.syntaxes,
                                                                        item_search,
                                                                        item_is_selected
                                                                            && should_scroll_to_find,
                                                                    );
                                                                item_scrolled |= scrolled;
                                                                if clicked {
                                                                    open_path_request = Some((
                                                                        path.clone(),
                                                                        None,
                                                                    ));
                                                                }
                                                            }
                                                            ToolOutput::Terminal(id) => {
                                                                agent_search_label(
                                                                    ui,
                                                                    &format!("Terminal {id}"),
                                                                    theme::typography::body(),
                                                                    theme::text().primary,
                                                                    item_search,
                                                                );
                                                            }
                                                            ToolOutput::Todo {
                                                                id,
                                                                content,
                                                                status,
                                                            } => {
                                                                agent_search_label(
                                                                    ui,
                                                                    &format!(
                                                                        "{status}  {content} ({id})"
                                                                    ),
                                                                    theme::typography::body(),
                                                                    theme::text().primary,
                                                                    item_search,
                                                                );
                                                            }
                                                            ToolOutput::Task {
                                                                description,
                                                                prompt,
                                                                subagent_type,
                                                                model,
                                                                agent_id: _,
                                                                duration_ms,
                                                            } => {
                                                                agent_search_label(
                                                                    ui,
                                                                    description,
                                                                    theme::typography::strong(),
                                                                    theme::text().primary,
                                                                    item_search,
                                                                );
                                                                let metadata = [
                                                                    Some(subagent_type.clone()),
                                                                    model.clone(),
                                                                    duration_ms.map(
                                                                        agent_task_duration,
                                                                    ),
                                                                ]
                                                                .into_iter()
                                                                .flatten()
                                                                .collect::<Vec<_>>()
                                                                .join(" · ");
                                                                agent_search_label(
                                                                    ui,
                                                                    &metadata,
                                                                    theme::typography::small(),
                                                                    theme::text().muted,
                                                                    item_search,
                                                                );
                                                                egui::CollapsingHeader::new(
                                                                    "Prompt",
                                                                )
                                                                .id_salt((
                                                                    "task_prompt",
                                                                    item_index,
                                                                    content_index,
                                                                ))
                                                                .icon(paint_agent_disclosure)
                                                                .show(ui, |ui| {
                                                                    agent_search_label(
                                                                        ui,
                                                                        prompt,
                                                                        theme::typography::body(),
                                                                        theme::text().primary,
                                                                        item_search,
                                                                    );
                                                                });
                                                            }
                                                            ToolOutput::GeneratedImage {
                                                                description,
                                                                file_path,
                                                                reference_image_paths,
                                                            } => {
                                                                agent_search_label(
                                                                    ui,
                                                                    description,
                                                                    theme::typography::body(),
                                                                    theme::text().primary,
                                                                    item_search,
                                                                );
                                                                if let Some(file_path) = file_path {
                                                                    if let Some(preview) =
                                                                        agent_generated_image_preview(
                                                                            ui, file_path,
                                                                        )
                                                                        && preview.clicked()
                                                                    {
                                                                        open_image_request = Some(
                                                                            AgentImageSource::Path(
                                                                            file_path.clone(),
                                                                            ),
                                                                        );
                                                                    }
                                                                    if agent_path_link(
                                                                        ui,
                                                                        &file_path
                                                                            .display()
                                                                            .to_string(),
                                                                        item_search,
                                                                    )
                                                                    .clicked()
                                                                    {
                                                                        open_path_request = Some((
                                                                            file_path.clone(),
                                                                            None,
                                                                        ));
                                                                    }
                                                                } else {
                                                                    ui.label(
                                                                        RichText::new(
                                                                            "No file path supplied",
                                                                        )
                                                                        .small()
                                                                        .weak(),
                                                                    );
                                                                }
                                                                for reference in
                                                                    reference_image_paths
                                                                {
                                                                    agent_search_label(
                                                                        ui,
                                                                        &format!(
                                                                            "Reference: {}",
                                                                            reference.display()
                                                                        ),
                                                                        theme::typography::code_small(),
                                                                        theme::text().muted,
                                                                        item_search,
                                                                    );
                                                                }
                                                            }
                                                        }
                                                    }
                                                    if detail.content.is_empty()
                                                        && let Some(text) = detail
                                                            .output
                                                            .as_deref()
                                                            .or(detail.input.as_deref())
                                                    {
                                                        let job = agent_text_job(
                                                            text,
                                                            ui.available_width(),
                                                            theme::typography::code_small(),
                                                            theme::text().secondary,
                                                            item_search,
                                                        );
                                                        ui.add(
                                                            Label::new(job)
                                                                .wrap()
                                                                .selectable(true),
                                                        );
                                                    }
                                                }
                                            };
                                        if dense_agent {
                                                agent_dense_tool(
                                                    ui,
                                                    Id::new(("dense_agent_tool", item_index)),
                                                title,
                                                tool.status.as_deref(),
                                                change,
                                                item_search,
                                                has_body,
                                                add_body,
                                            );
                                        } else {
                                            agent_collapsing_header(
                                                ui,
                                                ("tool", item_index, contains_diff),
                                                title,
                                                tool.status.as_deref(),
                                                change,
                                                transcript_width,
                                                item_search,
                                                has_body,
                                                is_subagent
                                                    || (contains_diff && !is_file_edit),
                                                add_body,
                                            );
                                        }
                                    }
                                    TranscriptItem::Permission(card) => {
                                        if let Some(selected) = card.selected.as_ref().and_then(
                                            |selected| {
                                                card.options
                                                    .iter()
                                                    .find(|option| &option.id == selected)
                                            },
                                        ) {
                                            let (status, color) = match selected.kind.as_str() {
                                                "AllowAlways" => (
                                                    "Allowed globally",
                                                    theme::diff::added_ink(),
                                                ),
                                                "AllowOnce" => (
                                                    "Allowed once",
                                                    theme::diff::added_ink(),
                                                ),
                                                "RejectAlways" => (
                                                    "Rejected globally",
                                                    theme::diff::removed_ink(),
                                                ),
                                                "RejectOnce" => (
                                                    "Rejected",
                                                    theme::diff::removed_ink(),
                                                ),
                                                _ => (
                                                    selected.name.as_str(),
                                                    theme::text().muted,
                                                ),
                                            };
                                            egui::Frame::new()
                                            .fill(theme::surface().raised)
                                                .stroke(egui::Stroke::new(
                                                    1.0,
                                                    theme::border::hairline_color(),
                                                ))
                                                .inner_margin(egui::Margin::same(8))
                                                .corner_radius(7)
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width().min(284.0));
                                                    ui.label(
                                                        RichText::new(status)
                                                            .small()
                                                            .strong()
                                                            .color(color),
                                                    );
                                                    ui.add(Label::new(&card.action).wrap());
                                                });
                                            item_heights[item_index] =
                                                ui.cursor().top() - item_top;
                                            scrolled_to_find |= paint_agent_search_item(
                                                ui,
                                                item_top,
                                                item_is_selected,
                                                should_scroll_to_find,
                                            );
                                            if item_index + 1 < transcript_len
                                                || transcript_has_footer
                                            {
                                                ui.add_space(gap_after_item);
                                            }
                                            continue;
                                        }
                                        egui::Frame::new()
                                            .fill(theme::callout(theme::semantic().warning).fill)
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                theme::callout(theme::semantic().warning).border,
                                            ))
                                            .inner_margin(egui::Margin::same(10))
                                            .corner_radius(8)
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width().min(278.0));
                                                ui.label(
                                                    RichText::new("Permission required")
                                                        .strong()
                                                        .color(theme::ink(theme::semantic().warning)),
                                                );
                                                ui.add_space(3.0);
                                                ui.add(Label::new(&card.action).wrap());
                                                ui.add_space(8.0);
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 6.0;
                                                    for option in &card.options {
                                                        let label = match option.kind.as_str() {
                                                            "AllowOnce" => "Allow once",
                                                            "AllowAlways" => "Always allow",
                                                            "RejectOnce" => "Reject",
                                                            "RejectAlways" => "Always reject",
                                                            _ => &option.name,
                                                        };
                                                        let (fill, stroke, text_color) =
                                                            match option.kind.as_str() {
                                                                "AllowAlways" => (
                                                                    theme::callout(theme::semantic().info).fill,
                                                                    theme::callout(theme::semantic().info).border,
                                                                    theme::ink(theme::semantic().info),
                                                                ),
                                                                "RejectOnce" | "RejectAlways" => (
                                                                    theme::callout(theme::semantic().danger).fill,
                                                                    theme::callout(theme::semantic().danger).border,
                                                                    theme::ink(theme::semantic().danger),
                                                                ),
                                                                _ => (
                                                                    theme::state::selected(),
                                                                    theme::border::strong_color(),
                                                                    theme::text().primary,
                                                                ),
                                                            };
                                                        let response = ui
                                                            .add(
                                                                egui::Button::new(
                                                                    RichText::new(label)
                                                                        .strong()
                                                                        .color(text_color),
                                                                )
                                                                .min_size(egui::vec2(0.0, 30.0))
                                                                .fill(fill)
                                                                .stroke(egui::Stroke::new(
                                                                    1.0, stroke,
                                                                ))
                                                                .corner_radius(6),
                                                            )
                                                            .on_hover_text(
                                                                if option.kind == "AllowAlways" {
                                                                    "Remember this permission globally"
                                                                } else {
                                                                    &option.name
                                                                },
                                                            );
                                                        if response.clicked() {
                                                            permission_decisions.push((
                                                                card.request_id,
                                                                option.id.clone(),
                                                            ));
                                                        }
                                                    }
                                                });
                                            });
                                    }
                                    TranscriptItem::Interaction(card) => {
                                        egui::Frame::new()
                                            .fill(theme::surface().input)
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                theme::border::strong_color(),
                                            ))
                                            .inner_margin(egui::Margin::same(10))
                                            .corner_radius(7)
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                match &card.request.kind {
                                                    InteractionKind::Questions {
                                                        title,
                                                        questions,
                                                    } => {
                                                        ui.label(
                                                            RichText::new(title).strong().color(
                                                                theme::text().primary,
                                                            ),
                                                        );
                                                        for question in questions {
                                                            ui.add_space(6.0);
                                                            ui.add(
                                                                Label::new(
                                                                    RichText::new(
                                                                        &question.prompt,
                                                                    )
                                                                    .strong(),
                                                                )
                                                                .wrap(),
                                                            );
                                                            let selected = card
                                                                .selections
                                                                .entry(question.id.clone())
                                                                .or_default();
                                                            for option in &question.options {
                                                                let is_selected = selected
                                                                    .contains(&option.id);
                                                                if ui
                                                                    .add_enabled(
                                                                        !card.answered,
                                                                        egui::Button::selectable(
                                                                            is_selected,
                                                                            &option.label,
                                                                        ),
                                                                    )
                                                                    .clicked()
                                                                {
                                                                    if question.allow_multiple {
                                                                        if is_selected {
                                                                            selected.retain(|id| {
                                                                                id != &option.id
                                                                            });
                                                                        } else {
                                                                            selected
                                                                                .push(option.id.clone());
                                                                        }
                                                                    } else {
                                                                        selected.clear();
                                                                        selected.push(
                                                                            option.id.clone(),
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        if !card.answered {
                                                            ui.horizontal(|ui| {
                                                                let complete = questions.iter().all(
                                                                    |question| {
                                                                        card.selections
                                                                            .get(&question.id)
                                                                            .is_some_and(|answer| {
                                                                                !answer.is_empty()
                                                                            })
                                                                    },
                                                                );
                                                                if ui
                                                                    .add_enabled(
                                                                        complete,
                                                                        egui::Button::new(
                                                                            "Submit answers",
                                                                        ),
                                                                    )
                                                                    .clicked()
                                                                {
                                                                    interaction_responses.push((
                                                                        card.request.request_id,
                                                                        InteractionResponse::Answers(
                                                                            questions
                                                                                .iter()
                                                                                .map(|question| {
                                                                                    QuestionAnswer {
                                                                                        question_id: question.id.clone(),
                                                                                        selected_option_ids: card.selections[&question.id].clone(),
                                                                                    }
                                                                                })
                                                                                .collect(),
                                                                        ),
                                                                    ));
                                                                }
                                                                if ui.button("Skip").clicked() {
                                                                    interaction_responses.push((
                                                                        card.request.request_id,
                                                                        InteractionResponse::Skipped,
                                                                    ));
                                                                }
                                                            });
                                                        }
                                                    }
                                                    InteractionKind::Plan(plan) => {
                                                        ui.label(
                                                            RichText::new(
                                                                plan.name
                                                                    .as_deref()
                                                                    .unwrap_or("Proposed plan"),
                                                            )
                                                            .strong()
                                                            .color(theme::text().primary),
                                                        );
                                                        if let Some(overview) = &plan.overview {
                                                            ui.add(
                                                                Label::new(overview).wrap(),
                                                            );
                                                        }
                                                        if !plan.plan.is_empty() {
                                                            ui.add(
                                                                Label::new(&plan.plan).wrap(),
                                                            );
                                                        }
                                                        for todo in &plan.todos {
                                                            ui.add(
                                                                Label::new(format!(
                                                                    "{}  {}",
                                                                    todo.status, todo.content
                                                                ))
                                                                .wrap(),
                                                            );
                                                        }
                                                        for phase in &plan.phases {
                                                            ui.label(
                                                                RichText::new(&phase.name).strong(),
                                                            );
                                                            for todo in &phase.todos {
                                                                ui.add(
                                                                    Label::new(format!(
                                                                        "{}  {}",
                                                                        todo.status, todo.content
                                                                    ))
                                                                    .wrap(),
                                                                );
                                                            }
                                                        }
                                                        if let Some(is_project) = plan.is_project {
                                                            ui.label(
                                                                RichText::new(if is_project {
                                                                    "Project plan"
                                                                } else {
                                                                    "Session plan"
                                                                })
                                                                .small()
                                                                .weak(),
                                                            );
                                                        }
                                                        if !card.answered {
                                                            ui.horizontal(|ui| {
                                                                if ui.button("Accept").clicked() {
                                                                    interaction_responses.push((
                                                                        card.request.request_id,
                                                                        InteractionResponse::PlanAccepted,
                                                                    ));
                                                                }
                                                                if ui.button("Reject").clicked() {
                                                                    interaction_responses.push((
                                                                        card.request.request_id,
                                                                        InteractionResponse::PlanRejected,
                                                                    ));
                                                                }
                                                            });
                                                        }
                                                    }
                                                }
                                            });
                                    }
                                    TranscriptItem::Error(error) => {
                                        egui::Frame::new()
                                            .fill(theme::callout(theme::semantic().danger).fill)
                                            .inner_margin(egui::Margin::same(10))
                                            .corner_radius(6)
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                let job = agent_text_job(
                                                    error,
                                                    ui.available_width(),
                                                    theme::typography::body(),
                                                    theme::ink(theme::semantic().danger),
                                                    item_search,
                                                );
                                                ui.add(Label::new(job).wrap());
                                            });
                                    }
                                }
                                item_heights[item_index] = ui.cursor().top() - item_top;
                                // A diff that already scrolled to its exact
                                // matching row wins over the coarser
                                // whole-item scroll, which would override it.
                                scrolled_to_find |= item_scrolled
                                    || paint_agent_search_item(
                                        ui,
                                        item_top,
                                        item_is_selected,
                                        should_scroll_to_find && !item_scrolled,
                                    );
                                if item_index + 1 < transcript_len || transcript_has_footer {
                                    ui.add_space(gap_after_item);
                                }
                            }
                            if !self.agent.changed_paths.is_empty() {
                                let item_top = ui.cursor().top();
                                if let Some(path) = draw_agent_changed_files(
                                    ui,
                                    &self.tree.root,
                                    &self.agent.changed_paths,
                                    (selected_find_item == Some(self.agent.transcript.len()))
                                        .then_some((
                                            find_query.as_str(),
                                            selected_find_occurrence,
                                        ))
                                        .or_else(|| {
                                            find_matches
                                                .binary_search(&self.agent.transcript.len())
                                                .is_ok()
                                                .then_some((find_query.as_str(), None))
                                        }),
                                ) {
                                    open_diff_request = Some(path);
                                }
                                let item_index = self.agent.transcript.len();
                                scrolled_to_find |= paint_agent_search_item(
                                    ui,
                                    item_top,
                                    selected_find_item == Some(item_index),
                                    should_scroll_to_find,
                                );
                            }
                            if self.agent.active {
                                if dense_agent {
                                    if active_work_start == Some(transcript_len) {
                                        draw_dense_agent_working(ui);
                                    }
                                } else {
                                    ui.add_space(theme::space::SMALL);
                                    draw_agent_working(ui, self.selected_provider);
                                }
                            }
                                });
                            });
                        });
                    self.agent_transcript_heights = item_heights;
                    self.agent_transcript_rendered = rendered_items;
                    if scrolled_to_find {
                        self.agent_find.scroll_to_match = false;
                        self.agent_follow_transcript = false;
                    }
                    let max_offset =
                        (output.content_size.y - output.inner_rect.height()).max(0.0);
                    let at_bottom = agent_at_bottom(output.state.offset.y, max_offset);
                    let page_result = if scrolling_up
                        && output.state.offset.y <= 0.5
                        && self.agent.has_earlier_transcript()
                    {
                        Some(self.agent.load_earlier_transcript())
                    } else if scrolling_down
                        && at_bottom
                        && self.agent.has_later_transcript()
                    {
                        Some(self.agent.load_later_transcript())
                    } else {
                        None
                    };
                    if let Some(page_result) = page_result {
                        match page_result {
                            Ok(true) => {
                                self.agent_transcript_heights.clear();
                                self.agent_find.dirty = true;
                                self.agent_follow_transcript = false;
                                ui.ctx().request_discard("page the agent transcript");
                            }
                            Ok(false) => {}
                            Err(error) => self.show_error(error),
                        }
                    }
                    self.agent_follow_transcript = !self.agent.has_later_transcript()
                        && !scrolling_up
                        && (self.agent_follow_transcript || at_bottom);
                    if self.agent_follow_transcript
                        && (output.state.offset.y - max_offset).abs() > 0.5
                    {
                        let mut state = output.state;
                        state.offset.y = max_offset;
                        state.store(ui.ctx(), output.id);
                        ui.ctx().request_discard("pin the agent transcript before painting");
                    }
                    if !self.agent_follow_transcript {
                        let button = egui::Rect::from_min_size(
                            egui::pos2(
                                output.inner_rect.right() - transcript_padding - 32.0,
                                output.inner_rect.bottom() - 32.0,
                            ),
                            egui::vec2(26.0, 26.0),
                        );
                        let jump = ui
                            .put(
                                button,
                                egui::Button::new("")
                                .fill(theme::state::selected())
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    theme::border::strong_color(),
                                ))
                                .corner_radius(6),
                            )
                            .on_hover_text("Jump to latest");
                        icons::paint(
                            ui.painter(),
                            Icon::ChevronDown,
                            egui::Rect::from_center_size(
                                jump.rect.center(),
                                egui::Vec2::splat(icons::GRID * 0.75),
                            ),
                            theme::text().secondary,
                        );
                        if jump.clicked() {
                            if self.agent.has_later_transcript() {
                                match self.agent.load_latest_transcript() {
                                    Ok(true) => {
                                        self.agent_transcript_heights.clear();
                                        self.agent_find.dirty = true;
                                        self.agent_follow_transcript = true;
                                        ui.ctx().request_discard("load the latest transcript page");
                                    }
                                    Ok(false) => {}
                                    Err(error) => self.show_error(error),
                                }
                            } else {
                                let mut state = output.state;
                                state.offset.y = max_offset;
                                state.store(ui.ctx(), output.id);
                                self.agent_follow_transcript = true;
                                ui.ctx().request_repaint();
                            }
                        }
                    }
                }
            },
        );

        let mut mode_change = None;
        let mut config_changes = Vec::new();
        let mut run_everything_change = None;
        let mut provider_change = None;
        let mut session_load = None;
        let mut session_remove = None;
        let has_config_mode = self.agent.config_options.iter().any(|option| {
            option.id.eq_ignore_ascii_case("mode") || option.name.eq_ignore_ascii_case("mode")
        });

        let mut send = false;
        let mut cancel = false;
        let mut open_file_picker = false;
        let mut submit_shortcut = false;
        let mut prompt_changed = false;
        let mut history_navigated = false;
        let mut mention_attach = None;
        let composer_enabled = self.agent.session_ready && !self.agent.active;
        let composer_hint = if self.agent.session_ready {
            format!(
                "Ask {} Agent…",
                provider_descriptor(self.selected_provider).display_name
            )
        } else {
            format!(
                "Connect {} to start…",
                provider_descriptor(self.selected_provider).display_name
            )
        };
        let mut open_menu = self.agent_menu.clone();
        if (!self.agent.session_ready || self.agent.active)
            && !matches!(open_menu, Some(AgentMenu::Providers))
        {
            open_menu = None;
        }
        let mut menu_anchor = if matches!(open_menu, Some(AgentMenu::Providers)) {
            self.provider_menu_anchor
        } else {
            session_menu_anchor
        };
        let mut menu_toggled = session_menu_toggled || provider_menu_toggled;
        let composer_panel = if self.agentic_mode {
            let padding =
                ((composer.width() - AGENTIC_CONTENT_WIDTH) * 0.5).max(theme::space::WIDE);
            let panel = egui::Rect::from_min_max(
                egui::pos2(
                    composer.left() + padding,
                    composer.top() + AGENTIC_COMPOSER_TOP_MARGIN,
                ),
                egui::pos2(
                    composer.right() - padding,
                    composer.bottom() - AGENTIC_COMPOSER_BOTTOM_MARGIN,
                ),
            );
            ui.painter().rect_filled(composer, 0.0, editor_background());
            ui.painter()
                .rect_filled(panel, AGENTIC_COMPOSER_RADIUS, agentic_composer_fill());
            ui.painter().rect_stroke(
                panel,
                AGENTIC_COMPOSER_RADIUS,
                egui::Stroke::new(1.0, theme::border::strong_color()),
                egui::StrokeKind::Inside,
            );
            panel
        } else {
            ui.painter()
                .rect_filled(composer, 0.0, theme::surface().chrome);
            ui.painter().hline(
                composer.x_range(),
                composer.top() + 0.5,
                egui::Stroke::new(1.0, theme::border::hairline_color()),
            );
            composer
        };
        let composer_content = agent_composer_content(composer_panel);
        let (hovered_files, dropped_files, pointer) = ui.input(|input| {
            (
                !input.raw.hovered_files.is_empty(),
                input
                    .raw
                    .dropped_files
                    .iter()
                    .filter_map(|file| file.path.clone())
                    .collect::<Vec<_>>(),
                input.pointer.hover_pos(),
            )
        });
        let pointer_over_composer = pointer.is_some_and(|pointer| composer_panel.contains(pointer));
        if hovered_files {
            self.agent_drop_hovered = composer_enabled && pointer_over_composer;
        }
        if !dropped_files.is_empty() {
            let dropped_over_composer =
                composer_enabled && (pointer_over_composer || self.agent_drop_hovered);
            self.agent_drop_hovered = false;
            if dropped_over_composer {
                self.attach_agent_files(ui.ctx(), dropped_files);
                ui.ctx().request_repaint();
            }
        } else if !hovered_files {
            self.agent_drop_hovered = false;
        }
        let attachment_height = if self.agent_attachments.is_empty() {
            0.0
        } else {
            AGENT_ATTACHMENT_ROW_HEIGHT
        };
        if attachment_height > 0.0 {
            let attachments = egui::Rect::from_min_max(
                egui::pos2(composer_content.left(), composer_content.top()),
                egui::pos2(
                    composer_content.right(),
                    composer_content.top() + attachment_height,
                ),
            );
            let mut remove = None;
            ui.scope_builder(
                UiBuilder::new()
                    .id_salt("agent_attachments")
                    .max_rect(attachments)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ScrollArea::horizontal()
                        .id_salt("agent_attachment_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                for (index, attachment) in self.agent_attachments.iter().enumerate()
                                {
                                    if agent_attachment_tile(ui, attachment).clicked() {
                                        remove = Some(index);
                                    }
                                }
                            });
                        });
                },
            );
            if let Some(index) = remove {
                self.agent_attachments.remove(index);
                ui.ctx().request_repaint();
            }
        }
        // The footer starts half an icon's dead zone to the left of the
        // content, so the attach glyph — not its invisible hit target — lines
        // up with the prompt text above it.
        let footer = egui::Rect::from_min_max(
            egui::pos2(
                composer_content.left() - (theme::control::STANDARD - icons::GRID) * 0.5,
                composer_content.bottom() - theme::control::STANDARD,
            ),
            composer_content.right_bottom(),
        );
        let input_rect = egui::Rect::from_min_max(
            egui::pos2(
                composer_content.left(),
                composer_content.top() + attachment_height,
            ),
            egui::pos2(composer_content.right(), footer.top() - theme::space::SMALL),
        );
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("agent_composer_region")
                .max_rect(input_rect)
                .layout(Layout::top_down(Align::LEFT)),
            |ui| {
                ScrollArea::vertical()
                    .id_salt("agent_prompt_scroll")
                    .max_height(input_rect.height())
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false])
                    .scroll_source(egui::scroll_area::ScrollSource {
                        mouse_wheel: !menu_owns_wheel,
                        ..Default::default()
                    })
                    .content_margin(0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let prompt_id = Id::new("agent_prompt");
                        let mention_open = matches!(open_menu, Some(AgentMenu::Mentions(_)))
                            && !self.agent_mention_matches.is_empty();
                        if mention_open && ui.memory(|memory| memory.has_focus(prompt_id)) {
                            let (up, down, tab) = ui.input(|input| {
                                (
                                    input.modifiers == egui::Modifiers::NONE
                                        && input.key_pressed(Key::ArrowUp),
                                    input.modifiers == egui::Modifiers::NONE
                                        && input.key_pressed(Key::ArrowDown),
                                    input.modifiers == egui::Modifiers::NONE
                                        && input.key_pressed(Key::Tab),
                                )
                            });
                            if up {
                                self.agent_mention_selected =
                                    self.agent_mention_selected.saturating_sub(1);
                                ui.input_mut(|input| {
                                    input.consume_key(egui::Modifiers::NONE, Key::ArrowUp);
                                });
                            } else if down {
                                self.agent_mention_selected = (self.agent_mention_selected + 1)
                                    .min(self.agent_mention_matches.len() - 1);
                                ui.input_mut(|input| {
                                    input.consume_key(egui::Modifiers::NONE, Key::ArrowDown);
                                });
                            }
                            if tab {
                                mention_attach = self
                                    .agent_mention_matches
                                    .get(self.agent_mention_selected)
                                    .map(|entry| entry.path.clone());
                                remove_agent_mention(&mut self.agent.prompt);
                                open_menu = None;
                                prompt_changed = true;
                                ui.input_mut(|input| {
                                    input.consume_key(egui::Modifiers::NONE, Key::Tab);
                                });
                            }
                        }
                        let cursor_at_start = egui::TextEdit::load_state(ui.ctx(), prompt_id)
                            .and_then(|state| state.cursor.char_range())
                            .is_some_and(|range| {
                                range.primary.index == egui::text::CharIndex(0)
                                    && range.secondary.index == egui::text::CharIndex(0)
                            });
                        let history_key = (!mention_open
                            && ui.memory(|memory| memory.has_focus(prompt_id))
                            && cursor_at_start)
                            .then(|| {
                                ui.input(|input| {
                                    if input.modifiers == egui::Modifiers::NONE
                                        && input.key_pressed(Key::ArrowUp)
                                    {
                                        Some((Key::ArrowUp, true))
                                    } else if input.modifiers == egui::Modifiers::NONE
                                        && input.key_pressed(Key::ArrowDown)
                                    {
                                        Some((Key::ArrowDown, false))
                                    } else {
                                        None
                                    }
                                })
                            })
                            .flatten();
                        if let Some((key, older)) = history_key
                            && self.navigate_agent_prompt_history(older)
                        {
                            history_navigated = true;
                            ui.memory_mut(|memory| {
                                memory.move_focus(egui::FocusDirection::None);
                            });
                            ui.input_mut(|input| {
                                input.consume_key(egui::Modifiers::NONE, key);
                            });
                        }
                        let input = ui.add_enabled(
                            composer_enabled,
                            TextEdit::multiline(&mut self.agent.prompt)
                                .id(prompt_id)
                                .hint_text(
                                    RichText::new(&composer_hint)
                                        .size(theme::typography::BODY_SIZE)
                                        .color(theme::text().secondary),
                                )
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .return_key(egui::KeyboardShortcut::new(
                                    egui::Modifiers::SHIFT,
                                    Key::Enter,
                                ))
                                .frame(egui::Frame::NONE),
                        );
                        if history_navigated
                            && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), prompt_id)
                        {
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::one(
                                    egui::text::CCursor::new(0),
                                )));
                            egui::TextEdit::store_state(ui.ctx(), prompt_id, state);
                        }
                        let input_changed = input.changed();
                        if input_changed {
                            self.agent_prompt_history_index = None;
                            self.agent_prompt_history_draft.clear();
                        }
                        prompt_changed |= history_navigated || input_changed;
                        submit_shortcut = input.has_focus()
                            && ui.input(|input| {
                                !input.modifiers.shift && input.key_pressed(Key::Enter)
                            });
                    });
            },
        );
        if history_navigated {
            ui.memory_mut(|memory| memory.request_focus(Id::new("agent_prompt")));
        }
        let mention_query = composer_enabled
            .then(|| agent_mention_query(&self.agent.prompt))
            .flatten();
        if let Some(query) = mention_query
            && (prompt_changed || matches!(open_menu, Some(AgentMenu::Mentions(_))))
        {
            if self.agent_mentions.is_none() {
                // ponytail: one path-only scan on the first @; move it to the existing search
                // worker only if very large workspaces make this measurable.
                self.agent_mentions = Some(collect_agent_mentions(&self.tree.root));
            }
            let query_changed = !matches!(
                &open_menu,
                Some(AgentMenu::Mentions(current)) if current == query
            );
            self.agent_mention_matches =
                agent_mention_matches(self.agent_mentions.as_deref().unwrap_or_default(), query);
            if query_changed {
                self.agent_mention_selected = 0;
                self.agent_menu_scroll_y = 0.0;
            } else {
                self.agent_mention_selected = self
                    .agent_mention_selected
                    .min(self.agent_mention_matches.len().saturating_sub(1));
            }
            open_menu = Some(AgentMenu::Mentions(query.to_owned()));
            menu_anchor = Some(input_rect);
        } else if prompt_changed && matches!(open_menu, Some(AgentMenu::Mentions(_))) {
            self.agent_mention_matches.clear();
            open_menu = None;
        }
        if mention_query.is_none()
            && composer_enabled
            && let Some(query) = slash_command_query(&self.agent.prompt)
            && (prompt_changed || matches!(open_menu, Some(AgentMenu::Commands(_))))
        {
            open_menu = Some(AgentMenu::Commands(query.to_owned()));
            menu_anchor = Some(input_rect);
        } else if prompt_changed && matches!(open_menu, Some(AgentMenu::Commands(_))) {
            open_menu = None;
        }
        let controls_footer = footer.with_max_x(
            (footer.right() - theme::control::STANDARD - theme::space::SMALL).max(footer.left()),
        );
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("agent_composer_footer")
                .max_rect(controls_footer.translate(egui::vec2(0.0, theme::space::SMALL)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = theme::space::SMALL;
                let attach = ui
                    .add_enabled_ui(composer_enabled, |ui| {
                        icons::button_with_id(
                            ui,
                            Some(Id::new("agent_attach")),
                            Icon::Plus,
                            "Attach files (or type @ for files and folders)",
                            theme::text().secondary,
                            egui::Vec2::splat(theme::control::STANDARD),
                        )
                    })
                    .inner;
                open_file_picker = attach.clicked();
                ui.add_enabled_ui(!self.agent.active, |ui| {
                    if self.agent.allow_run_everything {
                        let run_everything = self
                            .agent_run_everything
                            .or_else(|| run_everything_state(&self.agent.commands))
                            .unwrap_or(false);
                        let menu = AgentMenu::Permissions;
                        let selector = agent_selector_button(
                            ui,
                            if run_everything { "Allow all" } else { "Ask" },
                            "Permissions",
                        );
                        if selector.clicked() {
                            open_menu = (open_menu.as_ref() != Some(&menu)).then_some(menu.clone());
                            menu_toggled = true;
                        }
                        if open_menu.as_ref() == Some(&menu) {
                            menu_anchor = Some(selector.rect);
                        }
                    }
                    if !has_config_mode && !self.agent.modes.is_empty() {
                        let current = self.agent.current_mode.as_deref().unwrap_or_default();
                        let current_name = self
                            .agent
                            .modes
                            .iter()
                            .find(|mode| mode.id == current)
                            .map_or(current, |mode| mode.name.as_str());
                        let menu = AgentMenu::Mode;
                        let selector = agent_selector_button(ui, current_name, "Mode");
                        if selector.clicked() {
                            open_menu = (open_menu.as_ref() != Some(&menu)).then_some(menu.clone());
                            menu_toggled = true;
                        }
                        if open_menu.as_ref() == Some(&menu) {
                            menu_anchor = Some(selector.rect);
                        }
                    }
                    let model_config = self.agent.config_options.iter().find(|option| {
                        matches!(&option.value, ConfigValue::Select(_))
                            && is_model_config(&option.id, &option.name)
                    });
                    let thinking_config = self
                        .agent
                        .config_options
                        .iter()
                        .find(|option| {
                            matches!(&option.value, ConfigValue::Select(_))
                                && is_effort_config(&option.id, &option.name)
                        })
                        .or_else(|| {
                            self.agent.config_options.iter().find(|option| {
                                matches!(&option.value, ConfigValue::Select(_))
                                    && is_thinking_config(&option.id, &option.name)
                            })
                        });
                    for option in &self.agent.config_options {
                        if is_model_config(&option.id, &option.name)
                            || (model_config.is_some()
                                && is_thinking_config(&option.id, &option.name))
                            || (model_config.is_some() && is_fast_config(option))
                        {
                            continue;
                        }
                        match &option.value {
                            ConfigValue::Select(_) => {
                                let current_name = selected_config_name(option).unwrap();
                                let fast = is_thinking_config(&option.id, &option.name)
                                    && fast_mode_config(&self.agent.config_options)
                                        .is_some_and(|(_, enabled, _)| enabled);
                                let menu = AgentMenu::Config(option.id.clone());
                                let selector = agent_config_selector_button(
                                    ui,
                                    None,
                                    &current_name,
                                    option.description.as_deref().unwrap_or(&option.name),
                                    fast,
                                    f32::INFINITY,
                                );
                                if selector.clicked() {
                                    open_menu =
                                        (open_menu.as_ref() != Some(&menu)).then_some(menu.clone());
                                    menu_toggled = true;
                                }
                                if open_menu.as_ref() == Some(&menu) {
                                    menu_anchor = Some(selector.rect);
                                }
                            }
                            ConfigValue::Boolean(current) => {
                                if thinking_config.is_some()
                                    && fast_mode_config(&self.agent.config_options)
                                        .is_some_and(|(fast, _, _)| fast.id == option.id)
                                {
                                    continue;
                                }
                                let selected =
                                    ui.selectable_label(*current, &option.name).on_hover_text(
                                        option.description.as_deref().unwrap_or(&option.name),
                                    );
                                if selected.clicked() {
                                    config_changes
                                        .push((option.id.clone(), ConfigValue::Boolean(!current)));
                                }
                            }
                        }
                    }
                    if let Some(usage) = &self.agent.usage {
                        let cost = usage
                            .cost
                            .as_deref()
                            .map_or(String::new(), |cost| format!(" · {cost}"));
                        ui.label(
                            RichText::new(format!("{} / {}{cost}", usage.used, usage.size))
                                .size(theme::typography::MICRO_SIZE)
                                .weak(),
                        )
                        .on_hover_text("Context usage");
                    }
                    if let Some(model) = model_config {
                        let mut label = selected_config_name(model).unwrap().into_owned();
                        if let Some(thinking) = thinking_config {
                            label.push_str(" · ");
                            label.push_str(&selected_config_name(thinking).unwrap());
                        }
                        let fast = fast_mode_config(&self.agent.config_options)
                            .is_some_and(|(_, enabled, _)| enabled);
                        let menu = AgentMenu::Config(model.id.clone());
                        let selector = agent_config_selector_button(
                            ui,
                            Some(Id::new("agent_model_selector")),
                            &label,
                            "Model, thinking level, and speed",
                            fast,
                            ui.available_width(),
                        );
                        if selector.clicked() {
                            open_menu = (open_menu.as_ref() != Some(&menu)).then_some(menu.clone());
                            menu_toggled = true;
                        }
                        if open_menu.as_ref() == Some(&menu) {
                            menu_anchor = Some(selector.rect);
                        }
                    }
                });
            },
        );
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("agent_composer_action")
                .max_rect(footer)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                if self.agent.active {
                    cancel = agent_composer_action(
                        ui,
                        Icon::Stop,
                        "Stop",
                        theme::state::selected(),
                        theme::text().primary,
                        true,
                    )
                    .clicked();
                } else {
                    let ready = self.agent.session_ready
                        && (!self.agent.prompt.trim().is_empty()
                            || !self.agent_attachments.is_empty());
                    let (fill, color) = agent_send_button_colors(ready);
                    send = agent_composer_action(
                        ui,
                        Icon::ArrowUp,
                        "Send (Enter)",
                        fill,
                        color,
                        ready,
                    )
                    .clicked();
                }
            },
        );
        if self.agent_drop_hovered {
            let radius = if self.agentic_mode {
                AGENTIC_COMPOSER_RADIUS
            } else {
                0
            };
            ui.painter().rect_filled(
                composer_panel,
                radius,
                theme::surface().raised.gamma_multiply(0.93),
            );
            ui.painter().rect_stroke(
                composer_panel.shrink(1.0),
                radius,
                egui::Stroke::new(1.5, theme::accent()),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                composer_panel.center(),
                Align2::CENTER_CENTER,
                "Drop files or folders to attach",
                theme::typography::body(),
                theme::text().primary,
            );
        }

        let mut menu_popup = None;
        if let (Some(menu), Some(anchor)) = (open_menu.as_ref(), menu_anchor) {
            let item_count = match menu {
                AgentMenu::Providers => Some(provider_catalog().len()),
                AgentMenu::Sessions => self
                    .agent
                    .sessions
                    .as_ref()
                    .map(|sessions| sessions.len().max(1)),
                AgentMenu::Commands(query) => Some(
                    self.agent
                        .commands
                        .iter()
                        .filter(|command| command_matches(&command.name, query))
                        .count()
                        .max(1),
                ),
                AgentMenu::Mentions(_) => Some(self.agent_mention_matches.len().max(1)),
                AgentMenu::Permissions => Some(2),
                AgentMenu::Mode => Some(self.agent.modes.len()),
                AgentMenu::Config(id) => self
                    .agent
                    .config_options
                    .iter()
                    .find(|option| option.id == *id)
                    .map(|option| {
                        let thinking_rows = self
                            .agent
                            .config_options
                            .iter()
                            .filter(|other| is_thinking_config(&other.id, &other.name))
                            .map(|other| match &other.value {
                                ConfigValue::Select(_) => other.options.len() + 1,
                                ConfigValue::Boolean(_) => 1,
                            })
                            .sum::<usize>();
                        let fast_rows =
                            usize::from(fast_mode_config(&self.agent.config_options).is_some()) * 2;
                        if is_model_config(&option.id, &option.name)
                            && (thinking_rows > 0 || fast_rows > 0)
                        {
                            return option.options.len() + thinking_rows + 1 + fast_rows;
                        }
                        option.options.len()
                            + usize::from(
                                is_thinking_config(&option.id, &option.name)
                                    && fast_mode_config(&self.agent.config_options).is_some(),
                            ) * 2
                    }),
            };
            if let Some(item_count) = item_count {
                if self.agent_menu.as_ref() != Some(menu) {
                    self.agent_menu_scroll_y = 0.0;
                }
                let row_height = match menu {
                    AgentMenu::Providers => AGENT_PROVIDER_ROW_HEIGHT,
                    AgentMenu::Commands(_) => AGENT_COMMAND_ROW_HEIGHT,
                    AgentMenu::Mentions(_) => AGENT_MENTION_ROW_HEIGHT,
                    AgentMenu::Sessions => AGENT_SESSION_ROW_HEIGHT,
                    _ => AGENT_MENU_ROW_HEIGHT,
                };
                let menu_padding_y = if matches!(menu, AgentMenu::Mentions(_)) {
                    4.0
                } else {
                    8.0
                };
                let popup = match menu {
                    AgentMenu::Providers => agent_provider_menu_rect(
                        ui.ctx().content_rect(),
                        anchor,
                        item_count,
                        row_height,
                    ),
                    AgentMenu::Sessions => agent_session_menu_rect(
                        transcript,
                        anchor,
                        item_count,
                        row_height,
                        AGENT_MENU_WIDTH,
                    ),
                    _ => {
                        agent_menu_rect(transcript, anchor, item_count, row_height, menu_padding_y)
                    }
                };
                menu_popup = Some(popup);
                let max_scroll = (item_count as f32 * row_height
                    - (popup.height() - 2.0 * menu_padding_y))
                    .max(0.0);
                let wheel_delta = ui.input(|input| {
                    input
                        .pointer
                        .hover_pos()
                        .filter(|pointer| popup.contains(*pointer))
                        .map_or(0.0, |_| input.smooth_scroll_delta.y)
                });
                if wheel_delta != 0.0 {
                    self.agent_menu_scroll_y =
                        (self.agent_menu_scroll_y - wheel_delta).clamp(0.0, max_scroll);
                    ui.input_mut(|input| input.smooth_scroll_delta.y = 0.0);
                    ui.ctx().request_repaint();
                }
                self.agent_menu_scroll_y = self.agent_menu_scroll_y.min(max_scroll);
                let mut selected = false;
                let mut scroll_y = self.agent_menu_scroll_y;
                ui.scope_builder(
                    UiBuilder::new()
                        .id_salt("agent_menu")
                        .max_rect(popup)
                        .layout(Layout::top_down(Align::LEFT)),
                    |ui| {
                        ui.set_clip_rect(ui.ctx().content_rect());
                        ui.painter().add(
                            theme::shadow::popover()
                                .as_shape(popup, theme::corner(theme::radius::CARD)),
                        );
                        ui.painter().rect_filled(
                            popup,
                            theme::corner(theme::radius::CARD),
                            theme::surface().raised,
                        );
                        ui.painter().rect_stroke(
                            popup,
                            11.0,
                            egui::Stroke::new(1.0, theme::border::strong_color()),
                            egui::StrokeKind::Inside,
                        );
                        ui.scope_builder(
                            UiBuilder::new()
                                .id_salt("agent_menu_content")
                                .max_rect(popup.shrink2(egui::vec2(6.0, menu_padding_y)))
                                .layout(Layout::top_down_justified(Align::LEFT)),
                            |ui| {
                                let list_height = ui.available_height();
                                let output = ScrollArea::vertical()
                                    .id_salt(("agent_menu_values", menu))
                                    .max_height(list_height)
                                    .auto_shrink([false, false])
                                    .scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR)
                                    .vertical_scroll_offset(scroll_y)
                                    .show(ui, |ui| {
                                        ui.spacing_mut().interact_size.y = row_height;
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                        match menu {
                                            AgentMenu::Providers => {
                                                for provider in provider_catalog() {
                                                    let packaged = self
                                                        .available_providers
                                                        .contains(&provider.id);
                                                    let reason =
                                                        provider.unavailable_reason.or_else(|| {
                                                            (!packaged).then_some(
                                                                "Unavailable in this build",
                                                            )
                                                        });
                                                    let response = ui
                                                        .add_enabled_ui(
                                                            packaged && reason.is_none(),
                                                            |ui| {
                                                                provider_menu_option(
                                                                    ui,
                                                                    provider,
                                                                    packaged,
                                                                    self.selected_provider
                                                                        == provider.id,
                                                                )
                                                            },
                                                        )
                                                        .inner;
                                                    if response.clicked() {
                                                        provider_change = Some(provider.id);
                                                        selected = true;
                                                    }
                                                }
                                            }
                                            AgentMenu::Sessions => {
                                                if let Some(sessions) = &self.agent.sessions {
                                                    if sessions.is_empty() {
                                                        ui.add_sized(
                                                            [
                                                                ui.available_width(),
                                                                AGENT_SESSION_ROW_HEIGHT,
                                                            ],
                                                            Label::new(
                                                                RichText::new(
                                                                    "No previous sessions",
                                                                )
                                                                .weak(),
                                                            ),
                                                        );
                                                    }
                                                    for session in sessions {
                                                        let (open, remove) = agent_session_row(
                                                            ui,
                                                            session,
                                                            self.selected_provider,
                                                            false,
                                                            false,
                                                        );
                                                        if open {
                                                            session_load = Some(session.id.clone());
                                                            selected = true;
                                                        }
                                                        if remove {
                                                            session_remove =
                                                                Some(session.id.clone());
                                                        }
                                                    }
                                                }
                                            }
                                            AgentMenu::Commands(query) => {
                                                let mut matches = 0;
                                                for command in &self.agent.commands {
                                                    if !command_matches(&command.name, query) {
                                                        continue;
                                                    }
                                                    matches += 1;
                                                    let label =
                                                        command.input_hint.as_ref().map_or_else(
                                                            || format!("/{}", command.name),
                                                            |hint| {
                                                                format!("/{} {hint}", command.name)
                                                            },
                                                        );
                                                    if ui.selectable_label(false, label).clicked() {
                                                        self.agent.prompt =
                                                            format!("/{} ", command.name);
                                                        self.agent_prompt_history_index = None;
                                                        self.agent_prompt_history_draft.clear();
                                                        ui.memory_mut(|memory| {
                                                            memory.request_focus(Id::new(
                                                                "agent_prompt",
                                                            ));
                                                        });
                                                        selected = true;
                                                    }
                                                }
                                                if matches == 0 {
                                                    ui.add_sized(
                                                        [
                                                            ui.available_width(),
                                                            AGENT_COMMAND_ROW_HEIGHT,
                                                        ],
                                                        Label::new(
                                                            RichText::new("No matching commands")
                                                                .weak(),
                                                        ),
                                                    );
                                                }
                                            }
                                            AgentMenu::Mentions(_) => {
                                                if self.agent_mention_matches.is_empty() {
                                                    ui.add_sized(
                                                        [
                                                            ui.available_width(),
                                                            AGENT_COMMAND_ROW_HEIGHT,
                                                        ],
                                                        Label::new(
                                                            RichText::new(
                                                                "No matching files or folders",
                                                            )
                                                            .weak(),
                                                        ),
                                                    );
                                                }
                                                for (index, entry) in self
                                                    .agent_mention_matches
                                                    .iter()
                                                    .cloned()
                                                    .enumerate()
                                                {
                                                    let response = agent_mention_row(
                                                        ui,
                                                        &entry,
                                                        index == self.agent_mention_selected,
                                                    );
                                                    if index == self.agent_mention_selected {
                                                        response.scroll_to_me(Some(Align::Center));
                                                    }
                                                    if response.clicked() {
                                                        mention_attach = Some(entry.path);
                                                        remove_agent_mention(
                                                            &mut self.agent.prompt,
                                                        );
                                                        ui.memory_mut(|memory| {
                                                            memory.request_focus(Id::new(
                                                                "agent_prompt",
                                                            ));
                                                        });
                                                        selected = true;
                                                    }
                                                }
                                            }
                                            AgentMenu::Permissions => {
                                                for (enabled, label) in
                                                    [(false, "Ask"), (true, "Allow all")]
                                                {
                                                    if agent_menu_option(
                                                        ui,
                                                        label,
                                                        self.agent_run_everything.unwrap_or(false)
                                                            == enabled,
                                                        row_height,
                                                    )
                                                    .clicked()
                                                    {
                                                        run_everything_change = Some(enabled);
                                                        selected = true;
                                                    }
                                                }
                                            }
                                            AgentMenu::Mode => {
                                                let current = self
                                                    .agent
                                                    .current_mode
                                                    .as_deref()
                                                    .unwrap_or_default();
                                                for mode in &self.agent.modes {
                                                    let response = agent_menu_option(
                                                        ui,
                                                        &mode.name,
                                                        current == mode.id,
                                                        row_height,
                                                    );
                                                    if response.clicked() {
                                                        mode_change = Some(mode.id.clone());
                                                        selected = true;
                                                    }
                                                }
                                            }
                                            AgentMenu::Config(id) => {
                                                if let Some(option) = self
                                                    .agent
                                                    .config_options
                                                    .iter()
                                                    .find(|option| option.id == *id)
                                                    && let ConfigValue::Select(current) =
                                                        &option.value
                                                {
                                                    let model_menu =
                                                        is_model_config(&option.id, &option.name);
                                                    let combined_thinking = model_menu
                                                        && self.agent.config_options.iter().any(
                                                            |other| {
                                                                is_thinking_config(
                                                                    &other.id,
                                                                    &other.name,
                                                                )
                                                            },
                                                        );
                                                    let combined_fast = model_menu
                                                        .then(|| {
                                                            fast_mode_config(
                                                                &self.agent.config_options,
                                                            )
                                                        })
                                                        .flatten();
                                                    if combined_thinking {
                                                        for thinking in
                                                            self.agent.config_options.iter().filter(
                                                                |other| {
                                                                    is_thinking_config(
                                                                        &other.id,
                                                                        &other.name,
                                                                    )
                                                                },
                                                            )
                                                        {
                                                            match &thinking.value {
                                                                ConfigValue::Select(current) => {
                                                                    agent_menu_section_label(
                                                                        ui,
                                                                        &thinking.name,
                                                                        row_height,
                                                                    );
                                                                    for value in &thinking.options {
                                                                        if agent_menu_option(
                                                                            ui,
                                                                            &value.name,
                                                                            value.id == *current,
                                                                            row_height,
                                                                        )
                                                                        .clicked()
                                                                        {
                                                                            config_changes.push((
                                                                                thinking.id.clone(),
                                                                                ConfigValue::Select(
                                                                                    value
                                                                                        .id
                                                                                        .clone(),
                                                                                ),
                                                                            ));
                                                                            selected = true;
                                                                        }
                                                                    }
                                                                }
                                                                ConfigValue::Boolean(enabled) => {
                                                                    if agent_toggle_row(
                                                                        ui,
                                                                        &thinking.name,
                                                                        *enabled,
                                                                        row_height,
                                                                    )
                                                                    .clicked()
                                                                    {
                                                                        config_changes.push((
                                                                            thinking.id.clone(),
                                                                            ConfigValue::Boolean(
                                                                                !enabled,
                                                                            ),
                                                                        ));
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    if combined_fast.is_some() {
                                                        agent_menu_section_label(
                                                            ui, "Speed", row_height,
                                                        );
                                                    }
                                                    if let Some((fast, enabled, next)) =
                                                        combined_fast.as_ref()
                                                        && agent_toggle_row(
                                                            ui,
                                                            "Fast mode",
                                                            *enabled,
                                                            row_height,
                                                        )
                                                        .clicked()
                                                    {
                                                        config_changes
                                                            .push((fast.id.clone(), next.clone()));
                                                    }
                                                    if combined_thinking || combined_fast.is_some()
                                                    {
                                                        agent_menu_section_label(
                                                            ui, "Model", row_height,
                                                        );
                                                    }
                                                    for value in &option.options {
                                                        let label = if is_model_config(
                                                            &option.id,
                                                            &option.name,
                                                        ) {
                                                            model_display_name(
                                                                &value.id,
                                                                &value.name,
                                                            )
                                                        } else {
                                                            Cow::Borrowed(value.name.as_str())
                                                        };
                                                        let response = agent_menu_option(
                                                            ui,
                                                            &label,
                                                            value.id == *current,
                                                            row_height,
                                                        );
                                                        if response.clicked() {
                                                            config_changes.push((
                                                                option.id.clone(),
                                                                ConfigValue::Select(
                                                                    value.id.clone(),
                                                                ),
                                                            ));
                                                            selected = true;
                                                        }
                                                    }
                                                    if !combined_thinking
                                                        && is_thinking_config(
                                                            &option.id,
                                                            &option.name,
                                                        )
                                                        && let Some((fast, enabled, next)) =
                                                            fast_mode_config(
                                                                &self.agent.config_options,
                                                            )
                                                    {
                                                        agent_menu_section_label(
                                                            ui, "Speed", row_height,
                                                        );
                                                        if agent_toggle_row(
                                                            ui,
                                                            "Fast mode",
                                                            enabled,
                                                            row_height,
                                                        )
                                                        .clicked()
                                                        {
                                                            config_changes
                                                                .push((fast.id.clone(), next));
                                                        }
                                                    }
                                                }
                                            }
                                        }
                                    });
                                scroll_y = output.state.offset.y;
                            },
                        );
                    },
                );
                self.agent_menu_scroll_y = scroll_y;
                let close = selected
                    || ui.input(|input| input.key_pressed(Key::Escape))
                    || (!menu_toggled
                        && ui.input(|input| {
                            input.pointer.any_click()
                                && input.pointer.interact_pos().is_some_and(|position| {
                                    !popup.contains(position) && !anchor.contains(position)
                                })
                        }));
                if close {
                    open_menu = None;
                }
            } else {
                open_menu = None;
            }
        } else if open_menu.is_some() {
            open_menu = None;
        }
        if self.agent_menu != open_menu {
            self.agent_menu_scroll_y = 0.0;
        }
        if open_menu.is_none() {
            menu_popup = None;
        }
        self.agent_menu_popup = menu_popup;
        self.agent_menu = open_menu;

        if let Some(path) = mention_attach {
            self.attach_agent_files(ui.ctx(), [path]);
            ui.memory_mut(|memory| memory.request_focus(Id::new("agent_prompt")));
            ui.ctx().request_repaint();
        }
        if open_file_picker {
            self.open_agent_file_picker();
            ui.ctx().request_repaint();
        }

        for (request_id, option_id) in permission_decisions {
            if self.agent.decide_permission(request_id, &option_id)
                && let Some(controller) = self.agent_controllers.get(&self.selected_provider)
            {
                let _ = controller.send(AgentCommand::DecidePermission {
                    request_id,
                    option_id,
                });
            }
        }
        for (request_id, response) in interaction_responses {
            if self.agent.answer_interaction(request_id)
                && let Some(controller) = self.agent_controllers.get(&self.selected_provider)
            {
                let _ = controller.send(AgentCommand::RespondInteraction {
                    request_id,
                    response,
                });
            }
        }
        if let Some(source) = open_image_request {
            self.agent_image_lightbox = Some(source);
        }
        if let Some((path, line)) = open_path_request {
            self.open_agent_path(path, line);
        }
        if let Some(path) = open_diff_request {
            self.open_agent_diff(path);
        }
        if let Some(enabled) = run_everything_change
            && let Some(controller) = self.agent_controllers.get(&self.selected_provider)
        {
            match controller.send(AgentCommand::SetRunEverything(enabled)) {
                Ok(()) => {
                    self.agent_run_everything = Some(enabled);
                }
                Err(error) => self.show_error(error),
            }
        }
        if let Some(provider) = provider_change {
            self.request_provider_switch(provider, ui.ctx());
        }
        if let Some(controller) = self.agent_controllers.get(&self.selected_provider) {
            if let Some(session_id) = session_remove {
                let _ = controller.send(AgentCommand::RemoveSession(session_id));
            }
            if let Some(session_id) = session_load {
                let _ = controller.send(AgentCommand::LoadSession(session_id));
            }
            if let Some(mode) = mode_change {
                let _ = controller.send(AgentCommand::SetMode(mode));
            }
            for (id, value) in config_changes {
                let _ = controller.send(AgentCommand::SetConfig { id, value });
            }
            if cancel {
                let _ = controller.send(AgentCommand::Cancel);
            }
        }
        if send || submit_shortcut {
            self.queue_agent_prompt();
        }
        if new_session && let Some(controller) = self.agent_controllers.get(&self.selected_provider)
        {
            let _ = controller.send(AgentCommand::NewSession);
        }
        if let Some(method) = authenticate
            && let Some(controller) = self.agent_controllers.get(&self.selected_provider)
        {
            let _ = controller.send(AgentCommand::Authenticate(method));
        }
        if reconnect {
            self.reconnect_agent(ui.ctx());
        }
    }
}
