use std::time::{SystemTime, UNIX_EPOCH};

use super::*;
use crate::devin::{Activity, DevinMessage, SessionSummary};

const DEVIN_WEB: &str = "https://app.devin.ai/";
const MAX_DEVIN_BATCH_SESSIONS: usize = 10;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub(super) enum DevinView {
    #[default]
    Sessions,
    Filters,
    Create,
    Detail,
    Section(DevinSection),
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
    attachments: Vec<PromptAttachment>,
    failed: bool,
    confirmed: bool,
}

pub(super) struct DevinAdvancedDraft {
    pub(super) open: bool,
    title: String,
    mode: String,
    playbook_id: String,
    child_playbook_id: String,
    knowledge_ids: String,
    tags: String,
    max_acu_limit: String,
    platform: String,
    resumable: bool,
    session_links: String,
    structured_output_schema: String,
    structured_output_required: bool,
    secret_ids: String,
    session_secret_key: String,
    session_secret_value: String,
    create_as_user_id: String,
    bypass_approval: bool,
    batch_count: usize,
    parent_session_id: String,
}

impl Default for DevinAdvancedDraft {
    fn default() -> Self {
        Self {
            open: false,
            title: String::new(),
            mode: String::new(),
            playbook_id: String::new(),
            child_playbook_id: String::new(),
            knowledge_ids: String::new(),
            tags: String::new(),
            max_acu_limit: String::new(),
            platform: String::new(),
            resumable: true,
            session_links: String::new(),
            structured_output_schema: String::new(),
            structured_output_required: false,
            secret_ids: String::new(),
            session_secret_key: String::new(),
            session_secret_value: String::new(),
            create_as_user_id: String::new(),
            bypass_approval: false,
            batch_count: 1,
            parent_session_id: String::new(),
        }
    }
}

impl DevinScope {
    fn label(self) -> &'static str {
        match self {
            Self::Active => "Active sessions",
            Self::All => "All sessions",
            Self::Archived => "Archived sessions",
        }
    }
}

#[derive(Default)]
pub(super) struct DevinResourceDraft {
    id: String,
    name: String,
    content: String,
    folder_or_repository: String,
    extra: String,
    json: String,
    secret_value: String,
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
    #[cfg(debug_assertions)]
    SeedPreview,
    Back,
    NewSession,
    OpenFilters,
    ApplyFilters,
    ClearFilters,
    Connect,
    ConfirmDisconnect,
    ConfirmOrganizationSwitch(String),
    Reconnect,
    Refresh,
    LoadMoreSessions,
    Select(String),
    Create,
    SendMessage,
    RetryMessage,
    OpenSection(DevinSection),
    LoadKnowledge(String),
    LoadKnowledgeSuggestion(String),
    LoadPlaybook(String),
    LoadSchedule(String),
    LoadAutomation(String),
    LoadRepositoryWiki(String),
    AskRepository,
    GenerateInsight,
    MutateResource(ResourceMutation),
    ConfirmResourceMutation(ResourceMutation),
    ReplaceTags,
    AppendTags,
    SearchActivity,
    FetchActivity(String),
    GatherSessions,
    LaunchChild(String),
    LoadBlueprint(String),
    UploadBlueprintFiles,
    LoadBuildLogs(String),
    LoadBuild(String),
    Lifecycle(DevinLifecycle),
    ConfirmTerminate,
    OpenImage(String),
}

