use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use super::{
    credentials::Credentials,
    normalize,
    state::{
        Attachment, AutomationCatalog, ConnectionState, DevinError, DevinEvent, DevinResourceKind,
        DevinSection, PageUpdate, RepositoryState, SessionFilters, StatusCategory,
    },
    transport::{
        ENDPOINT, InteractAction, McpTransport, TransportError, V3Method, encode_path_segment,
    },
};
use crate::agent::controller::{MAX_PROMPT_ATTACHMENTS, PromptAttachment};

const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 64;
const ACTIVE_POLL: Duration = Duration::from_secs(4);
const LIST_POLL: Duration = Duration::from_secs(20);
const TERMINAL_POLL: Duration = Duration::from_secs(30);
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_RESOURCE_BODY_BYTES: usize = 512 * 1024;
const MAX_PREVIEW_FETCHES: usize = 8;
const MAX_BATCH_SESSIONS: usize = 10;

#[derive(Clone, Eq, PartialEq)]
pub struct SecretInput {
    pub key: String,
    pub value: String,
    pub kind: String,
}

impl std::fmt::Debug for SecretInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SecretInput")
            .field("key", &self.key)
            .field("value", &"[REDACTED]")
            .field("kind", &self.kind)
            .finish()
    }
}

#[derive(Clone, Default, PartialEq)]
pub struct CreateSessionRequest {
    pub repositories: Vec<String>,
    pub prompt: String,
    pub title: Option<String>,
    pub mode: Option<String>,
    pub playbook_id: Option<String>,
    pub child_playbook_id: Option<String>,
    pub knowledge_ids: Vec<String>,
    pub tags: Vec<String>,
    pub max_acu_limit: Option<u64>,
    pub platform: Option<String>,
    pub resumable: Option<bool>,
    pub session_links: Vec<String>,
    pub structured_output_schema: Option<serde_json::Value>,
    pub structured_output_required: bool,
    pub secret_ids: Vec<String>,
    pub session_secrets: Vec<SecretInput>,
    pub create_as_user_id: Option<String>,
    pub bypass_approval: bool,
    pub parent_session_id: Option<String>,
    pub attachments: Vec<PromptAttachment>,
    pub batch_count: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrudAction {
    Create,
    Update,
    Delete,
}

#[derive(Clone, PartialEq)]
pub enum ResourceMutation {
    TriggerReview {
        pull_request_url: String,
    },
    IndexRepository {
        repository: String,
        branches: Vec<String>,
    },
    RemoveRepositoryIndex {
        repository: String,
    },
    RemoveRepositoryBranch {
        repository: String,
        branch: String,
    },
    Knowledge {
        action: CrudAction,
        id: Option<String>,
        name: String,
        content: String,
        folder: Option<String>,
    },
    DismissKnowledgeSuggestion {
        id: String,
    },
    Playbook {
        action: CrudAction,
        id: Option<String>,
        title: String,
        content: String,
        automation_macro: Option<String>,
    },
    Schedule {
        action: CrudAction,
        id: Option<String>,
        payload: serde_json::Value,
    },
    Automation {
        action: CrudAction,
        id: Option<String>,
        payload: serde_json::Value,
    },
    Blueprint {
        action: CrudAction,
        id: Option<String>,
        repository: Option<String>,
        contents: String,
    },
    DeleteBlueprintFile {
        blueprint_id: String,
        file_id: String,
    },
    TriggerBuild,
    CancelBuild {
        id: String,
    },
    PinBuild {
        id: String,
        pinned: bool,
    },
    CreateSecret(SecretInput),
    DeleteSecret {
        id: String,
    },
}

#[derive(Clone, PartialEq)]
pub enum DevinCommand {
    SetVisible(bool),
    RefreshSessions,
    LoadMoreSessions,
    SetSessionFilters(SessionFilters),
    SelectSession {
        session_id: String,
        generation: u64,
    },
    LoadMoreMessages,
    LoadMoreEvents,
    LoadSection(DevinSection),
    LoadKnowledge(String),
    LoadKnowledgeSuggestion(String),
    LoadPlaybook(String),
    LoadSchedule(String),
    LoadAutomation(String),
    LoadRepositoryWiki {
        repository: String,
    },
    LoadBlueprint(String),
    UploadBlueprintFiles {
        blueprint_id: String,
        attachments: Vec<PromptAttachment>,
    },
    LoadBuildLogs(String),
    LoadBuild(String),
    AskRepositories {
        repositories: Vec<String>,
        question: String,
    },
    GenerateInsight,
    MutateResource(ResourceMutation),
    ReplaceTags(Vec<String>),
    AppendTags(Vec<String>),
    SearchActivity(String),
    FetchActivity(String),
    GatherSessions(Vec<String>),
    CreateSession(CreateSessionRequest),
    SendMessage {
        message: String,
        attachments: Vec<PromptAttachment>,
    },
    RefreshSelected,
    Sleep,
    Archive,
    Unarchive,
    TerminateConfirmed,
    SaveCredentials {
        api_key: String,
        org_id: Option<String>,
    },
    SelectOrganization(String),
    Disconnect,
    Shutdown,
}

pub struct DevinController {
    commands: SyncSender<DevinCommand>,
    events: Receiver<DevinEvent>,
    worker: Option<thread::JoinHandle<()>>,
}

impl DevinController {
    pub fn start(project_root: PathBuf, wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self::start_endpoint(project_root, ENDPOINT.into(), Arc::new(wake))
    }

    fn start_endpoint(
        project_root: PathBuf,
        endpoint: String,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (command_tx, command_rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (event_tx, event_rx) = mpsc::sync_channel(EVENT_CAPACITY);
        let worker = thread::Builder::new()
            .name("editur-devin".into())
            .spawn(move || Worker::new(project_root, endpoint, event_tx, wake).run(command_rx))
            .expect("failed to start Editur Devin controller thread");
        Self {
            commands: command_tx,
            events: event_rx,
            worker: Some(worker),
        }
    }

    pub fn send(&self, command: DevinCommand) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|_| "Devin command could not be sent".to_owned())
    }

    pub fn events(&self) -> &Receiver<DevinEvent> {
        &self.events
    }
}

impl Drop for DevinController {
    fn drop(&mut self) {
        let Some(worker) = self.worker.take() else {
            return;
        };
        loop {
            match self.commands.try_send(DevinCommand::Shutdown) {
                Ok(()) | Err(TrySendError::Disconnected(_)) => break,
                Err(TrySendError::Full(_)) => {
                    self.events.try_iter().for_each(drop);
                    if worker.is_finished() {
                        break;
                    }
                    thread::park_timeout(Duration::from_millis(1));
                }
            }
        }
        while !worker.is_finished() {
            self.events.try_iter().for_each(drop);
            thread::park_timeout(Duration::from_millis(1));
        }
        let _ = worker.join();
    }
}

#[derive(Clone)]
struct SelectedSession {
    id: String,
    generation: u64,
    messages_cursor: Option<String>,
    events_cursor: Option<String>,
    category: StatusCategory,
}

struct Worker {
    project_root: PathBuf,
    endpoint: String,
    events: SyncSender<DevinEvent>,
    wake: Arc<dyn Fn() + Send + Sync>,
    credentials: Option<Credentials>,
    transport: Option<McpTransport>,
    selected: Option<SelectedSession>,
    sessions_cursor: Option<String>,
    session_filters: SessionFilters,
    schedule: PollSchedule,
    repository_resolved: bool,
    /// Attachment ids already fetched (or attempted) for the selected
    /// session, so refresh polls do not re-download previews.
    fetched_previews: HashSet<String>,
    review_url: Option<String>,
}

