use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::devin::{Activity, DevinMessage, SessionSummary};

const DEVIN_WEB: &str = "https://app.devin.ai/";

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(super) enum DevinView {
    #[default]
    Sessions,
    Create,
    Detail,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) enum DevinScope {
    #[default]
    Active,
    All,
    Archived,
}

#[derive(Clone, Debug)]
pub(super) struct PendingDevinMessage {
    text: String,
    attachment_ids: Vec<String>,
    failed: bool,
    confirmed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DevinLifecycle {
    Sleep,
    Archive,
    Unarchive,
    Terminate,
}

enum DevinUiAction {
    Close,
    Back,
    NewSession,
    Connect,
    ConfirmDisconnect,
    Reconnect,
    Refresh,
    LoadMoreSessions,
    Select(String),
    Create,
    SendMessage,
    RetryMessage,
    LoadMoreMessages,
    LoadMoreEvents,
    Lifecycle(DevinLifecycle),
    ConfirmTerminate,
    ToggleAttachment(String),
    ToggleActivity(String),
}

impl EditorApp {
    pub(super) fn draw_devin_sidebar(&mut self, ui: &mut egui::Ui) {
        // The same text roles the Agent sidebar installs, so both panels
        // resolve default labels, buttons, and inputs to identical faces.
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Body, theme::typography::title());
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Small, theme::typography::small());
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Button, theme::typography::body());
        let rect = ui.max_rect();
        ui.painter()
            .rect_filled(rect, 0.0, theme::state::secondary_material());
        let header = rect.with_max_y((rect.top() + TITLEBAR_HEIGHT).min(rect.bottom()));
        let body = rect.with_min_y(header.bottom());
        let mut action = None;

        ui.painter()
            .rect_filled(header, 0.0, theme::surface().chrome);
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("devin_header")
                .max_rect(header.shrink2(egui::vec2(theme::space::MEDIUM, 0.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| self.draw_devin_header(ui, &mut action),
        );
        ui.painter().hline(
            header.x_range(),
            header.bottom() - 0.5,
            egui::Stroke::new(1.0, theme::border::hairline_color()),
        );
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("devin_body", self.devin_view))
                .max_rect(body.shrink2(egui::vec2(theme::space::LARGE, theme::space::MEDIUM))),
            |ui| {
                ui.set_width(ui.available_width());
                if self.devin_needs_connect_card() {
                    ScrollArea::vertical()
                        .id_salt("devin_connect_scroll")
                        .show(ui, |ui| {
                            self.draw_devin_connect(ui, &mut action);
                        });
                    return;
                }
                self.draw_devin_connection_banner(ui, &mut action);
                match self.devin_view {
                    DevinView::Sessions => self.draw_devin_sessions(ui, &mut action),
                    DevinView::Create => self.draw_devin_create(ui, &mut action),
                    DevinView::Detail => self.draw_devin_detail(ui, &mut action),
                }
            },
        );

        if self.devin_view != DevinView::Sessions
            && !devin_text_field_focused(ui.ctx())
            && ui.input(|input| input.key_pressed(Key::Escape))
        {
            action = Some(DevinUiAction::Back);
        }
        if let Some(action) = action {
            self.apply_devin_ui_action(action, ui.ctx());
        }
    }

    fn draw_devin_header(&mut self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        let connected_surface = !self.devin_needs_connect_card();
        let header_title = |text: &str| {
            RichText::new(text)
                .font(theme::typography::body())
                .color(theme::text().primary)
        };
        match self.devin_view {
            DevinView::Sessions => ui.label(header_title("Devin")),
            DevinView::Create => {
                if icons::button_sized(
                    ui,
                    Icon::ChevronLeft,
                    "Back to Devin sessions",
                    theme::text().secondary,
                    egui::Vec2::splat(theme::control::STANDARD),
                )
                .clicked()
                {
                    *action = Some(DevinUiAction::Back);
                }
                ui.label(header_title("New session"))
            }
            DevinView::Detail => {
                if icons::button_sized(
                    ui,
                    Icon::ChevronLeft,
                    "Back to Devin sessions",
                    theme::text().secondary,
                    egui::Vec2::splat(theme::control::STANDARD),
                )
                .clicked()
                {
                    *action = Some(DevinUiAction::Back);
                }
                let title = self
                    .devin_state
                    .detail
                    .as_ref()
                    .map(|detail| detail.summary.title.as_str())
                    .or_else(|| {
                        self.selected_devin_summary()
                            .map(|summary| summary.title.as_str())
                    })
                    .unwrap_or("Session");
                ui.add_sized(
                    [
                        (ui.available_width() - 80.0).max(48.0),
                        theme::control::STANDARD,
                    ],
                    Label::new(header_title(title)).truncate(),
                )
            }
        };

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if close_icon_button(ui)
                .on_hover_text("Close Devin sidebar")
                .clicked()
            {
                *action = Some(DevinUiAction::Close);
            }
            match self.devin_view {
                DevinView::Sessions if connected_surface => {
                    let menu = ui.menu_button("⋯", |ui| {
                        ui.hyperlink_to("Open Devin web app", DEVIN_WEB);
                        if let Some(source) = self.devin_state.credential_source {
                            ui.separator();
                            ui.label(match source {
                                CredentialSource::Environment => "Using environment credentials",
                                CredentialSource::Keyring => "Using operating-system credentials",
                            });
                            if source == CredentialSource::Keyring
                                && ui.button("Disconnect…").clicked()
                            {
                                *action = Some(DevinUiAction::ConfirmDisconnect);
                            }
                        }
                    });
                    menu.response.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Button,
                            ui.is_enabled(),
                            "More Devin actions",
                        )
                    });
                    if icons::button_sized(
                        ui,
                        Icon::Plus,
                        "New Devin session",
                        theme::text().secondary,
                        egui::Vec2::splat(theme::control::STANDARD),
                    )
                    .clicked()
                    {
                        *action = Some(DevinUiAction::NewSession);
                    }
                    if icons::button_sized(
                        ui,
                        Icon::Refresh,
                        "Refresh Devin sessions",
                        theme::text().secondary,
                        egui::Vec2::splat(theme::control::STANDARD),
                    )
                    .clicked()
                    {
                        *action = Some(DevinUiAction::Refresh);
                    }
                }
                DevinView::Detail if connected_surface => {
                    self.draw_devin_lifecycle_menu(ui, action)
                }
                DevinView::Sessions | DevinView::Detail | DevinView::Create => {}
            }
        });
    }

    fn draw_devin_lifecycle_menu(&self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        let summary = self.selected_devin_summary();
        let archived = summary.is_some_and(|summary| summary.archived);
        let url = summary
            .and_then(|summary| summary.url.as_deref())
            .unwrap_or(DEVIN_WEB);
        let menu = ui.menu_button("⋯", |ui| {
            ui.hyperlink_to("Open in Devin web app", url);
            ui.separator();
            ui.add_enabled_ui(!self.devin_state.busy, |ui| {
                if ui.button("Sleep").clicked() {
                    *action = Some(DevinUiAction::Lifecycle(DevinLifecycle::Sleep));
                }
                let label = if archived { "Unarchive" } else { "Archive" };
                if ui.button(label).clicked() {
                    *action = Some(DevinUiAction::Lifecycle(if archived {
                        DevinLifecycle::Unarchive
                    } else {
                        DevinLifecycle::Archive
                    }));
                }
                ui.separator();
                if ui
                    .button(RichText::new("Terminate…").color(theme::semantic().danger))
                    .clicked()
                {
                    *action = Some(DevinUiAction::ConfirmTerminate);
                }
            });
        });
        menu.response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                "Devin session actions",
            )
        });
    }

    fn devin_needs_connect_card(&self) -> bool {
        if self.devin_state.credential_source == Some(CredentialSource::Environment) {
            return false;
        }
        (self.devin_state.credential_source.is_none() && self.devin_state.sessions.is_empty())
            || self.devin_state.connection == DevinConnectionState::Disconnected
            || (self.devin_state.connection == DevinConnectionState::AuthenticationRequired
                && self.devin_state.sessions.is_empty())
    }

    fn draw_devin_connect(&mut self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        ui.add_space(theme::space::WIDE);
        let field_label = |text: &str| {
            RichText::new(text)
                .font(theme::typography::small_strong())
                .color(theme::text().secondary)
        };
        egui::Frame::new()
            .fill(theme::surface().raised)
            .stroke(egui::Stroke::new(1.0, theme::border::hairline_color()))
            .inner_margin(egui::Margin::same(14))
            .corner_radius(7)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(
                    RichText::new("Connect to Devin")
                        .size(theme::typography::TITLE_SIZE)
                        .strong()
                        .color(theme::text().primary),
                );
                ui.add_space(3.0);
                ui.add(
                    Label::new(
                        RichText::new("Use a personal access token or service-user key. It is saved only in your operating-system credential store.")
                            .font(theme::typography::body())
                            .color(theme::text().muted),
                    )
                    .wrap(),
                );
                ui.add_space(10.0);
                ui.label(field_label("API key"));
                ui.add(
                    TextEdit::singleline(&mut self.devin_api_key)
                        .id(Id::new("devin_api_key"))
                        .password(true)
                        .hint_text("cog_…")
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(theme::space::SNUG);
                ui.label(field_label("Organization ID (when required)"));
                ui.add(
                    TextEdit::singleline(&mut self.devin_org_id)
                        .id(Id::new("devin_org_id"))
                        .hint_text("Optional")
                        .desired_width(f32::INFINITY),
                );
                if let Some(error) = self.devin_state.error.as_ref() {
                    ui.add_space(theme::space::SNUG);
                    devin_callout(ui, &error.message, theme::semantic().danger);
                }
                let connecting = self.devin_state.connection == DevinConnectionState::Connecting;
                let enabled = !connecting && self.devin_api_key.trim().starts_with("cog_");
                if connecting {
                    ui.add_space(theme::space::SNUG);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Validating credentials…")
                                .font(theme::typography::small())
                                .color(theme::text().muted),
                        );
                    });
                }
                ui.add_space(10.0);
                let (fill, color) = agent_send_button_colors(enabled);
                let button = egui::Button::new(RichText::new("Connect").strong().color(color))
                    .fill(fill)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(5)
                    .min_size(egui::vec2(ui.available_width(), 30.0));
                if ui.add_enabled(enabled, button).clicked() {
                    *action = Some(DevinUiAction::Connect);
                }
                ui.add_space(theme::space::SMALL);
                ui.hyperlink_to(
                    RichText::new("Create a Devin personal access token")
                        .font(theme::typography::small()),
                    "https://docs.devin.ai/api-reference/personal-access-tokens",
                );
            });
        ui.add_space(theme::space::MEDIUM);
        ui.add(
            Label::new(
                RichText::new("Developer builds may instead set DEVIN_API_KEY and DEVIN_ORG_ID before launching Editur.")
                    .font(theme::typography::small())
                    .color(theme::text().muted),
            )
            .wrap(),
        );
    }

    fn draw_devin_connection_banner(&self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        match self.devin_state.connection {
            DevinConnectionState::AuthenticationRequired => {
                let callout = theme::callout(theme::semantic().warning);
                egui::Frame::new()
                    .fill(callout.fill)
                    .stroke(egui::Stroke::new(1.0, callout.border))
                    .inner_margin(egui::Margin::symmetric(
                        theme::space::MEDIUM as i8,
                        theme::space::SMALL as i8,
                    ))
                    .corner_radius(theme::corner(theme::radius::CONTROL))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.add(
                            Label::new(
                                RichText::new("Devin connection lost. Fetched data may be stale.")
                                    .font(theme::typography::body())
                                    .color(callout.text),
                            )
                            .wrap(),
                        );
                        ui.add_space(theme::space::TIGHT);
                        if self.devin_state.credential_source == Some(CredentialSource::Environment)
                        {
                            if ui.button("Retry").clicked() {
                                *action = Some(DevinUiAction::Refresh);
                            }
                        } else if ui.button("Reconnect").clicked() {
                            *action = Some(DevinUiAction::Reconnect);
                        }
                    });
                ui.add_space(theme::space::SMALL);
            }
            DevinConnectionState::Connecting => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(
                        RichText::new("Refreshing Devin…")
                            .font(theme::typography::small())
                            .color(theme::text().muted),
                    );
                });
            }
            DevinConnectionState::Failed => {
                if let Some(error) = self.devin_state.error.as_ref() {
                    devin_callout(ui, &error.message, theme::semantic().danger);
                    ui.add_space(theme::space::SMALL);
                }
            }
            _ => {}
        }
    }

    fn draw_devin_sessions(&mut self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        ui.horizontal(|ui| {
            let compact = ui.available_width() < 360.0;
            ui.add(
                TextEdit::singleline(&mut self.devin_filter)
                    .id(Id::new("devin_filter"))
                    .hint_text("Filter sessions…")
                    .desired_width(
                        (ui.available_width() - if compact { 82.0 } else { 126.0 }).max(80.0),
                    ),
            );
            if compact {
                ui.menu_button(devin_scope_label(self.devin_scope), |ui| {
                    for (scope, label) in devin_scopes() {
                        if ui
                            .selectable_label(self.devin_scope == scope, label)
                            .clicked()
                        {
                            self.devin_scope = scope;
                            self.devin_list_cursor = 0;
                            self.devin_focus_list = true;
                        }
                    }
                });
            } else {
                for (scope, label) in devin_scopes() {
                    if segment(ui, label, self.devin_scope == scope, None).clicked() {
                        self.devin_scope = scope;
                        self.devin_list_cursor = 0;
                        self.devin_focus_list = true;
                    }
                }
            }
        });
        ui.add_space(theme::space::SMALL);

        let ids = visible_session_ids(
            &self.devin_state.sessions,
            self.devin_scope,
            &self.devin_filter,
        )
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
        self.devin_list_cursor = self.devin_list_cursor.min(ids.len().saturating_sub(1));
        let mut row_ids = Vec::new();
        let mut focused_row = None;
        let list_height = (ui.available_height() - 34.0).max(80.0);
        ScrollArea::vertical()
            .id_salt("devin_sessions_scroll")
            .max_height(list_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if ids.is_empty() {
                    ui.vertical_centered(|ui| {
                        ui.add_space(theme::space::WIDE);
                        let loading = self.devin_state.last_sessions_refresh.is_none()
                            && matches!(
                                self.devin_state.connection,
                                DevinConnectionState::Connected | DevinConnectionState::Connecting
                            );
                        if loading {
                            ui.spinner();
                            ui.label(
                                RichText::new("Loading Devin sessions…")
                                    .font(theme::typography::small())
                                    .color(theme::text().muted),
                            );
                        } else {
                            ui.label(
                                RichText::new(match self.devin_state.connection {
                                    DevinConnectionState::Offline => {
                                        "Devin is offline; sessions will appear after a retry."
                                    }
                                    DevinConnectionState::RateLimited => {
                                        "Session loading is paused by Devin's rate limit."
                                    }
                                    _ if self.devin_scope == DevinScope::Archived => {
                                        "No archived Devin sessions."
                                    }
                                    _ => "No Devin sessions match this view.",
                                })
                                .font(theme::typography::body())
                                .color(theme::text().muted),
                            );
                            ui.add_space(theme::space::SMALL);
                        }
                        if !loading
                            && matches!(
                                self.devin_state.connection,
                                DevinConnectionState::Connected
                            )
                            && self.devin_scope != DevinScope::Archived
                            && ui.button("New session").clicked()
                        {
                            *action = Some(DevinUiAction::NewSession);
                        }
                    });
                    return;
                }
                let mut previous_group = None;
                for (index, session_id) in ids.iter().enumerate() {
                    let Some(session) = self
                        .devin_state
                        .sessions
                        .iter()
                        .find(|session| &session.id == session_id)
                    else {
                        continue;
                    };
                    let group = session_group(session);
                    if previous_group != Some(group.0) {
                        if previous_group.is_some() {
                            ui.add_space(theme::space::SNUG);
                        }
                        ui.label(
                            RichText::new(group.1)
                                .size(theme::typography::MICRO_SIZE)
                                .strong()
                                .color(theme::text().muted),
                        );
                        ui.add_space(theme::space::HAIR);
                        previous_group = Some(group.0);
                    }
                    let selected =
                        self.devin_state.selected_session.as_deref() == Some(session.id.as_str());
                    let title = session_title(session);
                    let relative = session
                        .updated_at
                        .as_deref()
                        .or(session.created_at.as_deref())
                        .map(compact_relative_time)
                        .unwrap_or_default();
                    let status = semantic_status(session);
                    let accessible = format!(
                        "{title}, {status}, {}, {}",
                        session
                            .repository
                            .as_deref()
                            .unwrap_or("repository unknown"),
                        if relative.is_empty() {
                            "time unknown"
                        } else {
                            &relative
                        }
                    );
                    let response = ui
                        .push_id(("devin_session", &session.id), |ui| {
                            selectable_content_row(ui, selected, 50.0, |ui| {
                                ui.horizontal(|ui| {
                                    let color = devin_status_dot_color(ui, session.category);
                                    let (dot, hover) = ui.allocate_exact_size(
                                        egui::Vec2::splat(14.0),
                                        Sense::hover(),
                                    );
                                    ui.painter().circle_filled(dot.center(), 4.0, color);
                                    hover.on_hover_text(
                                        session.status_detail.as_deref().unwrap_or(&session.status),
                                    );
                                    ui.vertical(|ui| {
                                        let row_title = RichText::new(title)
                                            .font(theme::typography::strong())
                                            .color(theme::text().primary);
                                        if session.pull_request_count > 0 {
                                            ui.with_layout(
                                                Layout::right_to_left(Align::Center),
                                                |ui| {
                                                    chip(ui, "PR");
                                                    ui.add(Label::new(row_title).truncate());
                                                },
                                            );
                                        } else {
                                            ui.add(Label::new(row_title).truncate());
                                        }
                                        ui.add(
                                            Label::new(
                                                RichText::new(format!(
                                                    "{}{}{}{}",
                                                    status,
                                                    if relative.is_empty() {
                                                        String::new()
                                                    } else {
                                                        format!(" · {relative}")
                                                    },
                                                    session
                                                        .origin
                                                        .as_deref()
                                                        .map(|origin| format!(
                                                            " · {}",
                                                            origin_label(origin)
                                                        ))
                                                        .unwrap_or_default(),
                                                    session
                                                        .repository
                                                        .as_deref()
                                                        .map(|repository| format!(
                                                            " · {repository}"
                                                        ))
                                                        .unwrap_or_default(),
                                                ))
                                                .small()
                                                .color(theme::text().muted),
                                            )
                                            .truncate(),
                                        );
                                    });
                                });
                            })
                        })
                        .inner;
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::WidgetType::SelectableLabel,
                            ui.is_enabled(),
                            selected,
                            &accessible,
                        )
                    });
                    if response.has_focus() {
                        focused_row = Some(index);
                    }
                    if response.clicked()
                        || (response.has_focus() && ui.input(|input| input.key_pressed(Key::Enter)))
                    {
                        *action = Some(DevinUiAction::Select(session.id.clone()));
                    }
                    row_ids.push(response.id);
                }
                if self.devin_state.sessions_cursor.is_some()
                    && ui.button("Load more sessions").clicked()
                {
                    *action = Some(DevinUiAction::LoadMoreSessions);
                }
            });

        if self.devin_focus_list {
            if let Some(id) = row_ids.get(self.devin_list_cursor).copied() {
                ui.memory_mut(|memory| memory.request_focus(id));
            } else {
                ui.memory_mut(|memory| memory.request_focus(Id::new("devin_filter")));
            }
            self.devin_focus_list = false;
        }
        if let Some(index) = focused_row {
            let movement = ui.input(|input| {
                i32::from(input.key_pressed(Key::ArrowDown))
                    - i32::from(input.key_pressed(Key::ArrowUp))
            });
            if movement != 0 && !row_ids.is_empty() {
                self.devin_list_cursor = if movement > 0 {
                    (index + 1).min(row_ids.len() - 1)
                } else {
                    index.saturating_sub(1)
                };
                ui.memory_mut(|memory| memory.request_focus(row_ids[self.devin_list_cursor]));
            }
            if let Some(text) = ui.input(|input| {
                input.events.iter().find_map(|event| match event {
                    egui::Event::Text(text) if !text.chars().all(char::is_control) => {
                        Some(text.clone())
                    }
                    _ => None,
                })
            }) {
                self.devin_filter.push_str(&text);
                ui.memory_mut(|memory| memory.request_focus(Id::new("devin_filter")));
            }
        }
        ui.add_space(theme::space::TIGHT);
        ui.label(
            RichText::new(devin_freshness(&self.devin_state))
                .size(theme::typography::MICRO_SIZE)
                .color(theme::text().muted),
        );
    }

    fn draw_devin_create(&mut self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        let field_label = |text: &str| {
            RichText::new(text)
                .font(theme::typography::small_strong())
                .color(theme::text().secondary)
        };
        ScrollArea::vertical()
            .id_salt("devin_create_scroll")
            .show(ui, |ui| {
                ui.label(field_label("Repository"));
                let repository = ui.add_enabled(
                    !self.devin_state.busy,
                    TextEdit::singleline(&mut self.devin_repository)
                        .id(Id::new("devin_repository"))
                        .hint_text("owner/repository")
                        .desired_width(f32::INFINITY),
                );
                if matches!(self.devin_state.repository, RepositoryState::SelectionRequired) {
                    ui.label(
                        RichText::new("The current Git origin could not be mapped; choose a repository explicitly.")
                            .font(theme::typography::small())
                            .color(theme::ink(theme::semantic().warning)),
                    );
                }
                ui.add_space(theme::space::SNUG);
                ui.label(field_label("Prompt"));
                let prompt = ui.add_enabled(
                    !self.devin_state.busy,
                    TextEdit::multiline(&mut self.devin_create_prompt)
                        .id(Id::new("devin_create_prompt"))
                        .hint_text("Describe the task for Devin…")
                        .desired_rows(8)
                        .desired_width(f32::INFINITY),
                );
                if self.devin_focus_create {
                    if self.devin_repository.is_empty() {
                        repository.request_focus();
                    } else {
                        prompt.request_focus();
                    }
                    self.devin_focus_create = false;
                }
                ui.add_space(theme::space::TIGHT);
                ui.add(
                    Label::new(
                        RichText::new(self.devin_state.workspace_boundary.as_deref().unwrap_or(
                            "Devin works from the remote repository. Local uncommitted or unpushed changes are not visible to it.",
                        ))
                            .font(theme::typography::small())
                            .color(theme::text().muted),
                    )
                    .wrap(),
                );
                if let Some(error) = self.devin_state.error.as_ref() {
                    ui.add_space(theme::space::SNUG);
                    devin_callout(ui, &error.message, theme::semantic().danger);
                }
                let can_create = !self.devin_state.busy
                    && !self.devin_repository.trim().is_empty()
                    && !self.devin_create_prompt.trim().is_empty();
                ui.add_space(theme::space::SMALL);
                ui.horizontal(|ui| {
                    let (fill, color) = agent_send_button_colors(can_create);
                    let create = egui::Button::new(
                        RichText::new("Create session").strong().color(color),
                    )
                    .fill(fill)
                    .stroke(egui::Stroke::NONE)
                    .corner_radius(5)
                    .min_size(egui::vec2(0.0, 30.0));
                    if ui.add_enabled(can_create, create).clicked() {
                        *action = Some(DevinUiAction::Create);
                    }
                    if ui.button("Cancel").clicked() {
                        *action = Some(DevinUiAction::Back);
                    }
                    if self.devin_state.busy && self.devin_creating {
                        ui.spinner();
                        ui.label(
                            RichText::new("Creating session…")
                                .font(theme::typography::small())
                                .color(theme::text().muted),
                        );
                    }
                });
            });
    }

    fn draw_devin_detail(&mut self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        let Some(summary) = self.selected_devin_summary().cloned() else {
            ui.label(
                RichText::new("This Devin session is no longer available.")
                    .font(theme::typography::body())
                    .color(theme::text().muted),
            );
            if ui.button("Back to sessions").clicked() {
                *action = Some(DevinUiAction::Back);
            }
            return;
        };
        if self.devin_state.busy && self.devin_state.detail.is_none() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(
                    RichText::new("Loading session…")
                        .font(theme::typography::small())
                        .color(theme::text().muted),
                );
            });
        }
        let status = semantic_status(&summary);
        let status_response = chip(ui, status)
            .on_hover_text(summary.status_detail.as_deref().unwrap_or(&summary.status));
        status_response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Label,
                true,
                format!(
                    "Status: {status}; {}",
                    summary.status_detail.as_deref().unwrap_or(&summary.status)
                ),
            )
        });
        if let Some(status_detail) = summary.status_detail.as_deref() {
            ui.label(
                RichText::new(status_detail)
                    .size(theme::typography::MICRO_SIZE)
                    .color(theme::text().muted),
            );
        }
        ui.horizontal_wrapped(|ui| {
            if let Some(repository) = summary.repository.as_deref() {
                ui.label(RichText::new(repository).small().color(theme::text().muted));
            }
            if let Some(origin) = summary.origin.as_deref() {
                ui.label(
                    RichText::new(format!("· {}", origin_label(origin)))
                        .small()
                        .color(theme::text().muted),
                );
            }
            if let Some(created) = summary.created_at.as_deref() {
                ui.label(
                    RichText::new(format!("· started {}", compact_relative_time(created)))
                        .small()
                        .color(theme::text().muted),
                );
            }
            if let Some(usage) = self
                .devin_state
                .detail
                .as_ref()
                .and_then(|detail| detail.usage.as_ref())
                && let Some(used) = usage.acus
            {
                ui.label(
                    RichText::new(usage.limit.map_or_else(
                        || format!("· {used:.2} ACUs"),
                        |limit| format!("· {used:.2} / {limit:.2} ACUs"),
                    ))
                    .small()
                    .color(theme::text().muted),
                );
            }
        });
        ui.horizontal_wrapped(|ui| {
            if let Some(parent) = summary.parent_session_id.as_deref()
                && ui.small_button("Open parent session").clicked()
            {
                *action = Some(DevinUiAction::Select(parent.into()));
            }
            if let Some(detail) = self.devin_state.detail.as_ref() {
                for child in &detail.children {
                    if ui
                        .small_button(format!("Child: {} · {}", child.title, child.status))
                        .clicked()
                    {
                        *action = Some(DevinUiAction::Select(child.id.clone()));
                    }
                }
            }
        });
        if let Some(detail) = self.devin_state.detail.as_ref()
            && !detail.pull_requests.is_empty()
        {
            ui.add_space(theme::space::SNUG);
            egui::Frame::new()
                .fill(theme::surface().raised)
                .stroke(egui::Stroke::new(1.0, theme::border::hairline_color()))
                .corner_radius(theme::corner(theme::radius::CARD))
                .inner_margin(theme::space::MEDIUM as i8)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new("PULL REQUESTS")
                            .size(theme::typography::MICRO_SIZE)
                            .strong()
                            .color(theme::text().muted),
                    );
                    ui.add_space(theme::space::TIGHT);
                    for pull_request in &detail.pull_requests {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.hyperlink_to(
                                RichText::new("Open ↗").font(theme::typography::small()),
                                &pull_request.url,
                            );
                            if let Some(status) = pull_request.status.as_deref() {
                                chip(ui, status);
                            }
                            ui.add(
                                Label::new(
                                    RichText::new(&pull_request.title)
                                        .font(theme::typography::strong())
                                        .color(theme::text().primary),
                                )
                                .truncate(),
                            );
                        });
                    }
                });
        }
        ui.separator();
        let terminal = summary.archived
            || matches!(
                summary.category,
                StatusCategory::Completed | StatusCategory::Failed
            );
        let composer_height = if terminal { 56.0 } else { 150.0 };
        let stream_height = (ui.available_height() - composer_height).max(100.0);
        ScrollArea::vertical()
            .id_salt(("devin_stream", &summary.id))
            .stick_to_bottom(true)
            .max_height(stream_height)
            .auto_shrink([false, false])
            .show(ui, |ui| self.draw_devin_stream(ui, action));
        if self.devin_state.messages_cursor.is_some()
            && ui.small_button("Load earlier messages").clicked()
        {
            *action = Some(DevinUiAction::LoadMoreMessages);
        }
        if self.devin_state.activity_cursor.is_some()
            && ui.small_button("Load earlier activity").clicked()
        {
            *action = Some(DevinUiAction::LoadMoreEvents);
        }
        if terminal {
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    RichText::new(if summary.archived {
                        "Messaging is unavailable while this session is archived."
                    } else {
                        "Messaging is unavailable because this session has ended."
                    })
                    .small()
                    .color(theme::text().muted),
                );
                if summary.archived && ui.button("Unarchive").clicked() {
                    *action = Some(DevinUiAction::Lifecycle(DevinLifecycle::Unarchive));
                }
            });
            return;
        }
        if summary.category == StatusCategory::Waiting {
            devin_callout(
                ui,
                "Devin is waiting for your reply",
                theme::semantic().warning,
            );
        }
        if let Some(detail) = self.devin_state.detail.as_ref()
            && !detail.attachments.is_empty()
        {
            ui.horizontal_wrapped(|ui| {
                for attachment in &detail.attachments {
                    let selected = self.devin_attachment_ids.contains(&attachment.id);
                    if ui
                        .selectable_label(
                            selected,
                            RichText::new(&attachment.name).font(theme::typography::small()),
                        )
                        .clicked()
                    {
                        *action = Some(DevinUiAction::ToggleAttachment(attachment.id.clone()));
                    }
                }
            });
        }
        let hint = if summary.category == StatusCategory::Sleeping {
            "Message Devin — sending will wake this session"
        } else {
            "Message Devin"
        };
        // The Agent composer's anatomy: a hairline over a frameless input,
        // with the send control as a solid 32 px action on the trailing edge.
        ui.add_space(theme::space::SNUG);
        let divider = ui.cursor().top();
        ui.painter().hline(
            ui.max_rect().x_range(),
            divider,
            egui::Stroke::new(1.0, theme::border::hairline_color()),
        );
        ui.add_space(theme::space::SMALL);
        let input = ui.add(
            TextEdit::multiline(&mut self.devin_message)
                .id(Id::new("devin_prompt"))
                .hint_text(
                    RichText::new(hint)
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
        if self.devin_focus_detail {
            if summary.category == StatusCategory::Waiting {
                input.request_focus();
            }
            self.devin_focus_detail = false;
        }
        let can_send = !self.devin_state.busy
            && (!self.devin_message.trim().is_empty() || !self.devin_attachment_ids.is_empty());
        let enter = input.has_focus()
            && ui.input(|input| !input.modifiers.shift && input.key_pressed(Key::Enter));
        let mut send = false;
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let (fill, color) = agent_send_button_colors(can_send);
            send = agent_composer_action(ui, Icon::ArrowUp, "Send (Enter)", fill, color, can_send)
                .clicked();
        });
        if (send || enter) && can_send {
            *action = Some(DevinUiAction::SendMessage);
        }
    }

    fn draw_devin_stream(&mut self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        #[derive(Clone, Copy)]
        enum Kind {
            Message(usize),
            Activity(usize),
        }
        let mut stream = self
            .devin_state
            .messages
            .iter()
            .enumerate()
            .map(|(index, message)| (message.timestamp.as_str(), Kind::Message(index)))
            .chain(
                self.devin_state
                    .activity
                    .iter()
                    .enumerate()
                    .map(|(index, event)| (event.timestamp.as_str(), Kind::Activity(index))),
            )
            .collect::<Vec<_>>();
        stream.sort_by(|left, right| chronological_timestamp(left.0, right.0));
        if stream.is_empty() && self.devin_pending_message.is_none() {
            ui.label(
                RichText::new("No conversation or remote activity yet.")
                    .font(theme::typography::body())
                    .color(theme::text().muted),
            );
        }
        let mut index = 0;
        while index < stream.len() {
            if index > 0 {
                ui.add_space(theme::space::SMALL);
            }
            match stream[index].1 {
                Kind::Message(message) => {
                    draw_devin_message(
                        ui,
                        &self.devin_state.messages[message],
                        self.devin_state.detail.as_ref(),
                    );
                    index += 1;
                }
                Kind::Activity(_) => {
                    let start = index;
                    while index < stream.len() && matches!(stream[index].1, Kind::Activity(_)) {
                        index += 1;
                    }
                    let Kind::Activity(first) = stream[start].1 else {
                        unreachable!()
                    };
                    let group_id = self.devin_state.activity[first].id.clone();
                    let expanded = self.devin_expanded_activity_groups.contains(&group_id);
                    let count = index - start;
                    if ui
                        .selectable_label(
                            expanded,
                            RichText::new(format!(
                                "{count} remote action{}",
                                if count == 1 { "" } else { "s" }
                            ))
                            .font(theme::typography::small())
                            .color(theme::text().secondary),
                        )
                        .on_hover_text("Remote Devin activity; paths do not open local files")
                        .clicked()
                    {
                        *action = Some(DevinUiAction::ToggleActivity(group_id));
                    }
                    if expanded {
                        for (_, kind) in &stream[start..index] {
                            let Kind::Activity(activity) = kind else {
                                continue;
                            };
                            ui.add_space(theme::space::TIGHT);
                            draw_devin_activity(ui, &self.devin_state.activity[*activity]);
                        }
                    }
                }
            }
        }
        if let Some(pending) = self.devin_pending_message.as_ref() {
            if !stream.is_empty() {
                ui.add_space(theme::space::SMALL);
            }
            devin_role_label(ui, "YOU", None);
            devin_user_bubble(ui, |ui| {
                ui.add(
                    Label::new(
                        RichText::new(&pending.text)
                            .font(theme::typography::body())
                            .color(theme::text().primary),
                    )
                    .selectable(true)
                    .wrap(),
                );
                if pending.failed {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("Not sent")
                                .font(theme::typography::small())
                                .color(theme::ink(theme::semantic().danger)),
                        );
                        if ui.button("Retry").clicked() {
                            *action = Some(DevinUiAction::RetryMessage);
                        }
                    });
                } else {
                    ui.label(
                        RichText::new(if pending.confirmed {
                            "Sent — syncing…"
                        } else {
                            "Sending…"
                        })
                        .font(theme::typography::small())
                        .color(theme::text().muted),
                    );
                }
            });
        }
    }

    fn selected_devin_summary(&self) -> Option<&SessionSummary> {
        self.devin_state
            .detail
            .as_ref()
            .map(|detail| &detail.summary)
            .or_else(|| {
                let selected = self.devin_state.selected_session.as_deref()?;
                self.devin_state
                    .sessions
                    .iter()
                    .find(|session| session.id == selected)
            })
    }

    fn apply_devin_ui_action(&mut self, action: DevinUiAction, ctx: &egui::Context) {
        let command = match action {
            DevinUiAction::Close => {
                self.execute_keybinding(KeybindingCommand::AppToggleDevinSidebar, None, ctx);
                return;
            }
            DevinUiAction::Back => {
                self.devin_view = DevinView::Sessions;
                self.devin_focus_list = true;
                return;
            }
            DevinUiAction::NewSession => {
                self.devin_view = DevinView::Create;
                self.devin_focus_create = true;
                self.devin_state.error = None;
                return;
            }
            DevinUiAction::Connect => Some(DevinCommand::SaveCredentials {
                api_key: self.devin_api_key.clone(),
                org_id: (!self.devin_org_id.trim().is_empty())
                    .then(|| self.devin_org_id.trim().to_owned()),
            }),
            DevinUiAction::ConfirmDisconnect => {
                self.devin_confirm_disconnect = true;
                return;
            }
            DevinUiAction::Reconnect => Some(DevinCommand::Disconnect),
            DevinUiAction::Refresh => {
                if self.devin_state.selected_session.is_some() {
                    self.send_devin(DevinCommand::RefreshSelected);
                }
                Some(DevinCommand::RefreshSessions)
            }
            DevinUiAction::LoadMoreSessions => Some(DevinCommand::LoadMoreSessions),
            DevinUiAction::Select(session_id) => {
                let generation = self.devin_state.select(session_id.clone());
                self.devin_view = DevinView::Detail;
                self.devin_focus_detail = true;
                self.devin_attachment_ids.clear();
                Some(DevinCommand::SelectSession {
                    session_id,
                    generation,
                })
            }
            DevinUiAction::Create => {
                self.devin_state.busy = true;
                self.devin_state.error = None;
                self.devin_creating = true;
                Some(DevinCommand::CreateSession {
                    repository: self.devin_repository.trim().into(),
                    prompt: self.devin_create_prompt.trim().into(),
                })
            }
            DevinUiAction::SendMessage => {
                let pending = PendingDevinMessage {
                    text: self.devin_message.trim().into(),
                    attachment_ids: self.devin_attachment_ids.iter().cloned().collect(),
                    failed: false,
                    confirmed: false,
                };
                self.devin_message.clear();
                self.devin_state.busy = true;
                self.devin_pending_message = Some(pending.clone());
                Some(DevinCommand::SendMessage {
                    message: pending.text,
                    attachment_ids: pending.attachment_ids,
                })
            }
            DevinUiAction::RetryMessage => {
                let Some(pending) = self.devin_pending_message.as_mut() else {
                    return;
                };
                pending.failed = false;
                pending.confirmed = false;
                self.devin_state.busy = true;
                Some(DevinCommand::SendMessage {
                    message: pending.text.clone(),
                    attachment_ids: pending.attachment_ids.clone(),
                })
            }
            DevinUiAction::LoadMoreMessages => Some(DevinCommand::LoadMoreMessages),
            DevinUiAction::LoadMoreEvents => Some(DevinCommand::LoadMoreEvents),
            DevinUiAction::Lifecycle(lifecycle) => {
                self.devin_pending_lifecycle = Some(lifecycle);
                self.devin_state.busy = true;
                Some(match lifecycle {
                    DevinLifecycle::Sleep => DevinCommand::Sleep,
                    DevinLifecycle::Archive => DevinCommand::Archive,
                    DevinLifecycle::Unarchive => DevinCommand::Unarchive,
                    DevinLifecycle::Terminate => DevinCommand::TerminateConfirmed,
                })
            }
            DevinUiAction::ConfirmTerminate => {
                self.devin_confirm_terminate = true;
                return;
            }
            DevinUiAction::ToggleAttachment(id) => {
                if !self.devin_attachment_ids.remove(&id) {
                    self.devin_attachment_ids.insert(id);
                }
                return;
            }
            DevinUiAction::ToggleActivity(id) => {
                if !self.devin_expanded_activity_groups.remove(&id) {
                    self.devin_expanded_activity_groups.insert(id);
                }
                return;
            }
        };
        if let Some(command) = command {
            self.send_devin(command);
        }
    }

    pub(super) fn send_devin(&mut self, command: DevinCommand) {
        if let Some(controller) = self.devin_controller.as_ref()
            && let Err(error) = controller.send(command)
        {
            self.show_error(error);
        }
    }

    pub(super) fn open_devin(&mut self, ctx: &egui::Context) {
        if self.devin_controller.is_none() {
            let wake = ctx.clone();
            self.devin_controller =
                Some(DevinController::start(self.tree.root.clone(), move || {
                    wake.request_repaint();
                }));
        }
        match self.devin_view {
            DevinView::Sessions => self.devin_focus_list = true,
            DevinView::Create => self.devin_focus_create = true,
            DevinView::Detail => self.devin_focus_detail = true,
        }
        if let Some(controller) = self.devin_controller.as_ref() {
            let _ = controller.send(DevinCommand::SetVisible(true));
        }
    }

    pub(super) fn poll_devin(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        if let Some(controller) = self.devin_controller.as_ref() {
            for _ in 0..32 {
                let Ok(event) = controller.events().try_recv() else {
                    break;
                };
                events.push(event);
            }
        }
        if events.len() == 32 {
            ctx.request_repaint();
        }
        let mut select_created = None;
        for event in events {
            match &event {
                DevinEvent::RepositoryResolved(RepositoryState::Suggested(repository))
                    if self.devin_repository.is_empty() =>
                {
                    self.devin_repository = repository.clone();
                }
                DevinEvent::CredentialsChanged(Some(_)) => self.devin_api_key.clear(),
                DevinEvent::CredentialsChanged(None) => {
                    self.devin_view = DevinView::Sessions;
                    self.devin_pending_message = None;
                    self.devin_pending_lifecycle = None;
                }
                DevinEvent::SessionCreated(session) => {
                    self.devin_creating = false;
                    self.devin_create_prompt.clear();
                    select_created = Some(session.id.clone());
                }
                DevinEvent::SessionLoaded { detail, .. } => {
                    self.devin_attachment_ids.retain(|id| {
                        detail
                            .attachments
                            .iter()
                            .any(|attachment| &attachment.id == id)
                    });
                    if detail.summary.category == StatusCategory::Waiting {
                        self.devin_focus_detail = true;
                    }
                }
                DevinEvent::OperationFinished { .. } => {
                    if let Some(pending) = self.devin_pending_message.as_mut() {
                        pending.confirmed = true;
                    }
                    if let Some(lifecycle) = self.devin_pending_lifecycle.take() {
                        match lifecycle {
                            DevinLifecycle::Archive => {
                                self.toasts.push(Severity::Info, "Devin session archived");
                                self.devin_view = DevinView::Sessions;
                                self.devin_focus_list = true;
                            }
                            DevinLifecycle::Unarchive => {
                                self.toasts.push(Severity::Info, "Devin session unarchived");
                            }
                            DevinLifecycle::Terminate => {
                                self.toasts.push(Severity::Info, "Devin session terminated");
                            }
                            DevinLifecycle::Sleep => {}
                        }
                    }
                }
                DevinEvent::Failed(error) => {
                    self.devin_creating = false;
                    if let Some(pending) = self
                        .devin_pending_message
                        .as_mut()
                        .filter(|pending| !pending.confirmed)
                    {
                        pending.failed = true;
                    }
                    if self.devin_pending_lifecycle.take().is_some() {
                        self.toasts.push(Severity::Danger, &error.message);
                    }
                }
                DevinEvent::MessagesLoaded { messages, .. } => {
                    if let Some(pending) = self.devin_pending_message.as_ref()
                        && messages.iter().any(|message| {
                            message.role.eq_ignore_ascii_case("user")
                                && message.text.trim() == pending.text.trim()
                        })
                    {
                        self.devin_pending_message = None;
                    }
                }
                _ => {}
            }
            self.devin_state.apply(event);
        }
        if let Some(session_id) = select_created {
            let generation = self.devin_state.select(session_id.clone());
            self.devin_view = DevinView::Detail;
            self.devin_focus_detail = true;
            self.send_devin(DevinCommand::SelectSession {
                session_id,
                generation,
            });
        }
    }
}

