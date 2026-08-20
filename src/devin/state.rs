use std::{
    collections::{BTreeSet, HashSet},
    time::Instant,
};

use super::credentials::CredentialSource;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ConnectionState {
    #[default]
    Disconnected,
    Connecting,
    Connected,
    AuthenticationRequired,
    Offline,
    RateLimited,
    Failed,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DevinCapabilities {
    tools: BTreeSet<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DevinOrganization {
    pub id: String,
    pub name: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionFilters {
    pub origin: String,
    pub repository: String,
    pub tags: Vec<String>,
    pub playbook_id: String,
    pub schedule_id: String,
    pub user_id: String,
    pub parent_session_id: String,
    pub category: String,
    pub status: String,
    pub created_after: String,
    pub created_before: String,
    pub updated_after: String,
    pub updated_before: String,
}

impl SessionFilters {
    pub fn is_empty(&self) -> bool {
        self.origin.is_empty()
            && self.repository.is_empty()
            && self.tags.is_empty()
            && self.playbook_id.is_empty()
            && self.schedule_id.is_empty()
            && self.user_id.is_empty()
            && self.parent_session_id.is_empty()
            && self.category.is_empty()
            && self.status.is_empty()
            && self.created_after.is_empty()
            && self.created_before.is_empty()
            && self.updated_after.is_empty()
            && self.updated_before.is_empty()
    }
}

impl DevinCapabilities {
    pub(crate) fn new(tools: impl IntoIterator<Item = String>) -> Self {
        Self {
            tools: tools.into_iter().collect(),
        }
    }

    pub fn has(&self, tool: &str) -> bool {
        self.tools.contains(tool)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StatusCategory {
    Active,
    Waiting,
    WaitingApproval,
    Sleeping,
    Suspended,
    Completed,
    Failed,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum DevinSection {
    Review,
    Repositories,
    Knowledge,
    Playbooks,
    Automations,
    Environment,
    Integrations,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DevinResourceKind {
    Repositories,
    Documents,
    Knowledge,
    KnowledgeFolders,
    KnowledgeSuggestions,
    Playbooks,
    Schedules,
    Automations,
    AutomationCatalog,
    Integrations,
    Reviews,
    Blueprints,
    BlueprintFiles,
    Builds,
    Secrets,
    Insights,
}

impl DevinSection {
    pub const ALL: [Self; 7] = [
        Self::Review,
        Self::Repositories,
        Self::Knowledge,
        Self::Playbooks,
        Self::Automations,
        Self::Environment,
        Self::Integrations,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Review => "Review",
            Self::Repositories => "Repositories",
            Self::Knowledge => "Knowledge",
            Self::Playbooks => "Playbooks",
            Self::Automations => "Automations",
            Self::Environment => "Environment",
            Self::Integrations => "Integrations",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LoadState {
    #[default]
    Idle,
    Loading,
    Loaded,
    Stale,
    Forbidden,
    Failed,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ResourceState<T> {
    pub status: LoadState,
    pub items: Vec<T>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DevinRepository {
    pub id: String,
    pub name: String,
    pub indexed: bool,
    pub indexing_status: Option<String>,
    pub branches: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct WikiDocument {
    pub repository: String,
    pub title: String,
    pub content: String,
    pub citations: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KnowledgeNote {
    pub id: String,
    pub name: String,
    pub content: String,
    pub folder: Option<String>,
    pub repositories: Vec<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KnowledgeFolder {
    pub id: String,
    pub name: String,
    pub note_count: usize,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct KnowledgeSuggestion {
    pub id: String,
    pub title: String,
    pub content: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Playbook {
    pub id: String,
    pub title: String,
    pub content: String,
    pub automation_macro: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Schedule {
    pub id: String,
    pub title: String,
    pub prompt: String,
    pub cadence: String,
    pub enabled: bool,
    pub configuration: serde_json::Value,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Automation {
    pub id: String,
    pub title: String,
    pub enabled: bool,
    pub summary: String,
    pub configuration: serde_json::Value,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct AutomationCatalog {
    pub schemas: serde_json::Value,
    pub templates: serde_json::Value,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Integration {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub installed: bool,
    pub url: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Review {
    pub pull_request_url: String,
    pub status: String,
    pub result_url: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Blueprint {
    pub id: String,
    pub name: String,
    pub repository: Option<String>,
    pub contents_url: Option<String>,
    pub contents: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BlueprintFile {
    pub id: String,
    pub name: String,
    pub size: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SnapshotBuild {
    pub id: String,
    pub status: String,
    pub created_at: Option<String>,
    pub logs_url: Option<String>,
    pub pinned: bool,
    pub configuration: serde_json::Value,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SecretMetadata {
    pub id: String,
    pub name: String,
    pub scope: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SessionInsight {
    pub session_id: String,
    pub status: String,
    pub summary: Option<String>,
    pub message_count: Option<u64>,
}

#[derive(Default)]
pub struct DevinResources {
    pub repositories: ResourceState<DevinRepository>,
    pub documents: ResourceState<WikiDocument>,
    pub knowledge: ResourceState<KnowledgeNote>,
    pub knowledge_folders: ResourceState<KnowledgeFolder>,
    pub knowledge_suggestions: ResourceState<KnowledgeSuggestion>,
    pub playbooks: ResourceState<Playbook>,
    pub schedules: ResourceState<Schedule>,
    pub automations: ResourceState<Automation>,
    pub automation_catalog: ResourceState<AutomationCatalog>,
    pub integrations: ResourceState<Integration>,
    pub reviews: ResourceState<Review>,
    pub blueprints: ResourceState<Blueprint>,
    pub blueprint_files: ResourceState<BlueprintFile>,
    pub builds: ResourceState<SnapshotBuild>,
    pub secrets: ResourceState<SecretMetadata>,
    pub insights: ResourceState<SessionInsight>,
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct SessionSummary {
    pub id: String,
    pub title: String,
    pub prompt: Option<String>,
    pub status: String,
    pub status_detail: Option<String>,
    pub category: StatusCategory,
    pub origin: Option<String>,
    pub repository: Option<String>,
    pub created_at: Option<String>,
    pub updated_at: Option<String>,
    pub archived: bool,
    pub parent_session_id: Option<String>,
    pub url: Option<String>,
    pub pull_request_count: usize,
    pub tags: Vec<String>,
    pub org_id: Option<String>,
    pub user_id: Option<String>,
    pub service_user_id: Option<String>,
    pub automation_id: Option<String>,
    pub devin_mode: Option<String>,
    pub playbook_id: Option<String>,
    pub session_category: Option<String>,
    pub subcategory: Option<String>,
    pub structured_output: Option<serde_json::Value>,
}

impl std::fmt::Debug for SessionSummary {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SessionSummary")
            .field("id", &self.id)
            .field("title", &self.title)
            .field("prompt", &self.prompt.as_ref().map(|_| "[REDACTED]"))
            .field("status", &self.status)
            .field("status_detail", &self.status_detail)
            .field("category", &self.category)
            .field("archived", &self.archived)
            .field(
                "structured_output",
                &self.structured_output.as_ref().map(|_| "[REDACTED]"),
            )
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct DevinMessage {
    pub id: String,
    pub timestamp: String,
    pub role: String,
    pub text: String,
    pub attachment_ids: Vec<String>,
}

impl std::fmt::Debug for DevinMessage {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DevinMessage")
            .field("id", &self.id)
            .field("timestamp", &self.timestamp)
            .field("role", &self.role)
            .field("text", &"[REDACTED]")
            .field("attachment_ids", &self.attachment_ids)
            .finish()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Activity {
    pub id: String,
    pub timestamp: String,
    pub category: String,
    pub summary: String,
    pub details: Option<String>,
    pub path: Option<String>,
    pub command: Option<String>,
    pub url: Option<String>,
    pub attachment_id: Option<String>,
    pub pull_request_url: Option<String>,
    pub child_session_id: Option<String>,
}

#[derive(Clone, Default, Eq, PartialEq)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub media_type: Option<String>,
    pub size: Option<u64>,
    pub url: Option<String>,
}

impl std::fmt::Debug for Attachment {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Attachment")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("media_type", &self.media_type)
            .field("size", &self.size)
            .field("url", &self.url.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PullRequest {
    pub id: String,
    pub title: String,
    pub url: String,
    pub status: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ChildSession {
    pub id: String,
    pub title: String,
    pub status: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub acus: Option<f64>,
    pub limit: Option<f64>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SessionDetail {
    pub summary: SessionSummary,
    pub attachments: Vec<Attachment>,
    pub pull_requests: Vec<PullRequest>,
    pub children: Vec<ChildSession>,
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DevinError {
    pub message: String,
    pub retry_after_seconds: Option<u64>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum RepositoryState {
    #[default]
    Unknown,
    Suggested(String),
    SelectionRequired,
}

#[derive(Clone, Debug, PartialEq)]
pub enum DevinEvent {
    ConnectionChanged(ConnectionState),
    CapabilitiesChanged(DevinCapabilities),
    CredentialsChanged(Option<CredentialSource>),
    OrganizationsDiscovered(Vec<DevinOrganization>),
    OrganizationSelected(String),
    OrganizationSwitching,
    RepositoryResolved(RepositoryState),
    FiltersChanged(SessionFilters),
    WorkspaceBoundary(String),
    ResourceLoading(DevinSection),
    ResourceForbidden(DevinSection),
    ResourceFailed {
        section: DevinSection,
        error: DevinError,
    },
    ResourceKindLoading(DevinResourceKind),
    ResourceKindForbidden(DevinResourceKind),
    ResourceKindFailed {
        kind: DevinResourceKind,
        error: DevinError,
    },
    RepositoriesLoaded(Vec<DevinRepository>),
    DocumentsLoaded(Vec<WikiDocument>),
    KnowledgeLoaded(Vec<KnowledgeNote>),
    KnowledgeDetailLoaded(KnowledgeNote),
    KnowledgeFoldersLoaded(Vec<KnowledgeFolder>),
    KnowledgeSuggestionsLoaded(Vec<KnowledgeSuggestion>),
    KnowledgeSuggestionDetailLoaded(KnowledgeSuggestion),
    PlaybooksLoaded(Vec<Playbook>),
    PlaybookDetailLoaded(Playbook),
    SchedulesLoaded(Vec<Schedule>),
    ScheduleDetailLoaded(Schedule),
    AutomationsLoaded(Vec<Automation>),
    AutomationDetailLoaded(Automation),
    AutomationCatalogLoaded(AutomationCatalog),
    IntegrationsLoaded(Vec<Integration>),
    ReviewsLoaded(Vec<Review>),
    BlueprintsLoaded(Vec<Blueprint>),
    BlueprintLoaded(Blueprint),
    BlueprintFilesLoaded(Vec<BlueprintFile>),
    BuildLogLoaded {
        build_id: String,
        logs_url: String,
    },
    BuildsLoaded(Vec<SnapshotBuild>),
    BuildDetailLoaded(SnapshotBuild),
    SecretsLoaded(Vec<SecretMetadata>),
    InsightsLoaded(Vec<SessionInsight>),
    SessionsLoaded {
        sessions: Vec<SessionSummary>,
        next_cursor: Option<String>,
        total: Option<usize>,
        has_next: bool,
        append: bool,
    },
    BatchCreated(Vec<String>),
    SessionCreated(SessionSummary),
    SessionLoaded {
        session_id: String,
        generation: u64,
        detail: SessionDetail,
    },
    MessagesLoaded {
        session_id: String,
        generation: u64,
        messages: Vec<DevinMessage>,
        next_cursor: Option<String>,
        update: PageUpdate,
    },
    ActivityLoaded {
        session_id: String,
        generation: u64,
        activity: Vec<Activity>,
        next_cursor: Option<String>,
        update: PageUpdate,
    },
    ActivityDetailsLoaded {
        session_id: String,
        generation: u64,
        event_id: String,
        details: String,
    },
    AttachmentFetched {
        session_id: String,
        generation: u64,
        attachment_id: String,
        bytes: std::sync::Arc<[u8]>,
    },
    OperationFinished {
        session_id: Option<String>,
    },
    PermissionDenied(DevinError),
    Failed(DevinError),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PageUpdate {
    Initial,
    History,
    Refresh,
}

#[derive(Default)]
pub struct DevinState {
    pub connection: ConnectionState,
    pub capabilities: DevinCapabilities,
    pub credential_source: Option<CredentialSource>,
    pub organizations: Vec<DevinOrganization>,
    pub selected_org_id: Option<String>,
    pub repository: RepositoryState,
    pub filters: SessionFilters,
    pub workspace_boundary: Option<String>,
    pub resources: DevinResources,
    pub sessions: Vec<SessionSummary>,
    pub sessions_revision: u64,
    pub sessions_cursor: Option<String>,
    pub sessions_total: Option<usize>,
    pub sessions_has_next: bool,
    pub last_created_batch: Vec<String>,
    pub last_sessions_refresh: Option<Instant>,
    pub selected_session: Option<String>,
    pub selected_generation: u64,
    pub detail: Option<SessionDetail>,
    pub messages: Vec<DevinMessage>,
    pub messages_revision: u64,
    pub activity: Vec<Activity>,
    pub activity_revision: u64,
    #[doc(hidden)]
    pub message_ids: HashSet<String>,
    #[doc(hidden)]
    pub activity_ids: HashSet<String>,
    /// Downloaded bytes for the selected session's image attachments, keyed by
    /// attachment id, so the transcript can paint the same previews the Agent
    /// paints for local prompt images.
    pub attachment_previews: std::collections::HashMap<String, std::sync::Arc<[u8]>>,
    pub error: Option<DevinError>,
    pub busy: bool,
    pub session_loading: bool,
}

impl DevinState {
    pub fn select(&mut self, session_id: String) -> u64 {
        self.selected_generation = self.selected_generation.wrapping_add(1);
        self.selected_session = Some(session_id);
        self.detail = None;
        self.messages.clear();
        self.messages_revision = self.messages_revision.wrapping_add(1);
        self.message_ids.clear();
        self.activity.clear();
        self.activity_revision = self.activity_revision.wrapping_add(1);
        self.activity_ids.clear();
        self.attachment_previews.clear();
        self.error = None;
        self.busy = true;
        self.session_loading = true;
        self.selected_generation
    }

    pub fn clear_selection(&mut self) {
        self.selected_generation = self.selected_generation.wrapping_add(1);
        self.selected_session = None;
        self.detail = None;
        self.messages.clear();
        self.messages_revision = self.messages_revision.wrapping_add(1);
        self.message_ids.clear();
        self.activity.clear();
        self.activity_revision = self.activity_revision.wrapping_add(1);
        self.activity_ids.clear();
        self.attachment_previews.clear();
        self.busy = false;
        self.session_loading = false;
    }

    pub fn apply(&mut self, event: DevinEvent) {
        match event {
            DevinEvent::ConnectionChanged(connection) => self.connection = connection,
            DevinEvent::CapabilitiesChanged(capabilities) => self.capabilities = capabilities,
            DevinEvent::CredentialsChanged(source) => {
                self.credential_source = source;
                if source.is_none() {
                    self.capabilities = DevinCapabilities::default();
                    self.organizations.clear();
                    self.selected_org_id = None;
                    self.sessions.clear();
                    self.bump_sessions_revision();
                    self.filters = SessionFilters::default();
                    self.sessions_cursor = None;
                    self.sessions_total = None;
                    self.sessions_has_next = false;
                    self.last_created_batch.clear();
                    self.last_sessions_refresh = None;
                    self.clear_selection();
                    self.error = None;
                    self.resources = DevinResources::default();
                }
            }
            DevinEvent::OrganizationsDiscovered(organizations) => {
                self.organizations = organizations;
                self.connection = ConnectionState::AuthenticationRequired;
                self.busy = false;
            }
            DevinEvent::OrganizationSelected(org_id) => {
                self.selected_org_id = Some(org_id);
            }
            DevinEvent::OrganizationSwitching => {
                self.sessions.clear();
                self.bump_sessions_revision();
                self.filters = SessionFilters::default();
                self.sessions_cursor = None;
                self.sessions_total = None;
                self.sessions_has_next = false;
                self.last_created_batch.clear();
                self.last_sessions_refresh = None;
                self.clear_selection();
                self.resources = DevinResources::default();
                self.error = None;
            }
            DevinEvent::RepositoryResolved(repository) => self.repository = repository,
            DevinEvent::FiltersChanged(filters) => self.filters = filters,
            DevinEvent::WorkspaceBoundary(message) => self.workspace_boundary = Some(message),
            DevinEvent::ResourceLoading(section) => {
                *resource_state_mut(&mut self.resources, section).0 = LoadState::Loading;
            }
            DevinEvent::ResourceForbidden(section) => {
                let (status, error) = resource_state_mut(&mut self.resources, section);
                *status = LoadState::Forbidden;
                *error =
                    Some("This credential does not have permission for this Devin feature".into());
                self.busy = false;
            }
            DevinEvent::ResourceFailed { section, error } => {
                let (status, message) = resource_state_mut(&mut self.resources, section);
                *status = LoadState::Failed;
                *message = Some(error.message);
                self.busy = false;
            }
            DevinEvent::ResourceKindLoading(kind) => {
                *resource_kind_state_mut(&mut self.resources, kind).0 = LoadState::Loading;
            }
            DevinEvent::ResourceKindForbidden(kind) => {
                let (status, error) = resource_kind_state_mut(&mut self.resources, kind);
                *status = LoadState::Forbidden;
                *error =
                    Some("This credential does not have permission for this Devin feature".into());
            }
            DevinEvent::ResourceKindFailed { kind, error } => {
                let stale = resource_kind_has_items(&self.resources, kind);
                let (status, message) = resource_kind_state_mut(&mut self.resources, kind);
                *status = if stale {
                    LoadState::Stale
                } else {
                    LoadState::Failed
                };
                *message = Some(error.message);
            }
            DevinEvent::RepositoriesLoaded(items) => {
                loaded(&mut self.resources.repositories, items)
            }
            DevinEvent::DocumentsLoaded(items) => loaded(&mut self.resources.documents, items),
            DevinEvent::KnowledgeLoaded(items) => loaded(&mut self.resources.knowledge, items),
            DevinEvent::KnowledgeDetailLoaded(item) => {
                upsert_loaded(&mut self.resources.knowledge, item, |item| item.id.as_str())
            }
            DevinEvent::KnowledgeFoldersLoaded(items) => {
                loaded(&mut self.resources.knowledge_folders, items)
            }
            DevinEvent::KnowledgeSuggestionsLoaded(items) => {
                loaded(&mut self.resources.knowledge_suggestions, items)
            }
            DevinEvent::KnowledgeSuggestionDetailLoaded(item) => {
                upsert_loaded(&mut self.resources.knowledge_suggestions, item, |item| {
                    item.id.as_str()
                })
            }
            DevinEvent::PlaybooksLoaded(items) => loaded(&mut self.resources.playbooks, items),
            DevinEvent::PlaybookDetailLoaded(item) => {
                upsert_loaded(&mut self.resources.playbooks, item, |item| item.id.as_str())
            }
            DevinEvent::SchedulesLoaded(items) => loaded(&mut self.resources.schedules, items),
            DevinEvent::ScheduleDetailLoaded(item) => {
                upsert_loaded(&mut self.resources.schedules, item, |item| item.id.as_str())
            }
            DevinEvent::AutomationsLoaded(items) => loaded(&mut self.resources.automations, items),
            DevinEvent::AutomationDetailLoaded(item) => {
                upsert_loaded(&mut self.resources.automations, item, |item| {
                    item.id.as_str()
                })
            }
            DevinEvent::AutomationCatalogLoaded(catalog) => {
                loaded(&mut self.resources.automation_catalog, vec![catalog])
            }
            DevinEvent::IntegrationsLoaded(items) => {
                loaded(&mut self.resources.integrations, items)
            }
            DevinEvent::ReviewsLoaded(items) => loaded(&mut self.resources.reviews, items),
            DevinEvent::BlueprintsLoaded(items) => loaded(&mut self.resources.blueprints, items),
            DevinEvent::BlueprintLoaded(blueprint) => {
                if let Some(current) = self
                    .resources
                    .blueprints
                    .items
                    .iter_mut()
                    .find(|current| current.id == blueprint.id)
                {
                    *current = blueprint;
                } else {
                    self.resources.blueprints.items.push(blueprint);
                }
                self.resources.blueprints.status = LoadState::Loaded;
                self.resources.blueprints.error = None;
            }
            DevinEvent::BlueprintFilesLoaded(items) => {
                loaded(&mut self.resources.blueprint_files, items)
            }
            DevinEvent::BuildsLoaded(items) => loaded(&mut self.resources.builds, items),
            DevinEvent::BuildDetailLoaded(item) => {
                upsert_loaded(&mut self.resources.builds, item, |item| item.id.as_str())
            }
            DevinEvent::BuildLogLoaded { build_id, logs_url } => {
                if let Some(build) = self
                    .resources
                    .builds
                    .items
                    .iter_mut()
                    .find(|build| build.id == build_id)
                {
                    build.logs_url = Some(logs_url);
                }
            }
            DevinEvent::SecretsLoaded(items) => loaded(&mut self.resources.secrets, items),
            DevinEvent::InsightsLoaded(items) => loaded(&mut self.resources.insights, items),
            DevinEvent::SessionsLoaded {
                sessions,
                next_cursor,
                total,
                has_next,
                append,
            } => {
                if append {
                    append_unique(&mut self.sessions, sessions, |session| session.id.as_str());
                } else {
                    self.sessions = sessions;
                }
                self.bump_sessions_revision();
                self.sessions_cursor = next_cursor;
                if total.is_some() || !append {
                    self.sessions_total = total;
                }
                self.sessions_has_next = has_next;
                self.last_sessions_refresh = Some(Instant::now());
                self.error = None;
            }
            DevinEvent::SessionCreated(session) => {
                if let Some(existing) = self
                    .sessions
                    .iter_mut()
                    .find(|existing| existing.id == session.id)
                {
                    *existing = session;
                } else {
                    self.sessions.insert(0, session);
                }
                self.bump_sessions_revision();
                self.busy = false;
                self.error = None;
            }
            DevinEvent::BatchCreated(session_ids) => {
                self.last_created_batch = session_ids;
                self.busy = false;
                self.error = None;
            }
            DevinEvent::SessionLoaded {
                session_id,
                generation,
                detail,
            } if self.is_current(&session_id, generation) => {
                if let Some(summary) = self
                    .sessions
                    .iter_mut()
                    .find(|summary| summary.id == session_id)
                {
                    *summary = detail.summary.clone();
                    self.bump_sessions_revision();
                }
                self.detail = Some(detail);
                self.error = None;
                self.busy = false;
            }
            DevinEvent::MessagesLoaded {
                session_id,
                generation,
                messages,
                next_cursor: _,
                update,
            } if self.is_current(&session_id, generation) => {
                let cleared = update == PageUpdate::Initial && !self.messages.is_empty();
                if update == PageUpdate::Initial {
                    self.messages.clear();
                    self.message_ids.clear();
                }
                if self.message_ids.len() < self.messages.len() {
                    self.message_ids
                        .extend(self.messages.iter().map(|message| message.id.clone()));
                }
                let changed = merge_chronological(
                    &mut self.messages,
                    messages,
                    &mut self.message_ids,
                    |message| message.id.as_str(),
                    |message| message.timestamp.as_str(),
                );
                if cleared || changed {
                    self.messages_revision = self.messages_revision.wrapping_add(1);
                }
                self.error = None;
                self.busy = false;
            }
            DevinEvent::ActivityLoaded {
                session_id,
                generation,
                activity,
                next_cursor,
                update,
            } if self.is_current(&session_id, generation) => {
                let history_complete = next_cursor.is_none();
                let cleared = update == PageUpdate::Initial && !self.activity.is_empty();
                if update == PageUpdate::Initial {
                    self.activity.clear();
                    self.activity_ids.clear();
                }
                if self.activity_ids.len() < self.activity.len() {
                    self.activity_ids
                        .extend(self.activity.iter().map(|event| event.id.clone()));
                }
                let changed = merge_chronological(
                    &mut self.activity,
                    activity,
                    &mut self.activity_ids,
                    |event| event.id.as_str(),
                    |event| event.timestamp.as_str(),
                );
                if cleared || changed {
                    self.activity_revision = self.activity_revision.wrapping_add(1);
                }
                if self.session_loading && history_complete {
                    self.session_loading = false;
                }
                self.error = None;
                self.busy = false;
            }
            DevinEvent::ActivityDetailsLoaded {
                session_id,
                generation,
                event_id,
                details,
            } if self.is_current(&session_id, generation) => {
                if let Some(activity) = self
                    .activity
                    .iter_mut()
                    .find(|activity| activity.id == event_id)
                {
                    activity.details = Some(details);
                }
                self.error = None;
                self.busy = false;
            }
            DevinEvent::AttachmentFetched {
                session_id,
                generation,
                attachment_id,
                bytes,
            } if self.is_current(&session_id, generation) => {
                self.attachment_previews.insert(attachment_id, bytes);
            }
            DevinEvent::OperationFinished { session_id } => {
                if session_id
                    .as_deref()
                    .is_none_or(|id| self.selected_session.as_deref() == Some(id))
                {
                    self.busy = false;
                    self.error = None;
                }
            }
            DevinEvent::PermissionDenied(error) => {
                self.error = Some(error);
                self.busy = false;
                self.session_loading = false;
            }
            DevinEvent::Failed(error) => {
                self.connection = if error.retry_after_seconds.is_some() {
                    ConnectionState::RateLimited
                } else if matches!(
                    self.connection,
                    ConnectionState::AuthenticationRequired | ConnectionState::Offline
                ) {
                    self.connection
                } else {
                    ConnectionState::Failed
                };
                self.error = Some(error);
                self.busy = false;
                self.session_loading = false;
            }
            _ => {}
        }
    }

    fn is_current(&self, session_id: &str, generation: u64) -> bool {
        self.selected_session.as_deref() == Some(session_id)
            && self.selected_generation == generation
    }

    fn bump_sessions_revision(&mut self) {
        self.sessions_revision = self.sessions_revision.wrapping_add(1);
    }
}

fn loaded<T>(state: &mut ResourceState<T>, items: Vec<T>) {
    state.status = LoadState::Loaded;
    state.items = items;
    state.error = None;
}

fn upsert_loaded<T>(state: &mut ResourceState<T>, item: T, id: impl Fn(&T) -> &str) {
    if let Some(index) = state
        .items
        .iter()
        .position(|existing| id(existing) == id(&item))
    {
        state.items[index] = item;
    } else {
        state.items.push(item);
    }
    state.status = LoadState::Loaded;
    state.error = None;
}

fn resource_state_mut(
    resources: &mut DevinResources,
    section: DevinSection,
) -> (&mut LoadState, &mut Option<String>) {
    match section {
        DevinSection::Review => (&mut resources.reviews.status, &mut resources.reviews.error),
        DevinSection::Repositories => (
            &mut resources.repositories.status,
            &mut resources.repositories.error,
        ),
        DevinSection::Knowledge => (
            &mut resources.knowledge.status,
            &mut resources.knowledge.error,
        ),
        DevinSection::Playbooks => (
            &mut resources.playbooks.status,
            &mut resources.playbooks.error,
        ),
        DevinSection::Automations => (
            &mut resources.automations.status,
            &mut resources.automations.error,
        ),
        DevinSection::Environment => (
            &mut resources.blueprints.status,
            &mut resources.blueprints.error,
        ),
        DevinSection::Integrations => (
            &mut resources.integrations.status,
            &mut resources.integrations.error,
        ),
    }
}

fn resource_kind_state_mut(
    resources: &mut DevinResources,
    kind: DevinResourceKind,
) -> (&mut LoadState, &mut Option<String>) {
    match kind {
        DevinResourceKind::Repositories => (
            &mut resources.repositories.status,
            &mut resources.repositories.error,
        ),
        DevinResourceKind::Documents => (
            &mut resources.documents.status,
            &mut resources.documents.error,
        ),
        DevinResourceKind::Knowledge => (
            &mut resources.knowledge.status,
            &mut resources.knowledge.error,
        ),
        DevinResourceKind::KnowledgeFolders => (
            &mut resources.knowledge_folders.status,
            &mut resources.knowledge_folders.error,
        ),
        DevinResourceKind::KnowledgeSuggestions => (
            &mut resources.knowledge_suggestions.status,
            &mut resources.knowledge_suggestions.error,
        ),
        DevinResourceKind::Playbooks => (
            &mut resources.playbooks.status,
            &mut resources.playbooks.error,
        ),
        DevinResourceKind::Schedules => (
            &mut resources.schedules.status,
            &mut resources.schedules.error,
        ),
        DevinResourceKind::Automations => (
            &mut resources.automations.status,
            &mut resources.automations.error,
        ),
        DevinResourceKind::AutomationCatalog => (
            &mut resources.automation_catalog.status,
            &mut resources.automation_catalog.error,
        ),
        DevinResourceKind::Integrations => (
            &mut resources.integrations.status,
            &mut resources.integrations.error,
        ),
        DevinResourceKind::Reviews => (&mut resources.reviews.status, &mut resources.reviews.error),
        DevinResourceKind::Blueprints => (
            &mut resources.blueprints.status,
            &mut resources.blueprints.error,
        ),
        DevinResourceKind::BlueprintFiles => (
            &mut resources.blueprint_files.status,
            &mut resources.blueprint_files.error,
        ),
        DevinResourceKind::Builds => (&mut resources.builds.status, &mut resources.builds.error),
        DevinResourceKind::Secrets => (&mut resources.secrets.status, &mut resources.secrets.error),
        DevinResourceKind::Insights => (
            &mut resources.insights.status,
            &mut resources.insights.error,
        ),
    }
}

fn resource_kind_has_items(resources: &DevinResources, kind: DevinResourceKind) -> bool {
    match kind {
        DevinResourceKind::Repositories => !resources.repositories.items.is_empty(),
        DevinResourceKind::Documents => !resources.documents.items.is_empty(),
        DevinResourceKind::Knowledge => !resources.knowledge.items.is_empty(),
        DevinResourceKind::KnowledgeFolders => !resources.knowledge_folders.items.is_empty(),
        DevinResourceKind::KnowledgeSuggestions => {
            !resources.knowledge_suggestions.items.is_empty()
        }
        DevinResourceKind::Playbooks => !resources.playbooks.items.is_empty(),
        DevinResourceKind::Schedules => !resources.schedules.items.is_empty(),
        DevinResourceKind::Automations => !resources.automations.items.is_empty(),
        DevinResourceKind::AutomationCatalog => !resources.automation_catalog.items.is_empty(),
        DevinResourceKind::Integrations => !resources.integrations.items.is_empty(),
        DevinResourceKind::Reviews => !resources.reviews.items.is_empty(),
        DevinResourceKind::Blueprints => !resources.blueprints.items.is_empty(),
        DevinResourceKind::BlueprintFiles => !resources.blueprint_files.items.is_empty(),
        DevinResourceKind::Builds => !resources.builds.items.is_empty(),
        DevinResourceKind::Secrets => !resources.secrets.items.is_empty(),
        DevinResourceKind::Insights => !resources.insights.items.is_empty(),
    }
}

pub(crate) fn parse_timestamp_seconds(value: &str) -> Option<i64> {
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
    let suffix = value.get(19..)?;
    let fraction_digits = suffix
        .strip_prefix('.')
        .map(|suffix| suffix.bytes().take_while(u8::is_ascii_digit).take(9));
    let fraction = fraction_digits
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
    let zone = suffix
        .strip_prefix('.')
        .map_or(suffix, |suffix| suffix.trim_start_matches(char::is_numeric));
    let offset = match zone.as_bytes() {
        [] | [b'Z'] => 0,
        [sign @ (b'+' | b'-'), h1, h2, b':', m1, m2] => {
            let hours = i64::from((h1 - b'0') * 10 + (h2 - b'0'));
            let minutes = i64::from((m1 - b'0') * 10 + (m2 - b'0'));
            if hours > 23 || minutes > 59 {
                return None;
            }
            let seconds = hours * 3_600 + minutes * 60;
            if *sign == b'+' { seconds } else { -seconds }
        }
        _ => return None,
    };
    Some(
        i128::from(
            days_from_civil(year, month, day) * 86_400 + hour * 3_600 + minute * 60 + second
                - offset,
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

pub(crate) fn chronological_timestamp(left: &str, right: &str) -> std::cmp::Ordering {
    match (parse_timestamp_nanos(left), parse_timestamp_nanos(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        _ => left.cmp(right),
    }
}

fn append_unique<T>(target: &mut Vec<T>, incoming: Vec<T>, id: impl Fn(&T) -> &str) {
    let mut known = target
        .iter()
        .map(&id)
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    target.extend(
        incoming
            .into_iter()
            .filter(|item| known.insert(id(item).to_owned())),
    );
}

fn merge_chronological<T>(
    target: &mut Vec<T>,
    mut incoming: Vec<T>,
    known: &mut HashSet<String>,
    id: impl Fn(&T) -> &str,
    timestamp: impl Fn(&T) -> &str,
) -> bool {
    incoming.retain(|item| known.insert(id(item).to_owned()));
    if incoming.is_empty() {
        return false;
    }
    incoming.sort_by(|left, right| chronological_timestamp(timestamp(left), timestamp(right)));
    let mut left = std::mem::take(target).into_iter().peekable();
    let mut right = incoming.into_iter().peekable();
    target.reserve(left.len() + right.len());
    while let (Some(existing), Some(added)) = (left.peek(), right.peek()) {
        if chronological_timestamp(timestamp(existing), timestamp(added))
            != std::cmp::Ordering::Greater
        {
            target.push(left.next().expect("peeked existing item"));
        } else {
            target.push(right.next().expect("peeked added item"));
        }
    }
    target.extend(left);
    target.extend(right);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paginated_messages_are_ordered_by_time_across_timestamp_formats() {
        let mut state = DevinState::default();
        let generation = state.select("session".into());
        let message = |id: &str, timestamp: &str| DevinMessage {
            id: id.into(),
            timestamp: timestamp.into(),
            ..Default::default()
        };
        state.apply(DevinEvent::MessagesLoaded {
            session_id: "session".into(),
            generation,
            messages: vec![message("later", "10")],
            next_cursor: Some("next".into()),
            update: PageUpdate::Initial,
        });
        state.apply(DevinEvent::MessagesLoaded {
            session_id: "session".into(),
            generation,
            messages: vec![message("earlier", "1970-01-01T00:00:02Z")],
            next_cursor: None,
            update: PageUpdate::History,
        });

        assert_eq!(
            state
                .messages
                .iter()
                .map(|message| message.id.as_str())
                .collect::<Vec<_>>(),
            ["earlier", "later"]
        );
    }
}