impl Worker {
    fn new(
        project_root: PathBuf,
        endpoint: String,
        events: SyncSender<DevinEvent>,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            project_root,
            endpoint,
            events,
            wake,
            credentials: None,
            transport: None,
            selected: None,
            sessions_cursor: None,
            session_filters: SessionFilters::default(),
            schedule: PollSchedule::new(Instant::now()),
            repository_resolved: false,
            fetched_previews: HashSet::new(),
            review_url: None,
        }
    }

    fn run(mut self, commands: Receiver<DevinCommand>) {
        loop {
            let now = Instant::now();
            let received = match self.schedule.wait(now) {
                Some(wait) => commands.recv_timeout(wait),
                None => commands.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match received {
                Ok(DevinCommand::Shutdown) | Err(RecvTimeoutError::Disconnected) => return,
                Ok(command) => self.handle(command),
                Err(RecvTimeoutError::Timeout) => self.poll(),
            }
        }
    }

    fn handle(&mut self, command: DevinCommand) {
        match command {
            DevinCommand::SetVisible(visible) => {
                let now = Instant::now();
                self.schedule.set_visible(visible, now);
                if visible {
                    self.resolve_repository();
                    self.poll();
                }
            }
            DevinCommand::RefreshSessions => {
                self.schedule.set_visible(true, Instant::now());
                self.run_request(|worker| worker.refresh_sessions(false));
            }
            DevinCommand::LoadMoreSessions => {
                self.run_request(|worker| worker.refresh_sessions(true))
            }
            DevinCommand::SetSessionFilters(filters) => {
                if !valid_session_filters(&filters) {
                    self.local_error("The Devin session filters are invalid");
                    return;
                }
                self.session_filters = filters.clone();
                self.sessions_cursor = None;
                self.emit(DevinEvent::FiltersChanged(filters));
                self.run_request(|worker| worker.refresh_sessions(false));
            }
            DevinCommand::SelectSession {
                session_id,
                generation,
            } => {
                self.selected = Some(SelectedSession {
                    id: session_id,
                    generation,
                    messages_cursor: None,
                    events_cursor: None,
                    category: StatusCategory::Unknown,
                });
                self.fetched_previews.clear();
                self.run_request(Self::load_selected);
            }
            DevinCommand::LoadMoreMessages => self.run_request(Self::load_messages),
            DevinCommand::LoadMoreEvents => self.run_request(Self::load_events),
            DevinCommand::LoadSection(section) => self.run_section_request(section),
            DevinCommand::LoadKnowledge(id) => {
                if safe_remote_id(&id) {
                    self.run_request(|worker| worker.load_knowledge_detail(&id));
                } else {
                    self.local_error("The Devin knowledge note ID is invalid");
                }
            }
            DevinCommand::LoadKnowledgeSuggestion(id) => {
                if safe_remote_id(&id) {
                    self.run_request(|worker| worker.load_knowledge_suggestion_detail(&id));
                } else {
                    self.local_error("The Devin knowledge suggestion ID is invalid");
                }
            }
            DevinCommand::LoadPlaybook(id) => {
                if safe_remote_id(&id) {
                    self.run_request(|worker| worker.load_playbook_detail(&id));
                } else {
                    self.local_error("The Devin playbook ID is invalid");
                }
            }
            DevinCommand::LoadSchedule(id) => {
                if safe_remote_id(&id) {
                    self.run_request(|worker| worker.load_schedule_detail(&id));
                } else {
                    self.local_error("The Devin schedule ID is invalid");
                }
            }
            DevinCommand::LoadAutomation(id) => {
                if safe_remote_id(&id) {
                    self.run_request(|worker| worker.load_automation_detail(&id));
                } else {
                    self.local_error("The Devin automation ID is invalid");
                }
            }
            DevinCommand::LoadRepositoryWiki { repository } => {
                if normalize_github_repository(&repository).as_deref() != Some(repository.trim()) {
                    self.local_error("Choose a GitHub repository in owner/repository form");
                } else {
                    self.run_request(|worker| worker.load_repository_wiki(repository.trim()));
                }
            }
            DevinCommand::LoadBlueprint(blueprint_id) => {
                if !safe_remote_id(&blueprint_id) {
                    self.local_error("The Devin blueprint ID is invalid");
                } else {
                    self.run_request(|worker| worker.load_blueprint(&blueprint_id));
                }
            }
            DevinCommand::UploadBlueprintFiles {
                blueprint_id,
                attachments,
            } => {
                if !safe_remote_id(&blueprint_id)
                    || attachments.is_empty()
                    || attachments.len() > MAX_PROMPT_ATTACHMENTS
                    || attachments.iter().any(PromptAttachment::is_directory)
                {
                    self.local_error("The blueprint file upload is invalid");
                } else {
                    self.run_request(|worker| {
                        worker.upload_blueprint_files(&blueprint_id, &attachments)
                    });
                }
            }
            DevinCommand::LoadBuildLogs(build_id) => {
                if !safe_remote_id(&build_id) {
                    self.local_error("The Devin build ID is invalid");
                } else {
                    self.run_request(|worker| worker.load_build_logs(&build_id));
                }
            }
            DevinCommand::LoadBuild(build_id) => {
                if safe_remote_id(&build_id) {
                    self.run_request(|worker| worker.load_build_detail(&build_id));
                } else {
                    self.local_error("The Devin build ID is invalid");
                }
            }
            DevinCommand::AskRepositories {
                repositories,
                question,
            } => {
                if repositories.is_empty()
                    || repositories.len() > 10
                    || question.trim().is_empty()
                    || question.len() > MAX_MESSAGE_BYTES
                {
                    self.local_error("Choose up to ten repositories and enter a question");
                } else {
                    self.run_request(|worker| {
                        worker.ask_repositories(&repositories, question.trim())
                    });
                }
            }
            DevinCommand::GenerateInsight => self.run_request(Self::generate_insight),
            DevinCommand::MutateResource(mutation) => {
                if let Err(message) = validate_resource_mutation(&mutation) {
                    self.local_error(message);
                } else {
                    self.run_resource_mutation(mutation);
                }
            }
            DevinCommand::ReplaceTags(tags) => {
                if tags.len() > 100
                    || tags.iter().any(|tag| {
                        tag.trim().is_empty()
                            || tag.len() > 256
                            || tag.chars().any(char::is_control)
                    })
                {
                    self.local_error("Session tags are invalid");
                } else {
                    self.run_request(|worker| worker.replace_tags(tags));
                }
            }
            DevinCommand::AppendTags(tags) => {
                if tags.len() > 100
                    || tags.iter().any(|tag| {
                        tag.trim().is_empty()
                            || tag.len() > 256
                            || tag.chars().any(char::is_control)
                    })
                {
                    self.local_error("Session tags are invalid");
                } else {
                    self.run_request(|worker| worker.append_tags(tags));
                }
            }
            DevinCommand::SearchActivity(query) => {
                if query.trim().is_empty() || query.len() > MAX_MESSAGE_BYTES {
                    self.local_error("Enter an activity search query");
                } else {
                    self.run_request(|worker| worker.search_activity(query.trim()));
                }
            }
            DevinCommand::FetchActivity(event_id) => {
                if !safe_remote_id(&event_id) {
                    self.local_error("The Devin event ID is invalid");
                } else {
                    self.run_request(|worker| worker.fetch_activity(&event_id));
                }
            }
            DevinCommand::GatherSessions(session_ids) => {
                if session_ids.is_empty()
                    || session_ids.len() > MAX_BATCH_SESSIONS
                    || session_ids.iter().any(|id| !safe_remote_id(id))
                {
                    self.local_error("Choose between one and ten Devin sessions to wait for");
                } else {
                    self.run_request(|worker| worker.gather_sessions(&session_ids));
                }
            }
            DevinCommand::CreateSession(request) => {
                if let Err(message) = validate_create_request(&request) {
                    self.local_error(message);
                } else {
                    self.run_request(|worker| worker.create_sessions(request));
                }
            }
            DevinCommand::SendMessage {
                message,
                attachments,
            } => {
                if (message.trim().is_empty() && attachments.is_empty())
                    || message.len() > MAX_MESSAGE_BYTES
                    || attachments.len() > MAX_PROMPT_ATTACHMENTS
                    || attachments.iter().any(PromptAttachment::is_directory)
                {
                    self.local_error("The Devin message or attachment list is invalid");
                } else {
                    self.run_request(|worker| worker.send_message(message.trim(), &attachments));
                }
            }
            DevinCommand::RefreshSelected => self.run_request(Self::refresh_selected),
            DevinCommand::Sleep => {
                self.run_request(|worker| worker.lifecycle(InteractAction::Sleep))
            }
            DevinCommand::Archive => {
                self.run_request(|worker| worker.lifecycle(InteractAction::Archive))
            }
            DevinCommand::Unarchive => {
                self.run_request(|worker| worker.lifecycle(InteractAction::Unarchive))
            }
            DevinCommand::TerminateConfirmed => {
                self.run_request(|worker| worker.lifecycle(InteractAction::Terminate));
            }
            DevinCommand::SaveCredentials { api_key, org_id } => {
                let connected = Credentials::new(
                    api_key,
                    org_id,
                    super::credentials::CredentialSource::Keyring,
                )
                .map_err(TransportError::Protocol)
                .and_then(|credentials| self.connect_and_store(credentials));
                match connected {
                    Ok(()) => {
                        let now = Instant::now();
                        self.schedule.set_visible(true, now);
                        self.schedule.sessions_succeeded(now);
                        if self.selected.is_none() {
                            self.schedule.next_selected = self.schedule.next_sessions;
                        }
                        self.poll();
                    }
                    Err(TransportError::OrganizationSelectionRequired(organizations)) => {
                        self.emit(DevinEvent::OrganizationsDiscovered(organizations));
                    }
                    Err(error) => self.request_failed(error),
                }
            }
            DevinCommand::SelectOrganization(org_id) => {
                if !safe_remote_id(&org_id) {
                    self.local_error("The Devin organization ID is invalid");
                    return;
                }
                let connected = self
                    .credentials_for_connection()
                    .and_then(|credentials| {
                        credentials
                            .with_org_id(org_id)
                            .map_err(TransportError::Protocol)
                    })
                    .and_then(|credentials| {
                        self.emit(DevinEvent::OrganizationSwitching);
                        self.selected = None;
                        self.sessions_cursor = None;
                        self.fetched_previews.clear();
                        self.connect(credentials.clone())?;
                        if credentials.source() == super::credentials::CredentialSource::Keyring {
                            credentials.save().map_err(TransportError::Protocol)?;
                        }
                        Ok(())
                    });
                match connected {
                    Ok(()) => self.poll(),
                    Err(error) => self.request_failed(error),
                }
            }
            DevinCommand::Disconnect => {
                self.transport = None;
                self.selected = None;
                self.sessions_cursor = None;
                self.schedule.set_visible(false, Instant::now());
                match Credentials::delete() {
                    Ok(()) => {
                        self.credentials = None;
                        self.emit(DevinEvent::CredentialsChanged(None));
                        self.emit(DevinEvent::ConnectionChanged(
                            ConnectionState::AuthenticationRequired,
                        ));
                    }
                    Err(message) => self.local_error(&message),
                }
            }
            DevinCommand::Shutdown => {}
        }
    }

    fn poll(&mut self) {
        if !self.schedule.visible {
            return;
        }
        let now = Instant::now();
        if self.schedule.due_sessions(now) {
            if let Err(error) = self.refresh_sessions(false) {
                self.request_failed(error);
                return;
            }
            self.schedule.sessions_succeeded(now);
            if self.selected.is_none() {
                self.schedule.next_selected = self.schedule.next_sessions;
            }
        }
        if self.selected.is_some() && self.schedule.due_selected(now) {
            if let Err(error) = self.refresh_selected() {
                self.request_failed(error);
                return;
            }
            let terminal = self.selected.as_ref().is_some_and(|selected| {
                matches!(
                    selected.category,
                    StatusCategory::Completed | StatusCategory::Failed
                )
            });
            self.schedule.selected_succeeded(now, terminal);
        }
        self.schedule.failures = 0;
        self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connected));
    }

    fn run_request(&mut self, request: impl FnOnce(&mut Self) -> Result<(), TransportError>) {
        match request(self) {
            Ok(()) => {
                self.schedule.failures = 0;
                self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connected));
            }
            Err(error) => self.request_failed(error),
        }
    }

    fn run_section_request(&mut self, section: DevinSection) {
        self.emit(DevinEvent::ResourceLoading(section));
        match self.load_section(section) {
            Ok(()) => {
                self.schedule.failures = 0;
                self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connected));
            }
            Err(TransportError::Forbidden | TransportError::Unavailable) => {
                self.emit(DevinEvent::ResourceForbidden(section));
            }
            Err(error @ (TransportError::Authentication | TransportError::CredentialsMissing)) => {
                self.request_failed(error)
            }
            Err(error) => self.emit(DevinEvent::ResourceFailed {
                section,
                error: DevinError {
                    message: error.user_message().into(),
                    retry_after_seconds: error.retry_after_seconds(),
                },
            }),
        }
    }

    fn run_resource_mutation(&mut self, mutation: ResourceMutation) {
        let section = mutation.section();
        let kind = mutation.kind();
        self.emit(DevinEvent::ResourceKindLoading(kind));
        let result = self
            .mutate_resource(mutation)
            .and_then(|reload| reload.then(|| self.load_section(section)).transpose())
            .map(drop);
        match result {
            Ok(()) => self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connected)),
            Err(TransportError::Forbidden | TransportError::Unavailable) => {
                self.emit(DevinEvent::ResourceKindForbidden(kind))
            }
            Err(error @ (TransportError::Authentication | TransportError::CredentialsMissing)) => {
                self.request_failed(error)
            }
            Err(error) => self.emit(DevinEvent::ResourceKindFailed {
                kind,
                error: DevinError {
                    message: error.user_message().into(),
                    retry_after_seconds: error.retry_after_seconds(),
                },
            }),
        }
    }

    fn load_optional_resource(
        &mut self,
        kind: DevinResourceKind,
        request: impl FnOnce(&mut Self) -> Result<DevinEvent, TransportError>,
    ) -> Result<(), TransportError> {
        self.emit(DevinEvent::ResourceKindLoading(kind));
        match request(self) {
            Ok(event) => self.emit(event),
            Err(error @ (TransportError::Authentication | TransportError::CredentialsMissing)) => {
                return Err(error);
            }
            Err(TransportError::Forbidden | TransportError::Unavailable) => {
                self.emit(DevinEvent::ResourceKindForbidden(kind));
            }
            Err(error) => self.emit(DevinEvent::ResourceKindFailed {
                kind,
                error: DevinError {
                    message: error.user_message().into(),
                    retry_after_seconds: error.retry_after_seconds(),
                },
            }),
        }
        Ok(())
    }

    fn mutate_resource(&mut self, mutation: ResourceMutation) -> Result<bool, TransportError> {
        match mutation {
            ResourceMutation::TriggerReview { pull_request_url } => {
                self.review_url = Some(pull_request_url.clone());
                let mut result = self.ensure_connected()?.v3(
                    V3Method::Post,
                    "/pr-reviews",
                    Some(&serde_json::json!({ "pr_url": pull_request_url })),
                )?;
                if let Some(result) = result.as_object_mut() {
                    result.insert("pr_url".into(), serde_json::json!(pull_request_url));
                }
                self.emit(DevinEvent::ReviewsLoaded(normalize::reviews(&result)));
                Ok(false)
            }
            ResourceMutation::IndexRepository {
                repository,
                branches,
            } => {
                self.ensure_connected()?.v3beta(
                    V3Method::Put,
                    &format!(
                        "/repositories/{}/indexing",
                        encode_path_segment(&repository)
                    ),
                    Some(&serde_json::json!({ "branch_names": branches })),
                )?;
                Ok(true)
            }
            ResourceMutation::RemoveRepositoryIndex { repository } => {
                self.ensure_connected()?.v3beta(
                    V3Method::Delete,
                    &format!(
                        "/repositories/{}/indexing",
                        encode_path_segment(&repository)
                    ),
                    None,
                )?;
                Ok(true)
            }
            ResourceMutation::RemoveRepositoryBranch { repository, branch } => {
                self.ensure_connected()?.v3beta(
                    V3Method::Delete,
                    &format!(
                        "/repositories/{}/indexing/branches/{}",
                        encode_path_segment(&repository),
                        encode_path_segment(&branch)
                    ),
                    None,
                )?;
                Ok(true)
            }
            ResourceMutation::Knowledge {
                action,
                id,
                name,
                content,
                folder,
            } => {
                let mut arguments = serde_json::Map::new();
                insert_optional(&mut arguments, "note_id", id);
                insert_nonempty(&mut arguments, "name", name);
                insert_nonempty(&mut arguments, "content", content);
                insert_optional(&mut arguments, "folder", folder);
                self.ensure_connected()?.manage(
                    "devin_knowledge_manage",
                    action.as_str(),
                    arguments,
                )?;
                Ok(true)
            }
            ResourceMutation::DismissKnowledgeSuggestion { id } => {
                let mut arguments = serde_json::Map::new();
                arguments.insert("suggestion_id".into(), serde_json::json!(id));
                self.ensure_connected()?.manage(
                    "devin_knowledge_manage",
                    "dismiss_suggestion",
                    arguments,
                )?;
                Ok(true)
            }
            ResourceMutation::Playbook {
                action,
                id,
                title,
                content,
                automation_macro,
            } => {
                let mut arguments = serde_json::Map::new();
                insert_optional(&mut arguments, "playbook_id", id);
                insert_nonempty(&mut arguments, "title", title);
                insert_nonempty(&mut arguments, "content", content);
                insert_optional(&mut arguments, "automation_macro", automation_macro);
                self.ensure_connected()?.manage(
                    "devin_playbook_manage",
                    action.as_str(),
                    arguments,
                )?;
                Ok(true)
            }
            ResourceMutation::Schedule {
                action,
                id,
                payload,
            } => {
                let mut arguments = payload.as_object().cloned().unwrap_or_default();
                insert_optional(&mut arguments, "schedule_id", id);
                self.ensure_connected()?.manage(
                    "devin_schedule_manage",
                    action.as_str(),
                    arguments,
                )?;
                Ok(true)
            }
            ResourceMutation::Automation {
                action,
                id,
                payload,
            } => {
                let path = match id.as_deref() {
                    Some(id) => format!("/automations/{}", encode_path_segment(id)),
                    None => "/automations".into(),
                };
                let method = match action {
                    CrudAction::Create => V3Method::Post,
                    CrudAction::Update => V3Method::Patch,
                    CrudAction::Delete => V3Method::Delete,
                };
                self.ensure_connected()?.v3(
                    method,
                    &path,
                    (action != CrudAction::Delete).then_some(&payload),
                )?;
                Ok(true)
            }
            ResourceMutation::Blueprint {
                action,
                id,
                repository,
                contents,
            } => {
                let path = match id.as_deref() {
                    Some(id) => format!("/snapshot-setup/blueprints/{}", encode_path_segment(id)),
                    None => "/snapshot-setup/blueprints".into(),
                };
                let method = match action {
                    CrudAction::Create => V3Method::Post,
                    CrudAction::Update => V3Method::Patch,
                    CrudAction::Delete => V3Method::Delete,
                };
                let body = match action {
                    CrudAction::Create => serde_json::json!({
                        "repo_name": repository,
                        "contents": contents,
                    }),
                    CrudAction::Update => serde_json::json!({ "contents": contents }),
                    CrudAction::Delete => serde_json::Value::Null,
                };
                self.ensure_connected()?.v3beta(
                    method,
                    &path,
                    (action != CrudAction::Delete).then_some(&body),
                )?;
                Ok(true)
            }
            ResourceMutation::DeleteBlueprintFile {
                blueprint_id,
                file_id,
            } => {
                self.ensure_connected()?.v3beta(
                    V3Method::Delete,
                    &format!(
                        "/snapshot-setup/blueprints/{}/files/{}",
                        encode_path_segment(&blueprint_id),
                        encode_path_segment(&file_id)
                    ),
                    None,
                )?;
                self.load_blueprint(&blueprint_id)?;
                Ok(false)
            }
            ResourceMutation::TriggerBuild => {
                self.ensure_connected()?.v3beta(
                    V3Method::Post,
                    "/snapshot-setup/builds",
                    Some(&serde_json::json!({})),
                )?;
                Ok(true)
            }
            ResourceMutation::CancelBuild { id } => {
                self.ensure_connected()?.v3beta(
                    V3Method::Post,
                    &format!("/snapshot-setup/builds/{}/cancel", encode_path_segment(&id)),
                    Some(&serde_json::json!({})),
                )?;
                Ok(true)
            }
            ResourceMutation::PinBuild { id, pinned } => {
                self.ensure_connected()?.v3beta(
                    if pinned {
                        V3Method::Post
                    } else {
                        V3Method::Delete
                    },
                    &format!("/snapshot-setup/builds/{}/pin", encode_path_segment(&id)),
                    pinned.then_some(&serde_json::json!({})),
                )?;
                Ok(true)
            }
            ResourceMutation::CreateSecret(secret) => {
                self.ensure_connected()?.v3(
                    V3Method::Post,
                    "/secrets",
                    Some(&serde_json::json!({
                        "type": secret.kind,
                        "key": secret.key,
                        "value": secret.value,
                        "is_sensitive": true,
                    })),
                )?;
                Ok(true)
            }
            ResourceMutation::DeleteSecret { id } => {
                self.ensure_connected()?.v3(
                    V3Method::Delete,
                    &format!("/secrets/{}", encode_path_segment(&id)),
                    None,
                )?;
                Ok(true)
            }
        }
    }

    fn load_section(&mut self, section: DevinSection) -> Result<(), TransportError> {
        match section {
            DevinSection::Review => {
                let Some(pull_request_url) = self.review_url.clone() else {
                    self.emit(DevinEvent::ReviewsLoaded(Vec::new()));
                    return Ok(());
                };
                let mut result = self.ensure_connected()?.v3(
                    V3Method::Get,
                    &format!(
                        "/pr-reviews?pr_url={}",
                        encode_path_segment(&pull_request_url)
                    ),
                    None,
                )?;
                if let Some(result) = result.as_object_mut() {
                    result.insert("pr_url".into(), serde_json::json!(pull_request_url));
                }
                self.emit(DevinEvent::ReviewsLoaded(normalize::reviews(&result)));
            }
            DevinSection::Repositories => {
                let result = self.ensure_connected()?.list_repositories()?;
                let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
                let mut repositories = normalize::repositories(&payload);
                if let Ok(available) =
                    self.ensure_connected()?
                        .v3beta(V3Method::Get, "/repositories", None)
                {
                    for available in normalize::repositories(&available) {
                        if let Some(repository) = repositories.iter_mut().find(|repository| {
                            available.id.ends_with(&repository.name)
                                || repository.id.ends_with(&available.name)
                        }) {
                            repository.id = available.id;
                            repository.indexed = available.indexed;
                            repository.indexing_status = available.indexing_status;
                            repository.branches = available.branches;
                        } else {
                            repositories.push(available);
                        }
                    }
                }
                if let Ok(indexed) =
                    self.ensure_connected()?
                        .v3beta(V3Method::Get, "/repositories/indexing", None)
                {
                    for indexed in normalize::repositories(&indexed) {
                        if let Some(repository) = repositories.iter_mut().find(|repository| {
                            repository.id == indexed.id
                                || repository.name == indexed.name
                                || indexed.id.ends_with(&repository.name)
                        }) {
                            repository.indexed = true;
                            repository.indexing_status = indexed.indexing_status;
                            repository.branches = indexed.branches;
                        } else {
                            repositories.push(indexed);
                        }
                    }
                }
                self.emit(DevinEvent::RepositoriesLoaded(repositories));
            }
            DevinSection::Knowledge => {
                let result =
                    self.ensure_connected()?
                        .v3(V3Method::Get, "/knowledge/notes", None)?;
                self.emit(DevinEvent::KnowledgeLoaded(normalize::knowledge(&result)));
                self.load_optional_resource(DevinResourceKind::KnowledgeFolders, |worker| {
                    let folders =
                        worker
                            .ensure_connected()?
                            .v3(V3Method::Get, "/knowledge/folders", None)?;
                    Ok(DevinEvent::KnowledgeFoldersLoaded(
                        normalize::knowledge_folders(&folders),
                    ))
                })?;
                if self
                    .ensure_connected()?
                    .supports_action("devin_knowledge_manage", "list_suggestions")
                {
                    self.load_optional_resource(
                        DevinResourceKind::KnowledgeSuggestions,
                        |worker| {
                            let suggestions = worker.ensure_connected()?.manage(
                                "devin_knowledge_manage",
                                "list_suggestions",
                                serde_json::Map::new(),
                            )?;
                            let suggestions = normalize::tool_payload(suggestions)
                                .map_err(TransportError::Protocol)?;
                            Ok(DevinEvent::KnowledgeSuggestionsLoaded(
                                normalize::knowledge_suggestions(&suggestions),
                            ))
                        },
                    )?;
                }
            }
            DevinSection::Playbooks => {
                let result = self
                    .ensure_connected()?
                    .v3(V3Method::Get, "/playbooks", None)?;
                self.emit(DevinEvent::PlaybooksLoaded(normalize::playbooks(&result)));
            }
            DevinSection::Automations => {
                if self
                    .transport
                    .as_ref()
                    .is_some_and(|transport| transport.capabilities().has("devin_schedule_manage"))
                {
                    self.load_optional_resource(DevinResourceKind::Schedules, |worker| {
                        let result =
                            worker
                                .ensure_connected()?
                                .v3(V3Method::Get, "/schedules", None)?;
                        Ok(DevinEvent::SchedulesLoaded(normalize::schedules(&result)))
                    })?;
                }
                self.load_optional_resource(DevinResourceKind::Automations, |worker| {
                    let value =
                        worker
                            .ensure_connected()?
                            .v3(V3Method::Get, "/automations", None)?;
                    Ok(DevinEvent::AutomationsLoaded(normalize::automations(
                        &value,
                    )))
                })?;
                self.load_optional_resource(DevinResourceKind::AutomationCatalog, |worker| {
                    let schemas = worker.ensure_connected()?.v3(
                        V3Method::Get,
                        "/automations/schemas",
                        None,
                    )?;
                    let templates = worker.ensure_connected()?.v3(
                        V3Method::Get,
                        "/automations/templates",
                        None,
                    )?;
                    Ok(DevinEvent::AutomationCatalogLoaded(AutomationCatalog {
                        schemas,
                        templates,
                    }))
                })?;
            }
            DevinSection::Environment => {
                self.load_optional_resource(DevinResourceKind::Blueprints, |worker| {
                    let value = worker.ensure_connected()?.v3beta(
                        V3Method::Get,
                        "/snapshot-setup/blueprints",
                        None,
                    )?;
                    Ok(DevinEvent::BlueprintsLoaded(normalize::blueprints(&value)))
                })?;
                self.load_optional_resource(DevinResourceKind::Builds, |worker| {
                    let value = worker.ensure_connected()?.v3beta(
                        V3Method::Get,
                        "/snapshot-setup/builds",
                        None,
                    )?;
                    Ok(DevinEvent::BuildsLoaded(normalize::builds(&value)))
                })?;
                self.load_optional_resource(DevinResourceKind::Secrets, |worker| {
                    let value = worker
                        .ensure_connected()?
                        .v3(V3Method::Get, "/secrets", None)?;
                    Ok(DevinEvent::SecretsLoaded(normalize::secrets(&value)))
                })?;
            }
            DevinSection::Integrations => {
                let result = self.ensure_connected()?.list_integrations()?;
                let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
                self.emit(DevinEvent::IntegrationsLoaded(normalize::integrations(
                    &payload,
                )));
            }
        }
        Ok(())
    }

    fn load_repository_wiki(&mut self, repository: &str) -> Result<(), TransportError> {
        let structure = self.ensure_connected()?.wiki_structure(repository)?;
        let structure = normalize::tool_payload(structure).map_err(TransportError::Protocol)?;
        let result = self.ensure_connected()?.wiki_contents(repository)?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let mut documents = normalize::wiki_documents(&structure, repository);
        documents.extend(normalize::wiki_documents(&payload, repository));
        self.emit(DevinEvent::DocumentsLoaded(documents));
        Ok(())
    }

    fn load_knowledge_detail(&mut self, id: &str) -> Result<(), TransportError> {
        let result = self.ensure_connected()?.v3(
            V3Method::Get,
            &format!("/knowledge/notes/{}", encode_path_segment(id)),
            None,
        )?;
        let note = normalize::knowledge_detail(&result).ok_or_else(|| {
            TransportError::Protocol("knowledge note response was invalid".into())
        })?;
        self.emit(DevinEvent::KnowledgeDetailLoaded(note));
        Ok(())
    }

    fn load_knowledge_suggestion_detail(&mut self, id: &str) -> Result<(), TransportError> {
        let mut arguments = serde_json::Map::new();
        arguments.insert("suggestion_id".into(), serde_json::json!(id));
        let result = self.ensure_connected()?.manage(
            "devin_knowledge_manage",
            "get_suggestion",
            arguments,
        )?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let suggestion = normalize::knowledge_suggestion_detail(&payload).ok_or_else(|| {
            TransportError::Protocol("knowledge suggestion response was invalid".into())
        })?;
        self.emit(DevinEvent::KnowledgeSuggestionDetailLoaded(suggestion));
        Ok(())
    }

    fn load_playbook_detail(&mut self, id: &str) -> Result<(), TransportError> {
        let result = self.ensure_connected()?.v3(
            V3Method::Get,
            &format!("/playbooks/{}", encode_path_segment(id)),
            None,
        )?;
        let playbook = normalize::playbook_detail(&result)
            .ok_or_else(|| TransportError::Protocol("playbook response was invalid".into()))?;
        self.emit(DevinEvent::PlaybookDetailLoaded(playbook));
        Ok(())
    }

    fn load_schedule_detail(&mut self, id: &str) -> Result<(), TransportError> {
        let result = self.ensure_connected()?.v3(
            V3Method::Get,
            &format!("/schedules/{}", encode_path_segment(id)),
            None,
        )?;
        let schedule = normalize::schedule_detail(&result)
            .ok_or_else(|| TransportError::Protocol("schedule response was invalid".into()))?;
        self.emit(DevinEvent::ScheduleDetailLoaded(schedule));
        Ok(())
    }

    fn load_automation_detail(&mut self, id: &str) -> Result<(), TransportError> {
        let value = self.ensure_connected()?.v3(
            V3Method::Get,
            &format!("/automations/{}", encode_path_segment(id)),
            None,
        )?;
        let automation = normalize::automation_detail(&value)
            .ok_or_else(|| TransportError::Protocol("automation response was invalid".into()))?;
        self.emit(DevinEvent::AutomationDetailLoaded(automation));
        Ok(())
    }

    fn load_blueprint(&mut self, blueprint_id: &str) -> Result<(), TransportError> {
        let base = format!(
            "/snapshot-setup/blueprints/{}",
            encode_path_segment(blueprint_id)
        );
        let detail = self
            .ensure_connected()?
            .v3beta(V3Method::Get, &base, None)?;
        let contents_response =
            self.ensure_connected()?
                .v3beta(V3Method::Get, &format!("{base}/contents"), None)?;
        let contents_url = contents_response
            .get("url")
            .or_else(|| contents_response.get("contents_url"))
            .and_then(serde_json::Value::as_str)
            .filter(|url| url.starts_with("https://"))
            .map(str::to_owned);
        let contents = if let Some(url) = contents_url.as_deref() {
            let bytes = self.ensure_connected()?.fetch_attachment(url)?;
            if bytes.len() > MAX_RESOURCE_BODY_BYTES {
                return Err(TransportError::Oversized);
            }
            Some(
                String::from_utf8(bytes)
                    .map_err(|_| TransportError::Protocol("blueprint YAML was not UTF-8".into()))?,
            )
        } else {
            None
        };
        let mut blueprints = normalize::blueprints(&serde_json::json!({
            "blueprints": [detail]
        }));
        let mut blueprint = blueprints
            .pop()
            .ok_or_else(|| TransportError::Protocol("blueprint response was invalid".into()))?;
        blueprint.contents_url = contents_url;
        blueprint.contents = contents;
        self.emit(DevinEvent::BlueprintLoaded(blueprint));

        let files =
            self.ensure_connected()?
                .v3beta(V3Method::Get, &format!("{base}/files"), None)?;
        self.emit(DevinEvent::BlueprintFilesLoaded(
            normalize::blueprint_files(&files),
        ));
        Ok(())
    }

    fn upload_blueprint_files(
        &mut self,
        blueprint_id: &str,
        attachments: &[PromptAttachment],
    ) -> Result<(), TransportError> {
        for attachment in attachments {
            self.ensure_connected()?
                .upload_blueprint_file(blueprint_id, attachment.path())?;
        }
        self.load_blueprint(blueprint_id)
    }

    fn load_build_logs(&mut self, build_id: &str) -> Result<(), TransportError> {
        let result = self.ensure_connected()?.v3beta(
            V3Method::Get,
            &format!(
                "/snapshot-setup/builds/{}/logs",
                encode_path_segment(build_id)
            ),
            None,
        )?;
        let logs_url = result
            .get("url")
            .or_else(|| result.get("logs_url"))
            .and_then(serde_json::Value::as_str)
            .filter(|url| url.starts_with("https://"))
            .ok_or_else(|| TransportError::Protocol("build log URL was invalid".into()))?
            .to_owned();
        self.emit(DevinEvent::BuildLogLoaded {
            build_id: build_id.into(),
            logs_url,
        });
        Ok(())
    }

    fn load_build_detail(&mut self, build_id: &str) -> Result<(), TransportError> {
        let value = self.ensure_connected()?.v3beta(
            V3Method::Get,
            &format!("/snapshot-setup/builds/{}", encode_path_segment(build_id)),
            None,
        )?;
        let build = normalize::build_detail(&value)
            .ok_or_else(|| TransportError::Protocol("build response was invalid".into()))?;
        self.emit(DevinEvent::BuildDetailLoaded(build));
        Ok(())
    }

    fn ask_repositories(
        &mut self,
        repositories: &[String],
        question: &str,
    ) -> Result<(), TransportError> {
        let result = self
            .ensure_connected()?
            .ask_question(repositories, question)?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        self.emit(DevinEvent::DocumentsLoaded(normalize::wiki_documents(
            &payload,
            &repositories.join(", "),
        )));
        Ok(())
    }

    fn generate_insight(&mut self) -> Result<(), TransportError> {
        let selected = self
            .selected
            .as_ref()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        if !safe_remote_id(&selected.id) {
            return Err(TransportError::Protocol("invalid Devin session ID".into()));
        }
        let path = format!("/sessions/{}/insights", selected.id);
        self.ensure_connected()?.v3(
            V3Method::Post,
            &format!("{path}/generate"),
            Some(&serde_json::json!({})),
        )?;
        let result = self.ensure_connected()?.v3(V3Method::Get, &path, None)?;
        self.emit(DevinEvent::InsightsLoaded(normalize::insights(&result)));
        Ok(())
    }

    fn ensure_connected(&mut self) -> Result<&mut McpTransport, TransportError> {
        if self.transport.is_none() {
            let credentials = self.credentials_for_connection()?;
            self.connect(credentials)?;
        }
        self.transport
            .as_mut()
            .ok_or_else(|| TransportError::Protocol("transport was not created".into()))
    }

    fn credentials_for_connection(&mut self) -> Result<Credentials, TransportError> {
        if let Some(credentials) = self.credentials.clone() {
            return Ok(credentials);
        }
        let credentials = Credentials::load()
            .map_err(TransportError::Protocol)?
            .ok_or(TransportError::CredentialsMissing)?;
        self.credentials = Some(credentials.clone());
        Ok(credentials)
    }

    fn connect(&mut self, credentials: Credentials) -> Result<(), TransportError> {
        let source = credentials.source();
        self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connecting));
        let transport = self.open_transport(credentials.clone())?;
        self.emit(DevinEvent::CapabilitiesChanged(transport.capabilities()));
        self.emit(DevinEvent::OrganizationSelected(transport.org_id().into()));
        self.credentials = Some(credentials);
        self.transport = Some(transport);
        self.emit(DevinEvent::CredentialsChanged(Some(source)));
        self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connected));
        Ok(())
    }

    fn connect_and_store(&mut self, credentials: Credentials) -> Result<(), TransportError> {
        let source = credentials.source();
        self.credentials = Some(credentials.clone());
        self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connecting));
        let mut transport = self.open_transport(credentials.clone())?;
        self.emit(DevinEvent::CapabilitiesChanged(transport.capabilities()));
        self.emit(DevinEvent::OrganizationSelected(transport.org_id().into()));
        let result = transport.search(None)?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let (sessions, next_cursor, total, has_next) =
            normalize::sessions(&payload).map_err(TransportError::Protocol)?;
        credentials.save().map_err(TransportError::Protocol)?;
        self.sessions_cursor = next_cursor.clone();
        self.transport = Some(transport);
        self.emit(DevinEvent::CredentialsChanged(Some(source)));
        self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connected));
        self.emit(DevinEvent::SessionsLoaded {
            sessions,
            next_cursor,
            total,
            has_next,
            append: false,
        });
        Ok(())
    }

    fn open_transport(&self, credentials: Credentials) -> Result<McpTransport, TransportError> {
        if self.endpoint == ENDPOINT {
            McpTransport::connect(credentials)
        } else {
            McpTransport::connect_endpoint(credentials, &self.endpoint)
        }
    }

    fn refresh_sessions(&mut self, append: bool) -> Result<(), TransportError> {
        let cursor = append
            .then_some(self.sessions_cursor.as_deref())
            .flatten()
            .map(str::to_owned);
        let filters = self.session_filters.clone();
        let result = self
            .ensure_connected()?
            .search_with(cursor.as_deref(), &filters)?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let (sessions, next_cursor, total, has_next) =
            normalize::sessions(&payload).map_err(TransportError::Protocol)?;
        self.sessions_cursor = next_cursor.clone();
        self.emit(DevinEvent::SessionsLoaded {
            sessions,
            next_cursor,
            total,
            has_next,
            append,
        });
        Ok(())
    }

    fn refresh_selected(&mut self) -> Result<(), TransportError> {
        self.refresh_selected_with(PageUpdate::Refresh)
    }

    fn load_selected(&mut self) -> Result<(), TransportError> {
        self.refresh_selected_with(PageUpdate::Initial)
    }

    fn refresh_selected_with(&mut self, update: PageUpdate) -> Result<(), TransportError> {
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        let session_path = format!("/sessions/{}", encode_path_segment(&selected.id));
        let result = self
            .ensure_connected()?
            .v3(V3Method::Get, &session_path, None)?;
        let mut detail = normalize::detail(&result).map_err(TransportError::Protocol)?;
        let attachment_path = format!("{session_path}/attachments");
        if let Ok(result) = self
            .ensure_connected()?
            .v3(V3Method::Get, &attachment_path, None)
        {
            let attachments = normalize::attachments(&result);
            if !attachments.is_empty() {
                detail.attachments = attachments;
            }
        }
        for child in detail.children.iter_mut().take(20) {
            if child.status != "unknown" || !safe_remote_id(&child.id) {
                continue;
            }
            let child_path = format!("/sessions/{}", encode_path_segment(&child.id));
            let Ok(result) = self
                .ensure_connected()?
                .v3(V3Method::Get, &child_path, None)
            else {
                continue;
            };
            let Ok(child_detail) = normalize::detail(&result) else {
                continue;
            };
            child.title = child_detail.summary.title;
            child.status = child_detail.summary.status;
        }
        if let Some(current) = self.selected.as_mut()
            && current.id == selected.id
            && current.generation == selected.generation
        {
            current.category = detail.summary.category;
        }
        let previews = detail
            .attachments
            .iter()
            .filter(|attachment| is_image_attachment(attachment))
            .map(|attachment| (attachment.id.clone(), attachment.name.clone()))
            .collect::<Vec<_>>();
        self.emit(DevinEvent::SessionLoaded {
            session_id: selected.id.clone(),
            generation: selected.generation,
            detail,
        });
        self.load_message_page(update)?;
        self.load_event_page(update)?;
        self.fetch_attachment_previews(&selected, previews);
        Ok(())
    }

    /// Downloads image attachments so the transcript can paint the same
    /// previews the Agent paints for local prompt images. Previews are an
    /// enhancement: failures are swallowed and never retried for the
    /// selection, and the text transcript stands on its own without them.
    fn fetch_attachment_previews(
        &mut self,
        selected: &SelectedSession,
        previews: Vec<(String, String)>,
    ) {
        for (attachment_id, name) in previews.into_iter().take(MAX_PREVIEW_FETCHES) {
            if !self.fetched_previews.insert(attachment_id.clone()) {
                continue;
            }
            let Ok(bytes) = self
                .ensure_connected()
                .and_then(|transport| transport.fetch_session_attachment(&attachment_id, &name))
            else {
                continue;
            };
            self.emit(DevinEvent::AttachmentFetched {
                session_id: selected.id.clone(),
                generation: selected.generation,
                attachment_id,
                bytes: bytes.into(),
            });
        }
    }

    fn load_messages(&mut self) -> Result<(), TransportError> {
        self.load_message_page(PageUpdate::History)
    }

    fn load_message_page(&mut self, update: PageUpdate) -> Result<(), TransportError> {
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        let cursor = (update == PageUpdate::History)
            .then_some(selected.messages_cursor.as_deref())
            .flatten();
        let mut path = format!(
            "/sessions/{}/messages?first=100",
            encode_path_segment(&selected.id)
        );
        if let Some(cursor) = cursor {
            path.push_str("&after=");
            path.push_str(&encode_path_segment(cursor));
        }
        let payload = self.ensure_connected()?.v3(V3Method::Get, &path, None)?;
        let (messages, next_cursor) =
            normalize::messages(&payload).map_err(TransportError::Protocol)?;
        if update != PageUpdate::Refresh
            && let Some(current) = self.selected.as_mut()
            && current.id == selected.id
            && current.generation == selected.generation
        {
            current.messages_cursor = next_cursor.clone();
        }
        self.emit(DevinEvent::MessagesLoaded {
            session_id: selected.id,
            generation: selected.generation,
            messages,
            next_cursor,
            update,
        });
        Ok(())
    }

    fn load_events(&mut self) -> Result<(), TransportError> {
        self.load_event_page(PageUpdate::History)
    }

    fn load_event_page(&mut self, update: PageUpdate) -> Result<(), TransportError> {
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        if !self
            .ensure_connected()?
            .capabilities()
            .has("devin_session_events")
        {
            if update == PageUpdate::Initial {
                self.emit(DevinEvent::ActivityLoaded {
                    session_id: selected.id,
                    generation: selected.generation,
                    activity: Vec::new(),
                    next_cursor: None,
                    update,
                });
            }
            return Ok(());
        }
        let result = self.ensure_connected()?.events(
            &selected.id,
            (update == PageUpdate::History)
                .then_some(selected.events_cursor.as_deref())
                .flatten(),
        )?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let (activity, next_cursor) =
            normalize::activity(&payload).map_err(TransportError::Protocol)?;
        if update != PageUpdate::Refresh
            && let Some(current) = self.selected.as_mut()
            && current.id == selected.id
            && current.generation == selected.generation
        {
            current.events_cursor = next_cursor.clone();
        }
        self.emit(DevinEvent::ActivityLoaded {
            session_id: selected.id,
            generation: selected.generation,
            activity,
            next_cursor,
            update,
        });
        Ok(())
    }

    fn create_sessions(&mut self, request: CreateSessionRequest) -> Result<(), TransportError> {
        let mut attachment_urls = Vec::with_capacity(request.attachments.len());
        for attachment in &request.attachments {
            attachment_urls.push(
                self.ensure_connected()?
                    .upload_attachment(attachment.path())?,
            );
        }
        let mut session = serde_json::Map::new();
        session.insert("prompt".into(), serde_json::json!(request.prompt.trim()));
        session.insert("repos".into(), serde_json::json!(request.repositories));
        insert_optional(&mut session, "title", request.title);
        insert_optional(&mut session, "devin_mode", request.mode);
        insert_optional(&mut session, "playbook_id", request.playbook_id);
        insert_optional(&mut session, "child_playbook_id", request.child_playbook_id);
        insert_optional(&mut session, "platform", request.platform);
        insert_optional(&mut session, "create_as_user_id", request.create_as_user_id);
        insert_optional(&mut session, "parent_session_id", request.parent_session_id);
        if !request.knowledge_ids.is_empty() {
            session.insert(
                "knowledge_ids".into(),
                serde_json::json!(request.knowledge_ids),
            );
        }
        if !request.tags.is_empty() {
            session.insert("tags".into(), serde_json::json!(request.tags));
        }
        if let Some(limit) = request.max_acu_limit {
            session.insert("max_acu_limit".into(), serde_json::json!(limit));
        }
        if let Some(resumable) = request.resumable {
            session.insert("resumable".into(), serde_json::json!(resumable));
        }
        if !request.session_links.is_empty() {
            session.insert(
                "session_links".into(),
                serde_json::json!(request.session_links),
            );
        }
        if let Some(schema) = request.structured_output_schema {
            session.insert("structured_output_schema".into(), schema);
            session.insert(
                "structured_output_required".into(),
                serde_json::json!(request.structured_output_required),
            );
        }
        if !request.secret_ids.is_empty() {
            session.insert("secret_ids".into(), serde_json::json!(request.secret_ids));
        }
        if !request.session_secrets.is_empty() {
            session.insert(
                "session_secrets".into(),
                serde_json::Value::Array(
                    request
                        .session_secrets
                        .into_iter()
                        .map(|secret| {
                            serde_json::json!({
                                "key": secret.key,
                                "value": secret.value,
                                "sensitive": true,
                            })
                        })
                        .collect(),
                ),
            );
        }
        if !attachment_urls.is_empty() {
            session.insert("attachment_urls".into(), serde_json::json!(attachment_urls));
        }
        if request.bypass_approval {
            session.insert("bypass_approval".into(), serde_json::json!(true));
        }

        let count = request.batch_count.max(1);
        let mut sessions = Vec::with_capacity(count);
        for _ in 0..count {
            let result = self.ensure_connected()?.v3(
                V3Method::Post,
                "/sessions",
                Some(&serde_json::Value::Object(session.clone())),
            )?;
            sessions.push(
                normalize::detail(&result)
                    .map_err(TransportError::Protocol)?
                    .summary,
            );
        }
        let ids = sessions
            .iter()
            .map(|session| session.id.clone())
            .collect::<Vec<_>>();
        for session in sessions {
            self.emit(DevinEvent::SessionCreated(session));
        }
        self.emit(DevinEvent::BatchCreated(ids));
        Ok(())
    }

    fn replace_tags(&mut self, tags: Vec<String>) -> Result<(), TransportError> {
        self.update_tags(V3Method::Put, tags)
    }

    fn append_tags(&mut self, tags: Vec<String>) -> Result<(), TransportError> {
        self.update_tags(V3Method::Post, tags)
    }

    fn update_tags(&mut self, method: V3Method, tags: Vec<String>) -> Result<(), TransportError> {
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        self.ensure_connected()?.v3(
            method,
            &format!("/sessions/{}/tags", encode_path_segment(&selected.id)),
            Some(&serde_json::json!({ "tags": tags })),
        )?;
        self.refresh_selected()
    }

    fn search_activity(&mut self, query: &str) -> Result<(), TransportError> {
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        let result = self
            .ensure_connected()?
            .event_action(&selected.id, "search", query)?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let (activity, _) = normalize::activity(&payload).map_err(TransportError::Protocol)?;
        self.emit(DevinEvent::ActivityLoaded {
            session_id: selected.id,
            generation: selected.generation,
            activity,
            next_cursor: None,
            update: PageUpdate::Refresh,
        });
        Ok(())
    }

    fn fetch_activity(&mut self, event_id: &str) -> Result<(), TransportError> {
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        let result = self
            .ensure_connected()?
            .event_action(&selected.id, "details", event_id)?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let details = normalize::activity_details(&payload).map_err(TransportError::Protocol)?;
        self.emit(DevinEvent::ActivityDetailsLoaded {
            session_id: selected.id,
            generation: selected.generation,
            event_id: event_id.into(),
            details,
        });
        Ok(())
    }

    fn gather_sessions(&mut self, session_ids: &[String]) -> Result<(), TransportError> {
        let result = self.ensure_connected()?.gather(session_ids)?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        if let Ok((sessions, _, _, _)) = normalize::sessions(&payload)
            && !sessions.is_empty()
        {
            self.emit(DevinEvent::SessionsLoaded {
                sessions,
                next_cursor: self.sessions_cursor.clone(),
                total: None,
                has_next: self.sessions_cursor.is_some(),
                append: true,
            });
        }
        self.emit(DevinEvent::OperationFinished { session_id: None });
        Ok(())
    }

    fn send_message(
        &mut self,
        message: &str,
        attachments: &[PromptAttachment],
    ) -> Result<(), TransportError> {
        let mut urls = Vec::with_capacity(attachments.len());
        for attachment in attachments {
            urls.push(
                self.ensure_connected()?
                    .upload_attachment(attachment.path())?,
            );
        }
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        if urls.is_empty() {
            let result = self.ensure_connected()?.interact(
                &selected.id,
                InteractAction::SendMessage {
                    message,
                    attachment_ids: &[],
                },
            )?;
            normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        } else {
            self.ensure_connected()?
                .send_v3_message(&selected.id, message, &urls)?;
        }
        self.emit(DevinEvent::OperationFinished {
            session_id: Some(selected.id),
        });
        self.refresh_selected()
    }

    fn lifecycle(&mut self, action: InteractAction<'_>) -> Result<(), TransportError> {
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        let result = self.ensure_connected()?.interact(&selected.id, action)?;
        normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        self.emit(DevinEvent::OperationFinished {
            session_id: Some(selected.id),
        });
        self.refresh_selected()?;
        self.refresh_sessions(false)
    }

    fn resolve_repository(&mut self) {
        if self.repository_resolved {
            return;
        }
        self.repository_resolved = true;
        let repository = repository_from_origin(&self.project_root)
            .map(RepositoryState::Suggested)
            .unwrap_or(RepositoryState::SelectionRequired);
        self.emit(DevinEvent::RepositoryResolved(repository));
        self.emit(DevinEvent::WorkspaceBoundary(workspace_boundary(
            &self.project_root,
        )));
    }

    fn request_failed(&mut self, error: TransportError) {
        if let TransportError::OrganizationSelectionRequired(organizations) = error {
            self.transport = None;
            self.emit(DevinEvent::OrganizationsDiscovered(organizations));
            self.schedule.set_visible(false, Instant::now());
            return;
        }
        if error == TransportError::CredentialsMissing {
            self.transport = None;
            self.emit(DevinEvent::CredentialsChanged(None));
            self.emit(DevinEvent::ConnectionChanged(
                ConnectionState::AuthenticationRequired,
            ));
            self.schedule.set_visible(false, Instant::now());
            return;
        }
        if matches!(
            error,
            TransportError::Authentication | TransportError::Offline
        ) {
            self.transport = None;
        }
        if matches!(
            error,
            TransportError::Forbidden | TransportError::Unavailable
        ) {
            self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connected));
            self.emit(DevinEvent::PermissionDenied(DevinError {
                message: error.user_message().into(),
                retry_after_seconds: None,
            }));
            return;
        }
        let connection = match error {
            TransportError::Authentication => ConnectionState::AuthenticationRequired,
            TransportError::RateLimited(_) => ConnectionState::RateLimited,
            TransportError::Offline => ConnectionState::Offline,
            _ => ConnectionState::Failed,
        };
        self.emit(DevinEvent::ConnectionChanged(connection));
        self.emit(DevinEvent::Failed(DevinError {
            message: error.user_message().into(),
            retry_after_seconds: error.retry_after_seconds(),
        }));
        if error == TransportError::Authentication {
            self.schedule.set_visible(false, Instant::now());
        } else {
            self.schedule
                .failed(Instant::now(), error.retry_after_seconds());
        }
    }

    fn local_error(&self, message: &str) {
        self.emit(DevinEvent::Failed(DevinError {
            message: message.into(),
            retry_after_seconds: None,
        }));
    }

    fn emit(&self, event: DevinEvent) {
        if self.events.send(event).is_ok() {
            (self.wake)();
        }
    }
}

