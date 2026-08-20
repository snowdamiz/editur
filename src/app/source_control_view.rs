use super::*;

#[derive(Clone)]
enum SourceControlAction {
    Refresh,
    Init,
    SelectRepository(PathBuf),
    Stage(PathBuf, Vec<PathBuf>),
    Unstage(PathBuf, Vec<PathBuf>),
    Discard(PathBuf, Vec<PathBuf>),
    Commit(PathBuf),
    Diff(PathBuf, PathBuf, DiffArea),
    Open(PathBuf),
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum SourceGroup {
    Merge,
    Staged,
    Changes,
}

impl EditorApp {
    #[cfg(test)]
    pub(super) fn benchmark_draw_source_control(&mut self, ui: &mut egui::Ui) {
        let _ = self.draw_source_control_repository(ui);
    }

    pub(super) fn open_source_control(&mut self, ctx: &egui::Context) {
        if self.git_controller.is_none() {
            let wake = ctx.clone();
            self.git_controller = Some(GitController::start(self.tree.root.clone(), move || {
                wake.request_repaint();
            }));
        }
        self.git_state.focus_commit = true;
        self.schedule_git_refresh();
        ctx.request_repaint();
    }

    pub(super) fn schedule_git_refresh(&mut self) {
        if self.git_controller.is_some() {
            self.git_refresh_at = Some(Instant::now() + Duration::from_millis(200));
        }
    }

    pub(super) fn flush_git_refresh(&mut self, ctx: &egui::Context) {
        let Some(deadline) = self.git_refresh_at else {
            return;
        };
        let now = Instant::now();
        if now < deadline {
            ctx.request_repaint_after(deadline - now);
            return;
        }
        self.git_refresh_at = None;
        self.send_git(GitCommand::Refresh);
    }

    pub(super) fn poll_git(&mut self) {
        let events = self
            .git_controller
            .as_ref()
            .map(|controller| controller.events().try_iter().collect::<Vec<_>>())
            .unwrap_or_default();
        for event in events {
            match &event {
                GitEvent::OperationFailed { op, message } => {
                    self.toasts.push(
                        Severity::Danger,
                        format!("Git {op} failed — {}", redact_home(message)),
                    );
                }
                GitEvent::Committed {
                    short_hash,
                    subject,
                    ..
                } => {
                    self.toasts.push(
                        Severity::Neutral,
                        format!("Committed {short_hash} — {subject}"),
                    );
                }
                GitEvent::WorktreeMutated { repository, paths } => {
                    self.reconcile_git_paths(repository, paths)
                }
                GitEvent::Diff { .. } => self.open_git_diff(event.clone()),
                _ => {}
            }
            self.git_state.apply(&event);
            if let GitEvent::Status(snapshot) = &event {
                self.refresh_active_git_diff(snapshot.generation);
            }
        }
    }

    fn refresh_active_git_diff(&mut self, generation: u64) {
        let Some(diff) = self
            .git_diff
            .as_ref()
            .filter(|diff| diff.generation < generation)
            .cloned()
        else {
            return;
        };
        let still_changed = self
            .git_state
            .snapshot
            .as_ref()
            .and_then(|snapshot| {
                snapshot
                    .repositories
                    .iter()
                    .find(|repository| repository.root == diff.repository)
            })
            .is_some_and(|repository| {
                repository.entries.iter().any(|entry| {
                    entry.path == diff.path
                        && match diff.area {
                            DiffArea::Staged => entry.index.is_some(),
                            DiffArea::Worktree => entry.worktree.is_some(),
                        }
                })
            });
        if still_changed {
            self.send_git(GitCommand::LoadDiff {
                repository: diff.repository,
                path: diff.path,
                area: diff.area,
            });
        }
    }