fn visible_sessions<'a>(
    sessions: &'a [SessionSummary],
    scope: DevinScope,
    query: &str,
) -> Vec<&'a SessionSummary> {
    let query = query.trim().to_ascii_lowercase();
    let mut sessions = sessions
        .iter()
        .filter(|session| match scope {
            DevinScope::Active => {
                !session.archived
                    && !matches!(
                        session.category,
                        StatusCategory::Completed | StatusCategory::Failed
                    )
            }
            DevinScope::All => !session.archived,
            DevinScope::Archived => session.archived,
        })
        .filter(|session| {
            query.is_empty()
                || [
                    Some(session.title.as_str()),
                    session.prompt.as_deref(),
                    session.repository.as_deref(),
                ]
                .into_iter()
                .flatten()
                .any(|value| value.to_ascii_lowercase().contains(&query))
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| {
        session_group(left)
            .0
            .cmp(&session_group(right).0)
            .then_with(|| right.updated_at.cmp(&left.updated_at))
    });
    sessions
}

fn devin_scopes() -> [(DevinScope, &'static str); 3] {
    [
        (DevinScope::Active, "Active"),
        (DevinScope::All, "All"),
        (DevinScope::Archived, "Archived"),
    ]
}

fn devin_scope_label(scope: DevinScope) -> &'static str {
    devin_scopes()
        .into_iter()
        .find_map(|(candidate, label)| (candidate == scope).then_some(label))
        .unwrap_or("Active")
}