struct PollSchedule {
    visible: bool,
    next_sessions: Instant,
    next_selected: Instant,
    failures: u32,
}

impl PollSchedule {
    fn new(now: Instant) -> Self {
        Self {
            visible: false,
            next_sessions: now,
            next_selected: now,
            failures: 0,
        }
    }

    fn set_visible(&mut self, visible: bool, now: Instant) {
        self.visible = visible;
        if visible {
            self.next_sessions = now;
            self.next_selected = now;
        }
    }

    fn wait(&self, now: Instant) -> Option<Duration> {
        self.visible.then(|| {
            self.next_sessions
                .min(self.next_selected)
                .saturating_duration_since(now)
        })
    }

    fn due_sessions(&self, now: Instant) -> bool {
        self.visible && now >= self.next_sessions
    }

    fn due_selected(&self, now: Instant) -> bool {
        self.visible && now >= self.next_selected
    }

    fn sessions_succeeded(&mut self, now: Instant) {
        self.next_sessions = now + LIST_POLL;
    }

    fn selected_succeeded(&mut self, now: Instant, terminal: bool) {
        self.next_selected = now + if terminal { TERMINAL_POLL } else { ACTIVE_POLL };
    }

    fn failed(&mut self, now: Instant, retry_after_seconds: Option<u64>) {
        self.failures = self.failures.saturating_add(1);
        let exponential = 2_u64.saturating_pow(self.failures.min(5));
        let jitter = u64::from(self.failures.wrapping_mul(137) % 1_000);
        let delay = retry_after_seconds
            .map(Duration::from_secs)
            .unwrap_or_else(|| Duration::from_secs(exponential) + Duration::from_millis(jitter));
        self.next_sessions = now + delay;
        self.next_selected = now + delay;
    }
}

