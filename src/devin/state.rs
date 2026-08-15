use std::{collections::HashSet, time::Instant};

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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StatusCategory {
    Active,
    Waiting,
    Sleeping,
    Completed,
    Failed,
    #[default]
    Unknown,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
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
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DevinMessage {
    pub id: String,
    pub timestamp: String,
    pub role: String,
    pub text: String,
    pub attachment_ids: Vec<String>,
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

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Attachment {
    pub id: String,
    pub name: String,
    pub media_type: Option<String>,
    pub size: Option<u64>,
    pub url: Option<String>,
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
    CredentialsChanged(Option<CredentialSource>),
    RepositoryResolved(RepositoryState),
    WorkspaceBoundary(String),
    SessionsLoaded {
        sessions: Vec<SessionSummary>,
        next_cursor: Option<String>,
        append: bool,
    },
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
        replace: bool,
    },
    ActivityLoaded {
        session_id: String,
        generation: u64,
        activity: Vec<Activity>,
        next_cursor: Option<String>,
        replace: bool,
    },
    OperationFinished {
        session_id: Option<String>,
    },
    Failed(DevinError),
}

#[derive(Default)]
pub struct DevinState {
    pub connection: ConnectionState,
    pub credential_source: Option<CredentialSource>,
    pub repository: RepositoryState,
    pub workspace_boundary: Option<String>,
    pub sessions: Vec<SessionSummary>,
    pub sessions_cursor: Option<String>,
    pub last_sessions_refresh: Option<Instant>,
    pub selected_session: Option<String>,
    pub selected_generation: u64,
    pub detail: Option<SessionDetail>,
    pub messages: Vec<DevinMessage>,
    pub messages_cursor: Option<String>,
    pub activity: Vec<Activity>,
    pub activity_cursor: Option<String>,
    pub error: Option<DevinError>,
    pub busy: bool,
}

impl DevinState {
    pub fn select(&mut self, session_id: String) -> u64 {
        self.selected_generation = self.selected_generation.wrapping_add(1);
        self.selected_session = Some(session_id);
        self.detail = None;
        self.messages.clear();
        self.messages_cursor = None;
        self.activity.clear();
        self.activity_cursor = None;
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
        self.busy = false;
    }

    pub fn apply(&mut self, event: DevinEvent) {
        match event {
            DevinEvent::ConnectionChanged(connection) => self.connection = connection,
            DevinEvent::CredentialsChanged(source) => {
                self.credential_source = source;
                if source.is_none() {
                    self.sessions.clear();
                    self.sessions_cursor = None;
                    self.last_sessions_refresh = None;
                    self.clear_selection();
                    self.error = None;
                }
            }
            DevinEvent::RepositoryResolved(repository) => self.repository = repository,
            DevinEvent::WorkspaceBoundary(message) => self.workspace_boundary = Some(message),
            DevinEvent::SessionsLoaded {
                sessions,
                next_cursor,
                append,
            } => {
                if append {
                    append_unique(&mut self.sessions, sessions, |session| session.id.as_str());
                } else {
                    self.sessions = sessions;
                }
                self.sessions_cursor = next_cursor;
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
            DevinEvent::SessionLoaded {
                session_id,
                generation,
                detail,
            } if self.is_current(&session_id, generation) => {
                self.detail = Some(detail);
                self.error = None;
                self.busy = false;
            }
            DevinEvent::MessagesLoaded {
                session_id,
                generation,
                messages,
                next_cursor,
                replace,
            } if self.is_current(&session_id, generation) => {
                if replace {
                    self.messages.clear();
                }
                append_unique(&mut self.messages, messages, |message| message.id.as_str());
                self.messages
                    .sort_by(|left, right| chronological(&left.timestamp, &right.timestamp));
                self.messages_cursor = next_cursor;
                self.error = None;
                self.busy = false;
            }
            DevinEvent::ActivityLoaded {
                session_id,
                generation,
                activity,
                next_cursor,
                replace,
            } if self.is_current(&session_id, generation) => {
                if replace {
                    self.activity.clear();
                }
                append_unique(&mut self.activity, activity, |event| event.id.as_str());
                self.activity
                    .sort_by(|left, right| chronological(&left.timestamp, &right.timestamp));
                self.activity_cursor = next_cursor;
                self.error = None;
                self.busy = false;
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