fn visible_session_ids<'a>(
    sessions: &'a [SessionSummary],
    scope: DevinScope,
    query: &str,
) -> Vec<&'a str> {
    visible_sessions(sessions, scope, query)
        .into_iter()
        .map(|session| session.id.as_str())
        .collect()
}

fn session_group(session: &SessionSummary) -> (u8, &'static str) {
    match session.category {
        StatusCategory::Waiting => (0, "NEEDS YOU"),
        StatusCategory::Active | StatusCategory::Unknown => (1, "WORKING"),
        StatusCategory::Sleeping => (2, "IDLE"),
        StatusCategory::Completed | StatusCategory::Failed => (3, "DONE"),
    }
}

fn session_title(session: &SessionSummary) -> &str {
    if session.title == "Untitled session" {
        session.prompt.as_deref().unwrap_or(&session.title)
    } else {
        &session.title
    }
}

fn semantic_status(session: &SessionSummary) -> &str {
    match session.category {
        StatusCategory::Waiting => "Needs you",
        StatusCategory::Active => "Working",
        StatusCategory::Sleeping => "Idle",
        StatusCategory::Completed => "Finished",
        StatusCategory::Failed => "Failed",
        StatusCategory::Unknown => &session.status,
    }
}

fn origin_label(origin: &str) -> &str {
    match origin.to_ascii_lowercase().as_str() {
        "slack" => "Slack",
        "web" | "devin" | "devin web" => "Devin web",
        "editur" | "api" | "mcp" => "Editur",
        _ => origin,
    }
}