    fn reconcile_git_paths(&mut self, repository: &Path, paths: &[PathBuf]) {
        let affected = paths
            .iter()
            .map(|path| repository.join(path))
            .collect::<HashSet<_>>();
        let mut close = Vec::new();
        let mut conflict = None;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            if !affected.contains(&tab.buffer.path) {
                continue;
            }
            if !tab.buffer.path.exists() {
                if tab.buffer.dirty {
                    conflict.get_or_insert(index);
                } else {
                    close.push(index);
                }
                continue;
            }
            match reconcile_buffer(&mut tab.buffer) {
                Ok(ReconcileOutcome::Unchanged) => {}
                Ok(ReconcileOutcome::Reloaded) => {
                    tab.editor_surface = EditorSurface::default();
                    tab.highlight_cache = HighlightCache::default();
                    tab.markdown_layout = None;
                    self.lsp_sync_needed = true;
                }
                Ok(ReconcileOutcome::Conflict) => {
                    conflict.get_or_insert(index);
                }
                Err(message) => {
                    self.toasts.push(Severity::Danger, message);
                }
            }
        }
        for index in close.into_iter().rev() {
            self.close_tab(index);
        }
        if let Some(index) = conflict {
            self.activate_tab(index);
            self.conflict = true;
        }
    }

    pub(super) fn open_git_diff(&mut self, event: GitEvent) {
        let GitEvent::Diff {
            repository,
            path,
            area,
            old,
            new,
            generation,
        } = event
        else {
            return;
        };
        if self
            .git_state
            .snapshot
            .as_ref()
            .is_some_and(|snapshot| generation < snapshot.generation)
        {
            return;
        }
        let source_path = repository.join(&path);
        let unsaved_editor_changes = self
            .tabs
            .iter()
            .any(|tab| tab.buffer.path == source_path && tab.buffer.dirty);
        let mut content = DefaultHasher::new();
        old.hash(&mut content);
        new.hash(&mut content);
        generation.hash(&mut content);
        let diff = GitDiffPreview {
            repository,
            path,
            area,
            old,
            new,
            generation,
            content_revision: content.finish(),
            unsaved_editor_changes,
        };
        self.git_diff = Some(diff);
        self.focus_editor = false;
    }

    pub(super) fn draw_source_control(&mut self, ui: &mut egui::Ui) {
        if self.git_controller.is_none() {
            self.open_source_control(ui.ctx());
        }
        let action = match self.git_state.availability {
            GitAvailability::Unavailable => {
                source_control_empty(ui, "Git isn't available on this machine.");
                None
            }
            GitAvailability::Loading if self.git_state.snapshot.is_none() => {
                source_control_empty(ui, "Loading…");
                None
            }
            _ if self
                .git_state
                .snapshot
                .as_ref()
                .is_some_and(|snapshot| snapshot.repositories.is_empty()) =>
            {
                source_control_no_repository(
                    ui,
                    self.git_state.pending_operation == Some("initialize"),
                )
            }
            _ => self.draw_source_control_repository(ui),
        };
        if let Some(action) = action {
            self.handle_source_control_action(action, ui.ctx());
        }
    }

