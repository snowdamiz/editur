use super::*;

impl EditorApp {
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        if self.shortcut_recorder.is_some()
            || self.new_profile.is_some()
            || self.vim_overlay.is_some()
        {
            return;
        }
        if self.settings_open && ctx.input(|input| input.key_pressed(Key::Escape)) {
            ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
            self.settings_open = false;
            return;
        }
        if self.pending.is_some()
            || self.conflict
            || self.save_as.is_some()
            || self.error.is_some()
            || self.attachment_file_picker.is_some()
            || self.project_folder_picker.is_some()
            || self.devin_confirm_terminate
            || self.devin_confirm_disconnect
            || self.tree_prompt.is_some()
            || self.tree_delete.is_some()
        {
            return;
        }
        if !self.terminal.focused(ctx) && self.handle_lsp_popup_keys(ctx) {
            return;
        }
        let save_quit = ctx.input(|input| {
            input.modifiers.command
                && ((input.key_pressed(Key::Q) && input.key_down(Key::S))
                    || (input.key_pressed(Key::S) && input.key_down(Key::Q)))
        });
        if save_quit {
            if self.save(None) {
                self.request_close();
            }
            return;
        }
        let events = ctx.input(|input| input.events.clone());
        let special_copy = events
            .iter()
            .any(|event| matches!(event, egui::Event::Copy));
        let special_cut = events.iter().any(|event| matches!(event, egui::Event::Cut));
        let special_paste = events
            .iter()
            .any(|event| matches!(event, egui::Event::Paste(_)));
        let now = ctx.input(|input| Duration::from_secs_f64(input.time.max(0.0)));
        let mut consumed = HashSet::new();
        let mut duplicate_events = Vec::new();
        let mut special_handled = [false; 3];
        for (index, event) in events.iter().enumerate() {
            let scopes = self.active_keybinding_scopes(ctx);
            let (stroke, repeated, paste) = match event {
                egui::Event::Copy => continue,
                egui::Event::Cut => (
                    InputStroke::new(Key::X, Some(Key::X), primary_modifiers()),
                    false,
                    None,
                ),
                egui::Event::Paste(value) => (
                    InputStroke::new(Key::V, Some(Key::V), primary_modifiers()),
                    false,
                    Some(value.clone()),
                ),
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    repeat,
                    modifiers,
                } => {
                    if (special_copy && *key == Key::C && modifiers.command)
                        || (special_cut && *key == Key::X && modifiers.command)
                        || (special_paste && *key == Key::V && modifiers.command)
                    {
                        let kind = if *key == Key::C {
                            0
                        } else if *key == Key::X {
                            1
                        } else {
                            2
                        };
                        duplicate_events.push((index, kind));
                        continue;
                    }
                    if !modifiers.ctrl
                        && !modifiers.alt
                        && !modifiers.command
                        && !modifiers.mac_cmd
                        && key_character(*key, *modifiers).is_some()
                        && text_scope_owns_printable(
                            &scopes,
                            self.settings.keybindings.active_behavior(),
                        )
                    {
                        continue;
                    }
                    if self.settings.keybindings.active_behavior() == KeybindingBehavior::Vim
                        && scopes.iter().any(|scope| scope.is_vim())
                        && self
                            .active_tab
                            .and_then(|tab| self.tabs.get(tab))
                            .is_some_and(|tab| tab.vim.awaits_character())
                        && let Some(character) = key_character(*key, *modifiers)
                    {
                        consumed.insert(index);
                        self.execute_vim_character(character, ctx);
                        continue;
                    }
                    (
                        InputStroke::new(*key, *physical_key, *modifiers),
                        *repeat,
                        None,
                    )
                }
                _ => continue,
            };
            let result = self
                .keybinding_resolver
                .resolve(stroke, &scopes, repeated, now);
            if matches!(
                event,
                egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
            ) && !ctx.text_edit_focused()
                && !scopes.contains(&Scope::Terminal)
                && !scopes.contains(&Scope::Devin)
            {
                consumed.insert(index);
            }
            if result.consumed {
                match event {
                    egui::Event::Copy => special_handled[0] = true,
                    egui::Event::Cut => special_handled[1] = true,
                    egui::Event::Paste(_) => special_handled[2] = true,
                    _ => {}
                }
                consumed.insert(index);
                if result.command.is_none() {
                    ctx.request_repaint_after(Duration::from_secs(1));
                }
            }
            if result.consumed
                && result.command.is_none()
                && stroke.key == Key::Escape
                && self.settings.keybindings.active_behavior() == KeybindingBehavior::Vim
                && scopes.iter().any(|scope| scope.is_vim())
            {
                self.execute_keybinding(KeybindingCommand::VimNormal, None, ctx);
            }
            if let Some(command) = result.command {
                self.execute_keybinding(command, paste.as_deref(), ctx);
            }
        }
        consumed.extend(
            duplicate_events
                .into_iter()
                .filter_map(|(index, kind)| special_handled[kind].then_some(index)),
        );
        if !consumed.is_empty() {
            let consumed_events = consumed
                .iter()
                .filter_map(|index| events.get(*index))
                .collect::<Vec<_>>();
            ctx.input_mut(|input| {
                input
                    .events
                    .retain(|event| !consumed_events.contains(&event));
            });
        }
    }

    pub(super) fn active_keybinding_scopes(&self, ctx: &egui::Context) -> Vec<Scope> {
        if ctx.memory(|memory| {
            [
                "devin_api_key",
                "devin_org_id",
                "devin_filter",
                "devin_repository",
                "devin_create_prompt",
                "devin_prompt",
            ]
            .into_iter()
            .any(|id| memory.has_focus(Id::new(id)))
        }) {
            return vec![Scope::Devin];
        }
        if self.settings_open {
            return vec![Scope::Settings];
        }
        if self.terminal.focused(ctx) {
            return vec![Scope::Terminal];
        }
        if ctx.memory(|memory| memory.has_focus(Id::new("agent_prompt"))) {
            return vec![Scope::Agent];
        }
        if self.agent_find.open
            && ctx.memory(|memory| memory.has_focus(Id::new("agent_find_query")))
        {
            return vec![Scope::Agent];
        }
        if self.search_open {
            return vec![Scope::ProjectSearch];
        }
        if self
            .pane_find
            .get(&self.active_pane)
            .is_some_and(|find| find.open)
            && ctx.memory(|memory| {
                memory.has_focus(Id::new(("file_search_query", self.active_pane.0)))
            })
        {
            return vec![Scope::Find];
        }
        if ctx.text_edit_focused() {
            return Vec::new();
        }
        if self.scm_focused {
            return vec![Scope::SourceControl];
        }
        if self.tree_focused {
            return vec![Scope::FilesTree];
        }
        if self.settings.keybindings.active_behavior() == KeybindingBehavior::Vim
            && let Some(tab) = self.active_tab.and_then(|index| self.tabs.get(index))
        {
            return vec![tab.vim.scope(), Scope::DocumentEditor];
        }
        vec![Scope::DocumentEditor]
    }

    pub(super) fn execute_keybinding(
        &mut self,
        command: KeybindingCommand,
        paste: Option<&str>,
        ctx: &egui::Context,
    ) {
        if self.git_diff.is_some()
            && (command.id().starts_with("editor.")
                || command.id().starts_with("vim.")
                || matches!(
                    command,
                    KeybindingCommand::ViewToggleMarkdownPreview | KeybindingCommand::SearchFind
                ))
        {
            return;
        }
        match command {
            KeybindingCommand::AppOpenSettings => {
                if self.settings_open {
                    self.settings_open = false;
                } else {
                    self.open_settings();
                }
            }
            KeybindingCommand::AppOpenKeybindings => {
                self.settings_section = SettingsSection::Keybindings;
                self.open_settings();
            }
            KeybindingCommand::AppIncreaseUiScale => self.change_ui_scale(true, ctx),
            KeybindingCommand::AppDecreaseUiScale => self.change_ui_scale(false, ctx),
            KeybindingCommand::AppCloseWindow => self.request_close(),
            KeybindingCommand::AppToggleAgentSidebar => {
                self.agent_sidebar = !self.agent_sidebar;
                self.agent_sidebar_dragging = false;
                if self.agent_sidebar {
                    self.devin_sidebar = false;
                    self.devin_sidebar_dragging = false;
                    if let Some(controller) = self.devin_controller.as_ref() {
                        let _ = controller.send(DevinCommand::SetVisible(false));
                    }
                    self.open_agent(ctx);
                }
            }
            KeybindingCommand::AppToggleDevinSidebar => {
                self.devin_sidebar = !self.devin_sidebar;
                self.devin_sidebar_dragging = false;
                if self.devin_sidebar {
                    self.agent_sidebar = false;
                    self.agent_sidebar_dragging = false;
                    self.agentic_mode = false;
                    self.open_devin(ctx);
                } else if let Some(controller) = self.devin_controller.as_ref() {
                    let _ = controller.send(DevinCommand::SetVisible(false));
                }
            }
            KeybindingCommand::AppToggleAgenticView => {
                self.set_agentic_mode(!self.agentic_mode, ctx)
            }
            KeybindingCommand::FileSave => {
                self.save(None);
            }
            KeybindingCommand::FileSaveAndClose => {
                if self.save(None)
                    && let Some(index) = self.active_tab
                {
                    self.request(PendingAction::CloseTab(index));
                }
            }
            KeybindingCommand::FileCloseActive => {
                if self.terminal.focused(ctx) {
                    self.terminal.close_active();
                    if self.terminal.is_empty() {
                        self.terminal_open = false;
                    }
                } else if let Some(index) = self.active_tab {
                    self.request(PendingAction::CloseTab(index));
                } else {
                    self.request_close();
                }
            }
            KeybindingCommand::FileFocusPane1
            | KeybindingCommand::FileFocusPane2
            | KeybindingCommand::FileFocusPane3
            | KeybindingCommand::FileFocusPane4
            | KeybindingCommand::FileFocusPane5
            | KeybindingCommand::FileFocusPane6
            | KeybindingCommand::FileFocusPane7
            | KeybindingCommand::FileFocusPane8
            | KeybindingCommand::FileFocusPane9 => {
                let index = match command {
                    KeybindingCommand::FileFocusPane1 => 0,
                    KeybindingCommand::FileFocusPane2 => 1,
                    KeybindingCommand::FileFocusPane3 => 2,
                    KeybindingCommand::FileFocusPane4 => 3,
                    KeybindingCommand::FileFocusPane5 => 4,
                    KeybindingCommand::FileFocusPane6 => 5,
                    KeybindingCommand::FileFocusPane7 => 6,
                    KeybindingCommand::FileFocusPane8 => 7,
                    _ => 8,
                };
                self.focus_pane(index);
            }
            KeybindingCommand::FileSplitEditor => {
                self.pane_layout.split(self.active_pane, DropZone::Right);
            }
            KeybindingCommand::FileFocusNextPane | KeybindingCommand::FileFocusRightPane => {
                self.focus_relative_pane(1);
            }
            KeybindingCommand::FileFocusPreviousPane | KeybindingCommand::FileFocusLeftPane => {
                self.focus_relative_pane(-1);
            }
            KeybindingCommand::ViewToggleSidebar => {
                self.sidebar = !self.sidebar;
                if self.sidebar && self.sidebar_pane == SidebarPane::SourceControl {
                    self.scm_focused = true;
                    self.focus_editor = false;
                    self.open_source_control(ctx);
                } else if !self.sidebar {
                    self.tree_focused = false;
                    self.scm_focused = false;
                    self.focus_editor = self.active_tab.is_some();
                }
            }
            KeybindingCommand::ViewToggleExplorer => {
                if self.sidebar && self.sidebar_pane == SidebarPane::Files {
                    self.sidebar = false;
                    self.tree_focused = false;
                } else {
                    if self.agentic_mode {
                        self.set_agentic_mode(false, ctx);
                    }
                    self.sidebar = true;
                    self.sidebar_pane = SidebarPane::Files;
                    self.scm_focused = false;
                    self.tree_focused = true;
                }
            }
            KeybindingCommand::ViewToggleSourceControl => {
                if self.sidebar && self.sidebar_pane == SidebarPane::SourceControl {
                    self.sidebar_pane = SidebarPane::Files;
                    self.scm_focused = false;
                    self.tree_focused = true;
                } else {
                    if self.agentic_mode {
                        self.set_agentic_mode(false, ctx);
                    }
                    self.sidebar = true;
                    self.sidebar_pane = SidebarPane::SourceControl;
                    self.tree_focused = false;
                    self.scm_focused = true;
                    self.focus_editor = false;
                    self.open_source_control(ctx);
                }
            }
            KeybindingCommand::ViewFocusExplorer => {
                self.sidebar = true;
                self.sidebar_pane = SidebarPane::Files;
                self.focus_editor = false;
                self.scm_focused = false;
                self.tree_focused = true;
                ctx.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
            }
            KeybindingCommand::ViewToggleTerminal => self.toggle_terminal(ctx),
            KeybindingCommand::ViewToggleMarkdownPreview => {
                if let Some(index) = self.active_tab {
                    self.tabs[index].markdown_preview = !self.tabs[index].markdown_preview;
                }
            }
            KeybindingCommand::SearchFind => self.open_find(ctx),
            KeybindingCommand::SearchNext => self.step_find(false),
            KeybindingCommand::SearchPrevious => self.step_find(true),
            KeybindingCommand::SearchProject => self.open_project_search(ctx),
            KeybindingCommand::SearchClose => {
                self.search_open = false;
                self.agent_find.open = false;
                if let Some(find) = self.pane_find.get_mut(&self.active_pane) {
                    find.open = false;
                }
                self.focus_editor = self.active_tab.is_some();
            }
            KeybindingCommand::TreeMoveUp
            | KeybindingCommand::TreeMoveDown
            | KeybindingCommand::TreeExpand
            | KeybindingCommand::TreeCollapse
            | KeybindingCommand::TreeOpen => self.execute_tree_command(command),
            KeybindingCommand::EditorTriggerSuggest => {
                if let Some(tag) = self.active_request_tag() {
                    self.lsp_completion = None;
                    self.lsp_pending_completion = Some((tag, None));
                    self.lsp_sync_needed = true;
                }
            }
            KeybindingCommand::EditorGoToDefinition => {
                if let Some(tag) = self.active_request_tag() {
                    self.lsp_pending_definition = Some(tag);
                    self.lsp_definitions = None;
                    self.lsp_sync_needed = true;
                }
            }
            KeybindingCommand::EditorNextDiagnostic => self.navigate_diagnostic(true),
            KeybindingCommand::EditorPreviousDiagnostic => self.navigate_diagnostic(false),
            command if command.id().starts_with("editor.") => {
                if command == KeybindingCommand::EditorPaste && paste.is_none() {
                    self.clipboard_request = Some(ClipboardRequest::EditorPaste);
                    return;
                }
                let Some(index) = self.active_tab else {
                    return;
                };
                let changed = {
                    let tab = &mut self.tabs[index];
                    tab.editor_surface
                        .execute_command(ctx, &mut tab.buffer.text, command, paste)
                };
                if changed {
                    self.mark_tab_changed(index);
                }
            }
            command if command.id().starts_with("vim.") => self.execute_vim_command(command, ctx),
            _ => {}
        }
    }

    pub(super) fn rebuild_keybinding_resolver(&mut self) -> Result<(), String> {
        self.keybinding_resolver = Resolver::new(
            self.settings.keybindings.effective_bindings()?,
            KeybindingPlatform::current(),
        )?;
        Ok(())
    }

    pub(super) fn mark_tab_changed(&mut self, index: usize) {
        let tab = &mut self.tabs[index];
        let edits = tab.editor_surface.take_applied_edits();
        tab.buffer.mark_changed_with_edits(&edits);
        self.tabs[index].highlight_cache.valid = false;
        self.lsp_sync_needed = true;
        self.lsp_completion = None;
        self.cursor = self.tabs[index]
            .buffer
            .line_column(self.tabs[index].editor_surface.cursor());
    }

    pub(super) fn focus_pane(&mut self, pane_index: usize) {
        let Some(pane) = self.pane_layout.panes().get(pane_index).copied() else {
            return;
        };
        if let Some(index) = self
            .pane_active_tabs
            .get(&pane)
            .and_then(|path| self.tabs.iter().position(|tab| &tab.buffer.path == path))
        {
            self.activate_tab(index);
        } else {
            self.active_pane = pane;
        }
    }

    pub(super) fn focus_relative_pane(&mut self, direction: isize) {
        let panes = self.pane_layout.panes();
        let Some(current) = panes.iter().position(|pane| *pane == self.active_pane) else {
            return;
        };
        let next = (current as isize + direction).rem_euclid(panes.len() as isize) as usize;
        self.focus_pane(next);
    }

    pub(super) fn open_find(&mut self, ctx: &egui::Context) {
        // Transcript search exists only in the agentic view; in editor mode
        // the agent sidebar leaves Cmd/Ctrl+F to the document.
        if self.agentic_mode {
            self.open_agent_find(ctx);
            return;
        }
        self.lsp_completion = None;
        if let Some(tab) = self.active_tab.and_then(|index| self.tabs.get_mut(index)) {
            tab.markdown_preview = false;
            tab.agent_diff = None;
        }
        self.search_open = false;
        let find = self.pane_find.entry(self.active_pane).or_default();
        find.open = true;
        find.focus = true;
        find.scroll_to_match = !find.matches.is_empty();
        self.focus_editor = false;
        self.tree_focused = false;
        ctx.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
    }

    pub(super) fn open_project_search(&mut self, ctx: &egui::Context) {
        self.lsp_completion = None;
        self.lsp_definitions = None;
        self.search_open = true;
        if let Some(find) = self.pane_find.get_mut(&self.active_pane) {
            find.open = false;
        }
        self.focus_search = true;
        self.focus_editor = false;
        self.tree_focused = false;
        ctx.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
    }

    pub(super) fn step_find(&mut self, previous: bool) {
        if self.agent_find.open {
            if self.agent_find.matches.is_empty() {
                return;
            }
            self.agent_find.selected = next_find_match(
                self.agent_find.selected,
                self.agent_find.matches.len(),
                previous,
            );
            self.agent_find.scroll_to_match = true;
            return;
        }
        self.refresh_find_matches(self.active_pane);
        let Some(find) = self.pane_find.get_mut(&self.active_pane) else {
            return;
        };
        if find.matches.is_empty() {
            return;
        }
        find.selected = next_find_match(find.selected, find.matches.len(), previous);
        find.scroll_to_match = true;
    }

    pub(super) fn open_agent_find(&mut self, ctx: &egui::Context) {
        self.agent_find.open = true;
        self.agent_find.focus = true;
        self.agent_find.dirty = true;
        self.agent_find.scroll_to_match = !self.agent_find.matches.is_empty();
        ctx.memory_mut(|memory| memory.surrender_focus(Id::new("agent_prompt")));
    }

    pub(super) fn execute_tree_command(&mut self, command: KeybindingCommand) {
        let count = self.tree.visible.len();
        if count == 0 {
            return;
        }
        let current = self.tree.selected_index.unwrap_or(0).min(count - 1);
        if matches!(
            command,
            KeybindingCommand::TreeMoveUp | KeybindingCommand::TreeMoveDown
        ) {
            let next = if command == KeybindingCommand::TreeMoveDown {
                (current + 1).min(count - 1)
            } else {
                current.saturating_sub(1)
            };
            self.tree
                .select(Some(self.tree.visible[next].entry.path.clone()));
            return;
        }
        let entry = self.tree.visible[current].entry.clone();
        match command {
            KeybindingCommand::TreeExpand if entry.is_dir => {
                if !self.tree.expanded.contains(&entry.path)
                    && let Err(error) = self.tree.toggle(&entry.path)
                {
                    self.show_error(error);
                }
            }
            KeybindingCommand::TreeCollapse if entry.is_dir => self.tree.collapse(&entry.path),
            KeybindingCommand::TreeOpen if entry.is_dir => {
                if let Err(error) = self.tree.toggle(&entry.path) {
                    self.show_error(error);
                }
            }
            KeybindingCommand::TreeOpen => self.request(PendingAction::Open(entry.path)),
            _ => {}
        }
    }

    pub(super) fn execute_vim_command(&mut self, command: KeybindingCommand, ctx: &egui::Context) {
        let Some(index) = self.active_tab else {
            return;
        };
        let outcome = {
            let tab = &mut self.tabs[index];
            let (text, line_starts, line_byte_starts, character_len) = tab.buffer.vim_parts();
            let index = VimTextIndex::new(line_starts, line_byte_starts, character_len);
            tab.vim.execute_indexed(
                command,
                &mut tab.editor_surface,
                text,
                &mut self.vim_session,
                index,
            )
        };
        self.apply_vim_outcome(index, outcome, ctx);
    }

    pub(super) fn execute_vim_character(&mut self, character: char, ctx: &egui::Context) {
        let Some(index) = self.active_tab else {
            return;
        };
        let outcome = {
            let tab = &mut self.tabs[index];
            let (text, line_starts, line_byte_starts, character_len) = tab.buffer.vim_parts();
            let index = VimTextIndex::new(line_starts, line_byte_starts, character_len);
            tab.vim.provide_character_indexed(
                character,
                &mut tab.editor_surface,
                text,
                &mut self.vim_session,
                index,
            )
        };
        self.apply_vim_outcome(index, outcome, ctx);
    }

    pub(super) fn apply_vim_outcome(
        &mut self,
        index: usize,
        outcome: crate::vim::VimOutcome,
        ctx: &egui::Context,
    ) {
        if outcome.changed {
            self.mark_tab_changed(index);
        }
        if let Some(text) = outcome.copy_to_system {
            ctx.output_mut(|output| {
                output.commands.push(egui::OutputCommand::CopyText(text));
            });
        }
        match outcome.request {
            Some(VimRequest::Search {
                direction,
                seed: Some(seed),
            }) if !seed.is_empty() => {
                self.apply_vim_search(direction, seed);
            }
            Some(VimRequest::Search { direction, seed }) => {
                self.vim_overlay = Some(VimOverlay {
                    kind: VimOverlayKind::Search(direction),
                    input: seed.unwrap_or_default(),
                    error: None,
                    focus: true,
                });
            }
            Some(VimRequest::Ex) => {
                self.vim_overlay = Some(VimOverlay {
                    kind: VimOverlayKind::Ex,
                    input: String::new(),
                    error: None,
                    focus: true,
                });
            }
            Some(VimRequest::SystemPaste { before }) => {
                self.clipboard_request = Some(ClipboardRequest::VimPaste { before });
            }
            None => {}
        }
    }

    pub(super) fn apply_vim_search(&mut self, direction: VimSearchDirection, query: String) {
        if query.is_empty() {
            return;
        }
        self.vim_session.last_search.clone_from(&query);
        self.vim_session.search_direction = direction;
        let find = self.pane_find.entry(self.active_pane).or_default();
        find.open = true;
        find.query = query;
        find.focus = false;
        find.match_revision = u64::MAX;
        self.refresh_find_matches(self.active_pane);
        let Some(index) = self.active_tab else {
            return;
        };
        let cursor = self.tabs[index]
            .buffer
            .byte_index(self.tabs[index].editor_surface.cursor());
        let Some(find) = self.pane_find.get_mut(&self.active_pane) else {
            return;
        };
        if find.matches.is_empty() {
            return;
        }
        find.selected = match direction {
            VimSearchDirection::Forward => find
                .matches
                .iter()
                .position(|range| range.start > cursor)
                .unwrap_or(0),
            VimSearchDirection::Backward => find
                .matches
                .iter()
                .rposition(|range| range.start < cursor)
                .unwrap_or(find.matches.len() - 1),
        };
        find.scroll_to_match = true;
        let byte = find.matches[find.selected].start;
        let character = self.tabs[index].buffer.text[..byte].chars().count();
        self.tabs[index]
            .editor_surface
            .set_selection(character, character);
        self.cursor = self.tabs[index].buffer.line_column(character);
    }

    pub(super) fn execute_ex(&mut self, command: ExCommand) -> Result<(), String> {
        match command {
            ExCommand::Write => {
                if self.save(None) {
                    Ok(())
                } else {
                    Err("write failed".into())
                }
            }
            ExCommand::Quit { force } => {
                let index = self
                    .active_tab
                    .ok_or_else(|| "no active editor".to_owned())?;
                if force {
                    self.close_tab(index);
                    Ok(())
                } else if self.tabs[index].buffer.dirty {
                    Err("changes are unsaved; use :q! to discard them".into())
                } else {
                    self.close_tab(index);
                    Ok(())
                }
            }
            ExCommand::WriteQuit => {
                if self.save(None) {
                    if let Some(index) = self.active_tab {
                        self.close_tab(index);
                    }
                    Ok(())
                } else {
                    Err("write failed".into())
                }
            }
            ExCommand::Exit => {
                let dirty = self
                    .active_tab
                    .is_some_and(|index| self.tabs[index].buffer.dirty);
                if !dirty || self.save(None) {
                    if let Some(index) = self.active_tab {
                        self.close_tab(index);
                    }
                    Ok(())
                } else {
                    Err("write failed".into())
                }
            }
            ExCommand::Edit(path) => {
                let path = PathBuf::from(path);
                let path = if path.is_absolute() {
                    path
                } else {
                    self.tree.root.join(path)
                };
                self.request(PendingAction::Open(path));
                Ok(())
            }
            ExCommand::NoHighlight => {
                if let Some(find) = self.pane_find.get_mut(&self.active_pane) {
                    find.open = false;
                }
                Ok(())
            }
            ExCommand::Line(line) => {
                let index = self
                    .active_tab
                    .ok_or_else(|| "no active editor".to_owned())?;
                let text = &self.tabs[index].buffer.text;
                let mut current_line = 1;
                let mut target = 0;
                for (character, value) in text.chars().enumerate() {
                    if current_line == line {
                        break;
                    }
                    if value == '\n' {
                        current_line += 1;
                        target = character + 1;
                    }
                }
                self.tabs[index]
                    .editor_surface
                    .set_selection(target, target);
                self.cursor = self.tabs[index].buffer.line_column(target);
                Ok(())
            }
        }
    }

    pub(super) fn take_clipboard_request(&mut self) -> Option<ClipboardRequest> {
        self.clipboard_request.take()
    }

    pub(super) fn receive_clipboard(
        &mut self,
        request: ClipboardRequest,
        text: &str,
        ctx: &egui::Context,
    ) {
        let Some(index) = self.active_tab else {
            return;
        };
        let changed = match request {
            ClipboardRequest::EditorPaste => {
                let tab = &mut self.tabs[index];
                let changed = tab.editor_surface.execute_command(
                    ctx,
                    &mut tab.buffer.text,
                    KeybindingCommand::EditorPaste,
                    Some(text),
                );
                if changed && self.settings.keybindings.active_behavior() == KeybindingBehavior::Vim
                {
                    tab.vim.record_insert_text(text);
                }
                changed
            }
            ClipboardRequest::VimPaste { before } => {
                let tab = &mut self.tabs[index];
                tab.vim.paste_system_text(
                    before,
                    text,
                    &mut tab.editor_surface,
                    &mut tab.buffer.text,
                    &mut self.vim_session,
                )
            }
        };
        if changed {
            self.mark_tab_changed(index);
        }
    }

    pub(super) fn handle_lsp_popup_keys(&mut self, ctx: &egui::Context) -> bool {
        if self.lsp_definitions.is_some() {
            let down =
                ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
            let up = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
            let enter = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
            let escape =
                ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
            let chooser = self.lsp_definitions.as_mut().unwrap();
            if down {
                chooser.selected = (chooser.selected + 1).min(chooser.locations.len() - 1);
            } else if up {
                chooser.selected = chooser.selected.saturating_sub(1);
            }
            if enter {
                let location = chooser.locations[chooser.selected].clone();
                self.lsp_definitions = None;
                self.navigate_to_definition(location);
            } else if escape {
                self.lsp_definitions = None;
            }
            return down || up || enter || escape;
        }
        if self.lsp_completion.is_some() {
            let down =
                ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowDown));
            let up = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::ArrowUp));
            let enter = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Enter));
            let tab = ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Tab));
            let escape =
                ctx.input_mut(|input| input.consume_key(egui::Modifiers::NONE, Key::Escape));
            let popup = self.lsp_completion.as_mut().unwrap();
            if down {
                popup.selected = (popup.selected + 1).min(popup.items.len() - 1);
            } else if up {
                popup.selected = popup.selected.saturating_sub(1);
            }
            if enter || tab {
                self.accept_completion();
            } else if escape {
                self.lsp_completion = None;
            }
            return down || up || enter || tab || escape;
        }
        false
    }

    pub(super) fn open_settings(&mut self) {
        self.settings_open = true;
        self.lsp_completion = None;
        self.lsp_definitions = None;
    }

    pub(super) fn accept_completion(&mut self) {
        let Some(popup) = self.lsp_completion.take() else {
            return;
        };
        if !self.tag_matches_cursor(&popup.tag) {
            return;
        }
        let Some(item) = popup.items.get(popup.selected) else {
            return;
        };
        let Some(index) = self.active_tab else {
            return;
        };
        let tab = &mut self.tabs[index];
        let (range, replacement) = if let Some(edit) = &item.edit {
            if edit.range.end > tab.buffer.text.len()
                || !tab.buffer.text.is_char_boundary(edit.range.start)
                || !tab.buffer.text.is_char_boundary(edit.range.end)
            {
                return;
            }
            (
                tab.buffer.text[..edit.range.start].chars().count()
                    ..tab.buffer.text[..edit.range.end].chars().count(),
                edit.new_text.as_str(),
            )
        } else {
            (
                completion_word_range(&tab.buffer.text, popup.tag.cursor),
                item.insert_text.as_str(),
            )
        };
        tab.editor_surface.set_selection(range.start, range.end);
        if tab
            .editor_surface
            .replace_selection(&mut tab.buffer.text, replacement)
        {
            self.mark_tab_changed(index);
            self.lsp_caret = None;
        }
    }

    pub(super) fn navigate_diagnostic(&mut self, forward: bool) {
        let Some(index) = self.active_tab else {
            return;
        };
        let path = self.tabs[index].buffer.path.clone();
        let Some(state) = self.lsp_diagnostics.get(&path) else {
            return;
        };
        if state.diagnostics.is_empty() {
            return;
        }
        let cursor = self.tabs[index]
            .buffer
            .byte_index(self.tabs[index].editor_surface.cursor());
        let target = if forward {
            state
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.range.start > cursor)
                .min_by_key(|diagnostic| diagnostic.range.start)
                .or_else(|| {
                    state
                        .diagnostics
                        .iter()
                        .min_by_key(|diagnostic| diagnostic.range.start)
                })
        } else {
            state
                .diagnostics
                .iter()
                .filter(|diagnostic| diagnostic.range.start < cursor)
                .max_by_key(|diagnostic| diagnostic.range.start)
                .or_else(|| {
                    state
                        .diagnostics
                        .iter()
                        .max_by_key(|diagnostic| diagnostic.range.start)
                })
        };
        let Some(target) = target else {
            return;
        };
        let character = self.tabs[index].buffer.text[..target.range.start]
            .chars()
            .count();
        self.tabs[index]
            .editor_surface
            .set_selection(character, character);
        self.lsp_scroll_to = Some((path, character));
        self.focus_editor = true;
        self.tree_focused = false;
    }

    pub(super) fn navigate_to_definition(&mut self, location: DefinitionLocation) {
        if (location.end_line, location.end_character) < (location.line, location.character) {
            self.show_error("Language server returned a reversed definition range".into());
            return;
        }
        match fs::metadata(&location.path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => {
                self.show_error(format!(
                    "Definition target is not a file: {}",
                    location.path.display()
                ));
                return;
            }
            Err(error) => {
                self.show_error(format!(
                    "Cannot open definition {}: {error}",
                    location.path.display()
                ));
                return;
            }
        }
        self.open_tab(location.path.clone(), false);
        let Some(index) = self
            .tabs
            .iter()
            .position(|tab| tab.buffer.path == location.path)
        else {
            return;
        };
        let byte = crate::lsp::byte_for_position(
            &self.tabs[index].buffer.text,
            lsp_types::Position::new(location.line, location.character),
        );
        let character = self.tabs[index].buffer.text[..byte].chars().count();
        self.tabs[index]
            .editor_surface
            .set_selection(character, character);
        self.activate_tab(index);
        self.lsp_scroll_to = Some((location.path, character));
    }

    /// Opens a path the agent surfaced in the transcript, optionally placing
    /// the cursor at a 1-based line number.
    pub(super) fn open_agent_path(&mut self, path: PathBuf, line: Option<u32>) {
        let path = if path.is_absolute() {
            path
        } else {
            self.tree.root.join(path)
        };
        match fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => {
                self.show_error(format!("Not a file: {}", path.display()));
                return;
            }
            Err(error) => {
                self.show_error(format!("Cannot open {}: {error}", path.display()));
                return;
            }
        }
        self.open_tab(path.clone(), false);
        let Some(index) = self.tabs.iter().position(|tab| tab.buffer.path == path) else {
            return;
        };
        if let Some(line) = line {
            let text = &self.tabs[index].buffer.text;
            let character = text
                .split_inclusive('\n')
                .take(line.saturating_sub(1) as usize)
                .map(|line| line.chars().count())
                .sum::<usize>();
            self.tabs[index]
                .editor_surface
                .set_selection(character, character);
            self.lsp_scroll_to = Some((path, character));
        }
        self.focus_editor = true;
        self.tree_focused = false;
    }

    /// Opens the session diff for a file the agent changed: a right-hand
    /// panel beside the conversation in agentic mode, a diff view inside the
    /// file's tab otherwise. `path` is the changed-files key, so the
    /// baseline lookup happens before resolving it against the root.
    pub(super) fn open_agent_diff(&mut self, path: PathBuf) {
        let baseline = self.agent.baselines.get(&path).cloned();
        let absolute = if path.is_absolute() {
            path
        } else {
            self.tree.root.join(path)
        };
        if self.agentic_mode {
            match read_utf8_bounded(&absolute, AGENTIC_DIFF_MAX_BYTES) {
                Ok(text) => {
                    // Without a recorded baseline (kind-only edit, oversized
                    // file) the panel still shows the file; old == new
                    // renders every line as context.
                    let baseline = baseline.unwrap_or_else(|| Some(text.clone()));
                    let panel = AgenticDiff {
                        path: absolute,
                        baseline,
                        text,
                        error: None,
                    };
                    if let Some(index) = self
                        .agentic_diffs
                        .iter()
                        .position(|open| open.path == panel.path)
                    {
                        self.agentic_diffs[index] = panel;
                        self.active_agentic_diff = index;
                    } else {
                        self.agentic_diffs.push(panel);
                        self.active_agentic_diff = self.agentic_diffs.len() - 1;
                    }
                }
                Err(error) => {
                    self.show_error(format!("Cannot open {}: {error}", absolute.display()));
                }
            }
            return;
        }
        self.open_agent_path(absolute.clone(), None);
        // Without a baseline the plain open above is the whole action.
        let Some(baseline) = baseline else {
            return;
        };
        if let Some(tab) = self.tabs.iter_mut().find(|tab| tab.buffer.path == absolute) {
            tab.agent_diff = Some(baseline);
        }
    }

    /// Keeps the agentic diff panel current while the agent continues to
    /// edit open files; the baseline side never moves.
    pub(super) fn refresh_agentic_diff(&mut self) {
        for panel in &mut self.agentic_diffs {
            refresh_agentic_diff_panel(panel);
        }
    }

    pub(super) fn refresh_agentic_diff_paths(&mut self, paths: &HashSet<PathBuf>) {
        for panel in self
            .agentic_diffs
            .iter_mut()
            .filter(|panel| paths.contains(&panel.path))
        {
            refresh_agentic_diff_panel(panel);
        }
    }

    pub(super) fn refresh_find_matches(&mut self, pane: PaneId) {
        let index = self
            .pane_active_tabs
            .get(&pane)
            .and_then(|path| self.tabs.iter().position(|tab| &tab.buffer.path == path))
            .or_else(|| self.tabs.iter().position(|tab| tab.pane == pane));
        let Some(find) = self.pane_find.get_mut(&pane) else {
            return;
        };
        let Some(index) = index else {
            find.matches.clear();
            find.selected = 0;
            return;
        };
        let buffer = &self.tabs[index].buffer;
        if find.match_revision == buffer.revision && find.match_query == find.query {
            return;
        }
        find.matches = match_spans(&buffer.text, &find.query);
        find.match_revision = buffer.revision;
        find.match_query.clone_from(&find.query);
        find.selected = find.selected.min(find.matches.len().saturating_sub(1));
        self.tabs[index].highlight_cache.find_valid = false;
    }
}

fn refresh_agentic_diff_panel(panel: &mut AgenticDiff) {
    match read_utf8_bounded(&panel.path, AGENTIC_DIFF_MAX_BYTES) {
        Ok(text) => {
            panel.error = None;
            if text != panel.text {
                panel.text = text;
            }
        }
        Err(error) => panel.error = Some(error),
    }
}