fn devin_status_dot_color(ui: &egui::Ui, status: StatusCategory) -> Color32 {
    let base = match status {
        StatusCategory::Active => theme::accent(),
        StatusCategory::Waiting => theme::semantic().warning,
        StatusCategory::Sleeping | StatusCategory::Unknown => theme::text().muted,
        StatusCategory::Completed => theme::semantic().success,
        StatusCategory::Failed => theme::semantic().danger,
    };
    if status == StatusCategory::Active && !theme::motion::reduced(ui.ctx()) {
        let phase = ui.input(|input| input.time * std::f64::consts::TAU / 1.6);
        ui.ctx().request_repaint_after(Duration::from_millis(50));
        base.gamma_multiply((0.78 + phase.sin() as f32 * 0.16).clamp(0.62, 0.94))
    } else {
        base
    }
}

fn devin_freshness(state: &DevinState) -> String {
    match state.connection {
        DevinConnectionState::RateLimited => "Rate limited — refresh slowed".into(),
        DevinConnectionState::Offline => "Offline — will retry".into(),
        DevinConnectionState::Failed => "Retrying…".into(),
        DevinConnectionState::AuthenticationRequired => {
            "Connection lost — data may be stale".into()
        }
        DevinConnectionState::Connecting => "Refreshing…".into(),
        _ => state.last_sessions_refresh.map_or_else(
            || "Waiting for first refresh…".into(),
            |updated| {
                format!(
                    "Updated {} ago",
                    compact_duration(updated.elapsed().as_secs())
                )
            },
        ),
    }
}