    fn draw_source_control_repository(&mut self, ui: &mut egui::Ui) -> Option<SourceControlAction> {
        let snapshot = self.git_state.snapshot.clone()?;
        let repository = self.git_state.repository()?.clone();
        let mut action = None;
        ui.spacing_mut().item_spacing = egui::vec2(theme::space::TIGHT, 0.0);

        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(theme::space::MEDIUM as i8, 2))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (icon, _) =
                        ui.allocate_exact_size(egui::Vec2::splat(icons::GRID), Sense::hover());
                    icons::paint(
                        ui.painter(),
                        Icon::SourceControl,
                        icon,
                        theme::text().secondary,
                    );
                    let (branch, counts) = branch_label(&repository.info.branch);
                    let font = theme::typography::small_strong();
                    let branch_width = ui
                        .painter()
                        .layout_no_wrap(branch.clone(), font.clone(), theme::text().primary)
                        .size()
                        .x;
                    let counts_width = (!counts.is_empty()).then(|| {
                        ui.painter()
                            .layout_no_wrap(
                                counts.clone(),
                                theme::typography::micro(),
                                theme::text().muted,
                            )
                            .size()
                            .x
                    });
                    let reserved = theme::control::COMPACT
                        + theme::space::TIGHT
                        + counts_width
                            .map(|width| width + theme::space::TIGHT)
                            .unwrap_or_default();
                    let branch_width = branch_width
                        .min((ui.available_width() - reserved).max(theme::control::COMPACT));
                    ui.add_sized(
                        egui::vec2(branch_width, theme::control::COMPACT),
                        Label::new(
                            RichText::new(branch)
                                .font(font)
                                .color(theme::text().primary),
                        )
                        .truncate(),
                    );
                    if counts_width.is_some() {
                        ui.label(
                            RichText::new(counts)
                                .font(theme::typography::micro())
                                .color(theme::text().muted),
                        );
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icons::button_with_id(
                            ui,
                            Some(Id::new("source_control_refresh")),
                            Icon::Refresh,
                            "Refresh",
                            theme::text().secondary,
                            egui::Vec2::splat(theme::control::COMPACT),
                        )
                        .clicked()
                        {
                            action = Some(SourceControlAction::Refresh);
                        }
                    });
                });
            });

        if snapshot.repositories.len() > 1 {
            let selected = repository_name(&self.tree.root, &repository.root);
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(theme::space::MEDIUM as i8, 0))
                .show(ui, |ui| {
                    egui::ComboBox::from_id_salt("git_repository")
                        .selected_text(selected)
                        .width(ui.available_width())
                        .show_ui(ui, |ui| {
                            for candidate in &snapshot.repositories {
                                let label = repository_name(&self.tree.root, &candidate.root);
                                if ui
                                    .selectable_label(candidate.root == repository.root, label)
                                    .clicked()
                                {
                                    action = Some(SourceControlAction::SelectRepository(
                                        candidate.root.clone(),
                                    ));
                                }
                            }
                        });
                });
        }

        let staged_count = repository
            .entries
            .iter()
            .filter(|entry| entry.index.is_some() && entry.index != Some(ChangeKind::Conflicted))
            .count();
        let committing = self.git_state.pending_operation == Some("commit");
        let commit_enabled =
            staged_count > 0 && !self.git_state.commit_message.trim().is_empty() && !committing;
        let disabled_reason = if committing {
            "Commit in progress"
        } else if staged_count == 0 {
            "Stage changes to commit"
        } else {
            "Enter a commit message"
        };
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(theme::space::MEDIUM as i8, 0))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let commit_frame = egui::Frame::new()
                    .fill(theme::surface().input)
                    .corner_radius(theme::corner(theme::radius::CONTROL))
                    .inner_margin(egui::Margin::symmetric(6, 4))
                    .show(ui, |ui| {
                        let (rect, _) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), 44.0),
                            Sense::hover(),
                        );
                        let commit_rect = egui::Rect::from_min_size(
                            rect.right_bottom()
                                - egui::vec2(theme::control::COMPACT, theme::control::COMPACT),
                            egui::Vec2::splat(theme::control::COMPACT),
                        );
                        let text_rect = rect
                            .with_max_x(commit_rect.left() - theme::space::TIGHT)
                            .shrink(theme::space::HAIR);
                        let response = ui.put(
                            text_rect,
                            TextEdit::multiline(&mut self.git_state.commit_message)
                                .id(Id::new("git_commit_message"))
                                .font(theme::typography::body())
                                .hint_text("Commit message")
                                .desired_rows(2)
                                .frame(egui::Frame::NONE),
                        );
                        let commit = ui
                            .interact(
                                commit_rect,
                                Id::new("git_commit"),
                                if commit_enabled {
                                    Sense::click()
                                } else {
                                    Sense::hover()
                                },
                            )
                            .on_hover_text(if commit_enabled {
                                "Commit (Ctrl/Cmd+Enter)"
                            } else {
                                disabled_reason
                            });
                        commit.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                commit_enabled,
                                if commit_enabled {
                                    "Commit".to_owned()
                                } else {
                                    format!("Commit — {disabled_reason}")
                                },
                            )
                        });
                        icons::paint_button(
                            ui.painter(),
                            Icon::Check,
                            commit_rect,
                            &commit,
                            commit_enabled,
                            theme::accent(),
                        );
                        (response, commit.clicked())
                    });
                ui.painter().rect_stroke(
                    commit_frame.response.rect,
                    theme::corner(theme::radius::CONTROL),
                    theme::border::hairline(),
                    egui::StrokeKind::Inside,
                );
                let (response, commit_clicked) = commit_frame.inner;
                if std::mem::take(&mut self.git_state.focus_commit) {
                    response.request_focus();
                }
                let command_enter = response.has_focus()
                    && ui.input_mut(|input| {
                        input.modifiers.command && input.consume_key(input.modifiers, Key::Enter)
                    });
                if commit_clicked || (command_enter && commit_enabled) {
                    action = Some(SourceControlAction::Commit(repository.root.clone()));
                }
            });
        ui.add_space(theme::space::SMALL);

        let keyboard = self.source_control_keyboard(ui.ctx(), &repository);
        if keyboard.is_some() {
            action = keyboard;
        }
        let mut row_index = 0;
        let list_bottom = if self.git_state.last_refresh_error.is_some() {
            ui.available_height() - theme::control::COMPACT
        } else {
            ui.available_height()
        };
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), list_bottom.max(0.0)),
            Layout::top_down(Align::Min),
            |ui| {
                ScrollArea::vertical()
                    .id_salt(("source_control_changes", &repository.root))
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let (mut merge, mut staged, mut changes) =
                            (Vec::new(), Vec::new(), Vec::new());
                        for entry in &repository.entries {
                            if is_conflicted(entry) {
                                merge.push(entry);
                                continue;
                            }
                            if entry.index.is_some() {
                                staged.push(entry);
                            }
                            if entry.worktree.is_some() {
                                changes.push(entry);
                            }
                        }
                        for (group, label, entries) in [
                            (SourceGroup::Merge, "MERGE CHANGES", merge),
                            (SourceGroup::Staged, "STAGED CHANGES", staged),
                            (SourceGroup::Changes, "CHANGES", changes),
                        ] {
                            if entries.is_empty() {
                                continue;
                            }
                            let pending = entries.iter().any(|entry| {
                                self.git_state
                                    .pending_paths
                                    .contains(&(repository.root.clone(), entry.path.clone()))
                            });
                            if row_is_visible(ui, theme::control::COMPACT + theme::space::TIGHT) {
                                if let Some(found) = source_group_header(
                                    ui,
                                    &repository.root,
                                    group,
                                    label,
                                    &entries,
                                    pending,
                                ) {
                                    action = Some(found);
                                }
                            } else {
                                reserve_row(ui, theme::control::COMPACT + theme::space::TIGHT);
                            }
                            for entry in entries {
                                if row_is_visible(ui, theme::control::COMPACT) {
                                    if let Some(found) = source_control_row(
                                        ui,
                                        &repository.root,
                                        group,
                                        entry,
                                        row_index,
                                        self.git_state.focus_index == Some(row_index),
                                        self.git_state.pending_paths.contains(&(
                                            repository.root.clone(),
                                            entry.path.clone(),
                                        )),
                                    ) {
                                        self.git_state.focus_index = Some(row_index);
                                        self.scm_focused = true;
                                        action = Some(found);
                                    }
                                } else {
                                    reserve_row(ui, theme::control::COMPACT);
                                }
                                row_index += 1;
                            }
                        }
                        if row_index == 0 {
                            ui.vertical_centered(|ui| {
                                ui.add_space(theme::space::SMALL);
                                ui.label(
                                    RichText::new("No changes")
                                        .font(theme::typography::body())
                                        .color(theme::text().muted),
                                );
                            });
                        }
                    });
            },
        );
        if self.git_state.last_refresh_error.is_some() {
            ui.label(
                RichText::new("Couldn't refresh status — retrying")
                    .font(theme::typography::micro())
                    .color(theme::text().muted),
            );
        }
        action
    }

    fn source_control_keyboard(
        &mut self,
        ctx: &egui::Context,
        repository: &RepositoryStatus,
    ) -> Option<SourceControlAction> {
        if !self.scm_focused || ctx.memory(|memory| memory.has_focus(Id::new("git_commit_message")))
        {
            return None;
        }
        let rows = source_rows(repository);
        if rows.is_empty() {
            return None;
        }
        let (up, down, enter, space, delete, escape) = ctx.input_mut(|input| {
            (
                input.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                input.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                input.consume_key(egui::Modifiers::NONE, Key::Enter),
                input.consume_key(egui::Modifiers::NONE, Key::Space),
                input.consume_key(egui::Modifiers::NONE, Key::Delete)
                    || input.consume_key(egui::Modifiers::NONE, Key::Backspace),
                input.consume_key(egui::Modifiers::NONE, Key::Escape),
            )
        });
        if escape {
            self.scm_focused = false;
            self.focus_editor = self.active_tab.is_some();
            return None;
        }
        if down {
            self.git_state.focus_index = Some(
                self.git_state
                    .focus_index
                    .map_or(0, |index| index.saturating_add(1).min(rows.len() - 1)),
            );
        } else if up {
            self.git_state.focus_index = Some(
                self.git_state
                    .focus_index
                    .map_or(0, |index| index.saturating_sub(1)),
            );
        }
        let (group, entry) = &rows[self.git_state.focus_index?.min(rows.len() - 1)];
        if enter {
            return Some(if *group == SourceGroup::Merge {
                SourceControlAction::Open(repository.root.join(&entry.path))
            } else {
                SourceControlAction::Diff(
                    repository.root.clone(),
                    entry.path.clone(),
                    group_area(*group),
                )
            });
        }
        if space {
            return Some(match group {
                SourceGroup::Staged => {
                    SourceControlAction::Unstage(repository.root.clone(), vec![entry.path.clone()])
                }
                SourceGroup::Merge | SourceGroup::Changes => {
                    SourceControlAction::Stage(repository.root.clone(), vec![entry.path.clone()])
                }
            });
        }
        if delete && *group == SourceGroup::Changes {
            return Some(SourceControlAction::Discard(
                repository.root.clone(),
                vec![entry.path.clone()],
            ));
        }
        None
    }

    fn handle_source_control_action(&mut self, action: SourceControlAction, ctx: &egui::Context) {
        match action {
            SourceControlAction::Refresh => self.send_git(GitCommand::Refresh),
            SourceControlAction::Init => {
                self.git_state.pending_operation = Some("initialize");
                self.send_git(GitCommand::Init);
            }
            SourceControlAction::SelectRepository(root) => {
                self.git_state.selected_repository = Some(root);
                self.git_state.focus_index = None;
                self.git_state.focus_commit = true;
            }
            SourceControlAction::Stage(repository, paths) => {
                self.git_state.begin("stage", &repository, &paths);
                self.send_git(GitCommand::Stage { repository, paths });
            }
            SourceControlAction::Unstage(repository, paths) => {
                self.git_state.begin("unstage", &repository, &paths);
                self.send_git(GitCommand::Unstage { repository, paths });
            }
            SourceControlAction::Discard(repository, paths) => {
                let untracked = self
                    .git_state
                    .repository()
                    .map(|snapshot| {
                        paths
                            .iter()
                            .filter(|path| {
                                snapshot.entry(path).is_some_and(|entry| {
                                    entry.worktree == Some(ChangeKind::Untracked)
                                })
                            })
                            .count()
                    })
                    .unwrap_or_default();
                self.git_discard = Some(GitDiscardRequest {
                    repository,
                    paths,
                    untracked,
                });
            }
            SourceControlAction::Commit(repository) => {
                self.git_state.pending_operation = Some("commit");
                self.send_git(GitCommand::Commit {
                    repository,
                    message: self.git_state.commit_message.clone(),
                });
            }
            SourceControlAction::Diff(repository, path, area) => {
                self.send_git(GitCommand::LoadDiff {
                    repository,
                    path,
                    area,
                });
            }
            SourceControlAction::Open(path) => self.request(PendingAction::Open(path)),
        }
        ctx.request_repaint();
    }

    fn send_git(&mut self, command: GitCommand) {
        let result = self
            .git_controller
            .as_ref()
            .ok_or_else(|| "Git controller is not running".to_owned())
            .and_then(|controller| controller.send(command));
        if let Err(message) = result {
            self.git_state.pending_paths.clear();
            self.git_state.pending_operation = None;
            self.show_error(message);
        }
    }

    pub(super) fn draw_git_discard_dialog(&mut self, ctx: &egui::Context) {
        let Some(request) = self.git_discard.clone() else {
            return;
        };
        let count = request.paths.len();
        let body = if count == 1 && request.untracked == 1 {
            "This file is untracked. Discarding deletes it permanently.".to_owned()
        } else if count == 1 {
            "Changes are restored from the index and cannot be undone.".to_owned()
        } else if request.untracked == 0 {
            format!("Discard changes in {count} files? These changes cannot be undone.")
        } else {
            format!(
                "Discard changes in {count} files? {} untracked file{} will be deleted permanently.",
                request.untracked,
                if request.untracked == 1 { "" } else { "s" }
            )
        };
        let mut dialog = Dialog::new("git_discard_dialog", "Discard changes")
            .severity(Severity::Danger)
            .body(body)
            .destructive("Discard Changes");
        if count == 1 {
            dialog = dialog.path(&request.paths[0]);
        }
        match dialog.show(ctx) {
            Outcome::Destructive => {
                self.git_discard = None;
                self.git_state
                    .begin("discard", &request.repository, &request.paths);
                self.send_git(GitCommand::Discard {
                    repository: request.repository,
                    paths: request.paths,
                });
            }
            Outcome::Cancel | Outcome::Dismissed => self.git_discard = None,
            _ => {}
        }
    }
}