/// Whether an attachment is an image worth previewing inline.
fn is_image_attachment(attachment: &Attachment) -> bool {
    let by_media_type = attachment
        .media_type
        .as_deref()
        .is_some_and(|media_type| media_type.to_ascii_lowercase().starts_with("image/"));
    let by_extension = attachment
        .name
        .rsplit_once('.')
        .is_some_and(|(_, extension)| {
            matches!(
                extension.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp"
            )
        });
    by_media_type || by_extension
}

fn repository_from_origin(root: &Path) -> Option<String> {
    let output = Command::new("git")
        .args(["-C"])
        .arg(root)
        .args(["config", "--get-all", "remote.origin.url"])
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > 64 * 1024 {
        return None;
    }
    let output = String::from_utf8(output.stdout).ok()?;
    let mut repositories = output
        .lines()
        .filter_map(normalize_github_repository)
        .collect::<Vec<_>>();
    repositories.sort();
    repositories.dedup();
    (repositories.len() == 1).then(|| repositories.remove(0))
}

pub fn normalize_github_repository(remote: &str) -> Option<String> {
    let remote = remote.trim().trim_end_matches('/').trim_end_matches(".git");
    let path = if let Some(path) = remote.strip_prefix("git@github.com:") {
        path
    } else if let Some(path) = remote.strip_prefix("ssh://git@github.com/") {
        path
    } else if let Some(path) = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("http://github.com/"))
        .or_else(|| remote.strip_prefix("git://github.com/"))
    {
        path
    } else if !remote.contains(':') && remote.split('/').count() == 2 {
        remote
    } else {
        return None;
    };
    let mut parts = path.split('/');
    let owner = parts.next()?;
    let repository = parts.next()?;
    if parts.next().is_some()
        || owner.is_empty()
        || repository.is_empty()
        || !owner.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
        || !repository.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
    {
        return None;
    }
    Some(format!("{owner}/{repository}"))
}

