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
    pub sessions_cursor: Option<String>,
    pub sessions_total: Option<usize>,
    pub sessions_has_next: bool,
    pub last_created_batch: Vec<String>,
    pub last_sessions_refresh: Option<Instant>,
    pub selected_session: Option<String>,
    pub selected_generation: u64,
    pub detail: Option<SessionDetail>,
    pub messages: Vec<DevinMessage>,
    pub messages_cursor: Option<String>,
    pub activity: Vec<Activity>,
    pub activity_cursor: Option<String>,
    /// Downloaded bytes for the selected session's image attachments, keyed by
    /// attachment id, so the transcript can paint the same previews the Agent
    /// paints for local prompt images.
    pub attachment_previews: std::collections::HashMap<String, std::sync::Arc<[u8]>>,
    pub error: Option<DevinError>,
    pub busy: bool,
    #[cfg(debug_assertions)]
    pub preview: bool,
}

impl DevinState {
    #[cfg(debug_assertions)]
    pub(crate) fn seed_preview(&mut self) {
        *self = Self {
            connection: ConnectionState::Connected,
            credential_source: Some(CredentialSource::Environment),
            repository: RepositoryState::Suggested("openai/editur".into()),
            workspace_boundary: Some(
                "Preview data only — no remote Devin session is running.".into(),
            ),
            sessions: preview_sessions(),
            last_sessions_refresh: Some(Instant::now()),
            preview: true,
            ..Self::default()
        };
        self.select_preview("preview-passkeys".into());
    }

    #[cfg(debug_assertions)]
    pub(crate) fn select_preview(&mut self, session_id: String) -> bool {
        if !self.preview {
            return false;
        }
        let Some(summary) = self
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .cloned()
        else {
            return false;
        };
        let generation = self.select(session_id.clone());
        self.apply(DevinEvent::SessionLoaded {
            session_id: session_id.clone(),
            generation,
            detail: SessionDetail {
                summary,
                attachments: vec![
                    Attachment {
                        id: "preview-design".into(),
                        name: "passkey-flow.png".into(),
                        media_type: Some("image/png".into()),
                        size: Some(248_320),
                        url: Some("https://app.devin.ai/".into()),
                    },
                    Attachment {
                        id: "preview-log".into(),
                        name: "test-results.txt".into(),
                        media_type: Some("text/plain".into()),
                        size: Some(8_192),
                        url: None,
                    },
                ],
                pull_requests: vec![PullRequest {
                    id: "preview-pr".into(),
                    title: "Add passkey sign-in and recovery flow".into(),
                    url: "https://github.com/openai/editur/pull/42".into(),
                    status: Some("Ready for review".into()),
                }],
                children: vec![ChildSession {
                    id: "preview-tests".into(),
                    title: "Run cross-platform auth tests".into(),
                    status: "working".into(),
                }],
                usage: Some(Usage {
                    acus: Some(3.72),
                    limit: Some(10.0),
                }),
            },
        });
        self.apply(DevinEvent::MessagesLoaded {
            session_id: session_id.clone(),
            generation,
            messages: vec![
                DevinMessage {
                    id: "preview-message-1".into(),
                    timestamp: "2026-08-15T23:42:00Z".into(),
                    role: "user".into(),
                    text: "Add passkey authentication and keep email recovery as a fallback."
                        .into(),
                    attachment_ids: vec!["preview-design".into()],
                },
                DevinMessage {
                    id: "preview-message-2".into(),
                    timestamp: "2026-08-15T23:48:00Z".into(),
                    role: "devin".into(),
                    text: "I mapped the existing auth flow and implemented **WebAuthn** registration and sign-in in `src/auth/passkeys.rs`. The focused tests pass.".into(),
                    attachment_ids: vec!["preview-log".into()],
                },
                DevinMessage {
                    id: "preview-message-3".into(),
                    timestamp: "2026-08-16T00:05:00Z".into(),
                    role: "devin".into(),
                    text: "Should recovery codes be required during enrollment, or can users add them later from settings?".into(),
                    attachment_ids: Vec::new(),
                },
            ],
            next_cursor: None,
            update: PageUpdate::Initial,
        });
        self.apply(DevinEvent::AttachmentFetched {
            session_id: session_id.clone(),
            generation,
            attachment_id: "preview-design".into(),
            bytes: preview_image_bytes(),
        });
        self.apply(DevinEvent::ActivityLoaded {
            session_id,
            generation,
            activity: vec![
                Activity {
                    id: "preview-activity-1".into(),
                    timestamp: "2026-08-15T23:45:00Z".into(),
                    category: "shell".into(),
                    summary: "Inspected the authentication tests".into(),
                    details: Some(
                        "Found the existing session boundary and recovery coverage.".into(),
                    ),
                    path: Some("tests/auth.rs".into()),
                    command: Some("cargo test auth".into()),
                    ..Activity::default()
                },
                Activity {
                    id: "preview-activity-2".into(),
                    timestamp: "2026-08-15T23:54:00Z".into(),
                    category: "code".into(),
                    summary: "Implemented passkey registration".into(),
                    details: Some("Added challenge validation and credential persistence.".into()),
                    path: Some("src/auth/passkeys.rs".into()),
                    ..Activity::default()
                },
                Activity {
                    id: "preview-activity-3".into(),
                    timestamp: "2026-08-16T00:02:00Z".into(),
                    category: "browser".into(),
                    summary: "Verified the sign-in flow".into(),
                    details: Some(
                        "Registration, sign-in, and fallback recovery all completed.".into(),
                    ),
                    url: Some("https://app.devin.ai/".into()),
                    ..Activity::default()
                },
            ],
            next_cursor: None,
            update: PageUpdate::Initial,
        });
        true
    }

