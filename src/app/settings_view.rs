use super::*;

impl EditorApp {
    /// The Settings entry pinned to the bottom of the left rail — a gear and
    /// its label above a hairline — shared by the file tree and the agentic
    /// sessions list. Returns which of the row's actions was clicked: opening
    /// settings, or installing a detected update.
    pub(super) fn draw_settings_row(&self, ui: &mut egui::Ui, row: egui::Rect) -> (bool, bool) {
        let response = ui
            .interact(row, Id::new("settings_toggle"), Sense::click())
            .on_hover_text("Open Settings");
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Open Settings")
        });
        ui.painter().hline(
            row.x_range(),
            row.top() + 0.5,
            egui::Stroke::new(theme::stroke::DIVIDER, theme::border::hairline_color()),
        );
        let color = if response.hovered() {
            theme::text().primary
        } else {
            theme::text().secondary
        };
        let icon = egui::Rect::from_center_size(
            egui::pos2(
                row.left() + theme::space::MEDIUM + icons::GRID * 0.5,
                row.center().y,
            ),
            egui::Vec2::splat(icons::GRID),
        );
        icons::paint(ui.painter(), Icon::Gear, icon, color);
        ui.painter().text(
            egui::pos2(icon.right() + theme::space::SNUG, row.center().y),
            Align2::LEFT_CENTER,
            "Settings",
            theme::typography::small(),
            color,
        );
        let update = self
            .update_available
            .load(std::sync::atomic::Ordering::Relaxed)
            && self.draw_update_button(ui, row);
        (response.clicked(), update)
    }

    /// The circular accent button that appears on the settings row once a
    /// newer release is detected, opposite the gear.
    pub(super) fn draw_update_button(&self, ui: &mut egui::Ui, row: egui::Rect) -> bool {
        let button = egui::Rect::from_center_size(
            egui::pos2(
                row.right() - theme::space::MEDIUM - UPDATE_BUTTON_SIZE * 0.5,
                row.center().y,
            ),
            egui::Vec2::splat(UPDATE_BUTTON_SIZE),
        );
        let response = ui
            .interact(button, Id::new("settings_update"), Sense::click())
            .on_hover_text("Update Editur");
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Update Editur")
        });
        let fill = if response.hovered() {
            theme::composite(theme::state::hover(), theme::accent())
        } else {
            theme::accent()
        };
        ui.painter()
            .circle_filled(button.center(), UPDATE_BUTTON_SIZE * 0.5, fill);
        icons::paint(
            ui.painter(),
            Icon::Download,
            egui::Rect::from_center_size(button.center(), egui::Vec2::splat(icons::GRID * 0.75)),
            theme::text().on_accent,
        );
        response.clicked()
    }

    pub(super) fn draw_settings(&mut self, root: &mut egui::Ui, window: egui::Rect) {
        let rail_width = 270.0_f32.min((window.width() * 0.38).max(210.0));
        let rail = egui::Rect::from_min_max(
            window.left_top(),
            egui::pos2(window.left() + rail_width, window.bottom()),
        );
        let content = egui::Rect::from_min_max(
            egui::pos2(rail.right(), window.top()),
            window.right_bottom(),
        );
        root.painter()
            .rect_filled(rail, 0.0, theme::state::sidebar_material());
        root.painter()
            .rect_filled(content, 0.0, theme::state::secondary_material());
        root.painter().vline(
            rail.right(),
            rail.y_range(),
            egui::Stroke::new(1.0, theme::border::hairline_color()),
        );
        let mut actions = Vec::new();
        root.scope_builder(
            UiBuilder::new()
                .id_salt("settings_rail")
                .max_rect(rail.shrink2(egui::vec2(20.0, 18.0))),
            |ui| {
                ui.set_width(ui.available_width());
                let back = settings_quiet_button(ui, "settings_back", "Back to app", 16.0);
                icons::paint(
                    ui.painter(),
                    Icon::ChevronLeft,
                    egui::Rect::from_center_size(
                        egui::pos2(back.rect.left() + 11.0, back.rect.center().y),
                        egui::Vec2::splat(icons::GRID * 0.75),
                    ),
                    if back.hovered() {
                        theme::text().primary
                    } else {
                        theme::text().secondary
                    },
                );
                if back.clicked() {
                    actions.push(SettingsAction::Back);
                }
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Settings")
                        .size(theme::typography::TITLE_SIZE)
                        .strong()
                        .color(theme::text().primary),
                );
                ui.add_space(14.0);
                ui.add(
                    TextEdit::singleline(&mut self.settings_search)
                        .hint_text("Search settings…")
                        .margin(egui::Margin::symmetric(10, 7))
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(24.0);
                ui.label(
                    RichText::new("EDITOR")
                        .size(theme::typography::MICRO_SIZE)
                        .strong()
                        .color(theme::text().muted),
                );
                ui.add_space(8.0);
                ui.scope(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if settings_navigation_row(
                        ui,
                        "settings_appearance",
                        "Appearance",
                        self.settings_section == SettingsSection::Appearance,
                    )
                    .clicked()
                    {
                        self.settings_section = SettingsSection::Appearance;
                        self.settings_search.clear();
                    }
                    if settings_navigation_row(
                        ui,
                        "settings_keybindings",
                        "Keybindings",
                        self.settings_section == SettingsSection::Keybindings,
                    )
                    .clicked()
                    {
                        self.settings_section = SettingsSection::Keybindings;
                        self.settings_search.clear();
                    }
                    if settings_navigation_row(
                        ui,
                        "settings_language_servers",
                        "Language Servers",
                        self.settings_section == SettingsSection::LanguageServers,
                    )
                    .clicked()
                    {
                        self.settings_section = SettingsSection::LanguageServers;
                        self.settings_search.clear();
                    }
                    ui.add_space(20.0);
                    ui.label(
                        RichText::new("INTEGRATIONS")
                            .size(theme::typography::MICRO_SIZE)
                            .strong()
                            .color(theme::text().muted),
                    );
                    ui.add_space(8.0);
                    if settings_navigation_row(
                        ui,
                        "settings_devin",
                        "Devin",
                        self.settings_section == SettingsSection::Devin,
                    )
                    .clicked()
                    {
                        self.settings_section = SettingsSection::Devin;
                        self.settings_search.clear();
                        actions.push(SettingsAction::OpenDevin);
                    }
                });
            },
        );
        root.scope_builder(
            UiBuilder::new()
                .id_salt("settings_content")
                .max_rect(content),
            |ui| {
                ScrollArea::vertical()
                    .id_salt("settings_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        let width = ui.available_width().min(780.0);
                        ui.horizontal(|ui| {
                            ui.add_space(((ui.available_width() - width) * 0.5).max(20.0));
                            ui.vertical(|ui| {
                                ui.set_width(width.min(ui.available_width()));
                                ui.add_space(40.0);
                                if self.settings_section == SettingsSection::Keybindings {
                                    self.draw_keybinding_settings(ui);
                                    ui.add_space(34.0);
                                    return;
                                }
                                if self.settings_section == SettingsSection::Appearance {
                                    self.draw_appearance_settings(ui);
                                    ui.add_space(34.0);
                                    return;
                                }
                                if self.settings_section == SettingsSection::Devin {
                                    self.draw_devin_settings(ui, &mut actions);
                                    ui.add_space(34.0);
                                    return;
                                }
                                settings_page_header(
                                    ui,
                                    "Language Servers",
                                    "Diagnostics, completion, and navigation, one server per \
                                     language. Auto uses the bundled defaults.",
                                );
                                let query = self.settings_search.trim().to_ascii_lowercase();
                                let show_general = query.is_empty()
                                    || [
                                        "general",
                                        "enable language servers",
                                        "start only for supported open files",
                                    ]
                                    .iter()
                                    .any(|label| label.contains(&query));
                                let show_servers = lsp_catalog()
                                    .iter()
                                    .any(|preset| settings_preset_matches(preset, &query));
                                if show_general {
                                    settings_section_label(ui, "General");
                                    let enabled = self.settings.language_servers.enabled;
                                    settings_card(ui, |ui| {
                                        if settings_switch_row(
                                            ui,
                                            "Enable language servers",
                                            "Servers start only for supported open files.",
                                            enabled,
                                        )
                                        .clicked()
                                        {
                                            actions.push(SettingsAction::Enabled(!enabled));
                                        }
                                    });
                                }
                                if show_servers {
                                    if show_general {
                                        ui.add_space(theme::space::XWIDE);
                                    }
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            RichText::new("SERVERS")
                                                .size(theme::typography::MICRO_SIZE)
                                                .strong()
                                                .color(theme::text().muted),
                                        );
                                        ui.with_layout(
                                            Layout::right_to_left(Align::Center),
                                            |ui| {
                                                if settings_secondary_button(
                                                    ui,
                                                    "settings_rescan",
                                                    "Rescan",
                                                )
                                                .clicked()
                                                {
                                                    actions.push(SettingsAction::Rescan);
                                                }
                                            },
                                        );
                                    });
                                    ui.add_space(theme::space::SMALL);
                                    settings_card(ui, |ui| {
                                        self.draw_server_settings(ui, &mut actions);
                                    });
                                }
                                if !show_general && !show_servers {
                                    ui.label(
                                        RichText::new("No matching settings")
                                            .color(theme::text().muted),
                                    );
                                }
                                if let Some(error) = &self.settings_error {
                                    ui.add_space(12.0);
                                    ui.colored_label(
                                        theme::ink(theme::semantic().danger),
                                        format!("Settings were not changed: {error}"),
                                    );
                                }
                                ui.add_space(34.0);
                            });
                        });
                    });
            },
        );
        for action in actions {
            self.apply_settings_action(action, root.ctx());
        }
        self.draw_keybinding_dialogs(root.ctx());
        if self.lsp_sync_needed {
            root.ctx().request_repaint_after(Duration::from_millis(50));
        }
    }

    fn draw_devin_settings(&mut self, ui: &mut egui::Ui, actions: &mut Vec<SettingsAction>) {
        settings_page_header(
            ui,
            "Devin",
            "Connect once to use Devin sessions from Editur.",
        );
        settings_section_label(ui, "Connection");
        settings_card(ui, |ui| {
            if let Some(source) = self.devin_state.credential_source {
                let status = match self.devin_state.connection {
                    DevinConnectionState::Connecting => "Connecting",
                    DevinConnectionState::Connected => "Connected",
                    DevinConnectionState::AuthenticationRequired => "Sign-in required",
                    DevinConnectionState::Offline => "Offline",
                    DevinConnectionState::RateLimited => "Rate limited",
                    DevinConnectionState::Failed => "Connection failed",
                    DevinConnectionState::Disconnected => "Disconnected",
                };
                settings_row(ui, "Status", status, |ui| {
                    if source == CredentialSource::Keyring
                        && settings_danger_button(ui, "settings_devin_disconnect", "Disconnect")
                            .clicked()
                    {
                        actions.push(SettingsAction::DisconnectDevin);
                    }
                });
                if source == CredentialSource::Environment {
                    ui.separator();
                    settings_row(ui, "Credentials", "Managed by DEVIN_API_KEY", |_| {});
                }
                return;
            }

            ui.add_space(theme::space::SMALL);
            ui.label(RichText::new("API key").color(theme::text().secondary));
            ui.add(
                TextEdit::singleline(&mut self.devin_api_key)
                    .id(Id::new("devin_api_key"))
                    .password(true)
                    .hint_text("cog_…")
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(theme::space::MEDIUM);
            ui.label(RichText::new("Organization ID").color(theme::text().secondary));
            ui.add(
                TextEdit::singleline(&mut self.devin_org_id)
                    .id(Id::new("devin_org_id"))
                    .hint_text("Only when your key requires it")
                    .desired_width(f32::INFINITY),
            );
            if let Some(error) = self.devin_state.error.as_ref() {
                ui.add_space(theme::space::MEDIUM);
                ui.colored_label(theme::ink(theme::semantic().danger), &error.message);
            }
            ui.add_space(theme::space::LARGE);
            ui.add_enabled_ui(self.devin_api_key.trim().starts_with("cog_"), |ui| {
                if settings_primary_button(ui, "settings_devin_connect", "Connect").clicked() {
                    actions.push(SettingsAction::ConnectDevin);
                }
            });
            ui.add_space(theme::space::SMALL);
        });
    }

    pub(super) fn draw_appearance_settings(&mut self, ui: &mut egui::Ui) {
        settings_page_header(
            ui,
            "Appearance",
            "Theme, density, and the editor face. Changes apply immediately.",
        );
        let mut dirty = false;
        let appearance = &mut self.settings.appearance;

        settings_section_label(ui, "Interface");
        settings_card(ui, |ui| {
            dirty |= settings_choice_row(
                ui,
                "Theme",
                "Dark, light, or follow the system.",
                &mut appearance.theme,
                [
                    (ThemePreference::Dark, "Dark"),
                    (ThemePreference::Light, "Light"),
                    (ThemePreference::System, "System"),
                ],
            );
            ui.separator();
            dirty |= settings_choice_row(
                ui,
                "Density",
                "Comfortable is the default rhythm; Compact tightens every list.",
                &mut appearance.density,
                [
                    (DensityPreference::Comfortable, "Comfortable"),
                    (DensityPreference::Compact, "Compact"),
                ],
            );
            ui.separator();
            let dense_agent = settings_switch_row(
                ui,
                "Dense Agent",
                "Group each turn into compact, expandable work.",
                appearance.dense_agent,
            );
            if dense_agent.clicked() {
                appearance.dense_agent = !appearance.dense_agent;
                dirty = true;
            }
            ui.separator();
            settings_row(
                ui,
                "UI scale",
                "Scales text, icons, spacing, panes, and hit targets together.",
                |ui| {
                    let (_, changed) =
                        settings_ui_scale_slider(ui, &mut appearance.ui_scale_percent);
                    dirty |= changed;
                },
            );
            ui.separator();
            let reduced = settings_switch_row(
                ui,
                "Reduce motion",
                "Skip animated transitions. Every change still lands.",
                appearance.reduced_motion,
            );
            if reduced.clicked() {
                appearance.reduced_motion = !appearance.reduced_motion;
                dirty = true;
            }
        });

        ui.add_space(theme::space::XWIDE);
        settings_section_label(ui, "Editor type");
        settings_card(ui, |ui| {
            settings_row(
                ui,
                "Font size",
                "The editor's monospace size in pixels.",
                |ui| {
                    ui.spacing_mut().slider_width = 190.0;
                    let before = appearance.editor_font_size;
                    ui.add(
                        egui::Slider::new(&mut appearance.editor_font_size, 10.0..=24.0)
                            .step_by(1.0)
                            .suffix(" px"),
                    );
                    dirty |= (appearance.editor_font_size - before).abs() > f32::EPSILON;
                },
            );
            ui.separator();
            settings_row(
                ui,
                "Font family",
                "Any monospace family installed on this machine.",
                |ui| {
                    let mut family = appearance.editor_font_family.clone().unwrap_or_default();
                    let response = ui.add(
                        TextEdit::singleline(&mut family)
                            .hint_text("JetBrains Mono (bundled)")
                            .margin(egui::Margin::symmetric(10, 7))
                            .desired_width(250.0),
                    );
                    if response.changed() {
                        appearance.editor_font_family =
                            (!family.trim().is_empty()).then_some(family);
                        dirty = true;
                    }
                },
            );
            ui.separator();
            dirty |= settings_choice_row(
                ui,
                "Line height",
                "The editor's line box as a ratio of its font size.",
                &mut appearance.line_height,
                [
                    (LineHeightPreference::Compact, "Compact"),
                    (LineHeightPreference::Default, "Default"),
                    (LineHeightPreference::Comfortable, "Comfortable"),
                ],
            );
            ui.separator();
            dirty |= settings_choice_row(
                ui,
                "Line wrapping",
                "Wrap long lines to the editor width or scroll horizontally.",
                &mut appearance.line_wrap,
                [
                    (LineWrapPreference::NoWrap, "No wrap"),
                    (LineWrapPreference::Wrap, "Wrap"),
                ],
            );
        });

        if dirty {
            let ctx = ui.ctx().clone();
            if !self.persist_settings() {
                return;
            }
            self.apply_appearance(&ctx);
            ctx.request_repaint();
        }
    }

    pub(super) fn draw_keybinding_settings(&mut self, ui: &mut egui::Ui) {
        settings_page_header(
            ui,
            "Keybindings",
            "Profiles, chords, and Vim scopes. Changes save to the active profile immediately.",
        );
        let active = self.settings.keybindings.active_profile.clone();
        let behavior = self.settings.keybindings.active_behavior();
        let active_label = self.keybinding_profile_label(&active);
        let choices = self.keybinding_profile_choices();
        let custom_active = self.settings.keybindings.profiles.contains_key(&active);
        let mut action = None;
        settings_section_label(ui, "Profile");
        settings_card(ui, |ui| {
            settings_row(
                ui,
                "Active profile",
                "The keymap every window uses.",
                |ui| {
                    settings_combo_box(
                        ui,
                        "keybinding_profile",
                        230.0,
                        &format!("{active_label} · {}", behavior.label()),
                        |ui| {
                            for (id, label, behavior) in &choices {
                                if settings_combo_choice(
                                    ui,
                                    &format!("{label} · {}", behavior.label()),
                                    *id == active,
                                ) {
                                    action = Some(KeybindingUiAction::Activate(id.clone()));
                                }
                            }
                        },
                    );
                    ui.add_space(theme::space::TIGHT);
                    if settings_secondary_button(ui, "profile_duplicate", "Duplicate").clicked() {
                        action = Some(KeybindingUiAction::Duplicate);
                    }
                    ui.add_space(theme::space::TIGHT);
                    if settings_secondary_button(ui, "profile_new", "New profile").clicked() {
                        self.new_profile = Some(NewProfileDraft {
                            name: String::new(),
                            base: Some(BUILTIN_VSCODE.to_owned()),
                            behavior: KeybindingBehavior::Standard,
                        });
                    }
                    if !custom_active {
                        ui.add_space(theme::space::TIGHT);
                        if settings_secondary_button(ui, "profile_customize", "Customize").clicked()
                        {
                            action = Some(KeybindingUiAction::Customize);
                        }
                    }
                },
            );
            if custom_active {
                let name = self
                    .settings
                    .keybindings
                    .profiles
                    .get(&active)
                    .map(|profile| profile.name.clone())
                    .unwrap_or_default();
                let rename = self.rename_profile.get_or_insert(name);
                ui.separator();
                settings_row(ui, "Profile name", "Shown in the profile menu.", |ui| {
                    if settings_secondary_button(ui, "profile_rename", "Rename").clicked() {
                        action = Some(KeybindingUiAction::Rename(rename.clone()));
                    }
                    ui.add_space(theme::space::TIGHT);
                    ui.add(
                        TextEdit::singleline(rename)
                            .hint_text("Profile name")
                            .margin(egui::Margin::symmetric(10, 7))
                            .desired_width(180.0),
                    );
                });
                ui.separator();
                settings_row(
                    ui,
                    "Maintenance",
                    "Reset every deviation, or delete this profile and return to VS Code.",
                    |ui| {
                        if settings_danger_button(ui, "profile_delete", "Delete → VS Code")
                            .clicked()
                        {
                            action = Some(KeybindingUiAction::DeleteToVsCode);
                        }
                        ui.add_space(theme::space::TIGHT);
                        if self.confirm_profile_reset {
                            if settings_danger_button(
                                ui,
                                "profile_reset_confirm",
                                "Confirm reset all",
                            )
                            .clicked()
                            {
                                self.confirm_profile_reset = false;
                                action = Some(KeybindingUiAction::ResetAll);
                            }
                            ui.add_space(theme::space::TIGHT);
                            if settings_secondary_button(ui, "profile_reset_cancel", "Cancel")
                                .clicked()
                            {
                                self.confirm_profile_reset = false;
                            }
                        } else if settings_secondary_button(
                            ui,
                            "profile_reset",
                            "Reset all deviations",
                        )
                        .clicked()
                        {
                            self.confirm_profile_reset = true;
                        }
                    },
                );
            }
        });
        if !custom_active {
            self.rename_profile = None;
        }
        ui.add_space(theme::space::XWIDE);
        settings_section_label(ui, "Commands");
        ui.horizontal(|ui| {
            ui.add(
                TextEdit::singleline(&mut self.settings_search)
                    .hint_text("Search commands or keys…")
                    .margin(egui::Margin::symmetric(10, 7))
                    .desired_width(220.0),
            );
            ui.add_space(theme::space::TIGHT);
            for (filter, label) in [
                (KeybindingFilter::All, "All"),
                (KeybindingFilter::Bound, "Bound"),
                (KeybindingFilter::Unbound, "Unbound"),
                (KeybindingFilter::Modified, "Modified"),
            ] {
                if segment(ui, label, self.keybinding_filter == filter, None).clicked() {
                    self.keybinding_filter = filter;
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if behavior == KeybindingBehavior::Vim {
                    settings_combo_box(
                        ui,
                        "keybinding_vim_scope",
                        140.0,
                        self.keybinding_vim_scope
                            .map_or("All Vim modes", Scope::label),
                        |ui| {
                            if settings_combo_choice(
                                ui,
                                "All Vim modes",
                                self.keybinding_vim_scope.is_none(),
                            ) {
                                self.keybinding_vim_scope = None;
                            }
                            for scope in [
                                Scope::VimNormal,
                                Scope::VimInsert,
                                Scope::VimReplace,
                                Scope::VimVisual,
                                Scope::VimOperator,
                            ] {
                                if settings_combo_choice(
                                    ui,
                                    scope.label(),
                                    self.keybinding_vim_scope == Some(scope),
                                ) {
                                    self.keybinding_vim_scope = Some(scope);
                                }
                            }
                        },
                    );
                    ui.add_space(theme::space::TIGHT);
                } else {
                    self.keybinding_vim_scope = None;
                }
                let category_label = self
                    .keybinding_category
                    .clone()
                    .unwrap_or_else(|| "All categories".to_owned());
                settings_combo_box(ui, "keybinding_category", 150.0, &category_label, |ui| {
                    if settings_combo_choice(
                        ui,
                        "All categories",
                        self.keybinding_category.is_none(),
                    ) {
                        self.keybinding_category = None;
                    }
                    let mut categories = KEYBINDING_CATALOG
                        .iter()
                        .map(|info| info.category)
                        .collect::<Vec<_>>();
                    categories.sort_unstable();
                    categories.dedup();
                    for category in categories {
                        if settings_combo_choice(
                            ui,
                            category,
                            self.keybinding_category.as_deref() == Some(category),
                        ) {
                            self.keybinding_category = Some(category.to_owned());
                        }
                    }
                });
            });
        });
        ui.add_space(theme::space::MEDIUM);
        let effective = self
            .settings
            .keybindings
            .effective_bindings()
            .unwrap_or_default();
        let query = self.settings_search.trim().to_lowercase();
        let removed = self
            .settings
            .keybindings
            .profiles
            .get(&active)
            .map_or(&[][..], |profile| profile.removed.as_slice());
        let base_bindings = self
            .settings
            .keybindings
            .profiles
            .get(&active)
            .and_then(|profile| profile.base.as_deref())
            .and_then(crate::keybindings::builtin_bindings)
            .unwrap_or_default();
        let mut shown = 0;
        let mut current_category = None;
        let mut category_open = false;
        for info in KEYBINDING_CATALOG {
            if behavior == KeybindingBehavior::Standard && info.category == "Vim" {
                continue;
            }
            if self
                .keybinding_category
                .as_deref()
                .is_some_and(|category| category != info.category)
            {
                continue;
            }
            if self
                .keybinding_vim_scope
                .is_some_and(|scope| !info.scopes.contains(&scope))
            {
                continue;
            }
            let bindings = effective
                .iter()
                .filter(|binding| {
                    binding.rule.command == info.id
                        && self
                            .keybinding_vim_scope
                            .is_none_or(|scope| binding.rule.scope == scope)
                })
                .collect::<Vec<_>>();
            let removed_for_command = base_bindings
                .iter()
                .any(|binding| binding.rule.command == info.id && removed.contains(&binding.id));
            let modified = removed_for_command
                || bindings
                    .iter()
                    .any(|binding| binding.source == BindingSource::Custom);
            if matches!(
                (self.keybinding_filter, bindings.is_empty(), modified),
                (KeybindingFilter::Bound, true, _)
                    | (KeybindingFilter::Unbound, false, _)
                    | (KeybindingFilter::Modified, _, false)
            ) {
                continue;
            }
            let rendered = bindings
                .iter()
                .map(|binding| {
                    binding.rule.label(
                        binding
                            .rule
                            .platform
                            .unwrap_or_else(KeybindingPlatform::current),
                    )
                })
                .collect::<Vec<_>>()
                .join(" ");
            if !query.is_empty()
                && ![
                    info.label.to_lowercase(),
                    info.id.to_lowercase(),
                    info.category.to_lowercase(),
                    rendered.to_lowercase(),
                ]
                .iter()
                .any(|value| value.contains(&query))
                && !bindings
                    .iter()
                    .any(|binding| binding.rule.scope.label().to_lowercase().contains(&query))
            {
                continue;
            }
            if current_category != Some(info.category) {
                if category_open {
                    ui.add_space(theme::space::XWIDE);
                }
                current_category = Some(info.category);
                category_open = true;
                settings_section_label(ui, info.category);
            } else if shown > 0 {
                ui.add_space(theme::space::SMALL);
            }
            shown += 1;
            egui::Frame::new()
                .fill(settings_card_fill())
                .stroke(egui::Stroke::new(1.0, theme::border::hairline_color()))
                .corner_radius(theme::radius::CARD)
                .inner_margin(egui::Margin::symmetric(16, 12))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            ui.set_min_width(220.0);
                            ui.label(
                                RichText::new(info.label)
                                    .strong()
                                    .color(theme::text().primary),
                            );
                            ui.label(
                                RichText::new(info.id)
                                    .monospace()
                                    .size(theme::typography::MICRO_SIZE)
                                    .color(theme::text().muted),
                            );
                        });
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if settings_secondary_button(
                                ui,
                                ("add_binding", info.id),
                                "Add binding",
                            )
                            .clicked()
                            {
                                self.shortcut_recorder = Some(ShortcutRecorder {
                                    command: info.command,
                                    strokes: Vec::new(),
                                    logical_keys: Vec::new(),
                                    physical_keys: Vec::new(),
                                    scope: info.scopes[0],
                                    platform: None,
                                    replace_index: None,
                                    disable_id: None,
                                    error: None,
                                    can_replace: false,
                                });
                            }
                            if modified {
                                ui.add_space(theme::space::TIGHT);
                                if settings_secondary_button(
                                    ui,
                                    ("reset_command", info.id),
                                    "Reset command",
                                )
                                .clicked()
                                {
                                    action = Some(KeybindingUiAction::ResetCommand(info.command));
                                }
                            }
                        });
                    });
                    if bindings.is_empty() {
                        ui.label(RichText::new("Unbound").color(theme::text().muted));
                    }
                    for binding in &bindings {
                        ui.separator();
                        ui.horizontal(|ui| {
                            chip(
                                ui,
                                &binding.rule.label(
                                    binding
                                        .rule
                                        .platform
                                        .unwrap_or_else(KeybindingPlatform::current),
                                ),
                            );
                            ui.add_space(theme::space::TIGHT);
                            ui.label(
                                RichText::new(format!(
                                    "{} · {} · {}",
                                    binding.rule.platform.map_or_else(
                                        || "All platforms".to_owned(),
                                        |platform| platform.to_string(),
                                    ),
                                    binding.rule.scope.label(),
                                    match binding.source {
                                        BindingSource::BuiltIn => "Built-in",
                                        BindingSource::Custom => "Custom",
                                    },
                                ))
                                .size(theme::typography::MICRO_SIZE)
                                .color(theme::text().muted),
                            );
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if binding.source == BindingSource::Custom {
                                    if settings_secondary_button(
                                        ui,
                                        ("binding_remove", binding.id.as_str()),
                                        "Remove",
                                    )
                                    .clicked()
                                        && let Some(index) = binding
                                            .id
                                            .strip_prefix("custom-")
                                            .and_then(|index| index.parse().ok())
                                    {
                                        action = Some(KeybindingUiAction::Remove(index));
                                    }
                                    ui.add_space(theme::space::TIGHT);
                                } else if custom_active {
                                    if settings_secondary_button(
                                        ui,
                                        ("binding_disable", binding.id.as_str()),
                                        "Disable",
                                    )
                                    .clicked()
                                    {
                                        action =
                                            Some(KeybindingUiAction::Disable(binding.id.clone()));
                                    }
                                    ui.add_space(theme::space::TIGHT);
                                }
                                if settings_secondary_button(
                                    ui,
                                    ("binding_change", binding.id.as_str()),
                                    "Change",
                                )
                                .clicked()
                                {
                                    self.shortcut_recorder = Some(ShortcutRecorder {
                                        command: info.command,
                                        strokes: binding.rule.sequence.clone(),
                                        logical_keys: binding
                                            .rule
                                            .sequence
                                            .iter()
                                            .map(|stroke| stroke.key.clone())
                                            .collect(),
                                        physical_keys: vec![None; binding.rule.sequence.len()],
                                        scope: binding.rule.scope,
                                        platform: binding.rule.platform,
                                        replace_index: binding
                                            .id
                                            .strip_prefix("custom-")
                                            .and_then(|index| index.parse().ok()),
                                        disable_id: (binding.source == BindingSource::BuiltIn)
                                            .then(|| binding.id.clone()),
                                        error: None,
                                        can_replace: false,
                                    });
                                }
                            });
                        });
                    }
                });
        }
        if shown == 0 {
            ui.label(RichText::new("No matching commands").color(theme::text().muted));
        }
        if let Some(error) = &self.settings_error {
            ui.add_space(12.0);
            ui.colored_label(
                theme::ink(theme::semantic().danger),
                format!("Settings were not changed: {error}"),
            );
        }
        if let Some(action) = action {
            self.apply_keybinding_ui_action(action, ui.ctx());
        }
    }

    pub(super) fn keybinding_profile_choices(&self) -> Vec<(String, String, KeybindingBehavior)> {
        let mut profiles = vec![
            (
                BUILTIN_VSCODE.to_owned(),
                "VS Code (Built-in)".to_owned(),
                KeybindingBehavior::Standard,
            ),
            (
                BUILTIN_VIM.to_owned(),
                "Vim (Built-in)".to_owned(),
                KeybindingBehavior::Vim,
            ),
        ];
        let mut custom = self
            .settings
            .keybindings
            .profiles
            .iter()
            .map(|(id, profile)| (id.clone(), profile.name.clone(), profile.behavior))
            .collect::<Vec<_>>();
        custom.sort_by_key(|profile| profile.1.to_lowercase());
        profiles.extend(custom);
        profiles
    }

    pub(super) fn keybinding_profile_label(&self, id: &str) -> String {
        match id {
            BUILTIN_VSCODE => "VS Code (Built-in)".into(),
            BUILTIN_VIM => "Vim (Built-in)".into(),
            _ => self
                .settings
                .keybindings
                .profiles
                .get(id)
                .map_or_else(|| id.to_owned(), |profile| profile.name.clone()),
        }
    }

    pub(super) fn apply_keybinding_ui_action(
        &mut self,
        action: KeybindingUiAction,
        ctx: &egui::Context,
    ) {
        let old = self.settings.clone();
        let active = self.settings.keybindings.active_profile.clone();
        let result = match action {
            KeybindingUiAction::Activate(id) => self.settings.keybindings.set_active(&id),
            KeybindingUiAction::Customize => self
                .settings
                .keybindings
                .derive_profile(&active)
                .map(|_| ()),
            KeybindingUiAction::Duplicate => self
                .settings
                .keybindings
                .duplicate_profile(&active)
                .map(|_| ()),
            KeybindingUiAction::ResetAll => self.settings.keybindings.reset_all(&active),
            KeybindingUiAction::DeleteToVsCode => self
                .settings
                .keybindings
                .delete_profile(&active, BUILTIN_VSCODE),
            KeybindingUiAction::Rename(name) => {
                self.settings.keybindings.rename_profile(&active, &name)
            }
            KeybindingUiAction::Disable(id) => {
                self.settings.keybindings.disable_binding(&active, &id)
            }
            KeybindingUiAction::Remove(index) => {
                self.settings.keybindings.remove_binding(&active, index)
            }
            KeybindingUiAction::ResetCommand(command) => {
                self.settings.keybindings.reset_command(&active, command)
            }
        };
        if let Err(error) = result {
            self.settings = old;
            self.settings_error = Some(error);
            return;
        }
        self.commit_keybinding_settings(old, ctx);
    }

    pub(super) fn commit_keybinding_settings(
        &mut self,
        old: Settings,
        ctx: &egui::Context,
    ) -> bool {
        let profile_changed =
            old.keybindings.active_profile != self.settings.keybindings.active_profile;
        if !self.persist_settings() {
            self.settings = old;
            return false;
        }
        if let Err(error) = self.rebuild_keybinding_resolver() {
            self.settings = old;
            self.settings_error = Some(error);
            return false;
        }
        if profile_changed {
            self.confirm_profile_reset = false;
            self.keybinding_vim_scope = None;
            let behavior = self.settings.keybindings.active_behavior();
            for tab in &mut self.tabs {
                if behavior == KeybindingBehavior::Vim {
                    tab.vim = VimState::default();
                    let cursor = tab.editor_surface.cursor();
                    tab.editor_surface.set_selection(cursor, cursor);
                } else {
                    tab.vim.execute(
                        KeybindingCommand::VimNormal,
                        &mut tab.editor_surface,
                        &mut tab.buffer.text,
                        &mut self.vim_session,
                    );
                    tab.vim = VimState::default();
                }
            }
            self.vim_overlay = None;
            self.focus_editor = self.active_tab.is_some();
            ctx.request_repaint();
        }
        true
    }

    pub(super) fn draw_keybinding_dialogs(&mut self, ctx: &egui::Context) {
        self.draw_new_profile_dialog(ctx);
        self.draw_shortcut_recorder(ctx);
    }

    pub(super) fn draw_new_profile_dialog(&mut self, ctx: &egui::Context) {
        let Some(draft) = &mut self.new_profile else {
            return;
        };
        let empty = draft.name.trim().is_empty();
        let outcome = Dialog::new("new_keybinding_profile", "New keybinding profile")
            .primary("Create")
            .primary_enabled(!empty)
            .show_with(ctx, |ui| {
                ui.add_space(theme::space::TIGHT);
                ui.label(
                    RichText::new("Name")
                        .font(theme::typography::small())
                        .color(theme::text().muted),
                );
                ui.add(TextEdit::singleline(&mut draft.name).desired_width(320.0));
                ui.label(
                    RichText::new("Start from")
                        .font(theme::typography::small())
                        .color(theme::text().muted),
                );
                egui::ComboBox::from_id_salt("new_profile_base")
                    .selected_text(match draft.base.as_deref() {
                        Some(BUILTIN_VSCODE) => "VS Code",
                        Some(BUILTIN_VIM) => "Vim",
                        _ => "Empty",
                    })
                    .show_ui(ui, |ui| {
                        if ui
                            .selectable_label(
                                draft.base.as_deref() == Some(BUILTIN_VSCODE),
                                "VS Code",
                            )
                            .clicked()
                        {
                            draft.base = Some(BUILTIN_VSCODE.to_owned());
                            draft.behavior = KeybindingBehavior::Standard;
                        }
                        if ui
                            .selectable_label(draft.base.as_deref() == Some(BUILTIN_VIM), "Vim")
                            .clicked()
                        {
                            draft.base = Some(BUILTIN_VIM.to_owned());
                            draft.behavior = KeybindingBehavior::Vim;
                        }
                        if ui.selectable_label(draft.base.is_none(), "Empty").clicked() {
                            draft.base = None;
                        }
                    });
                if draft.base.is_none() {
                    ui.horizontal(|ui| {
                        ui.label("Editing behavior");
                        ui.selectable_value(
                            &mut draft.behavior,
                            KeybindingBehavior::Standard,
                            "Standard",
                        );
                        ui.selectable_value(
                            &mut draft.behavior,
                            KeybindingBehavior::Vim,
                            "Vim modal",
                        );
                    });
                }
            });
        if matches!(outcome, Outcome::Cancel | Outcome::Dismissed) {
            self.new_profile = None;
        } else if outcome == Outcome::Primary && !empty {
            let draft = self.new_profile.take().expect("draft exists");
            let old = self.settings.clone();
            match self.settings.keybindings.create_profile(
                &draft.name,
                draft.base.as_deref(),
                draft.behavior,
            ) {
                Ok(id) => {
                    self.settings.keybindings.active_profile = id;
                    self.commit_keybinding_settings(old, ctx);
                }
                Err(error) => {
                    self.settings = old;
                    self.settings_error = Some(error);
                }
            }
        }
    }

    pub(super) fn draw_shortcut_recorder(&mut self, ctx: &egui::Context) {
        let Some(recorder) = &mut self.shortcut_recorder else {
            return;
        };
        let events = ctx.input(|input| input.events.clone());
        let mut captured = HashSet::new();
        for (index, event) in events.iter().enumerate() {
            let (key, physical_key, modifiers) = match event {
                egui::Event::Copy => (Key::C, Some(Key::C), primary_modifiers()),
                egui::Event::Cut => (Key::X, Some(Key::X), primary_modifiers()),
                egui::Event::Paste(_) => (Key::V, Some(Key::V), primary_modifiers()),
                egui::Event::Key {
                    key: Key::Escape,
                    pressed: true,
                    ..
                } => {
                    self.shortcut_recorder = None;
                    return;
                }
                egui::Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    modifiers,
                    ..
                } => (*key, *physical_key, *modifiers),
                _ => continue,
            };
            if recorder.strokes.len() == 4 {
                break;
            }
            let platform = KeybindingPlatform::current();
            let primary = modifiers.command;
            recorder.strokes.push(BindingStroke {
                key: key.name().to_owned(),
                primary,
                ctrl: modifiers.ctrl && !(primary && platform != KeybindingPlatform::Macos),
                alt: modifiers.alt,
                shift: modifiers.shift,
                super_key: modifiers.mac_cmd && !(primary && platform == KeybindingPlatform::Macos),
                physical: false,
            });
            recorder.logical_keys.push(key.name().to_owned());
            recorder
                .physical_keys
                .push(physical_key.map(|key| key.name().to_owned()));
            recorder.error = None;
            recorder.can_replace = false;
            captured.insert(index);
        }
        if !captured.is_empty() {
            ctx.input_mut(|input| {
                input.events = input
                    .events
                    .drain(..)
                    .enumerate()
                    .filter_map(|(index, event)| (!captured.contains(&index)).then_some(event))
                    .collect();
            });
        }
        let mut replace = false;
        let recorded = !recorder.strokes.is_empty();
        // The recorder is itself a key-capture field, so it cannot hand Enter
        // and Esc to the buttons; it consumes Esc above.
        let outcome = Dialog::new("shortcut_recorder", "Record shortcut")
            .primary("Done")
            .primary_enabled(recorded)
            .neutral("Clear")
            .without_keyboard()
            .show_with(ctx, |ui| {
                ui.add_space(theme::space::TIGHT);
                ui.label(
                    RichText::new(recorder.command.info().label)
                        .font(theme::typography::strong())
                        .color(theme::text().primary),
                );
                ui.horizontal_wrapped(|ui| {
                    if recorder.strokes.is_empty() {
                        ui.label(RichText::new("Press up to four keys").color(theme::text().muted));
                    }
                    for stroke in &recorder.strokes {
                        chip(ui, &stroke.label(KeybindingPlatform::current()));
                    }
                });
                if let Some(last) = recorder.strokes.last_mut() {
                    let response = ui.checkbox(
                        &mut last.physical,
                        "Use physical key position for last stroke",
                    );
                    if response.changed() {
                        if last.physical {
                            if let Some(physical) =
                                recorder.physical_keys.last().and_then(Option::as_ref)
                            {
                                last.key.clone_from(physical);
                            } else {
                                last.physical = false;
                                recorder.error =
                                    Some("This input did not report a physical key".into());
                                recorder.can_replace = false;
                            }
                        } else if let Some(logical) = recorder.logical_keys.last() {
                            last.key.clone_from(logical);
                        }
                    }
                }
                ui.horizontal(|ui| {
                    ui.label("Scope");
                    egui::ComboBox::from_id_salt("recorder_scope")
                        .selected_text(recorder.scope.label())
                        .show_ui(ui, |ui| {
                            for scope in recorder.command.info().scopes {
                                ui.selectable_value(&mut recorder.scope, *scope, scope.label());
                            }
                        });
                    ui.label("Platform");
                    egui::ComboBox::from_id_salt("recorder_platform")
                        .selected_text(
                            recorder
                                .platform
                                .map_or_else(|| "All".to_owned(), |platform| platform.to_string()),
                        )
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut recorder.platform, None, "All");
                            ui.selectable_value(
                                &mut recorder.platform,
                                Some(KeybindingPlatform::Macos),
                                "macOS",
                            );
                            ui.selectable_value(
                                &mut recorder.platform,
                                Some(KeybindingPlatform::Windows),
                                "Windows",
                            );
                            ui.selectable_value(
                                &mut recorder.platform,
                                Some(KeybindingPlatform::Linux),
                                "Linux",
                            );
                        });
                });
                if recorder.scope == Scope::Global
                    && recorder.strokes.iter().any(|stroke| {
                        (stroke.ctrl
                            && matches!(
                                stroke.parsed_key(),
                                Some(Key::C | Key::D | Key::Q | Key::S | Key::W)
                            ))
                            || (stroke.ctrl
                                && stroke.alt
                                && stroke.parsed_key().is_some_and(|key| {
                                    key_character(key, egui::Modifiers::NONE).is_some()
                                }))
                    })
                {
                    ui.colored_label(
                        theme::semantic().warning,
                        "This global binding may capture terminal or AltGr input.",
                    );
                }
                if let Some(error) = &recorder.error {
                    ui.colored_label(theme::ink(theme::semantic().danger), error);
                    if recorder.can_replace {
                        replace = ui.button("Replace existing").clicked();
                    }
                }
            });
        if matches!(outcome, Outcome::Cancel | Outcome::Dismissed) {
            self.shortcut_recorder = None;
        } else if outcome == Outcome::Neutral {
            if let Some(recorder) = &mut self.shortcut_recorder {
                recorder.strokes.clear();
                recorder.logical_keys.clear();
                recorder.physical_keys.clear();
                recorder.error = None;
                recorder.can_replace = false;
            }
        } else if outcome == Outcome::Primary || replace {
            self.finish_shortcut_recording(replace, ctx);
        }
    }

    pub(super) fn finish_shortcut_recording(
        &mut self,
        replace_conflict: bool,
        ctx: &egui::Context,
    ) {
        let Some(mut recorder) = self.shortcut_recorder.take() else {
            return;
        };
        let rule = BindingRule {
            sequence: recorder.strokes.clone(),
            command: recorder.command.id().to_owned(),
            scope: recorder.scope,
            platform: recorder.platform,
        };
        if let Err(error) = crate::keybindings::validate_rule(&rule) {
            recorder.error = Some(error);
            recorder.can_replace = false;
            self.shortcut_recorder = Some(recorder);
            return;
        }
        let old = self.settings.clone();
        let active = self.settings.keybindings.active_profile.clone();
        if !self.settings.keybindings.profiles.contains_key(&active) {
            match self.settings.keybindings.derive_profile(&active) {
                Ok(_) => {}
                Err(error) => {
                    recorder.error = Some(error);
                    self.settings = old;
                    self.shortcut_recorder = Some(recorder);
                    return;
                }
            }
        }
        let active = self.settings.keybindings.active_profile.clone();
        let conflict = self
            .settings
            .keybindings
            .effective_bindings()
            .unwrap_or_default()
            .into_iter()
            .find(|binding| {
                rules_have_sequence_conflict(&binding.rule, &rule)
                    && recorder
                        .replace_index
                        .is_none_or(|index| binding.id != format!("custom-{index}"))
                    && recorder.disable_id.as_deref() != Some(binding.id.as_str())
            });
        if let Some(conflict) = &conflict
            && !replace_conflict
        {
            self.settings = old;
            recorder.error = Some(format!(
                "{} already uses this shortcut in {}",
                KeybindingCommand::from_id(&conflict.rule.command)
                    .map_or(conflict.rule.command.as_str(), |command| command
                        .info()
                        .label),
                conflict.rule.scope.label(),
            ));
            recorder.can_replace = true;
            self.shortcut_recorder = Some(recorder);
            return;
        }
        let result = (|| {
            let mut remove = recorder.replace_index.into_iter().collect::<Vec<_>>();
            let mut disable = recorder
                .disable_id
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>();
            if replace_conflict && let Some(conflict) = &conflict {
                if conflict.source == BindingSource::BuiltIn {
                    disable.push(&conflict.id);
                } else if let Some(index) = conflict
                    .id
                    .strip_prefix("custom-")
                    .and_then(|index| index.parse().ok())
                {
                    remove.push(index);
                }
            }
            remove.sort_unstable();
            remove.dedup();
            for index in remove.into_iter().rev() {
                self.settings.keybindings.remove_binding(&active, index)?;
            }
            disable.sort_unstable();
            disable.dedup();
            for id in disable {
                self.settings.keybindings.disable_binding(&active, id)?;
            }
            self.settings.keybindings.add_binding(&active, rule, true)
        })();
        match result {
            Ok(()) => {
                self.commit_keybinding_settings(old, ctx);
            }
            Err(error) => {
                self.settings = old;
                recorder.error = Some(error);
                recorder.can_replace = false;
                self.shortcut_recorder = Some(recorder);
            }
        }
    }

    pub(super) fn draw_server_settings(
        &mut self,
        ui: &mut egui::Ui,
        actions: &mut Vec<SettingsAction>,
    ) {
        let query = self.settings_search.trim().to_ascii_lowercase();
        let presets = lsp_catalog()
            .iter()
            .filter(|preset| settings_preset_matches(preset, &query))
            .copied()
            .collect::<Vec<_>>();
        if presets.is_empty() {
            ui.label(RichText::new("No matching language servers").color(theme::text().muted));
            return;
        }
        for (row, preset) in presets.into_iter().enumerate() {
            if row > 0 {
                ui.separator();
            }
            let mut mode = self.server_mode(preset.id);
            let (_, row_rect) = ui.allocate_space(egui::vec2(ui.available_width(), 64.0));
            ui.painter().text(
                egui::pos2(row_rect.left(), row_rect.center().y - 11.0),
                Align2::LEFT_CENTER,
                preset.language,
                theme::typography::body(),
                theme::text().primary,
            );
            ui.painter().text(
                egui::pos2(row_rect.left(), row_rect.center().y + 8.0),
                Align2::LEFT_CENTER,
                preset.name,
                theme::typography::micro(),
                theme::text().muted,
            );
            let controls = egui::Rect::from_min_max(
                egui::pos2(row_rect.center().x, row_rect.top()),
                row_rect.right_bottom(),
            );
            ui.scope_builder(UiBuilder::new().max_rect(controls), |ui| {
                ui.set_width(ui.available_width());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    settings_mode_combo(ui, preset.id, &mut mode);
                    ui.add_space(12.0);
                    ui.label(
                        RichText::new(self.server_status_label(preset.id))
                            .size(theme::typography::MICRO_SIZE)
                            .color(theme::text().secondary),
                    );
                });
            });
            if mode != self.server_mode(preset.id) {
                actions.push(SettingsAction::Mode(preset.id, mode));
            }
            if mode == ServerMode::Custom {
                let default = || {
                    self.settings
                        .language_servers
                        .servers
                        .get(preset.id.as_str())
                        .and_then(|override_| {
                            override_
                                .command
                                .clone()
                                .map(|command| (command, override_.args.join("\n")))
                        })
                        .unwrap_or_else(|| (preset.command.to_owned(), preset.args.join("\n")))
                };
                let draft = self
                    .settings_drafts
                    .entry(preset.id)
                    .or_insert_with(default);
                ui.add_space(2.0);
                ui.label(
                    RichText::new("Executable")
                        .small()
                        .color(theme::text().secondary),
                );
                ui.add(
                    TextEdit::singleline(&mut draft.0)
                        .hint_text("Executable path or command")
                        .margin(egui::Margin::symmetric(9, 7))
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(10.0);
                ui.label(
                    RichText::new("Arguments (one per line)")
                        .small()
                        .color(theme::text().secondary),
                );
                ui.add(
                    TextEdit::multiline(&mut draft.1)
                        .hint_text("One argument per line")
                        .margin(egui::Margin::symmetric(9, 7))
                        .desired_rows(3)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if settings_primary_button(
                        ui,
                        ("settings_apply", preset.id.as_str()),
                        "Apply and restart",
                    )
                    .clicked()
                    {
                        actions.push(SettingsAction::Apply(
                            preset.id,
                            draft.0.clone(),
                            draft.1.clone(),
                        ));
                    }
                    if settings_quiet_button(
                        ui,
                        ("settings_reset", preset.id.as_str()),
                        "Reset to auto",
                        0.0,
                    )
                    .clicked()
                    {
                        actions.push(SettingsAction::Reset(preset.id));
                    }
                });
                ui.add_space(16.0);
            }
            let detail = match self.lsp_status.get(&preset.id) {
                Some(ServerStatus::Failed(error)) => Some(
                    self.lsp_detail
                        .get(&preset.id)
                        .map_or_else(|| error.clone(), |detail| format!("{error}\n{detail}")),
                ),
                _ => self.lsp_detail.get(&preset.id).cloned(),
            };
            if let Some(detail) = detail {
                ui.add_space(5.0);
                ui.colored_label(
                    theme::ink(theme::semantic().danger),
                    truncate_lines(&detail, 3),
                );
            }
        }
    }

    pub(super) fn server_mode(&self, preset: PresetId) -> ServerMode {
        self.settings
            .language_servers
            .servers
            .get(preset.as_str())
            .map_or(ServerMode::Auto, |override_| override_.mode)
    }

    pub(super) fn server_status_label(&self, preset: PresetId) -> &'static str {
        match self.lsp_status.get(&preset) {
            Some(ServerStatus::Starting) => "Starting",
            Some(ServerStatus::Ready(_)) => "Ready",
            Some(ServerStatus::NotFound) => "Not found",
            Some(ServerStatus::Failed(_)) => "Failed",
            Some(ServerStatus::Stopped) => "Stopped",
            Some(ServerStatus::NotStarted) | None => "Not started",
        }
    }

    pub(super) fn apply_settings_action(&mut self, action: SettingsAction, ctx: &egui::Context) {
        match action {
            SettingsAction::Back => {
                self.settings_open = false;
                if !self.devin_sidebar {
                    self.send_devin(DevinCommand::SetVisible(false));
                }
            }
            SettingsAction::OpenDevin => {
                self.ensure_devin_controller(ctx);
                self.send_devin(DevinCommand::SetVisible(true));
            }
            SettingsAction::ConnectDevin => {
                self.ensure_devin_controller(ctx);
                self.send_devin(DevinCommand::SaveCredentials {
                    api_key: self.devin_api_key.clone(),
                    org_id: (!self.devin_org_id.trim().is_empty())
                        .then(|| self.devin_org_id.trim().to_owned()),
                });
            }
            SettingsAction::DisconnectDevin => self.devin_confirm_disconnect = true,
            SettingsAction::Enabled(enabled) => {
                let old = self.settings.clone();
                self.settings.language_servers.enabled = enabled;
                if !self.persist_settings() {
                    self.settings = old;
                    return;
                }
                self.lsp_sync_needed = true;
                let presets = self.lsp_controllers.keys().copied().collect::<Vec<_>>();
                for preset in presets {
                    self.restart_lsp(preset);
                }
            }
            SettingsAction::Mode(preset, mode) => {
                let old = self.settings.clone();
                match mode {
                    ServerMode::Auto => {
                        self.settings
                            .language_servers
                            .servers
                            .remove(preset.as_str());
                        self.settings_drafts.remove(&preset);
                    }
                    ServerMode::Off => {
                        self.settings.language_servers.servers.insert(
                            preset.as_str().into(),
                            ServerOverride {
                                mode,
                                command: None,
                                args: Vec::new(),
                            },
                        );
                    }
                    ServerMode::Custom => {
                        let descriptor = lsp_catalog()
                            .iter()
                            .find(|candidate| candidate.id == preset)
                            .unwrap();
                        let command = descriptor.command.to_owned();
                        let args = descriptor
                            .args
                            .iter()
                            .map(|arg| (*arg).to_owned())
                            .collect::<Vec<_>>();
                        self.settings_drafts
                            .insert(preset, (command.clone(), args.join("\n")));
                        self.settings.language_servers.servers.insert(
                            preset.as_str().into(),
                            ServerOverride {
                                mode,
                                command: Some(command),
                                args,
                            },
                        );
                    }
                }
                if !self.persist_settings() {
                    self.settings = old;
                    return;
                }
                self.lsp_sync_needed = true;
                self.restart_lsp(preset);
            }
            SettingsAction::Apply(preset, command, args) => {
                let old = self.settings.clone();
                self.settings.language_servers.servers.insert(
                    preset.as_str().into(),
                    ServerOverride {
                        mode: ServerMode::Custom,
                        command: Some(command),
                        args: args.lines().map(str::to_owned).collect(),
                    },
                );
                if !self.persist_settings() {
                    self.settings = old;
                    return;
                }
                self.lsp_sync_needed = true;
                self.restart_lsp(preset);
            }
            SettingsAction::Reset(preset) => {
                let old = self.settings.clone();
                self.settings
                    .language_servers
                    .servers
                    .remove(preset.as_str());
                self.settings_drafts.remove(&preset);
                if !self.persist_settings() {
                    self.settings = old;
                    return;
                }
                self.lsp_sync_needed = true;
                self.restart_lsp(preset);
            }
            SettingsAction::Rescan => {
                for preset in lsp_catalog() {
                    self.ensure_lsp_controller(preset.id, ctx);
                    self.send_lsp_control(preset.id, LspCommand::Rescan);
                }
            }
        }
    }

    pub(super) fn persist_settings(&mut self) -> bool {
        self.settings.appearance = self.settings.appearance.clone().normalized();
        let result = data_dir()
            .and_then(|directory| settings::save(&directory.join("settings.json"), &self.settings));
        match result {
            Ok(()) => {
                self.settings_error = None;
                true
            }
            Err(error) => {
                self.settings_error = Some(error);
                false
            }
        }
    }

    pub(super) fn change_ui_scale(&mut self, increase: bool, ctx: &egui::Context) {
        let current = self.settings.appearance.ui_scale_percent;
        let next = if increase {
            current
                .saturating_add(UI_SCALE_STEP_PERCENT)
                .min(UI_SCALE_MAX_PERCENT)
        } else {
            current
                .saturating_sub(UI_SCALE_STEP_PERCENT)
                .max(UI_SCALE_MIN_PERCENT)
        };
        if next == current {
            return;
        }
        self.settings.appearance.ui_scale_percent = next;
        self.persist_settings();
        self.apply_appearance(ctx);
        ctx.request_repaint();
    }

    /// Pushes the Appearance settings into the live token layer so a change is
    /// visible on the next frame without a restart.
    pub(super) fn apply_appearance(&self, ctx: &egui::Context) {
        let appearance = &self.settings.appearance;
        let light = match appearance.theme {
            ThemePreference::Light => true,
            ThemePreference::Dark => false,
            ThemePreference::System => ctx
                .system_theme()
                .map(|theme| theme == egui::Theme::Light)
                .unwrap_or(false),
        };
        theme::set_light(light);
        theme::set_density(match appearance.density {
            DensityPreference::Comfortable => theme::Density::Comfortable,
            DensityPreference::Compact => theme::Density::Compact,
        });
        theme::typography::set_code_metrics(
            appearance.editor_font_size,
            appearance.line_height.ratio(),
        );
        theme::typography::set_code_family(appearance.editor_font_family.clone());
        theme::motion::set_reduced(ctx, appearance.reduced_motion);
        ctx.options_mut(|options| options.zoom_with_keyboard = false);
        ctx.set_zoom_factor(f32::from(appearance.ui_scale_percent) / 100.0);
        theme::apply(ctx);
    }
}