fn workspace_boundary(root: &Path) -> String {
    let branch = git_text(root, &["branch", "--show-current"]);
    let dirty =
        git_text(root, &["status", "--porcelain"]).is_some_and(|status| !status.trim().is_empty());
    let ahead = git_text(root, &["rev-list", "--count", "@{upstream}..HEAD"])
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0);
    workspace_boundary_message(branch.as_deref(), dirty, ahead)
}

fn workspace_boundary_message(branch: Option<&str>, dirty: bool, ahead: u64) -> String {
    if ahead > 0 {
        return format!(
            "You have {ahead} unpushed commit{} on `{}`; Devin cannot see {}.",
            if ahead == 1 { "" } else { "s" },
            branch
                .filter(|branch| !branch.is_empty())
                .unwrap_or("this branch"),
            if dirty {
                "them or your uncommitted changes"
            } else {
                "them"
            }
        );
    }
    if dirty {
        return "You have uncommitted changes; Devin cannot see them.".into();
    }
    "Devin works from the remote repository. Local uncommitted or unpushed changes are not visible to it.".into()
}

fn git_text(root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() || output.stdout.len() > 64 * 1024 {
        return None;
    }
    String::from_utf8(output.stdout)
        .ok()
        .map(|value| value.trim().to_owned())
}

impl CrudAction {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Create => "create",
            Self::Update => "update",
            Self::Delete => "delete",
        }
    }
}

impl ResourceMutation {
    const fn section(&self) -> DevinSection {
        match self {
            Self::TriggerReview { .. } => DevinSection::Review,
            Self::IndexRepository { .. }
            | Self::RemoveRepositoryIndex { .. }
            | Self::RemoveRepositoryBranch { .. } => DevinSection::Repositories,
            Self::Knowledge { .. } | Self::DismissKnowledgeSuggestion { .. } => {
                DevinSection::Knowledge
            }
            Self::Playbook { .. } => DevinSection::Playbooks,
            Self::Schedule { .. } | Self::Automation { .. } => DevinSection::Automations,
            Self::Blueprint { .. }
            | Self::DeleteBlueprintFile { .. }
            | Self::TriggerBuild
            | Self::CancelBuild { .. }
            | Self::PinBuild { .. }
            | Self::CreateSecret(_)
            | Self::DeleteSecret { .. } => DevinSection::Environment,
        }
    }

    const fn kind(&self) -> DevinResourceKind {
        match self {
            Self::TriggerReview { .. } => DevinResourceKind::Reviews,
            Self::IndexRepository { .. }
            | Self::RemoveRepositoryIndex { .. }
            | Self::RemoveRepositoryBranch { .. } => DevinResourceKind::Repositories,
            Self::Knowledge { .. } => DevinResourceKind::Knowledge,
            Self::DismissKnowledgeSuggestion { .. } => DevinResourceKind::KnowledgeSuggestions,
            Self::Playbook { .. } => DevinResourceKind::Playbooks,
            Self::Schedule { .. } => DevinResourceKind::Schedules,
            Self::Automation { .. } => DevinResourceKind::Automations,
            Self::Blueprint { .. } => DevinResourceKind::Blueprints,
            Self::DeleteBlueprintFile { .. } => DevinResourceKind::BlueprintFiles,
            Self::TriggerBuild | Self::CancelBuild { .. } | Self::PinBuild { .. } => {
                DevinResourceKind::Builds
            }
            Self::CreateSecret(_) | Self::DeleteSecret { .. } => DevinResourceKind::Secrets,
        }
    }
}

fn validate_create_request(request: &CreateSessionRequest) -> Result<(), &'static str> {
    let batch_count = request.batch_count.max(1);
    if request.prompt.trim().is_empty()
        || request.prompt.len() > MAX_MESSAGE_BYTES
        || request.repositories.is_empty()
        || request.repositories.len() > 10
        || request.repositories.iter().any(|repository| {
            normalize_github_repository(repository).as_deref() != Some(repository.trim())
        })
        || batch_count > MAX_BATCH_SESSIONS
        || request.attachments.len() > MAX_PROMPT_ATTACHMENTS
        || request
            .attachments
            .iter()
            .any(PromptAttachment::is_directory)
        || request.max_acu_limit.is_some_and(|limit| limit == 0)
        || request
            .structured_output_schema
            .as_ref()
            .is_some_and(|schema| {
                serde_json::to_vec(schema).map_or(true, |body| body.len() > MAX_RESOURCE_BODY_BYTES)
            })
        || request.session_secrets.iter().any(|secret| {
            secret.key.trim().is_empty()
                || secret.key.len() > 256
                || secret.key.chars().any(char::is_control)
                || secret.value.is_empty()
                || secret.value.len() > MAX_MESSAGE_BYTES
                || !matches!(secret.kind.as_str(), "cookie" | "key-value" | "totp")
        })
    {
        return Err("The Devin session request is invalid");
    }
    Ok(())
}

fn valid_session_filters(filters: &SessionFilters) -> bool {
    let values = [
        filters.origin.as_str(),
        filters.repository.as_str(),
        filters.playbook_id.as_str(),
        filters.schedule_id.as_str(),
        filters.user_id.as_str(),
        filters.parent_session_id.as_str(),
        filters.category.as_str(),
        filters.status.as_str(),
        filters.created_after.as_str(),
        filters.created_before.as_str(),
        filters.updated_after.as_str(),
        filters.updated_before.as_str(),
    ];
    values
        .into_iter()
        .all(|value| value.len() <= 8 * 1024 && !value.chars().any(char::is_control))
        && filters.tags.len() <= 100
        && filters
            .tags
            .iter()
            .all(|tag| !tag.is_empty() && tag.len() <= 256 && !tag.chars().any(char::is_control))
}

fn validate_resource_mutation(mutation: &ResourceMutation) -> Result<(), &'static str> {
    let invalid_id = |id: &Option<String>| id.as_deref().is_none_or(|id| !safe_remote_id(id));
    let invalid_text = |text: &str| {
        text.len() > MAX_RESOURCE_BODY_BYTES || text.chars().any(|character| character == '\0')
    };
    let invalid_json = |value: &serde_json::Value| {
        serde_json::to_vec(value).map_or(true, |body| body.len() > MAX_RESOURCE_BODY_BYTES)
    };
    let invalid = match mutation {
        ResourceMutation::TriggerReview { pull_request_url } => !valid_review_url(pull_request_url),
        ResourceMutation::IndexRepository {
            repository,
            branches,
        } => {
            repository.trim().is_empty()
                || repository.len() > 2_048
                || branches.len() > 100
                || branches
                    .iter()
                    .any(|branch| branch.is_empty() || branch.len() > 1_024)
        }
        ResourceMutation::RemoveRepositoryIndex { repository } => repository.trim().is_empty(),
        ResourceMutation::RemoveRepositoryBranch { repository, branch } => {
            repository.trim().is_empty() || branch.trim().is_empty()
        }
        ResourceMutation::Knowledge {
            action,
            id,
            name,
            content,
            ..
        } => {
            (*action != CrudAction::Create && invalid_id(id))
                || (*action != CrudAction::Delete
                    && (name.trim().is_empty() || invalid_text(content)))
        }
        ResourceMutation::DismissKnowledgeSuggestion { id } => !safe_remote_id(id),
        ResourceMutation::Playbook {
            action,
            id,
            title,
            content,
            ..
        } => {
            (*action != CrudAction::Create && invalid_id(id))
                || (*action != CrudAction::Delete
                    && (title.trim().is_empty() || invalid_text(content)))
        }
        ResourceMutation::Schedule {
            action,
            id,
            payload,
        }
        | ResourceMutation::Automation {
            action,
            id,
            payload,
        } => (*action != CrudAction::Create && invalid_id(id)) || invalid_json(payload),
        ResourceMutation::Blueprint {
            action,
            id,
            contents,
            ..
        } => (*action != CrudAction::Create && invalid_id(id)) || invalid_text(contents),
        ResourceMutation::DeleteBlueprintFile {
            blueprint_id,
            file_id,
        } => !safe_remote_id(blueprint_id) || !safe_remote_id(file_id),
        ResourceMutation::TriggerBuild => false,
        ResourceMutation::CancelBuild { id } | ResourceMutation::PinBuild { id, .. } => {
            !safe_remote_id(id)
        }
        ResourceMutation::CreateSecret(secret) => {
            secret.key.trim().is_empty()
                || secret.key.len() > 256
                || secret.key.chars().any(char::is_control)
                || secret.value.is_empty()
                || secret.value.len() > MAX_MESSAGE_BYTES
                || !matches!(secret.kind.as_str(), "cookie" | "key-value" | "totp")
        }
        ResourceMutation::DeleteSecret { id } => !safe_remote_id(id),
    };
    if invalid {
        Err("The Devin resource request is invalid")
    } else {
        Ok(())
    }
}

fn valid_review_url(value: &str) -> bool {
    let value = value.trim();
    value.len() <= 8 * 1024
        && (value.starts_with("https://github.com/") || value.starts_with("https://gitlab.com/"))
        && (value.contains("/pull/") || value.contains("/merge_requests/"))
        && !value.chars().any(char::is_control)
}

fn insert_nonempty(
    target: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
    value: String,
) {
    if !value.is_empty() {
        target.insert(name.into(), serde_json::json!(value));
    }
}

fn insert_optional(
    target: &mut serde_json::Map<String, serde_json::Value>,
    name: &str,
    value: Option<String>,
) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        target.insert(name.into(), serde_json::json!(value));
    }
}