fn source_control_empty(ui: &mut egui::Ui, message: &str) {
    ui.centered_and_justified(|ui| {
        ui.label(
            RichText::new(message)
                .font(theme::typography::body())
                .color(theme::text().muted),
        );
    });
}

fn row_is_visible(ui: &egui::Ui, height: f32) -> bool {
    let row = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), height));
    ui.clip_rect().expand(height).intersects(row)
}

fn reserve_row(ui: &mut egui::Ui, height: f32) {
    ui.allocate_space(egui::vec2(ui.available_width(), height));
}

fn redact_home(message: &str) -> String {
    directories::UserDirs::new()
        .map(|directories| directories.home_dir().to_string_lossy().into_owned())
        .filter(|home| !home.is_empty())
        .map_or_else(|| message.to_owned(), |home| message.replace(&home, "~"))
}

fn source_control_no_repository(
    ui: &mut egui::Ui,
    initializing: bool,
) -> Option<SourceControlAction> {
    let mut initialize = false;
    ui.centered_and_justified(|ui| {
        ui.vertical_centered(|ui| {
            ui.label(
                RichText::new("This folder isn't a Git repository.")
                    .font(theme::typography::body())
                    .color(theme::text().muted),
            );
            initialize = ui
                .add_enabled(!initializing, egui::Button::new("Initialize Repository"))
                .clicked();
        });
    });
    initialize.then_some(SourceControlAction::Init)
}