    pub fn select(&mut self, session_id: String) -> u64 {
        self.selected_generation = self.selected_generation.wrapping_add(1);
        self.selected_session = Some(session_id);
        self.detail = None;
        self.messages.clear();
        self.messages_cursor = None;
        self.activity.clear();
        self.activity_cursor = None;
        self.attachment_previews.clear();
        self.error = None;
        self.busy = true;
        self.selected_generation
    }

    pub fn clear_selection(&mut self) {
        self.selected_generation = self.selected_generation.wrapping_add(1);
        self.selected_session = None;
        self.detail = None;
        self.messages.clear();
        self.messages_cursor = None;
        self.activity.clear();
        self.activity_cursor = None;
        self.attachment_previews.clear();
        self.busy = false;
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
                }
                self.detail = Some(detail);
                self.error = None;
                self.busy = false;
            }
            DevinEvent::MessagesLoaded {
                session_id,
                generation,
                messages,
                next_cursor,
                update,
            } if self.is_current(&session_id, generation) => {
                if update == PageUpdate::Initial {
                    self.messages.clear();
                }
                append_unique(&mut self.messages, messages, |message| message.id.as_str());
                self.messages
                    .sort_by(|left, right| chronological(&left.timestamp, &right.timestamp));
                if update != PageUpdate::Refresh {
                    self.messages_cursor = next_cursor;
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
                if update == PageUpdate::Initial {
                    self.activity.clear();
                }
                append_unique(&mut self.activity, activity, |event| event.id.as_str());
                self.activity
                    .sort_by(|left, right| chronological(&left.timestamp, &right.timestamp));
                if update != PageUpdate::Refresh {
                    self.activity_cursor = next_cursor;
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
            }
            _ => {}
        }
    }

    fn is_current(&self, session_id: &str, generation: u64) -> bool {
        self.selected_session.as_deref() == Some(session_id)
            && self.selected_generation == generation
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

/// A small generated PNG so the preview session's image attachment renders a
/// real thumbnail without shipping a fixture.
#[cfg(debug_assertions)]
fn preview_image_bytes() -> std::sync::Arc<[u8]> {
    let mut pixels = image::RgbaImage::new(48, 48);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        *pixel = image::Rgba([36 + (x * 3) as u8, 48 + (y * 2) as u8, 112, 255]);
    }
    let mut bytes = Vec::new();
    let _ = image::DynamicImage::ImageRgba8(pixels).write_to(
        &mut std::io::Cursor::new(&mut bytes),
        image::ImageFormat::Png,
    );
    bytes.into()
}