fn safe_remote_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 1_024
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
            mpsc,
        },
        thread,
        time::{Duration, Instant},
    };

    use serde_json::{Value, json};

    type FakeMcpServer = (
        String,
        Arc<Mutex<Vec<String>>>,
        Arc<AtomicBool>,
        thread::JoinHandle<()>,
    );

    #[test]
    fn polling_pauses_while_hidden_and_backoff_delays_the_next_request() {
        let now = Instant::now();
        let mut schedule = super::PollSchedule::new(now);
        assert_eq!(schedule.wait(now), None);

        schedule.set_visible(true, now);
        assert_eq!(schedule.wait(now), Some(Duration::ZERO));

        schedule.failed(now, None);
        assert!(schedule.wait(now).unwrap() >= Duration::from_secs(2));
    }

    #[test]
    fn missing_credentials_open_the_connect_state_without_an_error() {
        let (event_tx, event_rx) = mpsc::sync_channel(8);
        let mut worker = super::Worker::new(
            std::env::temp_dir(),
            super::ENDPOINT.into(),
            event_tx,
            Arc::new(|| {}),
        );
        let now = Instant::now();
        worker.schedule.set_visible(true, now);

        worker.request_failed(super::TransportError::CredentialsMissing);

        assert_eq!(
            event_rx.try_iter().collect::<Vec<_>>(),
            vec![
                super::DevinEvent::CredentialsChanged(None),
                super::DevinEvent::ConnectionChanged(
                    super::ConnectionState::AuthenticationRequired,
                ),
            ]
        );
        assert_eq!(worker.schedule.wait(now), None);
    }

    #[test]
    fn organization_discovery_pauses_polling_until_the_user_selects_one() {
        let (event_tx, event_rx) = mpsc::sync_channel(8);
        let mut worker = super::Worker::new(
            std::env::temp_dir(),
            super::ENDPOINT.into(),
            event_tx,
            Arc::new(|| {}),
        );
        let now = Instant::now();
        worker.schedule.set_visible(true, now);
        let organizations = vec![crate::devin::DevinOrganization {
            id: "org-1".into(),
            name: "Sanitized".into(),
        }];

        worker.request_failed(super::TransportError::OrganizationSelectionRequired(
            organizations.clone(),
        ));

        assert_eq!(
            event_rx.try_iter().collect::<Vec<_>>(),
            vec![super::DevinEvent::OrganizationsDiscovered(organizations)]
        );
        assert_eq!(worker.schedule.wait(now), None);
    }

    #[test]
    fn permission_failure_does_not_disconnect_unrelated_devin_features() {
        let (event_tx, event_rx) = mpsc::sync_channel(8);
        let mut worker = super::Worker::new(
            std::env::temp_dir(),
            super::ENDPOINT.into(),
            event_tx,
            Arc::new(|| {}),
        );
        worker.schedule.set_visible(true, Instant::now());

        worker.request_failed(super::TransportError::Forbidden);

        let events = event_rx.try_iter().collect::<Vec<_>>();
        assert!(matches!(
            events.as_slice(),
            [
                super::DevinEvent::ConnectionChanged(super::ConnectionState::Connected),
                super::DevinEvent::PermissionDenied(_)
            ]
        ));
        assert!(worker.schedule.visible);

        worker.request_failed(super::TransportError::Unavailable);
        let events = event_rx.try_iter().collect::<Vec<_>>();
        assert!(matches!(
            events.as_slice(),
            [
                super::DevinEvent::ConnectionChanged(super::ConnectionState::Connected),
                super::DevinEvent::PermissionDenied(_)
            ]
        ));
    }

    #[test]
    fn only_images_qualify_for_preview_downloads() {
        let image = super::Attachment {
            id: "a".into(),
            name: "mock.png".into(),
            media_type: None,
            size: None,
            url: Some("https://storage.example/mock.png?sig=abc".into()),
        };
        assert!(super::is_image_attachment(&image));

        let by_media_type = super::Attachment {
            name: "mock".into(),
            media_type: Some("image/webp".into()),
            ..image.clone()
        };
        assert!(super::is_image_attachment(&by_media_type));

        let log = super::Attachment {
            name: "results.txt".into(),
            media_type: Some("text/plain".into()),
            ..image.clone()
        };
        assert!(!super::is_image_attachment(&log));

        let unlinked = super::Attachment { url: None, ..image };
        assert!(super::is_image_attachment(&unlinked));
    }

    #[test]
    fn github_remotes_normalize_without_accepting_arbitrary_hosts() {
        assert_eq!(
            super::normalize_github_repository("git@github.com:editur/editor.git").as_deref(),
            Some("editur/editor")
        );
        assert_eq!(
            super::normalize_github_repository("https://example.com/editur/editor.git"),
            None
        );
    }

    #[test]
    fn request_only_secret_values_are_redacted_from_debug_output() {
        let secret = super::SecretInput {
            key: "TOKEN".into(),
            value: "must-never-appear".into(),
            kind: "key-value".into(),
        };

        assert!(!format!("{secret:?}").contains("must-never-appear"));
    }

    #[test]
    fn remote_boundary_calls_out_unpushed_work_before_generic_dirty_work() {
        assert_eq!(
            super::workspace_boundary_message(Some("feature"), true, 2),
            "You have 2 unpushed commits on `feature`; Devin cannot see them or your uncommitted changes."
        );
        assert_eq!(
            super::workspace_boundary_message(None, true, 0),
            "You have uncommitted changes; Devin cannot see them."
        );
    }

    #[cfg(feature = "network")]
    #[test]
    fn reconnect_reuses_credentials_loaded_for_this_app_run() {
        let (endpoint, _requests, stop, server) = fake_mcp_server();
        let (event_tx, _event_rx) = mpsc::sync_channel(16);
        let mut worker =
            super::Worker::new(std::env::temp_dir(), endpoint, event_tx, Arc::new(|| {}));
        let credentials = super::super::credentials::Credentials::new(
            "cog_test-only".into(),
            None,
            super::super::credentials::CredentialSource::Environment,
        )
        .unwrap();

        worker.connect(credentials).unwrap();
        worker.transport = None;

        assert!(worker.credentials.is_some());
        worker.ensure_connected().unwrap();

        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    #[cfg(feature = "network")]
    #[test]
    fn fake_mcp_drives_the_complete_interactive_session_flow() {
        let (endpoint, requests, stop, server) = fake_mcp_server();
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let mut worker =
            super::Worker::new(std::env::temp_dir(), endpoint, event_tx, Arc::new(|| {}));
        let credentials = super::super::credentials::Credentials::new(
            "cog_test-only".into(),
            None,
            super::super::credentials::CredentialSource::Environment,
        )
        .unwrap();
        worker.connect(credentials).unwrap();
        worker
            .create_sessions(super::CreateSessionRequest {
                repositories: vec!["editur/editor".into()],
                prompt: "Fix it".into(),
                batch_count: 1,
                ..super::CreateSessionRequest::default()
            })
            .unwrap();

        let mut state = super::super::state::DevinState::default();
        let generation = state.select("session-1".into());
        worker.selected = Some(super::SelectedSession {
            id: "session-1".into(),
            generation,
            messages_cursor: None,
            events_cursor: None,
            category: super::super::state::StatusCategory::Unknown,
        });
        worker.load_selected().unwrap();
        worker.load_events().unwrap();
        worker.search_activity("tests").unwrap();
        worker.fetch_activity("event-1").unwrap();
        worker.append_tags(vec!["triage".into()]).unwrap();
        worker.replace_tags(vec!["ready".into()]).unwrap();
        worker.generate_insight().unwrap();
        worker.send_message("Continue", &[]).unwrap();
        worker.lifecycle(super::InteractAction::Sleep).unwrap();
        worker.lifecycle(super::InteractAction::Archive).unwrap();
        worker.lifecycle(super::InteractAction::Unarchive).unwrap();
        worker.lifecycle(super::InteractAction::Terminate).unwrap();
        event_rx.try_iter().for_each(|event| state.apply(event));

        let event_arguments = requests
            .lock()
            .unwrap()
            .iter()
            .filter_map(|request| request.split("\r\n\r\n").nth(1))
            .filter_map(|body| serde_json::from_str::<Value>(body).ok())
            .filter(|body| body["params"]["name"] == "devin_session_events")
            .map(|body| body["params"]["arguments"].clone())
            .collect::<Vec<_>>();
        assert!(event_arguments.contains(&json!({
            "session_id": "session-1",
            "action": "search",
            "query": "tests"
        })));
        assert!(event_arguments.contains(&json!({
            "session_id": "session-1",
            "action": "list",
            "limit": 100,
            "after": "event-next"
        })));
        assert!(event_arguments.contains(&json!({
            "session_id": "session-1",
            "action": "details",
            "event_ids": ["event-1"]
        })));
        let requests = requests.lock().unwrap();
        assert!(requests.iter().any(|request| {
            request.starts_with("POST /v3/organizations/org-fixture/sessions ")
                && request.contains(r#""prompt":"Fix it""#)
        }));
        assert!(requests.iter().any(|request| {
            request.starts_with("GET /v3/organizations/org-fixture/sessions/session-1 ")
        }));
        assert!(requests.iter().any(|request| {
            request.starts_with(
                "GET /v3/organizations/org-fixture/sessions/session-1/messages?first=100 ",
            )
        }));
        assert!(requests.iter().any(|request| {
            request.starts_with("GET /v3/organizations/org-fixture/sessions/session-1/attachments ")
        }));
        assert!(
            requests.iter().any(|request| {
                request.starts_with(
                    "GET /v3/organizations/org-fixture/attachments/attachment-1/reference%20image.png ",
                )
            }),
            "{requests:#?}"
        );
        assert!(requests.iter().any(|request| {
            request.starts_with("POST /v3/organizations/org-fixture/sessions/session-1/tags ")
                && request.ends_with(r#"{"tags":["triage"]}"#)
        }));
        assert!(requests.iter().any(|request| {
            request.starts_with("PUT /v3/organizations/org-fixture/sessions/session-1/tags ")
                && request.ends_with(r#"{"tags":["ready"]}"#)
        }));
        drop(requests);

        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        assert_eq!(
            (
                state.sessions.len(),
                state.messages.len(),
                state.activity.len()
            ),
            (1, 1, 2)
        );
        assert_eq!(state.resources.insights.items.len(), 1);
        assert_eq!(
            state.activity[0].details.as_deref(),
            Some("Focused test details")
        );
        assert_eq!(
            state
                .attachment_previews
                .get("attachment-1")
                .map(|bytes| bytes.as_ref()),
            Some(b"fixture image".as_slice())
        );
    }

    #[cfg(feature = "network")]
    #[test]
    #[ignore = "requires DEVIN_API_KEY and live Devin access"]
    fn live_credentials_load_a_complete_session_view() {
        let (event_tx, event_rx) = mpsc::sync_channel(256);
        let mut worker = super::Worker::new(
            std::env::temp_dir(),
            super::ENDPOINT.into(),
            event_tx,
            Arc::new(|| {}),
        );
        let credentials = super::super::credentials::Credentials::load()
            .unwrap()
            .expect("DEVIN_API_KEY is required");
        worker.connect(credentials).unwrap();
        worker.refresh_sessions(false).unwrap();
        let session = event_rx
            .try_iter()
            .find_map(|event| match event {
                super::DevinEvent::SessionsLoaded { sessions, .. } => sessions.into_iter().next(),
                _ => None,
            })
            .expect("a live session is required");
        worker.selected = Some(super::SelectedSession {
            id: session.id,
            generation: 1,
            messages_cursor: None,
            events_cursor: None,
            category: super::super::state::StatusCategory::Unknown,
        });

        worker.load_selected().unwrap();

        let events = event_rx.try_iter().collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, super::DevinEvent::SessionLoaded { .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, super::DevinEvent::MessagesLoaded { .. }))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, super::DevinEvent::ActivityLoaded { .. }))
        );
        let event_id = events
            .iter()
            .find_map(|event| match event {
                super::DevinEvent::ActivityLoaded { activity, .. } => {
                    activity.first().map(|activity| activity.id.clone())
                }
                _ => None,
            })
            .expect("a live event is required");
        let has_more_activity = events.iter().any(|event| {
            matches!(
                event,
                super::DevinEvent::ActivityLoaded {
                    next_cursor: Some(_),
                    ..
                }
            )
        });
        if has_more_activity {
            worker.load_events().unwrap();
            assert!(event_rx.try_iter().any(|event| matches!(
                event,
                super::DevinEvent::ActivityLoaded {
                    activity,
                    update: super::PageUpdate::History,
                    ..
                } if !activity.is_empty()
            )));
        }
        worker.fetch_activity(&event_id).unwrap();
        assert!(event_rx.try_iter().any(|event| matches!(
            event,
            super::DevinEvent::ActivityDetailsLoaded { details, .. } if !details.is_empty()
        )));
    }

    #[cfg(feature = "network")]
    #[test]
    #[ignore = "creates and terminates a live Devin smoke-test session"]
    fn live_credentials_complete_disposable_session_flow() {
        let (event_tx, event_rx) = mpsc::sync_channel(256);
        let mut worker = super::Worker::new(
            std::env::temp_dir(),
            super::ENDPOINT.into(),
            event_tx,
            Arc::new(|| {}),
        );
        let credentials = super::super::credentials::Credentials::load()
            .unwrap()
            .expect("DEVIN_API_KEY is required");
        worker.connect(credentials).unwrap();
        worker
            .create_sessions(super::CreateSessionRequest {
                prompt: "Reply with ready, then wait.".into(),
                title: Some("Editur disposable integration smoke test".into()),
                tags: vec!["editur-smoke-test".into()],
                max_acu_limit: Some(1),
                batch_count: 1,
                ..super::CreateSessionRequest::default()
            })
            .unwrap();
        let session_id = event_rx
            .try_iter()
            .find_map(|event| match event {
                super::DevinEvent::SessionCreated(session) => Some(session.id),
                _ => None,
            })
            .expect("session creation did not return an id");
        worker.selected = Some(super::SelectedSession {
            id: session_id,
            generation: 1,
            messages_cursor: None,
            events_cursor: None,
            category: super::super::state::StatusCategory::Active,
        });
        let temp = tempfile::tempdir().unwrap();
        let attachment = temp.path().join("editur-smoke.txt");
        std::fs::write(&attachment, b"Editur attachment smoke test").unwrap();
        let attachment = crate::agent::controller::PromptAttachment::from_path(attachment).unwrap();

        let result = (|| {
            worker.send_message("Acknowledge the attached smoke-test file.", &[attachment])?;
            worker.lifecycle(super::InteractAction::Sleep)?;
            worker.lifecycle(super::InteractAction::Archive)?;
            worker.lifecycle(super::InteractAction::Unarchive)?;
            Ok::<_, super::TransportError>(())
        })();
        let cleanup = worker.lifecycle(super::InteractAction::Terminate);

        result.unwrap();
        cleanup.unwrap();
    }

    #[cfg(feature = "network")]
    #[test]
    #[ignore = "requires DEVIN_API_KEY and live Devin access"]
    fn live_credentials_load_all_read_only_resources() {
        let (event_tx, event_rx) = mpsc::sync_channel(256);
        let mut worker = super::Worker::new(
            std::env::temp_dir(),
            super::ENDPOINT.into(),
            event_tx,
            Arc::new(|| {}),
        );
        let credentials = super::super::credentials::Credentials::load()
            .unwrap()
            .expect("DEVIN_API_KEY is required");
        worker.connect(credentials).unwrap();
        for section in [
            super::DevinSection::Repositories,
            super::DevinSection::Knowledge,
            super::DevinSection::Playbooks,
            super::DevinSection::Automations,
            super::DevinSection::Environment,
            super::DevinSection::Integrations,
        ] {
            worker.load_section(section).unwrap();
        }
        worker.load_repository_wiki("octocat/Hello-World").unwrap();

        let events = event_rx.try_iter().collect::<Vec<_>>();
        assert!(
            events
                .iter()
                .any(|event| matches!(event, super::DevinEvent::RepositoriesLoaded(_)))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, super::DevinEvent::KnowledgeLoaded(_)))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, super::DevinEvent::PlaybooksLoaded(_)))
        );
        assert!(
            events
                .iter()
                .any(|event| matches!(event, super::DevinEvent::AutomationsLoaded(_)))
        );
        assert!(events.iter().any(|event| matches!(
            event,
            super::DevinEvent::IntegrationsLoaded(integrations) if !integrations.is_empty()
        )));
        assert!(events.iter().any(|event| matches!(
            event,
            super::DevinEvent::DocumentsLoaded(documents) if !documents.is_empty()
        )));
    }

    #[cfg(feature = "network")]
    #[test]
    fn background_refresh_never_sends_the_manual_history_cursor() {
        let (endpoint, requests, stop, server) = fake_mcp_server();
        let (event_tx, _event_rx) = mpsc::sync_channel(64);
        let mut worker =
            super::Worker::new(std::env::temp_dir(), endpoint, event_tx, Arc::new(|| {}));
        let credentials = super::super::credentials::Credentials::new(
            "cog_test-only".into(),
            None,
            super::super::credentials::CredentialSource::Environment,
        )
        .unwrap();
        worker.connect(credentials).unwrap();
        worker.selected = Some(super::SelectedSession {
            id: "session-1".into(),
            generation: 1,
            messages_cursor: None,
            events_cursor: None,
            category: super::super::state::StatusCategory::Active,
        });

        worker.load_selected().unwrap();
        worker.load_messages().unwrap();
        worker.refresh_selected().unwrap();

        let message_requests = requests
            .lock()
            .unwrap()
            .iter()
            .filter_map(|request| request.lines().next())
            .filter(|request| request.contains("/sessions/session-1/messages?"))
            .map(str::to_owned)
            .collect::<Vec<_>>();

        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        assert_eq!(
            message_requests,
            [
                "GET /v3/organizations/org-fixture/sessions/session-1/messages?first=100 HTTP/1.1",
                "GET /v3/organizations/org-fixture/sessions/session-1/messages?first=100&after=message-next HTTP/1.1",
                "GET /v3/organizations/org-fixture/sessions/session-1/messages?first=100 HTTP/1.1",
            ],
            "polling must not consume the manual history cursor"
        );
    }

    #[cfg(feature = "network")]
    #[test]
    fn missing_advanced_tools_do_not_break_core_session_connection() {
        let (endpoint, _requests, stop, server) = fake_mcp_server();
        let credentials = super::super::credentials::Credentials::new(
            "cog_test-only".into(),
            None,
            super::super::credentials::CredentialSource::Environment,
        )
        .unwrap();

        let transport = super::McpTransport::connect_endpoint(credentials, &endpoint).unwrap();

        assert!(transport.capabilities().has("devin_session_search"));
        assert!(!transport.capabilities().has("devin_session_gather"));
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    #[cfg(feature = "network")]
    #[test]
    fn sending_a_local_image_uses_v3_upload_and_attachment_url_fields() {
        let (endpoint, requests, stop, server) = fake_mcp_server();
        let (event_tx, _event_rx) = mpsc::sync_channel(64);
        let mut worker =
            super::Worker::new(std::env::temp_dir(), endpoint, event_tx, Arc::new(|| {}));
        let credentials = super::super::credentials::Credentials::new(
            "cog_test-only".into(),
            None,
            super::super::credentials::CredentialSource::Environment,
        )
        .unwrap();
        worker.connect(credentials).unwrap();
        worker.selected = Some(super::SelectedSession {
            id: "session-1".into(),
            generation: 1,
            messages_cursor: None,
            events_cursor: None,
            category: super::super::state::StatusCategory::Active,
        });
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("reference.png");
        std::fs::write(&image, b"image").unwrap();

        worker
            .send_message(
                "Use this image",
                &[crate::agent::controller::PromptAttachment::from_path(image).unwrap()],
            )
            .unwrap();

        let requests = requests.lock().unwrap();
        assert!(requests.iter().any(|request| {
            request.starts_with("POST /v3/organizations/org-fixture/attachments ")
        }));
        let sent = requests
            .iter()
            .find(|request| {
                request
                    .starts_with("POST /v3/organizations/org-fixture/sessions/session-1/messages ")
            })
            .and_then(|request| request.split("\r\n\r\n").nth(1))
            .and_then(|body| serde_json::from_str::<Value>(body).ok())
            .unwrap();
        assert_eq!(
            sent,
            json!({
                "message": "Use this image",
                "attachment_urls": ["https://storage.example/reference.png"]
            })
        );
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    #[cfg(feature = "network")]
    #[test]
    fn advanced_session_create_sends_the_complete_advertised_request() {
        let (endpoint, requests, stop, server) = fake_mcp_server();
        let (event_tx, _event_rx) = mpsc::sync_channel(64);
        let mut worker =
            super::Worker::new(std::env::temp_dir(), endpoint, event_tx, Arc::new(|| {}));
        let credentials = super::super::credentials::Credentials::new(
            "cog_test-only".into(),
            None,
            super::super::credentials::CredentialSource::Environment,
        )
        .unwrap();
        worker.connect(credentials).unwrap();

        worker
            .create_sessions(super::CreateSessionRequest {
                repositories: vec!["editur/editor".into(), "editur/site".into()],
                prompt: "Implement the feature".into(),
                title: Some("Capability parity".into()),
                mode: Some("fast".into()),
                playbook_id: Some("playbook-1".into()),
                child_playbook_id: Some("playbook-child".into()),
                knowledge_ids: vec!["knowledge-1".into()],
                tags: vec!["editur".into(), "parity".into()],
                max_acu_limit: Some(12),
                platform: Some("linux".into()),
                resumable: Some(true),
                session_links: vec!["session-link".into()],
                structured_output_schema: Some(json!({"type":"object"})),
                structured_output_required: true,
                secret_ids: vec!["secret-1".into()],
                session_secrets: vec![super::SecretInput {
                    key: "EPHEMERAL_TOKEN".into(),
                    value: "sanitized-value".into(),
                    kind: "key-value".into(),
                }],
                create_as_user_id: Some("user-1".into()),
                bypass_approval: true,
                parent_session_id: Some("session-parent".into()),
                batch_count: 1,
                ..super::CreateSessionRequest::default()
            })
            .unwrap();

        let body = requests
            .lock()
            .unwrap()
            .iter()
            .find(|request| request.starts_with("POST /v3/organizations/org-fixture/sessions "))
            .and_then(|request| request.split("\r\n\r\n").nth(1))
            .and_then(|body| serde_json::from_str::<Value>(body).ok())
            .unwrap();
        assert_eq!(
            body,
            json!({
                "prompt": "Implement the feature",
                "repos": ["editur/editor", "editur/site"],
                "title": "Capability parity",
                "devin_mode": "fast",
                "playbook_id": "playbook-1",
                "child_playbook_id": "playbook-child",
                "knowledge_ids": ["knowledge-1"],
                "tags": ["editur", "parity"],
                "max_acu_limit": 12,
                "platform": "linux",
                "resumable": true,
                "session_links": ["session-link"],
                "structured_output_schema": {"type":"object"},
                "structured_output_required": true,
                "secret_ids": ["secret-1"],
                "session_secrets": [{
                    "key": "EPHEMERAL_TOKEN",
                    "value": "sanitized-value",
                    "sensitive": true
                }],
                "create_as_user_id": "user-1",
                "bypass_approval": true,
                "parent_session_id": "session-parent"
            })
        );

        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    #[cfg(feature = "network")]
    #[test]
    fn advertised_mcp_resources_and_v3_workflows_load_without_cross_feature_coupling() {
        let (endpoint, requests, stop, server) = fake_full_mcp_server();
        let (event_tx, event_rx) = mpsc::sync_channel(64);
        let mut worker =
            super::Worker::new(std::env::temp_dir(), endpoint, event_tx, Arc::new(|| {}));
        let credentials = super::super::credentials::Credentials::new(
            "cog_test-only".into(),
            None,
            super::super::credentials::CredentialSource::Environment,
        )
        .unwrap();
        worker.connect(credentials).unwrap();

        for section in [
            super::DevinSection::Repositories,
            super::DevinSection::Knowledge,
            super::DevinSection::Playbooks,
            super::DevinSection::Automations,
            super::DevinSection::Environment,
            super::DevinSection::Integrations,
        ] {
            worker.load_section(section).unwrap();
        }
        worker.load_knowledge_detail("note-1").unwrap();
        worker
            .load_knowledge_suggestion_detail("suggestion-1")
            .unwrap();
        worker.load_playbook_detail("playbook-1").unwrap();
        worker.load_schedule_detail("schedule-1").unwrap();
        worker.load_automation_detail("automation-1").unwrap();
        worker.load_build_detail("build-1").unwrap();
        worker.load_repository_wiki("editur/editor").unwrap();

        let mut state = super::super::state::DevinState::default();
        event_rx.try_iter().for_each(|event| state.apply(event));
        assert_eq!(state.resources.repositories.items.len(), 1);
        assert_eq!(state.resources.knowledge.items.len(), 1);
        assert_eq!(state.resources.knowledge_folders.items.len(), 1);
        assert_eq!(state.resources.knowledge_suggestions.items.len(), 1);
        assert_eq!(state.resources.playbooks.items.len(), 1);
        assert_eq!(state.resources.schedules.items.len(), 1);
        assert_eq!(state.resources.automations.items.len(), 1);
        assert_eq!(state.resources.automation_catalog.items.len(), 1);
        assert_eq!(state.resources.blueprints.items.len(), 1);
        assert_eq!(state.resources.builds.items.len(), 1);
        assert_eq!(state.resources.secrets.items.len(), 1);
        assert_eq!(state.resources.integrations.items.len(), 1);
        assert_eq!(state.resources.documents.items.len(), 2);
        assert_eq!(state.resources.knowledge.items[0].content, "Full note");
        assert_eq!(
            state.resources.knowledge_suggestions.items[0].content,
            "Full suggestion"
        );
        assert_eq!(state.resources.playbooks.items[0].content, "Full playbook");
        assert_eq!(state.resources.schedules.items[0].prompt, "Full prompt");
        assert!(state.resources.automations.items[0].configuration["triggers"].is_array());
        assert_eq!(
            state.resources.builds.items[0].configuration["snapshot_id"],
            "snapshot-1"
        );
        let requests = requests.lock().unwrap();
        for endpoint in ["knowledge/notes", "playbooks", "schedules"] {
            assert!(requests.iter().any(|request| {
                request.starts_with(&format!("GET /v3/organizations/org-fixture/{endpoint} "))
            }));
        }
        assert!(requests.iter().any(|request| {
            request
                .split("\r\n\r\n")
                .nth(1)
                .and_then(|body| serde_json::from_str::<Value>(body).ok())
                .is_some_and(|body| {
                    body["params"]["name"] == "read_wiki_structure"
                        && body["params"]["arguments"]["repoName"] == "editur/editor"
                })
        }));
        drop(requests);

        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    #[cfg(feature = "network")]
    #[test]
    fn resource_mutations_send_the_documented_mcp_and_v3_requests() {
        let (endpoint, requests, stop, server) = fake_full_mcp_server();
        let (event_tx, _event_rx) = mpsc::sync_channel(64);
        let mut worker =
            super::Worker::new(std::env::temp_dir(), endpoint, event_tx, Arc::new(|| {}));
        let credentials = super::super::credentials::Credentials::new(
            "cog_test-only".into(),
            None,
            super::super::credentials::CredentialSource::Environment,
        )
        .unwrap();
        worker.connect(credentials).unwrap();

        for mutation in [
            super::ResourceMutation::Knowledge {
                action: super::CrudAction::Create,
                id: None,
                name: "Testing".into(),
                content: "Run focused tests".into(),
                folder: Some("Engineering".into()),
            },
            super::ResourceMutation::Playbook {
                action: super::CrudAction::Update,
                id: Some("playbook-1".into()),
                title: "Verify".into(),
                content: "Run the suite".into(),
                automation_macro: Some("verify".into()),
            },
            super::ResourceMutation::Schedule {
                action: super::CrudAction::Update,
                id: Some("schedule-1".into()),
                payload: json!({"name":"Nightly","enabled":false}),
            },
            super::ResourceMutation::Automation {
                action: super::CrudAction::Update,
                id: Some("automation-1".into()),
                payload: json!({"name":"Triage","enabled":false}),
            },
            super::ResourceMutation::IndexRepository {
                repository: "editur/editor".into(),
                branches: vec!["main".into()],
            },
            super::ResourceMutation::Blueprint {
                action: super::CrudAction::Update,
                id: Some("blueprint-1".into()),
                repository: None,
                contents: "packages: []\n".into(),
            },
            super::ResourceMutation::TriggerBuild,
            super::ResourceMutation::CreateSecret(super::SecretInput {
                key: "TEST_TOKEN".into(),
                value: "sanitized-value".into(),
                kind: "key-value".into(),
            }),
            super::ResourceMutation::TriggerReview {
                pull_request_url: "https://github.com/editur/editor/pull/1".into(),
            },
        ] {
            worker.mutate_resource(mutation).unwrap();
        }

        let requests = requests.lock().unwrap();
        let tool_arguments = |tool: &str| {
            requests
                .iter()
                .filter_map(|request| request.split("\r\n\r\n").nth(1))
                .filter_map(|body| serde_json::from_str::<Value>(body).ok())
                .filter(|body| body["params"]["name"] == tool)
                .map(|body| body["params"]["arguments"].clone())
                .collect::<Vec<_>>()
        };
        assert!(tool_arguments("devin_knowledge_manage").contains(&json!({
            "action":"create","name":"Testing","content":"Run focused tests","folder":"Engineering"
        })));
        assert!(tool_arguments("devin_playbook_manage").contains(&json!({
            "action":"update","playbook_id":"playbook-1","title":"Verify","content":"Run the suite","automation_macro":"verify"
        })));
        assert!(tool_arguments("devin_schedule_manage").contains(&json!({
            "action":"update","schedule_id":"schedule-1","name":"Nightly","enabled":false
        })));
        for (prefix, body) in [
            (
                "PATCH /v3/organizations/org-fixture/automations/automation-1 ",
                json!({"name":"Triage","enabled":false}),
            ),
            (
                "PUT /v3beta1/organizations/org-fixture/repositories/editur%2Feditor/indexing ",
                json!({"branch_names":["main"]}),
            ),
            (
                "PATCH /v3beta1/organizations/org-fixture/snapshot-setup/blueprints/blueprint-1 ",
                json!({"contents":"packages: []\n"}),
            ),
            (
                "POST /v3beta1/organizations/org-fixture/snapshot-setup/builds ",
                json!({}),
            ),
            (
                "POST /v3/organizations/org-fixture/secrets ",
                json!({"type":"key-value","key":"TEST_TOKEN","value":"sanitized-value","is_sensitive":true}),
            ),
            (
                "POST /v3/organizations/org-fixture/pr-reviews ",
                json!({"pr_url":"https://github.com/editur/editor/pull/1"}),
            ),
        ] {
            assert!(
                requests.iter().any(|request| {
                    request.starts_with(prefix)
                        && request
                            .split("\r\n\r\n")
                            .nth(1)
                            .and_then(|body| serde_json::from_str::<Value>(body).ok())
                            .as_ref()
                            == Some(&body)
                }),
                "missing {prefix}"
            );
        }
        drop(requests);

        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    fn fake_mcp_server() -> FakeMcpServer {
        fake_mcp_server_with_resources(false)
    }

    fn fake_full_mcp_server() -> FakeMcpServer {
        fake_mcp_server_with_resources(true)
    }

    fn fake_mcp_server_with_resources(full: bool) -> FakeMcpServer {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let endpoint = format!("http://{}/mcp", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let server_requests = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let server_stop = Arc::clone(&stop);
        let server = thread::spawn(move || {
            while !server_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        // Accepted sockets inherit the listener's non-blocking
                        // mode on macOS; the request reader expects blocking
                        // reads, so WouldBlock would panic it under load.
                        stream.set_nonblocking(false).unwrap();
                        serve_mcp_request(&mut stream, &server_requests, full);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(error) => panic!("fake MCP accept failed: {error}"),
                }
            }
        });
        (endpoint, requests, stop, server)
    }

    fn serve_mcp_request(stream: &mut TcpStream, requests: &Mutex<Vec<String>>, full: bool) {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let request = read_http_request(stream);
        requests.lock().unwrap().push(request.clone());
        assert!(
            request
                .to_ascii_lowercase()
                .contains("authorization: bearer cog_test-only")
        );
        if request.starts_with("GET /v3/self ") {
            write_http(
                stream,
                200,
                Some(
                    r#"{"principal_type":"service_user","service_user_id":"service-fixture","service_user_name":"Fixture","org_id":"org-fixture"}"#,
                ),
            );
            return;
        }
        if request.starts_with("POST /v3/organizations/org-fixture/attachments ") {
            write_http(
                stream,
                200,
                Some(
                    r#"{"attachment_id":"attachment-fixture","name":"reference.png","url":"https://storage.example/reference.png"}"#,
                ),
            );
            return;
        }
        if request.starts_with("POST /v3/organizations/org-fixture/sessions ") {
            write_http(stream, 200, Some(&fake_session().to_string()));
            return;
        }
        if request
            .starts_with("GET /v3/organizations/org-fixture/sessions/session-1/messages?first=100")
        {
            write_http(
                stream,
                200,
                Some(
                    r#"{"items":[{"event_id":"message-1","created_at":1786809600,"source":"devin","message":"Working"}],"end_cursor":"message-next","has_next_page":true,"total":1}"#,
                ),
            );
            return;
        }
        if request.starts_with("GET /v3/organizations/org-fixture/sessions/session-1/attachments ")
        {
            write_http(
                stream,
                200,
                Some(
                    r#"[{"attachment_id":"attachment-1","name":"reference image.png","content_type":"image/png"}]"#,
                ),
            );
            return;
        }
        if request.starts_with(
            "GET /v3/organizations/org-fixture/attachments/attachment-1/reference%20image.png ",
        ) {
            write_http(stream, 200, Some("fixture image"));
            return;
        }
        if request.starts_with("GET /v3/organizations/org-fixture/sessions/session-1 ") {
            write_http(stream, 200, Some(&fake_session().to_string()));
            return;
        }
        if request.starts_with("POST /v3/organizations/org-fixture/sessions/session-1/messages ") {
            write_http(
                stream,
                200,
                Some(r#"{"session_id":"session-1","status":"running"}"#),
            );
            return;
        }
        if request.starts_with("POST /v3/organizations/org-fixture/sessions/session-1/tags ")
            || request.starts_with("PUT /v3/organizations/org-fixture/sessions/session-1/tags ")
        {
            write_http(stream, 200, Some(r#"{"tags":["sanitized"]}"#));
            return;
        }
        if request
            .starts_with("POST /v3/organizations/org-fixture/sessions/session-1/insights/generate ")
        {
            write_http(stream, 202, None);
            return;
        }
        if request.starts_with("GET /v3/organizations/org-fixture/sessions/session-1/insights ") {
            write_http(
                stream,
                200,
                Some(
                    r#"{"session_id":"session-1","status":"completed","analysis":"Sanitized insight"}"#,
                ),
            );
            return;
        }
        if full {
            let response = if request
                .starts_with("GET /v3beta1/organizations/org-fixture/repositories/indexing ")
            {
                Some(
                    json!({"repositories":[{"repository_path":"editur/editor","indexing_enabled":true,"indexing_status":"ready"}]}),
                )
            } else if request.starts_with("GET /v3beta1/organizations/org-fixture/repositories ") {
                Some(json!({"repositories":[{"repository_path":"editur/editor"}]}))
            } else if request
                .starts_with("GET /v3/organizations/org-fixture/knowledge/notes/note-1 ")
            {
                Some(json!({"note_id":"note-1","name":"Testing","content":"Full note"}))
            } else if request.starts_with("GET /v3/organizations/org-fixture/knowledge/notes ") {
                Some(json!({"items":[{"note_id":"note-1","name":"Testing","content":"Run tests"}]}))
            } else if request.starts_with("GET /v3/organizations/org-fixture/knowledge/folders ") {
                Some(
                    json!({"folders":[{"folder_id":"folder-1","name":"Engineering","note_count":1}]}),
                )
            } else if request.starts_with("GET /v3/organizations/org-fixture/playbooks/playbook-1 ")
            {
                Some(json!({"playbook_id":"playbook-1","title":"Triage","body":"Full playbook"}))
            } else if request.starts_with("GET /v3/organizations/org-fixture/playbooks ") {
                Some(
                    json!({"items":[{"playbook_id":"playbook-1","title":"Triage","body":"Run checks"}]}),
                )
            } else if request.starts_with("GET /v3/organizations/org-fixture/schedules/schedule-1 ")
            {
                Some(
                    json!({"schedule_id":"schedule-1","name":"Nightly","prompt":"Full prompt","frequency":"daily","enabled":true}),
                )
            } else if request.starts_with("GET /v3/organizations/org-fixture/schedules ") {
                Some(
                    json!({"items":[{"schedule_id":"schedule-1","name":"Nightly","prompt":"Test","frequency":"daily","enabled":true}]}),
                )
            } else if request.starts_with("GET /v3/organizations/org-fixture/automations/schemas ")
            {
                Some(json!({"github":{"issue_opened":{"type":"object"}}}))
            } else if request
                .starts_with("GET /v3/organizations/org-fixture/automations/templates ")
            {
                Some(json!({"items":[{"id":"template-1","name":"Issue triage"}]}))
            } else if request
                .starts_with("GET /v3/organizations/org-fixture/automations/automation-1 ")
            {
                Some(json!({
                    "automation_id":"automation-1",
                    "name":"Triage",
                    "enabled":true,
                    "triggers":[{"type":"github.issue_opened"}],
                    "actions":[{"type":"start_session"}]
                }))
            } else if request.starts_with("GET /v3/organizations/org-fixture/automations ") {
                Some(
                    json!({"automations":[{"automation_id":"automation-1","name":"Triage","enabled":true}]}),
                )
            } else if request
                .starts_with("GET /v3beta1/organizations/org-fixture/snapshot-setup/blueprints ")
            {
                Some(json!({"blueprints":[{"blueprint_id":"blueprint-1","type":"organization"}]}))
            } else if request.starts_with(
                "GET /v3beta1/organizations/org-fixture/snapshot-setup/builds/build-1 ",
            ) {
                Some(json!({
                    "build_id":"build-1",
                    "status":"succeeded",
                    "pinned":false,
                    "snapshot_id":"snapshot-1"
                }))
            } else if request
                .starts_with("GET /v3beta1/organizations/org-fixture/snapshot-setup/builds ")
            {
                Some(json!({"builds":[{"build_id":"build-1","status":"succeeded","pinned":false}]}))
            } else if request.starts_with("GET /v3/organizations/org-fixture/secrets ") {
                Some(json!({"secrets":[{"secret_id":"secret-1","key":"TOKEN","type":"key-value"}]}))
            } else {
                None
            };
            if let Some(response) = response {
                write_http(stream, 200, Some(&response.to_string()));
                return;
            }
        }
        let body = request.split("\r\n\r\n").nth(1).unwrap_or_default();
        let request: Value = serde_json::from_str(body).unwrap();
        if request.get("id").is_none() {
            write_http(stream, 202, None);
            return;
        }
        let id = request["id"].clone();
        let result = match request["method"].as_str().unwrap() {
            "initialize" => {
                json!({"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"fake","version":"1"}})
            }
            "tools/list" => json!({"tools": fake_tools(full)}),
            "tools/call" => fake_tool_result(&request["params"]),
            method => panic!("unexpected method {method}"),
        };
        write_http(
            stream,
            200,
            Some(&json!({"jsonrpc":"2.0","id":id,"result":result}).to_string()),
        );
    }

    fn fake_tools(full: bool) -> Vec<Value> {
        let mut tools = vec![
            json!({"name":"devin_session_search","inputSchema":{"type":"object","properties":{"cursor":{"type":"string"},"limit":{"type":"integer"}}}}),
            json!({"name":"devin_session_create","inputSchema":{"type":"object","properties":{"sessions":{"type":"array","items":{"type":"object"}}}}}),
            json!({"name":"devin_session_interact","inputSchema":{"type":"object","properties":{"session_id":{"type":"string"},"action":{"type":"string","enum":["get_status","get_messages","get_attachments","send_message","sleep","archive","unarchive","terminate"]},"message":{"type":"string"},"attachment_ids":{"type":"array"},"cursor":{"type":"string"}}}}),
            json!({"name":"devin_session_events","inputSchema":{"type":"object","properties":{"session_id":{"type":"string"},"action":{"type":"string","enum":["list","details","search"]},"query":{"type":"string"},"event_ids":{"type":"array","items":{"type":"string"}},"after":{"type":"string"},"limit":{"type":"integer"}}}}),
        ];
        if full {
            tools.extend([
                json!({"name":"read_wiki_structure","inputSchema":{"type":"object","properties":{"repoName":{"type":"string"}}}}),
                json!({"name":"read_wiki_contents","inputSchema":{"type":"object","properties":{"repoName":{"type":"string"}}}}),
                json!({"name":"ask_question","inputSchema":{"type":"object","properties":{"repoName":{"anyOf":[{"type":"string"},{"type":"array"}]},"question":{"type":"string"}}}}),
                json!({"name":"list_available_repos","inputSchema":{"type":"object","properties":{}}}),
                json!({"name":"devin_session_gather","inputSchema":{"type":"object","properties":{"session_ids":{"type":"array"}}}}),
                json!({"name":"devin_playbook_manage","inputSchema":{"type":"object","properties":{"action":{"type":"string","enum":["list","get","create","update","delete"]}}}}),
                json!({"name":"devin_knowledge_manage","inputSchema":{"type":"object","properties":{"action":{"type":"string","enum":["list","get","create","update","delete","list_suggestions","get_suggestion","dismiss_suggestion"]}}}}),
                json!({"name":"devin_schedule_manage","inputSchema":{"type":"object","properties":{"action":{"type":"string","enum":["list","get","create","update","delete"]}}}}),
                json!({"name":"list_integrations","inputSchema":{"type":"object","properties":{}}}),
            ]);
        }
        tools
    }

    fn fake_tool_result(params: &Value) -> Value {
        let name = params["name"].as_str().unwrap();
        let arguments = &params["arguments"];
        if name == "devin_session_events" && arguments["action"] == "details" {
            return json!({
                "structuredContent": {"result": "Focused test details"},
                "content": []
            });
        }
        let structured = match name {
            "devin_session_search" => json!({"sessions":[fake_session()],"next_cursor":null}),
            "devin_session_create" => json!({"sessions":[fake_session()]}),
            "list_available_repos" => json!({"repositories":["editur/editor"]}),
            "read_wiki_structure" => json!({"topics":[{"title":"Overview"}]}),
            "read_wiki_contents" => json!({"documents":[{"title":"Overview","content":"Docs"}]}),
            "ask_question" => json!({"answer":"Answer","citations":["README.md"]}),
            "devin_playbook_manage" if arguments["action"] == "get" => {
                json!({"playbook":{"playbook_id":"playbook-1","title":"Verify","content":"Full playbook"}})
            }
            "devin_playbook_manage" => {
                json!({"playbooks":[{"playbook_id":"playbook-1","title":"Verify","content":"Test"}]})
            }
            "devin_knowledge_manage" => match arguments["action"].as_str() {
                Some("list_suggestions") => {
                    json!({"suggestions":[{"suggestion_id":"suggestion-1","title":"Keep tests focused"}]})
                }
                Some("get_suggestion") => {
                    json!({"suggestion":{"suggestion_id":"suggestion-1","title":"Keep tests focused","content":"Full suggestion"}})
                }
                Some("get") => {
                    json!({"note":{"note_id":"note-1","name":"Testing","content":"Full note"}})
                }
                _ => json!({"notes":[{"note_id":"note-1","name":"Testing","content":"Run tests"}]}),
            },
            "devin_schedule_manage" if arguments["action"] == "get" => {
                json!({"schedule":{"schedule_id":"schedule-1","name":"Nightly","prompt":"Full prompt","frequency":"daily","enabled":true}})
            }
            "devin_schedule_manage" => {
                json!({"schedules":[{"schedule_id":"schedule-1","name":"Nightly","prompt":"Test","frequency":"daily","enabled":true}]})
            }
            "list_integrations" => {
                json!({"integrations":[{"id":"github","name":"GitHub","installed":true,"settings_url":"https://app.devin.ai/settings"}]})
            }
            "devin_session_gather" => json!({"sessions":[fake_session()]}),
            "devin_session_events" => json!({"events":[{
                "event_id": if arguments["after"] == "event-next" { "event-2" } else { "event-1" },
                "timestamp":"2026-08-15T12:01:00Z",
                "type":"shell",
                "summary": if arguments["action"] == "search" { "Matched tests" } else { "Ran tests" },
                "command":"cargo test"
            }],"next_cursor": (arguments["after"] != "event-next").then_some("event-next")}),
            "devin_session_interact" => match arguments["action"].as_str().unwrap() {
                "get_status" => fake_session(),
                "get_messages" => {
                    json!({"messages":[{"message_id":"message-1","timestamp":"2026-08-15T12:00:00Z","role":"devin","text":"Working"}],"next_cursor":"message-next"})
                }
                "get_attachments" => {
                    json!({"attachments":[{"attachment_id":"attachment-1","name":"report.txt"}]})
                }
                _ => json!({"ok":true}),
            },
            _ => unreachable!(),
        };
        json!({"structuredContent": structured, "content": []})
    }

    fn fake_session() -> Value {
        json!({
            "session_id":"session-1",
            "title":"Fake session",
            "status":"running",
            "status_detail":"Testing",
            "origin":"slack",
            "repository":"editur/editor"
        })
    }

    fn read_http_request(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 4096];
        loop {
            let count = stream.read(&mut buffer).unwrap();
            bytes.extend_from_slice(&buffer[..count]);
            let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    line.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(str::trim)
                        .and_then(|value| value.parse::<usize>().ok())
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + content_length {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }

    fn write_http(stream: &mut TcpStream, status: u16, body: Option<&str>) {
        let body = body.unwrap_or_default();
        let reason = if status == 202 { "Accepted" } else { "OK" };
        write!(
            stream,
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
        .unwrap();
    }
}