fn compact_relative_time(value: &str) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs() as i64);
    parse_timestamp_seconds(value).map_or_else(
        || value.chars().take(24).collect(),
        |timestamp| compact_duration(now.saturating_sub(timestamp).max(0) as u64),
    )
}

fn compact_duration(seconds: u64) -> String {
    match seconds {
        0..=59 => format!("{seconds}s"),
        60..=3_599 => format!("{}m", seconds / 60),
        3_600..=86_399 => format!("{}h", seconds / 3_600),
        _ => format!("{}d", seconds / 86_400),
    }
}

fn parse_timestamp_seconds(value: &str) -> Option<i64> {
    if let Ok(number) = value.parse::<f64>() {
        return Some(if number > 10_000_000_000.0 {
            (number / 1_000.0) as i64
        } else {
            number as i64
        });
    }
    let bytes = value.as_bytes();
    if bytes.len() < 19 || bytes.get(4) != Some(&b'-') || bytes.get(7) != Some(&b'-') {
        return None;
    }
    let number = |range: std::ops::Range<usize>| value.get(range)?.parse::<i64>().ok();
    let year = number(0..4)?;
    let month = number(5..7)?;
    let day = number(8..10)?;
    let hour = number(11..13)?;
    let minute = number(14..16)?;
    let second = number(17..19)?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second)
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