fn branch_label(branch: &BranchInfo) -> (String, String) {
    match branch {
        BranchInfo::Named {
            name,
            ahead,
            behind,
            ..
        } => (
            name.clone(),
            format!(
                "{}{}",
                if *ahead > 0 {
                    format!("↑{ahead} ")
                } else {
                    String::new()
                },
                if *behind > 0 {
                    format!("↓{behind}")
                } else {
                    String::new()
                }
            )
            .trim()
            .to_owned(),
        ),
        BranchInfo::Detached { oid } => (oid.clone(), String::new()),
    }
}

fn repository_name(workspace: &Path, repository: &Path) -> String {
    repository
        .strip_prefix(workspace)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| repository.file_name().map(Path::new).unwrap_or(repository))
        .display()
        .to_string()
}

fn source_rows(repository: &RepositoryStatus) -> Vec<(SourceGroup, &GitEntry)> {
    let mut rows = Vec::new();
    rows.extend(
        repository
            .entries
            .iter()
            .filter(|entry| is_conflicted(entry))
            .map(|entry| (SourceGroup::Merge, entry)),
    );
    rows.extend(
        repository
            .entries
            .iter()
            .filter(|entry| entry.index.is_some() && !is_conflicted(entry))
            .map(|entry| (SourceGroup::Staged, entry)),
    );
    rows.extend(
        repository
            .entries
            .iter()
            .filter(|entry| entry.worktree.is_some() && !is_conflicted(entry))
            .map(|entry| (SourceGroup::Changes, entry)),
    );
    rows
}

