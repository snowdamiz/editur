use super::*;

impl EditorApp {
    pub fn request_close(&mut self) {
        self.request(PendingAction::Close);
    }

    pub(super) fn request_target(&mut self, target: OpenTarget) {
        if target.root == self.tree.root {
            if let Some(path) = target.file {
                self.open_tab(path, target.create);
            }
        } else {
            self.request(PendingAction::OpenTarget(target));
        }
    }

    pub(super) fn buffer(&self) -> Option<&Buffer> {
        if !self.git_diffs.is_empty() {
            return None;
        }
        self.active_tab
            .and_then(|index| self.tabs.get(index))
            .map(|tab| &tab.buffer)
    }

    pub(super) fn activate_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        self.git_diffs.clear();
        self.git_panes = PaneTabs::default();
        let changed = self.active_tab != Some(index);
        self.active_tab = Some(index);
        self.editor_panes
            .activate(self.tabs[index].pane, self.tabs[index].buffer.path.clone());
        self.tree.select(Some(self.tabs[index].buffer.path.clone()));
        if changed {
            self.lsp_completion = None;
            self.lsp_definitions = None;
            self.lsp_pending_completion = None;
            self.lsp_pending_definition = None;
            if let Some(find) = self.pane_find.get_mut(&self.editor_panes.active_pane) {
                find.match_revision = u64::MAX;
                find.scroll_to_match = find.open;
            }
            self.bracket_pair = None;
            self.bracket_pair_key = None;
            self.cursor = self.tabs[index]
                .buffer
                .line_column(self.tabs[index].editor_surface.cursor());
        }
        self.focus_editor = true;
        self.tree_focused = false;
        self.scm_focused = false;
    }

    pub(super) fn open_tab(&mut self, path: PathBuf, create: bool) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.buffer.path == path) {
            self.activate_tab(index);
            return;
        }
        let buffer = if create {
            Ok(Buffer::new(path.clone()))
        } else {
            load_buffer(&path)
        };
        match buffer {
            Ok(buffer) => {
                self.tabs
                    .push(FileTab::new(buffer, self.editor_panes.active_pane));
                self.activate_tab(self.tabs.len() - 1);
                self.lsp_sync_needed = true;
            }
            Err(error) => self.show_error(error),
        }
    }

    pub(super) fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let was_active = self.active_tab == Some(index);
        let removed = self.tabs.remove(index);
        self.lsp_sync_needed = true;
        if self.active_tab.is_some_and(|active| active > index) {
            self.active_tab = self.active_tab.map(|active| active - 1);
        }
        let next_in_pane = self.tabs.iter().position(|tab| tab.pane == removed.pane);
        if self.editor_panes.active(removed.pane) == Some(&removed.buffer.path) {
            if let Some(next) = next_in_pane {
                self.editor_panes
                    .active_tabs
                    .insert(removed.pane, self.tabs[next].buffer.path.clone());
            } else {
                self.editor_panes.active_tabs.remove(&removed.pane);
            }
        }
        if next_in_pane.is_none() {
            self.editor_panes.layout.remove(removed.pane);
            self.pane_find.remove(&removed.pane);
        }
        if was_active {
            self.active_tab = None;
            if let Some(next) = next_in_pane.or_else(|| (!self.tabs.is_empty()).then_some(0)) {
                self.activate_tab(next);
            } else {
                self.tree.select(None);
                self.cursor = (1, 1);
                self.bracket_pair = None;
                self.bracket_pair_key = None;
            }
        }
    }

    pub(super) fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active_tab = self.active_tab.map(|active| {
            if active == from {
                to
            } else if from < active && active <= to {
                active - 1
            } else if to <= active && active < from {
                active + 1
            } else {
                active
            }
        });
    }

    pub(super) fn drop_tab(&mut self, index: usize, target: PaneId, zone: DropZone) {
        if index >= self.tabs.len() {
            return;
        }
        let source = self.tabs[index].pane;
        if source == target
            && zone != DropZone::Center
            && self.tabs.iter().filter(|tab| tab.pane == source).count() == 1
        {
            return;
        }
        let moved_path = self.tabs[index].buffer.path.clone();
        let pane = if zone == DropZone::Center {
            target
        } else if let Some(pane) = self.editor_panes.layout.split(target, zone) {
            pane
        } else {
            target
        };
        self.tabs[index].pane = pane;
        if source != pane {
            if let Some(tab) = self.tabs.iter().find(|tab| tab.pane == source) {
                if self.editor_panes.active(source) == Some(&moved_path) {
                    self.editor_panes
                        .active_tabs
                        .insert(source, tab.buffer.path.clone());
                }
            } else {
                self.editor_panes.active_tabs.remove(&source);
                self.editor_panes.layout.remove(source);
                self.pane_find.remove(&source);
            }
        }
        self.activate_tab(index);
    }

    pub(super) fn drop_path(&mut self, path: PathBuf, target: PaneId, zone: DropZone) {
        self.open_tab(path.clone(), false);
        if let Some(index) = self.tabs.iter().position(|tab| tab.buffer.path == path) {
            self.drop_tab(index, target, zone);
        }
    }

    pub(super) fn request(&mut self, action: PendingAction) {
        let dirty = match &action {
            PendingAction::Open(_) => None,
            PendingAction::CloseTab(index) => self
                .tabs
                .get(*index)
                .filter(|tab| tab.buffer.dirty)
                .map(|_| *index),
            PendingAction::OpenTarget(_) | PendingAction::Close => {
                self.tabs.iter().position(|tab| tab.buffer.dirty)
            }
        };
        if let Some(index) = dirty {
            self.activate_tab(index);
            self.pending = Some(action);
            return;
        }
        self.perform(action);
    }

    pub(super) fn perform(&mut self, action: PendingAction) {
        match action {
            PendingAction::Open(path) => self.open_tab(path, false),
            PendingAction::OpenTarget(target) => self.replace_project(target),
            PendingAction::CloseTab(index) => self.close_tab(index),
            PendingAction::Close => self.should_close = true,
        }
    }

    fn replace_project(&mut self, target: OpenTarget) {
        let preserve_agent_sidebar = self.agentic_mode;
        let previous_account = self.selected_account;
        let previous_project = self.tree.root.clone();
        let previous_sessions = self.agent.sessions.clone();
        let project_order = self.agentic_project_roots();
        match Self::new(target) {
            Ok(mut editor) => {
                editor.agentic_mode = self.agentic_mode;
                editor.agent_boot_pending = self.agentic_mode;
                if preserve_agent_sidebar {
                    let mut sessions = std::mem::take(&mut self.agent_project_sessions);
                    if let Some(previous_sessions) = previous_sessions {
                        sessions.insert(
                            (previous_account, previous_project),
                            Some(previous_sessions),
                        );
                    }
                    editor.agent.sessions = sessions
                        .get(&(editor.selected_account, editor.tree.root.clone()))
                        .cloned()
                        .flatten();
                    editor.agent.history_available = editor.agent.sessions.is_some();
                    editor.agent_sidebar_history_pending = editor.agent.sessions.is_some();
                    editor.agent_project_sessions = sessions;
                    editor.agent_project_session_limits =
                        std::mem::take(&mut self.agent_project_session_limits);
                    editor.agent_collapsed_projects =
                        std::mem::take(&mut self.agent_collapsed_projects);
                    editor.recent_projects = project_order;
                }
                *self = editor;
            }
            Err(error) => self.show_error(error),
        }
    }

    /// Replaces the workspace with `root`, remembering both the project we are
    /// leaving and the one we are entering so either shows up in the switcher
    /// next time.
    pub(super) fn switch_project(&mut self, root: PathBuf) {
        self.project_menu = false;
        if root == self.tree.root {
            return;
        }
        if let Ok(directory) = data_dir() {
            let _ = crate::projects::remember(&directory, &self.tree.root);
            if let Err(error) = crate::projects::remember(&directory, &root) {
                self.show_error(error);
            }
        }
        let target = OpenTarget {
            root,
            file: None,
            create: false,
        };
        self.request(PendingAction::OpenTarget(target));
    }

    /// Opens the custom folder picker instead of a platform dialog: the same
    /// instant browser everywhere, on every OS, with no modal frame stall.
    pub(super) fn add_project_via_dialog(&mut self) {
        self.project_menu = false;
        let start = directories::UserDirs::new()
            .map(|directories| directories.home_dir().to_path_buf())
            .unwrap_or_else(|| self.tree.root.clone());
        match WorkspaceFilePicker::open_directories(start) {
            Ok(mut picker) => {
                picker.recent = self
                    .recent_projects
                    .iter()
                    .filter(|path| **path != self.tree.root && path.is_dir())
                    .take(6)
                    .cloned()
                    .collect();
                self.project_folder_picker = Some(picker);
            }
            Err(error) => self.show_error(error),
        }
    }

    pub(super) fn save(&mut self, destination: Option<PathBuf>) -> bool {
        let Some(index) = self.active_tab else {
            return true;
        };
        if !self.git_diffs.is_empty() {
            return true;
        }
        let save_as = destination.is_some();
        let old_path = self.tabs[index].buffer.path.clone();
        let path = destination.unwrap_or_else(|| old_path.clone());
        let path_changed = path != old_path;
        match safe_save(&mut self.tabs[index].buffer, &path) {
            Ok(()) => {
                self.schedule_git_refresh();
                if path_changed {
                    self.tabs[index].highlight_cache.valid = false;
                    self.lsp_sync_needed = true;
                } else if self.lsp_open.contains_key(&path) {
                    self.lsp_pending_saves.insert(path);
                    self.lsp_sync_needed = true;
                }
                true
            }
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

    pub(super) fn finish_pending(&mut self) {
        if let Some(action) = self.pending.take() {
            self.request(action);
        }
    }

    pub(super) fn discard_pending(&mut self) {
        let Some(action) = self.pending.take() else {
            return;
        };
        match action {
            PendingAction::CloseTab(index) => self.close_tab(index),
            PendingAction::OpenTarget(_) | PendingAction::Close => {
                if let Some(index) = self.active_tab {
                    self.close_tab(index);
                }
                self.request(action);
            }
            PendingAction::Open(path) => self.open_tab(path, false),
        }
    }

    pub(super) fn show_error(&mut self, error: String) {
        eprintln!("editur: {error}");
        self.toasts.push(Severity::Danger, error);
    }
}