fn chronological_timestamp(left: &str, right: &str) -> std::cmp::Ordering {
    match (
        parse_timestamp_seconds(left),
        parse_timestamp_seconds(right),
    ) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

/// The Agent transcript's section label: micro, semibold, muted, with an
/// optional relative timestamp trailing it.
fn devin_role_label(ui: &mut egui::Ui, role: &str, timestamp: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(role)
                .size(theme::typography::MICRO_SIZE)
                .strong()
                .color(theme::text().muted),
        );
        if let Some(timestamp) = timestamp.filter(|timestamp| !timestamp.is_empty()) {
            ui.label(
                RichText::new(compact_relative_time(timestamp))
                    .size(theme::typography::MICRO_SIZE)
                    .color(theme::text().muted),
            );
        }
    });
}

/// The Agent's user-message bubble: input fill, strong border, 12 px inset.
fn devin_user_bubble(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::surface().input)
        .stroke(egui::Stroke::new(1.0, theme::border::strong_color()))
        .inner_margin(egui::Margin::same(12))
        .corner_radius(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui);
        });
}

fn draw_devin_message_attachments(
    ui: &mut egui::Ui,
    message: &DevinMessage,
    detail: Option<&crate::devin::SessionDetail>,
) {
    let Some(detail) = detail else { return };
    if message.attachment_ids.is_empty() {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        for id in &message.attachment_ids {
            let Some(attachment) = detail
                .attachments
                .iter()
                .find(|attachment| &attachment.id == id)
            else {
                continue;
            };
            if let Some(url) = attachment.url.as_deref() {
                ui.hyperlink_to(
                    RichText::new(&attachment.name).font(theme::typography::small()),
                    url,
                );
            } else {
                chip(ui, &attachment.name);
            }
        }
    });
}