fn source_group_header(
    ui: &mut egui::Ui,
    repository: &Path,
    group: SourceGroup,
    label: &str,
    entries: &[&GitEntry],
    pending: bool,
) -> Option<SourceControlAction> {
    let mut action = None;
    let header_rect = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(
            ui.available_width(),
            theme::control::COMPACT + theme::space::TIGHT,
        ),
    );
    let hovered = ui.rect_contains_pointer(header_rect);
    ui.interact(
        header_rect,
        Id::new(("source_control_group_header", repository, group as u8)),
        Sense::hover(),
    );
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(theme::space::MEDIUM as i8, 2))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{label} ({})", entries.len()))
                        .font(theme::typography::micro())
                        .color(theme::text().muted),
                );
                ui.add_enabled_ui(!pending, |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let paths = || entries.iter().map(|entry| entry.path.clone()).collect();
                        match group {
                            SourceGroup::Merge => {}
                            SourceGroup::Staged => {
                                if icons::button_sized(
                                    ui,
                                    Icon::Minus,
                                    "Unstage all",
                                    theme::text().secondary,
                                    egui::Vec2::splat(theme::control::COMPACT),
                                )
                                .clicked()
                                {
                                    action = Some(SourceControlAction::Unstage(
                                        repository.to_path_buf(),
                                        paths(),
                                    ));
                                }
                            }
                            SourceGroup::Changes => {
                                if icons::button_sized(
                                    ui,
                                    Icon::Plus,
                                    "Stage all",
                                    theme::text().secondary,
                                    egui::Vec2::splat(theme::control::COMPACT),
                                )
                                .clicked()
                                {
                                    action = Some(SourceControlAction::Stage(
                                        repository.to_path_buf(),
                                        paths(),
                                    ));
                                }
                                if hovered
                                    && icons::button_with_id(
                                        ui,
                                        Some(Id::new(("source_control_discard_all", repository))),
                                        Icon::Undo,
                                        "Discard all changes…",
                                        theme::semantic().danger,
                                        egui::Vec2::splat(theme::control::COMPACT),
                                    )
                                    .clicked()
                                {
                                    action = Some(SourceControlAction::Discard(
                                        repository.to_path_buf(),
                                        paths(),
                                    ));
                                }
                            }
                        }
                    });
                });
            });
        });
    action
}