#[cfg(debug_assertions)]
fn preview_sessions() -> Vec<SessionSummary> {
    vec![
        SessionSummary {
            id: "preview-passkeys".into(),
            title: "Add passkey authentication".into(),
            prompt: Some("Implement passkeys with an email recovery fallback.".into()),
            status: "blocked".into(),
            status_detail: Some("Waiting for your recovery-code decision".into()),
            category: StatusCategory::Waiting,
            origin: Some("slack".into()),
            repository: Some("openai/editur".into()),
            created_at: Some("2026-08-15T23:40:00Z".into()),
            updated_at: Some("2026-08-16T00:05:00Z".into()),
            url: Some("https://app.devin.ai/sessions/preview-passkeys".into()),
            pull_request_count: 1,
            tags: vec!["auth".into(), "frontend".into()],
            ..SessionSummary::default()
        },
        SessionSummary {
            id: "preview-tests".into(),
            title: "Run cross-platform auth tests".into(),
            prompt: Some("Verify the new auth flow on macOS, Windows, and Linux.".into()),
            status: "working".into(),
            status_detail: Some("Running the Windows test matrix".into()),
            category: StatusCategory::Active,
            origin: Some("editur".into()),
            repository: Some("openai/editur".into()),
            created_at: Some("2026-08-15T23:51:00Z".into()),
            updated_at: Some("2026-08-16T00:08:00Z".into()),
            parent_session_id: Some("preview-passkeys".into()),
            url: Some("https://app.devin.ai/sessions/preview-tests".into()),
            tags: vec!["tests".into()],
            ..SessionSummary::default()
        },
        SessionSummary {
            id: "preview-docs".into(),
            title: "Document the plugin API".into(),
            prompt: Some("Write a migration guide for plugin authors.".into()),
            status: "sleeping".into(),
            status_detail: Some("Sleeping until new instructions arrive".into()),
            category: StatusCategory::Sleeping,
            origin: Some("web".into()),
            repository: Some("openai/editur".into()),
            created_at: Some("2026-08-14T14:20:00Z".into()),
            updated_at: Some("2026-08-15T21:14:00Z".into()),
            url: Some("https://app.devin.ai/sessions/preview-docs".into()),
            tags: vec!["docs".into()],
            ..SessionSummary::default()
        },
        SessionSummary {
            id: "preview-release".into(),
            title: "Prepare v0.2 release".into(),
            prompt: Some("Cut the release and draft release notes.".into()),
            status: "finished".into(),
            status_detail: Some("Release published successfully".into()),
            category: StatusCategory::Completed,
            origin: Some("editur".into()),
            repository: Some("openai/editur".into()),
            created_at: Some("2026-08-13T17:10:00Z".into()),
            updated_at: Some("2026-08-14T02:32:00Z".into()),
            url: Some("https://app.devin.ai/sessions/preview-release".into()),
            pull_request_count: 2,
            tags: vec!["release".into()],
            ..SessionSummary::default()
        },
        SessionSummary {
            id: "preview-ci".into(),
            title: "Repair flaky Linux CI".into(),
            prompt: Some("Find and fix the intermittent Linux failure.".into()),
            status: "failed".into(),
            status_detail: Some("Runner lost network access".into()),
            category: StatusCategory::Failed,
            origin: Some("slack".into()),
            repository: Some("openai/editur".into()),
            created_at: Some("2026-08-12T19:06:00Z".into()),
            updated_at: Some("2026-08-12T20:41:00Z".into()),
            url: Some("https://app.devin.ai/sessions/preview-ci".into()),
            tags: vec!["ci".into()],
            ..SessionSummary::default()
        },
        SessionSummary {
            id: "preview-archived".into(),
            title: "Prototype command palette".into(),
            prompt: Some("Explore a compact command palette interaction.".into()),
            status: "finished".into(),
            status_detail: Some("Prototype complete".into()),
            category: StatusCategory::Completed,
            origin: Some("web".into()),
            repository: Some("openai/editur".into()),
            created_at: Some("2026-08-01T15:00:00Z".into()),
            updated_at: Some("2026-08-02T18:26:00Z".into()),
            archived: true,
            url: Some("https://app.devin.ai/sessions/preview-archived".into()),
            tags: vec!["prototype".into()],
            ..SessionSummary::default()
        },
    ]
}

fn chronological(left: &str, right: &str) -> std::cmp::Ordering {
    match (left.parse::<f64>(), right.parse::<f64>()) {
        (Ok(left), Ok(right)) => left
            .partial_cmp(&right)
            .unwrap_or(std::cmp::Ordering::Equal),
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