fn draw_devin_message(
    ui: &mut egui::Ui,
    message: &DevinMessage,
    detail: Option<&crate::devin::SessionDetail>,
) {
    let body = |ui: &mut egui::Ui| {
        ui.add(
            Label::new(
                RichText::new(&message.text)
                    .font(theme::typography::body())
                    .color(theme::text().primary),
            )
            .selectable(true)
            .wrap(),
        );
        draw_devin_message_attachments(ui, message, detail);
    };
    if message.role.eq_ignore_ascii_case("user") {
        devin_role_label(ui, "YOU", Some(&message.timestamp));
        devin_user_bubble(ui, body);
    } else {
        // Devin's turns read like the Agent's: an identity line over plain
        // text, not a bubble.
        let identity = if message.role.eq_ignore_ascii_case("devin")
            || message.role.eq_ignore_ascii_case("devin_ai")
            || message.role.eq_ignore_ascii_case("assistant")
        {
            "Devin".to_owned()
        } else {
            message.role.clone()
        };
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(identity)
                    .size(theme::typography::BODY_SIZE)
                    .strong()
                    .color(theme::text().primary),
            );
            if !message.timestamp.is_empty() {
                ui.label(
                    RichText::new(compact_relative_time(&message.timestamp))
                        .size(theme::typography::MICRO_SIZE)
                        .color(theme::text().muted),
                );
            }
        });
        ui.add_space(theme::space::TIGHT);
        body(ui);
    }
}