fn source_control_row(
    ui: &mut egui::Ui,
    repository: &Path,
    group: SourceGroup,
    entry: &GitEntry,
    index: usize,
    focused: bool,
    pending: bool,
) -> Option<SourceControlAction> {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), theme::control::COMPACT),
        Sense::click(),
    );
    let response = ui.interact(
        rect,
        Id::new(("source_control_row", repository, &entry.path, group as u8)),
        Sense::click(),
    );
    let kind = group_kind(group, entry);
    let word = change_word(kind);
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled() && !pending,
            focused,
            format!("{word}, {}", entry.path.display()),
        )
    });
    let hovered = ui.rect_contains_pointer(rect);
    let fill = theme::state::fill(focused, focused, hovered, false);
    if fill != Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(rect, theme::corner(theme::radius::ROW), fill);
    }
    let color = if pending {
        theme::text_disabled()
    } else {
        theme::text().primary
    };
    let badge_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + theme::space::WIDE, rect.center().y),
        egui::Vec2::splat(icons::GRID),
    );
    ui.painter().text(
        badge_rect.center(),
        Align2::CENTER_CENTER,
        change_badge(kind),
        theme::typography::micro(),
        change_color(kind),
    );
    let actions_width = if hovered || focused { 72.0 } else { 0.0 };
    let text_rect = egui::Rect::from_min_max(
        egui::pos2(badge_rect.right() + theme::space::SMALL, rect.top()),
        egui::pos2(
            rect.right() - theme::space::SMALL - actions_width,
            rect.bottom(),
        ),
    );
    let name = entry
        .path
        .file_name()
        .unwrap_or(entry.path.as_os_str())
        .to_string_lossy();
    let parent = entry
        .path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| format!("  {}", parent.display()))
        .unwrap_or_default();
    let painter = ui.painter().with_clip_rect(text_rect);
    let name_galley = painter.layout_no_wrap(name.into_owned(), theme::typography::small(), color);
    painter.galley(
        egui::pos2(
            text_rect.left(),
            text_rect.center().y - name_galley.size().y * 0.5,
        ),
        name_galley.clone(),
        color,
    );
    if kind == ChangeKind::Deleted {
        painter.hline(
            text_rect.left()..=text_rect.left() + name_galley.size().x,
            text_rect.center().y,
            egui::Stroke::new(theme::stroke::DIVIDER, color),
        );
    }
    painter.text(
        egui::pos2(
            text_rect.left() + name_galley.size().x,
            text_rect.center().y,
        ),
        Align2::LEFT_CENTER,
        parent,
        theme::typography::small(),
        theme::text().muted,
    );
    let mut action = None;
    if !pending && (hovered || focused) {
        let specs = match group {
            SourceGroup::Merge => [
                Some((Icon::File, "Open file", 0_u8)),
                None,
                Some((Icon::Plus, "Mark resolved", 1)),
            ],
            SourceGroup::Staged => [
                Some((Icon::File, "Open file", 0)),
                None,
                Some((Icon::Minus, "Unstage", 2)),
            ],
            SourceGroup::Changes => [
                Some((Icon::File, "Open file", 0)),
                Some((Icon::Undo, "Discard…", 3)),
                Some((Icon::Plus, "Stage", 1)),
            ],
        };
        for (slot, spec) in specs.into_iter().flatten().enumerate() {
            let button = egui::Rect::from_center_size(
                egui::pos2(
                    rect.right()
                        - theme::space::SMALL
                        - theme::control::COMPACT * (specs.len() - slot) as f32
                        + theme::control::COMPACT * 0.5,
                    rect.center().y,
                ),
                egui::Vec2::splat(theme::control::COMPACT),
            );
            let hit = ui
                .interact(
                    button,
                    Id::new(("source_control_action", index, spec.2)),
                    Sense::click(),
                )
                .on_hover_text(spec.1);
            icons::paint_button(
                ui.painter(),
                spec.0,
                button,
                &hit,
                true,
                if spec.2 == 3 {
                    theme::semantic().danger
                } else {
                    theme::text().secondary
                },
            );
            if hit.clicked() {
                action = Some(match spec.2 {
                    0 => SourceControlAction::Open(repository.join(&entry.path)),
                    1 => SourceControlAction::Stage(
                        repository.to_path_buf(),
                        vec![entry.path.clone()],
                    ),
                    2 => SourceControlAction::Unstage(
                        repository.to_path_buf(),
                        vec![entry.path.clone()],
                    ),
                    _ => SourceControlAction::Discard(
                        repository.to_path_buf(),
                        vec![entry.path.clone()],
                    ),
                });
            }
        }
    }
    if action.is_none() && response.clicked() && !pending {
        action = Some(if matches!(group, SourceGroup::Merge) {
            SourceControlAction::Open(repository.join(&entry.path))
        } else {
            SourceControlAction::Diff(
                repository.to_path_buf(),
                entry.path.clone(),
                group_area(group),
            )
        });
    }
    action
}

