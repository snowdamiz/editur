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
    state::{Attachment, ConnectionState, DevinError, DevinEvent, RepositoryState, StatusCategory},
    transport::{ENDPOINT, InteractAction, McpTransport, TransportError},
};
use crate::agent::controller::{MAX_PROMPT_ATTACHMENTS, PromptAttachment};

const COMMAND_CAPACITY: usize = 64;
const EVENT_CAPACITY: usize = 64;
const ACTIVE_POLL: Duration = Duration::from_secs(4);
const LIST_POLL: Duration = Duration::from_secs(20);
const TERMINAL_POLL: Duration = Duration::from_secs(30);
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const MAX_PREVIEW_FETCHES: usize = 8;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DevinCommand {
    SetVisible(bool),
    RefreshSessions,
    LoadMoreSessions,
    SelectSession {
        session_id: String,
        generation: u64,
    },
    LoadMoreMessages,
    LoadMoreEvents,
    CreateSession {
        repository: String,
        prompt: String,
    },
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
    transport: Option<McpTransport>,
    selected: Option<SelectedSession>,
    sessions_cursor: Option<String>,
    schedule: PollSchedule,
    repository_resolved: bool,
    /// Attachment ids already fetched (or attempted) for the selected
    /// session, so refresh polls do not re-download previews.
    fetched_previews: HashSet<String>,
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
            transport: None,
            selected: None,
            sessions_cursor: None,
            schedule: PollSchedule::new(Instant::now()),
            repository_resolved: false,
            fetched_previews: HashSet::new(),
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
                self.run_request(Self::refresh_selected);
            }
            DevinCommand::LoadMoreMessages => self.run_request(Self::load_messages),
            DevinCommand::LoadMoreEvents => self.run_request(Self::load_events),
            DevinCommand::CreateSession { repository, prompt } => {
                if prompt.trim().is_empty() || prompt.len() > MAX_MESSAGE_BYTES {
                    self.local_error(
                        "Enter a prompt of at most 64 KiB before creating a Devin session",
                    );
                } else if normalize_github_repository(&repository).as_deref()
                    != Some(repository.trim())
                {
                    self.local_error("Choose a GitHub repository in owner/repository form");
                } else {
                    self.run_request(|worker| {
                        worker.create_session(repository.trim(), prompt.trim())
                    });
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

    fn ensure_connected(&mut self) -> Result<&mut McpTransport, TransportError> {
        if self.transport.is_none() {
            let credentials = Credentials::load()
                .map_err(TransportError::Protocol)?
                .ok_or(TransportError::CredentialsMissing)?;
            self.emit(DevinEvent::CredentialsChanged(Some(credentials.source())));
            self.connect(credentials)?;
        }
        self.transport
            .as_mut()
            .ok_or_else(|| TransportError::Protocol("transport was not created".into()))
    }

    fn connect(&mut self, credentials: Credentials) -> Result<(), TransportError> {
        let source = credentials.source();
        self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connecting));
        let transport = self.open_transport(credentials)?;
        self.transport = Some(transport);
        self.emit(DevinEvent::CredentialsChanged(Some(source)));
        self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connected));
        Ok(())
    }

    fn connect_and_store(&mut self, credentials: Credentials) -> Result<(), TransportError> {
        let source = credentials.source();
        self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connecting));
        let mut transport = self.open_transport(credentials.clone())?;
        let result = transport.search(None)?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let (sessions, next_cursor) =
            normalize::sessions(&payload).map_err(TransportError::Protocol)?;
        credentials.save().map_err(TransportError::Protocol)?;
        self.sessions_cursor = next_cursor.clone();
        self.transport = Some(transport);
        self.emit(DevinEvent::CredentialsChanged(Some(source)));
        self.emit(DevinEvent::ConnectionChanged(ConnectionState::Connected));
        self.emit(DevinEvent::SessionsLoaded {
            sessions,
            next_cursor,
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
        let result = self.ensure_connected()?.search(cursor.as_deref())?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let (sessions, next_cursor) =
            normalize::sessions(&payload).map_err(TransportError::Protocol)?;
        self.sessions_cursor = next_cursor.clone();
        self.emit(DevinEvent::SessionsLoaded {
            sessions,
            next_cursor,
            append,
        });
        Ok(())
    }

    fn refresh_selected(&mut self) -> Result<(), TransportError> {
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        let result = self
            .ensure_connected()?
            .interact(&selected.id, InteractAction::Status)?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let mut detail = normalize::detail(&payload).map_err(TransportError::Protocol)?;
        if let Ok(result) = self
            .ensure_connected()?
            .interact(&selected.id, InteractAction::Attachments)
            && let Ok(payload) = normalize::tool_payload(result)
        {
            let attachments = normalize::attachments(&payload);
            if !attachments.is_empty() {
                detail.attachments = attachments;
            }
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
            .filter_map(|attachment| {
                image_attachment_url(attachment).map(|url| (attachment.id.clone(), url.to_owned()))
            })
            .collect::<Vec<_>>();
        self.emit(DevinEvent::SessionLoaded {
            session_id: selected.id.clone(),
            generation: selected.generation,
            detail,
        });
        self.load_messages()?;
        self.load_events()?;
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
        for (attachment_id, url) in previews.into_iter().take(MAX_PREVIEW_FETCHES) {
            if !self.fetched_previews.insert(attachment_id.clone()) {
                continue;
            }
            let Ok(bytes) = self
                .ensure_connected()
                .and_then(|transport| transport.fetch_attachment(&url))
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
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        let result = self.ensure_connected()?.interact(
            &selected.id,
            InteractAction::Messages {
                cursor: selected.messages_cursor.as_deref(),
            },
        )?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let (messages, next_cursor) =
            normalize::messages(&payload).map_err(TransportError::Protocol)?;
        if let Some(current) = self.selected.as_mut()
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
            replace: selected.messages_cursor.is_none(),
        });
        Ok(())
    }

    fn load_events(&mut self) -> Result<(), TransportError> {
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        let result = self
            .ensure_connected()?
            .events(&selected.id, selected.events_cursor.as_deref())?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let (activity, next_cursor) =
            normalize::activity(&payload).map_err(TransportError::Protocol)?;
        if let Some(current) = self.selected.as_mut()
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
            replace: selected.events_cursor.is_none(),
        });
        Ok(())
    }

    fn create_session(&mut self, repository: &str, prompt: &str) -> Result<(), TransportError> {
        let result = self.ensure_connected()?.create(repository, prompt)?;
        let payload = normalize::tool_payload(result).map_err(TransportError::Protocol)?;
        let session = normalize::created_session(&payload).map_err(TransportError::Protocol)?;
        self.emit(DevinEvent::SessionCreated(session));
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
        let message = message_with_attachment_urls(message, &urls);
        let selected = self
            .selected
            .clone()
            .ok_or_else(|| TransportError::Protocol("no Devin session is selected".into()))?;
        let result = self.ensure_connected()?.interact(
            &selected.id,
            InteractAction::SendMessage {
                message: &message,
                attachment_ids: &[],
            },
        )?;
        normalize::tool_payload(result).map_err(TransportError::Protocol)?;
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
            TransportError::Authentication | TransportError::Forbidden | TransportError::Offline
        ) {
            self.transport = None;
        }
        let connection = match error {
            TransportError::Authentication | TransportError::Forbidden => {
                ConnectionState::AuthenticationRequired
            }
            TransportError::RateLimited(_) => ConnectionState::RateLimited,
            TransportError::Offline => ConnectionState::Offline,
            _ => ConnectionState::Failed,
        };
        self.emit(DevinEvent::ConnectionChanged(connection));
        self.emit(DevinEvent::Failed(DevinError {
            message: error.user_message().into(),
            retry_after_seconds: error.retry_after_seconds(),
        }));
        if matches!(
            error,
            TransportError::Authentication | TransportError::Forbidden
        ) {
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

/// The download URL for an attachment worth previewing inline: an image by
/// declared media type or by file extension, with a link to fetch it from.
fn image_attachment_url(attachment: &Attachment) -> Option<&str> {
    let url = attachment.url.as_deref()?;
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
    (by_media_type || by_extension).then_some(url)
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

fn message_with_attachment_urls(message: &str, urls: &[String]) -> String {
    let mut message = message.trim().to_owned();
    for url in urls {
        if !message.is_empty() {
            message.push_str("\n\n");
        }
        message.push_str("ATTACHMENT:\"");
        message.push_str(url);
        message.push('"');
    }
    message
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
    fn uploaded_attachments_are_added_to_the_message_as_devin_references() {
        assert_eq!(
            super::message_with_attachment_urls(
                "Use this image",
                &["https://storage.example/reference.png".into()],
            ),
            "Use this image\n\nATTACHMENT:\"https://storage.example/reference.png\""
        );
    }

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
    fn only_linked_images_qualify_for_preview_downloads() {
        let image = super::Attachment {
            id: "a".into(),
            name: "mock.png".into(),
            media_type: None,
            size: None,
            url: Some("https://storage.example/mock.png?sig=abc".into()),
        };
        assert_eq!(
            super::image_attachment_url(&image),
            Some("https://storage.example/mock.png?sig=abc")
        );

        let by_media_type = super::Attachment {
            name: "mock".into(),
            media_type: Some("image/webp".into()),
            ..image.clone()
        };
        assert!(super::image_attachment_url(&by_media_type).is_some());

        let log = super::Attachment {
            name: "results.txt".into(),
            media_type: Some("text/plain".into()),
            ..image.clone()
        };
        assert_eq!(super::image_attachment_url(&log), None);

        let unlinked = super::Attachment { url: None, ..image };
        assert_eq!(super::image_attachment_url(&unlinked), None);
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

    #[test]
    fn fake_mcp_drives_the_complete_interactive_session_flow() {
        let (endpoint, _requests, stop, server) = fake_mcp_server();
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
        worker.create_session("editur/editor", "Fix it").unwrap();

        let mut state = super::super::state::DevinState::default();
        let generation = state.select("session-1".into());
        worker.selected = Some(super::SelectedSession {
            id: "session-1".into(),
            generation,
            messages_cursor: None,
            events_cursor: None,
            category: super::super::state::StatusCategory::Unknown,
        });
        worker.refresh_selected().unwrap();
        worker.send_message("Continue", &[]).unwrap();
        worker.lifecycle(super::InteractAction::Sleep).unwrap();
        worker.lifecycle(super::InteractAction::Archive).unwrap();
        worker.lifecycle(super::InteractAction::Unarchive).unwrap();
        worker.lifecycle(super::InteractAction::Terminate).unwrap();
        event_rx.try_iter().for_each(|event| state.apply(event));

        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
        assert_eq!(
            (
                state.sessions.len(),
                state.messages.len(),
                state.activity.len()
            ),
            (1, 1, 1)
        );
    }

    #[cfg(feature = "network")]
    #[test]
    fn sending_a_local_image_uploads_and_references_it_in_the_devin_message() {
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

        let sent = requests
            .lock()
            .unwrap()
            .iter()
            .filter_map(|request| request.split("\r\n\r\n").nth(1))
            .filter_map(|body| serde_json::from_str::<Value>(body).ok())
            .find(|request| request["params"]["arguments"]["action"] == "send_message")
            .unwrap();
        assert_eq!(
            sent["params"]["arguments"]["message"],
            "Use this image\n\nATTACHMENT:\"https://storage.example/reference.png\""
        );
        stop.store(true, Ordering::Relaxed);
        server.join().unwrap();
    }

    fn fake_mcp_server() -> FakeMcpServer {
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
                        serve_mcp_request(&mut stream, &server_requests);
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

    fn serve_mcp_request(stream: &mut TcpStream, requests: &Mutex<Vec<String>>) {
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
        if request.starts_with("POST /v1/attachments ") {
            write_http(
                stream,
                200,
                Some("\"https://storage.example/reference.png\""),
            );
            return;
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
            "tools/list" => json!({"tools": fake_tools()}),
            "tools/call" => fake_tool_result(&request["params"]),
            method => panic!("unexpected method {method}"),
        };
        write_http(
            stream,
            200,
            Some(&json!({"jsonrpc":"2.0","id":id,"result":result}).to_string()),
        );
    }

    fn fake_tools() -> Vec<Value> {
        vec![
            json!({"name":"devin_session_search","inputSchema":{"type":"object","properties":{"cursor":{"type":"string"},"limit":{"type":"integer"}}}}),
            json!({"name":"devin_session_create","inputSchema":{"type":"object","properties":{"sessions":{"type":"array","items":{"type":"object"}}}}}),
            json!({"name":"devin_session_interact","inputSchema":{"type":"object","properties":{"session_id":{"type":"string"},"action":{"type":"string","enum":["get_status","get_messages","get_attachments","send_message","sleep","archive","unarchive","terminate"]},"message":{"type":"string"},"attachment_ids":{"type":"array"},"cursor":{"type":"string"}}}}),
            json!({"name":"devin_session_events","inputSchema":{"type":"object","properties":{"session_id":{"type":"string"},"action":{"type":"string","enum":["list"]},"cursor":{"type":"string"},"limit":{"type":"integer"}}}}),
            json!({"name":"devin_session_gather","inputSchema":{"type":"object","properties":{"session_ids":{"type":"array"}}}}),
        ]
    }

    fn fake_tool_result(params: &Value) -> Value {
        let name = params["name"].as_str().unwrap();
        let arguments = &params["arguments"];
        let structured = match name {
            "devin_session_search" => json!({"sessions":[fake_session()],"next_cursor":null}),
            "devin_session_create" => json!({"sessions":[fake_session()]}),
            "devin_session_events" => {
                json!({"events":[{"event_id":"event-1","timestamp":"2026-08-15T12:01:00Z","type":"shell","summary":"Ran tests","command":"cargo test"}],"next_cursor":"event-next"})
            }
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
