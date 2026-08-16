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
    AttachmentFetched {
        session_id: String,
        generation: u64,
        attachment_id: String,
        bytes: std::sync::Arc<[u8]>,
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
            replace: true,
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
            replace: true,
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