fn devin_header_button(
    ui: &mut egui::Ui,
    id: &'static str,
    icon: Icon,
    label: &'static str,
    size: egui::Vec2,
) -> egui::Response {
    let (rect, _) = ui.allocate_exact_size(size, Sense::click());
    let response = ui.interact(rect, Id::new(id), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let color = if !ui.is_enabled() {
        theme::text_disabled()
    } else if response.hovered() {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    icons::paint(
        ui.painter(),
        icon,
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(icons::GRID)),
        color,
    );
    response.on_hover_text(label)
}

impl EditorApp {
    pub(super) fn draw_devin_sidebar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        draw_assistant_sidebar_surface(ui, rect);
        let header = assistant_sidebar_header(rect);
        // The session home mirrors its header with an equally thin footer:
        // data freshness on the left, the panel-level overflow on the right.
        let footer = (self.devin_view == DevinView::Sessions && !self.devin_needs_connect_card())
            .then(|| rect.with_min_y((rect.bottom() - TITLEBAR_HEIGHT).max(header.bottom())));
        let body = egui::Rect::from_min_max(
            egui::pos2(rect.left(), header.bottom()),
            egui::pos2(
                rect.right(),
                footer.map_or(rect.bottom(), |footer| footer.top()),
            ),
        );
        let mut action = None;

        // The Agent header's exact chrome: the title 14 px in from the left,
        // and 28 px full-height glyph buttons stacked flush against the right
        // edge, so the two panels wear identical hats.
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("devin_header")
                .max_rect(header)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| self.draw_devin_header(ui, &mut action),
        );
        paint_assistant_header_divider(ui.painter(), header);
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("devin_body", self.devin_view))
                .max_rect(body.shrink2(egui::vec2(theme::space::MEDIUM, theme::space::SMALL))),
            |ui| {
                ui.set_width(ui.available_width());
                if self.devin_needs_connect_card() {
                    ScrollArea::vertical()
                        .id_salt("devin_connect_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            self.draw_devin_connect(ui, &mut action);
                        });
                    return;
                }
                self.draw_devin_connection_banner(ui, &mut action);
                match self.devin_view {
                    DevinView::Sessions => self.draw_devin_sessions(ui, &mut action),
                    DevinView::Filters => self.draw_devin_filters(ui, &mut action),
                    DevinView::Create => self.draw_devin_create(ui, &mut action),
                    DevinView::Detail => self.draw_devin_detail(ui, &mut action),
                    DevinView::Section(section) => {
                        self.draw_devin_section(ui, section, &mut action)
                    }
                }
            },
        );
        if let Some(footer) = footer {
            ui.painter().hline(
                footer.x_range(),
                footer.top() + 0.5,
                egui::Stroke::new(1.0, theme::border::hairline_color()),
            );
            let footer_content = egui::Rect::from_min_max(
                egui::pos2(footer.left() + theme::space::MEDIUM, footer.top()),
                egui::pos2(footer.right() - theme::space::HAIR, footer.bottom()),
            );
            ui.scope_builder(
                UiBuilder::new()
                    .id_salt("devin_footer")
                    .max_rect(footer_content)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| self.draw_devin_footer(ui, &mut action),
            );
        }

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
        // The Agent header's metrics verbatim: the title 14 px in, glyph
        // buttons 28 px wide at full header height, packed with no gaps.
        let button_size = egui::vec2(28.0, ui.available_height());
        ui.spacing_mut().item_spacing.x = 0.0;
        let header_title = |text: &str| {
            RichText::new(text)
                .font(theme::typography::body())
                .color(theme::text().primary)
        };
        let back = |ui: &mut egui::Ui, action: &mut Option<DevinUiAction>| {
            if devin_header_button(
                ui,
                "devin_back",
                Icon::ChevronLeft,
                "Back to Devin sessions",
                button_size,
            )
            .clicked()
            {
                *action = Some(DevinUiAction::Back);
            }
            ui.add_space(theme::space::SNUG);
        };
        match self.devin_view {
            DevinView::Sessions => {
                ui.add_space(14.0);
                ui.label(header_title("Devin"))
            }
            DevinView::Filters => {
                back(ui, action);
                ui.label(header_title("Session filters"))
            }
            DevinView::Create => {
                back(ui, action);
                ui.label(header_title("New session"))
            }
            DevinView::Detail => {
                back(ui, action);
                // The session's status lives here as the list's dot language,
                // between the chevron and the title, with the detail on hover
                // — not as a chip-and-caption stack pushing the transcript
                // down.
                if connected_surface && let Some(summary) = self.selected_devin_summary() {
                    let status = semantic_status(summary);
                    let color = devin_status_dot_color(summary.category);
                    let (dot, hover) =
                        ui.allocate_exact_size(egui::vec2(14.0, button_size.y), Sense::hover());
                    ui.painter().circle_filled(dot.center(), 4.0, color);
                    hover.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::WidgetType::Label,
                            true,
                            format!(
                                "Status: {status}; {}",
                                summary.status_detail.as_deref().unwrap_or(&summary.status)
                            ),
                        )
                    });
                    hover
                        .on_hover_text(summary.status_detail.as_deref().unwrap_or(&summary.status));
                    ui.add_space(theme::space::TIGHT);
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
                let pull_request = self
                    .devin_state
                    .detail
                    .as_ref()
                    .and_then(|detail| detail.pull_requests.first());
                // The title packs left against the dot at its own text width
                // — `add_sized` would center it in the leftover header width —
                // capped so it truncates before the open button and the
                // trailing chrome.
                let mut trailing = if cfg!(debug_assertions) { 88.0 } else { 60.0 };
                if pull_request.is_some() {
                    trailing += button_size.x + theme::space::TIGHT;
                }
                let text_width = ui
                    .painter()
                    .layout_no_wrap(
                        title.to_owned(),
                        theme::typography::body(),
                        theme::text().primary,
                    )
                    .size()
                    .x;
                let width = (text_width + 1.0).min((ui.available_width() - trailing).max(48.0));
                let response = ui
                    .allocate_ui_with_layout(
                        egui::vec2(width, ui.available_height()),
                        Layout::left_to_right(Align::Center),
                        |ui| ui.add(Label::new(header_title(title)).truncate()),
                    )
                    .inner;
                // The session's pull request opens from the header, right
                // beside the title it belongs to.
                if let Some(pull_request) = pull_request {
                    ui.add_space(theme::space::TIGHT);
                    if devin_header_button(
                        ui,
                        "devin_pr_open",
                        Icon::ExternalLink,
                        "Open pull request in browser",
                        button_size,
                    )
                    .clicked()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(&pull_request.url));
                    }
                }
                response
            }
            DevinView::Section(section) => {
                back(ui, action);
                ui.label(header_title(section.label()))
            }
        };

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            if devin_header_button(
                ui,
                "devin_close",
                Icon::Close,
                "Close Devin sidebar",
                button_size,
            )
            .clicked()
            {
                *action = Some(DevinUiAction::Close);
            }
            #[cfg(debug_assertions)]
            if devin_header_button(
                ui,
                "devin_seed_preview",
                Icon::Sparkle,
                "Seed Devin preview data",
                button_size,
            )
            .clicked()
            {
                *action = Some(DevinUiAction::SeedPreview);
            }
            match self.devin_view {
                DevinView::Sessions if connected_surface => {
                    if ui
                        .add_enabled_ui(
                            self.devin_state.capabilities.has("devin_session_create"),
                            |ui| {
                                devin_header_button(
                                    ui,
                                    "devin_new_session",
                                    Icon::Plus,
                                    "New Devin session",
                                    button_size,
                                )
                            },
                        )
                        .inner
                        .clicked()
                    {
                        *action = Some(DevinUiAction::NewSession);
                    }
                    if devin_header_button(
                        ui,
                        "devin_refresh",
                        Icon::Refresh,
                        "Refresh Devin sessions",
                        button_size,
                    )
                    .clicked()
                    {
                        *action = Some(DevinUiAction::Refresh);
                    }
                    if devin_header_button(
                        ui,
                        "devin_filters",
                        Icon::Search,
                        "Filter Devin sessions",
                        button_size,
                    )
                    .clicked()
                    {
                        *action = Some(DevinUiAction::OpenFilters);
                    }
                }
                DevinView::Detail if connected_surface => {
                    self.draw_devin_lifecycle_menu(ui, action)
                }
                DevinView::Section(section) if connected_surface => {
                    let switcher = devin_header_button(
                        ui,
                        "devin_section_switcher",
                        Icon::Ellipsis,
                        "Switch Devin section",
                        button_size,
                    );
                    egui::Popup::menu(&switcher).show(|ui| {
                        if ui.button("Sessions").clicked() {
                            *action = Some(DevinUiAction::Back);
                        }
                        for destination in DevinSection::ALL {
                            if ui
                                .add_enabled(
                                    destination != section
                                        && devin_section_available(&self.devin_state, destination),
                                    egui::Button::new(destination.label()),
                                )
                                .clicked()
                            {
                                *action = Some(DevinUiAction::OpenSection(destination));
                            }
                        }
                    });
                    if devin_header_button(
                        ui,
                        "devin_resource_refresh",
                        Icon::Refresh,
                        "Refresh Devin resource",
                        button_size,
                    )
                    .clicked()
                    {
                        *action = Some(DevinUiAction::OpenSection(section));
                    }
                }
                DevinView::Sessions
                | DevinView::Filters
                | DevinView::Detail
                | DevinView::Create
                | DevinView::Section(_) => {}
            }
        });
    }

    /// The freshness readout and account overflow, in a strip as thin as the
    /// header so the panel is book-ended by matching chrome.
    fn draw_devin_footer(&self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        ui.label(
            RichText::new(devin_freshness(&self.devin_state))
                .size(theme::typography::MICRO_SIZE)
                .color(theme::text().muted),
        );
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let button = icons::button_with_id(
                ui,
                Some(Id::new("devin_overflow_menu")),
                Icon::Ellipsis,
                "More Devin actions",
                theme::text().secondary,
                egui::vec2(theme::control::STANDARD, theme::control::COMPACT + 2.0),
            );
            egui::Popup::menu(&button).show(|ui| {
                for section in DevinSection::ALL {
                    if ui
                        .add_enabled(
                            devin_section_available(&self.devin_state, section),
                            egui::Button::new(section.label()),
                        )
                        .clicked()
                    {
                        *action = Some(DevinUiAction::OpenSection(section));
                    }
                }
                if self.devin_state.organizations.len() > 1 {
                    ui.separator();
                    for organization in &self.devin_state.organizations {
                        if ui
                            .add_enabled(
                                self.devin_state.selected_org_id.as_deref()
                                    != Some(organization.id.as_str()),
                                egui::Button::new(format!("Switch to {}", organization.name)),
                            )
                            .clicked()
                        {
                            *action = Some(DevinUiAction::ConfirmOrganizationSwitch(
                                organization.id.clone(),
                            ));
                        }
                    }
                }
                ui.separator();
                ui.hyperlink_to("Open Devin web app", DEVIN_WEB);
                if self.devin_state.credential_source == Some(CredentialSource::Keyring) {
                    ui.separator();
                    if ui.button("Disconnect…").clicked() {
                        *action = Some(DevinUiAction::ConfirmDisconnect);
                    }
                }
            });
        });
    }

    fn draw_devin_lifecycle_menu(&mut self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        let summary = self.selected_devin_summary().cloned();
        let archived = summary.as_ref().is_some_and(|summary| summary.archived);
        let url = summary
            .as_ref()
            .and_then(|summary| summary.url.as_deref())
            .unwrap_or(DEVIN_WEB)
            .to_owned();
        let button = devin_header_button(
            ui,
            "devin_lifecycle_menu",
            Icon::Ellipsis,
            "Devin session actions",
            egui::vec2(28.0, ui.available_height()),
        );
        egui::Popup::menu(&button).show(|ui| {
            ui.hyperlink_to("Open in Devin web app", &url);
            if let Some(summary) = summary.as_ref() {
                ui.separator();
                if ui.button("Launch child session").clicked() {
                    *action = Some(DevinUiAction::LaunchChild(summary.id.clone()));
                }
                if ui.button("Generate session insight").clicked() {
                    *action = Some(DevinUiAction::GenerateInsight);
                }
            }
            // Lineage navigation lives here rather than as buttons stacked at
            // the top of the panel.
            let parent = summary
                .as_ref()
                .and_then(|summary| summary.parent_session_id.as_deref());
            let children = self
                .devin_state
                .detail
                .as_ref()
                .map(|detail| detail.children.as_slice())
                .unwrap_or_default();
            if parent.is_some() || !children.is_empty() {
                ui.separator();
                if let Some(parent) = parent
                    && ui.button("Open parent session").clicked()
                {
                    *action = Some(DevinUiAction::Select(parent.into()));
                }
                for child in children {
                    if ui
                        .button(format!("Child: {} · {}", child.title, child.status))
                        .clicked()
                    {
                        *action = Some(DevinUiAction::Select(child.id.clone()));
                    }
                }
            }
            ui.separator();
            ui.menu_button("Edit labels", |ui| {
                ui.set_width(260.0);
                ui.add(
                    TextEdit::singleline(&mut self.devin_tags)
                        .id(Id::new("devin_tags"))
                        .hint_text(devin_field_hint("Comma-separated labels"))
                        .desired_width(260.0),
                );
                ui.horizontal(|ui| {
                    if ui.button("Add").clicked() {
                        *action = Some(DevinUiAction::AppendTags);
                        ui.close();
                    }
                    if ui.button("Replace").clicked() {
                        *action = Some(DevinUiAction::ReplaceTags);
                        ui.close();
                    }
                });
            });
            ui.menu_button("Find work history", |ui| {
                ui.set_width(260.0);
                ui.add(
                    TextEdit::singleline(&mut self.devin_activity_query)
                        .id(Id::new("devin_activity_query"))
                        .hint_text(devin_field_hint("Search remote actions"))
                        .desired_width(260.0),
                );
                if ui
                    .add_enabled(
                        !self.devin_activity_query.trim().is_empty(),
                        egui::Button::new("Search"),
                    )
                    .clicked()
                {
                    *action = Some(DevinUiAction::SearchActivity);
                    ui.close();
                }
            });
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

    /// The connect card sits centered in the panel like a sign-in sheet: a
    /// capped column, generous field spacing, and one full-width primary
    /// action, rather than a form crammed under the header.
    fn draw_devin_connect(&mut self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        const CARD_WIDTH: f32 = 288.0;
        const CARD_HEIGHT: f32 = 316.0;
        let field_label = |text: &str| {
            RichText::new(text)
                .font(theme::typography::small_strong())
                .color(theme::text().secondary)
        };
        let field_background = theme::mix(theme::surface().input, theme::surface().raised, 0.25);
        const FIELD_MARGIN: egui::Margin = egui::Margin::symmetric(10, 8);
        let width = ui.available_width().min(CARD_WIDTH);
        let indent = ((ui.available_width() - width) / 2.0).max(0.0);
        // Optically centered: a touch above true center, and the scroll area
        // still owns overflow when the panel is short.
        ui.add_space(((ui.available_height() - CARD_HEIGHT) * 0.42).max(theme::space::WIDE));
        ui.horizontal(|ui| {
            ui.add_space(indent);
            ui.vertical(|ui| {
                ui.set_width(width);
                ui.vertical_centered(|ui| {
                    let (mark, _) = ui.allocate_exact_size(egui::vec2(40.0, 44.0), Sense::hover());
                    paint_devin_mark(ui.painter(), mark);
                    ui.add_space(theme::space::MEDIUM);
                    ui.label(
                        RichText::new("Connect to Devin")
                            .font(theme::typography::title())
                            .color(theme::text().primary),
                    );
                });
                ui.add_space(theme::space::XWIDE);
                ui.label(field_label("API key"));
                ui.add_space(theme::space::TIGHT);
                if !self.devin_state.organizations.is_empty() {
                    let selected = self
                        .devin_state
                        .organizations
                        .iter()
                        .find(|organization| organization.id == self.devin_org_id)
                        .map(|organization| organization.name.as_str())
                        .unwrap_or("Choose an organization")
                        .to_owned();
                    settings_combo_box(
                        ui,
                        "devin_organization_picker",
                        ui.available_width(),
                        &selected,
                        |ui| {
                            for organization in &self.devin_state.organizations {
                                if settings_combo_choice(
                                    ui,
                                    &organization.name,
                                    organization.id == self.devin_org_id,
                                ) {
                                    self.devin_org_id = organization.id.clone();
                                }
                            }
                        },
                    );
                    ui.add_space(theme::space::TIGHT);
                }
                ui.add(
                    TextEdit::singleline(&mut self.devin_api_key)
                        .id(Id::new("devin_api_key"))
                        .font(theme::typography::body())
                        .margin(FIELD_MARGIN)
                        .background_color(field_background)
                        .desired_width(f32::INFINITY)
                        .password(true)
                        .hint_text(devin_field_hint("cog_…")),
                );
                ui.add_space(theme::space::MEDIUM);
                ui.label(field_label("Organization ID"));
                ui.add_space(theme::space::TIGHT);
                ui.add(
                    TextEdit::singleline(&mut self.devin_org_id)
                        .id(Id::new("devin_org_id"))
                        .font(theme::typography::body())
                        .margin(FIELD_MARGIN)
                        .background_color(field_background)
                        .desired_width(f32::INFINITY)
                        .hint_text(devin_field_hint("Only when your key requires it")),
                );
                if let Some(error) = self.devin_state.error.as_ref() {
                    ui.add_space(theme::space::MEDIUM);
                    devin_callout(ui, &error.message, theme::semantic().danger);
                }
                let connecting = self.devin_state.connection == DevinConnectionState::Connecting;
                let enabled = !connecting
                    && (self.devin_api_key.trim().starts_with("cog_")
                        || (self.devin_state.credential_source
                            == Some(CredentialSource::Environment)
                            && !self.devin_org_id.trim().is_empty()));
                ui.add_space(theme::space::MEDIUM);
                let color = if enabled {
                    theme::accent()
                } else {
                    theme::text_disabled()
                };
                let button = egui::Button::new(
                    RichText::new("Connect")
                        .font(theme::typography::strong())
                        .color(color),
                )
                .frame(false);
                if ui
                    .add_enabled_ui(enabled, |ui| {
                        ui.add_sized(egui::vec2(width, theme::control::PRIMARY), button)
                    })
                    .inner
                    .clicked()
                {
                    *action = Some(DevinUiAction::Connect);
                }
                if connecting {
                    ui.add_space(theme::space::MEDIUM);
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Validating credentials…")
                                .font(theme::typography::small())
                                .color(theme::text().muted),
                        );
                    });
                }
            });
        });
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
        // Laid right-to-left so the scope select keeps its full width and the
        // search input absorbs whatever the panel has left.
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), theme::control::STANDARD),
            Layout::right_to_left(Align::Center),
            |ui| {
                let selected_scope = self.devin_scope;
                let mut next_scope = selected_scope;
                settings_combo_box(
                    ui,
                    "devin_scope_select",
                    150.0,
                    selected_scope.label(),
                    |ui| {
                        for scope in devin_scopes() {
                            if settings_combo_choice(ui, scope.label(), selected_scope == scope) {
                                next_scope = scope;
                            }
                        }
                    },
                );
                if next_scope != selected_scope {
                    self.devin_scope = next_scope;
                    self.devin_list_cursor = 0;
                    self.devin_focus_list = true;
                }
                ui.add(
                    TextEdit::singleline(&mut self.devin_filter)
                        .id(Id::new("devin_filter"))
                        .font(theme::typography::body())
                        .hint_text(devin_field_hint("Filter sessions…"))
                        .margin(egui::Margin::symmetric(10, 7))
                        .desired_width(ui.available_width()),
                );
            },
        );
        if !self.devin_state.filters.is_empty() {
            ui.horizontal_wrapped(|ui| {
                chip(ui, "Server filters active");
                ui.label(
                    RichText::new(match self.devin_state.sessions_total {
                        Some(total) => {
                            format!("{} of {total} results", self.devin_state.sessions.len())
                        }
                        None => format!("{} loaded results", self.devin_state.sessions.len()),
                    })
                    .font(theme::typography::small())
                    .color(theme::text().muted),
                );
            });
        }
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
        ScrollArea::vertical()
            .id_salt("devin_sessions_scroll")
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
                    let pr_width = (session.pull_request_count > 0).then(|| chip_width(ui, "PR"));
                    let response = ui
                        .push_id(("devin_session", &session.id), |ui| {
                            // Content-sized: two text lines plus the frame
                            // margin, rather than a fixed height that leaves
                            // dead space under short rows.
                            selectable_content_row(ui, selected, 0.0, |ui| {
                                ui.horizontal(|ui| {
                                    let color = devin_status_dot_color(session.category);
                                    let (dot, hover) = ui.allocate_exact_size(
                                        egui::Vec2::splat(14.0),
                                        Sense::hover(),
                                    );
                                    ui.painter().circle_filled(dot.center(), 4.0, color);
                                    hover.on_hover_text(
                                        session.status_detail.as_deref().unwrap_or(&session.status),
                                    );
                                    ui.vertical(|ui| {
                                        ui.spacing_mut().item_spacing.y = theme::space::HAIR;
                                        let reserved = pr_width
                                            .map_or(0.0, |width| width + theme::space::SMALL);
                                        ui.set_width((ui.available_width() - reserved).max(48.0));
                                        let row_title = RichText::new(title)
                                            .font(theme::typography::strong())
                                            .color(theme::text().primary);
                                        ui.add(Label::new(row_title).truncate());
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
                    if let Some(width) = pr_width {
                        let badge = egui::Rect::from_center_size(
                            egui::pos2(
                                response.rect.right() - theme::space::SMALL - width * 0.5,
                                response.rect.center().y,
                            ),
                            egui::vec2(width, theme::control::COMPACT),
                        );
                        ui.painter().rect_filled(
                            badge,
                            badge.height() * 0.5,
                            theme::state::selected(),
                        );
                        ui.painter().text(
                            badge.center(),
                            Align2::CENTER_CENTER,
                            "PR",
                            theme::typography::code_small(),
                            theme::text().primary,
                        );
                    }
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
                if self.devin_state.sessions_has_next
                    && self.devin_state.sessions_cursor.is_some()
                    && ui.button("Load more sessions").clicked()
                {
                    *action = Some(DevinUiAction::LoadMoreSessions);
                }
                if self.devin_state.last_created_batch.len() > 1
                    && self.devin_state.capabilities.has("devin_session_gather")
                    && ui
                        .button(format!(
                            "Wait for {} batch sessions",
                            self.devin_state.last_created_batch.len()
                        ))
                        .clicked()
                {
                    *action = Some(DevinUiAction::GatherSessions);
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
    }

    fn draw_devin_filters(&mut self, ui: &mut egui::Ui, action: &mut Option<DevinUiAction>) {
        ScrollArea::vertical()
            .id_salt("devin_server_filters")
            .show(ui, |ui| {
                for (label, value, hint) in [
                    (
                        "Origin",
                        &mut self.devin_server_filters.origin,
                        "web, slack, api…",
                    ),
                    (
                        "Repository",
                        &mut self.devin_server_filters.repository,
                        "owner/repository",
                    ),
                    (
                        "Playbook ID",
                        &mut self.devin_server_filters.playbook_id,
                        "playbook-…",
                    ),
                    (
                        "Schedule ID",
                        &mut self.devin_server_filters.schedule_id,
                        "schedule-…",
                    ),
                    ("User ID", &mut self.devin_server_filters.user_id, "user-…"),
                    (
                        "Parent session ID",
                        &mut self.devin_server_filters.parent_session_id,
                        "session-…",
                    ),
                    (
                        "Category",
                        &mut self.devin_server_filters.category,
                        "Category",
                    ),
                    (
                        "Status",
                        &mut self.devin_server_filters.status,
                        "running, exit…",
                    ),
                    (
                        "Created after",
                        &mut self.devin_server_filters.created_after,
                        "RFC 3339 timestamp",
                    ),
                    (
                        "Created before",
                        &mut self.devin_server_filters.created_before,
                        "RFC 3339 timestamp",
                    ),
                    (
                        "Updated after",
                        &mut self.devin_server_filters.updated_after,
                        "RFC 3339 timestamp",
                    ),
                    (
                        "Updated before",
                        &mut self.devin_server_filters.updated_before,
                        "RFC 3339 timestamp",
                    ),
                ] {
                    ui.label(
                        RichText::new(label)
                            .font(theme::typography::small_strong())
                            .color(theme::text().secondary),
                    );
                    ui.add(
                        TextEdit::singleline(value)
                            .hint_text(devin_field_hint(hint))
                            .desired_width(f32::INFINITY),
                    );
                }
                ui.label(
                    RichText::new("Tags")
                        .font(theme::typography::small_strong())
                        .color(theme::text().secondary),
                );
                ui.add(
                    TextEdit::singleline(&mut self.devin_filter_tags)
                        .hint_text(devin_field_hint("Comma-separated tags"))
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(theme::space::SMALL);
                ui.horizontal(|ui| {
                    if ui.button("Apply filters").clicked() {
                        *action = Some(DevinUiAction::ApplyFilters);
                    }
                    if ui.button("Clear").clicked() {
                        *action = Some(DevinUiAction::ClearFilters);
                    }
                });
            });
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
                        .font(theme::typography::body())
                        .hint_text(devin_field_hint("owner/repository"))
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
                        .font(theme::typography::body())
                        .hint_text(devin_field_hint("Describe the task for Devin…"))
                        .desired_rows(8)
                        .desired_width(f32::INFINITY),
                );
                ui.add_space(theme::space::SNUG);
                let advanced = ui
                    .selectable_label(self.devin_advanced.open, "Advanced options")
                    .on_hover_text("Session metadata, batch, output, and permission-gated options");
                if advanced.clicked() {
                    self.devin_advanced.open = !self.devin_advanced.open;
                }
                if self.devin_advanced.open {
                    ui.indent("devin_advanced_fields", |ui| {
                        ui.label(field_label("Title"));
                        ui.add(
                            TextEdit::singleline(&mut self.devin_advanced.title)
                                .desired_width(f32::INFINITY),
                        );
                        ui.horizontal(|ui| {
                            ui.label(field_label("Mode"));
                            let selected = if self.devin_advanced.mode.is_empty() {
                                "Default"
                            } else {
                                &self.devin_advanced.mode
                            }
                            .to_owned();
                            settings_combo_box(ui, "devin_mode", 120.0, &selected, |ui| {
                                for mode in ["", "normal", "fast", "lite", "ultra", "fusion"] {
                                    let label = if mode.is_empty() { "Default" } else { mode };
                                    if settings_combo_choice(
                                        ui,
                                        label,
                                        self.devin_advanced.mode == mode,
                                    ) {
                                        self.devin_advanced.mode = mode.into();
                                    }
                                }
                            });
                            ui.label(field_label("Batch"));
                            ui.add(
                                egui::DragValue::new(&mut self.devin_advanced.batch_count)
                                    .range(1..=MAX_DEVIN_BATCH_SESSIONS),
                            );
                        });
                        for (label, value, hint) in [
                            ("Playbook ID", &mut self.devin_advanced.playbook_id, "playbook-…"),
                            (
                                "Child playbook ID",
                                &mut self.devin_advanced.child_playbook_id,
                                "playbook-…",
                            ),
                            (
                                "Knowledge IDs",
                                &mut self.devin_advanced.knowledge_ids,
                                "Comma-separated IDs",
                            ),
                            ("Tags", &mut self.devin_advanced.tags, "Comma-separated tags"),
                            (
                                "Maximum ACUs",
                                &mut self.devin_advanced.max_acu_limit,
                                "Optional positive number",
                            ),
                            ("Platform", &mut self.devin_advanced.platform, "Optional platform"),
                            (
                                "Session links",
                                &mut self.devin_advanced.session_links,
                                "Comma-separated session IDs",
                            ),
                            (
                                "Organization secret IDs",
                                &mut self.devin_advanced.secret_ids,
                                "Comma-separated IDs",
                            ),
                            (
                                "Create as user ID",
                                &mut self.devin_advanced.create_as_user_id,
                                "Permission required",
                            ),
                        ] {
                            ui.label(field_label(label));
                            ui.add(
                                TextEdit::singleline(value)
                                    .hint_text(devin_field_hint(hint))
                                    .desired_width(f32::INFINITY),
                            );
                        }
                        ui.checkbox(&mut self.devin_advanced.resumable, "Resumable session");
                        ui.checkbox(
                            &mut self.devin_advanced.bypass_approval,
                            "Bypass approval (permission required)",
                        );
                        ui.label(field_label("Structured output JSON Schema"));
                        ui.add(
                            TextEdit::multiline(&mut self.devin_advanced.structured_output_schema)
                                .font(theme::typography::code_small())
                                .desired_rows(5)
                                .desired_width(f32::INFINITY),
                        );
                        ui.checkbox(
                            &mut self.devin_advanced.structured_output_required,
                            "Structured output required",
                        );
                        ui.label(field_label("Ephemeral session secret"));
                        ui.horizontal(|ui| {
                            ui.add(
                                TextEdit::singleline(&mut self.devin_advanced.session_secret_key)
                                    .hint_text(devin_field_hint("Name")),
                            );
                            ui.add(
                                TextEdit::singleline(&mut self.devin_advanced.session_secret_value)
                                    .password(true)
                                    .hint_text(devin_field_hint("Value")),
                            );
                        });
                        ui.label(
                            RichText::new("The value is cleared as soon as the request is submitted and is never retained for retry.")
                                .font(theme::typography::small())
                                .color(theme::text().muted),
                        );
                    });
                }
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Attach files").clicked() {
                        self.open_devin_file_picker();
                    }
                    for attachment in &self.devin_attachments {
                        chip(
                            ui,
                            &attachment
                                .file
                                .path()
                                .file_name()
                                .unwrap_or_default()
                                .to_string_lossy(),
                        );
                    }
                });
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
                    let (fill, color) = assistant_send_button_colors(can_create);
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
            draw_assistant_connecting(
                ui,
                "Loading Devin session",
                "Fetching conversation and actions…",
                None,
                paint_devin_icon,
            );
            return;
        }
        let terminal = summary.archived
            || matches!(
                summary.category,
                StatusCategory::Completed | StatusCategory::Failed
            );
        let composer_enabled = !terminal && !self.devin_state.busy;
        let mut composer_height = if terminal {
            56.0
        } else {
            measure_assistant_composer_height(
                ui,
                &self.devin_message,
                ui.max_rect().width(),
                ui.max_rect().height(),
                !self.devin_attachments.is_empty(),
            )
        };
        if !terminal {
            let available = ui
                .available_rect_before_wrap()
                .expand2(egui::vec2(theme::space::MEDIUM, theme::space::SMALL));
            let drop_panel =
                available.with_min_y((available.bottom() - composer_height).max(available.top()));
            let had_attachments = !self.devin_attachments.is_empty();
            if let Some(error) = AssistantComposer::stage_drop(
                ui,
                drop_panel,
                composer_enabled,
                &mut self.devin_attachments,
                &mut self.devin_drop_hovered,
                false,
            ) {
                self.show_error(error);
            }
            if !had_attachments && !self.devin_attachments.is_empty() {
                composer_height += ASSISTANT_ATTACHMENT_ROW_HEIGHT;
            }
        }
        let stream_height = (ui.available_height() - composer_height).max(100.0);
        ScrollArea::vertical()
            .id_salt(("devin_stream", &summary.id))
            .stick_to_bottom(true)
            .max_height(stream_height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.draw_devin_detail_summary(ui);
                self.draw_devin_stream(ui, action);
            });
        if summary.category == StatusCategory::WaitingApproval {
            let url = summary.url.as_deref().unwrap_or(DEVIN_WEB);
            devin_callout(
                ui,
                "Needs approval — approve or reject the action in Devin.",
                theme::semantic().warning,
            );
            ui.hyperlink_to("Open Devin to review", url);
            return;
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
        let hint = if summary.category == StatusCategory::Sleeping {
            "Message Devin — sending will wake this session"
        } else {
            "Message Devin"
        };
        ui.add_space(theme::space::SNUG);
        let divider = ui.cursor().top();
        ui.painter().hline(
            ui.max_rect().expand(theme::space::MEDIUM).x_range(),
            divider,
            egui::Stroke::new(1.0, theme::border::hairline_color()),
        );
        ui.add_space(theme::space::SMALL);
        let can_send = composer_enabled
            && (!self.devin_message.trim().is_empty() || !self.devin_attachments.is_empty());
        let focus = self.devin_focus_detail && summary.category == StatusCategory::Waiting;
        self.devin_focus_detail = false;
        let output = (AssistantComposer {
            panel: ui
                .available_rect_before_wrap()
                .expand2(egui::vec2(theme::space::MEDIUM, theme::space::SMALL)),
            prompt_id: Id::new("devin_prompt"),
            attach_id: Id::new("devin_attach"),
            scroll_id: Id::new("devin_prompt_scroll"),
            hint,
            attach_tooltip: "Attach files",
            drop_hint: "Drop files to attach",
            enabled: composer_enabled,
            send_enabled: can_send,
            active: false,
            allow_directories: false,
            handle_drop: false,
            mouse_wheel: true,
            focus,
            radius: 0.0,
        })
        .show(
            ui,
            &mut self.devin_message,
            &mut self.devin_attachments,
            &mut self.devin_drop_hovered,
        );
        if let Some(error) = output.error {
            self.show_error(error);
        }
        if output.open_file_picker {
            self.open_devin_file_picker();
            ui.ctx().request_repaint();
        }
        if (output.send || output.submit) && can_send {
            *action = Some(DevinUiAction::SendMessage);
        }
    }

    fn draw_devin_detail_summary(&mut self, ui: &mut egui::Ui) {
        let Some(detail) = self.devin_state.detail.as_ref() else {
            return;
        };
        let summary = &detail.summary;
        if let Some(insight) = self
            .devin_state
            .resources
            .insights
            .items
            .iter()
            .find(|insight| insight.session_id == summary.id)
        {
            resource_card(
                ui,
                "Session insight",
                Some(&insight.status),
                insight
                    .summary
                    .as_deref()
                    .unwrap_or("Analysis is still being generated."),
            );
        }
        if !detail.attachments.is_empty() {
            ui.add_space(theme::space::SNUG);
            ui.label(
                RichText::new("ATTACHMENTS")
                    .size(theme::typography::MICRO_SIZE)
                    .strong()
                    .color(theme::text().muted),
            );
            ui.horizontal_wrapped(|ui| {
                for attachment in &detail.attachments {
                    let response = devin_attachment_pill(ui, attachment);
                    if response.clicked()
                        && let Some(url) = attachment.url.as_deref()
                    {
                        ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                    }
                }
            });
        }
        if let Some(output) = summary.structured_output.as_ref() {
            let output = devin_json_preview(output);
            ui.add_space(theme::space::SNUG);
            ui.label(
                RichText::new("STRUCTURED OUTPUT")
                    .size(theme::typography::MICRO_SIZE)
                    .strong()
                    .color(theme::text().muted),
            );
            ui.add(
                Label::new(
                    RichText::new(output)
                        .font(theme::typography::code_small())
                        .color(theme::text().secondary),
                )
                .selectable(true)
                .wrap(),
            );
        }
        ui.add_space(theme::space::SMALL);
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
                    self.draw_devin_message(ui, &self.devin_state.messages[message], action);
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
                    let group_id = self.devin_state.activity[first].id.as_str();
                    let count = index - start;
                    // The Agent transcript's dense work anatomy: one disclosure
                    // row over per-action tool rows, so grouped remote work
                    // reads exactly like grouped local work.
                    let open = assistant_dense_disclosure_row(
                        ui,
                        Id::new(("devin_activity_group", group_id)),
                        &format!("{count} remote action{}", if count == 1 { "" } else { "s" }),
                        None,
                        false,
                        false,
                    );
                    if open {
                        for (_, kind) in &stream[start..index] {
                            let Kind::Activity(activity) = kind else {
                                continue;
                            };
                            draw_devin_activity(ui, &self.devin_state.activity[*activity], action);
                        }
                    }
                }
            }
        }
        if let Some(pending) = self.devin_pending_message.as_ref() {
            if !stream.is_empty() {
                ui.add_space(theme::space::SMALL);
            }
            chat_user_message(ui, &pending.text, None, |ui| {
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

    /// One Devin turn, rendered with the Agent transcript's exact anatomy:
    /// the shared bubble for the human, the shared identity line and markdown
    /// pipeline for the assistant, and the Agent's image previews for
    /// attachments whose bytes have arrived.
    fn draw_devin_message(
        &self,
        ui: &mut egui::Ui,
        message: &DevinMessage,
        action: &mut Option<DevinUiAction>,
    ) {
        if message.role.eq_ignore_ascii_case("user") {
            chat_user_message(ui, &message.text, None, |ui| {
                self.draw_devin_message_attachments(ui, message, true, action);
            });
        } else {
            let identity = if message.role.eq_ignore_ascii_case("devin")
                || message.role.eq_ignore_ascii_case("devin_ai")
                || message.role.eq_ignore_ascii_case("assistant")
            {
                "Devin"
            } else {
                message.role.as_str()
            };
            chat_identity(ui, identity, None, paint_devin_icon);
            let width = ui.available_width();
            let galley = assistant_markdown_galley(
                ui,
                Id::new(("devin_markdown", message.id.as_str())),
                &message.text,
                width,
                &self.highlighter,
                &self.syntaxes,
                false,
                None,
            );
            ui.add(Label::new(galley).wrap());
            self.draw_devin_message_attachments(ui, message, false, action);
        }
    }

    /// Attachments on a turn: images that have bytes render as the Agent's
    /// previews — square in the prompt bubble, thumbnail elsewhere — and
    /// everything else as the 48 px tile, which opens its remote link.
    fn draw_devin_message_attachments(
        &self,
        ui: &mut egui::Ui,
        message: &DevinMessage,
        in_bubble: bool,
        action: &mut Option<DevinUiAction>,
    ) {
        let Some(detail) = self.devin_state.detail.as_ref() else {
            return;
        };
        if message.attachment_ids.is_empty() {
            return;
        }
        if !message.text.is_empty() {
            ui.add_space(if in_bubble {
                theme::space::MEDIUM
            } else {
                theme::space::TIGHT
            });
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
                let bytes = self.devin_state.attachment_previews.get(id);
                let preview = bytes.and_then(|bytes| {
                    if in_bubble {
                        assistant_prompt_image_preview(ui, bytes)
                    } else {
                        assistant_embedded_image_preview(ui, bytes)
                    }
                });
                if let Some(preview) = preview {
                    if preview.clicked() {
                        *action = Some(DevinUiAction::OpenImage(id.clone()));
                    }
                    continue;
                }
                let pill = devin_attachment_pill(ui, attachment);
                if pill.clicked()
                    && let Some(url) = attachment.url.as_deref()
                {
                    ui.ctx().open_url(egui::OpenUrl::new_tab(url));
                }
            }
        });
    }

    fn draw_devin_section(
        &mut self,
        ui: &mut egui::Ui,
        section: DevinSection,
        action: &mut Option<DevinUiAction>,
    ) {
        ScrollArea::vertical()
            .id_salt(("devin_section", section))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                match section {
                    DevinSection::Review => {
                        ui.label(
                            RichText::new("Pull or merge request URL")
                                .font(theme::typography::small_strong())
                                .color(theme::text().secondary),
                        );
                        ui.add(
                            TextEdit::singleline(&mut self.devin_resource_query)
                                .id(Id::new("devin_resource_query"))
                                .hint_text(devin_field_hint("https://github.com/…/pull/…"))
                                .desired_width(f32::INFINITY),
                        );
                        ui.label(
                            RichText::new(
                                "Triggering a review can incur Devin usage and always requires confirmation.",
                            )
                            .font(theme::typography::small())
                            .color(theme::text().muted),
                        );
                        if ui
                            .add_enabled(
                                !self.devin_resource_query.trim().is_empty(),
                                egui::Button::new("Trigger review"),
                            )
                            .clicked()
                        {
                            *action = Some(DevinUiAction::ConfirmResourceMutation(
                                ResourceMutation::TriggerReview {
                                    pull_request_url: self.devin_resource_query.trim().into(),
                                },
                            ));
                        }
                        for review in &self.devin_state.resources.reviews.items {
                            ui.separator();
                            ui.label(format!("{} · {}", review.status, review.pull_request_url));
                            ui.hyperlink_to("Open pull request", &review.pull_request_url);
                            if let Some(url) = review.result_url.as_deref() {
                                ui.hyperlink_to("Open review result", url);
                            }
                        }
                    }
                    DevinSection::Repositories => {
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.repositories.status,
                            self.devin_state.resources.repositories.error.as_deref(),
                        );
                        ui.add(
                            TextEdit::singleline(&mut self.devin_resource_filter)
                                .id(Id::new("devin_resource_filter"))
                                .hint_text(devin_field_hint("Search repositories"))
                                .desired_width(f32::INFINITY),
                        );
                        let filter = self.devin_resource_filter.trim().to_ascii_lowercase();
                        for repository in self
                            .devin_state
                            .resources
                            .repositories
                            .items
                            .iter()
                            .filter(|repository| {
                                filter.is_empty()
                                    || repository.name.to_ascii_lowercase().contains(&filter)
                            })
                        {
                            ui.horizontal_wrapped(|ui| {
                                if ui.button(&repository.name).clicked() {
                                    self.devin_resource_draft.id = repository.id.clone();
                                    *action = Some(DevinUiAction::LoadRepositoryWiki(
                                        repository.name.clone(),
                                    ));
                                }
                                ui.label(
                                    RichText::new(if repository.indexed {
                                        "Indexed"
                                    } else {
                                        "Not indexed"
                                    })
                                    .font(theme::typography::small())
                                    .color(theme::text().muted),
                                );
                                if let Some(status) = repository.indexing_status.as_deref() {
                                    chip(ui, status);
                                }
                                if ui
                                    .small_button(if repository.indexed {
                                        "Re-index"
                                    } else {
                                        "Index"
                                    })
                                    .clicked()
                                {
                                    *action = Some(DevinUiAction::MutateResource(
                                        ResourceMutation::IndexRepository {
                                            repository: repository.id.clone(),
                                            branches: split_devin_values(
                                                &self.devin_resource_draft.extra,
                                            ),
                                        },
                                    ));
                                }
                                if repository.indexed && ui.small_button("Remove index").clicked() {
                                    *action = Some(DevinUiAction::ConfirmResourceMutation(
                                        ResourceMutation::RemoveRepositoryIndex {
                                            repository: repository.id.clone(),
                                        },
                                    ));
                                }
                            });
                        }
                        ui.label(
                            RichText::new("Branches")
                                .font(theme::typography::small_strong())
                                .color(theme::text().secondary),
                        );
                        ui.add(
                            TextEdit::singleline(&mut self.devin_resource_draft.extra)
                                .hint_text(devin_field_hint(
                                    "Optional comma-separated branches for indexing",
                                ))
                                .desired_width(f32::INFINITY),
                        );
                        if !self.devin_repository.trim().is_empty()
                            && split_devin_values(&self.devin_resource_draft.extra).len() == 1
                            && ui.small_button("Remove branch from index").clicked()
                        {
                            *action = Some(DevinUiAction::ConfirmResourceMutation(
                                ResourceMutation::RemoveRepositoryBranch {
                                    repository: optional_devin_value(
                                        &self.devin_resource_draft.id,
                                    )
                                    .unwrap_or_else(|| self.devin_repository.trim().into()),
                                    branch: self.devin_resource_draft.extra.trim().into(),
                                },
                            ));
                        }
                        ui.add_space(theme::space::SNUG);
                        ui.label(
                            RichText::new("Ask repositories")
                                .font(theme::typography::small_strong())
                                .color(theme::text().secondary),
                        );
                        ui.add(
                            TextEdit::singleline(&mut self.devin_repository)
                                .id(Id::new("devin_repository"))
                                .hint_text(devin_field_hint(
                                    "Up to 10 owner/repository names, comma-separated",
                                ))
                                .desired_width(f32::INFINITY),
                        );
                        ui.add(
                            TextEdit::multiline(&mut self.devin_resource_query)
                                .id(Id::new("devin_resource_query"))
                                .hint_text(devin_field_hint("Ask Devin about the selected repository…"))
                                .desired_rows(3)
                                .desired_width(f32::INFINITY),
                        );
                        if ui
                            .add_enabled(
                                (1..=10)
                                    .contains(&split_devin_values(&self.devin_repository).len())
                                    && !self.devin_resource_query.trim().is_empty(),
                                egui::Button::new("Ask Devin"),
                            )
                            .clicked()
                        {
                            *action = Some(DevinUiAction::AskRepository);
                        }
                        for document in &self.devin_state.resources.documents.items {
                            ui.separator();
                            ui.label(
                                RichText::new(&document.title)
                                    .font(theme::typography::small_strong())
                                    .color(theme::text().primary),
                            );
                            if !document.content.is_empty() {
                                ui.add(Label::new(&document.content).selectable(true).wrap());
                            }
                            for citation in &document.citations {
                                ui.label(
                                    RichText::new(citation)
                                        .font(theme::typography::code_small())
                                        .color(theme::text().muted),
                                );
                            }
                        }
                    }
                    DevinSection::Knowledge => {
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.knowledge.status,
                            self.devin_state.resources.knowledge.error.as_deref(),
                        );
                        ui.add(
                            TextEdit::singleline(&mut self.devin_resource_query)
                                .id(Id::new("devin_resource_query"))
                                .hint_text(devin_field_hint("Search notes or folders"))
                                .desired_width(f32::INFINITY),
                        );
                        for folder in &self.devin_state.resources.knowledge_folders.items {
                            ui.label(format!("{} · {} notes", folder.name, folder.note_count));
                        }
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.knowledge_folders.status,
                            self.devin_state.resources.knowledge_folders.error.as_deref(),
                        );
                        let query = self.devin_resource_query.trim().to_ascii_lowercase();
                        for note in self
                            .devin_state
                            .resources
                            .knowledge
                            .items
                            .iter()
                            .filter(|note| {
                                query.is_empty()
                                    || note.name.to_ascii_lowercase().contains(&query)
                                    || note.content.to_ascii_lowercase().contains(&query)
                                    || note.repositories.iter().any(|repository| {
                                        repository.to_ascii_lowercase().contains(&query)
                                    })
                                    || note
                                        .folder
                                        .as_deref()
                                        .is_some_and(|folder| {
                                            folder.to_ascii_lowercase().contains(&query)
                                        })
                            })
                        {
                            resource_card(ui, &note.name, note.folder.as_deref(), &note.content);
                            if ui.small_button("Edit").clicked() {
                                *action = Some(DevinUiAction::LoadKnowledge(note.id.clone()));
                            }
                        }
                        if !self
                            .devin_state
                            .resources
                            .knowledge_suggestions
                            .items
                            .is_empty()
                        {
                            ui.label(
                                RichText::new("SUGGESTIONS")
                                    .size(theme::typography::MICRO_SIZE)
                                    .strong()
                                    .color(theme::text().muted),
                            );
                            for suggestion in &self
                                .devin_state
                                .resources
                                .knowledge_suggestions
                                .items
                            {
                                resource_card(
                                    ui,
                                    &suggestion.title,
                                    None,
                                    &suggestion.content,
                                );
                                ui.horizontal_wrapped(|ui| {
                                    if ui.small_button("View").clicked() {
                                        *action = Some(
                                            DevinUiAction::LoadKnowledgeSuggestion(
                                                suggestion.id.clone(),
                                            ),
                                        );
                                    }
                                    if ui.small_button("Dismiss").clicked() {
                                        *action = Some(DevinUiAction::ConfirmResourceMutation(
                                            ResourceMutation::DismissKnowledgeSuggestion {
                                                id: suggestion.id.clone(),
                                            },
                                        ));
                                    }
                                });
                            }
                        }
                        resource_editor_fields(
                            ui,
                            &mut self.devin_resource_draft,
                            "Note name",
                            "Folder",
                            "Knowledge content",
                        );
                        ui.horizontal_wrapped(|ui| {
                            for (label, crud) in [
                                ("Create", CrudAction::Create),
                                ("Update", CrudAction::Update),
                                ("Delete", CrudAction::Delete),
                            ] {
                                if ui.button(label).clicked() {
                                    let mutation = ResourceMutation::Knowledge {
                                        action: crud,
                                        id: optional_devin_value(&self.devin_resource_draft.id),
                                        name: self.devin_resource_draft.name.trim().into(),
                                        content: self.devin_resource_draft.content.clone(),
                                        folder: optional_devin_value(
                                            &self.devin_resource_draft.folder_or_repository,
                                        ),
                                    };
                                    *action = Some(if crud == CrudAction::Delete {
                                        DevinUiAction::ConfirmResourceMutation(mutation)
                                    } else {
                                        DevinUiAction::MutateResource(mutation)
                                    });
                                }
                            }
                        });
                    }
                    DevinSection::Playbooks => {
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.playbooks.status,
                            self.devin_state.resources.playbooks.error.as_deref(),
                        );
                        for playbook in &self.devin_state.resources.playbooks.items {
                            resource_card(
                                ui,
                                &playbook.title,
                                playbook.automation_macro.as_deref(),
                                &playbook.content,
                            );
                            if ui.small_button("Edit").clicked() {
                                *action = Some(DevinUiAction::LoadPlaybook(playbook.id.clone()));
                            }
                        }
                        resource_editor_fields(
                            ui,
                            &mut self.devin_resource_draft,
                            "Playbook title",
                            "Automation macro",
                            "Playbook content",
                        );
                        ui.horizontal_wrapped(|ui| {
                            for (label, crud) in [
                                ("Create", CrudAction::Create),
                                ("Update", CrudAction::Update),
                                ("Delete", CrudAction::Delete),
                            ] {
                                if ui.button(label).clicked() {
                                    let mutation = ResourceMutation::Playbook {
                                        action: crud,
                                        id: optional_devin_value(&self.devin_resource_draft.id),
                                        title: self.devin_resource_draft.name.trim().into(),
                                        content: self.devin_resource_draft.content.clone(),
                                        automation_macro: optional_devin_value(
                                            &self.devin_resource_draft.folder_or_repository,
                                        ),
                                    };
                                    *action = Some(if crud == CrudAction::Delete {
                                        DevinUiAction::ConfirmResourceMutation(mutation)
                                    } else {
                                        DevinUiAction::MutateResource(mutation)
                                    });
                                }
                            }
                        });
                    }
                    DevinSection::Automations => {
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.automations.status,
                            self.devin_state.resources.automations.error.as_deref(),
                        );
                        ui.label(
                            RichText::new("SCHEDULES")
                                .size(theme::typography::MICRO_SIZE)
                                .strong()
                                .color(theme::text().muted),
                        );
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.schedules.status,
                            self.devin_state.resources.schedules.error.as_deref(),
                        );
                        for schedule in &self.devin_state.resources.schedules.items {
                            resource_card(
                                ui,
                                &schedule.title,
                                Some(if schedule.enabled { "Enabled" } else { "Disabled" }),
                                &format!("{}\n{}", schedule.cadence, schedule.prompt),
                            );
                            if ui.small_button("Edit schedule").clicked() {
                                *action = Some(DevinUiAction::LoadSchedule(schedule.id.clone()));
                            }
                        }
                        resource_json_editor(ui, &mut self.devin_resource_draft, "Schedule JSON");
                        resource_crud_buttons(
                            ui,
                            "schedule",
                            &self.devin_resource_draft,
                            action,
                            |crud, id, payload| {
                            ResourceMutation::Schedule {
                                action: crud,
                                id,
                                payload,
                            }
                        },
                        );
                        ui.label(
                            RichText::new("EVENT AUTOMATIONS")
                                .size(theme::typography::MICRO_SIZE)
                                .strong()
                                .color(theme::text().muted),
                        );
                        if let Some(catalog) = self
                            .devin_state
                            .resources
                            .automation_catalog
                            .items
                            .first()
                        {
                            egui::CollapsingHeader::new("Server schemas and templates")
                                .show(ui, |ui| {
                                    for (label, value) in [
                                        ("Schemas", &catalog.schemas),
                                        ("Templates", &catalog.templates),
                                    ] {
                                        ui.label(
                                            RichText::new(label)
                                                .font(theme::typography::small_strong())
                                                .color(theme::text().secondary),
                                        );
                                        ui.add(
                                            Label::new(
                                                RichText::new(
                                                    devin_json_preview(value),
                                                )
                                                .font(theme::typography::code_small()),
                                            )
                                            .selectable(true)
                                            .wrap(),
                                        );
                                    }
                                });
                        }
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.automation_catalog.status,
                            self.devin_state.resources.automation_catalog.error.as_deref(),
                        );
                        for automation in &self.devin_state.resources.automations.items {
                            resource_card(
                                ui,
                                &automation.title,
                                Some(if automation.enabled { "Enabled" } else { "Disabled" }),
                                &automation.summary,
                            );
                            if ui.small_button("Edit automation").clicked() {
                                *action =
                                    Some(DevinUiAction::LoadAutomation(automation.id.clone()));
                            }
                        }
                        resource_json_editor(
                            ui,
                            &mut self.devin_resource_draft,
                            "Automation JSON (use the server schemas/templates above)",
                        );
                        resource_crud_buttons(
                            ui,
                            "automation",
                            &self.devin_resource_draft,
                            action,
                            |crud, id, payload| ResourceMutation::Automation {
                                action: crud,
                                id,
                                payload,
                            },
                        );
                    }
                    DevinSection::Environment => {
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.blueprints.status,
                            self.devin_state.resources.blueprints.error.as_deref(),
                        );
                        ui.label(
                            RichText::new("BLUEPRINTS")
                                .size(theme::typography::MICRO_SIZE)
                                .strong()
                                .color(theme::text().muted),
                        );
                        for blueprint in &self.devin_state.resources.blueprints.items {
                            resource_card(
                                ui,
                                &blueprint.name,
                                blueprint.repository.as_deref(),
                                &blueprint.id,
                            );
                            if ui.small_button("Edit blueprint").clicked() {
                                self.devin_resource_draft.folder_or_repository =
                                    blueprint.repository.clone().unwrap_or_default();
                                *action = Some(DevinUiAction::LoadBlueprint(
                                    blueprint.id.clone(),
                                ));
                            }
                        }
                        resource_editor_fields(
                            ui,
                            &mut self.devin_resource_draft,
                            "Blueprint name (display only)",
                            "owner/repository, blank for organization",
                            "Blueprint YAML",
                        );
                        ui.horizontal_wrapped(|ui| {
                            for (label, crud) in [
                                ("Create", CrudAction::Create),
                                ("Update", CrudAction::Update),
                                ("Delete", CrudAction::Delete),
                            ] {
                                if ui.button(label).clicked() {
                                    let mutation = ResourceMutation::Blueprint {
                                        action: crud,
                                        id: optional_devin_value(&self.devin_resource_draft.id),
                                        repository: optional_devin_value(
                                            &self.devin_resource_draft.folder_or_repository,
                                        ),
                                        contents: self.devin_resource_draft.content.clone(),
                                    };
                                    *action = Some(if crud == CrudAction::Delete {
                                        DevinUiAction::ConfirmResourceMutation(mutation)
                                    } else {
                                        DevinUiAction::MutateResource(mutation)
                                    });
                                }
                            }
                        });
                        if !self.devin_resource_draft.id.trim().is_empty() {
                            ui.label(
                                RichText::new("BLUEPRINT FILES")
                                    .size(theme::typography::MICRO_SIZE)
                                    .strong()
                                    .color(theme::text().muted),
                            );
                            for file in &self.devin_state.resources.blueprint_files.items {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(&file.name);
                                    if ui.small_button("Delete").clicked() {
                                        *action = Some(
                                            DevinUiAction::ConfirmResourceMutation(
                                                ResourceMutation::DeleteBlueprintFile {
                                                    blueprint_id: self
                                                        .devin_resource_draft
                                                        .id
                                                        .trim()
                                                        .into(),
                                                    file_id: file.id.clone(),
                                                },
                                            ),
                                        );
                                    }
                                });
                            }
                            ui.horizontal_wrapped(|ui| {
                                if ui.button("Choose blueprint files").clicked() {
                                    self.open_devin_file_picker();
                                }
                                if !self.devin_attachments.is_empty()
                                    && ui.button("Upload staged files").clicked()
                                {
                                    *action = Some(DevinUiAction::UploadBlueprintFiles);
                                }
                            });
                        }
                        ui.label(
                            RichText::new("BUILDS")
                                .size(theme::typography::MICRO_SIZE)
                                .strong()
                                .color(theme::text().muted),
                        );
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.builds.status,
                            self.devin_state.resources.builds.error.as_deref(),
                        );
                        for build in &self.devin_state.resources.builds.items {
                            resource_card(
                                ui,
                                &build.id,
                                Some(&build.status),
                                build.created_at.as_deref().unwrap_or_default(),
                            );
                            if let Some(url) = build.logs_url.as_deref() {
                                ui.hyperlink_to("Open build logs", url);
                            } else if ui.small_button("Load logs").clicked() {
                                *action = Some(DevinUiAction::LoadBuildLogs(build.id.clone()));
                            }
                            if ui.small_button("Load details").clicked() {
                                *action = Some(DevinUiAction::LoadBuild(build.id.clone()));
                            }
                            egui::CollapsingHeader::new("Build details")
                                .id_salt(("devin_build_details", &build.id))
                                .show(ui, |ui| {
                                    ui.add(
                                        Label::new(
                                            RichText::new(devin_json_preview(&build.configuration))
                                                .font(theme::typography::code_small()),
                                        )
                                        .selectable(true)
                                        .wrap(),
                                    );
                                });
                            ui.horizontal_wrapped(|ui| {
                                if matches!(build.status.as_str(), "pending" | "running")
                                    && ui.small_button("Cancel").clicked()
                                {
                                    *action = Some(DevinUiAction::ConfirmResourceMutation(
                                        ResourceMutation::CancelBuild {
                                            id: build.id.clone(),
                                        },
                                    ));
                                }
                                if ui
                                    .small_button(if build.pinned { "Unpin" } else { "Pin" })
                                    .clicked()
                                {
                                    *action = Some(DevinUiAction::MutateResource(
                                        ResourceMutation::PinBuild {
                                            id: build.id.clone(),
                                            pinned: !build.pinned,
                                        },
                                    ));
                                }
                            });
                        }
                        if ui.button("Trigger snapshot build").clicked() {
                            *action = Some(DevinUiAction::ConfirmResourceMutation(
                                ResourceMutation::TriggerBuild,
                            ));
                        }
                        ui.label(
                            RichText::new("SECRETS")
                                .size(theme::typography::MICRO_SIZE)
                                .strong()
                                .color(theme::text().muted),
                        );
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.secrets.status,
                            self.devin_state.resources.secrets.error.as_deref(),
                        );
                        for secret in &self.devin_state.resources.secrets.items {
                            resource_card(ui, &secret.name, secret.scope.as_deref(), &secret.id);
                            if ui.small_button("Delete secret").clicked() {
                                *action = Some(DevinUiAction::ConfirmResourceMutation(
                                    ResourceMutation::DeleteSecret {
                                        id: secret.id.clone(),
                                    },
                                ));
                            }
                        }
                        ui.separator();
                        ui.add(
                            TextEdit::singleline(&mut self.devin_resource_draft.name)
                                .hint_text(devin_field_hint("Secret name"))
                                .desired_width(f32::INFINITY),
                        );
                        ui.add(
                            TextEdit::singleline(&mut self.devin_resource_draft.secret_value)
                                .password(true)
                                .hint_text(devin_field_hint("Secret value"))
                                .desired_width(f32::INFINITY),
                        );
                        ui.label(
                            RichText::new("Secret values are write-only and cleared immediately on submission.")
                                .font(theme::typography::small())
                                .color(theme::text().muted),
                        );
                        if ui
                            .add_enabled(
                                !self.devin_resource_draft.name.trim().is_empty()
                                    && !self.devin_resource_draft.secret_value.is_empty(),
                                egui::Button::new("Create secret"),
                            )
                            .clicked()
                        {
                            *action = Some(DevinUiAction::MutateResource(
                                ResourceMutation::CreateSecret(SecretInput {
                                    key: self.devin_resource_draft.name.trim().into(),
                                    value: std::mem::take(
                                        &mut self.devin_resource_draft.secret_value,
                                    ),
                                    kind: "key-value".into(),
                                }),
                            ));
                        }
                    }
                    DevinSection::Integrations => {
                        draw_resource_status(
                            ui,
                            self.devin_state.resources.integrations.status,
                            self.devin_state.resources.integrations.error.as_deref(),
                        );
                        let selected = match self.devin_resource_query.as_str() {
                            "installed" => "Installed",
                            "not_installed" => "Not installed",
                            _ => "All integrations",
                        };
                        settings_combo_box(
                            ui,
                            "devin_integration_filter",
                            ui.available_width(),
                            selected,
                            |ui| {
                                for (value, label) in [
                                    ("", "All integrations"),
                                    ("installed", "Installed"),
                                    ("not_installed", "Not installed"),
                                ] {
                                    if settings_combo_choice(
                                        ui,
                                        label,
                                        self.devin_resource_query == value,
                                    ) {
                                        self.devin_resource_query = value.into();
                                    }
                                }
                            },
                        );
                        for integration in self
                            .devin_state
                            .resources
                            .integrations
                            .items
                            .iter()
                            .filter(|integration| match self.devin_resource_query.as_str() {
                                "installed" => integration.installed,
                                "not_installed" => !integration.installed,
                                _ => true,
                            })
                        {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(&integration.name)
                                        .font(theme::typography::small_strong())
                                        .color(theme::text().primary),
                                );
                                chip(
                                    ui,
                                    if integration.installed {
                                        "Installed"
                                    } else {
                                        "Not installed"
                                    },
                                );
                                if let Some(url) = integration.url.as_deref() {
                                    ui.hyperlink_to(
                                        if integration.installed { "Settings" } else { "Set up" },
                                        url,
                                    );
                                }
                            });
                        }
                    }
                }
            });
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
            #[cfg(debug_assertions)]
            DevinUiAction::SeedPreview => {
                self.devin_controller = None;
                self.devin_state.seed_preview();
                self.devin_view = DevinView::Sessions;
                self.devin_scope = DevinScope::Active;
                self.devin_filter.clear();
                self.devin_list_cursor = 0;
                self.devin_focus_list = true;
                self.devin_pending_message = None;
                self.devin_pending_lifecycle = None;
                ctx.request_repaint();
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
            DevinUiAction::OpenFilters => {
                self.devin_server_filters = self.devin_state.filters.clone();
                self.devin_filter_tags = self.devin_server_filters.tags.join(", ");
                self.devin_view = DevinView::Filters;
                return;
            }
            DevinUiAction::ApplyFilters => {
                self.devin_server_filters.tags = split_devin_values(&self.devin_filter_tags);
                self.devin_view = DevinView::Sessions;
                Some(DevinCommand::SetSessionFilters(
                    self.devin_server_filters.clone(),
                ))
            }
            DevinUiAction::ClearFilters => {
                self.devin_server_filters = SessionFilters::default();
                self.devin_filter_tags.clear();
                self.devin_view = DevinView::Sessions;
                Some(DevinCommand::SetSessionFilters(SessionFilters::default()))
            }
            DevinUiAction::OpenSection(section) => {
                self.devin_view = DevinView::Section(section);
                if devin_section_available(&self.devin_state, section) {
                    Some(DevinCommand::LoadSection(section))
                } else {
                    self.devin_state
                        .apply(DevinEvent::ResourceForbidden(section));
                    return;
                }
            }
            DevinUiAction::LoadKnowledge(id) => Some(DevinCommand::LoadKnowledge(id)),
            DevinUiAction::LoadKnowledgeSuggestion(id) => {
                Some(DevinCommand::LoadKnowledgeSuggestion(id))
            }
            DevinUiAction::LoadPlaybook(id) => Some(DevinCommand::LoadPlaybook(id)),
            DevinUiAction::LoadSchedule(id) => Some(DevinCommand::LoadSchedule(id)),
            DevinUiAction::LoadAutomation(id) => Some(DevinCommand::LoadAutomation(id)),
            DevinUiAction::LoadRepositoryWiki(repository) => {
                self.devin_repository = repository.clone();
                Some(DevinCommand::LoadRepositoryWiki { repository })
            }
            DevinUiAction::AskRepository => Some(DevinCommand::AskRepositories {
                repositories: split_devin_values(&self.devin_repository),
                question: self.devin_resource_query.trim().to_owned(),
            }),
            DevinUiAction::GenerateInsight => Some(DevinCommand::GenerateInsight),
            DevinUiAction::MutateResource(mutation) => Some(DevinCommand::MutateResource(mutation)),
            DevinUiAction::ConfirmResourceMutation(mutation) => {
                self.devin_confirm_mutation = Some(mutation);
                return;
            }
            DevinUiAction::ReplaceTags => Some(DevinCommand::ReplaceTags(split_devin_values(
                &self.devin_tags,
            ))),
            DevinUiAction::AppendTags => Some(DevinCommand::AppendTags(split_devin_values(
                &self.devin_tags,
            ))),
            DevinUiAction::SearchActivity => Some(DevinCommand::SearchActivity(
                self.devin_activity_query.trim().into(),
            )),
            DevinUiAction::FetchActivity(event_id) => Some(DevinCommand::FetchActivity(event_id)),
            DevinUiAction::GatherSessions => Some(DevinCommand::GatherSessions(
                self.devin_state.last_created_batch.clone(),
            )),
            DevinUiAction::LaunchChild(parent_session_id) => {
                self.devin_view = DevinView::Create;
                self.devin_advanced.open = true;
                self.devin_advanced.parent_session_id = parent_session_id;
                self.devin_focus_create = true;
                return;
            }
            DevinUiAction::LoadBlueprint(blueprint_id) => {
                self.devin_resource_draft.id = blueprint_id.clone();
                Some(DevinCommand::LoadBlueprint(blueprint_id))
            }
            DevinUiAction::UploadBlueprintFiles => {
                let attachments = self
                    .devin_attachments
                    .drain(..)
                    .map(|attachment| attachment.file)
                    .collect();
                Some(DevinCommand::UploadBlueprintFiles {
                    blueprint_id: self.devin_resource_draft.id.trim().into(),
                    attachments,
                })
            }
            DevinUiAction::LoadBuildLogs(build_id) => Some(DevinCommand::LoadBuildLogs(build_id)),
            DevinUiAction::LoadBuild(build_id) => Some(DevinCommand::LoadBuild(build_id)),
            DevinUiAction::Connect
                if self.devin_api_key.trim().is_empty()
                    && self.devin_state.credential_source
                        == Some(CredentialSource::Environment) =>
            {
                Some(DevinCommand::SelectOrganization(
                    self.devin_org_id.trim().into(),
                ))
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
            DevinUiAction::ConfirmOrganizationSwitch(org_id) => {
                self.devin_confirm_org = Some(org_id);
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
                #[cfg(debug_assertions)]
                if self.devin_state.preview {
                    self.devin_state.select_preview(session_id);
                    self.devin_view = DevinView::Detail;
                    self.devin_focus_detail = true;
                    return;
                }
                let generation = self.devin_state.select(session_id.clone());
                self.devin_view = DevinView::Detail;
                self.devin_focus_detail = true;
                Some(DevinCommand::SelectSession {
                    session_id,
                    generation,
                })
            }
            DevinUiAction::Create => {
                let max_acu_limit = if self.devin_advanced.max_acu_limit.trim().is_empty() {
                    None
                } else {
                    let Ok(limit) = self.devin_advanced.max_acu_limit.trim().parse::<u64>() else {
                        self.show_error("Maximum ACUs must be a positive whole number".into());
                        return;
                    };
                    Some(limit)
                };
                let structured_output_schema = if self
                    .devin_advanced
                    .structured_output_schema
                    .trim()
                    .is_empty()
                {
                    None
                } else {
                    let Ok(schema) =
                        serde_json::from_str(self.devin_advanced.structured_output_schema.trim())
                    else {
                        self.show_error("Structured output must be valid JSON Schema".into());
                        return;
                    };
                    Some(schema)
                };
                let has_secret_name = !self.devin_advanced.session_secret_key.trim().is_empty();
                let has_secret_value = !self.devin_advanced.session_secret_value.is_empty();
                if has_secret_name != has_secret_value {
                    self.show_error("Enter both the ephemeral secret name and value".into());
                    return;
                }
                let session_secrets = (has_secret_name && has_secret_value)
                    .then(|| SecretInput {
                        key: self.devin_advanced.session_secret_key.trim().into(),
                        value: std::mem::take(&mut self.devin_advanced.session_secret_value),
                        kind: "key-value".into(),
                    })
                    .into_iter()
                    .collect();
                self.devin_state.busy = true;
                self.devin_state.error = None;
                self.devin_creating = true;
                Some(DevinCommand::CreateSession(CreateSessionRequest {
                    repositories: split_devin_values(&self.devin_repository),
                    prompt: self.devin_create_prompt.trim().into(),
                    title: optional_devin_value(&self.devin_advanced.title),
                    mode: optional_devin_value(&self.devin_advanced.mode),
                    playbook_id: optional_devin_value(&self.devin_advanced.playbook_id),
                    child_playbook_id: optional_devin_value(&self.devin_advanced.child_playbook_id),
                    knowledge_ids: split_devin_values(&self.devin_advanced.knowledge_ids),
                    tags: {
                        let mut tags = split_devin_values(&self.devin_advanced.tags);
                        if !tags.iter().any(|tag| tag == "editur") {
                            tags.push("editur".into());
                        }
                        tags
                    },
                    max_acu_limit,
                    platform: optional_devin_value(&self.devin_advanced.platform),
                    resumable: Some(self.devin_advanced.resumable),
                    session_links: split_devin_values(&self.devin_advanced.session_links),
                    structured_output_schema,
                    structured_output_required: self.devin_advanced.structured_output_required,
                    secret_ids: split_devin_values(&self.devin_advanced.secret_ids),
                    session_secrets,
                    create_as_user_id: optional_devin_value(&self.devin_advanced.create_as_user_id),
                    bypass_approval: self.devin_advanced.bypass_approval,
                    parent_session_id: optional_devin_value(&self.devin_advanced.parent_session_id),
                    attachments: self
                        .devin_attachments
                        .iter()
                        .map(|attachment| attachment.file.clone())
                        .collect(),
                    batch_count: self.devin_advanced.batch_count,
                }))
            }
            DevinUiAction::SendMessage => {
                let pending = PendingDevinMessage {
                    text: self.devin_message.trim().into(),
                    attachments: self
                        .devin_attachments
                        .iter()
                        .map(|attachment| attachment.file.clone())
                        .collect(),
                    failed: false,
                    confirmed: false,
                };
                self.devin_message.clear();
                self.devin_attachments.clear();
                self.devin_state.busy = true;
                self.devin_pending_message = Some(pending.clone());
                Some(DevinCommand::SendMessage {
                    message: pending.text,
                    attachments: pending.attachments,
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
                    attachments: pending.attachments.clone(),
                })
            }
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
            DevinUiAction::OpenImage(id) => {
                if let Some(bytes) = self.devin_state.attachment_previews.get(&id) {
                    self.assistant_image_lightbox =
                        Some(AssistantImageSource::Bytes(std::sync::Arc::clone(bytes)));
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
        #[cfg(debug_assertions)]
        if self.devin_state.preview {
            return;
        }
        self.ensure_devin_controller(ctx);
        match self.devin_view {
            DevinView::Sessions => self.devin_focus_list = true,
            DevinView::Filters => {}
            DevinView::Create => self.devin_focus_create = true,
            DevinView::Detail => self.devin_focus_detail = true,
            DevinView::Section(_) => {}
        }
        if let Some(controller) = self.devin_controller.as_ref() {
            let _ = controller.send(DevinCommand::SetVisible(true));
        }
    }

    pub(super) fn ensure_devin_controller(&mut self, ctx: &egui::Context) {
        if self.devin_controller.is_none() {
            let wake = ctx.clone();
            self.devin_controller =
                Some(DevinController::start(self.tree.root.clone(), move || {
                    wake.request_repaint();
                }));
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
                    self.devin_attachments.clear();
                    select_created = Some(session.id.clone());
                }
                DevinEvent::BatchCreated(session_ids) if session_ids.len() > 1 => {
                    select_created = None;
                    self.devin_view = DevinView::Sessions;
                }
                DevinEvent::SessionLoaded { detail, .. } => {
                    self.devin_tags = detail.summary.tags.join(", ");
                    if detail.summary.category == StatusCategory::Waiting {
                        self.devin_focus_detail = true;
                    }
                }
                DevinEvent::KnowledgeDetailLoaded(note) => {
                    self.devin_resource_draft.id = note.id.clone();
                    self.devin_resource_draft.name = note.name.clone();
                    self.devin_resource_draft.content = note.content.clone();
                    self.devin_resource_draft.folder_or_repository =
                        note.folder.clone().unwrap_or_default();
                }
                DevinEvent::KnowledgeSuggestionDetailLoaded(suggestion) => {
                    self.devin_resource_draft.id.clear();
                    self.devin_resource_draft.name = suggestion.title.clone();
                    self.devin_resource_draft.content = suggestion.content.clone();
                    self.devin_resource_draft.folder_or_repository.clear();
                }
                DevinEvent::PlaybookDetailLoaded(playbook) => {
                    self.devin_resource_draft.id = playbook.id.clone();
                    self.devin_resource_draft.name = playbook.title.clone();
                    self.devin_resource_draft.content = playbook.content.clone();
                    self.devin_resource_draft.folder_or_repository =
                        playbook.automation_macro.clone().unwrap_or_default();
                }
                DevinEvent::ScheduleDetailLoaded(schedule) => {
                    self.devin_resource_draft.id = schedule.id.clone();
                    self.devin_resource_draft.json = devin_json_fields(
                        &schedule.configuration,
                        &[
                            "agent",
                            "bypass_approval",
                            "enabled",
                            "frequency",
                            "interval_count",
                            "name",
                            "notify_on",
                            "platform",
                            "playbook_id",
                            "prompt",
                            "run_as_user_id",
                            "schedule_type",
                            "scheduled_at",
                            "slack_channel_id",
                            "slack_team_id",
                            "tags",
                            "target_devin_id",
                        ],
                    );
                }
                DevinEvent::AutomationDetailLoaded(automation) => {
                    self.devin_resource_draft.id = automation.id.clone();
                    self.devin_resource_draft.json = devin_json_fields(
                        &automation.configuration,
                        &[
                            "actions",
                            "concurrency",
                            "enabled",
                            "limits",
                            "metadata",
                            "name",
                            "notifications",
                            "run_as",
                            "security_profile",
                            "session_settings",
                            "tools",
                            "triggers",
                        ],
                    );
                }
                DevinEvent::BlueprintLoaded(blueprint) => {
                    self.devin_resource_draft.id = blueprint.id.clone();
                    self.devin_resource_draft.folder_or_repository =
                        blueprint.repository.clone().unwrap_or_default();
                    self.devin_resource_draft.content =
                        blueprint.contents.clone().unwrap_or_default();
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
                DevinEvent::Failed(error) | DevinEvent::PermissionDenied(error) => {
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
                                && (message.text.trim() == pending.text.trim()
                                    || (!pending.attachments.is_empty()
                                        && !pending.text.trim().is_empty()
                                        && message.text.trim().starts_with(pending.text.trim()))
                                    || (pending.text.trim().is_empty()
                                        && !pending.attachments.is_empty()
                                        && !message.attachment_ids.is_empty()))
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
                    && matches!(
                        session.category,
                        StatusCategory::Active
                            | StatusCategory::Waiting
                            | StatusCategory::WaitingApproval
                    )
            }
            DevinScope::All => true,
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

fn split_devin_values(value: &str) -> Vec<String> {
    value
        .split([',', '\n'])
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

fn optional_devin_value(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_owned())
}

fn devin_json_preview(value: &serde_json::Value) -> String {
    const MAX_BYTES: usize = 64 * 1024;
    let mut preview = serde_json::to_string_pretty(value).unwrap_or_default();
    if preview.len() > MAX_BYTES {
        let mut end = MAX_BYTES;
        while !preview.is_char_boundary(end) {
            end -= 1;
        }
        preview.truncate(end);
        preview.push('…');
    }
    preview
}

fn devin_json_fields(value: &serde_json::Value, names: &[&str]) -> String {
    let mut fields = serde_json::Map::new();
    if let Some(object) = value.as_object() {
        for name in names {
            if let Some(value) = object.get(*name) {
                fields.insert((*name).into(), value.clone());
            }
        }
    }
    devin_json_preview(&serde_json::Value::Object(fields))
}

fn devin_section_available(state: &DevinState, section: DevinSection) -> bool {
    match section {
        DevinSection::Repositories => state.capabilities.has("list_available_repos"),
        DevinSection::Knowledge => state.capabilities.has("devin_knowledge_manage"),
        DevinSection::Playbooks => state.capabilities.has("devin_playbook_manage"),
        DevinSection::Integrations => state.capabilities.has("list_integrations"),
        DevinSection::Review | DevinSection::Automations | DevinSection::Environment => true,
    }
}

fn draw_resource_status(ui: &mut egui::Ui, status: LoadState, error: Option<&str>) {
    match status {
        LoadState::Loading => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label("Loading…");
            });
        }
        LoadState::Forbidden => devin_callout(
            ui,
            error.unwrap_or("This Devin feature is unavailable for this credential."),
            theme::semantic().warning,
        ),
        LoadState::Failed => devin_callout(
            ui,
            error.unwrap_or("This Devin resource could not be loaded."),
            theme::semantic().danger,
        ),
        LoadState::Stale => {
            ui.label(
                RichText::new("Showing stale data")
                    .font(theme::typography::small())
                    .color(theme::semantic().warning),
            );
        }
        LoadState::Idle | LoadState::Loaded => {}
    }
}

fn resource_card(ui: &mut egui::Ui, title: &str, subtitle: Option<&str>, body: &str) {
    ui.add_space(theme::space::SNUG);
    ui.label(
        RichText::new(title)
            .font(theme::typography::small_strong())
            .color(theme::text().primary),
    );
    if let Some(subtitle) = subtitle {
        ui.label(
            RichText::new(subtitle)
                .font(theme::typography::small())
                .color(theme::text().muted),
        );
    }
    if !body.is_empty() {
        ui.add(Label::new(body).selectable(true).wrap());
    }
}

fn resource_editor_fields(
    ui: &mut egui::Ui,
    draft: &mut DevinResourceDraft,
    name_hint: &str,
    secondary_hint: &str,
    content_hint: &str,
) {
    ui.separator();
    ui.add(
        TextEdit::singleline(&mut draft.id)
            .hint_text(devin_field_hint("ID for update/delete"))
            .desired_width(f32::INFINITY),
    );
    ui.add(
        TextEdit::singleline(&mut draft.name)
            .hint_text(devin_field_hint(name_hint))
            .desired_width(f32::INFINITY),
    );
    ui.add(
        TextEdit::singleline(&mut draft.folder_or_repository)
            .hint_text(devin_field_hint(secondary_hint))
            .desired_width(f32::INFINITY),
    );
    ui.add(
        TextEdit::multiline(&mut draft.content)
            .hint_text(devin_field_hint(content_hint))
            .desired_rows(6)
            .desired_width(f32::INFINITY),
    );
}

fn resource_json_editor(ui: &mut egui::Ui, draft: &mut DevinResourceDraft, hint: &str) {
    ui.separator();
    ui.add(
        TextEdit::singleline(&mut draft.id)
            .hint_text(devin_field_hint("ID for update/delete"))
            .desired_width(f32::INFINITY),
    );
    ui.add(
        TextEdit::multiline(&mut draft.json)
            .font(theme::typography::code_small())
            .hint_text(devin_field_hint(hint))
            .desired_rows(8)
            .desired_width(f32::INFINITY),
    );
}

fn resource_crud_buttons(
    ui: &mut egui::Ui,
    resource: &str,
    draft: &DevinResourceDraft,
    action: &mut Option<DevinUiAction>,
    mutation: impl Fn(CrudAction, Option<String>, serde_json::Value) -> ResourceMutation,
) {
    let payload = if draft.json.trim().is_empty() {
        Ok(serde_json::json!({}))
    } else {
        serde_json::from_str(&draft.json)
    };
    if payload.is_err() {
        devin_callout(
            ui,
            &format!("The {resource} payload must be valid JSON."),
            theme::semantic().danger,
        );
    }
    ui.horizontal_wrapped(|ui| {
        for (label, crud) in [
            ("Create", CrudAction::Create),
            ("Update", CrudAction::Update),
            ("Delete", CrudAction::Delete),
        ] {
            let enabled = crud == CrudAction::Delete || payload.is_ok();
            if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                let mutation = mutation(
                    crud,
                    optional_devin_value(&draft.id),
                    payload
                        .as_ref()
                        .ok()
                        .cloned()
                        .unwrap_or_else(|| serde_json::json!({})),
                );
                *action = Some(if crud == CrudAction::Delete {
                    DevinUiAction::ConfirmResourceMutation(mutation)
                } else {
                    DevinUiAction::MutateResource(mutation)
                });
            }
        }
    });
}

fn devin_scopes() -> [DevinScope; 3] {
    [DevinScope::Active, DevinScope::All, DevinScope::Archived]
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
        StatusCategory::WaitingApproval => (1, "NEEDS APPROVAL"),
        StatusCategory::Active => (2, "WORKING"),
        StatusCategory::Suspended => (3, "SUSPENDED"),
        StatusCategory::Sleeping => (4, "IDLE"),
        StatusCategory::Completed | StatusCategory::Failed => (5, "DONE"),
        StatusCategory::Unknown => (6, "OTHER"),
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
    if session.archived {
        return "Archived";
    }
    match session.category {
        StatusCategory::Waiting => "Needs you",
        StatusCategory::WaitingApproval => "Needs approval",
        StatusCategory::Active => "Working",
        StatusCategory::Sleeping => "Idle",
        StatusCategory::Suspended => "Suspended",
        StatusCategory::Completed => "Finished",
        StatusCategory::Failed => "Failed",
        StatusCategory::Unknown => &session.status,
    }
}

fn origin_label(origin: &str) -> &str {
    match origin.to_ascii_lowercase().as_str() {
        "slack" => "Slack",
        "web" | "webapp" | "devin" | "devin web" => "Devin web",
        "editur" | "api" | "mcp" => "Editur",
        _ => origin,
    }
}

fn devin_status_dot_color(status: StatusCategory) -> Color32 {
    match status {
        StatusCategory::Active => theme::accent(),
        StatusCategory::Waiting | StatusCategory::WaitingApproval => theme::semantic().warning,
        StatusCategory::Sleeping | StatusCategory::Suspended | StatusCategory::Unknown => {
            theme::text().muted
        }
        StatusCategory::Completed => theme::semantic().success,
        StatusCategory::Failed => theme::semantic().danger,
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
    parse_timestamp_nanos(value).map(|timestamp| (timestamp / 1_000_000_000) as i64)
}

fn parse_timestamp_nanos(value: &str) -> Option<i128> {
    if let Ok(number) = value.parse::<f64>() {
        if !number.is_finite() {
            return None;
        }
        return Some(if number > 10_000_000_000.0 {
            (number * 1_000_000.0) as i128
        } else {
            (number * 1_000_000_000.0) as i128
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
    let fraction = value
        .get(19..)
        .and_then(|suffix| suffix.strip_prefix('.'))
        .map(|suffix| suffix.bytes().take_while(u8::is_ascii_digit).take(9))
        .map(|digits| {
            let mut value = 0_i128;
            let mut count = 0;
            for digit in digits {
                value = value * 10 + i128::from(digit - b'0');
                count += 1;
            }
            value * 10_i128.pow(9 - count)
        })
        .unwrap_or_default();
    Some(
        i128::from(
            days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second,
        ) * 1_000_000_000
            + fraction,
    )
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
    match (parse_timestamp_nanos(left), parse_timestamp_nanos(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

/// One remote action as the Agent's dense tool row: the summary as the title,
/// with the path, command, details, and remote link folded into the same
/// indented body the Agent uses for local tool calls.
fn draw_devin_activity(ui: &mut egui::Ui, activity: &Activity, action: &mut Option<DevinUiAction>) {
    let missing_details = activity.details.is_none();
    let has_body = !missing_details
        || activity.path.is_some()
        || activity.command.is_some()
        || activity.url.is_some()
        || activity.pull_request_url.is_some()
        || activity.attachment_id.is_some();
    let opened = assistant_dense_tool(
        ui,
        Id::new(("devin_activity", activity.id.as_str())),
        &activity.summary,
        None,
        None,
        None,
        has_body || missing_details,
        |ui| {
            for line in [activity.path.as_deref(), activity.command.as_deref()]
                .into_iter()
                .flatten()
            {
                ui.label(
                    RichText::new(line)
                        .font(theme::typography::code_small())
                        .color(theme::text().secondary),
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
            } else {
                ui.label(
                    RichText::new("Loading details…")
                        .font(theme::typography::small())
                        .color(theme::text().muted),
                );
            }
            if let Some(url) = activity.url.as_deref() {
                ui.hyperlink_to(
                    RichText::new("Open remote link").font(theme::typography::small()),
                    url,
                );
            }
            if let Some(url) = activity.pull_request_url.as_deref() {
                ui.hyperlink_to(
                    RichText::new("Open pull request").font(theme::typography::small()),
                    url,
                );
            }
            if let Some(attachment_id) = activity.attachment_id.as_deref() {
                ui.label(
                    RichText::new(format!("Attachment {attachment_id}"))
                        .font(theme::typography::small())
                        .color(theme::text().secondary),
                );
            }
        },
    );
    if opened && missing_details {
        *action = Some(DevinUiAction::FetchActivity(activity.id.clone()));
    }
    if let Some(child_session_id) = activity.child_session_id.as_deref()
        && ui.small_button("Open child session").clicked()
    {
        *action = Some(DevinUiAction::Select(child_session_id.into()));
    }
}

/// A transcript attachment that is not a previewable image: a compact pill
/// carrying the file glyph and the full name, so a log or archive reads as
/// "this file", not as a cryptic extension swatch.
fn devin_attachment_pill(
    ui: &mut egui::Ui,
    attachment: &crate::devin::Attachment,
) -> egui::Response {
    let name = ui.painter().layout_no_wrap(
        attachment.name.clone(),
        theme::typography::small(),
        theme::text().secondary,
    );
    let size = egui::vec2(
        theme::space::SMALL
            + icons::GRID
            + theme::space::TIGHT
            + name.size().x
            + theme::space::SMALL,
        theme::control::COMPACT,
    );
    let linked = attachment.url.is_some();
    let (rect, response) = ui.allocate_exact_size(
        size,
        if linked {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            if linked {
                egui::WidgetType::Link
            } else {
                egui::WidgetType::Label
            },
            ui.is_enabled(),
            format!("Attachment: {}", attachment.name),
        )
    });
    let fill = if linked && response.hovered() {
        theme::state::hover()
    } else {
        theme::surface().input
    };
    ui.painter()
        .rect_filled(rect, theme::corner(theme::radius::CONTROL), fill);
    ui.painter().rect_stroke(
        rect,
        theme::corner(theme::radius::CONTROL),
        egui::Stroke::new(1.0, theme::state::selected()),
        egui::StrokeKind::Inside,
    );
    icons::paint(
        ui.painter(),
        Icon::File,
        egui::Rect::from_center_size(
            egui::pos2(
                rect.left() + theme::space::SMALL + icons::GRID * 0.5,
                rect.center().y,
            ),
            egui::Vec2::splat(icons::GRID * 0.85),
        ),
        theme::text().muted,
    );
    let name_y = rect.center().y - name.size().y * 0.5;
    ui.painter().galley(
        egui::pos2(
            rect.left() + theme::space::SMALL + icons::GRID + theme::space::TIGHT,
            name_y,
        ),
        name,
        theme::text().secondary,
    );
    if linked {
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Open attachment")
    } else {
        response
    }
}

/// Hint text carries its own face: the sidebar maps the default body style to
/// the title face, which would render placeholders large and semibold.
fn devin_field_hint(text: &str) -> RichText {
    RichText::new(text).font(theme::typography::body())
}

/// The Devin mark from the official brand lockup, so the connect card reads
/// as Devin's own sign-in sheet.
fn paint_devin_mark(painter: &egui::Painter, rect: egui::Rect) {
    paint_devin_texture(painter, rect, Color32::WHITE, false);
}

pub(super) fn paint_devin_icon(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    paint_devin_texture(painter, rect, color, true);
}

fn paint_devin_texture(
    painter: &egui::Painter,
    rect: egui::Rect,
    color: Color32,
    monochrome: bool,
) {
    let ctx = painter.ctx();
    let cache_id = Id::new(("devin_mark_texture", monochrome));
    let texture = ctx.data_mut(|data| data.get_temp::<egui::TextureHandle>(cache_id));
    let texture = texture.or_else(|| {
        let bytes = &include_bytes!("../../assets/icons/devin.png")[..];
        let mut pixels = image::load_from_memory(bytes).ok()?.into_rgba8();
        if monochrome {
            for pixel in pixels.pixels_mut() {
                let alpha = if pixel.0[..3].iter().copied().max().unwrap_or(0) > 32 {
                    pixel.0[3]
                } else {
                    0
                };
                pixel.0 = [255, 255, 255, alpha];
            }
        }
        let size = [pixels.width() as usize, pixels.height() as usize];
        let texture = ctx.load_texture(
            if monochrome {
                "Devin icon"
            } else {
                "Devin logo"
            },
            egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_raw()),
            egui::TextureOptions::LINEAR,
        );
        ctx.data_mut(|data| data.insert_temp(cache_id, texture.clone()));
        Some(texture)
    });
    if let Some(texture) = texture {
        let source = texture.size_vec2();
        let scale = (rect.width() / source.x).min(rect.height() / source.y);
        let rect = egui::Rect::from_center_size(rect.center(), source * scale);
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            color,
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
            "devin_resource_query",
            "devin_resource_filter",
            "devin_tags",
            "devin_activity_query",
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
            session("approval", StatusCategory::WaitingApproval, false),
            session("suspended", StatusCategory::Suspended, false),
            session("idle", StatusCategory::Sleeping, false),
            session("archived", StatusCategory::Completed, true),
        ];

        assert_eq!(
            visible_session_ids(&sessions, DevinScope::Active, ""),
            vec!["waiting", "approval", "working"]
        );
        assert_eq!(
            visible_session_ids(&sessions, DevinScope::All, ""),
            vec![
                "waiting",
                "approval",
                "working",
                "suspended",
                "idle",
                "archived"
            ]
        );
        assert_eq!(
            visible_session_ids(&sessions, DevinScope::Archived, ""),
            vec!["archived"]
        );
        assert_eq!(semantic_status(sessions.last().unwrap()), "Archived");
        assert_eq!(origin_label("webapp"), "Devin web");
        assert_eq!(
            session_group(&session("suspended", StatusCategory::Suspended, false)).1,
            "SUSPENDED"
        );
        assert_eq!(
            session_group(&session("unknown", StatusCategory::Unknown, false)).1,
            "OTHER"
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