fn group_area(group: SourceGroup) -> DiffArea {
    match group {
        SourceGroup::Staged => DiffArea::Staged,
        SourceGroup::Merge | SourceGroup::Changes => DiffArea::Worktree,
    }
}

fn group_kind(group: SourceGroup, entry: &GitEntry) -> ChangeKind {
    match group {
        SourceGroup::Merge => ChangeKind::Conflicted,
        SourceGroup::Staged => entry.index.unwrap_or(ChangeKind::Modified),
        SourceGroup::Changes => entry.worktree.unwrap_or(ChangeKind::Modified),
    }
}

fn is_conflicted(entry: &GitEntry) -> bool {
    entry.index == Some(ChangeKind::Conflicted) || entry.worktree == Some(ChangeKind::Conflicted)
}

fn change_badge(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Modified => "M",
        ChangeKind::Added => "A",
        ChangeKind::Deleted => "D",
        ChangeKind::Renamed => "R",
        ChangeKind::Untracked => "U",
        ChangeKind::Conflicted => "!",
        ChangeKind::TypeChange => "T",
    }
}

fn change_word(kind: ChangeKind) -> &'static str {
    match kind {
        ChangeKind::Modified => "Modified",
        ChangeKind::Added => "Added",
        ChangeKind::Deleted => "Deleted",
        ChangeKind::Renamed => "Renamed",
        ChangeKind::Untracked => "Untracked",
        ChangeKind::Conflicted => "Conflicted",
        ChangeKind::TypeChange => "Type changed",
    }
}

fn change_color(kind: ChangeKind) -> Color32 {
    match kind {
        ChangeKind::Modified => theme::semantic().warning,
        ChangeKind::Added | ChangeKind::Untracked => theme::semantic().success,
        ChangeKind::Deleted | ChangeKind::Conflicted => theme::semantic().danger,
        ChangeKind::Renamed | ChangeKind::TypeChange => theme::semantic().info,
    }
}