fn draw_devin_activity(ui: &mut egui::Ui, activity: &Activity) {
    ui.horizontal_wrapped(|ui| {
        ui.label(
            RichText::new("REMOTE")
                .size(theme::typography::MICRO_SIZE)
                .strong()
                .color(theme::text().muted),
        );
        ui.label(
            RichText::new(format!("{} · {}", activity.category, activity.summary))
                .font(theme::typography::small())
                .color(theme::text().secondary),
        );
        if !activity.timestamp.is_empty() {
            ui.label(
                RichText::new(compact_relative_time(&activity.timestamp))
                    .size(theme::typography::MICRO_SIZE)
                    .color(theme::text().muted),
            );
        }
    });
    for detail_line in [activity.path.as_deref(), activity.command.as_deref()]
        .into_iter()
        .flatten()
    {
        ui.label(
            RichText::new(detail_line)
                .font(theme::typography::code_small())
                .color(theme::text().muted),
        );
    }
    if let Some(details) = activity.details.as_deref() {
        ui.add(
            Label::new(
                RichText::new(details)
                    .font(theme::typography::small())
                    .color(theme::text().secondary),
            )
            .selectable(true)
            .wrap(),
        );
    }
    if let Some(url) = activity.url.as_deref() {
        ui.hyperlink_to(
            RichText::new("Open remote link").font(theme::typography::small()),
            url,
        );
    }
}

fn devin_callout(ui: &mut egui::Ui, message: &str, color: Color32) {
    let callout = theme::callout(color);
    egui::Frame::new()
        .fill(callout.fill)
        .stroke(egui::Stroke::new(1.0, callout.border))
        .inner_margin(egui::Margin::symmetric(
            theme::space::MEDIUM as i8,
            theme::space::SMALL as i8,
        ))
        .corner_radius(theme::corner(theme::radius::CONTROL))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                Label::new(
                    RichText::new(message)
                        .font(theme::typography::body())
                        .color(callout.text),
                )
                .wrap(),
            );
        });
}

fn devin_text_field_focused(ctx: &egui::Context) -> bool {
    ctx.memory(|memory| {
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_home_filters_archives_and_prioritizes_waiting_work() {
        let session = |id: &str, category, archived| SessionSummary {
            id: id.into(),
            title: id.into(),
            category,
            archived,
            ..SessionSummary::default()
        };
        let sessions = vec![
            session("working", StatusCategory::Active, false),
            session("waiting", StatusCategory::Waiting, false),
            session("archived", StatusCategory::Completed, true),
        ];

        assert_eq!(
            visible_session_ids(&sessions, DevinScope::Active, ""),
            vec!["waiting", "working"]
        );
        assert_eq!(
            visible_session_ids(&sessions, DevinScope::Archived, ""),
            vec!["archived"]
        );
    }

    #[test]
    fn rfc3339_and_numeric_timestamps_share_one_chronology() {
        assert_eq!(parse_timestamp_seconds("1970-01-01T00:00:02Z"), Some(2));
        assert_eq!(
            chronological_timestamp("2", "1970-01-01T00:00:10Z"),
            std::cmp::Ordering::Less
        );
    }
}
