use super::*;

pub(super) fn cursor_cloud_prompt(
    provider: ProviderId,
    prompt: &str,
    has_attachments: bool,
) -> Result<Option<String>, String> {
    if provider != ProviderId::Cursor || !prompt.starts_with('&') {
        return Ok(None);
    }
    if has_attachments {
        return Err("Cursor Cloud prompts do not support local attachments".into());
    }
    let prompt = prompt[1..].trim();
    if prompt.is_empty() {
        return Err("enter a prompt after & to start Cursor Cloud".into());
    }
    Ok(Some(prompt.to_owned()))
}

pub(super) fn next_failover_account(
    accounts: &[ProviderAccount],
    provider: ProviderId,
    attempted: &HashSet<u64>,
    exhausted: &HashMap<AccountKey, Option<SystemTime>>,
    now: SystemTime,
) -> Option<AccountKey> {
    accounts
        .iter()
        .find(|account| {
            account.key.provider == provider
                && account.auto_failover
                && !attempted.contains(&account.key.account_id)
                && match exhausted.get(&account.key) {
                    None => true,
                    Some(Some(reset_at)) => *reset_at <= now,
                    Some(None) => false,
                }
        })
        .map(|account| account.key)
}

fn scoped_agent_id(
    agentic_mode: bool,
    pane: PaneId,
    source: impl std::hash::Hash + std::fmt::Debug,
) -> Id {
    if agentic_mode && pane != PaneId(0) {
        Id::new((source, pane.0))
    } else {
        Id::new(source)
    }
}

impl EditorApp {
    fn agent_id(&self, source: impl std::hash::Hash + std::fmt::Debug) -> Id {
        scoped_agent_id(self.agentic_mode, self.active_agent_pane, source)
    }

    pub(super) fn draw_agent_sidebar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        draw_assistant_sidebar_surface(ui, rect);
        self.draw_agent(ui, rect);
    }

    pub(super) fn open_agent(&mut self, ctx: &egui::Context) {
        self.ensure_provider_catalog();
        let fresh_session = std::mem::take(&mut self.agent_boot_fresh_session);
        self.start_provider(self.selected_provider, ctx, fresh_session);
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
        let query_id = self.agent_id("agent_find_query");
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
        if let Ok(directory) = data_dir() {
            match crate::agent::provider::load_accounts(&directory, &self.available_providers) {
                Ok(accounts) => self.accounts = accounts,
                Err(error) => self.show_error(error),
            }
        }
        self.selected_account = self
            .accounts
            .account(self.accounts.selected)
            .filter(|account| self.available_providers.contains(&account.key.provider))
            .or_else(|| {
                self.accounts
                    .accounts
                    .iter()
                    .find(|account| self.available_providers.contains(&account.key.provider))
            })
            .map(|account| account.key)
            .unwrap_or(self.accounts.selected);
        self.selected_provider = self.selected_account.provider;
    }

    pub(super) fn warm_providers(&mut self, ctx: &egui::Context) {
        self.ensure_provider_catalog();
        self.start_provider(self.selected_provider, ctx, false);
    }

    pub(super) fn start_provider(
        &mut self,
        provider: ProviderId,
        ctx: &egui::Context,
        fresh_session: bool,
    ) {
        let Some(account) = self
            .accounts
            .accounts_for(provider)
            .find(|account| account.key == self.selected_account)
            .or_else(|| self.accounts.accounts_for(provider).next())
            .cloned()
        else {
            return;
        };
        self.start_account(account, ctx, fresh_session);
    }

    fn start_account(
        &mut self,
        account: ProviderAccount,
        ctx: &egui::Context,
        fresh_session: bool,
    ) {
        let key = account.key;
        if self.agent_controllers.contains_key(&key) {
            return;
        }
        let state = if key == self.selected_account {
            &mut self.agent
        } else {
            self.provider_agents.entry(key).or_default()
        };
        state.session_ready = false;
        state.active = false;
        state.connection = ConnectionState::Starting;
        let preferred_session = state.session_id.clone();
        let wake = ctx.clone();
        self.agent_controllers.insert(
            key,
            AgentController::start_account_with_wake(
                account,
                self.agent_project_root.clone(),
                preferred_session,
                fresh_session,
                move || wake.request_repaint(),
            ),
        );
    }

    pub(super) fn reconnect_agent(&mut self, ctx: &egui::Context) {
        if let Some(controller) = self.agent_controllers.remove(&self.selected_account) {
            drop(controller);
        }
        let provider = self.selected_provider;
        self.start_provider(provider, ctx, false);
    }

    pub(super) fn request_provider_switch(&mut self, target: ProviderId, ctx: &egui::Context) {
        if target == self.selected_provider || !self.available_providers.contains(&target) {
            return;
        }
        let Some(target) = self
            .accounts
            .accounts_for(target)
            .next()
            .map(|account| account.key)
        else {
            return;
        };
        self.request_account_switch(target, ctx);
    }

    pub(super) fn can_switch_accounts(&self) -> bool {
        !self.agent.active && !self.agent.waiting_permission()
    }

    /// Every account plus one placeholder row for any catalog provider the
    /// registry does not know yet.
    fn provider_menu_row_count(&self) -> usize {
        provider_catalog()
            .iter()
            .map(|provider| self.accounts.accounts_for(provider.id).count().max(1))
            .sum()
    }

    pub(super) fn request_account_switch(&mut self, target: AccountKey, ctx: &egui::Context) {
        if target == self.selected_account
            || !self.available_providers.contains(&target.provider)
            || self.accounts.account(target).is_none()
            || !self.can_switch_accounts()
        {
            return;
        }
        self.agent_failover = None;
        self.exhausted_accounts.remove(&target);
        self.select_account_state(target);
        self.accounts.selected = target;
        if let Ok(directory) = data_dir()
            && let Err(error) = crate::agent::provider::save_accounts(&directory, &self.accounts)
        {
            self.show_error(error);
        }
        self.agent_menu = None;
        self.agent_menu_popup = None;
        self.agent_prompt_history_index = None;
        self.agent_prompt_history_draft.clear();
        self.agent_follow_transcript = true;
        self.agent_find.dirty = true;
        self.attachment_file_picker = None;
        self.agent_run_everything = None;
        self.start_provider(target.provider, ctx, false);
    }

    #[cfg(test)]
    pub(super) fn select_provider_state(&mut self, target: ProviderId) {
        let Some(target) = self
            .accounts
            .accounts_for(target)
            .next()
            .map(|account| account.key)
        else {
            return;
        };
        self.select_account_state(target);
    }

    pub(super) fn select_account_state(&mut self, target: AccountKey) {
        if target == self.selected_account || self.accounts.account(target).is_none() {
            return;
        }
        let draft = std::mem::take(&mut self.agent.prompt);
        let mut next = self.provider_agents.remove(&target).unwrap_or_default();
        next.prompt = draft;
        let previous = std::mem::replace(&mut self.agent, next);
        self.provider_agents.insert(self.selected_account, previous);
        self.selected_account = target;
        self.selected_provider = target.provider;
    }

    fn commit_accounts(&mut self, accounts: AccountRegistry) -> Result<(), String> {
        let directory = data_dir()?;
        crate::agent::provider::save_accounts(&directory, &accounts)?;
        self.accounts = accounts;
        Ok(())
    }

    pub(super) fn finish_account_prompt(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let Some(prompt) = self.account_prompt.as_ref() else {
            return Ok(());
        };
        let mut accounts = self.accounts.clone();
        match prompt.action {
            AccountPromptAction::Add(provider) => {
                let key = accounts.add_account(
                    provider,
                    prompt.label.trim().to_owned(),
                    AuthSource::ProviderManaged,
                )?;
                self.commit_accounts(accounts)?;
                self.account_prompt = None;
                self.request_account_switch(key, ctx);
            }
            AccountPromptAction::Rename(key) => {
                let account = accounts
                    .accounts
                    .iter_mut()
                    .find(|account| account.key == key)
                    .ok_or_else(|| "account no longer exists".to_owned())?;
                account.label = prompt.label.trim().to_owned();
                self.commit_accounts(accounts)?;
                self.account_prompt = None;
            }
        }
        Ok(())
    }

    pub(super) fn poll_agent_panes(&mut self, ctx: &egui::Context) -> bool {
        let selected = self.active_agent_pane;
        let mut any_active = false;
        for pane in self.agent_pane_layout.panes() {
            if self.agent_pane_picker == Some(pane) || !self.activate_agent_session_pane(pane) {
                continue;
            }
            self.poll_agent(ctx);
            any_active |= self.agent.active;
        }
        self.activate_agent_session_pane(selected);
        any_active
    }

    pub(super) fn poll_agent(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        for (&account, controller) in &self.agent_controllers {
            for _ in 0..8 {
                let Ok(event) = controller.events().try_recv() else {
                    break;
                };
                events.push((account, event));
            }
        }
        if events.len() >= 8 {
            ctx.request_repaint();
        }
        let mut changed_paths = HashSet::new();
        let mut reconcile_all = false;
        let mut refresh_providers = HashSet::new();
        for (account, event) in events {
            let selected = account == self.selected_account;
            if let AgentEvent::ProjectSessionsUpdated { project, sessions } = &event {
                self.agent_project_sessions
                    .insert((account, project.clone()), Some(sessions.clone()));
                if selected && project == &self.agent_project_root {
                    self.agent.sessions = Some(sessions.clone());
                    self.agent_sidebar_history_pending = false;
                }
            }
            let usage_exhausted = match &event {
                AgentEvent::TurnFailed {
                    kind: TurnFailureKind::UsageExhausted { reset_at },
                    ..
                } => Some(*reset_at),
                _ => None,
            };
            let failover_ready = matches!(event, AgentEvent::SessionReady { .. });
            let failover_start_failed = matches!(
                &event,
                AgentEvent::ConnectionChanged(ConnectionState::AuthenticationRequired(_))
                    | AgentEvent::ProcessExited { .. }
            );
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
                self.attachment_file_picker = None;
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
            if let AgentEvent::ToolCallUpdated(tool) = &event {
                changed_paths.extend(tool.paths.iter().map(|tool_path| tool_path.path.clone()));
            }
            let turn_finished = matches!(event, AgentEvent::TurnFinished { .. });
            let refresh_project =
                turn_finished || matches!(event, AgentEvent::ProcessExited { .. });
            if selected {
                self.agent.apply(event);
            } else {
                self.provider_agents
                    .entry(account)
                    .or_default()
                    .apply(event);
            }
            if selected && let Some(reset_at) = usage_exhausted {
                self.exhausted_accounts.insert(account, reset_at);
                if let Some(attempt) = &mut self.agent_failover
                    && matches!(attempt.stage, FailoverStage::Running)
                {
                    attempt.pending_exhaustion = true;
                }
            }
            if selected && failover_ready {
                self.finish_failover_start(ctx);
            } else if selected && failover_start_failed {
                self.skip_failed_failover_candidate(ctx);
            }
            if selected && turn_finished {
                if self
                    .agent_failover
                    .as_ref()
                    .is_some_and(|attempt| attempt.pending_exhaustion)
                {
                    self.start_next_failover(ctx, true);
                } else if self
                    .agent_failover
                    .as_ref()
                    .is_some_and(|attempt| matches!(attempt.stage, FailoverStage::Running))
                {
                    self.agent_failover = None;
                }
            }
            if turn_finished {
                reconcile_all = true;
            }
            if refresh_project {
                refresh_providers.insert(account);
            }
        }
        if reconcile_all {
            self.reconcile_open_buffer();
            self.refresh_agentic_diff();
        } else if !changed_paths.is_empty() {
            self.reconcile_open_paths(&changed_paths);
            self.refresh_agentic_diff_paths(&changed_paths);
        }
        for account in refresh_providers {
            self.refresh_after_agent(account);
        }
    }

    fn start_next_failover(&mut self, ctx: &egui::Context, rebuild_handoff: bool) {
        let Some(mut attempt) = self.agent_failover.take() else {
            return;
        };
        if rebuild_handoff {
            attempt.pending_exhaustion = false;
            let from = self.account_display_name(self.selected_account);
            attempt.visible_transcript = Some(self.agent.transcript.clone());
            attempt.from_label = Some(from);
        }
        let Some(target) = next_failover_account(
            &self.accounts.accounts,
            attempt.provider,
            &attempt.attempted_accounts,
            &self.exhausted_accounts,
            SystemTime::now(),
        ) else {
            if let Some(transcript) = attempt.visible_transcript.take() {
                self.agent.restore_account_handoff(transcript);
            }
            self.agent.apply(AgentEvent::Error(
                "All enabled accounts for this provider are exhausted or unavailable. Select an account or retry after its plan resets."
                    .into(),
            ));
            return;
        };
        let Some(account) = self.accounts.account(target).cloned() else {
            return;
        };
        if let Some(transcript) = attempt.visible_transcript.clone() {
            self.agent.restore_account_handoff(transcript);
        }
        let from = attempt.from_label.as_deref().unwrap_or("Previous account");
        let to = self.account_display_name(target);
        attempt.handoff =
            match self
                .agent
                .account_handoff_prompt(from, &to, &attempt.original_prompt.text)
            {
                Ok(handoff) => Some(handoff),
                Err(error) => {
                    self.show_error(error);
                    return;
                }
            };
        attempt.attempted_accounts.insert(target.account_id);
        self.select_account_state(target);
        self.accounts.selected = target;
        if let Ok(directory) = data_dir()
            && let Err(error) = crate::agent::provider::save_accounts(&directory, &self.accounts)
        {
            self.show_error(error);
        }
        let resumed = self
            .agent_controllers
            .get(&target)
            .is_some_and(|controller| controller.send(AgentCommand::NewSession).is_ok());
        if resumed {
            self.agent.session_ready = false;
        } else {
            self.agent_controllers.remove(&target);
            self.start_account(account, ctx, true);
        }
        if let Some(transcript) = attempt.visible_transcript.clone() {
            self.agent.restore_account_handoff(transcript);
        }
        self.agent_follow_transcript = true;
        self.agent_find.dirty = true;
        attempt.stage = FailoverStage::Starting(target);
        self.agent_failover = Some(attempt);
    }

    fn finish_failover_start(&mut self, ctx: &egui::Context) {
        let Some(mut attempt) = self.agent_failover.take() else {
            return;
        };
        let FailoverStage::Starting(target) = attempt.stage else {
            self.agent_failover = Some(attempt);
            return;
        };
        if target != self.selected_account {
            self.agent_failover = Some(attempt);
            return;
        }
        let Some(handoff) = attempt.handoff.take() else {
            return;
        };
        if let Some(transcript) = attempt.visible_transcript.take() {
            self.agent.restore_account_handoff(transcript);
        }
        let from = attempt
            .from_label
            .take()
            .unwrap_or_else(|| "Previous account".into());
        let to = self.account_display_name(target);
        self.agent.record_account_switch(from, to);
        let result = self
            .agent_controllers
            .get(&target)
            .ok_or_else(|| "replacement account controller did not start".to_owned())
            .and_then(|controller| {
                controller.send(AgentCommand::HiddenPromptWithAttachments {
                    text: handoff.clone(),
                    attachments: attempt.original_prompt.attachments.clone(),
                })
            });
        match result {
            Ok(()) => {
                self.agent.active = true;
                attempt.stage = FailoverStage::Running;
                self.agent_failover = Some(attempt);
            }
            Err(error) => {
                attempt.handoff = Some(handoff);
                attempt.visible_transcript = Some(self.agent.transcript.clone());
                self.agent_failover = Some(attempt);
                self.show_error(error);
                self.skip_failed_failover_candidate(ctx);
            }
        }
    }

    fn skip_failed_failover_candidate(&mut self, ctx: &egui::Context) {
        if self
            .agent_failover
            .as_ref()
            .is_some_and(|attempt| matches!(attempt.stage, FailoverStage::Starting(_)))
        {
            self.start_next_failover(ctx, false);
        }
    }

    fn account_display_name(&self, key: AccountKey) -> String {
        let provider = provider_descriptor(key.provider).display_name;
        self.accounts.account(key).map_or_else(
            || format!("{provider} · Account {}", key.account_id),
            |account| format!("{provider} · {}", account.label),
        )
    }

    pub(super) fn reconcile_open_buffer(&mut self) {
        self.reconcile_open_buffers(None);
    }

    pub(super) fn reconcile_open_paths(&mut self, paths: &HashSet<PathBuf>) {
        self.reconcile_open_buffers(Some(paths));
    }

    fn reconcile_open_buffers(&mut self, paths: Option<&HashSet<PathBuf>>) {
        let mut active_reloaded = false;
        let mut conflict = None;
        let mut error = None;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if paths.is_some_and(|paths| !paths.contains(&tab.buffer.path)) {
                continue;
            }
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

    pub(super) fn refresh_after_agent(&mut self, account: AccountKey) {
        let changed = if account == self.selected_account {
            std::mem::take(&mut self.agent.refresh_queue)
        } else {
            std::mem::take(
                &mut self
                    .provider_agents
                    .entry(account)
                    .or_default()
                    .refresh_queue,
            )
        };
        if self.agent_project_root != self.tree.root {
            return;
        }
        self.schedule_git_refresh();
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
        let prompt = self.agent.prompt.trim().to_owned();
        if (prompt.is_empty() && self.agent_attachments.is_empty())
            || (self.agent.active && !self.agent.steering)
            || !self.agent.session_ready
        {
            return;
        }
        let was_active = self.agent.active;
        let attachments = self
            .agent_attachments
            .iter()
            .map(|attachment| attachment.file.clone())
            .collect::<Vec<_>>();
        let cloud_prompt =
            match cursor_cloud_prompt(self.selected_provider, &prompt, !attachments.is_empty()) {
                Ok(prompt) => prompt,
                Err(error) => {
                    self.show_error(error);
                    return;
                }
            };
        let Some(controller) = self.agent_controllers.get(&self.selected_account) else {
            return;
        };
        let envelope = PromptEnvelope {
            text: prompt.clone(),
            attachments: attachments.clone(),
        };
        self.agent.active = true;
        let command = cloud_prompt
            .clone()
            .map(AgentCommand::CloudPrompt)
            .unwrap_or(AgentCommand::PromptWithAttachments {
                text: prompt,
                attachments,
            });
        match controller.send(command) {
            Ok(()) => {
                self.agent_prompt_history_index = None;
                self.agent_prompt_history_draft.clear();
                if !was_active {
                    self.exhausted_accounts.remove(&self.selected_account);
                    self.agent_failover = if cloud_prompt.is_some() {
                        None
                    } else {
                        self.accounts
                            .accounts_for(self.selected_provider)
                            .any(|account| {
                                account.key != self.selected_account && account.auto_failover
                            })
                            .then(|| FailoverAttempt {
                                provider: self.selected_provider,
                                attempted_accounts: HashSet::from([self
                                    .selected_account
                                    .account_id]),
                                original_prompt: envelope,
                                pending_exhaustion: false,
                                stage: FailoverStage::Running,
                                handoff: None,
                                visible_transcript: None,
                                from_label: None,
                            })
                    };
                }
            }
            Err(error) => {
                self.agent.active = was_active;
                self.show_error(error);
            }
        }
    }

    pub(super) fn attach_agent_files(
        &mut self,
        ctx: &egui::Context,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        if let Some(error) = stage_composer_files(ctx, &mut self.agent_attachments, paths, true) {
            self.show_error(error);
        }
    }

    pub(super) fn attach_devin_files(
        &mut self,
        ctx: &egui::Context,
        paths: impl IntoIterator<Item = PathBuf>,
    ) {
        if let Some(error) = stage_composer_files(ctx, &mut self.devin_attachments, paths, false) {
            self.show_error(error);
        }
    }

    pub(super) fn open_agent_file_picker(&mut self) {
        self.open_attachment_file_picker(AttachmentTarget::Agent);
    }

    pub(super) fn open_devin_file_picker(&mut self) {
        self.open_attachment_file_picker(AttachmentTarget::Devin);
    }

    fn open_attachment_file_picker(&mut self, target: AttachmentTarget) {
        let directory = if target == AttachmentTarget::Agent {
            self.agent_project_root.clone()
        } else {
            self.tree.root.clone()
        };
        let picker = (directory == self.tree.root)
            .then(|| self.tree.children.get(&directory))
            .flatten()
            .cloned()
            .map(|entries| WorkspaceFilePicker::with_entries(directory.clone(), entries))
            .map(Ok)
            .unwrap_or_else(|| WorkspaceFilePicker::open(directory));
        match picker {
            Ok(picker) => {
                self.attachment_file_picker = Some(picker);
                self.attachment_picker_target = target;
            }
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
        let agentic_mode = self.agentic_mode;
        let agent_pane = self.active_agent_pane;
        let compact_agent_composer = self.agentic_mode && rect.width() <= AGENT_PANE_COMPACT_WIDTH;
        let floating_agent_composer = self.agentic_mode && !compact_agent_composer;
        // The exact width the prompt is laid out at later, so the measured
        // text height matches what the composer actually shows.
        let prompt_width = if floating_agent_composer {
            (rect.width() - 2.0 * theme::space::WIDE).min(AGENTIC_CONTENT_WIDTH)
                - 2.0 * theme::space::MEDIUM
        } else {
            rect.width() - 2.0 * theme::space::MEDIUM
        };
        let composer_height = measure_assistant_composer_height(
            ui,
            &self.agent.prompt,
            prompt_width,
            rect.height(),
            !self.agent_attachments.is_empty(),
        );
        let composer_height = if floating_agent_composer {
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
        let mut new_pane = false;
        let mut session_menu_anchor = None;
        let mut session_menu_toggled = false;
        let mut provider_menu_toggled = false;
        let painter = ui.painter().clone();
        paint_assistant_header_divider(&painter, header);
        let project_title = self
            .agent_project_root
            .file_name()
            .unwrap_or(self.agent_project_root.as_os_str())
            .to_string_lossy();
        let title = self.agent.title.as_deref().unwrap_or(if self.agentic_mode {
            project_title.as_ref()
        } else {
            "Agent"
        });
        let mut title_x = if self.agentic_mode && !self.sidebar {
            let titlebar = ui
                .ctx()
                .content_rect()
                .with_min_y(header.top())
                .with_max_y(header.bottom());
            let file_tree_button = file_tree_toggle_rect(titlebar, header);
            let terminal_button = terminal_toggle_rect(file_tree_button);
            let source_control_button = source_control_toggle_rect(terminal_button);
            agentic_toggle_rect(source_control_button, None).right() + theme::space::SMALL
        } else {
            #[cfg(target_os = "macos")]
            {
                header.left() + 14.0
            }
            #[cfg(not(target_os = "macos"))]
            {
                header.left() + if self.agentic_mode { 76.0 } else { 14.0 }
            }
        };
        if !self.agentic_mode
            && provider_selector_visible(&self.available_providers, self.accounts.accounts.len())
        {
            let provider_rect = egui::Rect::from_min_max(
                egui::pos2(header.left() + 10.0, header.top() + 3.0),
                egui::pos2(
                    (header.left() + 50.0).min(header.right()),
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
                        draw_provider_selector_identity(ui, self.selected_provider, true, false);
                    self.provider_menu_anchor = Some(response.rect);
                    if response.clicked() {
                        let menu = AgentMenu::Providers;
                        self.agent_menu = (self.agent_menu.as_ref() != Some(&menu)).then_some(menu);
                        provider_menu_toggled = true;
                    }
                },
            );
            painter.vline(
                header.left() + 52.0,
                (header.center().y - 7.0)..=(header.center().y + 7.0),
                egui::Stroke::new(1.0, theme::border::strong_color()),
            );
            title_x = header.left() + 64.0;
        } else if !self.agentic_mode {
            self.provider_menu_anchor = None;
        }
        let agent_panes = self.agent_pane_layout.panes();
        if self.agentic_mode
            && agent_panes.len() > 1
            && let Some(index) = agent_panes.iter().position(|pane| *pane == agent_pane)
        {
            let badge = egui::Rect::from_center_size(
                egui::pos2(title_x + 14.0, header.center().y),
                egui::vec2(28.0, 18.0),
            );
            draw_agent_pane_badge(
                ui,
                badge,
                index + 1,
                scoped_agent_id(agentic_mode, agent_pane, "agent_pane_badge"),
            );
            title_x = badge.right() + theme::space::SMALL;
        }
        if !self.agentic_mode && self.agent.session_ready && self.agent.history_available {
            let button = agent_session_selector_rect(ui, header, title_x, title);
            let response = draw_agent_session_selector(ui, button, title);
            if response.clicked() {
                let menu = AgentMenu::Sessions;
                self.agent_menu = (self.agent_menu.as_ref() != Some(&menu)).then_some(menu.clone());
                session_menu_toggled = true;
                if self.agent_menu.is_some()
                    && let Some(controller) = self.agent_controllers.get(&self.selected_account)
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
        if self.agentic_mode {
            if agent_panes.len() > 1 {
                let pane_title = title.to_owned();
                let (close, dragging) =
                    draw_agent_pane_controls(ui, header, agent_pane, &pane_title, true);
                if close {
                    self.agent_pane_close_requested = Some(agent_pane);
                }
                if dragging {
                    self.agent_pane_drag = Some(AgentPaneDrag {
                        pane: agent_pane,
                        title: pane_title,
                    });
                }
            }
            let button = agent_pane_button_rect(header, ui.ctx().content_rect().right());
            let response = ui
                .interact(
                    button,
                    scoped_agent_id(agentic_mode, agent_pane, "agent_new_pane"),
                    Sense::click(),
                )
                .on_hover_text("Open session pane");
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    "Open session pane",
                )
            });
            let color = if response.hovered() {
                theme::text().primary
            } else {
                theme::text().secondary
            };
            let icon = egui::Rect::from_center_size(button.center(), egui::vec2(16.0, 12.0));
            painter.rect_stroke(
                icon,
                1.5,
                egui::Stroke::new(1.2, color),
                egui::StrokeKind::Inside,
            );
            painter.vline(
                icon.center().x,
                icon.y_range(),
                egui::Stroke::new(1.2, color),
            );
            new_pane = response.clicked();
        }
        let agent_button = agent_toggle_rect(header);
        if !self.agentic_mode && self.draw_agent_toggle(ui, agent_button) {
            self.agent_sidebar = false;
            self.agent_sidebar_dragging = false;
            self.agent_menu = None;
            self.attachment_file_picker = None;
            ui.ctx().request_repaint();
        }
        if !self.agentic_mode && self.agent.session_ready {
            let button = agent_new_session_rect(header);
            let response = ui
                .interact(
                    button,
                    scoped_agent_id(agentic_mode, agent_pane, "agent_new_session"),
                    Sense::click(),
                )
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
        let mut open_image_request: Option<AssistantImageSource> = None;
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
                    let provider = provider_descriptor(self.selected_provider);
                    let title = format!("Connect {}", provider.display_name);
                    let region = ui.max_rect();
                    let width = region.width().min(288.0);
                    let height = region
                        .height()
                        .min(132.0 + methods.len() as f32 * 92.0 + if environment_only { 52.0 } else { 0.0 });
                    let top = region.top() + (region.height() - height).max(0.0) * 0.42;
                    let block = egui::Rect::from_min_size(
                        egui::pos2(region.center().x - width * 0.5, top),
                        egui::vec2(width, height),
                    );
                    let response = ui.interact(
                        block,
                        scoped_agent_id(agentic_mode, agent_pane, "agent_auth_state"),
                        Sense::hover(),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &title)
                    });
                    ui.scope_builder(
                        UiBuilder::new()
                            .id_salt("agent_auth_state_content")
                            .max_rect(block)
                            .layout(Layout::top_down(Align::LEFT)),
                        |ui| {
                            ui.set_width(width);
                            ui.vertical_centered(|ui| {
                                let (mark, _) = ui.allocate_exact_size(
                                    egui::vec2(40.0, 44.0),
                                    Sense::hover(),
                                );
                                paint_provider_icon(
                                    ui.painter(),
                                    egui::Rect::from_center_size(
                                        mark.center(),
                                        egui::Vec2::splat(40.0),
                                    ),
                                    provider.icon,
                                    theme::text().primary,
                                );
                                ui.add_space(theme::space::MEDIUM);
                                ui.label(
                                    RichText::new(&title)
                                        .font(theme::typography::title())
                                        .color(theme::text().primary),
                                );
                            });
                            ui.add_space(theme::space::XWIDE);
                            for (index, method) in methods.iter().enumerate() {
                                if index > 0 {
                                    ui.add_space(theme::space::LARGE);
                                }
                                if method.can_authenticate {
                                    let label = if method.kind == AuthKind::Environment {
                                        "Use environment API key"
                                    } else {
                                        &method.name
                                    };
                                    let button = egui::Button::new(
                                        RichText::new(label)
                                            .font(theme::typography::strong())
                                            .color(theme::text().on_accent),
                                    )
                                    .fill(theme::accent())
                                    .stroke(egui::Stroke::NONE)
                                    .corner_radius(theme::corner(theme::radius::CONTROL))
                                    .min_size(egui::vec2(width, theme::control::PRIMARY));
                                    if ui.add(button).clicked() {
                                        authenticate = Some(method.id.clone());
                                    }
                                } else {
                                    ui.label(
                                        RichText::new(&method.name)
                                            .font(theme::typography::small_strong())
                                            .color(theme::text().secondary),
                                    );
                                }
                                if let Some(description) = &method.description {
                                    ui.add_space(theme::space::TIGHT);
                                    ui.add(
                                        Label::new(
                                            RichText::new(description)
                                                .font(theme::typography::small())
                                                .color(theme::text().muted),
                                        )
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
                                    ui.add_space(theme::space::TIGHT);
                                    ui.add(
                                        Label::new(
                                            RichText::new(setup)
                                                .font(theme::typography::small())
                                                .color(theme::text().muted),
                                        )
                                        .wrap(),
                                    );
                                }
                                if let Some(details) = &method.setup {
                                    ui.add_space(theme::space::TIGHT);
                                    ui.add(
                                        Label::new(
                                            RichText::new(details)
                                                .font(theme::typography::small())
                                                .monospace()
                                                .color(theme::text().muted),
                                        )
                                        .wrap(),
                                    );
                                }
                            }
                            if environment_only {
                                ui.add_space(theme::space::XWIDE);
                                let button = egui::Button::new(
                                    RichText::new("Retry")
                                        .font(theme::typography::strong())
                                        .color(theme::text().on_accent),
                                )
                                .fill(theme::accent())
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(theme::corner(theme::radius::CONTROL))
                                .min_size(egui::vec2(width, theme::control::PRIMARY));
                                if ui.add(button).clicked() {
                                    reconnect = true;
                                }
                            }
                        },
                    );
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
                    let provider = provider_descriptor(self.selected_provider);
                    let title = format!("Connect {}", provider.display_name);
                    let region = ui.max_rect();
                    let width = region.width().min(288.0);
                    let height = region.height().min(140.0);
                    let block = egui::Rect::from_min_size(
                        egui::pos2(
                            region.center().x - width * 0.5,
                            region.top() + (region.height() - height).max(0.0) * 0.42,
                        ),
                        egui::vec2(width, height),
                    );
                    let response =
                        ui.interact(
                            block,
                            scoped_agent_id(
                                agentic_mode,
                                agent_pane,
                                "agent_disconnected_state",
                            ),
                            Sense::hover(),
                        );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &title)
                    });
                    ui.scope_builder(
                        UiBuilder::new()
                            .id_salt("agent_disconnected_state_content")
                            .max_rect(block)
                            .layout(Layout::top_down(Align::LEFT)),
                        |ui| {
                            ui.set_width(width);
                            ui.vertical_centered(|ui| {
                                let (mark, _) = ui.allocate_exact_size(
                                    egui::vec2(40.0, 44.0),
                                    Sense::hover(),
                                );
                                paint_provider_icon(
                                    ui.painter(),
                                    egui::Rect::from_center_size(
                                        mark.center(),
                                        egui::Vec2::splat(40.0),
                                    ),
                                    provider.icon,
                                    theme::text().primary,
                                );
                                ui.add_space(theme::space::MEDIUM);
                                ui.label(
                                    RichText::new(&title)
                                        .font(theme::typography::title())
                                        .color(theme::text().primary),
                                );
                            });
                            ui.add_space(theme::space::XWIDE);
                            let button = egui::Button::new(
                                RichText::new("Connect")
                                    .font(theme::typography::strong())
                                    .color(theme::text().on_accent),
                            )
                            .fill(theme::accent())
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(theme::corner(theme::radius::CONTROL))
                            .min_size(egui::vec2(width, theme::control::PRIMARY));
                            reconnect = ui.add(button).clicked();
                        },
                    );
                }
                ConnectionState::Ready if self.agent.transcript.is_empty() && !self.agent.active => {
                    let project = self
                        .agent_project_root
                        .file_name()
                        .unwrap_or(self.agent_project_root.as_os_str())
                        .to_string_lossy();
                    draw_agent_empty_state(
                        ui,
                        scoped_agent_id(agentic_mode, agent_pane, "agent_empty_state"),
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
                        .scroll_source(if menu_owns_wheel {
                            egui::scroll_area::ScrollSource::NONE
                        } else {
                            egui::scroll_area::ScrollSource::default()
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
                                let single_item_dense_cluster = dense_cluster.is_some()
                                    && dense_work_items.get(item_index + 1) != Some(&true);
                                if let Some(cluster) = dense_cluster {
                                    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
                                        ui.ctx(),
                                        scoped_agent_id(agentic_mode, agent_pane, ("dense_agent_work", item_index, cluster.active)),
                                        false,
                                    );
                                    if !find_matches.is_empty() {
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
                                    dense_work_open = assistant_dense_disclosure_row(
                                        ui,
                                        scoped_agent_id(agentic_mode, agent_pane, ("dense_agent_work", item_index, cluster.active)),
                                        &cluster.label,
                                        cluster.change,
                                        false,
                                        !find_matches.is_empty(),
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
                                        chat_user_message(ui, text, item_search, |ui| {
                                                if !user_prompt_images[item_index].is_empty() {
                                                    if !text.is_empty() {
                                                        ui.add_space(theme::space::MEDIUM);
                                                    }
                                                    ui.horizontal_wrapped(|ui| {
                                                        for data in &user_prompt_images[item_index] {
                                                            if assistant_prompt_image_preview(ui, data)
                                                                .is_some_and(|preview| {
                                                                    preview.clicked()
                                                                })
                                                            {
                                                                open_image_request = Some(
                                                                    AssistantImageSource::Bytes(
                                                                        Arc::clone(data),
                                                                    ),
                                                                );
                                                            }
                                                        }
                                                    });
                                                }
                                            });
                                    }
                                    TranscriptItem::AccountSwitch { from, to } => {
                                        egui::Frame::new()
                                            .fill(theme::surface().raised)
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                theme::border::hairline_color(),
                                            ))
                                            .inner_margin(egui::Margin::same(10))
                                            .corner_radius(6)
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.add(
                                                    Label::new(
                                                        RichText::new(format!(
                                                            "Switched from {from} to {to} after {from} reached its usage limit."
                                                        ))
                                                        .small()
                                                        .color(theme::text().muted),
                                                    )
                                                    .wrap(),
                                                );
                                            });
                                    }
                                    TranscriptItem::Assistant(text) => {
                                        if !dense_agent || dense_final_responses[item_index] {
                                            draw_provider_identity(ui, self.selected_provider);
                                        }
                                        let width = ui.available_width();
                                        let galley = assistant_markdown_galley(
                                            ui,
                                            scoped_agent_id(agentic_mode, agent_pane, ("agent_markdown", item_index, dense_agent)),
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
                                        if dense_agent && single_item_dense_cluster {
                                            egui::Frame::new()
                                                .inner_margin(egui::Margin {
                                                    left: 24,
                                                    right: 4,
                                                    top: 2,
                                                    bottom: 6,
                                                })
                                                .show(ui, add_thought);
                                        } else if dense_agent {
                                            assistant_dense_tool(
                                                ui,
                                                scoped_agent_id(agentic_mode, agent_pane, ("dense_agent_thought", item_index)),
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
                                            && single_item_dense_cluster
                                        {
                                            egui::Frame::new()
                                                .inner_margin(egui::Margin {
                                                    left: 24,
                                                    right: 4,
                                                    top: 2,
                                                    bottom: 6,
                                                })
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
                                            assistant_dense_tool(
                                                ui,
                                                scoped_agent_id(agentic_mode, agent_pane, ("dense_agent_content", item_index)),
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
                                                    ContentRole::User => {}
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
                                            assistant_dense_tool(
                                                ui,
                                                scoped_agent_id(agentic_mode, agent_pane, ("dense_agent_plan", item_index)),
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
                                        let is_terminal = agent_tool_is_terminal(tool);
                                        let terminal_command = tool.command();
                                        let terminal_output = agent_terminal_output(tool);
                                        let change = is_file_edit
                                            .then(|| tool_changes.get(&tool.id).copied())
                                            .flatten();
                                        let subagent_title = tool
                                            .detail
                                            .as_ref()
                                            .and_then(|detail| {
                                                detail.content.iter().find_map(|content| {
                                                    if let ToolOutput::Task { description, .. } = content {
                                                        Some(description.as_str())
                                                    } else {
                                                        None
                                                    }
                                                })
                                            })
                                            .unwrap_or_else(|| {
                                                title.strip_prefix("Subagent: ").unwrap_or(title)
                                            });
                                        let subagent_metadata = tool.detail.as_ref().and_then(|detail| {
                                            detail.content.iter().find_map(|content| {
                                                let ToolOutput::Task { model, duration_ms, .. } = content else {
                                                    return None;
                                                };
                                                let metadata = [
                                                    model.as_ref().map(|model| {
                                                        model_display_name(model, model).into_owned()
                                                    }),
                                                    duration_ms.map(agent_task_duration),
                                                ]
                                                .into_iter()
                                                .flatten()
                                                .collect::<Vec<_>>()
                                                .join(" · ");
                                                (!metadata.is_empty()).then_some(metadata)
                                            })
                                        });
                                        let has_body = if is_subagent {
                                            (!title_includes_paths && !tool.paths.is_empty())
                                                || tool.detail.as_ref().is_some_and(|detail| {
                                                    detail.output.as_deref().is_some_and(|text| !text.is_empty())
                                                        || detail.content.iter().any(|content| match content {
                                                            ToolOutput::Task { agents, .. } => agents.iter().any(|agent| {
                                                                agent.message.as_deref().is_some_and(|message| !message.is_empty())
                                                            }),
                                                            _ => true,
                                                        })
                                                })
                                        } else {
                                            tool.detail.is_some()
                                                || (!title_includes_paths && !tool.paths.is_empty())
                                        };
                                        let add_body = |ui: &mut egui::Ui| {
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
                                                    if is_terminal {
                                                        crate::terminal::show_transcript_terminal(
                                                            ui,
                                                            terminal_command.as_deref(),
                                                            terminal_output.as_deref().unwrap_or_default(),
                                                        );
                                                    }
                                                    for (content_index, content) in
                                                        detail.content.iter().enumerate()
                                                    {
                                                        match content {
                                                            ToolOutput::Text(_) if is_terminal => {}
                                                            ToolOutput::Text(text) => {
                                                                if is_subagent {
                                                                    agent_search_label(
                                                                        ui,
                                                                        "Output",
                                                                        theme::typography::small(),
                                                                        theme::text().muted,
                                                                        item_search,
                                                                    );
                                                                }
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
                                                                        scoped_agent_id(agentic_mode, agent_pane, (
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
                                                            ToolOutput::Log { label, .. }
                                                                if is_terminal
                                                                    && label.eq_ignore_ascii_case(
                                                                        "Terminal output",
                                                                    ) => {}
                                                            ToolOutput::Log { label, text } => {
                                                                agent_search_label(
                                                                    ui,
                                                                    label,
                                                                    theme::typography::small(),
                                                                    theme::text().muted,
                                                                    item_search,
                                                                );
                                                                agent_search_label(
                                                                    ui,
                                                                    text,
                                                                    theme::typography::body(),
                                                                    theme::text().primary,
                                                                    item_search,
                                                                );
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
                                                                        scoped_agent_id(agentic_mode, agent_pane, (
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
                                                            ToolOutput::Terminal(_) if is_terminal => {}
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
                                                                description: _,
                                                                prompt: _,
                                                                subagent_type: _,
                                                                model: _,
                                                                agent_id: _,
                                                                agents,
                                                                path: _,
                                                                activity: _,
                                                                duration_ms: _,
                                                            } => {
                                                                for agent in agents {
                                                                    if let Some(message) = &agent.message {
                                                                        agent_search_label(
                                                                            ui,
                                                                            message,
                                                                            theme::typography::body(),
                                                                            theme::text().primary,
                                                                            item_search,
                                                                        );
                                                                    }
                                                                }
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
                                                                            AssistantImageSource::Path(
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
                                                        && !is_terminal
                                                        && let Some(text) = if is_subagent {
                                                            detail.output.as_deref()
                                                        } else {
                                                            detail.output.as_deref().or(detail.input.as_deref())
                                                        }
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
                                        if is_subagent {
                                            agent_subagent_card(
                                                ui,
                                                ("subagent", item_index),
                                                subagent_title,
                                                subagent_metadata.as_deref(),
                                                tool.status.as_deref(),
                                                transcript_width,
                                                item_search,
                                                has_body,
                                                add_body,
                                            );
                                        } else if dense_agent {
                                                assistant_dense_tool(
                                                    ui,
                                                    scoped_agent_id(agentic_mode, agent_pane, ("dense_agent_tool", item_index)),
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
                                                contains_diff && !is_file_edit,
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
                                                        let response = ui.add(
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
                                                            if question.options.is_empty() {
                                                                if selected.is_empty() {
                                                                    selected.push(String::new());
                                                                }
                                                                let value = &mut selected[0];
                                                                ui.add_enabled(
                                                                    !card.answered,
                                                                    egui::TextEdit::singleline(value)
                                                                        .password(question.secret)
                                                                        .hint_text(match question.value_kind {
                                                                            crate::agent::controller::QuestionValueKind::Number => "Number",
                                                                            crate::agent::controller::QuestionValueKind::Integer => "Whole number",
                                                                            _ if question.secret => "Secret",
                                                                            _ => "Answer",
                                                                        }),
                                                                );
                                                            }
                                                        }
                                                        if !card.answered {
                                                            ui.horizontal(|ui| {
                                                                let complete = questions.iter().all(
                                                                    |question| {
                                                                        if !question.required {
                                                                            return true;
                                                                        }
                                                                        card.selections
                                                                            .get(&question.id)
                                                                            .is_some_and(|answer| {
                                                                                answer.iter().any(|value| !value.is_empty())
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
                                                    InteractionKind::Url { title, url } => {
                                                        ui.label(
                                                            RichText::new(title)
                                                                .strong()
                                                                .color(theme::text().primary),
                                                        );
                                                        ui.add(
                                                            egui::Hyperlink::from_label_and_url(
                                                                "Open authentication page",
                                                                url,
                                                            )
                                                            .open_in_new_tab(true),
                                                        );
                                                        if !card.answered {
                                                            ui.horizontal(|ui| {
                                                                if ui.button("Open").clicked() {
                                                                    ui.ctx().open_url(
                                                                        egui::OpenUrl::new_tab(url),
                                                                    );
                                                                    interaction_responses.push((
                                                                        card.request.request_id,
                                                                        InteractionResponse::Accepted,
                                                                    ));
                                                                }
                                                                if ui.button("Cancel").clicked() {
                                                                    interaction_responses.push((
                                                                        card.request.request_id,
                                                                        InteractionResponse::Declined,
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
                                    &self.agent_project_root,
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
                                    draw_dense_agent_working(
                                        ui,
                                        scoped_agent_id(
                                            agentic_mode,
                                            agent_pane,
                                            "dense_agent_working",
                                        ),
                                    );
                                } else {
                                    ui.add_space(theme::space::SMALL);
                                    draw_agent_working(
                                        ui,
                                        scoped_agent_id(agentic_mode, agent_pane, "agent_working"),
                                        self.selected_provider,
                                    );
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
                    let scrollbar_active = ui
                        .ctx()
                        .read_response(output.id.with(1))
                        .is_some_and(|response| response.is_pointer_button_down_on());
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
                        && !scrollbar_active
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
        let mut goal_action = None;
        let mut account_change = None;
        let mut provider_change = None;
        let mut account_toggle = None;
        let mut account_move = None;
        let mut account_add = None;
        let mut account_rename = None;
        let mut session_load = None;
        let mut session_remove = None;
        let has_config_mode = self.agent.config_options.iter().any(|option| {
            option.id.eq_ignore_ascii_case("mode") || option.name.eq_ignore_ascii_case("mode")
        });

        let mut prompt_changed = false;
        let mut history_navigated = false;
        let mut mention_attach = None;
        let composer_enabled =
            self.agent.session_ready && (!self.agent.active || self.agent.steering);
        let composer_hint =
            if self.agent.session_ready && self.selected_provider == ProviderId::Cursor {
                "Ask Cursor Agent… · & for Cloud".to_owned()
            } else if self.agent.session_ready {
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
            && !matches!(
                open_menu,
                Some(AgentMenu::Providers | AgentMenu::AddAccount)
            )
        {
            open_menu = None;
        }
        let mut menu_anchor = if matches!(
            open_menu,
            Some(AgentMenu::Providers | AgentMenu::AddAccount)
        ) {
            self.provider_menu_anchor
        } else {
            session_menu_anchor
        };
        let mut menu_toggled = session_menu_toggled || provider_menu_toggled;
        let composer_panel = if floating_agent_composer {
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
            if compact_agent_composer {
                ui.painter()
                    .rect_filled(composer, 0.0, theme::state::sidebar_material());
            }
            ui.painter().hline(
                composer.x_range(),
                composer.top() + 0.5,
                egui::Stroke::new(1.0, theme::border::hairline_color()),
            );
            composer
        };
        let prompt_id = scoped_agent_id(agentic_mode, agent_pane, "agent_prompt");
        let mention_open = matches!(open_menu, Some(AgentMenu::Mentions(_)))
            && !self.agent_mention_matches.is_empty();
        if mention_open && ui.memory(|memory| memory.has_focus(prompt_id)) {
            let (up, down, tab) = ui.input(|input| {
                (
                    input.modifiers == egui::Modifiers::NONE && input.key_pressed(Key::ArrowUp),
                    input.modifiers == egui::Modifiers::NONE && input.key_pressed(Key::ArrowDown),
                    input.modifiers == egui::Modifiers::NONE && input.key_pressed(Key::Tab),
                )
            });
            if up {
                self.agent_mention_selected = self.agent_mention_selected.saturating_sub(1);
                ui.input_mut(|input| {
                    input.consume_key(egui::Modifiers::NONE, Key::ArrowUp);
                });
            } else if down {
                self.agent_mention_selected =
                    (self.agent_mention_selected + 1).min(self.agent_mention_matches.len() - 1);
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
                    if input.modifiers == egui::Modifiers::NONE && input.key_pressed(Key::ArrowUp) {
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
            ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
            ui.input_mut(|input| {
                input.consume_key(egui::Modifiers::NONE, key);
            });
        }
        let ready = self.agent.session_ready
            && (!self.agent.prompt.trim().is_empty() || !self.agent_attachments.is_empty());
        let output = (AssistantComposer {
            panel: composer_panel,
            prompt_id,
            attach_id: scoped_agent_id(agentic_mode, agent_pane, "agent_attach"),
            scroll_id: scoped_agent_id(agentic_mode, agent_pane, "agent_prompt_scroll"),
            hint: &composer_hint,
            attach_tooltip: "Attach files (or type @ for files and folders)",
            drop_hint: "Drop files or folders to attach",
            enabled: composer_enabled,
            send_enabled: ready,
            active: self.agent.active,
            allow_active_send: self.agent.steering,
            allow_directories: true,
            handle_drop: true,
            mouse_wheel: !menu_owns_wheel,
            focus: false,
            radius: if floating_agent_composer {
                f32::from(AGENTIC_COMPOSER_RADIUS)
            } else {
                0.0
            },
        })
        .show(
            ui,
            &mut self.agent.prompt,
            &mut self.agent_attachments,
            &mut self.agent_drop_hovered,
        );
        let send = output.send;
        let cancel = output.cancel;
        let open_file_picker = output.open_file_picker;
        let submit_shortcut = output.submit;
        let input_rect = output.input_rect;
        let controls_footer = output.controls_rect;
        if let Some(error) = output.error {
            self.show_error(error);
        }
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
        if output.input_changed {
            self.agent_prompt_history_index = None;
            self.agent_prompt_history_draft.clear();
        }
        prompt_changed |= history_navigated || output.input_changed;
        if history_navigated {
            let prompt_id = scoped_agent_id(agentic_mode, agent_pane, "agent_prompt");
            ui.memory_mut(|memory| memory.request_focus(prompt_id));
        }
        let mention_query = composer_enabled
            .then(|| agent_mention_query(&self.agent.prompt))
            .flatten();
        if let Some(query) = mention_query
            && (prompt_changed || matches!(open_menu, Some(AgentMenu::Mentions(_))))
        {
            let mentions_changed = self.agent_mentions.is_none();
            if mentions_changed {
                // ponytail: one path-only scan on the first @; move it to the existing search
                // worker only if very large workspaces make this measurable.
                self.agent_mentions = Some(collect_agent_mentions(&self.agent_project_root));
            }
            let query_changed = !matches!(
                &open_menu,
                Some(AgentMenu::Mentions(current)) if current == query
            );
            if query_changed || mentions_changed {
                self.agent_mention_matches = agent_mention_matches(
                    self.agent_mentions.as_deref().unwrap_or_default(),
                    query,
                );
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
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("agent_composer_footer")
                .max_rect(controls_footer)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = theme::space::SMALL;
                if let Some(goal) = &self.agent.goal {
                    ui.label(
                        RichText::new(format!("Goal: {}", goal.status))
                            .size(theme::typography::MICRO_SIZE)
                            .weak(),
                    );
                    if goal.status == "active"
                        && self
                            .agent
                            .goal_actions
                            .iter()
                            .any(|action| action == "pause")
                        && ui.small_button("Pause").clicked()
                    {
                        goal_action = Some(GoalAction::Pause);
                    } else if matches!(goal.status.as_str(), "paused" | "blocked" | "limited")
                        && self
                            .agent
                            .goal_actions
                            .iter()
                            .any(|action| action == "resume")
                        && ui.small_button("Resume").clicked()
                    {
                        goal_action = Some(GoalAction::Resume);
                    }
                    if self
                        .agent
                        .goal_actions
                        .iter()
                        .any(|action| action == "clear")
                        && ui.small_button("Clear").clicked()
                    {
                        goal_action = Some(GoalAction::Clear);
                    }
                } else if self.agent.goal_actions.iter().any(|action| action == "set")
                    && ui.small_button("Goal").clicked()
                {
                    self.agent.prompt = "/goal ".into();
                    let prompt_id = scoped_agent_id(agentic_mode, agent_pane, "agent_prompt");
                    ui.memory_mut(|memory| memory.request_focus(prompt_id));
                }
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
                        let selector = agent_selector_button(ui, current_name);
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
                            || (model_config.is_some()
                                && is_context_config(&option.id, &option.name))
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
                                let selected = ui.selectable_label(*current, &option.name);
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
                        );
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
                            Some(scoped_agent_id(
                                agentic_mode,
                                agent_pane,
                                "agent_model_selector",
                            )),
                            &label,
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

        let mut menu_popup = None;
        if let (Some(menu), Some(anchor)) = (open_menu.as_ref(), menu_anchor) {
            let item_count = match menu {
                AgentMenu::Providers => Some(self.provider_menu_row_count() + 1),
                AgentMenu::AddAccount => Some(provider_catalog().len() + 1),
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
                        let context_rows = self
                            .agent
                            .config_options
                            .iter()
                            .filter(|other| is_context_config(&other.id, &other.name))
                            .map(|other| match &other.value {
                                ConfigValue::Select(_) => other.options.len() + 1,
                                ConfigValue::Boolean(_) => 1,
                            })
                            .sum::<usize>();
                        if is_model_config(&option.id, &option.name)
                            && (thinking_rows > 0 || fast_rows > 0 || context_rows > 0)
                        {
                            return option.options.len()
                                + thinking_rows
                                + fast_rows
                                + context_rows
                                + 1;
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
                    AgentMenu::Providers | AgentMenu::AddAccount => AGENT_PROVIDER_ROW_HEIGHT,
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
                let content_height = match menu {
                    AgentMenu::Providers => {
                        self.provider_menu_row_count() as f32 * AGENT_PROVIDER_ROW_HEIGHT
                            + AGENT_PROVIDER_FOOTER_HEIGHT
                    }
                    AgentMenu::AddAccount => {
                        AGENT_PROVIDER_HEADING_HEIGHT
                            + provider_catalog().len() as f32 * AGENT_PROVIDER_ROW_HEIGHT
                    }
                    _ => item_count as f32 * row_height,
                };
                let popup = match menu {
                    AgentMenu::Providers | AgentMenu::AddAccount => {
                        agent_provider_menu_rect(ui.ctx().content_rect(), anchor, content_height)
                    }
                    AgentMenu::Sessions => agent_session_menu_rect(
                        transcript,
                        anchor,
                        item_count,
                        row_height,
                        AGENT_MENU_WIDTH,
                    ),
                    _ => agent_menu_rect(
                        transcript,
                        anchor,
                        item_count,
                        row_height,
                        menu_padding_y,
                        if matches!(menu, AgentMenu::Config(id) if self
                            .agent
                            .config_options
                            .iter()
                            .any(|option| option.id == *id
                                && is_model_config(&option.id, &option.name)))
                        {
                            480.0
                        } else {
                            280.0
                        },
                    ),
                };
                menu_popup = Some(popup);
                let max_scroll =
                    (content_height - (popup.height() - 2.0 * menu_padding_y)).max(0.0);
                let wheel_delta = ui.input(|input| {
                    input
                        .pointer
                        .hover_pos()
                        .filter(|pointer| popup.contains(*pointer))
                        .map_or(0.0, |_| input.smooth_scroll_delta.y)
                });
                if wheel_delta != 0.0 && max_scroll > 1.0 {
                    self.agent_menu_scroll_y =
                        (self.agent_menu_scroll_y - wheel_delta).clamp(0.0, max_scroll);
                    ui.input_mut(|input| input.smooth_scroll_delta.y = 0.0);
                    ui.ctx().request_repaint();
                }
                self.agent_menu_scroll_y = self.agent_menu_scroll_y.min(max_scroll);
                let mut selected = false;
                let mut drill_to_add = false;
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
                        ui.set_clip_rect(popup);
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
                                    .scroll_bar_visibility(if max_scroll <= 1.0 {
                                        egui::scroll_area::ScrollBarVisibility::AlwaysHidden
                                    } else {
                                        egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
                                    })
                                    .scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR)
                                    .vertical_scroll_offset(scroll_y)
                                    .show(ui, |ui| {
                                        ui.spacing_mut().interact_size.y = row_height;
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                                match menu {
                                            AgentMenu::Providers => {
                                                for provider in provider_catalog().iter() {
                                                    let packaged = self
                                                        .available_providers
                                                        .contains(&provider.id);
                                                    let reason =
                                                        provider.unavailable_reason.or_else(|| {
                                                            (!packaged).then_some(
                                                                "Unavailable in this build",
                                                            )
                                                        });
                                                    let accounts = self
                                                        .accounts
                                                        .accounts_for(provider.id)
                                                        .cloned()
                                                        .collect::<Vec<_>>();
                                                    if accounts.is_empty() {
                                                        // The registry backfills a legacy
                                                        // account per available provider at
                                                        // startup; keep the provider reachable
                                                        // even when that has not happened.
                                                        let response = ui
                                                            .add_enabled_ui(
                                                                packaged
                                                                    && reason.is_none()
                                                                    && self.can_switch_accounts(),
                                                                |ui| {
                                                                    provider_pick_row(
                                                                        ui,
                                                                        provider,
                                                                        &format!(
                                                                            "Switch to {}",
                                                                            provider.display_name
                                                                        ),
                                                                    )
                                                                },
                                                            )
                                                            .inner;
                                                        if response.clicked() {
                                                            provider_change = Some(provider.id);
                                                            selected = true;
                                                        }
                                                        continue;
                                                    }
                                                    for (priority, account) in
                                                        accounts.iter().enumerate()
                                                    {
                                                        let exhaustion = self
                                                            .exhausted_accounts
                                                            .get(&account.key)
                                                            .filter(|reset_at| {
                                                                reset_at.is_none_or(|reset_at| {
                                                                    reset_at > SystemTime::now()
                                                                })
                                                            });
                                                        // Only states worth acting on are shown;
                                                        // a healthy account is simply quiet.
                                                        let status: Option<(
                                                            String,
                                                            AccountStatusTone,
                                                        )> = if let Some(reset_at) = exhaustion {
                                                            let text = match reset_at {
                                                                None => "Exhausted".to_owned(),
                                                                Some(reset_at) => {
                                                                    let seconds = reset_at
                                                                        .duration_since(
                                                                            SystemTime::now(),
                                                                        )
                                                                        .unwrap_or_default()
                                                                        .as_secs();
                                                                    format!(
                                                                        "Exhausted · {}m",
                                                                        seconds.div_ceil(60)
                                                                    )
                                                                }
                                                            };
                                                            Some((
                                                                text,
                                                                AccountStatusTone::Negative,
                                                            ))
                                                        } else if account.key
                                                            == self.selected_account
                                                        {
                                                            match &self.agent.connection {
                                                                ConnectionState::Ready => None,
                                                                ConnectionState::AuthenticationRequired(
                                                                    _,
                                                                ) => Some((
                                                                    "Sign in required".into(),
                                                                    AccountStatusTone::Caution,
                                                                )),
                                                                ConnectionState::Provisioning {
                                                                    ..
                                                                }
                                                                | ConnectionState::Starting => Some((
                                                                    "Starting…".into(),
                                                                    AccountStatusTone::Neutral,
                                                                )),
                                                                ConnectionState::Failed(_) => Some((
                                                                    "Unavailable".into(),
                                                                    AccountStatusTone::Negative,
                                                                )),
                                                                ConnectionState::Disconnected => Some((
                                                                    "Disconnected".into(),
                                                                    AccountStatusTone::Neutral,
                                                                )),
                                                            }
                                                        } else {
                                                            None
                                                        };
                                                        // A provider's only account reads as the
                                                        // provider itself; named accounts show
                                                        // the label the user gave them.
                                                        let title = if accounts.len() == 1 {
                                                            provider.display_name
                                                        } else {
                                                            account.label.as_str()
                                                        };
                                                        let enabled = packaged
                                                            && reason.is_none()
                                                            && self.can_switch_accounts();
                                                        let action = ui
                                                            .add_enabled_ui(enabled, |ui| {
                                                                account_menu_row(
                                                                    ui,
                                                                    provider,
                                                                    account,
                                                                    AccountRowModel {
                                                                        title,
                                                                        selected: self
                                                                            .selected_account
                                                                            == account.key,
                                                                        status: status.as_ref().map(
                                                                            |(text, tone)| {
                                                                                AccountStatus {
                                                                                    text,
                                                                                    tone: *tone,
                                                                                }
                                                                            },
                                                                        ),
                                                                        priority: priority + 1,
                                                                        account_count: accounts
                                                                            .len(),
                                                                    },
                                                                )
                                                            })
                                                            .inner;
                                                        if action.select {
                                                            account_change = Some(account.key);
                                                            selected = true;
                                                        }
                                                        if action.toggle_failover {
                                                            account_toggle = Some(account.key);
                                                        }
                                                        if action.move_up {
                                                            account_move =
                                                                Some((account.key, -1));
                                                        }
                                                        if action.move_down {
                                                            account_move = Some((account.key, 1));
                                                        }
                                                        if action.rename {
                                                            account_rename = Some(account.key);
                                                            selected = true;
                                                        }
                                                    }
                                                }
                                                let add = ui
                                                    .add_enabled_ui(
                                                        self.can_switch_accounts(),
                                                        add_account_menu_row,
                                                    )
                                                    .inner;
                                                if add.clicked() {
                                                    drill_to_add = true;
                                                }
                                            }
                                            AgentMenu::AddAccount => {
                                                provider_menu_heading(ui, "Add an account for");
                                                for provider in provider_catalog().iter() {
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
                                                            packaged
                                                                && reason.is_none()
                                                                && self.can_switch_accounts(),
                                                            |ui| {
                                                                provider_pick_row(
                                                                    ui,
                                                                    provider,
                                                                    &format!(
                                                                        "Add {} account",
                                                                        provider.display_name
                                                                    ),
                                                                )
                                                            },
                                                        )
                                                        .inner;
                                                    if response.clicked() {
                                                        account_add = Some(provider.id);
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
                                                        let (open, remove, _) = agent_session_row(
                                                            ui,
                                                            session,
                                                            false,
                                                            false,
                                                            &[],
                                                            true,
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
                                                            memory.request_focus(scoped_agent_id(
                                                                agentic_mode,
                                                                agent_pane,
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
                                                            memory.request_focus(scoped_agent_id(
                                                                agentic_mode,
                                                                agent_pane,
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
                                                    let combined_context = model_menu
                                                        && self.agent.config_options.iter().any(
                                                            |other| {
                                                                is_context_config(
                                                                    &other.id,
                                                                    &other.name,
                                                                )
                                                            },
                                                        );
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
                                                    if combined_context {
                                                        for context in
                                                            self.agent.config_options.iter().filter(
                                                                |other| {
                                                                    is_context_config(
                                                                        &other.id,
                                                                        &other.name,
                                                                    )
                                                                },
                                                            )
                                                        {
                                                            match &context.value {
                                                                ConfigValue::Select(current) => {
                                                                    agent_menu_section_label(
                                                                        ui,
                                                                        &context.name,
                                                                        row_height,
                                                                    );
                                                                    for value in &context.options {
                                                                        if agent_menu_option(
                                                                            ui,
                                                                            &value.name,
                                                                            value.id == *current,
                                                                            row_height,
                                                                        )
                                                                        .clicked()
                                                                        {
                                                                            config_changes.push((
                                                                                context.id.clone(),
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
                                                                        &context.name,
                                                                        *enabled,
                                                                        row_height,
                                                                    )
                                                                    .clicked()
                                                                    {
                                                                        config_changes.push((
                                                                            context.id.clone(),
                                                                            ConfigValue::Boolean(
                                                                                !enabled,
                                                                            ),
                                                                        ));
                                                                    }
                                                                }
                                                            }
                                                        }
                                                    }
                                                    if combined_thinking
                                                        || combined_fast.is_some()
                                                        || combined_context
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
                if drill_to_add {
                    open_menu = Some(AgentMenu::AddAccount);
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
            let prompt_id = scoped_agent_id(agentic_mode, agent_pane, "agent_prompt");
            ui.memory_mut(|memory| memory.request_focus(prompt_id));
            ui.ctx().request_repaint();
        }
        if open_file_picker {
            self.open_agent_file_picker();
            ui.ctx().request_repaint();
        }

        for (request_id, option_id) in permission_decisions {
            if self.agent.decide_permission(request_id, &option_id)
                && let Some(controller) = self.agent_controllers.get(&self.selected_account)
            {
                let _ = controller.send(AgentCommand::DecidePermission {
                    request_id,
                    option_id,
                });
            }
        }
        for (request_id, response) in interaction_responses {
            if self.agent.answer_interaction(request_id)
                && let Some(controller) = self.agent_controllers.get(&self.selected_account)
            {
                let _ = controller.send(AgentCommand::RespondInteraction {
                    request_id,
                    response,
                });
            }
        }
        if let Some(source) = open_image_request {
            self.assistant_image_lightbox = Some(source);
        }
        if let Some((path, line)) = open_path_request {
            self.open_agent_path(path, line);
        }
        if let Some(path) = open_diff_request {
            self.open_agent_diff(path);
        }
        if let Some(enabled) = run_everything_change
            && let Some(controller) = self.agent_controllers.get(&self.selected_account)
        {
            match controller.send(AgentCommand::SetRunEverything(enabled)) {
                Ok(()) => {
                    self.agent_run_everything = Some(enabled);
                }
                Err(error) => self.show_error(error),
            }
        }
        if let Some(key) = account_toggle {
            let mut accounts = self.accounts.clone();
            if let Some(account) = accounts
                .accounts
                .iter_mut()
                .find(|account| account.key == key)
            {
                account.auto_failover = !account.auto_failover;
                if let Err(error) = self.commit_accounts(accounts) {
                    self.show_error(error);
                }
            }
        }
        if let Some((key, offset)) = account_move {
            let mut accounts = self.accounts.clone();
            if accounts.move_account(key, offset)
                && let Err(error) = self.commit_accounts(accounts)
            {
                self.show_error(error);
            }
        }
        if let Some(key) = account_rename
            && let Some(account) = self.accounts.account(key)
        {
            self.account_prompt = Some(AccountPrompt {
                action: AccountPromptAction::Rename(key),
                label: account.label.clone(),
                focus: true,
            });
        }
        if let Some(provider) = account_add {
            let number = self.accounts.accounts_for(provider).count() + 1;
            self.account_prompt = Some(AccountPrompt {
                action: AccountPromptAction::Add(provider),
                label: format!("Account {number}"),
                focus: true,
            });
        }
        if let Some(account) = account_change {
            self.request_account_switch(account, ui.ctx());
        } else if let Some(provider) = provider_change {
            self.request_provider_switch(provider, ui.ctx());
        }
        if let Some(controller) = self.agent_controllers.get(&self.selected_account) {
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
            if let Some(action) = goal_action {
                let _ = controller.send(AgentCommand::ControlGoal(action));
            }
            if cancel {
                let _ = controller.send(AgentCommand::Cancel);
            }
        }
        if send || submit_shortcut {
            self.queue_agent_prompt();
        }
        if new_session && let Some(controller) = self.agent_controllers.get(&self.selected_account)
        {
            let _ = controller.send(AgentCommand::NewSession);
        }
        if new_pane {
            self.open_agent_session_pane();
        }
        if let Some(method) = authenticate
            && let Some(controller) = self.agent_controllers.get(&self.selected_account)
        {
            let _ = controller.send(AgentCommand::Authenticate(method));
        }
        if reconnect {
            self.reconnect_agent(ui.ctx());
        }
    }
}
