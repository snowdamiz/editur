use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    future::Future,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender},
    },
    thread,
    time::{Duration, SystemTime},
};

use agent_client_protocol::schema::{ProtocolVersion, v1::*};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo, LineDirection};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::external_sessions::{self, ExternalMessage, ExternalSession, ExternalTool};
use super::provider::{
    AccountKey, AuthSource, ProviderAccount, ProviderExtensions, ProviderId,
    authentication_required_choices, descriptor, normalize_auth_methods, visible_diagnostics,
};
pub use super::provider::{AuthChoice, AuthKind};

const EVENT_CAPACITY: usize = 8;
const COMMAND_CAPACITY: usize = 64;
const MAX_DETAIL_BYTES: usize = 64 * 1024;
const MAX_DISPLAY_IMAGE_BYTES: usize = 16 * 1024 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const MAX_CHOICES: usize = 128;
const MAX_PLAN_ITEMS: usize = MAX_CHOICES;
const MAX_TOOL_PATHS: usize = 256;
const MAX_STORED_SESSION_IDS: usize = 4_096;
const ACP_CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
/// Consecutive automatic resumes after retriable transport drops, per user turn.
const MAX_TURN_RESUMES: u64 = 2;
/// Prompt sent to resume a turn after the provider's upstream connection dropped.
pub const TURN_RESUME_PROMPT: &str = "Continue from where you left off.";
pub const MAX_PROMPT_ATTACHMENTS: usize = 8;
pub const MAX_PROMPT_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_PROMPT_ATTACHMENT_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

async fn wait_for_acp<T>(
    request: impl Future<Output = agent_client_protocol::Result<T>>,
    operation: &str,
) -> agent_client_protocol::Result<T> {
    wait_for_acp_with_timeout(request, operation, ACP_CONTROL_TIMEOUT).await
}

async fn wait_for_acp_with_timeout<T>(
    request: impl Future<Output = agent_client_protocol::Result<T>>,
    operation: &str,
    timeout: Duration,
) -> agent_client_protocol::Result<T> {
    let mut request = std::pin::pin!(request);
    let mut timer = std::pin::pin!(async_io::Timer::after(timeout));
    std::future::poll_fn(|context| {
        if let std::task::Poll::Ready(result) = request.as_mut().poll(context) {
            return std::task::Poll::Ready(result);
        }
        if timer.as_mut().poll(context).is_ready() {
            return std::task::Poll::Ready(Err(agent_client_protocol::util::internal_error(
                format!(
                    "{operation} timed out after {} seconds",
                    timeout.as_secs_f32()
                ),
            )));
        }
        std::task::Poll::Pending
    })
    .await
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PromptAttachmentKind {
    Image(&'static str),
    Audio(&'static str),
    File,
    Directory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptAttachment {
    path: PathBuf,
    kind: PromptAttachmentKind,
    byte_len: u64,
}

impl PromptAttachment {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, String> {
        let source = path.as_ref();
        let path = source
            .canonicalize()
            .map_err(|error| format!("cannot attach {}: {error}", source.display()))?;
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
        if !metadata.is_file() && !metadata.is_dir() {
            return Err(format!("{} is not a file or folder", path.display()));
        }
        if metadata.is_file()
            && (metadata.len() == 0 || metadata.len() > MAX_PROMPT_ATTACHMENT_BYTES)
        {
            return Err(format!(
                "{} must be a non-empty file no larger than {} MiB",
                path.display(),
                MAX_PROMPT_ATTACHMENT_BYTES / 1024 / 1024
            ));
        }
        let mut header = Vec::with_capacity(12);
        if metadata.is_file() {
            fs::File::open(&path)
                .and_then(|file| file.take(12).read_to_end(&mut header))
                .map_err(|error| {
                    format!("cannot read attached file {}: {error}", path.display())
                })?;
        }
        Ok(Self {
            path,
            kind: if metadata.is_dir() {
                PromptAttachmentKind::Directory
            } else {
                attachment_kind(&header)
            },
            byte_len: if metadata.is_dir() { 0 } else { metadata.len() },
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub const fn is_image(&self) -> bool {
        matches!(self.kind, PromptAttachmentKind::Image(_))
    }

    pub const fn is_directory(&self) -> bool {
        matches!(self.kind, PromptAttachmentKind::Directory)
    }

    pub const fn byte_len(&self) -> u64 {
        self.byte_len
    }

    fn read(&self) -> Result<Vec<u8>, String> {
        if self.is_directory() {
            return Ok(Vec::new());
        }
        let bytes = read_bounded_attachment(&self.path)?;
        if attachment_kind(&bytes) != self.kind {
            return Err(format!("{} changed file type", self.path.display()));
        }
        Ok(bytes)
    }
}

fn read_bounded_attachment(path: &Path) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path)
        .map_err(|error| format!("cannot read attached file {}: {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.take(MAX_PROMPT_ATTACHMENT_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("cannot read attached file {}: {error}", path.display()))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_PROMPT_ATTACHMENT_BYTES {
        return Err(format!(
            "{} must be a non-empty file no larger than {} MiB",
            path.display(),
            MAX_PROMPT_ATTACHMENT_BYTES / 1024 / 1024
        ));
    }
    Ok(bytes)
}

fn audio_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"ID3")
        || (bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] & 0xe0 == 0xe0)
    {
        Some("audio/mpeg")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WAVE" {
        Some("audio/wav")
    } else if bytes.starts_with(b"OggS") {
        Some("audio/ogg")
    } else if bytes.starts_with(b"fLaC") {
        Some("audio/flac")
    } else {
        None
    }
}

fn attachment_kind(bytes: &[u8]) -> PromptAttachmentKind {
    image_mime_type(bytes)
        .map(PromptAttachmentKind::Image)
        .or_else(|| audio_mime_type(bytes).map(PromptAttachmentKind::Audio))
        .unwrap_or(PromptAttachmentKind::File)
}

#[derive(Clone, Copy)]
struct AttachmentSupport {
    image: bool,
    audio: bool,
    embedded_context: bool,
}

fn image_mime_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]) {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionState {
    Provisioning { downloaded: u64, total: Option<u64> },
    Starting,
    Ready,
    AuthenticationRequired(Vec<AuthChoice>),
    Failed(String),
    Disconnected,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModeChoice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfigValue {
    Select(String),
    Boolean(bool),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigValueChoice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigChoice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub value: ConfigValue,
    pub options: Vec<ConfigValueChoice>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandChoice {
    pub name: String,
    pub description: String,
    pub input_hint: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SessionChoice {
    pub id: String,
    pub title: Option<String>,
    pub updated_at: Option<String>,
    pub started_in_editur: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SessionTranscriptMessage {
    User(String),
    Assistant(String),
    Thought(String),
    Content {
        role: ContentRole,
        content: DisplayContent,
    },
    Tool(ToolActivity),
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ContentRole {
    User,
    Assistant,
    Thought,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum DisplayContent {
    Image {
        mime_type: String,
        uri: Option<String>,
        encoded_bytes: usize,
        data: Option<Arc<[u8]>>,
    },
    Audio {
        mime_type: String,
        encoded_bytes: usize,
    },
    ResourceLink {
        name: String,
        title: Option<String>,
        uri: String,
        description: Option<String>,
        mime_type: Option<String>,
        size: Option<i64>,
    },
    TextResource {
        uri: String,
        mime_type: Option<String>,
        text: String,
    },
    BlobResource {
        uri: String,
        mime_type: Option<String>,
        encoded_bytes: usize,
    },
}

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    ConnectionChanged(ConnectionState),
    Capabilities {
        history: bool,
        allow_run_everything: bool,
        steering: bool,
        goal_actions: Vec<String>,
    },
    SessionReady {
        current_mode: Option<String>,
        modes: Vec<ModeChoice>,
        config_options: Vec<ConfigChoice>,
    },
    SessionsUpdated(Vec<SessionChoice>),
    ProjectSessionsUpdated {
        project: PathBuf,
        sessions: Vec<SessionChoice>,
    },
    SessionLoading {
        title: Option<String>,
    },
    SessionLoadFailed,
    SessionLoaded {
        current_mode: Option<String>,
        modes: Vec<ModeChoice>,
        config_options: Vec<ConfigChoice>,
    },
    SessionTranscriptStarted,
    SessionTranscriptLoaded(Vec<SessionTranscriptMessage>),
    SessionTranscriptFinished,
    ActiveSessionChanged(String),
    ModeChanged(String),
    ConfigOptionsUpdated(Vec<ConfigChoice>),
    CommandsUpdated(Vec<CommandChoice>),
    SessionTitleUpdated(Option<String>),
    UserMessage(String),
    AssistantDelta(String),
    ThoughtDelta(String),
    ContentReceived {
        role: ContentRole,
        content: DisplayContent,
    },
    PlanUpdated(Vec<PlanItem>),
    ToolCallUpdated(ToolActivity),
    PermissionRequested(PermissionRequest),
    InteractionRequested(InteractionRequest),
    UsageUpdated {
        used: u64,
        size: u64,
        cost: Option<String>,
    },
    GoalUpdated(Option<GoalState>),
    TurnFailed {
        kind: TurnFailureKind,
        message: String,
    },
    TurnFinished {
        cancelled: bool,
    },
    Error(String),
    ProcessExited {
        error: String,
        diagnostics: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TurnFailureKind {
    UsageExhausted { reset_at: Option<SystemTime> },
    Authentication,
    Transport,
    Other,
}

fn classify_turn_failure(
    provider: ProviderId,
    error: &agent_client_protocol::Error,
    claude_rate_limit_status: Option<&str>,
) -> TurnFailureKind {
    if error.code == ErrorCode::AuthRequired
        || (provider == ProviderId::Claude
            && error
                .data
                .as_ref()
                .and_then(|data| data.get("errorKind"))
                .and_then(serde_json::Value::as_str)
                == Some("authentication_failed"))
    {
        return TurnFailureKind::Authentication;
    }
    if is_retriable_transport_error(error) {
        return TurnFailureKind::Transport;
    }
    let exhausted = match provider {
        ProviderId::Codex => {
            error
                .data
                .as_ref()
                .and_then(|data| data.get("codexErrorInfo"))
                .and_then(serde_json::Value::as_str)
                == Some("usageLimitExceeded")
        }
        ProviderId::Claude => {
            claude_rate_limit_status == Some("rejected")
                && error
                    .data
                    .as_ref()
                    .and_then(|data| data.get("errorKind"))
                    .and_then(serde_json::Value::as_str)
                    == Some("rate_limit")
        }
        ProviderId::Cursor => false,
    };
    if exhausted {
        TurnFailureKind::UsageExhausted { reset_at: None }
    } else {
        TurnFailureKind::Other
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GoalState {
    pub objective: String,
    pub status: String,
    #[serde(default)]
    pub iterations: Option<u64>,
    #[serde(default)]
    pub last_reason: Option<String>,
    #[serde(default)]
    pub token_budget: Option<u64>,
    #[serde(default)]
    pub tokens_used: Option<u64>,
    #[serde(default)]
    pub time_used_seconds: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanItem {
    pub content: String,
    pub status: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolActivity {
    pub id: String,
    pub title: Option<String>,
    pub status: Option<String>,
    /// The ACP `ToolCall.kind` (`Read`, `Edit`, `Execute`, …) or `Task` for
    /// `cursor/task` subagent notifications.
    pub kind: Option<String>,
    pub paths: Vec<ToolPath>,
    pub detail: Option<ToolDetail>,
}

impl ToolActivity {
    /// Title shown on the activity card. Codex/Claude sometimes ship bare tool
    /// names (`wait`, `spawn_agent`, `Bash`); turn those into readable labels
    /// using kind, paths, and raw input when the agent did not.
    pub fn display_title(&self) -> std::borrow::Cow<'_, str> {
        tool_display_title(
            self.title.as_deref(),
            self.kind.as_deref(),
            &self.paths,
            self.detail
                .as_ref()
                .and_then(|detail| detail.input.as_deref()),
        )
    }

    pub fn command(&self) -> Option<String> {
        tool_command(
            self.title.as_deref(),
            self.kind.as_deref(),
            self.detail
                .as_ref()
                .and_then(|detail| detail.input.as_deref())
                .and_then(parse_tool_input)
                .as_ref(),
        )
    }
}

/// A file the tool touched, with the optional line number the agent reported
/// for that location.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolPath {
    pub path: PathBuf,
    pub line: Option<u32>,
}

impl From<PathBuf> for ToolPath {
    fn from(path: PathBuf) -> Self {
        Self { path, line: None }
    }
}

impl From<String> for ToolPath {
    fn from(path: String) -> Self {
        PathBuf::from(path).into()
    }
}

impl From<&str> for ToolPath {
    fn from(path: &str) -> Self {
        PathBuf::from(path).into()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ToolDetail {
    pub input: Option<String>,
    pub content: Vec<ToolOutput>,
    pub output: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ToolOutput {
    Text(String),
    Log {
        label: String,
        text: String,
    },
    Content(DisplayContent),
    Diff {
        path: PathBuf,
        old_text: Option<Arc<str>>,
        new_text: Arc<str>,
    },
    Terminal(String),
    Todo {
        id: String,
        content: String,
        status: String,
    },
    Task {
        description: String,
        prompt: String,
        subagent_type: String,
        model: Option<String>,
        agent_id: Option<String>,
        agents: Vec<SubagentInfo>,
        path: Option<String>,
        activity: Option<String>,
        duration_ms: Option<u64>,
    },
    GeneratedImage {
        description: String,
        file_path: Option<PathBuf>,
        reference_image_paths: Vec<PathBuf>,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SubagentInfo {
    pub id: String,
    pub status: Option<String>,
    pub message: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionRequest {
    pub request_id: u64,
    pub tool_call_id: String,
    pub action: String,
    pub options: Vec<PermissionChoice>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PermissionChoice {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct InteractionRequest {
    pub request_id: u64,
    pub tool_call_id: String,
    pub kind: InteractionKind,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum InteractionKind {
    Questions {
        title: String,
        questions: Vec<Question>,
    },
    Plan(PlanProposal),
    Url {
        title: String,
        url: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Question {
    pub id: String,
    pub prompt: String,
    pub options: Vec<QuestionOption>,
    pub allow_multiple: bool,
    pub required: bool,
    pub secret: bool,
    pub default_values: Vec<String>,
    pub value_kind: QuestionValueKind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum QuestionValueKind {
    String,
    Number,
    Integer,
    Boolean,
    StringArray,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanProposal {
    pub name: Option<String>,
    pub overview: Option<String>,
    pub plan: String,
    pub todos: Vec<PlanItem>,
    pub is_project: Option<bool>,
    pub phases: Vec<PlanPhase>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct PlanPhase {
    pub name: String,
    pub todos: Vec<PlanItem>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuestionAnswer {
    pub question_id: String,
    pub selected_option_ids: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractionResponse {
    Answers(Vec<QuestionAnswer>),
    Skipped,
    PlanAccepted,
    PlanRejected,
    Accepted,
    Declined,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Connect,
    Authenticate(String),
    NewSession,
    RefreshSessions,
    RefreshProjectSessions(Vec<PathBuf>),
    LoadSession(String),
    RemoveSession(String),
    SetMode(String),
    SetConfig {
        id: String,
        value: ConfigValue,
    },
    SetRunEverything(bool),
    ControlGoal(GoalAction),
    Prompt(String),
    PromptWithAttachments {
        text: String,
        attachments: Vec<PromptAttachment>,
    },
    HiddenPromptWithAttachments {
        text: String,
        attachments: Vec<PromptAttachment>,
    },
    DecidePermission {
        request_id: u64,
        option_id: String,
    },
    RespondInteraction {
        request_id: u64,
        response: InteractionResponse,
    },
    Cancel,
    Shutdown,
    #[doc(hidden)]
    TransportFailed(String),
    #[doc(hidden)]
    TerminalAuthFinished(Result<(), String>),
    #[doc(hidden)]
    ResumeTurn,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum GoalAction {
    Set(String),
    Pause,
    Resume,
    Clear,
}

pub struct AgentController {
    commands: async_channel::Sender<Command>,
    events: Receiver<Event>,
    shutdown: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}

fn legacy_account(provider: ProviderId) -> ProviderAccount {
    ProviderAccount {
        key: AccountKey {
            provider,
            account_id: 1,
        },
        label: "Current login".into(),
        auth_source: AuthSource::Legacy,
        auto_failover: false,
    }
}

impl AgentController {
    pub fn start(provider: ProviderId, project_root: PathBuf) -> Self {
        Self::start_account(legacy_account(provider), project_root)
    }

    pub fn start_account(account: ProviderAccount, project_root: PathBuf) -> Self {
        Self::start_launch(
            account.key.provider,
            managed_session_startup(&account, &project_root, None, false),
            project_root,
            Launch::Managed(account),
            Arc::new(|| {}),
        )
    }

    pub fn start_with_wake(
        provider: ProviderId,
        project_root: PathBuf,
        preferred_session: Option<String>,
        fresh_session: bool,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::start_account_with_wake(
            legacy_account(provider),
            project_root,
            preferred_session,
            fresh_session,
            wake,
        )
    }

    pub fn start_account_with_wake(
        account: ProviderAccount,
        project_root: PathBuf,
        preferred_session: Option<String>,
        fresh_session: bool,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::start_launch(
            account.key.provider,
            managed_session_startup(&account, &project_root, preferred_session, fresh_session),
            project_root,
            Launch::Managed(account),
            Arc::new(wake),
        )
    }

    #[doc(hidden)]
    pub fn start_process(project_root: PathBuf, command: PathBuf, args: Vec<String>) -> Self {
        Self::start_process_for(ProviderId::Cursor, project_root, command, args)
    }

    #[doc(hidden)]
    pub fn start_process_for(
        provider: ProviderId,
        project_root: PathBuf,
        command: PathBuf,
        args: Vec<String>,
    ) -> Self {
        let history = Some(project_root.join(format!(
            ".editur-test-hidden-sessions-{}.json",
            provider.as_str()
        )));
        let editur_sessions =
            Some(project_root.join(format!(".editur-test-sessions-{}.json", provider.as_str())));
        Self::start_launch(
            provider,
            SessionStartup {
                history,
                active_session: None,
                editur_sessions,
                preferred_session: None,
                fresh_session: false,
                account: None,
            },
            project_root,
            Launch::Process(AcpAgentConfig::new(command).args(args)),
            Arc::new(|| {}),
        )
    }

    #[doc(hidden)]
    pub fn start_process_fresh(project_root: PathBuf, command: PathBuf, args: Vec<String>) -> Self {
        let history = Some(project_root.join(".editur-test-hidden-sessions-cursor.json"));
        let editur_sessions = Some(project_root.join(".editur-test-sessions-cursor.json"));
        Self::start_launch(
            ProviderId::Cursor,
            SessionStartup {
                history,
                active_session: None,
                editur_sessions,
                preferred_session: None,
                fresh_session: true,
                account: None,
            },
            project_root,
            Launch::Process(AcpAgentConfig::new(command).args(args)),
            Arc::new(|| {}),
        )
    }

    #[doc(hidden)]
    pub fn start_process_resuming(
        project_root: PathBuf,
        command: PathBuf,
        args: Vec<String>,
        preferred_session: String,
    ) -> Self {
        let history = Some(project_root.join(".editur-test-hidden-sessions-cursor.json"));
        let editur_sessions = Some(project_root.join(".editur-test-sessions-cursor.json"));
        Self::start_launch(
            ProviderId::Cursor,
            SessionStartup {
                history,
                active_session: None,
                editur_sessions,
                preferred_session: Some(preferred_session),
                fresh_session: false,
                account: None,
            },
            project_root,
            Launch::Process(AcpAgentConfig::new(command).args(args)),
            Arc::new(|| {}),
        )
    }

    fn start_launch(
        provider: ProviderId,
        session_startup: SessionStartup,
        project_root: PathBuf,
        launch: Launch,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (command_tx, command_rx) = async_channel::bounded(COMMAND_CAPACITY);
        let debug_commands = command_tx.clone();
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(EVENT_CAPACITY);
        let shutdown = Arc::new(AtomicBool::new(false));
        let event_tx = EventSender {
            provider,
            event_tx,
            wake,
            active_session: session_startup.active_session.clone(),
        };
        let worker_shutdown = Arc::clone(&shutdown);
        let worker = thread::Builder::new()
            .name("editur-agent".into())
            .spawn(move || {
                run_thread(
                    provider,
                    project_root,
                    launch,
                    command_rx,
                    debug_commands,
                    event_tx,
                    session_startup,
                    worker_shutdown,
                )
            })
            .expect("failed to start Editur agent controller thread");
        Self {
            commands: command_tx,
            events: event_rx,
            shutdown,
            worker: Some(worker),
        }
    }

    pub fn send(&self, command: Command) -> Result<(), String> {
        self.commands
            .try_send(command)
            .map_err(|error| format!("agent command could not be sent: {error}"))
    }

    pub fn events(&self) -> &Receiver<Event> {
        &self.events
    }
}

impl Drop for AgentController {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = self.commands.try_send(Command::Shutdown);
            crate::reap_worker("editur-agent-reaper", worker);
        }
    }
}

enum Launch {
    Managed(ProviderAccount),
    Process(AcpAgentConfig),
}

struct SessionStartup {
    history: Option<PathBuf>,
    active_session: Option<PathBuf>,
    editur_sessions: Option<PathBuf>,
    preferred_session: Option<String>,
    fresh_session: bool,
    account: Option<ProviderAccount>,
}

#[derive(Clone)]
struct EventSender {
    provider: ProviderId,
    event_tx: SyncSender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
    active_session: Option<PathBuf>,
}

#[derive(Clone, Default)]
struct SessionNotificationGate(Arc<Mutex<Option<String>>>);

impl SessionNotificationGate {
    fn accepts(&self, session_id: &SessionId) -> bool {
        self.0
            .lock()
            .map(|active| {
                active
                    .as_deref()
                    .is_none_or(|active| active == session_id.0.as_ref())
            })
            .unwrap_or(false)
    }

    fn activate(&self, session_id: &SessionId) -> Option<String> {
        self.0
            .lock()
            .map(|mut active| active.replace(session_id.0.to_string()))
            .unwrap_or_default()
    }

    fn restore(&self, session_id: Option<String>) {
        if let Ok(mut active) = self.0.lock() {
            *active = session_id;
        }
    }
}

#[expect(clippy::too_many_arguments)]
fn run_thread(
    provider: ProviderId,
    project_root: PathBuf,
    launch: Launch,
    commands: async_channel::Receiver<Command>,
    debug_commands: async_channel::Sender<Command>,
    events: EventSender,
    session_startup: SessionStartup,
    shutdown: Arc<AtomicBool>,
) {
    let system_terminal = matches!(launch, Launch::Managed(_));
    let (config, _managed_tree, provider_data) = match launch {
        Launch::Managed(account) => match managed_config(&account, &project_root, &events) {
            Ok(config) => (
                config.0,
                Some(config.1),
                (account.auth_source != AuthSource::Legacy)
                    .then(|| crate::data_dir().ok())
                    .flatten()
                    .and_then(|data_dir| {
                        super::provision::account_provider_data_root(&data_dir, account.key).ok()
                    }),
            ),
            Err(error) => {
                send_event(
                    &events,
                    Event::ConnectionChanged(ConnectionState::Failed(error)),
                );
                return;
            }
        },
        Launch::Process(config) => (config, None, None),
    };
    send_event(&events, Event::ConnectionChanged(ConnectionState::Starting));
    let diagnostics = Arc::new(Mutex::new(String::new()));
    let debug_diagnostics = Arc::clone(&diagnostics);
    let protocol_debug = std::env::var("EDITUR_LOG").as_deref() == Ok("debug");
    let auth_config = config.clone();
    let internal_commands = debug_commands.clone();
    let agent = AcpAgent::new(config)
        .with_shutdown_flag(Arc::clone(&shutdown))
        .with_debug(move |line, direction| {
            if direction == LineDirection::Stderr {
                append_bounded(&debug_diagnostics, line, MAX_DIAGNOSTIC_BYTES);
            } else if direction == LineDirection::Stdout {
                let valid = if protocol_debug {
                    serde_json::from_str::<serde_json::Value>(line)
                        .inspect(|message| {
                            if let Some(label) = protocol_label(message) {
                                eprintln!("editur: ACP <- {label}");
                            }
                        })
                        .is_ok()
                } else {
                    serde_json::from_str::<serde::de::IgnoredAny>(line).is_ok()
                };
                if !valid {
                    let _ = debug_commands.try_send(Command::TransportFailed(format!(
                        "{} wrote malformed JSON to stdout",
                        descriptor(provider).display_name
                    )));
                }
            }
        });
    let result = async_io::block_on(run_connection(
        (agent, auth_config, system_terminal, internal_commands),
        (provider, project_root, provider_data),
        commands,
        events.clone(),
        Arc::clone(&shutdown),
        session_startup,
    ));
    if shutdown.load(Ordering::Acquire) {
        send_event(
            &events,
            Event::ConnectionChanged(ConnectionState::Disconnected),
        );
    } else if let Err(error) = result {
        let raw_diagnostics = diagnostics
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default();
        let error = connection_error(&error, &raw_diagnostics);
        let error = if provider == ProviderId::Claude {
            acp_error(provider, "connection failed", &error)
        } else {
            error
        };
        let diagnostics = visible_diagnostics(provider, raw_diagnostics);
        send_event(&events, Event::ProcessExited { error, diagnostics });
    }
}

fn connection_error(error: &agent_client_protocol::Error, diagnostics: &str) -> String {
    let mut message = error
        .data
        .as_ref()
        .and_then(|data| {
            data.as_str().or_else(|| {
                data.get("data")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| data.get("message").and_then(serde_json::Value::as_str))
            })
        })
        .unwrap_or(&error.message)
        .trim()
        .to_owned();
    if let Some(prefix) = message
        .strip_suffix(diagnostics.trim())
        .map(|prefix| {
            prefix.trim_end_matches(|character: char| character.is_whitespace() || character == ':')
        })
        .filter(|prefix| !prefix.is_empty())
    {
        message = prefix.to_owned();
    }
    message
}

fn acp_error(provider: ProviderId, action: &str, error: &impl std::fmt::Display) -> String {
    if provider == ProviderId::Claude {
        format!("Claude {action}")
    } else {
        format!("{action}: {error}")
    }
}

fn terminal_auth_config(agent: &AcpAgentConfig, method: &AuthMethodTerminal) -> AcpAgentConfig {
    AcpAgentConfig::new(agent.command())
        .args(agent.arguments())
        .args(&method.args)
        .envs(
            agent
                .environment()
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )
        .envs(
            method
                .env
                .iter()
                .map(|(name, value)| (name.clone(), value.clone())),
        )
}

fn launch_terminal_auth(
    config: &AcpAgentConfig,
    project_root: &Path,
    system_terminal: bool,
) -> Result<(), String> {
    if !system_terminal {
        return run_terminal_auth(config, project_root);
    }
    launch_system_terminal_auth(config, project_root)
}

fn run_terminal_auth(config: &AcpAgentConfig, project_root: &Path) -> Result<(), String> {
    let status = std::process::Command::new(config.command())
        .args(config.arguments())
        .envs(config.environment())
        .current_dir(project_root)
        .status()
        .map_err(|error| format!("cannot start Claude CLI authentication: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("Claude CLI authentication exited with {status}"))
}

#[cfg(target_os = "macos")]
fn launch_system_terminal_auth(config: &AcpAgentConfig, project_root: &Path) -> Result<(), String> {
    if !config.environment().is_empty() {
        return Err("Claude terminal authentication supplied unsupported environment data".into());
    }
    let command = std::iter::once(config.command().as_os_str())
        .chain(config.arguments().iter().map(std::ffi::OsStr::new))
        .map(shell_quote)
        .collect::<Result<Vec<_>, _>>()?
        .join(" ");
    let command = format!("cd {} && {command}", shell_quote(project_root.as_os_str())?);
    let script = format!(
        "tell application \"Terminal\"\nactivate\nset authTab to do script {}\nrepeat while busy of authTab\ndelay 0.2\nend repeat\nend tell",
        apple_script_string(&command)
    );
    let status = std::process::Command::new("/usr/bin/osascript")
        .args(["-e", &script])
        .status()
        .map_err(|error| format!("cannot open Claude authentication terminal: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| "cannot open Claude authentication terminal".into())
}

#[cfg(target_os = "macos")]
fn shell_quote(value: &std::ffi::OsStr) -> Result<String, String> {
    let value = value
        .to_str()
        .ok_or_else(|| "Claude authentication command is not valid UTF-8".to_owned())?;
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

#[cfg(target_os = "macos")]
fn apple_script_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('\"', "\\\""))
}

#[cfg(target_os = "linux")]
fn launch_system_terminal_auth(config: &AcpAgentConfig, project_root: &Path) -> Result<(), String> {
    for (terminal, prefix) in [
        ("x-terminal-emulator", &["-e"][..]),
        ("gnome-terminal", &["--wait", "--"][..]),
        ("konsole", &["-e"][..]),
    ] {
        match std::process::Command::new(terminal)
            .args(prefix)
            .arg(config.command())
            .args(config.arguments())
            .envs(config.environment())
            .current_dir(project_root)
            .spawn()
        {
            Ok(mut child) => {
                let status = child.wait().map_err(|error| {
                    format!("cannot wait for Claude CLI authentication: {error}")
                })?;
                return status
                    .success()
                    .then_some(())
                    .ok_or_else(|| format!("Claude CLI authentication exited with {status}"));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "cannot open Claude authentication terminal: {error}"
                ));
            }
        }
    }
    Err("cannot open Claude authentication terminal: no supported terminal was found".into())
}

#[cfg(windows)]
fn launch_system_terminal_auth(config: &AcpAgentConfig, project_root: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt as _;

    const CREATE_NEW_CONSOLE: u32 = 0x0000_0010;
    let status = std::process::Command::new(config.command())
        .args(config.arguments())
        .envs(config.environment())
        .current_dir(project_root)
        .creation_flags(CREATE_NEW_CONSOLE)
        .status()
        .map_err(|error| format!("cannot open Claude authentication terminal: {error}"))?;
    status
        .success()
        .then_some(())
        .ok_or_else(|| format!("Claude CLI authentication exited with {status}"))
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
fn launch_system_terminal_auth(
    _config: &AcpAgentConfig,
    _project_root: &Path,
) -> Result<(), String> {
    Err("Claude terminal authentication is unsupported on this operating system".into())
}

fn managed_config(
    account: &ProviderAccount,
    project_root: &std::path::Path,
    events: &EventSender,
) -> Result<(AcpAgentConfig, ManagedTree), String> {
    let data_dir = crate::data_dir()?;
    super::provider::prepare_account(account, &data_dir, |progress| {
        send_event(
            events,
            Event::ConnectionChanged(ConnectionState::Provisioning {
                downloaded: progress.downloaded,
                total: progress.total,
            }),
        );
    })?;
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate Editur agent launcher: {error}"))?;
    let config = AcpAgentConfig::new(executable)
        .arg("--agent-process")
        .arg(account.key.provider.as_str())
        .arg(account.key.account_id.to_string())
        .arg(project_root.to_string_lossy());
    protect_managed_process(config)
}

struct ManagedTree {
    #[cfg(windows)]
    _job: super::WindowsJob,
}

#[cfg(windows)]
fn protect_managed_process(
    config: AcpAgentConfig,
) -> Result<(AcpAgentConfig, ManagedTree), String> {
    let (name, job) = super::new_windows_job()?;
    Ok((
        config.env(super::WINDOWS_JOB_ENV, name),
        ManagedTree { _job: job },
    ))
}

#[cfg(not(windows))]
fn protect_managed_process(
    config: AcpAgentConfig,
) -> Result<(AcpAgentConfig, ManagedTree), String> {
    Ok((config, ManagedTree {}))
}

fn client_capabilities(provider: ProviderId) -> ClientCapabilities {
    let mut capabilities = ClientCapabilities::new()
        .session(ClientSessionCapabilities::new().config_options(
            SessionConfigOptionsCapabilities::new().boolean(BooleanConfigOptionCapabilities::new()),
        ))
        .elicitation(
            ElicitationCapabilities::new()
                .form(ElicitationFormCapabilities::new())
                .url(ElicitationUrlCapabilities::new()),
        );
    if provider == ProviderId::Cursor {
        capabilities = capabilities.meta(serde_json::Map::from_iter([(
            "parameterizedModelPicker".into(),
            serde_json::Value::Bool(true),
        )]));
    }
    if provider == ProviderId::Claude {
        capabilities = capabilities
            .auth(AuthCapabilities::new().terminal(true))
            .meta(serde_json::Map::from_iter([
                ("subagent-transcript".into(), serde_json::Value::Bool(true)),
                ("terminal_output".into(), serde_json::Value::Bool(true)),
            ]));
    }
    capabilities
}

fn session_extension_capabilities(
    provider: ProviderId,
    meta: Option<&Meta>,
) -> (bool, Vec<String>) {
    if provider == ProviderId::Cursor {
        return (false, Vec::new());
    }
    let steering = meta
        .and_then(|meta| meta.get("steering"))
        .and_then(|value| value.get("supported"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let goal_actions = meta
        .and_then(|meta| meta.get("goal"))
        .filter(|goal| {
            goal.get("controlMethod")
                .and_then(serde_json::Value::as_str)
                == Some("_session/goal")
        })
        .and_then(|goal| goal.get("actions"))
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(serde_json::Value::as_str)
        .filter(|action| matches!(*action, "set" | "pause" | "resume" | "clear"))
        .take(4)
        .map(str::to_owned)
        .collect();
    (steering, goal_actions)
}

async fn run_connection(
    (agent, auth_config, system_terminal, internal_commands): (
        AcpAgent,
        AcpAgentConfig,
        bool,
        async_channel::Sender<Command>,
    ),
    (provider, project_root, provider_data): (ProviderId, PathBuf, Option<PathBuf>),
    commands: async_channel::Receiver<Command>,
    events: EventSender,
    shutdown: Arc<AtomicBool>,
    session_startup: SessionStartup,
) -> agent_client_protocol::Result<()> {
    let extensions = descriptor(provider).extensions;
    let active = Arc::new(AtomicBool::new(false));
    let turn_resume = TurnResume {
        enabled: extensions == ProviderExtensions::Cursor,
        commands: internal_commands.clone(),
        attempts: Arc::new(AtomicU64::new(0)),
    };
    let auto_approve_permissions = Arc::new(AtomicBool::new(false));
    let permissions = Arc::new(Mutex::new(HashMap::new()));
    let interactions = Arc::new(Mutex::new(HashMap::new()));
    let elicitations = Arc::new(Mutex::new(HashMap::new()));
    let claude_rate_limit_status = Arc::new(Mutex::new(None::<String>));
    let next_permission = Arc::new(AtomicU64::new(1));
    let mut hidden_sessions = HiddenSessions::load(session_startup.history);
    let mut editur_sessions = EditurSessions::load(session_startup.editur_sessions);
    let preferred_session = session_startup.preferred_session;
    let fresh_session = session_startup.fresh_session;
    let project_account = session_startup.account;
    let session_notifications = SessionNotificationGate::default();
    agent_client_protocol::Client
        .builder()
        .name("editur")
        .on_receive_notification(
            {
                let events = events.clone();
                let session_notifications = session_notifications.clone();
                let claude_rate_limit_status = Arc::clone(&claude_rate_limit_status);
                async move |notification: SessionNotification, _connection| {
                    if session_notifications.accepts(&notification.session_id) {
                        normalize_update_with_rate_limit(
                            notification.update,
                            &events,
                            &claude_rate_limit_status,
                        );
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_notification(
            {
                let events = events.clone();
                async move |notification: agent_client_protocol::UntypedMessage, _connection| {
                    if extensions == ProviderExtensions::Cursor {
                        normalize_cursor_notification(
                            notification.method(),
                            notification.params().clone(),
                            &events,
                        );
                    }
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .on_receive_request(
            {
                let events = events.clone();
                let auto_approve_permissions = Arc::clone(&auto_approve_permissions);
                let permissions = Arc::clone(&permissions);
                let next_permission = Arc::clone(&next_permission);
                async move |request: RequestPermissionRequest,
                            responder,
                            connection: ConnectionTo<Agent>| {
                    if request.options.is_empty() || request.options.len() > MAX_CHOICES {
                        send_event(
                            &events,
                            Event::Error(format!(
                                "agent supplied {} permission choices; expected 1..={MAX_CHOICES}",
                                request.options.len()
                            )),
                        );
                        responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Cancelled,
                        ))?;
                        return Ok(());
                    }
                    if auto_approve_permissions.load(Ordering::Acquire)
                        && let Some(option) = request
                            .options
                            .iter()
                            .find(|option| option.kind == PermissionOptionKind::AllowAlways)
                            .or_else(|| {
                                request
                                    .options
                                    .iter()
                                    .find(|option| option.kind == PermissionOptionKind::AllowOnce)
                            })
                    {
                        responder.respond(RequestPermissionResponse::new(
                            RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                option.option_id.clone(),
                            )),
                        ))?;
                        return Ok(());
                    }
                    let request_id = next_permission.fetch_add(1, Ordering::Relaxed);
                    let choices = request
                        .options
                        .iter()
                        .take(MAX_CHOICES)
                        .map(|option| PermissionChoice {
                            id: option.option_id.0.to_string(),
                            name: option.name.clone(),
                            kind: format!("{:?}", option.kind),
                        })
                        .collect::<Vec<_>>();
                    let allowed = choices.iter().map(|choice| choice.id.clone()).collect();
                    let (decision_tx, decision_rx) = async_channel::bounded(1);
                    permissions
                        .lock()
                        .expect("permission lock poisoned")
                        .insert(
                            request_id,
                            PendingPermission {
                                allowed,
                                decision_tx,
                            },
                        );
                    send_event(
                        &events,
                        Event::PermissionRequested(PermissionRequest {
                            request_id,
                            tool_call_id: request.tool_call.tool_call_id.0.to_string(),
                            action: request
                                .tool_call
                                .fields
                                .title
                                .clone()
                                .unwrap_or_else(|| "Run requested action".into()),
                            options: choices,
                        }),
                    );
                    connection.spawn(async move {
                        let Ok(decision) = decision_rx.recv().await else {
                            return Ok(());
                        };
                        responder.respond(RequestPermissionResponse::new(match decision {
                            PendingDecision::Selected(option_id) => {
                                RequestPermissionOutcome::Selected(SelectedPermissionOutcome::new(
                                    option_id,
                                ))
                            }
                            PendingDecision::Cancelled => RequestPermissionOutcome::Cancelled,
                        }))
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let events = events.clone();
                let elicitations = Arc::clone(&elicitations);
                let next_request = Arc::clone(&next_permission);
                async move |request: CreateElicitationRequest,
                            responder,
                            connection: ConnectionTo<Agent>| {
                    let request_id = next_request.fetch_add(1, Ordering::Relaxed);
                    let (request, kind) = match parse_elicitation(request_id, request) {
                        Ok(parsed) => parsed,
                        Err(error) => {
                            send_event(&events, Event::Error(error));
                            responder.respond(CreateElicitationResponse::new(
                                ElicitationAction::Cancel,
                            ))?;
                            return Ok(());
                        }
                    };
                    let (response_tx, response_rx) = async_channel::bounded(1);
                    elicitations
                        .lock()
                        .expect("elicitation lock poisoned")
                        .insert(request_id, PendingElicitation { kind, response_tx });
                    send_event(&events, Event::InteractionRequested(request));
                    connection.spawn(async move {
                        let action = response_rx
                            .recv()
                            .await
                            .unwrap_or(ElicitationAction::Cancel);
                        responder.respond(CreateElicitationResponse::new(action))
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let events = events.clone();
                let interactions = Arc::clone(&interactions);
                let next_request = Arc::clone(&next_permission);
                async move |request: CursorRequest, responder, connection: ConnectionTo<Agent>| {
                    if extensions != ProviderExtensions::Cursor {
                        responder.respond(
                            serde_json::json!({"outcome": {"outcome": "cancelled"}}),
                        )?;
                        return Ok(());
                    }
                    let request_id = next_request.fetch_add(1, Ordering::Relaxed);
                    let (request, kind) = match parse_cursor_interaction(request_id, request) {
                        Ok(parsed) => parsed,
                        Err(error) => {
                            send_event(&events, Event::Error(error));
                            responder.respond(
                                serde_json::json!({"outcome": {"outcome": "cancelled"}}),
                            )?;
                            return Ok(());
                        }
                    };
                    let (response_tx, response_rx) = async_channel::bounded(1);
                    interactions
                        .lock()
                        .expect("interaction lock poisoned")
                        .insert(request_id, PendingInteraction { kind, response_tx });
                    send_event(&events, Event::InteractionRequested(request));
                    connection.spawn(async move {
                        let response = response_rx.recv().await.unwrap_or_else(
                            |_| serde_json::json!({"outcome": {"outcome": "cancelled"}}),
                        );
                        responder.respond(response)
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .connect_with(agent, |connection: ConnectionTo<Agent>| {
            let events = events.clone();
            let active = Arc::clone(&active);
            let auto_approve_permissions = Arc::clone(&auto_approve_permissions);
            let permissions = Arc::clone(&permissions);
            let interactions = Arc::clone(&interactions);
            let elicitations = Arc::clone(&elicitations);
            let shutdown = Arc::clone(&shutdown);
            async move {
                let client_capabilities = client_capabilities(provider);
                let initialized = wait_for_acp(
                    connection
                        .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(client_capabilities)
                            .client_info(Implementation::new("editur", env!("CARGO_PKG_VERSION"))),
                        )
                        .block_task(),
                    "agent initialization",
                )
                .await?;
                if initialized.protocol_version != ProtocolVersion::V1 {
                    return Err(agent_client_protocol::Error::invalid_request()
                        .data("agent does not support stable ACP v1"));
                }
                let attachment_support = AttachmentSupport {
                    image: initialized.agent_capabilities.prompt_capabilities.image,
                    audio: initialized.agent_capabilities.prompt_capabilities.audio,
                    embedded_context: initialized
                        .agent_capabilities
                        .prompt_capabilities
                        .embedded_context,
                };
                let mut auth = normalize_auth_methods(provider, &initialized.auth_methods);
                let supports_history = initialized.agent_capabilities.load_session
                    && initialized
                        .agent_capabilities
                        .session_capabilities
                        .list
                        .is_some();
                let supports_close = initialized
                    .agent_capabilities
                    .session_capabilities
                    .close
                    .is_some();
                let supports_resume = initialized
                    .agent_capabilities
                    .session_capabilities
                    .resume
                    .is_some();
                let supports_additional_directories = initialized
                    .agent_capabilities
                    .session_capabilities
                    .additional_directories
                    .is_some();
                let (supports_steering, goal_actions) =
                    session_extension_capabilities(provider, initialized.meta.as_ref());
                let discovered_external = external_sessions::discover(
                    provider,
                    &project_root,
                    provider_data.as_deref(),
                )
                    .unwrap_or_else(|error| {
                        if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
                            eprintln!("editur: {error}");
                        }
                        Vec::new()
                    });
                send_event(
                    &events,
                    Event::Capabilities {
                        history: supports_history || !discovered_external.is_empty(),
                        allow_run_everything: extensions == ProviderExtensions::Cursor,
                        steering: supports_steering,
                        goal_actions: goal_actions.clone(),
                    },
                );
                let (mut session_id, native_sessions) = match start_session(
                    &connection,
                    &project_root,
                    &events,
                    supports_history,
                    &hidden_sessions.ids,
                    &mut editur_sessions,
                    preferred_session.as_deref(),
                    fresh_session,
                    &session_notifications,
                    supports_close,
                )
                .await
                {
                    Ok((session_id, sessions)) => (Some(session_id), sessions),
                    Err(error) => {
                        auth = authentication_required_choices(provider, auth);
                        if auth.is_empty() {
                            return Err(error);
                        }
                        send_event(
                            &events,
                            Event::ConnectionChanged(ConnectionState::AuthenticationRequired(
                                auth.clone(),
                            )),
                        );
                        (None, Vec::new())
                    }
                };
                let (mut sessions, mut external_sessions) =
                    merge_external_sessions(
                        native_sessions,
                        discovered_external,
                        &hidden_sessions.ids,
                    );
                let mut pending_external = None;
                let mut additional_directories = Vec::new();
                send_event(
                    &events,
                    Event::ProjectSessionsUpdated {
                        project: project_root.clone(),
                        sessions: sessions.clone(),
                    },
                );
                if session_id.is_some()
                    && let Some(preferred) = preferred_session.as_deref()
                    && external_sessions::is_external_choice(preferred)
                    && let Some(external) = external_sessions.get(preferred).cloned()
                    && load_external_session(&external, preferred, &events).is_ok()
                {
                    pending_external = Some(external);
                }
                if pending_external.is_none()
                    && let Some(session) = session_id.as_ref()
                {
                    enrich_native_session(session.0.as_ref(), &external_sessions, &events);
                }
                let mut terminal_auth_in_progress = false;
                while let Ok(command) = commands.recv().await {
                    match command {
                        Command::Connect => {}
                        Command::Authenticate(method_id) => {
                            let Some(method) = auth.iter().find(|method| method.id == method_id)
                            else {
                                send_event(
                                    &events,
                                    Event::Error("unknown authentication method".into()),
                                );
                                continue;
                            };
                            if !method.can_authenticate {
                                send_event(
                                    &events,
                                    Event::Error(format!(
                                        "{} authentication requires a setup flow Editur does not support",
                                        method.name
                                    )),
                                );
                                continue;
                            }
                            if let Some(AuthMethod::Terminal(terminal)) = initialized
                                .auth_methods
                                .iter()
                                .find(|candidate| candidate.id().0.as_ref() == method_id)
                            {
                                if terminal_auth_in_progress {
                                    continue;
                                }
                                terminal_auth_in_progress = true;
                                send_event(
                                    &events,
                                    Event::ConnectionChanged(ConnectionState::Starting),
                                );
                                let config = terminal_auth_config(&auth_config, terminal);
                                let project_root = project_root.clone();
                                let internal_commands = internal_commands.clone();
                                if let Err(error) = thread::Builder::new()
                                    .name("editur-agent-auth".into())
                                    .spawn(move || {
                                        let result = launch_terminal_auth(
                                            &config,
                                            &project_root,
                                            system_terminal,
                                        );
                                        let _ = internal_commands
                                            .send_blocking(Command::TerminalAuthFinished(result));
                                    })
                                {
                                    terminal_auth_in_progress = false;
                                    send_event(
                                        &events,
                                        Event::Error(format!(
                                            "cannot start Claude CLI authentication: {error}"
                                        )),
                                    );
                                    send_event(
                                        &events,
                                        Event::ConnectionChanged(
                                            ConnectionState::AuthenticationRequired(auth.clone()),
                                        ),
                                    );
                                }
                                continue;
                            }
                            let authenticated = wait_for_acp(
                                connection
                                    .send_request(AuthenticateRequest::new(method.id.clone()))
                                    .block_task(),
                                "authentication",
                            )
                            .await;
                            if let Err(error) = authenticated {
                                send_event(
                                    &events,
                                    Event::Error(acp_error(
                                        provider,
                                        "authentication failed",
                                        &error,
                                    )),
                                );
                                continue;
                            }
                            match start_session(
                                &connection,
                                &project_root,
                                &events,
                                supports_history,
                                &hidden_sessions.ids,
                                &mut editur_sessions,
                                preferred_session.as_deref(),
                                fresh_session,
                                &session_notifications,
                                supports_close,
                            )
                            .await
                            {
                                Ok((session, listed)) => {
                                    additional_directories.clear();
                                    let loaded_id = session.0.to_string();
                                    session_id = Some(session);
                                    let discovered = external_sessions::discover(
                                        provider,
                                        &project_root,
                                        provider_data.as_deref(),
                                    )
                                    .unwrap_or_default();
                                    (sessions, external_sessions) =
                                        merge_external_sessions(
                                            listed,
                                            discovered,
                                            &hidden_sessions.ids,
                                        );
                                    send_event(
                                        &events,
                                        Event::ProjectSessionsUpdated {
                                            project: project_root.clone(),
                                            sessions: sessions.clone(),
                                        },
                                    );
                                    enrich_native_session(
                                        &loaded_id,
                                        &external_sessions,
                                        &events,
                                    );
                                }
                                Err(error) => {
                                    send_event(
                                        &events,
                                        Event::Error(acp_error(
                                            provider,
                                            "cannot start session",
                                            &error,
                                        )),
                                    );
                                    send_event(
                                        &events,
                                        Event::ConnectionChanged(
                                            ConnectionState::AuthenticationRequired(auth.clone()),
                                        ),
                                    );
                                }
                            }
                        }
                        Command::NewSession => {
                            if active.load(Ordering::Acquire) {
                                send_event(
                                    &events,
                                    Event::Error(
                                        "stop the active turn before starting a new session".into(),
                                    ),
                                );
                            } else {
                                match new_session(
                                    &connection,
                                    &project_root,
                                    &events,
                                    &mut editur_sessions,
                                    &session_notifications,
                                    supports_close,
                                )
                                .await
                                {
                                    Ok(session) => {
                                        additional_directories.clear();
                                        pending_external = None;
                                        let choice = untitled_session(&session);
                                        sessions.retain(|candidate| candidate.id != choice.id);
                                        sessions.insert(0, choice);
                                        sessions.truncate(MAX_CHOICES);
                                        send_event(
                                            &events,
                                            Event::SessionsUpdated(sessions.clone()),
                                        );
                                        session_id = Some(session);
                                    }
                                    Err(error) => send_event(
                                        &events,
                                        Event::Error(acp_error(
                                            provider,
                                            "cannot start session",
                                            &error,
                                        )),
                                    ),
                                }
                            }
                        }
                        Command::RefreshSessions => {
                            let native = if supports_history {
                                match list_sessions(
                                    &connection,
                                    &project_root,
                                    &events,
                                    &hidden_sessions.ids,
                                    &editur_sessions,
                                )
                                .await
                                {
                                    Ok(listed) => Some(listed),
                                    Err(error) => {
                                        send_event(
                                            &events,
                                            Event::Error(acp_error(
                                            provider,
                                            "cannot list sessions",
                                            &error,
                                        )),
                                        );
                                        None
                                    }
                                }
                            } else {
                                Some(
                                    sessions
                                        .iter()
                                        .filter(|session| {
                                            !external_sessions::is_external_choice(&session.id)
                                        })
                                        .cloned()
                                        .collect(),
                                )
                            };
                            if let Some(native) = native {
                                let discovered = external_sessions::discover(
                                    provider,
                                    &project_root,
                                    provider_data.as_deref(),
                                )
                                .unwrap_or_default();
                                let send_merged = !supports_history || !discovered.is_empty();
                                (sessions, external_sessions) =
                                    merge_external_sessions(
                                        native,
                                        discovered,
                                        &hidden_sessions.ids,
                                    );
                                if send_merged {
                                    send_event(&events, Event::SessionsUpdated(sessions.clone()));
                                }
                            }
                        }
                        Command::RefreshProjectSessions(projects) => {
                            for project in projects.into_iter().take(MAX_CHOICES) {
                                if project == project_root {
                                    send_event(
                                        &events,
                                        Event::ProjectSessionsUpdated {
                                            project,
                                            sessions: sessions.clone(),
                                        },
                                    );
                                    continue;
                                }
                                let project_hidden = HiddenSessions::load(
                                    project_account
                                        .as_ref()
                                        .and_then(|account| session_history_path(account, &project)),
                                );
                                let project_editur = EditurSessions::load(
                                    project_account.as_ref().and_then(|account| {
                                        crate::data_dir().ok().map(|directory| {
                                            editur_sessions_path_in(
                                                &directory,
                                                account,
                                                &project,
                                            )
                                        })
                                    }),
                                );
                                let native = if supports_history {
                                    match query_sessions(
                                        &connection,
                                        &project,
                                        &project_hidden.ids,
                                        &project_editur,
                                    )
                                    .await
                                    {
                                        Ok(sessions) => sessions,
                                        Err(error) => {
                                            send_event(
                                                &events,
                                                Event::Error(acp_error(
                                                    provider,
                                                    "cannot list project sessions",
                                                    &error,
                                                )),
                                            );
                                            Vec::new()
                                        }
                                    }
                                } else {
                                    Vec::new()
                                };
                                let discovered = external_sessions::discover(
                                    provider,
                                    &project,
                                    provider_data.as_deref(),
                                )
                                .unwrap_or_default();
                                let (project_sessions, _) = merge_external_sessions(
                                    native,
                                    discovered,
                                    &project_hidden.ids,
                                );
                                send_event(
                                    &events,
                                    Event::ProjectSessionsUpdated {
                                        project,
                                        sessions: project_sessions,
                                    },
                                );
                            }
                        }
                        Command::RemoveSession(id) => {
                            if !sessions.iter().any(|session| session.id == id) {
                                send_event(&events, Event::Error("unknown session".into()));
                                continue;
                            }
                            match hidden_sessions.hide(id.clone()) {
                                Ok(()) => {
                                    if pending_external
                                        .as_ref()
                                        .is_some_and(|session| session.choice_id() == id)
                                    {
                                        pending_external = None;
                                    }
                                    external_sessions.remove(&id);
                                    sessions.retain(|session| session.id != id);
                                    send_event(&events, Event::SessionsUpdated(sessions.clone()));
                                }
                                Err(error) => send_event(&events, Event::Error(error)),
                            }
                        }
                        Command::LoadSession(id) => {
                            if active.load(Ordering::Acquire) {
                                send_event(
                                    &events,
                                    Event::Error(
                                        "stop the active turn before loading a session".into(),
                                    ),
                                );
                                continue;
                            }
                            if external_sessions::is_external_choice(&id)
                                && let Some(external) = external_sessions.get(&id).cloned()
                            {
                                match load_external_session(&external, &id, &events) {
                                    Ok(()) => pending_external = Some(external),
                                    Err(error) => {
                                        send_event(&events, Event::SessionLoadFailed);
                                        send_event(&events, Event::Error(error));
                                        send_event(
                                            &events,
                                            Event::ConnectionChanged(ConnectionState::Ready),
                                        );
                                    }
                                }
                                continue;
                            }
                            let Some(session) = sessions.iter().find(|session| session.id == id)
                            else {
                                send_event(&events, Event::Error("unknown session".into()));
                                continue;
                            };
                            pending_external = None;
                            match load_session(
                                &connection,
                                &project_root,
                                session,
                                &events,
                                &session_notifications,
                                supports_close,
                            )
                            .await
                            {
                                Ok(loaded) => {
                                    additional_directories.clear();
                                    session_id = Some(loaded);
                                    enrich_native_session(&id, &external_sessions, &events);
                                }
                                Err(error) => {
                                    send_event(&events, Event::SessionLoadFailed);
                                    if session_not_found(&error) {
                                        sessions.retain(|session| session.id != id);
                                        send_event(
                                            &events,
                                            Event::SessionsUpdated(sessions.clone()),
                                        );
                                    } else {
                                        send_event(
                                            &events,
                                            Event::Error(acp_error(
                                                provider,
                                                "cannot load session",
                                                &error,
                                            )),
                                        );
                                    }
                                    send_event(
                                        &events,
                                        Event::ConnectionChanged(ConnectionState::Ready),
                                    );
                                }
                            }
                        }
                        Command::SetMode(mode_id) => {
                            let Some(session) = session_id.clone() else {
                                send_event(&events, Event::Error("no active session".into()));
                                continue;
                            };
                            match wait_for_acp(
                                connection
                                    .send_request(SetSessionModeRequest::new(
                                        session,
                                        mode_id.clone(),
                                    ))
                                    .block_task(),
                                "setting session mode",
                            )
                            .await
                            {
                                Ok(_) => send_event(&events, Event::ModeChanged(mode_id)),
                                Err(error) => send_event(
                                    &events,
                                    Event::Error(acp_error(
                                        provider,
                                        "cannot set session mode",
                                        &error,
                                    )),
                                ),
                            }
                        }
                        Command::SetConfig { id, value } => {
                            let Some(session) = session_id.clone() else {
                                send_event(&events, Event::Error("no active session".into()));
                                continue;
                            };
                            let value = match value {
                                ConfigValue::Select(value) => {
                                    SessionConfigOptionValue::value_id(value)
                                }
                                ConfigValue::Boolean(value) => {
                                    SessionConfigOptionValue::boolean(value)
                                }
                            };
                            let response = wait_for_acp(
                                connection
                                    .send_request(SetSessionConfigOptionRequest::new(
                                        session, id, value,
                                    ))
                                    .block_task(),
                                "setting session option",
                            )
                            .await;
                            match response {
                                Ok(response) => send_event(
                                    &events,
                                    Event::ConfigOptionsUpdated(normalize_config_options(
                                        &response.config_options,
                                    )),
                                ),
                                Err(error) => send_event(
                                    &events,
                                    Event::Error(acp_error(
                                        provider,
                                        "cannot set session option",
                                        &error,
                                    )),
                                ),
                            }
                        }
                        Command::SetRunEverything(enabled) => {
                            if extensions == ProviderExtensions::Cursor {
                                auto_approve_permissions.store(enabled, Ordering::Release);
                            }
                        }
                        Command::ControlGoal(action) => {
                            send_goal_control(
                                &connection,
                                &events,
                                session_id.clone(),
                                action,
                                &goal_actions,
                            )?;
                        }
                        Command::Prompt(text) => {
                            if active.load(Ordering::Acquire) && supports_steering {
                                send_steering(
                                    &connection,
                                    &events,
                                    session_id.clone(),
                                    text,
                                    Vec::new(),
                                    attachment_support,
                                )?;
                                continue;
                            }
                            turn_resume.attempts.store(0, Ordering::Release);
                            let visible_text = text.clone();
                            let (prompt_session, text, imported) =
                                if let Some(external) = pending_external.take() {
                                    match handoff_external_session(
                                        &connection,
                                        &project_root,
                                        &events,
                                        &mut editur_sessions,
                                        &mut sessions,
                                        &session_notifications,
                                        supports_close,
                                        &external,
                                        &text,
                                    )
                                    .await
                                    {
                                        Ok((session, text)) => (Some(session), text, true),
                                        Err(error) => {
                                            send_event(&events, Event::Error(error));
                                            send_event(
                                                &events,
                                                Event::TurnFinished { cancelled: false },
                                            );
                                            continue;
                                        }
                                    }
                                } else {
                                    (session_id.clone(), text, false)
                                };
                            session_id = prompt_session.clone();
                            if imported {
                                additional_directories.clear();
                            }
                            send_prompt(
                                &connection,
                                &events,
                                &active,
                                prompt_session,
                                text,
                                imported.then_some(visible_text),
                                Vec::new(),
                                attachment_support,
                                &claude_rate_limit_status,
                                turn_resume.clone(),
                            )?;
                        }
                        Command::PromptWithAttachments { text, attachments } => {
                            if active.load(Ordering::Acquire) && supports_steering {
                                if supports_additional_directories
                                    && attachments.iter().any(|attachment| {
                                        attachment.is_directory()
                                            && attachment.path() != project_root
                                            && !additional_directories
                                                .iter()
                                                .any(|path| path == attachment.path())
                                    })
                                {
                                    send_event(
                                        &events,
                                        Event::Error(
                                            "stop the active turn before adding a new workspace folder"
                                                .into(),
                                        ),
                                    );
                                    continue;
                                }
                                send_steering(
                                    &connection,
                                    &events,
                                    session_id.clone(),
                                    text,
                                    attachments,
                                    attachment_support,
                                )?;
                                continue;
                            }
                            turn_resume.attempts.store(0, Ordering::Release);
                            let visible_text = text.clone();
                            let (prompt_session, text, imported) =
                                if let Some(external) = pending_external.take() {
                                    match handoff_external_session(
                                        &connection,
                                        &project_root,
                                        &events,
                                        &mut editur_sessions,
                                        &mut sessions,
                                        &session_notifications,
                                        supports_close,
                                        &external,
                                        &text,
                                    )
                                    .await
                                    {
                                        Ok((session, text)) => (Some(session), text, true),
                                        Err(error) => {
                                            send_event(&events, Event::Error(error));
                                            send_event(
                                                &events,
                                                Event::TurnFinished { cancelled: false },
                                            );
                                            continue;
                                        }
                                    }
                                } else {
                                    (session_id.clone(), text, false)
                            };
                            session_id = prompt_session.clone();
                            if imported {
                                additional_directories.clear();
                            }
                            let requested_directories = attachments
                                .iter()
                                .filter(|attachment| {
                                    attachment.is_directory()
                                        && attachment.path() != project_root
                                        && !additional_directories
                                            .iter()
                                            .any(|path| path == attachment.path())
                                })
                                .map(|attachment| attachment.path().to_path_buf())
                                .collect::<Vec<_>>();
                            if supports_additional_directories
                                && !requested_directories.is_empty()
                            {
                                if !supports_resume {
                                    send_event(
                                        &events,
                                        Event::Error(
                                            "agent cannot add workspace folders to this session"
                                                .into(),
                                        ),
                                    );
                                    send_event(
                                        &events,
                                        Event::TurnFinished { cancelled: false },
                                    );
                                    continue;
                                }
                                let Some(prompt_session) = prompt_session.clone() else {
                                    send_event(
                                        &events,
                                        Event::Error("no active session".into()),
                                    );
                                    send_event(
                                        &events,
                                        Event::TurnFinished { cancelled: false },
                                    );
                                    continue;
                                };
                                let mut resumed_directories = additional_directories.clone();
                                resumed_directories.extend(requested_directories);
                                match wait_for_acp(
                                    connection
                                        .send_request(
                                        ResumeSessionRequest::new(prompt_session, &project_root)
                                            .additional_directories(resumed_directories.clone()),
                                        )
                                        .block_task(),
                                    "resuming session",
                                )
                                .await
                                {
                                    Ok(response) => {
                                        additional_directories = resumed_directories;
                                        let (current_mode, modes, config_options) =
                                            session_controls(
                                                response.modes.as_ref(),
                                                response.config_options.as_deref(),
                                            );
                                        if current_mode.is_some() || !config_options.is_empty() {
                                            send_event(
                                                &events,
                                                Event::SessionLoaded {
                                                    current_mode,
                                                    modes,
                                                    config_options,
                                                },
                                            );
                                        }
                                    }
                                    Err(error) => {
                                        send_event(
                                            &events,
                                            Event::Error(acp_error(
                                                provider,
                                                "cannot add workspace folders",
                                                &error,
                                            )),
                                        );
                                        send_event(
                                            &events,
                                            Event::TurnFinished { cancelled: false },
                                        );
                                        continue;
                                    }
                                }
                            }
                            send_prompt(
                                &connection,
                                &events,
                                &active,
                                prompt_session,
                                text,
                                imported.then_some(visible_text),
                                attachments,
                                attachment_support,
                                &claude_rate_limit_status,
                                turn_resume.clone(),
                            )?;
                        }
                        Command::HiddenPromptWithAttachments { text, attachments } => {
                            turn_resume.attempts.store(0, Ordering::Release);
                            send_prompt(
                                &connection,
                                &events,
                                &active,
                                session_id.clone(),
                                text,
                                Some(String::new()),
                                attachments,
                                attachment_support,
                                &claude_rate_limit_status,
                                turn_resume.clone(),
                            )?;
                        }
                        Command::ResumeTurn => {
                            send_prompt(
                                &connection,
                                &events,
                                &active,
                                session_id.clone(),
                                TURN_RESUME_PROMPT.into(),
                                None,
                                Vec::new(),
                                attachment_support,
                                &claude_rate_limit_status,
                                turn_resume.clone(),
                            )?;
                        }
                        Command::DecidePermission {
                            request_id,
                            option_id,
                        } => {
                            decide_permission(request_id, option_id, &permissions, &events);
                        }
                        Command::RespondInteraction {
                            request_id,
                            response,
                        } => {
                            if !respond_elicitation(
                                request_id,
                                response.clone(),
                                &elicitations,
                                &events,
                            ) {
                                respond_interaction(request_id, response, &interactions, &events);
                            }
                        }
                        Command::Cancel => {
                            if let Some(session) = session_id.clone()
                                && active.load(Ordering::Acquire)
                            {
                                let pending = permissions
                                    .lock()
                                    .expect("permission lock poisoned")
                                    .drain()
                                    .map(|(_, pending)| pending)
                                    .collect::<Vec<_>>();
                                for permission in pending {
                                    let _ =
                                        permission.decision_tx.try_send(PendingDecision::Cancelled);
                                }
                                for interaction in interactions
                                    .lock()
                                    .expect("interaction lock poisoned")
                                    .drain()
                                    .map(|(_, pending)| pending)
                                {
                                    let _ = interaction.response_tx.try_send(
                                        serde_json::json!({"outcome": {"outcome": "cancelled"}}),
                                    );
                                }
                                for elicitation in elicitations
                                    .lock()
                                    .expect("elicitation lock poisoned")
                                    .drain()
                                    .map(|(_, pending)| pending)
                                {
                                    let _ = elicitation
                                        .response_tx
                                        .try_send(ElicitationAction::Cancel);
                                }
                                connection.send_notification(CancelNotification::new(session))?;
                            }
                        }
                        Command::Shutdown => {
                            shutdown.store(true, Ordering::Release);
                            permissions
                                .lock()
                                .expect("permission lock poisoned")
                                .clear();
                            interactions
                                .lock()
                                .expect("interaction lock poisoned")
                                .clear();
                            elicitations
                                .lock()
                                .expect("elicitation lock poisoned")
                                .clear();
                            return Ok(());
                        }
                        Command::TransportFailed(message) => {
                            return Err(
                                agent_client_protocol::Error::invalid_request().data(message)
                            );
                        }
                        Command::TerminalAuthFinished(result) => {
                            if !terminal_auth_in_progress {
                                continue;
                            }
                            terminal_auth_in_progress = false;
                            if let Err(error) = result {
                                send_event(&events, Event::Error(error));
                                send_event(
                                    &events,
                                    Event::ConnectionChanged(
                                        ConnectionState::AuthenticationRequired(auth.clone()),
                                    ),
                                );
                                continue;
                            }
                            match start_session(
                                &connection,
                                &project_root,
                                &events,
                                supports_history,
                                &hidden_sessions.ids,
                                &mut editur_sessions,
                                preferred_session.as_deref(),
                                fresh_session,
                                &session_notifications,
                                supports_close,
                            )
                            .await
                            {
                                Ok((session, listed)) => {
                                    additional_directories.clear();
                                    let loaded_id = session.0.to_string();
                                    session_id = Some(session);
                                    let discovered = external_sessions::discover(
                                        provider,
                                        &project_root,
                                        provider_data.as_deref(),
                                    )
                                    .unwrap_or_default();
                                    (sessions, external_sessions) =
                                        merge_external_sessions(
                                            listed,
                                            discovered,
                                            &hidden_sessions.ids,
                                        );
                                    send_event(
                                        &events,
                                        Event::ProjectSessionsUpdated {
                                            project: project_root.clone(),
                                            sessions: sessions.clone(),
                                        },
                                    );
                                    enrich_native_session(
                                        &loaded_id,
                                        &external_sessions,
                                        &events,
                                    );
                                }
                                Err(error) => {
                                    send_event(
                                        &events,
                                        Event::Error(acp_error(
                                            provider,
                                            "cannot start session after authentication",
                                            &error,
                                        )),
                                    );
                                    send_event(
                                        &events,
                                        Event::ConnectionChanged(
                                            ConnectionState::AuthenticationRequired(auth.clone()),
                                        ),
                                    );
                                }
                            }
                        }
                    }
                }
                shutdown.store(true, Ordering::Release);
                Ok(())
            }
        })
        .await
}

#[derive(Clone)]
struct TurnResume {
    enabled: bool,
    commands: async_channel::Sender<Command>,
    attempts: Arc<AtomicU64>,
}

/// Cursor classifies upstream connection drops (for example
/// "RetriableError: [canceled] http/2 stream closed with error code CANCEL")
/// as retriable; progress up to the drop stays checkpointed in the session,
/// so the turn can be resumed with a follow-up prompt.
fn is_retriable_transport_error(error: &agent_client_protocol::Error) -> bool {
    let text = error.to_string();
    text.contains("RetriableError")
        || text.contains("http/2 stream closed")
        || text.contains("stream closed with error code CANCEL")
}

fn send_steering(
    connection: &ConnectionTo<Agent>,
    events: &EventSender,
    session: Option<SessionId>,
    text: String,
    attachments: Vec<PromptAttachment>,
    attachment_support: AttachmentSupport,
) -> agent_client_protocol::Result<()> {
    let Some(session) = session else {
        send_event(events, Event::Error("no active session".into()));
        return Ok(());
    };
    if text.trim().is_empty() && attachments.is_empty() {
        send_event(
            events,
            Event::Error("steering prompt cannot be empty".into()),
        );
        return Ok(());
    }
    let (content, displays) = match prompt_content(&text, &attachments, attachment_support) {
        Ok(content) => content,
        Err(error) => {
            send_event(events, Event::Error(error));
            return Ok(());
        }
    };
    let events = events.clone();
    connection
        .send_request(SessionExtensionRequest {
            method: "_session/steering",
            params: serde_json::json!({
                "sessionId": session.0,
                "prompt": content,
            }),
        })
        .on_receiving_result(async move |result| {
            match result {
                Ok(response)
                    if matches!(
                        response.get("outcome").and_then(serde_json::Value::as_str),
                        Some("injected" | "startedNewTurn")
                    ) =>
                {
                    send_event(&events, Event::UserMessage(text));
                    for content in displays {
                        send_event(
                            &events,
                            Event::ContentReceived {
                                role: ContentRole::User,
                                content,
                            },
                        );
                    }
                }
                Ok(_) => send_event(
                    &events,
                    Event::Error("agent could not apply steering".into()),
                ),
                Err(error) => send_event(
                    &events,
                    Event::Error(acp_error(
                        events.provider,
                        "cannot steer active turn",
                        &error,
                    )),
                ),
            }
            Ok(())
        })
}

fn send_goal_control(
    connection: &ConnectionTo<Agent>,
    events: &EventSender,
    session: Option<SessionId>,
    action: GoalAction,
    supported_actions: &[String],
) -> agent_client_protocol::Result<()> {
    let Some(session) = session else {
        send_event(events, Event::Error("no active session".into()));
        return Ok(());
    };
    let (action_name, objective) = match action {
        GoalAction::Set(objective) if !objective.trim().is_empty() => {
            ("set", Some(objective.trim().to_owned()))
        }
        GoalAction::Set(_) => {
            send_event(
                events,
                Event::Error("goal objective cannot be empty".into()),
            );
            return Ok(());
        }
        GoalAction::Pause => ("pause", None),
        GoalAction::Resume => ("resume", None),
        GoalAction::Clear => ("clear", None),
    };
    if !supported_actions.iter().any(|action| action == action_name) {
        send_event(
            events,
            Event::Error(format!("agent does not support goal action {action_name}")),
        );
        return Ok(());
    }
    let mut params = serde_json::json!({
        "sessionId": session.0,
        "action": action_name,
    });
    if let Some(objective) = objective {
        params["objective"] = objective.into();
    }
    let events = events.clone();
    connection
        .send_request(SessionExtensionRequest {
            method: "_session/goal",
            params,
        })
        .on_receiving_result(async move |result| {
            if let Err(error) = result {
                send_event(
                    &events,
                    Event::Error(acp_error(
                        events.provider,
                        "cannot update session goal",
                        &error,
                    )),
                );
            }
            Ok(())
        })
}

#[expect(clippy::too_many_arguments)]
fn send_prompt(
    connection: &ConnectionTo<Agent>,
    events: &EventSender,
    active: &Arc<AtomicBool>,
    session: Option<SessionId>,
    text: String,
    visible_text: Option<String>,
    attachments: Vec<PromptAttachment>,
    attachment_support: AttachmentSupport,
    claude_rate_limit_status: &Arc<Mutex<Option<String>>>,
    resume: TurnResume,
) -> agent_client_protocol::Result<()> {
    let Some(session) = session else {
        send_event(
            events,
            Event::Error("authenticate before sending a prompt".into()),
        );
        return Ok(());
    };
    if (text.trim().is_empty() && attachments.is_empty()) || active.swap(true, Ordering::AcqRel) {
        send_event(
            events,
            Event::Error("only one non-empty prompt can run at a time".into()),
        );
        return Ok(());
    }
    let (content, displays) = match prompt_content(&text, &attachments, attachment_support) {
        Ok(content) => content,
        Err(error) => {
            active.store(false, Ordering::Release);
            send_event(events, Event::Error(error));
            send_event(events, Event::TurnFinished { cancelled: false });
            return Ok(());
        }
    };
    send_event(events, Event::UserMessage(visible_text.unwrap_or(text)));
    for content in displays {
        send_event(
            events,
            Event::ContentReceived {
                role: ContentRole::User,
                content,
            },
        );
    }
    let events_for_result = events.clone();
    let active_for_result = Arc::clone(active);
    let rate_limit_for_result = Arc::clone(claude_rate_limit_status);
    connection
        .send_request(PromptRequest::new(session, content))
        .on_receiving_result(async move |result| {
            active_for_result.store(false, Ordering::Release);
            let cancelled = match result {
                Ok(response) => {
                    resume.attempts.store(0, Ordering::Release);
                    response.stop_reason == StopReason::Cancelled
                }
                Err(error) => {
                    let kind = classify_turn_failure(
                        events_for_result.provider,
                        &error,
                        rate_limit_for_result
                            .lock()
                            .ok()
                            .and_then(|status| status.clone())
                            .as_deref(),
                    );
                    let message =
                        acp_error(events_for_result.provider, "agent turn failed", &error);
                    let resuming = resume.enabled
                        && is_retriable_transport_error(&error)
                        && resume.attempts.fetch_add(1, Ordering::AcqRel) < MAX_TURN_RESUMES
                        && resume.commands.try_send(Command::ResumeTurn).is_ok();
                    send_event(
                        &events_for_result,
                        Event::TurnFailed {
                            kind,
                            message: if resuming {
                                format!(
                                    "{message}\n\nThe connection dropped mid-turn; progress is \
                                 preserved in this session — resuming automatically."
                                )
                            } else {
                                message
                            },
                        },
                    );
                    false
                }
            };
            send_event(&events_for_result, Event::TurnFinished { cancelled });
            Ok(())
        })
}

fn prompt_content(
    text: &str,
    attachments: &[PromptAttachment],
    support: AttachmentSupport,
) -> Result<(Vec<ContentBlock>, Vec<DisplayContent>), String> {
    if attachments.len() > MAX_PROMPT_ATTACHMENTS {
        return Err(format!("attach at most {MAX_PROMPT_ATTACHMENTS} items"));
    }
    let mut content = Vec::with_capacity(attachments.len() + usize::from(!text.trim().is_empty()));
    if !text.trim().is_empty() {
        content.push(ContentBlock::Text(TextContent::new(text)));
    }
    let mut displays = Vec::with_capacity(attachments.len());
    let mut total = 0_u64;
    for attachment in attachments {
        if attachment.kind == PromptAttachmentKind::Directory {
            if !attachment.path.is_dir() {
                return Err(format!(
                    "{} is no longer a folder",
                    attachment.path.display()
                ));
            }
            let uri = attachment_uri(&attachment.path);
            let name = attachment
                .path
                .file_name()
                .unwrap_or(attachment.path.as_os_str())
                .to_string_lossy()
                .into_owned();
            content.push(ContentBlock::ResourceLink(ResourceLink::new(
                name.clone(),
                uri.clone(),
            )));
            displays.push(DisplayContent::ResourceLink {
                name,
                title: None,
                uri,
                description: Some("Folder".into()),
                mime_type: None,
                size: None,
            });
            continue;
        }
        let bytes = attachment.read()?;
        total = total.saturating_add(bytes.len() as u64);
        if total > MAX_PROMPT_ATTACHMENT_TOTAL_BYTES {
            return Err(format!(
                "attached files must total no more than {} MiB",
                MAX_PROMPT_ATTACHMENT_TOTAL_BYTES / 1024 / 1024
            ));
        }
        let byte_len = i64::try_from(bytes.len()).ok();
        let uri = attachment_uri(&attachment.path);
        let name = attachment
            .path
            .file_name()
            .unwrap_or(attachment.path.as_os_str())
            .to_string_lossy()
            .into_owned();
        match attachment.kind {
            PromptAttachmentKind::Image(mime_type) if support.image => {
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                displays.push(DisplayContent::Image {
                    mime_type: mime_type.into(),
                    uri: Some(uri.clone()),
                    encoded_bytes: data.len(),
                    data: Some(bytes.into()),
                });
                content.push(ContentBlock::Image(
                    ImageContent::new(data, mime_type).uri(uri),
                ));
            }
            PromptAttachmentKind::Audio(mime_type) if support.audio => {
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                displays.push(DisplayContent::Audio {
                    mime_type: mime_type.into(),
                    encoded_bytes: data.len(),
                });
                content.push(ContentBlock::Audio(AudioContent::new(data, mime_type)));
            }
            PromptAttachmentKind::File if support.embedded_context => {
                let resource = if let Ok(text) = std::str::from_utf8(&bytes) {
                    EmbeddedResourceResource::TextResourceContents(
                        TextResourceContents::new(text, uri.clone()).mime_type("text/plain"),
                    )
                } else {
                    EmbeddedResourceResource::BlobResourceContents(
                        BlobResourceContents::new(
                            base64::engine::general_purpose::STANDARD.encode(bytes),
                            uri.clone(),
                        )
                        .mime_type("application/octet-stream"),
                    )
                };
                content.push(ContentBlock::Resource(EmbeddedResource::new(resource)));
                displays.push(DisplayContent::ResourceLink {
                    name,
                    title: None,
                    uri,
                    description: None,
                    mime_type: None,
                    size: byte_len,
                });
            }
            kind => {
                let mime_type = match kind {
                    PromptAttachmentKind::Image(mime_type)
                    | PromptAttachmentKind::Audio(mime_type) => Some(mime_type),
                    PromptAttachmentKind::File | PromptAttachmentKind::Directory => None,
                };
                let mut link = ResourceLink::new(name.clone(), uri.clone()).size(byte_len);
                if let Some(mime_type) = mime_type {
                    link = link.mime_type(mime_type);
                }
                content.push(ContentBlock::ResourceLink(link));
                displays.push(DisplayContent::ResourceLink {
                    name,
                    title: None,
                    uri,
                    description: None,
                    mime_type: mime_type.map(str::to_owned),
                    size: byte_len,
                });
            }
        }
    }
    Ok((content, displays))
}

fn attachment_uri(path: &Path) -> String {
    let path = path.to_string_lossy().replace('\\', "/");
    let mut uri = String::from(if cfg!(windows) { "file:///" } else { "file://" });
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b':' | b'-' | b'.' | b'_' | b'~') {
            uri.push(char::from(byte));
        } else {
            write!(uri, "%{byte:02X}").expect("writing to a string cannot fail");
        }
    }
    uri
}

fn merge_external_sessions(
    mut sessions: Vec<SessionChoice>,
    external: Vec<ExternalSession>,
    hidden_sessions: &HashSet<String>,
) -> (Vec<SessionChoice>, HashMap<String, ExternalSession>) {
    let native_ids = sessions
        .iter()
        .map(|session| session.id.clone())
        .collect::<HashSet<_>>();
    let mut external_by_choice = HashMap::new();
    for session in external {
        let choice_id = if native_ids.contains(&session.id) {
            session.id.clone()
        } else {
            let choice_id = session.choice_id();
            if hidden_sessions.contains(&choice_id) {
                continue;
            }
            sessions.push(SessionChoice {
                id: choice_id.clone(),
                title: session.title.clone(),
                updated_at: session.updated_at.clone(),
                started_in_editur: false,
            });
            choice_id
        };
        external_by_choice.insert(choice_id, session);
    }
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    sessions.truncate(MAX_CHOICES);
    external_by_choice.retain(|id, _| sessions.iter().any(|session| &session.id == id));
    (sessions, external_by_choice)
}

fn load_external_session(
    external: &ExternalSession,
    choice_id: &str,
    events: &EventSender,
) -> Result<(), String> {
    send_event(
        events,
        Event::SessionLoading {
            title: external.title.clone(),
        },
    );
    load_external_transcript(external, events)?;
    send_event(events, Event::ActiveSessionChanged(choice_id.to_owned()));
    send_event(events, Event::ConnectionChanged(ConnectionState::Ready));
    Ok(())
}

fn enrich_native_session(
    session_id: &str,
    external_sessions: &HashMap<String, ExternalSession>,
    events: &EventSender,
) {
    if let Some(external) = external_sessions.get(session_id)
        && let Err(error) = load_external_transcript(external, events)
    {
        send_event(events, Event::Error(error));
    }
}

fn load_external_transcript(
    external: &ExternalSession,
    events: &EventSender,
) -> Result<(), String> {
    let mut started = false;
    let count = external.visit_transcript(&mut |message| {
        if !started {
            send_event(events, Event::SessionTranscriptStarted);
            started = true;
        }
        send_event(
            events,
            Event::SessionTranscriptLoaded(vec![external_transcript_message(message)]),
        );
        Ok(())
    })?;
    if count == 0 {
        return Err("the external session transcript is empty".into());
    }
    send_event(events, Event::SessionTranscriptFinished);
    Ok(())
}

fn external_transcript_message(message: ExternalMessage) -> SessionTranscriptMessage {
    match message {
        ExternalMessage::User(text) => SessionTranscriptMessage::User(text),
        ExternalMessage::Assistant(text) => SessionTranscriptMessage::Assistant(text),
        ExternalMessage::Thought(text) => SessionTranscriptMessage::Thought(text),
        ExternalMessage::Image(image) => SessionTranscriptMessage::Content {
            role: ContentRole::User,
            content: DisplayContent::Image {
                mime_type: image.mime_type,
                uri: None,
                encoded_bytes: image.bytes.len().div_ceil(3) * 4,
                data: Some(image.bytes),
            },
        },
        ExternalMessage::Tool(tool) => SessionTranscriptMessage::Tool(external_tool_activity(tool)),
    }
}

fn external_tool_activity(tool: ExternalTool) -> ToolActivity {
    let task = external_subagent_task(&tool);
    let structured_task = task.is_some();
    let title = task
        .as_ref()
        .and_then(|task| match task {
            ToolOutput::Task { description, .. } => Some(format!("Subagent: {description}")),
            _ => None,
        })
        .unwrap_or_else(|| tool.name.clone());
    let mut content = tool
        .diffs
        .into_iter()
        .map(|diff| ToolOutput::Diff {
            path: diff.path,
            old_text: diff.old_text,
            new_text: diff.new_text,
        })
        .collect::<Vec<_>>();
    if let Some(task) = task {
        content.insert(0, task);
        if let Some(output) = external_subagent_output(tool.output.as_deref()) {
            content.push(ToolOutput::Text(output));
        }
    }
    let detail = (tool.input.is_some() || tool.output.is_some() || !content.is_empty()).then_some(
        ToolDetail {
            input: (!structured_task).then_some(tool.input).flatten(),
            content,
            output: (!structured_task).then_some(tool.output).flatten(),
        },
    );
    ToolActivity {
        id: tool.id,
        title: Some(title),
        status: tool
            .status
            .map(|status| match status.to_ascii_lowercase().as_str() {
                "pending" => "Pending".into(),
                "running" | "inprogress" | "in_progress" => "InProgress".into(),
                "error" | "failed" => "Failed".into(),
                "cancelled" | "canceled" => "Cancelled".into(),
                _ => "Completed".into(),
            }),
        kind: tool.kind,
        paths: tool.paths.into_iter().map(Into::into).collect(),
        detail,
    }
}

fn external_subagent_task(tool: &ExternalTool) -> Option<ToolOutput> {
    if tool.kind.as_deref() != Some("Task") {
        return None;
    }
    let input = tool.input.as_deref().and_then(parse_tool_input);
    let input_object = input.as_ref().and_then(serde_json::Value::as_object);
    let output = tool.output.as_deref().and_then(parse_tool_input);
    let output_object = output.as_ref().and_then(serde_json::Value::as_object);
    let input_string = |keys: &[&str]| {
        input_object.and_then(|input| {
            keys.iter()
                .find_map(|key| input.get(*key).and_then(serde_json::Value::as_str))
        })
    };
    let output_string = |keys: &[&str]| {
        output_object.and_then(|output| {
            keys.iter()
                .find_map(|key| output.get(*key).and_then(serde_json::Value::as_str))
        })
    };
    let prompt = input_string(&["prompt", "message", "task"])
        .or_else(|| input.as_ref().and_then(serde_json::Value::as_str))
        .unwrap_or_default()
        .to_owned();
    let agent_id = input_string(&["agent_id", "agentId"])
        .or_else(|| output_string(&["agent_id", "agentId"]))
        .or_else(|| {
            input_object
                .and_then(|input| input.get("receiverThreadIds"))
                .and_then(serde_json::Value::as_array)
                .and_then(|ids| ids.first())
                .and_then(serde_json::Value::as_str)
        })
        .map(str::to_owned);
    let duration_ms = input_object
        .and_then(|input| input.get("duration_ms").or_else(|| input.get("durationMs")))
        .and_then(serde_json::Value::as_u64);
    let duration_ms = duration_ms.or_else(|| {
        output_object
            .and_then(|output| {
                output
                    .get("duration_ms")
                    .or_else(|| output.get("durationMs"))
            })
            .and_then(serde_json::Value::as_u64)
    });
    Some(ToolOutput::Task {
        description: input_string(&["description"])
            .unwrap_or(&tool.name)
            .to_owned(),
        prompt,
        subagent_type: input_string(&["subagent_type", "subagentType"])
            .unwrap_or(&tool.name)
            .to_owned(),
        model: input_string(&["model"]).map(str::to_owned),
        agent_id,
        agents: Vec::new(),
        path: None,
        activity: None,
        duration_ms,
    })
}

fn external_subagent_output(output: Option<&str>) -> Option<String> {
    let output = parse_tool_input(output?)?;
    subagent_output_text(Some(&output))
}

#[expect(clippy::too_many_arguments)]
async fn handoff_external_session(
    connection: &ConnectionTo<Agent>,
    project_root: &Path,
    events: &EventSender,
    editur_sessions: &mut EditurSessions,
    sessions: &mut Vec<SessionChoice>,
    session_notifications: &SessionNotificationGate,
    supports_close: bool,
    external: &ExternalSession,
    next_message: &str,
) -> Result<(SessionId, String), String> {
    let prompt = external.handoff_prompt(next_message)?;
    let response = wait_for_acp(
        connection
            .send_request(NewSessionRequest::new(project_root))
            .block_task(),
        "starting imported session",
    )
    .await
    .map_err(|error| acp_error(events.provider, "cannot continue imported session", &error))?;
    let session_id = response.session_id;
    let previous = session_notifications.activate(&session_id);
    close_replaced_session(connection, previous, &session_id, supports_close, events).await;
    if let Err(error) = editur_sessions.remember(session_id.0.as_ref()) {
        send_event(events, Event::Error(error));
    }
    let (current_mode, modes, config_options) =
        session_controls(response.modes.as_ref(), response.config_options.as_deref());
    sessions.retain(|session| session.id != external.choice_id());
    sessions.insert(
        0,
        SessionChoice {
            id: session_id.0.to_string(),
            title: external.title.clone(),
            updated_at: external.updated_at.clone(),
            started_in_editur: true,
        },
    );
    sessions.truncate(MAX_CHOICES);
    send_event(
        events,
        Event::SessionLoaded {
            current_mode,
            modes,
            config_options,
        },
    );
    send_event(events, Event::SessionsUpdated(sessions.clone()));
    send_event(
        events,
        Event::ActiveSessionChanged(session_id.0.to_string()),
    );
    send_event(events, Event::ConnectionChanged(ConnectionState::Ready));
    Ok((session_id, prompt))
}

async fn new_session(
    connection: &ConnectionTo<Agent>,
    project_root: &std::path::Path,
    events: &EventSender,
    editur_sessions: &mut EditurSessions,
    session_notifications: &SessionNotificationGate,
    supports_close: bool,
) -> agent_client_protocol::Result<SessionId> {
    let response = wait_for_acp(
        connection
            .send_request(NewSessionRequest::new(project_root))
            .block_task(),
        "starting session",
    )
    .await?;
    let session_id = response.session_id;
    let previous = session_notifications.activate(&session_id);
    close_replaced_session(connection, previous, &session_id, supports_close, events).await;
    if let Err(error) = editur_sessions.remember(session_id.0.as_ref()) {
        send_event(events, Event::Error(error));
    }
    let (current_mode, modes, config_options) =
        session_controls(response.modes.as_ref(), response.config_options.as_deref());
    send_event(
        events,
        Event::SessionReady {
            current_mode,
            modes,
            config_options,
        },
    );
    send_event(
        events,
        Event::ActiveSessionChanged(session_id.0.to_string()),
    );
    send_event(events, Event::ConnectionChanged(ConnectionState::Ready));
    Ok(session_id)
}

#[expect(clippy::too_many_arguments)]
async fn start_session(
    connection: &ConnectionTo<Agent>,
    project_root: &std::path::Path,
    events: &EventSender,
    supports_history: bool,
    hidden_sessions: &HashSet<String>,
    editur_sessions: &mut EditurSessions,
    preferred_session: Option<&str>,
    fresh_session: bool,
    session_notifications: &SessionNotificationGate,
    supports_close: bool,
) -> agent_client_protocol::Result<(SessionId, Vec<SessionChoice>)> {
    let sessions = if supports_history {
        list_sessions(
            connection,
            project_root,
            events,
            hidden_sessions,
            editur_sessions,
        )
        .await
        .unwrap_or_default()
    } else {
        Vec::new()
    };
    let restore = if fresh_session {
        None
    } else {
        preferred_session
            .and_then(|id| sessions.iter().find(|session| session.id == id))
            .or_else(|| sessions.first())
    };
    if let Some(session) = restore
        && let Ok(session_id) = load_session(
            connection,
            project_root,
            session,
            events,
            session_notifications,
            supports_close,
        )
        .await
    {
        return Ok((session_id, sessions));
    }
    let session_id = new_session(
        connection,
        project_root,
        events,
        editur_sessions,
        session_notifications,
        supports_close,
    )
    .await?;
    let mut sessions = sessions;
    sessions.insert(0, untitled_session(&session_id));
    send_event(events, Event::SessionsUpdated(sessions.clone()));
    Ok((session_id, sessions))
}

fn untitled_session(session_id: &SessionId) -> SessionChoice {
    SessionChoice {
        id: session_id.0.to_string(),
        title: None,
        updated_at: None,
        started_in_editur: true,
    }
}

fn session_not_found(error: &agent_client_protocol::Error) -> bool {
    error.code == ErrorCode::ResourceNotFound
        || (error.code == ErrorCode::InvalidParams
            && error.data.as_ref().is_some_and(|data| {
                data.get("message")
                    .and_then(serde_json::Value::as_str)
                    .or_else(|| data.as_str())
                    .is_some_and(|message| message.to_ascii_lowercase().contains("not found"))
            }))
}

async fn list_sessions(
    connection: &ConnectionTo<Agent>,
    project_root: &std::path::Path,
    events: &EventSender,
    hidden_sessions: &HashSet<String>,
    editur_sessions: &EditurSessions,
) -> agent_client_protocol::Result<Vec<SessionChoice>> {
    let sessions =
        query_sessions(connection, project_root, hidden_sessions, editur_sessions).await?;
    send_event(events, Event::SessionsUpdated(sessions.clone()));
    Ok(sessions)
}

async fn query_sessions(
    connection: &ConnectionTo<Agent>,
    project_root: &std::path::Path,
    hidden_sessions: &HashSet<String>,
    editur_sessions: &EditurSessions,
) -> agent_client_protocol::Result<Vec<SessionChoice>> {
    let mut sessions = Vec::new();
    let mut cursor = None;
    let mut seen_cursors = HashSet::new();
    while sessions.len() < MAX_CHOICES {
        let response = wait_for_acp(
            connection
                .send_request(
                    ListSessionsRequest::new()
                        .cwd(project_root)
                        .cursor(cursor.clone()),
                )
                .block_task(),
            "listing sessions",
        )
        .await?;
        sessions.extend(
            response
                .sessions
                .into_iter()
                .filter(|session| {
                    session.cwd == project_root
                        && !hidden_sessions.contains(session.session_id.0.as_ref())
                })
                .map(|session| SessionChoice {
                    id: session.session_id.0.to_string(),
                    title: session.title,
                    updated_at: session.updated_at,
                    started_in_editur: editur_sessions.contains(session.session_id.0.as_ref()),
                }),
        );
        let Some(next_cursor) = response.next_cursor else {
            break;
        };
        if !seen_cursors.insert(next_cursor.clone()) {
            break;
        }
        cursor = Some(next_cursor);
    }
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    sessions.truncate(MAX_CHOICES);
    Ok(sessions)
}

struct HiddenSessions {
    path: Option<PathBuf>,
    ids: HashSet<String>,
}

#[derive(Deserialize, Serialize)]
struct HiddenSessionFile {
    version: u8,
    ids: Vec<String>,
}

impl HiddenSessions {
    fn load(path: Option<PathBuf>) -> Self {
        let ids = load_bounded_json(path.as_deref())
            .and_then(|bytes| serde_json::from_slice::<HiddenSessionFile>(&bytes).ok())
            .filter(|history| history.version == 2)
            .map(|history| {
                history
                    .ids
                    .into_iter()
                    .take(MAX_STORED_SESSION_IDS)
                    .collect()
            })
            .unwrap_or_default();
        Self { path, ids }
    }

    fn hide(&mut self, id: String) -> Result<(), String> {
        if self.ids.len() >= MAX_STORED_SESSION_IDS && !self.ids.contains(&id) {
            return Err("too many sessions have been removed from history".into());
        }
        let Some(path) = &self.path else {
            return Err("cannot determine where to save session history".into());
        };
        if !self.ids.insert(id.clone()) {
            return Ok(());
        }
        let mut ids = self.ids.iter().cloned().collect::<Vec<_>>();
        ids.sort_unstable();
        let bytes = serde_json::to_vec(&HiddenSessionFile { version: 2, ids })
            .map_err(|error| format!("cannot encode session history: {error}"))?;
        if let Err(error) = save_session_metadata(path, &bytes) {
            self.ids.remove(&id);
            return Err(error);
        }
        Ok(())
    }
}

struct EditurSessions {
    path: Option<PathBuf>,
    ids: HashSet<String>,
}

impl EditurSessions {
    fn load(path: Option<PathBuf>) -> Self {
        let ids = load_session_ids(path.as_deref());
        Self { path, ids }
    }

    fn contains(&self, id: &str) -> bool {
        self.ids.contains(id)
    }

    fn remember(&mut self, id: &str) -> Result<(), String> {
        if self.ids.len() >= MAX_STORED_SESSION_IDS && !self.ids.contains(id) {
            return Err("too many Editur sessions have been recorded".into());
        }
        let Some(path) = &self.path else {
            return Err("cannot determine where to save Editur session origins".into());
        };
        if !self.ids.insert(id.to_owned()) {
            return Ok(());
        }
        if let Err(error) = save_session_ids(path, &self.ids) {
            self.ids.remove(id);
            return Err(error);
        }
        Ok(())
    }
}

fn load_session_ids(path: Option<&Path>) -> HashSet<String> {
    load_bounded_json(path)
        .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
        .unwrap_or_default()
        .into_iter()
        .take(MAX_STORED_SESSION_IDS)
        .collect()
}

fn load_bounded_json(path: Option<&Path>) -> Option<Vec<u8>> {
    path.and_then(|path| {
        fs::symlink_metadata(path)
            .ok()
            .filter(|metadata| {
                metadata.is_file()
                    && !metadata.file_type().is_symlink()
                    && metadata.len() <= MAX_DETAIL_BYTES as u64
            })
            .and_then(|_| fs::read(path).ok())
    })
}

fn session_history_path(
    account: &ProviderAccount,
    project_root: &std::path::Path,
) -> Option<PathBuf> {
    crate::data_dir()
        .ok()
        .map(|directory| session_history_path_in(&directory, account, project_root))
}

fn managed_session_startup(
    account: &ProviderAccount,
    project_root: &Path,
    preferred_session: Option<String>,
    fresh_session: bool,
) -> SessionStartup {
    let history = session_history_path(account, project_root);
    let active_session = crate::data_dir()
        .ok()
        .map(|directory| active_session_path_in(&directory, account, project_root));
    let editur_sessions = crate::data_dir()
        .ok()
        .map(|directory| editur_sessions_path_in(&directory, account, project_root));
    let preferred_session = if fresh_session {
        None
    } else {
        preferred_session.or_else(|| active_session.as_deref().and_then(load_active_session))
    };
    SessionStartup {
        history,
        active_session,
        editur_sessions,
        preferred_session,
        fresh_session,
        account: Some(account.clone()),
    }
}

fn session_history_path_in(
    data_dir: &Path,
    account: &ProviderAccount,
    project_root: &Path,
) -> PathBuf {
    let digest = Sha256::digest(project_root.as_os_str().as_encoded_bytes());
    let mut name = String::with_capacity(digest.len() * 2 + 5);
    for byte in digest {
        write!(&mut name, "{byte:02x}").expect("writing to a String cannot fail");
    }
    name.push_str(".json");
    let destination = super::provision::account_root(data_dir, account.key)
        .expect("validated ACP account id")
        .join("session-history")
        .join(&name);
    if account.auth_source == AuthSource::Legacy && !destination.exists() {
        let provider_legacy = super::provision::provider_root(data_dir, account.key.provider)
            .join("session-history")
            .join(&name);
        if account.key.provider == ProviderId::Cursor && !provider_legacy.exists() {
            migrate_small_file(
                &data_dir.join("agents/session-history").join(&name),
                &provider_legacy,
            );
        }
        migrate_small_file(&provider_legacy, &destination);
    }
    destination
}

fn migrate_small_file(legacy: &Path, destination: &Path) {
    if destination.exists() {
        return;
    }
    let migratable = fs::symlink_metadata(legacy).ok().is_some_and(|metadata| {
        metadata.is_file()
            && !metadata.file_type().is_symlink()
            && metadata.len() <= MAX_DETAIL_BYTES as u64
    });
    if migratable
        && destination
            .parent()
            .is_some_and(|parent| fs::create_dir_all(parent).is_ok())
    {
        let _ = fs::rename(legacy, destination);
    }
}

fn active_session_path_in(
    data_dir: &Path,
    account: &ProviderAccount,
    project_root: &Path,
) -> PathBuf {
    let history = session_history_path_in(data_dir, account, project_root);
    let destination = super::provision::account_root(data_dir, account.key)
        .expect("validated ACP account id")
        .join("active-session")
        .join(
            history
                .file_name()
                .expect("session history has a file name"),
        );
    if account.auth_source == AuthSource::Legacy && !destination.exists() {
        migrate_small_file(
            &super::provision::provider_root(data_dir, account.key.provider)
                .join("active-session")
                .join(history.file_name().unwrap()),
            &destination,
        );
    }
    destination
}

fn editur_sessions_path_in(
    data_dir: &Path,
    account: &ProviderAccount,
    project_root: &Path,
) -> PathBuf {
    let history = session_history_path_in(data_dir, account, project_root);
    let destination = super::provision::account_root(data_dir, account.key)
        .expect("validated ACP account id")
        .join("editur-sessions")
        .join(
            history
                .file_name()
                .expect("session history has a file name"),
        );
    if account.auth_source == AuthSource::Legacy && !destination.exists() {
        migrate_small_file(
            &super::provision::provider_root(data_dir, account.key.provider)
                .join("editur-sessions")
                .join(history.file_name().unwrap()),
            &destination,
        );
    }
    destination
}

fn load_active_session(path: &Path) -> Option<String> {
    load_session_ids(Some(path)).into_iter().next()
}

fn save_active_session(path: &Path, id: &str) -> Result<(), String> {
    save_session_ids(path, &HashSet::from([id.to_owned()]))
}

fn save_session_ids(path: &std::path::Path, ids: &HashSet<String>) -> Result<(), String> {
    let mut ids = ids.iter().collect::<Vec<_>>();
    ids.sort_unstable();
    let bytes = serde_json::to_vec(&ids)
        .map_err(|error| format!("cannot encode session history: {error}"))?;
    save_session_metadata(path, &bytes)
}

fn save_session_metadata(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_DETAIL_BYTES {
        return Err("too many session identifiers to save".into());
    }
    let parent = path
        .parent()
        .ok_or_else(|| "session metadata path has no parent directory".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create session metadata directory: {error}"))?;
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("cannot stage session history: {error}"))?;
    staged
        .write_all(bytes)
        .and_then(|()| staged.flush())
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|error| format!("cannot write session metadata: {error}"))?;
    staged
        .persist(path)
        .map_err(|error| format!("cannot save session metadata: {}", error.error))?;
    Ok(())
}

async fn load_session(
    connection: &ConnectionTo<Agent>,
    project_root: &std::path::Path,
    session: &SessionChoice,
    events: &EventSender,
    session_notifications: &SessionNotificationGate,
    supports_close: bool,
) -> agent_client_protocol::Result<SessionId> {
    send_event(
        events,
        Event::SessionLoading {
            title: session.title.clone(),
        },
    );
    let session_id = SessionId::new(session.id.clone());
    let previous = session_notifications.activate(&session_id);
    let response = match wait_for_acp(
        connection
            .send_request(LoadSessionRequest::new(session_id.clone(), project_root))
            .block_task(),
        "loading session",
    )
    .await
    {
        Ok(response) => response,
        Err(error) => {
            session_notifications.restore(previous);
            return Err(error);
        }
    };
    close_replaced_session(connection, previous, &session_id, supports_close, events).await;
    let (current_mode, modes, config_options) =
        session_controls(response.modes.as_ref(), response.config_options.as_deref());
    send_event(
        events,
        Event::SessionLoaded {
            current_mode,
            modes,
            config_options,
        },
    );
    send_event(
        events,
        Event::ActiveSessionChanged(session_id.0.to_string()),
    );
    send_event(events, Event::ConnectionChanged(ConnectionState::Ready));
    Ok(session_id)
}

async fn close_replaced_session(
    connection: &ConnectionTo<Agent>,
    previous: Option<String>,
    current: &SessionId,
    supports_close: bool,
    events: &EventSender,
) {
    let Some(previous) = previous.filter(|previous| previous != current.0.as_ref()) else {
        return;
    };
    if !supports_close {
        return;
    }
    if let Err(error) = wait_for_acp(
        connection
            .send_request(CloseSessionRequest::new(previous))
            .block_task(),
        "closing previous session",
    )
    .await
    {
        send_event(
            events,
            Event::Error(acp_error(
                events.provider,
                "cannot close previous session",
                &error,
            )),
        );
    }
}

fn session_controls(
    modes: Option<&SessionModeState>,
    config_options: Option<&[SessionConfigOption]>,
) -> (Option<String>, Vec<ModeChoice>, Vec<ConfigChoice>) {
    (
        modes.map(|modes| modes.current_mode_id.0.to_string()),
        modes.map_or_else(Vec::new, |modes| {
            modes
                .available_modes
                .iter()
                .take(MAX_CHOICES)
                .map(|mode| ModeChoice {
                    id: mode.id.0.to_string(),
                    name: mode.name.clone(),
                    description: mode.description.clone(),
                })
                .collect()
        }),
        config_options.map_or_else(Vec::new, normalize_config_options),
    )
}

struct PendingPermission {
    allowed: HashSet<String>,
    decision_tx: async_channel::Sender<PendingDecision>,
}

enum PendingDecision {
    Selected(String),
    Cancelled,
}

#[derive(Clone, Debug)]
struct CursorRequest {
    method: String,
    params: serde_json::Value,
}

impl agent_client_protocol::JsonRpcMessage for CursorRequest {
    fn matches_method(method: &str) -> bool {
        matches!(method, "cursor/ask_question" | "cursor/create_plan")
    }

    fn method(&self) -> &str {
        &self.method
    }

    fn to_untyped_message(
        &self,
    ) -> Result<agent_client_protocol::UntypedMessage, agent_client_protocol::Error> {
        agent_client_protocol::UntypedMessage::new(&self.method, &self.params)
    }

    fn parse_message(
        method: &str,
        params: &impl serde::Serialize,
    ) -> Result<Self, agent_client_protocol::Error> {
        Ok(Self {
            method: method.into(),
            params: serde_json::to_value(params)?,
        })
    }
}

impl agent_client_protocol::JsonRpcRequest for CursorRequest {
    type Response = serde_json::Value;
}

#[derive(Clone, Debug)]
struct SessionExtensionRequest {
    method: &'static str,
    params: serde_json::Value,
}

impl agent_client_protocol::JsonRpcMessage for SessionExtensionRequest {
    fn matches_method(method: &str) -> bool {
        matches!(method, "_session/steering" | "_session/goal")
    }

    fn method(&self) -> &str {
        self.method
    }

    fn to_untyped_message(
        &self,
    ) -> Result<agent_client_protocol::UntypedMessage, agent_client_protocol::Error> {
        agent_client_protocol::UntypedMessage::new(self.method, &self.params)
    }

    fn parse_message(
        method: &str,
        params: &impl serde::Serialize,
    ) -> Result<Self, agent_client_protocol::Error> {
        let method = match method {
            "_session/steering" => "_session/steering",
            "_session/goal" => "_session/goal",
            _ => return Err(agent_client_protocol::Error::method_not_found()),
        };
        Ok(Self {
            method,
            params: serde_json::to_value(params)?,
        })
    }
}

impl agent_client_protocol::JsonRpcRequest for SessionExtensionRequest {
    type Response = serde_json::Value;
}

struct PendingInteraction {
    kind: PendingInteractionKind,
    response_tx: async_channel::Sender<serde_json::Value>,
}

enum PendingInteractionKind {
    Questions(HashMap<String, (HashSet<String>, bool)>),
    Plan,
}

struct PendingElicitation {
    kind: PendingElicitationKind,
    response_tx: async_channel::Sender<ElicitationAction>,
}

enum PendingElicitationKind {
    Form {
        fields: HashMap<String, ElicitationPropertySchema>,
        required: HashSet<String>,
    },
    Url,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorQuestionRequest {
    tool_call_id: String,
    #[serde(default)]
    title: Option<String>,
    questions: Vec<CursorQuestion>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorQuestion {
    id: String,
    prompt: String,
    options: Vec<CursorQuestionOption>,
    #[serde(default)]
    allow_multiple: bool,
}

#[derive(Deserialize)]
struct CursorQuestionOption {
    id: String,
    label: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorPlanRequest {
    tool_call_id: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    overview: Option<String>,
    #[serde(default)]
    plan: String,
    #[serde(default)]
    todos: Vec<CursorPlanItem>,
    #[serde(default)]
    is_project: Option<bool>,
    #[serde(default)]
    phases: Vec<CursorPlanPhase>,
}

#[derive(Deserialize)]
struct CursorPlanItem {
    #[serde(default)]
    id: String,
    content: String,
    status: String,
}

#[derive(Deserialize)]
struct CursorPlanPhase {
    name: String,
    todos: Vec<CursorPlanItem>,
}

fn parse_cursor_interaction(
    request_id: u64,
    request: CursorRequest,
) -> Result<(InteractionRequest, PendingInteractionKind), String> {
    match request.method.as_str() {
        "cursor/ask_question" => {
            let request: CursorQuestionRequest = serde_json::from_value(request.params)
                .map_err(|error| format!("invalid cursor/ask_question payload: {error}"))?;
            if request.questions.is_empty() || request.questions.len() > MAX_CHOICES {
                return Err(format!(
                    "agent supplied {} questions; expected 1..={MAX_CHOICES}",
                    request.questions.len()
                ));
            }
            let mut allowed = HashMap::new();
            let mut questions = Vec::with_capacity(request.questions.len());
            for question in request.questions {
                if question.options.is_empty() || question.options.len() > MAX_CHOICES {
                    return Err(format!(
                        "agent supplied {} choices for question {}; expected 1..={MAX_CHOICES}",
                        question.options.len(),
                        question.id
                    ));
                }
                let choices = question
                    .options
                    .iter()
                    .map(|option| option.id.clone())
                    .collect::<HashSet<_>>();
                if choices.len() != question.options.len()
                    || allowed
                        .insert(question.id.clone(), (choices, question.allow_multiple))
                        .is_some()
                {
                    return Err("agent supplied duplicate question or option identifiers".into());
                }
                questions.push(Question {
                    id: question.id,
                    prompt: question.prompt,
                    options: question
                        .options
                        .into_iter()
                        .map(|option| QuestionOption {
                            id: option.id,
                            label: option.label,
                        })
                        .collect(),
                    allow_multiple: question.allow_multiple,
                    required: true,
                    secret: false,
                    default_values: Vec::new(),
                    value_kind: QuestionValueKind::String,
                });
            }
            Ok((
                InteractionRequest {
                    request_id,
                    tool_call_id: request.tool_call_id,
                    kind: InteractionKind::Questions {
                        title: request.title.unwrap_or_else(|| "Questions".into()),
                        questions,
                    },
                },
                PendingInteractionKind::Questions(allowed),
            ))
        }
        "cursor/create_plan" => {
            let request: CursorPlanRequest = serde_json::from_value(request.params)
                .map_err(|error| format!("invalid cursor/create_plan payload: {error}"))?;
            let plan_item = |item: CursorPlanItem| PlanItem {
                content: if item.id.is_empty() {
                    item.content
                } else {
                    format!("{}: {}", item.id, item.content)
                },
                status: item.status,
            };
            let proposal = PlanProposal {
                name: request.name,
                overview: request.overview,
                plan: request.plan,
                todos: request
                    .todos
                    .into_iter()
                    .take(MAX_PLAN_ITEMS)
                    .map(plan_item)
                    .collect(),
                is_project: request.is_project,
                phases: request
                    .phases
                    .into_iter()
                    .take(MAX_PLAN_ITEMS)
                    .map(|phase| PlanPhase {
                        name: phase.name,
                        todos: phase
                            .todos
                            .into_iter()
                            .take(MAX_PLAN_ITEMS)
                            .map(plan_item)
                            .collect(),
                    })
                    .collect(),
            };
            Ok((
                InteractionRequest {
                    request_id,
                    tool_call_id: request.tool_call_id,
                    kind: InteractionKind::Plan(proposal),
                },
                PendingInteractionKind::Plan,
            ))
        }
        _ => Err("unsupported Cursor interaction".into()),
    }
}

fn parse_elicitation(
    request_id: u64,
    request: CreateElicitationRequest,
) -> Result<(InteractionRequest, PendingElicitationKind), String> {
    let tool_call_id = match request.mode.scope() {
        ElicitationScope::Session(scope) => scope
            .tool_call_id
            .as_ref()
            .map_or_else(|| "elicitation".into(), |id| id.0.to_string()),
        ElicitationScope::Request(_) => "elicitation".into(),
        _ => "elicitation".into(),
    };
    let form = match request.mode {
        ElicitationMode::Form(form) => form,
        ElicitationMode::Url(url) => {
            return Ok((
                InteractionRequest {
                    request_id,
                    tool_call_id,
                    kind: InteractionKind::Url {
                        title: request.message,
                        url: url.url,
                    },
                },
                PendingElicitationKind::Url,
            ));
        }
        _ => return Err("unsupported ACP elicitation mode".into()),
    };
    if form.requested_schema.properties.is_empty()
        || form.requested_schema.properties.len() > MAX_CHOICES
    {
        return Err(format!(
            "agent supplied {} elicitation fields; expected 1..={MAX_CHOICES}",
            form.requested_schema.properties.len()
        ));
    }
    let required = form
        .requested_schema
        .required
        .unwrap_or_default()
        .into_iter()
        .collect::<HashSet<_>>();
    if required
        .iter()
        .any(|field| !form.requested_schema.properties.contains_key(field))
    {
        return Err("agent marked an unknown elicitation field as required".into());
    }
    let mut fields = HashMap::new();
    let mut questions = Vec::with_capacity(form.requested_schema.properties.len());
    for (id, field) in form.requested_schema.properties {
        let (title, description, options, allow_multiple, secret, default_values, value_kind) =
            match &field {
                ElicitationPropertySchema::String(schema) => {
                    if let Some(pattern) = &schema.pattern
                        && (pattern.len() > MAX_DETAIL_BYTES || regex::Regex::new(pattern).is_err())
                    {
                        return Err(format!(
                            "agent supplied an invalid pattern for elicitation field {id}"
                        ));
                    }
                    let options = schema
                        .one_of
                        .as_ref()
                        .map(|options| {
                            options
                                .iter()
                                .map(|option| QuestionOption {
                                    id: option.value.clone(),
                                    label: option.title.clone(),
                                })
                                .collect()
                        })
                        .or_else(|| {
                            schema.enum_values.as_ref().map(|options| {
                                options
                                    .iter()
                                    .map(|option| QuestionOption {
                                        id: option.clone(),
                                        label: option.clone(),
                                    })
                                    .collect()
                            })
                        })
                        .unwrap_or_default();
                    (
                        schema.title.as_deref(),
                        schema.description.as_deref(),
                        options,
                        false,
                        elicitation_secret(schema.meta.as_ref()),
                        schema.default.iter().cloned().collect(),
                        QuestionValueKind::String,
                    )
                }
                ElicitationPropertySchema::Number(schema) => (
                    schema.title.as_deref(),
                    schema.description.as_deref(),
                    Vec::new(),
                    false,
                    false,
                    schema
                        .default
                        .map(|value| value.to_string())
                        .into_iter()
                        .collect(),
                    QuestionValueKind::Number,
                ),
                ElicitationPropertySchema::Integer(schema) => (
                    schema.title.as_deref(),
                    schema.description.as_deref(),
                    Vec::new(),
                    false,
                    false,
                    schema
                        .default
                        .map(|value| value.to_string())
                        .into_iter()
                        .collect(),
                    QuestionValueKind::Integer,
                ),
                ElicitationPropertySchema::Boolean(schema) => (
                    schema.title.as_deref(),
                    schema.description.as_deref(),
                    vec![
                        QuestionOption {
                            id: "true".into(),
                            label: "Yes".into(),
                        },
                        QuestionOption {
                            id: "false".into(),
                            label: "No".into(),
                        },
                    ],
                    false,
                    false,
                    schema
                        .default
                        .map(|value| value.to_string())
                        .into_iter()
                        .collect(),
                    QuestionValueKind::Boolean,
                ),
                ElicitationPropertySchema::Array(schema) => {
                    let options = match &schema.items {
                        MultiSelectItems::String(items) => items
                            .values
                            .iter()
                            .map(|value| QuestionOption {
                                id: value.clone(),
                                label: value.clone(),
                            })
                            .collect(),
                        MultiSelectItems::Titled(items) => items
                            .options
                            .iter()
                            .map(|option| QuestionOption {
                                id: option.value.clone(),
                                label: option.title.clone(),
                            })
                            .collect(),
                        MultiSelectItems::Other(_) => {
                            return Err("unsupported ACP elicitation array item type".into());
                        }
                        _ => return Err("unsupported ACP elicitation array item type".into()),
                    };
                    (
                        schema.title.as_deref(),
                        schema.description.as_deref(),
                        options,
                        true,
                        false,
                        schema.default.clone().unwrap_or_default(),
                        QuestionValueKind::StringArray,
                    )
                }
                ElicitationPropertySchema::Other(_) => {
                    return Err("unsupported ACP elicitation field type".into());
                }
                _ => return Err("unsupported ACP elicitation field type".into()),
            };
        if options.len() > MAX_CHOICES {
            return Err(format!(
                "agent supplied {} choices for elicitation field {id}; expected at most {MAX_CHOICES}",
                options.len()
            ));
        }
        let prompt = match (title, description) {
            (Some(title), Some(description)) => format!("{title}\n{description}"),
            (Some(title), None) => title.to_owned(),
            (None, Some(description)) => format!("{id}\n{description}"),
            (None, None) => id.clone(),
        };
        questions.push(Question {
            id: id.clone(),
            prompt,
            options,
            allow_multiple,
            required: required.contains(&id),
            secret,
            default_values,
            value_kind,
        });
        fields.insert(id, field);
    }
    Ok((
        InteractionRequest {
            request_id,
            tool_call_id,
            kind: InteractionKind::Questions {
                title: request.message,
                questions,
            },
        },
        PendingElicitationKind::Form { fields, required },
    ))
}

fn elicitation_secret(meta: Option<&serde_json::Map<String, serde_json::Value>>) -> bool {
    meta.and_then(|meta| meta.get("codex"))
        .and_then(|codex| codex.get("isSecret"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn respond_elicitation(
    request_id: u64,
    response: InteractionResponse,
    elicitations: &Mutex<HashMap<u64, PendingElicitation>>,
    events: &EventSender,
) -> bool {
    let mut elicitations = elicitations.lock().expect("elicitation lock poisoned");
    let Some(pending) = elicitations.get(&request_id) else {
        return false;
    };
    let action = match (&pending.kind, response) {
        (
            PendingElicitationKind::Form { fields, required },
            InteractionResponse::Answers(answers),
        ) => {
            if answers.len() > MAX_CHOICES
                || answers.iter().any(|answer| {
                    answer.question_id.len() > MAX_DETAIL_BYTES
                        || answer.selected_option_ids.len() > MAX_CHOICES
                        || answer
                            .selected_option_ids
                            .iter()
                            .any(|value| value.len() > MAX_DETAIL_BYTES)
                })
            {
                send_event(
                    events,
                    Event::Error("elicitation answer is too large".into()),
                );
                return true;
            }
            let mut content = std::collections::BTreeMap::new();
            let mut seen = HashSet::new();
            for answer in answers {
                let Some(field) = fields.get(&answer.question_id) else {
                    send_event(events, Event::Error("unknown elicitation answer".into()));
                    return true;
                };
                if !seen.insert(answer.question_id.clone()) {
                    send_event(events, Event::Error("duplicate elicitation answer".into()));
                    return true;
                }
                if answer.selected_option_ids.is_empty() {
                    continue;
                }
                let value = match elicitation_value(field, &answer.selected_option_ids) {
                    Ok(value) => value,
                    Err(error) => {
                        send_event(events, Event::Error(error));
                        return true;
                    }
                };
                content.insert(answer.question_id, value);
            }
            if required.iter().any(|field| !content.contains_key(field)) {
                send_event(
                    events,
                    Event::Error("not every required field was answered".into()),
                );
                return true;
            }
            ElicitationAction::Accept(ElicitationAcceptAction::new().content(content))
        }
        (PendingElicitationKind::Form { .. }, InteractionResponse::Skipped) => {
            ElicitationAction::Decline
        }
        (PendingElicitationKind::Url, InteractionResponse::Accepted) => {
            ElicitationAction::Accept(ElicitationAcceptAction::new())
        }
        (PendingElicitationKind::Url, InteractionResponse::Declined) => ElicitationAction::Decline,
        _ => {
            send_event(
                events,
                Event::Error("response does not match elicitation".into()),
            );
            return true;
        }
    };
    let pending = elicitations
        .remove(&request_id)
        .expect("pending elicitation disappeared");
    let _ = pending.response_tx.try_send(action);
    true
}

fn elicitation_value(
    field: &ElicitationPropertySchema,
    values: &[String],
) -> Result<ElicitationContentValue, String> {
    match field {
        ElicitationPropertySchema::String(schema) => {
            let [value] = values else {
                return Err("string elicitation fields require one value".into());
            };
            if schema
                .min_length
                .is_some_and(|minimum| value.chars().count() < minimum as usize)
                || schema
                    .max_length
                    .is_some_and(|maximum| value.chars().count() > maximum as usize)
                || schema
                    .enum_values
                    .as_ref()
                    .is_some_and(|options| !options.contains(value))
                || schema
                    .one_of
                    .as_ref()
                    .is_some_and(|options| !options.iter().any(|option| option.value == *value))
                || schema.pattern.as_ref().is_some_and(|pattern| {
                    !regex::Regex::new(pattern).is_ok_and(|re| re.is_match(value))
                })
                || schema
                    .format
                    .is_some_and(|format| !valid_string_format(value, format))
            {
                return Err("invalid string elicitation value".into());
            }
            Ok(ElicitationContentValue::String(value.clone()))
        }
        ElicitationPropertySchema::Number(schema) => {
            let [value] = values else {
                return Err("number elicitation fields require one value".into());
            };
            let value = value
                .parse::<f64>()
                .map_err(|_| "invalid number elicitation value".to_owned())?;
            if !value.is_finite()
                || schema.minimum.is_some_and(|minimum| value < minimum)
                || schema.maximum.is_some_and(|maximum| value > maximum)
            {
                return Err("number elicitation value is out of range".into());
            }
            Ok(ElicitationContentValue::Number(value))
        }
        ElicitationPropertySchema::Integer(schema) => {
            let [value] = values else {
                return Err("integer elicitation fields require one value".into());
            };
            let value = value
                .parse::<i64>()
                .map_err(|_| "invalid integer elicitation value".to_owned())?;
            if schema.minimum.is_some_and(|minimum| value < minimum)
                || schema.maximum.is_some_and(|maximum| value > maximum)
            {
                return Err("integer elicitation value is out of range".into());
            }
            Ok(ElicitationContentValue::Integer(value))
        }
        ElicitationPropertySchema::Boolean(_) => match values {
            [value] if value == "true" => Ok(ElicitationContentValue::Boolean(true)),
            [value] if value == "false" => Ok(ElicitationContentValue::Boolean(false)),
            _ => Err("invalid boolean elicitation value".into()),
        },
        ElicitationPropertySchema::Array(schema) => {
            let allowed = match &schema.items {
                MultiSelectItems::String(items) => items.values.iter().collect::<HashSet<_>>(),
                MultiSelectItems::Titled(items) => {
                    items.options.iter().map(|option| &option.value).collect()
                }
                MultiSelectItems::Other(_) => return Err("unsupported elicitation array".into()),
                _ => return Err("unsupported elicitation array".into()),
            };
            if values.iter().collect::<HashSet<_>>().len() != values.len()
                || values.iter().any(|value| !allowed.contains(value))
                || schema
                    .min_items
                    .is_some_and(|minimum| values.len() < minimum as usize)
                || schema
                    .max_items
                    .is_some_and(|maximum| values.len() > maximum as usize)
            {
                return Err("invalid multi-select elicitation value".into());
            }
            Ok(ElicitationContentValue::StringArray(values.to_vec()))
        }
        ElicitationPropertySchema::Other(_) => Err("unsupported elicitation field".into()),
        _ => Err("unsupported elicitation field".into()),
    }
}

fn valid_string_format(value: &str, format: StringFormat) -> bool {
    match format {
        StringFormat::Email => value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && !domain.is_empty()
                && !domain.contains('@')
                && !value.chars().any(char::is_whitespace)
        }),
        StringFormat::Uri => value.split_once(':').is_some_and(|(scheme, rest)| {
            !rest.is_empty()
                && scheme.starts_with(|character: char| character.is_ascii_alphabetic())
                && scheme
                    .chars()
                    .all(|character| character.is_ascii_alphanumeric() || "+-.".contains(character))
                && !value.chars().any(char::is_whitespace)
        }),
        StringFormat::Date => valid_date(value),
        StringFormat::DateTime => valid_date_time(value),
        _ => false,
    }
}

fn valid_date(value: &str) -> bool {
    let Some((year, month, day)) = value
        .split_once('-')
        .and_then(|(year, rest)| rest.split_once('-').map(|(month, day)| (year, month, day)))
    else {
        return false;
    };
    let (Ok(year), Ok(month), Ok(day)) = (
        year.parse::<u32>(),
        month.parse::<u32>(),
        day.parse::<u32>(),
    ) else {
        return false;
    };
    if value.len() != 10 || !(1..=12).contains(&month) {
        return false;
    }
    let leap = year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400));
    let days = match month {
        2 if leap => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    (1..=days).contains(&day)
}

fn valid_date_time(value: &str) -> bool {
    let Some((date, time)) = value.split_once(['T', 't']) else {
        return false;
    };
    let (clock, offset) = if let Some(clock) = time.strip_suffix(['Z', 'z']) {
        (clock, None)
    } else if time.len() >= 6 {
        let start = time.len() - 6;
        if !matches!(time.as_bytes()[start], b'+' | b'-') {
            return false;
        }
        (&time[..start], Some(&time.as_bytes()[start..]))
    } else {
        return false;
    };
    let mut parts = clock.split(':');
    let (Some(hour), Some(minute), Some(second), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return false;
    };
    let (second, fraction) = second
        .split_once('.')
        .map_or((second, None), |(second, fraction)| {
            (second, Some(fraction))
        });
    let valid_clock = hour.len() == 2
        && minute.len() == 2
        && second.len() == 2
        && hour.parse::<u8>().is_ok_and(|hour| hour <= 23)
        && minute.parse::<u8>().is_ok_and(|minute| minute <= 59)
        && second.parse::<u8>().is_ok_and(|second| second <= 60)
        && fraction.is_none_or(|fraction| {
            !fraction.is_empty() && fraction.chars().all(|character| character.is_ascii_digit())
        });
    let valid_offset = offset.is_none_or(|offset| {
        offset.len() == 6
            && offset[3] == b':'
            && offset[1..3].iter().all(u8::is_ascii_digit)
            && offset[4..].iter().all(u8::is_ascii_digit)
            && (offset[1] - b'0') * 10 + offset[2] - b'0' <= 23
            && (offset[4] - b'0') * 10 + offset[5] - b'0' <= 59
    });
    valid_date(date) && valid_clock && valid_offset
}

fn respond_interaction(
    request_id: u64,
    response: InteractionResponse,
    interactions: &Mutex<HashMap<u64, PendingInteraction>>,
    events: &EventSender,
) {
    let mut interactions = interactions.lock().expect("interaction lock poisoned");
    let Some(pending) = interactions.get(&request_id) else {
        send_event(
            events,
            Event::Error("interaction request was already answered".into()),
        );
        return;
    };
    let value = match (&pending.kind, response) {
        (PendingInteractionKind::Questions(allowed), InteractionResponse::Answers(answers)) => {
            if answers.len() != allowed.len() {
                send_event(
                    events,
                    Event::Error("not every question was answered".into()),
                );
                return;
            }
            let mut seen = HashSet::new();
            for answer in &answers {
                let Some((options, multiple)) = allowed.get(&answer.question_id) else {
                    send_event(events, Event::Error("unknown question answer".into()));
                    return;
                };
                if !seen.insert(&answer.question_id)
                    || answer.selected_option_ids.is_empty()
                    || (!multiple && answer.selected_option_ids.len() != 1)
                    || answer
                        .selected_option_ids
                        .iter()
                        .collect::<HashSet<_>>()
                        .len()
                        != answer.selected_option_ids.len()
                    || answer
                        .selected_option_ids
                        .iter()
                        .any(|option| !options.contains(option))
                {
                    send_event(events, Event::Error("invalid question answer".into()));
                    return;
                }
            }
            serde_json::json!({
                "outcome": {
                    "outcome": "answered",
                    "answers": answers.into_iter().map(|answer| serde_json::json!({
                        "questionId": answer.question_id,
                        "selectedOptionIds": answer.selected_option_ids,
                    })).collect::<Vec<_>>()
                }
            })
        }
        (PendingInteractionKind::Questions(_), InteractionResponse::Skipped) => {
            serde_json::json!({"outcome": {"outcome": "skipped", "reason": "Skipped by user"}})
        }
        (PendingInteractionKind::Plan, InteractionResponse::PlanAccepted) => {
            serde_json::json!({"outcome": {"outcome": "accepted"}})
        }
        (PendingInteractionKind::Plan, InteractionResponse::PlanRejected) => {
            serde_json::json!({"outcome": {"outcome": "rejected", "reason": "Rejected by user"}})
        }
        _ => {
            send_event(
                events,
                Event::Error("response does not match interaction".into()),
            );
            return;
        }
    };
    let pending = interactions
        .remove(&request_id)
        .expect("pending interaction disappeared");
    let _ = pending.response_tx.try_send(value);
}

fn decide_permission(
    request_id: u64,
    option_id: String,
    permissions: &Mutex<HashMap<u64, PendingPermission>>,
    events: &EventSender,
) {
    let mut permissions = permissions.lock().expect("permission lock poisoned");
    let Some(pending) = permissions.get(&request_id) else {
        send_event(
            events,
            Event::Error("permission request was already answered".into()),
        );
        return;
    };
    if !pending.allowed.contains(&option_id) {
        send_event(
            events,
            Event::Error("permission option was not supplied by the agent".into()),
        );
        return;
    }
    let pending = permissions
        .remove(&request_id)
        .expect("pending permission disappeared");
    let _ = pending
        .decision_tx
        .try_send(PendingDecision::Selected(option_id));
}

#[cfg(test)]
fn normalize_update(update: SessionUpdate, events: &EventSender) {
    normalize_update_with_rate_limit(update, events, &Mutex::new(None));
}

fn normalize_update_with_rate_limit(
    update: SessionUpdate,
    events: &EventSender,
    claude_rate_limit_status: &Mutex<Option<String>>,
) {
    match update {
        SessionUpdate::UserMessageChunk(chunk) => {
            normalize_content_chunk(ContentRole::User, chunk, events)
        }
        SessionUpdate::AgentMessageChunk(chunk) => {
            normalize_content_chunk(ContentRole::Assistant, chunk, events)
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            normalize_content_chunk(ContentRole::Thought, chunk, events)
        }
        SessionUpdate::Plan(plan) => send_event(
            events,
            Event::PlanUpdated(
                plan.entries
                    .into_iter()
                    .take(MAX_PLAN_ITEMS)
                    .map(|entry| PlanItem {
                        content: bounded_detail(entry.content),
                        status: bounded_detail(format!("{:?}", entry.status)),
                    })
                    .collect(),
            ),
        ),
        SessionUpdate::ToolCall(tool) => {
            let mut kind =
                normalized_tool_kind(events.provider, Some(tool.kind), tool.meta.as_ref());
            let detail = normalized_tool_detail(
                events.provider,
                &tool.title,
                tool.raw_input.as_ref(),
                &tool.content,
                tool.raw_output.as_ref(),
                tool.meta.as_ref(),
                true,
            );
            let title = subagent_description(detail.as_ref()).map_or(tool.title, |description| {
                kind = Some("Task".into());
                format!("Subagent: {description}")
            });
            send_tool_activity(
                events,
                tool.meta.as_ref(),
                ToolActivity {
                    id: bounded_detail(tool.tool_call_id.0.to_string()),
                    title: Some(bounded_detail(title)),
                    status: Some(format!("{:?}", tool.status)),
                    kind,
                    paths: tool_paths(&tool.locations, &tool.content),
                    detail,
                },
            );
        }
        SessionUpdate::ToolCallUpdate(update) => {
            let meta = update.meta;
            let fields = update.fields;
            let content = fields.content.as_deref().unwrap_or_default();
            let locations = fields.locations.as_deref().unwrap_or_default();
            let has_input = fields.raw_input.is_some();
            let subagent = is_subagent_tool(
                events.provider,
                fields.title.as_deref().unwrap_or_default(),
                meta.as_ref(),
            );
            let detail = normalized_tool_detail(
                events.provider,
                fields.title.as_deref().unwrap_or_default(),
                fields.raw_input.as_ref(),
                content,
                fields.raw_output.as_ref(),
                meta.as_ref(),
                false,
            );
            let mut kind = normalized_tool_kind(events.provider, fields.kind, meta.as_ref());
            if subagent {
                kind = Some("Task".into());
            }
            let title = subagent_description(detail.as_ref()).map_or_else(
                || {
                    if subagent && !has_input {
                        None
                    } else {
                        fields.title
                    }
                },
                |description| Some(format!("Subagent: {description}")),
            );
            send_tool_activity(
                events,
                meta.as_ref(),
                ToolActivity {
                    id: bounded_detail(update.tool_call_id.0.to_string()),
                    title: title.map(bounded_detail),
                    status: fields.status.map(|status| format!("{status:?}")),
                    kind,
                    paths: tool_paths(locations, content),
                    detail,
                },
            );
        }
        SessionUpdate::UsageUpdate(usage) => {
            if events.provider == ProviderId::Claude
                && let Some(status) = usage
                    .meta
                    .as_ref()
                    .and_then(|meta| meta.get("_claude/rateLimit"))
                    .and_then(|rate_limit| rate_limit.get("status"))
                    .and_then(serde_json::Value::as_str)
                && let Ok(mut latest) = claude_rate_limit_status.lock()
            {
                *latest = Some(status.chars().take(64).collect());
            }
            send_event(
                events,
                Event::UsageUpdated {
                    used: usage.used,
                    size: usage.size,
                    cost: usage
                        .cost
                        .map(|cost| format!("{} {}", cost.amount, cost.currency)),
                },
            );
        }
        SessionUpdate::CurrentModeUpdate(update) => send_event(
            events,
            Event::ModeChanged(update.current_mode_id.0.to_string()),
        ),
        SessionUpdate::ConfigOptionUpdate(update) => send_event(
            events,
            Event::ConfigOptionsUpdated(normalize_config_options(&update.config_options)),
        ),
        SessionUpdate::AvailableCommandsUpdate(update) => send_event(
            events,
            Event::CommandsUpdated(
                update
                    .available_commands
                    .into_iter()
                    .take(MAX_CHOICES)
                    .map(|command| CommandChoice {
                        name: command.name,
                        description: command.description,
                        input_hint: command.input.and_then(|input| match input {
                            AvailableCommandInput::Unstructured(input) => Some(input.hint),
                            _ => None,
                        }),
                    })
                    .collect(),
            ),
        ),
        SessionUpdate::SessionInfoUpdate(update) => {
            if let Some(title) = update.title.as_opt_ref() {
                send_event(events, Event::SessionTitleUpdated(title.cloned()));
            }
            if let Some(goal) = update.meta.as_ref().and_then(|meta| meta.get("goal")) {
                if goal.is_null() {
                    send_event(events, Event::GoalUpdated(None));
                } else if let Ok(goal) = serde_json::from_value::<GoalState>(goal.clone()) {
                    send_event(
                        events,
                        Event::GoalUpdated(Some(GoalState {
                            objective: bounded_detail(goal.objective),
                            status: bounded_detail(goal.status),
                            ..goal
                        })),
                    );
                }
            }
        }
        _ => {}
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorTodosUpdate {
    tool_call_id: String,
    #[serde(default)]
    merge: bool,
    todos: Vec<CursorTodo>,
}

#[derive(Deserialize)]
struct CursorTodo {
    id: String,
    content: String,
    status: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorTaskUpdate {
    tool_call_id: String,
    description: String,
    prompt: String,
    subagent_type: CursorSubagentType,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

/// Cursor documents `subagentType` as a set of known strings plus a
/// `{ custom: string }` object for user-defined subagent types. Live
/// `agent acp` (verified 2026-08-11) additionally sends `custom` as an
/// object, e.g. `{"custom": {"unspecified": {}}}`, so the payload is kept
/// permissive and normalized to a display string.
#[derive(Deserialize)]
#[serde(untagged)]
enum CursorSubagentType {
    Named(String),
    Custom { custom: serde_json::Value },
}

impl From<CursorSubagentType> for String {
    fn from(subagent_type: CursorSubagentType) -> Self {
        match subagent_type {
            CursorSubagentType::Named(name) => name,
            CursorSubagentType::Custom { custom } => match custom {
                serde_json::Value::String(name) => name,
                serde_json::Value::Object(map) if map.len() == 1 => {
                    map.into_iter().next().expect("one entry").0
                }
                _ => "custom".into(),
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorImageUpdate {
    tool_call_id: String,
    description: String,
    #[serde(default)]
    file_path: Option<PathBuf>,
    #[serde(default)]
    reference_image_paths: Vec<PathBuf>,
}

fn normalize_cursor_notification(method: &str, params: serde_json::Value, events: &EventSender) {
    let tool = match method {
        "cursor/update_todos" => {
            serde_json::from_value::<CursorTodosUpdate>(params).map(|update| ToolActivity {
                id: update.tool_call_id,
                title: Some(if update.merge {
                    "Todos (merged)".into()
                } else {
                    "Todos".into()
                }),
                status: None,
                kind: None,
                paths: Vec::new(),
                detail: Some(ToolDetail {
                    input: None,
                    content: update
                        .todos
                        .into_iter()
                        .take(MAX_CHOICES)
                        .map(|todo| ToolOutput::Todo {
                            id: bounded_detail(todo.id),
                            content: bounded_detail(todo.content),
                            status: bounded_detail(todo.status),
                        })
                        .collect(),
                    output: None,
                }),
            })
        }
        "cursor/task" => serde_json::from_value::<CursorTaskUpdate>(params).map(|update| {
            let description = bounded_detail(update.description);
            ToolActivity {
                id: bounded_detail(update.tool_call_id),
                title: Some(bounded_detail(format!("Subagent: {description}"))),
                status: Some("Completed".into()),
                kind: Some("Task".into()),
                paths: Vec::new(),
                detail: Some(ToolDetail {
                    input: None,
                    content: vec![ToolOutput::Task {
                        description,
                        prompt: bounded_detail(update.prompt),
                        subagent_type: bounded_detail(update.subagent_type.into()),
                        model: update.model.map(bounded_detail),
                        agent_id: update.agent_id.map(bounded_detail),
                        agents: Vec::new(),
                        path: None,
                        activity: None,
                        duration_ms: update.duration_ms,
                    }],
                    output: None,
                }),
            }
        }),
        "cursor/generate_image" => {
            serde_json::from_value::<CursorImageUpdate>(params).map(|update| ToolActivity {
                id: update.tool_call_id,
                title: Some("Generated image".into()),
                status: Some("Completed".into()),
                kind: None,
                paths: update
                    .file_path
                    .clone()
                    .into_iter()
                    .map(ToolPath::from)
                    .collect(),
                detail: Some(ToolDetail {
                    input: None,
                    content: vec![ToolOutput::GeneratedImage {
                        description: bounded_detail(update.description),
                        file_path: update.file_path,
                        reference_image_paths: update
                            .reference_image_paths
                            .into_iter()
                            .take(MAX_CHOICES)
                            .collect(),
                    }],
                    output: None,
                }),
            })
        }
        _ => return,
    };
    match tool {
        Ok(tool) => send_event(events, Event::ToolCallUpdated(tool)),
        Err(error) => send_event(
            events,
            Event::Error(format!("invalid {method} payload: {error}")),
        ),
    }
}

fn normalize_content_chunk(role: ContentRole, chunk: ContentChunk, events: &EventSender) {
    let Some(parent) = claude_parent_tool_use_id(events.provider, chunk.meta.as_ref()) else {
        normalize_content(role, chunk.content, events);
        return;
    };
    let Some(content) = normalize_display_content(chunk.content) else {
        return;
    };
    let text = match content {
        NormalizedContent::Text(text) => text,
        NormalizedContent::Display(content) => display_content_summary(&content),
    };
    let _ = role;
    send_event(events, subagent_log_update(parent, text));
}

fn normalize_content(role: ContentRole, content: ContentBlock, events: &EventSender) {
    match normalize_display_content(content) {
        Some(NormalizedContent::Text(text)) => {
            let event = match role {
                ContentRole::User => Event::UserMessage(text),
                ContentRole::Assistant => Event::AssistantDelta(text),
                ContentRole::Thought => Event::ThoughtDelta(text),
            };
            send_event(events, event);
        }
        Some(NormalizedContent::Display(content)) => {
            send_event(events, Event::ContentReceived { role, content });
        }
        None => {}
    }
}

fn display_content_summary(content: &DisplayContent) -> String {
    match content {
        DisplayContent::Image {
            mime_type,
            uri,
            encoded_bytes,
            ..
        } => format!(
            "Image · {mime_type} · {encoded_bytes} encoded bytes{}",
            uri.as_deref()
                .map_or(String::new(), |uri| format!(" · {uri}"))
        ),
        DisplayContent::Audio {
            mime_type,
            encoded_bytes,
        } => format!("Audio · {mime_type} · {encoded_bytes} encoded bytes"),
        DisplayContent::ResourceLink {
            name, title, uri, ..
        } => format!("{} · {uri}", title.as_deref().unwrap_or(name)),
        DisplayContent::TextResource { uri, text, .. } => format!("{uri}\n{text}"),
        DisplayContent::BlobResource {
            uri, encoded_bytes, ..
        } => format!("{uri} · {encoded_bytes} encoded bytes"),
    }
}

enum NormalizedContent {
    Text(String),
    Display(DisplayContent),
}

fn normalize_display_content(content: ContentBlock) -> Option<NormalizedContent> {
    Some(match content {
        ContentBlock::Text(text) => NormalizedContent::Text(bounded_detail(text.text)),
        ContentBlock::Image(image) => {
            let encoded_bytes = image.data.len();
            let data = (encoded_bytes <= MAX_DISPLAY_IMAGE_BYTES.div_ceil(3) * 4)
                .then(|| {
                    base64::engine::general_purpose::STANDARD
                        .decode(&image.data)
                        .ok()
                        .filter(|bytes| bytes.len() <= MAX_DISPLAY_IMAGE_BYTES)
                        .map(Arc::<[u8]>::from)
                })
                .flatten();
            NormalizedContent::Display(DisplayContent::Image {
                mime_type: image.mime_type,
                uri: image.uri,
                encoded_bytes,
                data,
            })
        }
        ContentBlock::Audio(audio) => NormalizedContent::Display(DisplayContent::Audio {
            mime_type: audio.mime_type,
            encoded_bytes: audio.data.len(),
        }),
        ContentBlock::ResourceLink(link) => {
            NormalizedContent::Display(DisplayContent::ResourceLink {
                name: link.name,
                title: link.title,
                uri: link.uri,
                description: link.description,
                mime_type: link.mime_type,
                size: link.size,
            })
        }
        ContentBlock::Resource(resource) => match resource.resource {
            EmbeddedResourceResource::TextResourceContents(resource) => {
                NormalizedContent::Display(DisplayContent::TextResource {
                    uri: resource.uri,
                    mime_type: resource.mime_type,
                    text: bounded_detail(resource.text),
                })
            }
            EmbeddedResourceResource::BlobResourceContents(resource) => {
                NormalizedContent::Display(DisplayContent::BlobResource {
                    uri: resource.uri,
                    mime_type: resource.mime_type,
                    encoded_bytes: resource.blob.len(),
                })
            }
            _ => return None,
        },
        _ => return None,
    })
}

fn normalize_config_options(options: &[SessionConfigOption]) -> Vec<ConfigChoice> {
    options
        .iter()
        .take(MAX_CHOICES)
        .filter_map(|option| {
            let (value, values) = match &option.kind {
                SessionConfigKind::Select(select) => {
                    let values: Vec<&SessionConfigSelectOption> = match &select.options {
                        SessionConfigSelectOptions::Ungrouped(options) => options.iter().collect(),
                        SessionConfigSelectOptions::Grouped(groups) => {
                            groups.iter().flat_map(|group| &group.options).collect()
                        }
                        _ => Vec::new(),
                    };
                    (
                        ConfigValue::Select(select.current_value.0.to_string()),
                        values
                            .into_iter()
                            .take(MAX_CHOICES)
                            .map(|value| ConfigValueChoice {
                                id: value.value.0.to_string(),
                                name: value.name.clone(),
                                description: value.description.clone(),
                            })
                            .collect(),
                    )
                }
                SessionConfigKind::Boolean(boolean) => {
                    (ConfigValue::Boolean(boolean.current_value), Vec::new())
                }
                _ => return None,
            };
            Some(ConfigChoice {
                id: option.id.0.to_string(),
                name: option.name.clone(),
                description: option.description.clone(),
                value,
                options: values,
            })
        })
        .collect()
}

fn tool_paths(locations: &[ToolCallLocation], content: &[ToolCallContent]) -> Vec<ToolPath> {
    let mut paths = locations
        .iter()
        .take(MAX_TOOL_PATHS)
        .map(|location| ToolPath {
            path: location.path.clone(),
            line: location.line,
        })
        .collect::<Vec<_>>();
    for item in content {
        if paths.len() == MAX_TOOL_PATHS {
            break;
        }
        if let ToolCallContent::Diff(diff) = item
            && !paths.iter().any(|existing| existing.path == diff.path)
        {
            paths.push(diff.path.clone().into());
        }
    }
    paths
}

fn tool_detail(
    input: Option<&serde_json::Value>,
    content: &[ToolCallContent],
    output: Option<&serde_json::Value>,
) -> Option<ToolDetail> {
    let input = input
        .filter(|value| json_has_content(value))
        .map(bounded_json);
    let content = content
        .iter()
        .take(MAX_CHOICES)
        .filter_map(|content| match content {
            ToolCallContent::Content(content) => {
                match normalize_display_content(content.content.clone())? {
                    NormalizedContent::Text(text) => Some(ToolOutput::Text(text)),
                    NormalizedContent::Display(content) => Some(ToolOutput::Content(content)),
                }
            }
            ToolCallContent::Diff(diff) => Some(ToolOutput::Diff {
                path: diff.path.clone(),
                old_text: diff
                    .old_text
                    .as_deref()
                    .map(|text| Arc::from(bounded_detail(text.to_owned()))),
                new_text: Arc::from(bounded_detail(diff.new_text.clone())),
            }),
            ToolCallContent::Terminal(terminal) => {
                Some(ToolOutput::Terminal(terminal.terminal_id.0.to_string()))
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    let output = output
        .filter(|value| json_has_content(value))
        .map(bounded_json);
    (input.is_some() || !content.is_empty() || output.is_some()).then_some(ToolDetail {
        input,
        content,
        output,
    })
}

fn normalized_tool_kind(
    provider: ProviderId,
    kind: Option<ToolKind>,
    meta: Option<&Meta>,
) -> Option<String> {
    if provider_meta_is_subagent(provider, meta) {
        Some("Task".into())
    } else {
        kind.map(|kind| format!("{kind:?}"))
    }
}

fn send_tool_activity(events: &EventSender, meta: Option<&Meta>, tool: ToolActivity) {
    if let Some(parent) = claude_parent_tool_use_id(events.provider, meta) {
        let summary = nested_tool_summary(&tool, meta);
        send_event(events, subagent_log_update(parent, summary));
    } else {
        send_event(events, Event::ToolCallUpdated(tool));
    }
}

fn claude_parent_tool_use_id(provider: ProviderId, meta: Option<&Meta>) -> Option<String> {
    (provider == ProviderId::Claude)
        .then_some(meta?)?
        .get("claudeCode")?
        .get("parentToolUseId")?
        .as_str()
        .map(|id| bounded_detail(id.to_owned()))
}

fn subagent_log_update(parent: String, text: String) -> Event {
    Event::ToolCallUpdated(ToolActivity {
        id: parent,
        title: None,
        status: None,
        kind: None,
        paths: Vec::new(),
        detail: Some(ToolDetail {
            input: None,
            content: vec![ToolOutput::Log {
                label: "Subagent transcript".into(),
                text: bounded_detail(text),
            }],
            output: None,
        }),
    })
}

fn nested_tool_summary(tool: &ToolActivity, meta: Option<&Meta>) -> String {
    let fallback = meta
        .and_then(|meta| meta.get("claudeCode"))
        .and_then(|claude| claude.get("toolName"))
        .and_then(serde_json::Value::as_str)
        .unwrap_or("Tool");
    let mut text = format!(
        "\nTool · {}",
        tool.title
            .as_deref()
            .filter(|title| !title.is_empty())
            .unwrap_or(fallback)
    );
    if let Some(status) = &tool.status {
        let _ = write!(text, " · {status}");
    }
    for path in &tool.paths {
        let _ = write!(text, "\n{}", path.path.display());
    }
    if let Some(detail) = &tool.detail {
        if let Some(input) = &detail.input {
            let _ = write!(text, "\n{input}");
        }
        for content in &detail.content {
            match content {
                ToolOutput::Text(value) => {
                    let _ = write!(text, "\n{value}");
                }
                ToolOutput::Log { label, text: value } => {
                    let _ = write!(text, "\n{label}\n{value}");
                }
                ToolOutput::Content(content) => {
                    let _ = write!(text, "\n{}", display_content_summary(content));
                }
                ToolOutput::Diff {
                    path,
                    old_text,
                    new_text,
                } => {
                    let _ = write!(
                        text,
                        "\nDiff {}\n{}\n{}",
                        path.display(),
                        old_text.as_deref().unwrap_or_default(),
                        new_text
                    );
                }
                ToolOutput::Terminal(id) => {
                    let _ = write!(text, "\nTerminal {id}");
                }
                ToolOutput::Todo {
                    id,
                    content,
                    status,
                } => {
                    let _ = write!(text, "\n{status} · {content} · {id}");
                }
                ToolOutput::Task { description, .. } => {
                    let _ = write!(text, "\nSubagent · {description}");
                }
                ToolOutput::GeneratedImage {
                    description,
                    file_path,
                    ..
                } => {
                    let _ = write!(text, "\n{description}");
                    if let Some(path) = file_path {
                        let _ = write!(text, "\n{}", path.display());
                    }
                }
            }
        }
        if let Some(output) = &detail.output {
            let _ = write!(text, "\n{output}");
        }
    }
    text.push('\n');
    text
}

fn normalized_tool_detail(
    provider: ProviderId,
    title: &str,
    input: Option<&serde_json::Value>,
    content: &[ToolCallContent],
    output: Option<&serde_json::Value>,
    meta: Option<&Meta>,
    initial: bool,
) -> Option<ToolDetail> {
    let mut detail = tool_detail(input, content, output);
    let subagent = is_subagent_tool(provider, title, meta);
    if (initial || input.is_some())
        && let Some(task) = normalized_subagent_task(provider, title, input, output, meta)
    {
        let detail = detail.get_or_insert_with(|| ToolDetail {
            input: None,
            content: Vec::new(),
            output: None,
        });
        if let ToolOutput::Task { prompt, .. } = &task {
            detail
                .content
                .retain(|content| !matches!(content, ToolOutput::Text(text) if text == prompt));
        }
        detail.input = None;
        detail.content.insert(0, task);
    }
    if subagent {
        let output_text = subagent_output_text(output);
        if detail.is_some() || output_text.is_some() {
            let detail = detail.get_or_insert_with(|| ToolDetail {
                input: None,
                content: Vec::new(),
                output: None,
            });
            detail.output = None;
            if let Some(text) = output_text {
                detail.content.push(ToolOutput::Log {
                    label: "Subagent output".into(),
                    text,
                });
            }
        }
    }
    let logs = tool_logs(provider, meta);
    if !logs.is_empty() {
        detail
            .get_or_insert_with(|| ToolDetail {
                input: None,
                content: Vec::new(),
                output: None,
            })
            .content
            .extend(logs);
    }
    detail
}

fn subagent_description(detail: Option<&ToolDetail>) -> Option<&str> {
    detail?.content.iter().find_map(|content| match content {
        ToolOutput::Task { description, .. } => Some(description.as_str()),
        _ => None,
    })
}

fn tool_logs(provider: ProviderId, meta: Option<&Meta>) -> Vec<ToolOutput> {
    let Some(meta) = meta else {
        return Vec::new();
    };
    [
        ("terminal_output", "Terminal output"),
        ("terminal_output_delta", "Terminal output"),
        ("mcp_output_delta", "MCP progress"),
    ]
    .into_iter()
    .filter_map(|(key, label)| {
        let text = meta
            .get(key)?
            .as_object()?
            .get("data")?
            .as_str()?
            .to_owned();
        (!text.is_empty()).then(|| ToolOutput::Log {
            label: label.to_owned(),
            text,
        })
    })
    .chain(claude_non_execution_log(provider, meta))
    .collect()
}

fn claude_non_execution_log(provider: ProviderId, meta: &Meta) -> Option<ToolOutput> {
    if provider != ProviderId::Claude {
        return None;
    }
    let claude = meta.get("claudeCode")?.as_object()?;
    let kind = claude.get("nonExecutionKind")?.as_str()?;
    let text = claude
        .get("userFeedback")
        .and_then(serde_json::Value::as_str)
        .map_or_else(
            || kind.to_owned(),
            |feedback| format!("{kind} · {feedback}"),
        );
    Some(ToolOutput::Log {
        label: "Not executed".into(),
        text,
    })
}

fn provider_meta_is_subagent(provider: ProviderId, meta: Option<&Meta>) -> bool {
    match provider {
        ProviderId::Codex => meta
            .and_then(|meta| meta.get("codex"))
            .and_then(serde_json::Value::as_object)
            .is_some_and(|codex| {
                codex.contains_key("collaboration") || codex.contains_key("subagent")
            }),
        ProviderId::Claude => {
            meta.and_then(|meta| meta.get("claudeCode"))
                .and_then(serde_json::Value::as_object)
                .and_then(|claude| claude.get("subagent"))
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        }
        ProviderId::Cursor => false,
    }
}

fn is_subagent_tool(provider: ProviderId, title: &str, meta: Option<&Meta>) -> bool {
    external_sessions::is_subagent_tool_name(title) || provider_meta_is_subagent(provider, meta)
}

fn normalized_subagent_task(
    provider: ProviderId,
    title: &str,
    input: Option<&serde_json::Value>,
    output: Option<&serde_json::Value>,
    meta: Option<&Meta>,
) -> Option<ToolOutput> {
    if !is_subagent_tool(provider, title, meta) {
        return None;
    }
    let input = input.and_then(serde_json::Value::as_object);
    let output = output.and_then(serde_json::Value::as_object);
    let input_string = |keys: &[&str]| {
        input.and_then(|input| {
            keys.iter()
                .find_map(|key| input.get(*key).and_then(serde_json::Value::as_str))
        })
    };
    let output_string = |keys: &[&str]| {
        output.and_then(|output| {
            keys.iter()
                .find_map(|key| output.get(*key).and_then(serde_json::Value::as_str))
        })
    };
    let prompt = input_string(&["prompt", "message", "task"])
        .unwrap_or_default()
        .to_owned();
    let model = input_string(&["model"]).map(str::to_owned);
    let supplied_agent_id =
        input_string(&["agent_id", "agentId"]).or_else(|| output_string(&["agent_id", "agentId"]));
    let duration_ms = input
        .and_then(|input| input.get("duration_ms").or_else(|| input.get("durationMs")))
        .or_else(|| {
            output.and_then(|output| {
                output
                    .get("duration_ms")
                    .or_else(|| output.get("durationMs"))
            })
        })
        .and_then(serde_json::Value::as_u64);
    let (description, subagent_type, agent_id, agents, path, activity) = match provider {
        ProviderId::Codex => {
            let codex = meta
                .and_then(|meta| meta.get("codex"))
                .and_then(serde_json::Value::as_object);
            let collaboration = codex
                .and_then(|codex| codex.get("collaboration"))
                .and_then(serde_json::Value::as_object);
            let subagent = codex
                .and_then(|codex| codex.get("subagent"))
                .and_then(serde_json::Value::as_object);
            let subagent_type = input_string(&["subagent_type", "subagentType"])
                .or_else(|| {
                    collaboration
                        .and_then(|collaboration| collaboration.get("tool"))
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or("subagent")
                .to_owned();
            let subagent_id = subagent
                .and_then(|subagent| subagent.get("threadId"))
                .and_then(serde_json::Value::as_str);
            let receiver_ids = input
                .and_then(|input| input.get("receiverThreadIds"))
                .and_then(serde_json::Value::as_array)
                .or_else(|| {
                    collaboration
                        .and_then(|collaboration| collaboration.get("receiverThreadIds"))
                        .and_then(serde_json::Value::as_array)
                });
            let states = input
                .and_then(|input| input.get("agentsStates"))
                .and_then(serde_json::Value::as_object);
            let mut agents = receiver_ids
                .into_iter()
                .flatten()
                .filter_map(serde_json::Value::as_str)
                .take(MAX_CHOICES)
                .map(|id| {
                    let state = states
                        .and_then(|states| states.get(id))
                        .and_then(serde_json::Value::as_object);
                    let status = state
                        .and_then(|state| state.get("status"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    let message = state
                        .and_then(|state| state.get("message"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    SubagentInfo {
                        id: id.to_owned(),
                        status,
                        message,
                    }
                })
                .collect::<Vec<_>>();
            if agents.is_empty()
                && let Some(id) = subagent_id
            {
                agents.push(SubagentInfo {
                    id: id.to_owned(),
                    status: None,
                    message: None,
                });
            }
            let agent_id = agents
                .first()
                .map(|agent| agent.id.clone())
                .or_else(|| supplied_agent_id.map(str::to_owned));
            let path = subagent
                .and_then(|subagent| subagent.get("path"))
                .and_then(serde_json::Value::as_str)
                .or_else(|| input_string(&["agentPath"]))
                .map(str::to_owned);
            let activity = subagent
                .and_then(|subagent| subagent.get("activity"))
                .and_then(serde_json::Value::as_str)
                .or_else(|| input_string(&["activityKind"]))
                .map(str::to_owned);
            (
                input_string(&["description"]).unwrap_or(title).to_owned(),
                subagent_type,
                agent_id,
                agents,
                path,
                activity,
            )
        }
        ProviderId::Claude => {
            let claude = meta
                .and_then(|meta| meta.get("claudeCode"))
                .and_then(serde_json::Value::as_object);
            let subagent_type = input_string(&["subagent_type", "subagentType"])
                .or_else(|| {
                    claude
                        .and_then(|claude| claude.get("toolName"))
                        .and_then(serde_json::Value::as_str)
                })
                .unwrap_or("subagent")
                .to_owned();
            (
                input_string(&["description"]).unwrap_or(title).to_owned(),
                subagent_type,
                supplied_agent_id.map(str::to_owned),
                Vec::new(),
                None,
                None,
            )
        }
        ProviderId::Cursor => {
            let subagent_type = input_string(&["subagent_type", "subagentType", "name"])
                .unwrap_or("subagent")
                .to_owned();
            (
                input_string(&["description"]).unwrap_or(title).to_owned(),
                subagent_type,
                supplied_agent_id.map(str::to_owned),
                Vec::new(),
                None,
                None,
            )
        }
    };
    Some(ToolOutput::Task {
        description,
        prompt,
        subagent_type,
        model,
        agent_id,
        agents,
        path,
        activity,
        duration_ms,
    })
}

fn subagent_output_text(output: Option<&serde_json::Value>) -> Option<String> {
    match output? {
        serde_json::Value::Null => None,
        serde_json::Value::String(text) => {
            let text = text.trim();
            (!text.is_empty()).then(|| bounded_detail(text.to_owned()))
        }
        serde_json::Value::Array(parts) => {
            let text = parts
                .iter()
                .filter_map(|part| subagent_output_text(Some(part)))
                .collect::<Vec<_>>()
                .join("\n");
            (!text.is_empty()).then(|| bounded_detail(text))
        }
        serde_json::Value::Object(fields) => {
            for key in ["output", "result", "response", "message", "text", "content"] {
                if let Some(text) = subagent_output_text(fields.get(key)) {
                    return Some(text);
                }
            }
            (!fields.keys().all(|key| {
                matches!(
                    key.as_str(),
                    "agentId" | "agent_id" | "durationMs" | "duration_ms" | "status"
                )
            }))
            .then(|| bounded_json(output.unwrap()))
        }
        value => Some(bounded_json(value)),
    }
}

fn json_has_content(value: &serde_json::Value) -> bool {
    match value {
        serde_json::Value::Null => false,
        serde_json::Value::String(value) => !value.is_empty(),
        serde_json::Value::Array(value) => !value.is_empty(),
        serde_json::Value::Object(value) => !value.is_empty(),
        _ => true,
    }
}

fn bounded_json(value: &serde_json::Value) -> String {
    let detail = value
        .as_object()
        .filter(|fields| fields.len() == 1)
        .and_then(|fields| fields.values().next())
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| "<unavailable>".into())
        });
    bounded_detail(detail)
}

fn bounded_detail(mut detail: String) -> String {
    if detail.len() > MAX_DETAIL_BYTES {
        let mut end = MAX_DETAIL_BYTES - '…'.len_utf8();
        while !detail.is_char_boundary(end) {
            end -= 1;
        }
        detail.truncate(end);
        detail.push('…');
    }
    detail
}

fn append_bounded(buffer: &Mutex<String>, line: &str, limit: usize) {
    let Ok(mut buffer) = buffer.lock() else {
        return;
    };
    if buffer.len() >= limit {
        return;
    }
    let remaining = limit - buffer.len();
    let mut end = line.len().min(remaining.saturating_sub(1));
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    buffer.push_str(&line[..end]);
    if buffer.len() < limit {
        buffer.push('\n');
    }
}

fn protocol_label(message: &serde_json::Value) -> Option<String> {
    let method = message.get("method")?.as_str()?;
    let update = message
        .pointer("/params/update/sessionUpdate")
        .and_then(serde_json::Value::as_str);
    Some(update.map_or_else(
        || method.to_owned(),
        |update| format!("{method} ({update})"),
    ))
}

fn send_event(events: &EventSender, event: Event) {
    if let (Event::ActiveSessionChanged(id), Some(path)) = (&event, &events.active_session) {
        let _ = save_active_session(path, id);
    }
    let _ = events.event_tx.send(event);
    (events.wake)();
}

fn tool_display_title<'a>(
    title: Option<&'a str>,
    kind: Option<&str>,
    paths: &[ToolPath],
    raw_input: Option<&str>,
) -> std::borrow::Cow<'a, str> {
    let trimmed = title.map(str::trim).filter(|title| !title.is_empty());
    let input = raw_input.and_then(parse_tool_input);
    if let Some(command) = tool_command(trimmed, kind, input.as_ref()) {
        return std::borrow::Cow::Owned(command);
    }
    if let Some(title) = trimmed
        && !tool_title_needs_humanizing(title)
    {
        return std::borrow::Cow::Borrowed(title);
    }
    if let Some(humanized) = humanize_machine_tool_title(trimmed, kind, paths, input.as_ref()) {
        return std::borrow::Cow::Owned(humanized);
    }
    trimmed
        .map(std::borrow::Cow::Borrowed)
        .unwrap_or_else(|| std::borrow::Cow::Borrowed("Tool activity"))
}

fn tool_command(
    title: Option<&str>,
    kind: Option<&str>,
    input: Option<&serde_json::Value>,
) -> Option<String> {
    let name = title.unwrap_or_default().to_ascii_lowercase();
    let execution = kind.is_some_and(|kind| kind.eq_ignore_ascii_case("execute"))
        || matches!(
            name.trim_start_matches(':').trim(),
            "bash" | "shell" | "exec" | "execute" | "run_terminal_cmd" | "run_command"
        )
        || name.contains("terminal command");
    if !execution {
        return None;
    }
    match input? {
        serde_json::Value::String(command) => {
            let command = command.trim();
            (!command.is_empty()).then(|| command.to_owned())
        }
        serde_json::Value::Object(input) => ["command", "cmd"].iter().find_map(|key| {
            input
                .get(*key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|command| !command.is_empty())
                .map(str::to_owned)
        }),
        _ => None,
    }
}

fn tool_title_needs_humanizing(title: &str) -> bool {
    let stripped = title.trim_start_matches(':').trim();
    if stripped.is_empty() {
        return true;
    }
    if stripped.contains(char::is_whitespace) {
        return false;
    }
    // Single-token titles that look like API / function names.
    stripped.contains('_')
        || stripped.contains('-')
        || stripped.bytes().all(|byte| byte.is_ascii_lowercase())
        || matches!(
            stripped,
            "Bash"
                | "Read"
                | "Edit"
                | "Write"
                | "Glob"
                | "Grep"
                | "Task"
                | "Wait"
                | "spawnAgent"
                | "Shell"
                | "Exec"
                | "ApplyPatch"
                | "apply_patch"
        )
}

fn parse_tool_input(raw: &str) -> Option<serde_json::Value> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    serde_json::from_str(trimmed)
        .ok()
        .or_else(|| Some(serde_json::Value::String(trimmed.to_owned())))
}

fn humanize_machine_tool_title(
    title: Option<&str>,
    kind: Option<&str>,
    paths: &[ToolPath],
    input: Option<&serde_json::Value>,
) -> Option<String> {
    let name = title
        .map(|title| title.trim_start_matches(':').trim())
        .filter(|title| !title.is_empty())
        .unwrap_or("");
    let path_label = paths.first().map(|path| {
        path.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.path.display().to_string())
    });
    let input_string = |keys: &[&str]| -> Option<String> {
        match input? {
            serde_json::Value::String(value) => {
                let value = value.trim();
                (!value.is_empty()).then(|| value.to_owned())
            }
            serde_json::Value::Object(object) => keys.iter().find_map(|key| {
                object
                    .get(*key)
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned)
            }),
            _ => None,
        }
    };
    let first_line = |value: &str| {
        value
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
            .unwrap_or(value)
            .to_owned()
    };
    let input_path_label = || {
        input_string(&["path", "file", "file_path", "target_file"]).map(|path| {
            Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or(path)
        })
    };

    let mapped = match name.to_ascii_lowercase().as_str() {
        "wait" | "wait_agent" => {
            if let Some(prompt) = input_string(&["prompt", "description", "reason"]) {
                Some(format!("Waiting · {}", first_line(&prompt)))
            } else if input
                .and_then(serde_json::Value::as_object)
                .is_some_and(|object| object.contains_key("agentsStates"))
            {
                Some("Waiting for agents".into())
            } else if let Some(command) = input_string(&["command", "cmd"]) {
                Some(format!("Waiting · {}", first_line(&command)))
            } else {
                Some("Waiting".into())
            }
        }
        "spawnagent" | "spawn_agent" | "spawn_agents" => {
            input_string(&["prompt", "description", "task"])
                .map(|prompt| format!("Spawn agent · {}", first_line(&prompt)))
                .or_else(|| Some("Spawn agent".into()))
        }
        "send_input" => input_string(&["prompt", "input", "message", "text"])
            .map(|prompt| format!("Send input · {}", first_line(&prompt)))
            .or_else(|| Some("Send input".into())),
        "close_agent" | "close_agents" => Some("Close agent".into()),
        "resume_agent" => Some("Resume agent".into()),
        "apply_patch" | "applypatch" => path_label
            .clone()
            .map(|path| format!("Edit {path}"))
            .or_else(|| Some("Editing files".into())),
        "bash" | "shell" | "exec" | "execute" | "run_terminal_cmd" | "run_command" => {
            input_string(&["command", "cmd", "description"])
                .map(|command| first_line(&command))
                .or_else(|| Some("Run command".into()))
        }
        "read" | "read_file" | "readfile" => path_label
            .clone()
            .or_else(input_path_label)
            .map(|path| format!("Read {path}"))
            .or_else(|| Some("Read file".into())),
        "edit" | "write" | "write_file" | "edit_file" | "edit_file_v2" | "search_replace" => {
            path_label
                .clone()
                .or_else(input_path_label)
                .map(|path| format!("Edit {path}"))
                .or_else(|| Some("Edit file".into()))
        }
        "grep" | "rg" | "search" => input_string(&["query", "pattern", "regex"])
            .map(|query| format!("Search · {query}"))
            .or_else(|| Some("Search".into())),
        "glob" | "find" => input_string(&["glob", "pattern", "path", "query"])
            .map(|query| format!("Find · {query}"))
            .or_else(|| Some("Find files".into())),
        "web_search" | "websearch" => input_string(&["query", "search_term"])
            .map(|query| format!("Web search · {query}"))
            .or_else(|| Some("Web search".into())),
        "" => None,
        other if other.contains('_') || other.contains('-') => {
            let words = other
                .split(['_', '-'])
                .filter(|part| !part.is_empty())
                .map(|part| {
                    let mut chars = part.chars();
                    match chars.next() {
                        Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                        None => String::new(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ");
            (!words.is_empty()).then_some(words)
        }
        other if other.bytes().all(|byte| byte.is_ascii_lowercase()) => {
            let mut chars = other.chars();
            chars
                .next()
                .map(|first| format!("{}{}", first.to_ascii_uppercase(), chars.as_str()))
        }
        _ => None,
    };
    mapped.or_else(|| {
        // No usable title: fall back to kind + path.
        let path = path_label.as_deref();
        match (kind, path) {
            (Some("Read"), Some(path)) => Some(format!("Read {path}")),
            (Some("Edit"), Some(path)) => Some(format!("Edit {path}")),
            (Some("Delete"), Some(path)) => Some(format!("Delete {path}")),
            (Some("Move"), Some(path)) => Some(format!("Move {path}")),
            (Some("Search"), _) => Some("Search".into()),
            (Some("Execute"), _) => input_string(&["command", "cmd", "description"])
                .map(|command| first_line(&command))
                .or_else(|| Some("Run command".into())),
            (Some("Fetch"), _) => Some("Fetch".into()),
            (Some("Think"), _) => Some("Thinking".into()),
            _ => None,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stalled_acp_control_request_has_a_deadline() {
        let error = async_io::block_on(wait_for_acp_with_timeout(
            std::future::pending::<agent_client_protocol::Result<()>>(),
            "test request",
            Duration::from_millis(1),
        ))
        .unwrap_err();

        assert!(error.to_string().contains("test request timed out"));
    }
    use std::sync::{Arc, Mutex, mpsc};

    fn one_provider_event(provider: ProviderId, update: SessionUpdate) -> Event {
        let (event_tx, event_rx) = mpsc::sync_channel(4);
        normalize_update(
            update,
            &EventSender {
                provider,
                event_tx,
                wake: Arc::new(|| {}),
                active_session: None,
            },
        );
        event_rx.recv().expect("update should be visible")
    }

    fn one_event(update: SessionUpdate) -> Event {
        one_provider_event(ProviderId::Cursor, update)
    }

    #[test]
    fn cursor_requests_parameterized_model_controls() {
        let capabilities = client_capabilities(ProviderId::Cursor);

        assert_eq!(
            capabilities.meta.as_ref().unwrap()["parameterizedModelPicker"],
            serde_json::json!(true)
        );
    }

    #[test]
    fn claude_opts_into_nested_transcripts_and_terminal_output() {
        let capabilities = client_capabilities(ProviderId::Claude);
        let meta = capabilities.meta.as_ref().unwrap();

        assert_eq!(meta["subagent-transcript"], serde_json::json!(true));
        assert_eq!(meta["terminal_output"], serde_json::json!(true));
        assert!(capabilities.auth.terminal);
    }

    #[test]
    fn claude_uses_provider_neutral_steering_and_goal_extensions() {
        let meta = serde_json::Map::from_iter([
            ("steering".into(), serde_json::json!({"supported": true})),
            (
                "goal".into(),
                serde_json::json!({
                    "version": 1,
                    "controlMethod": "_session/goal",
                    "actions": ["set", "pause", "resume", "clear"]
                }),
            ),
        ]);

        assert_eq!(
            session_extension_capabilities(ProviderId::Claude, Some(&meta)),
            (
                true,
                vec![
                    "set".into(),
                    "pause".into(),
                    "resume".into(),
                    "clear".into()
                ]
            )
        );
    }

    #[test]
    fn claude_subagent_text_is_attached_to_its_launch_card() {
        let event = one_provider_event(
            ProviderId::Claude,
            SessionUpdate::AgentMessageChunk(
                ContentChunk::new(ContentBlock::Text(TextContent::new("Found the bug"))).meta(
                    serde_json::Map::from_iter([(
                        "claudeCode".into(),
                        serde_json::json!({"parentToolUseId": "agent-1"}),
                    )]),
                ),
            ),
        );

        assert!(matches!(
            event,
            Event::ToolCallUpdated(ToolActivity {
                id,
                detail: Some(ToolDetail { content, .. }),
                ..
            }) if id == "agent-1" && matches!(
                content.as_slice(),
                [ToolOutput::Log { label, text }]
                    if label == "Subagent transcript" && text == "Found the bug"
            )
        ));
    }

    #[test]
    fn claude_subagent_tools_and_terminal_output_stay_in_the_nested_transcript() {
        let event = one_provider_event(
            ProviderId::Claude,
            SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new(
                    "bash-1",
                    ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
                )
                .meta(serde_json::Map::from_iter([
                    (
                        "claudeCode".into(),
                        serde_json::json!({
                            "toolName": "Bash",
                            "parentToolUseId": "agent-1"
                        }),
                    ),
                    (
                        "terminal_output".into(),
                        serde_json::json!({
                            "terminal_id": "bash-1",
                            "data": "tests passed\n"
                        }),
                    ),
                ])),
            ),
        );

        assert!(matches!(
            event,
            Event::ToolCallUpdated(ToolActivity {
                id,
                detail: Some(ToolDetail { content, .. }),
                ..
            }) if id == "agent-1" && matches!(
                content.as_slice(),
                [ToolOutput::Log { label, text }]
                    if label == "Subagent transcript" && text.contains("Bash")
                        && text.contains("tests passed")
            )
        ));
    }

    #[test]
    fn claude_tool_denials_preserve_the_reason_and_feedback() {
        let event = one_provider_event(
            ProviderId::Claude,
            SessionUpdate::ToolCallUpdate(
                ToolCallUpdate::new(
                    "bash-1",
                    ToolCallUpdateFields::new().status(ToolCallStatus::Failed),
                )
                .meta(serde_json::Map::from_iter([(
                    "claudeCode".into(),
                    serde_json::json!({
                        "toolName": "Bash",
                        "nonExecutionKind": "user-rejected",
                        "userFeedback": "Do not publish yet"
                    }),
                )])),
            ),
        );

        assert!(matches!(
            event,
            Event::ToolCallUpdated(ToolActivity {
                detail: Some(ToolDetail { content, .. }),
                ..
            }) if matches!(
                content.as_slice(),
                [ToolOutput::Log { label, text }]
                    if label == "Not executed" && text == "user-rejected · Do not publish yet"
            )
        ));
    }

    #[test]
    fn native_session_keeps_matching_filesystem_transcript_without_a_duplicate_choice() {
        let fixture = tempfile::tempdir().unwrap();
        let transcript_path = fixture.path().join("rollout.jsonl");
        let image = [0_u8, 1, 2, 3];
        let image_url = format!(
            "data:image/png;base64,{}",
            base64::engine::general_purpose::STANDARD.encode(image)
        );
        let records = [
            serde_json::json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_image", "image_url": image_url}]
                }
            }),
            serde_json::json!({
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "See attached"}
            }),
        ];
        std::fs::write(
            &transcript_path,
            records
                .into_iter()
                .map(|record| record.to_string())
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let native = SessionChoice {
            id: "same-session".into(),
            title: Some("Native session".into()),
            updated_at: None,
            started_in_editur: false,
        };

        let (sessions, external) = merge_external_sessions(
            vec![native],
            vec![ExternalSession::test_fixture(
                "same-session",
                transcript_path,
            )],
            &HashSet::new(),
        );

        assert_eq!(sessions.len(), 1);
        assert!(external.contains_key("same-session"));
        assert!(!external.contains_key("external:same-session"));

        let (event_tx, event_rx) = mpsc::sync_channel(4);
        enrich_native_session(
            "same-session",
            &external,
            &EventSender {
                provider: ProviderId::Codex,
                event_tx,
                wake: Arc::new(|| {}),
                active_session: None,
            },
        );
        assert!(matches!(
            event_rx.recv().unwrap(),
            Event::SessionTranscriptStarted
        ));
        assert!(matches!(
            event_rx.recv().unwrap(),
            Event::SessionTranscriptLoaded(messages)
                if matches!(
                    messages.as_slice(),
                    [SessionTranscriptMessage::User(text)] if text == "See attached"
                )
        ));
        assert!(matches!(
            event_rx.recv().unwrap(),
            Event::SessionTranscriptLoaded(messages)
                if matches!(
                    messages.as_slice(),
                    [SessionTranscriptMessage::Content {
                        role: ContentRole::User,
                        content: DisplayContent::Image { data: Some(bytes), .. },
                    }] if bytes.as_ref() == image
                )
        ));
        assert!(matches!(
            event_rx.recv().unwrap(),
            Event::SessionTranscriptFinished
        ));
    }

    #[test]
    fn hidden_session_history_is_account_scoped_and_migrates_legacy_once() {
        let data = tempfile::tempdir().unwrap();
        let project = Path::new("/work/project");
        let account = |provider, account_id, auth_source| ProviderAccount {
            key: AccountKey {
                provider,
                account_id,
            },
            label: "test".into(),
            auth_source,
            auto_failover: false,
        };
        let codex_legacy = account(ProviderId::Codex, 2, AuthSource::Legacy);
        let codex = session_history_path_in(data.path(), &codex_legacy, project);
        let legacy = data
            .path()
            .join("agents/codex/session-history")
            .join(codex.file_name().unwrap());
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, br#"["codex-session"]"#).unwrap();

        let codex = session_history_path_in(data.path(), &codex_legacy, project);
        let codex_work = session_history_path_in(
            data.path(),
            &account(ProviderId::Codex, 4, AuthSource::ProviderManaged),
            project,
        );
        let claude = session_history_path_in(
            data.path(),
            &account(ProviderId::Claude, 3, AuthSource::Legacy),
            project,
        );

        assert_ne!(codex_work, codex);
        assert_ne!(claude, codex_work);
        assert_ne!(claude, codex);
        assert_eq!(
            std::fs::read_to_string(&codex).unwrap(),
            r#"["codex-session"]"#
        );
        assert!(!legacy.exists());

        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, br#"["later"]"#).unwrap();
        assert_eq!(
            session_history_path_in(data.path(), &codex_legacy, project),
            codex
        );
        assert_eq!(
            std::fs::read_to_string(&codex).unwrap(),
            r#"["codex-session"]"#
        );
        assert!(legacy.exists());
    }

    #[test]
    fn active_session_is_restored_per_account_and_project() {
        let data = tempfile::tempdir().unwrap();
        let project = Path::new("/work/project");
        let cursor = ProviderAccount {
            key: AccountKey {
                provider: ProviderId::Cursor,
                account_id: 1,
            },
            label: "Current login".into(),
            auth_source: AuthSource::Legacy,
            auto_failover: false,
        };
        let codex = ProviderAccount {
            key: AccountKey {
                provider: ProviderId::Codex,
                account_id: 2,
            },
            label: "Work".into(),
            auth_source: AuthSource::ProviderManaged,
            auto_failover: false,
        };
        let cursor_path = active_session_path_in(data.path(), &cursor, project);
        let codex_path = active_session_path_in(data.path(), &codex, project);

        save_active_session(&cursor_path, "cursor-session").unwrap();
        save_active_session(&codex_path, "codex-session").unwrap();

        assert_eq!(
            load_active_session(&cursor_path).as_deref(),
            Some("cursor-session")
        );
        assert_eq!(
            load_active_session(&codex_path).as_deref(),
            Some("codex-session")
        );
        assert_ne!(cursor_path, codex_path);
        assert_ne!(
            cursor_path,
            active_session_path_in(data.path(), &cursor, Path::new("/work/other"))
        );
    }

    #[test]
    fn legacy_active_and_editur_session_metadata_migrate_to_the_account() {
        let data = tempfile::tempdir().unwrap();
        let project = Path::new("/work/project");
        let account = ProviderAccount {
            key: AccountKey {
                provider: ProviderId::Codex,
                account_id: 2,
            },
            label: "Current login".into(),
            auth_source: AuthSource::Legacy,
            auto_failover: false,
        };
        let file_name = session_history_path_in(data.path(), &account, project)
            .file_name()
            .unwrap()
            .to_owned();
        let old_root = data.path().join("agents/codex");
        for (directory, destination) in [
            (
                "active-session",
                active_session_path_in(data.path(), &account, project),
            ),
            (
                "editur-sessions",
                editur_sessions_path_in(data.path(), &account, project),
            ),
        ] {
            let legacy = old_root.join(directory).join(&file_name);
            std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
            std::fs::write(&legacy, br#"["session"]"#).unwrap();

            let resolved = match directory {
                "active-session" => active_session_path_in(data.path(), &account, project),
                _ => editur_sessions_path_in(data.path(), &account, project),
            };
            assert_eq!(resolved, destination);
            assert_eq!(std::fs::read_to_string(resolved).unwrap(), r#"["session"]"#);
            assert!(!legacy.exists());
        }
    }

    #[test]
    fn editur_session_origins_survive_a_restart() {
        let data = tempfile::tempdir().unwrap();
        let account = ProviderAccount {
            key: AccountKey {
                provider: ProviderId::Codex,
                account_id: 2,
            },
            label: "Work".into(),
            auth_source: AuthSource::ProviderManaged,
            auto_failover: false,
        };
        let path = editur_sessions_path_in(data.path(), &account, Path::new("/work/project"));
        let mut sessions = EditurSessions::load(Some(path.clone()));
        sessions.remember("editur-session").unwrap();

        assert!(EditurSessions::load(Some(path)).contains("editur-session"));
    }

    #[test]
    fn legacy_hidden_sessions_are_recovered_but_new_manual_removals_persist() {
        let data = tempfile::tempdir().unwrap();
        let path = data.path().join("hidden.json");
        std::fs::write(&path, br#"["stale-session"]"#).unwrap();

        let mut sessions = HiddenSessions::load(Some(path.clone()));
        assert!(!sessions.ids.contains("stale-session"));

        sessions.hide("manual-session".into()).unwrap();
        assert!(
            HiddenSessions::load(Some(path))
                .ids
                .contains("manual-session")
        );
    }

    #[test]
    fn claude_acp_errors_never_copy_provider_payloads() {
        let secret = "super-secret-provider-payload";

        assert_eq!(
            acp_error(ProviderId::Claude, "turn failed", &secret),
            "Claude turn failed"
        );
        assert!(acp_error(ProviderId::Cursor, "turn failed", &secret).contains(secret));
    }

    #[test]
    fn structured_turn_failures_enable_only_pinned_exhaustion_contracts() {
        let error =
            |data: serde_json::Value| agent_client_protocol::Error::internal_error().data(data);

        assert_eq!(
            classify_turn_failure(
                ProviderId::Codex,
                &error(serde_json::json!({"codexErrorInfo": "usageLimitExceeded"})),
                None,
            ),
            TurnFailureKind::UsageExhausted { reset_at: None }
        );
        assert_eq!(
            classify_turn_failure(
                ProviderId::Codex,
                &error(serde_json::json!({"message": "429 quota limit"})),
                None,
            ),
            TurnFailureKind::Other
        );
        assert_eq!(
            classify_turn_failure(
                ProviderId::Claude,
                &error(serde_json::json!({"errorKind": "rate_limit"})),
                Some("rejected"),
            ),
            TurnFailureKind::UsageExhausted { reset_at: None }
        );
        for (status, error_kind) in [
            (Some("allowed"), "rate_limit"),
            (None, "rate_limit"),
            (Some("rejected"), "billing_error"),
            (Some("rejected"), "overloaded"),
            (Some("rejected"), "max_output_tokens"),
        ] {
            assert_eq!(
                classify_turn_failure(
                    ProviderId::Claude,
                    &error(serde_json::json!({"errorKind": error_kind})),
                    status,
                ),
                TurnFailureKind::Other,
                "{status:?} {error_kind}"
            );
        }
        assert_eq!(
            classify_turn_failure(
                ProviderId::Cursor,
                &error(serde_json::json!({"codexErrorInfo": "usageLimitExceeded"})),
                Some("rejected"),
            ),
            TurnFailureKind::Other
        );

        let auth = agent_client_protocol::Error::auth_required();
        assert_eq!(
            classify_turn_failure(ProviderId::Claude, &auth, Some("rejected")),
            TurnFailureKind::Authentication
        );
        assert_eq!(
            classify_turn_failure(
                ProviderId::Claude,
                &error(serde_json::json!({"errorKind": "authentication_failed"})),
                Some("rejected"),
            ),
            TurnFailureKind::Authentication
        );
    }

    #[test]
    fn thought_and_non_text_message_chunks_are_visible() {
        assert_eq!(
            one_event(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                ContentBlock::Text(TextContent::new("checking")),
            ))),
            Event::ThoughtDelta("checking".into())
        );
        assert_eq!(
            one_event(SessionUpdate::UserMessageChunk(ContentChunk::new(
                ContentBlock::Image(ImageContent::new("YWJj", "image/png").uri("file:///shot.png")),
            ))),
            Event::ContentReceived {
                role: ContentRole::User,
                content: DisplayContent::Image {
                    mime_type: "image/png".into(),
                    uri: Some("file:///shot.png".into()),
                    encoded_bytes: 4,
                    data: Some(Arc::from(*b"abc")),
                },
            }
        );
    }

    #[test]
    fn streamed_text_is_bounded_before_entering_the_event_queue() {
        let Event::AssistantDelta(text) =
            one_event(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                ContentBlock::Text(TextContent::new("x".repeat(MAX_DETAIL_BYTES * 2))),
            )))
        else {
            panic!("expected assistant text");
        };

        assert!(text.len() <= MAX_DETAIL_BYTES);
    }

    #[test]
    fn attachment_file_uri_encodes_reserved_path_characters() {
        #[cfg(windows)]
        assert_eq!(
            attachment_uri(Path::new(r"C:\Project files\notes #1.md")),
            "file:///C:/Project%20files/notes%20%231.md"
        );
        #[cfg(not(windows))]
        assert_eq!(
            attachment_uri(Path::new("/tmp/Project files/notes #1.md")),
            "file:///tmp/Project%20files/notes%20%231.md"
        );
    }

    #[test]
    fn directory_attachment_becomes_an_acp_resource_link() {
        let project = tempfile::tempdir().unwrap();
        let directory = project.path().join("src");
        std::fs::create_dir(&directory).unwrap();
        let attachment = PromptAttachment::from_path(&directory).unwrap();

        let (content, displays) = prompt_content(
            "inspect this folder",
            &[attachment],
            AttachmentSupport {
                image: true,
                audio: true,
                embedded_context: true,
            },
        )
        .unwrap();

        assert!(matches!(
            content.as_slice(),
            [ContentBlock::Text(_), ContentBlock::ResourceLink(_)]
        ));
        assert!(matches!(
            displays.as_slice(),
            [DisplayContent::ResourceLink { name, uri, size: None, .. }]
                if name == "src" && uri.ends_with("/src")
        ));
    }

    #[test]
    fn session_title_and_available_commands_are_visible() {
        assert_eq!(
            one_event(SessionUpdate::SessionInfoUpdate(
                SessionInfoUpdate::new().title("Fix scrolling"),
            )),
            Event::SessionTitleUpdated(Some("Fix scrolling".into()))
        );
        assert_eq!(
            one_event(SessionUpdate::AvailableCommandsUpdate(
                AvailableCommandsUpdate::new(vec![
                    AvailableCommand::new("review", "Review code").input(
                        AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(
                            "optional focus"
                        ),)
                    )
                ]),
            )),
            Event::CommandsUpdated(vec![CommandChoice {
                name: "review".into(),
                description: "Review code".into(),
                input_hint: Some("optional focus".into()),
            }])
        );
    }

    #[test]
    fn tool_diffs_remain_structured_for_the_sidebar() {
        let content = vec![ToolCallContent::Diff(
            Diff::new("/tmp/file.rs", "after").old_text("before"),
        )];

        assert_eq!(
            tool_detail(None, &content, None),
            Some(ToolDetail {
                input: None,
                content: vec![ToolOutput::Diff {
                    path: "/tmp/file.rs".into(),
                    old_text: Some("before".into()),
                    new_text: "after".into(),
                }],
                output: None,
            })
        );
    }

    #[test]
    fn imported_subagent_tools_restore_the_structured_card() {
        let activity = external_tool_activity(ExternalTool {
            id: "task-1".into(),
            name: "spawn_agent".into(),
            status: Some("completed".into()),
            kind: Some("Task".into()),
            input: Some(
                serde_json::json!({
                    "prompt": "Review authentication",
                    "subagent_type": "reviewer",
                    "model": "gpt-5",
                    "agent_id": "agent-1",
                })
                .to_string(),
            ),
            output: Some("Authentication is sound".into()),
            paths: Vec::new(),
            diffs: Vec::new(),
        });

        assert!(matches!(
            activity.detail,
            Some(ToolDetail { content, .. }) if matches!(
                content.as_slice(),
                [
                    ToolOutput::Task {
                        prompt,
                        subagent_type,
                        model: Some(model),
                        agent_id: Some(agent_id),
                        ..
                    },
                    ToolOutput::Text(result),
                ] if prompt == "Review authentication"
                    && subagent_type == "reviewer"
                    && model == "gpt-5"
                    && agent_id == "agent-1"
                    && result == "Authentication is sound"
            )
        ));
    }

    #[test]
    fn imported_provider_subagents_keep_metadata_and_human_readable_output() {
        for name in ["task_v2", "spawnAgent", "Agent"] {
            let activity = external_tool_activity(ExternalTool {
                id: "task-1".into(),
                name: name.into(),
                status: Some("completed".into()),
                kind: Some("Task".into()),
                input: Some(
                    serde_json::json!({
                        "description": "Review changes",
                        "prompt": "Inspect the diff.",
                        "subagentType": "reviewer",
                    })
                    .to_string(),
                ),
                output: Some(
                    serde_json::json!({
                        "agentId": "agent-1",
                        "durationMs": 900,
                        "output": "No issues found",
                    })
                    .to_string(),
                ),
                paths: Vec::new(),
                diffs: Vec::new(),
            });

            assert!(
                matches!(
                    activity.detail,
                    Some(ToolDetail { content, .. }) if matches!(
                        content.as_slice(),
                        [
                            ToolOutput::Task {
                                agent_id: Some(agent_id),
                                duration_ms: Some(900),
                                ..
                            },
                            ToolOutput::Text(output),
                        ] if agent_id == "agent-1" && output == "No issues found"
                    )
                ),
                "{name} did not preserve imported subagent metadata and output"
            );
        }
    }

    #[test]
    fn imported_cursor_task_v2_restores_every_available_subagent_field() {
        let activity = external_tool_activity(ExternalTool {
            id: "task-1".into(),
            name: "task_v2".into(),
            status: Some("completed".into()),
            kind: Some("Task".into()),
            input: Some(
                serde_json::json!({
                    "description": "Explore sidebar and toggle plumbing",
                    "prompt": "Find the terminal toggle and sidebar layout.",
                    "subagentType": "explore",
                    "model": "cursor-grok-4.5-high-fast",
                    "name": "explore",
                })
                .to_string(),
            ),
            output: Some(
                serde_json::json!({
                    "agentId": "9a79bcc6-d9ed-4392-8782-544fdb078423",
                })
                .to_string(),
            ),
            paths: Vec::new(),
            diffs: Vec::new(),
        });

        assert_eq!(
            activity.title.as_deref(),
            Some("Subagent: Explore sidebar and toggle plumbing")
        );
        assert_eq!(activity.status.as_deref(), Some("Completed"));
        assert!(matches!(
            activity.detail,
            Some(ToolDetail {
                input: None,
                content,
                output: None,
            }) if matches!(
                content.as_slice(),
                [ToolOutput::Task {
                    description,
                    prompt,
                    subagent_type,
                    model: Some(model),
                    agent_id: Some(agent_id),
                    ..
                }] if description == "Explore sidebar and toggle plumbing"
                    && prompt == "Find the terminal toggle and sidebar layout."
                    && subagent_type == "explore"
                    && model == "cursor-grok-4.5-high-fast"
                    && agent_id == "9a79bcc6-d9ed-4392-8782-544fdb078423"
            )
        ));
    }

    #[test]
    fn imported_tool_diffs_reach_the_sidebar_as_structured_content() {
        use crate::agent::external_sessions::{ExternalDiff, ExternalTool};

        let tool = ExternalTool {
            id: "external-edit".into(),
            name: "edit_file_v2".into(),
            status: Some("completed".into()),
            kind: Some("Edit".into()),
            input: None,
            output: Some("raw fallback".into()),
            paths: vec!["src/app.rs".into()],
            diffs: vec![ExternalDiff {
                path: "src/app.rs".into(),
                old_text: Some("before".into()),
                new_text: "after".into(),
            }],
        };

        let activity = external_tool_activity(tool);

        assert!(matches!(
            activity.detail.as_ref().map(|detail| detail.content.as_slice()),
            Some([ToolOutput::Diff { path, old_text: Some(old_text), new_text }])
                if path == Path::new("src/app.rs")
                    && &**old_text == "before"
                    && &**new_text == "after"
        ));
    }

    #[test]
    fn imported_tool_diffs_keep_shared_snapshots() {
        use crate::agent::external_sessions::{ExternalDiff, ExternalTool};

        let snapshot: Arc<str> = "after".into();
        let activity = external_tool_activity(ExternalTool {
            id: "external-edit".into(),
            name: "edit_file_v2".into(),
            status: Some("completed".into()),
            kind: Some("Edit".into()),
            input: None,
            output: None,
            paths: vec!["src/app.rs".into()],
            diffs: vec![ExternalDiff {
                path: "src/app.rs".into(),
                old_text: None,
                new_text: Arc::clone(&snapshot),
            }],
        });
        let Some(ToolOutput::Diff { new_text, .. }) = activity
            .detail
            .as_ref()
            .and_then(|detail| detail.content.first())
        else {
            panic!("imported diff was not structured");
        };

        assert!(Arc::ptr_eq(new_text, &snapshot));
    }

    #[test]
    fn empty_tool_json_is_omitted_from_sidebar_details() {
        let empty = serde_json::json!({});
        let output = serde_json::json!({"totalMatches": 192, "truncated": true});

        assert_eq!(
            tool_detail(Some(&empty), &[], Some(&output)),
            Some(ToolDetail {
                input: None,
                content: Vec::new(),
                output: Some(bounded_json(&output)),
            })
        );
        assert_eq!(tool_detail(Some(&empty), &[], None), None);
    }

    #[test]
    fn single_string_tool_output_is_shown_as_text_instead_of_escaped_json() {
        let output = serde_json::json!({"content": "first line\nsecond line"});

        assert_eq!(bounded_json(&output), "first line\nsecond line");
    }

    #[test]
    fn subagent_result_fields_render_as_text_with_sibling_metadata() {
        let output = serde_json::json!({
            "agentId": "agent-1",
            "result": {"content": "No issues found"},
        });

        assert_eq!(
            subagent_output_text(Some(&output)).as_deref(),
            Some("No issues found")
        );
    }

    #[test]
    fn terminal_output_metadata_is_provider_independent() {
        let meta = serde_json::json!({
            "terminal_output_delta": {
                "terminal_id": "terminal-1",
                "data": "compiling\n"
            }
        });
        let meta = meta.as_object().unwrap();

        for provider in [ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude] {
            assert!(matches!(
                tool_logs(provider, Some(meta)).as_slice(),
                [ToolOutput::Log { label, text }]
                    if label == "Terminal output" && text == "compiling\n"
            ));
        }
    }

    #[test]
    fn cursor_extension_notifications_stay_structured() {
        let (event_tx, event_rx) = mpsc::sync_channel(4);
        let events = EventSender {
            provider: ProviderId::Cursor,
            event_tx,
            wake: Arc::new(|| {}),
            active_session: None,
        };

        normalize_cursor_notification(
            "cursor/update_todos",
            serde_json::json!({
                "toolCallId": "todos-1",
                "merge": true,
                "todos": [{"id": "a", "content": "Ship it", "status": "in_progress"}]
            }),
            &events,
        );

        assert!(matches!(
            event_rx.recv().unwrap(),
            Event::ToolCallUpdated(ToolActivity {
                id,
                detail: Some(ToolDetail { content, .. }),
                ..
            }) if id == "todos-1" && matches!(
                content.as_slice(),
                [ToolOutput::Todo { id, content, status }]
                    if id == "a" && content == "Ship it" && status == "in_progress"
            )
        ));
    }

    #[test]
    fn tool_call_kind_reaches_the_sidebar_and_updates_leave_it_unset() {
        assert!(matches!(
            one_event(SessionUpdate::ToolCall(
                ToolCall::new("run-1", "Run command").kind(ToolKind::Execute),
            )),
            Event::ToolCallUpdated(tool) if tool.kind.as_deref() == Some("Execute")
        ));
        assert!(matches!(
            one_event(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                "run-1",
                ToolCallUpdateFields::new().status(ToolCallStatus::Completed),
            ))),
            Event::ToolCallUpdated(tool) if tool.kind.is_none()
        ));
    }

    #[test]
    fn provider_neutral_task_tools_are_subagent_cards_for_every_provider() {
        for provider in [ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude] {
            let event = one_provider_event(
                provider,
                SessionUpdate::ToolCall(
                    ToolCall::new("task-live", "Task V2")
                        .status(ToolCallStatus::Completed)
                        .raw_input(serde_json::json!({
                            "description": "Explore sidebar",
                            "prompt": "Find the terminal toggle.",
                            "subagentType": "explore",
                            "model": "provider-model",
                        }))
                        .raw_output(serde_json::json!({
                            "agentId": "agent-9",
                            "durationMs": 1200,
                            "output": "Found the toggle in workspace_view.rs",
                        })),
                ),
            );

            assert!(
                matches!(
                    event,
                    Event::ToolCallUpdated(ToolActivity {
                        title: Some(title),
                        status: Some(status),
                        kind: Some(kind),
                        detail: Some(ToolDetail { content, .. }),
                        ..
                    }) if title == "Subagent: Explore sidebar"
                        && status == "Completed"
                        && kind == "Task"
                        && matches!(
                            content.as_slice(),
                            [
                                ToolOutput::Task {
                                    prompt,
                                    subagent_type,
                                    model: Some(model),
                                    agent_id: Some(agent_id),
                                    duration_ms: Some(1200),
                                    ..
                                },
                                ToolOutput::Log { label, text },
                            ] if prompt == "Find the terminal toggle."
                                && subagent_type == "explore"
                                && model == "provider-model"
                                && agent_id == "agent-9"
                                && label == "Subagent output"
                                && text == "Found the toggle in workspace_view.rs"
                        )
                ),
                "{provider:?} did not preserve the provider-neutral subagent contract"
            );
        }
    }

    #[test]
    fn provider_neutral_subagent_result_updates_merge_as_labeled_output() {
        for provider in [ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude] {
            let event = one_provider_event(
                provider,
                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                    "task-live",
                    ToolCallUpdateFields::new()
                        .title("Task V2")
                        .status(ToolCallStatus::Completed)
                        .raw_output(serde_json::json!({
                            "agentId": "agent-9",
                            "output": "Found the toggle",
                        })),
                )),
            );

            assert!(
                matches!(
                    event,
                    Event::ToolCallUpdated(ToolActivity {
                        title: None,
                        status: Some(status),
                        kind: Some(kind),
                        detail: Some(ToolDetail {
                            input: None,
                            content,
                            output: None,
                        }),
                        ..
                    }) if status == "Completed"
                        && kind == "Task"
                        && matches!(
                            content.as_slice(),
                            [ToolOutput::Log { label, text }]
                                if label == "Subagent output" && text == "Found the toggle"
                        )
                ),
                "{provider:?} did not merge a delayed subagent result"
            );
        }
    }

    #[test]
    fn codex_subagent_metadata_becomes_a_structured_task() {
        let event = one_provider_event(
            ProviderId::Codex,
            SessionUpdate::ToolCall(
                ToolCall::new("call-spawn-weather", "spawnAgent")
                    .kind(ToolKind::Other)
                    .status(ToolCallStatus::InProgress)
                    .raw_input(serde_json::json!({
                        "prompt": "Find the current weather in Paris.",
                        "receiverThreadIds": ["thread-paris", "thread-lyon"],
                        "agentsStates": {
                            "thread-paris": {
                                "status": "running",
                                "message": "Checking weather"
                            },
                            "thread-lyon": {
                                "status": "completed",
                                "message": null
                            }
                        },
                        "model": "gpt-5",
                    }))
                    .meta(serde_json::Map::from_iter([(
                        "codex".into(),
                        serde_json::json!({
                            "collaboration": {
                                "tool": "spawnAgent",
                                "senderThreadId": "thread-main",
                                "receiverThreadIds": ["thread-paris", "thread-lyon"],
                            }
                        }),
                    )])),
            ),
        );

        assert!(matches!(
            event,
            Event::ToolCallUpdated(ToolActivity {
                kind: Some(kind),
                detail: Some(ToolDetail { content, .. }),
                ..
            }) if kind == "Task" && matches!(
                content.as_slice(),
                [ToolOutput::Task {
                    prompt,
                    subagent_type,
                    model: Some(model),
                    agent_id: Some(agent_id),
                    agents,
                    ..
                }] if prompt == "Find the current weather in Paris."
                    && subagent_type == "spawnAgent"
                    && model == "gpt-5"
                    && agent_id == "thread-paris"
                    && matches!(agents.as_slice(), [
                        SubagentInfo {
                            id: paris,
                            status: Some(running),
                            message: Some(message),
                        },
                        SubagentInfo {
                            id: lyon,
                            status: Some(completed),
                            message: None,
                        },
                    ] if paris == "thread-paris"
                        && running == "running"
                        && message == "Checking weather"
                        && lyon == "thread-lyon"
                        && completed == "completed")
            )
        ));
    }

    #[test]
    fn claude_subagent_metadata_becomes_a_structured_task() {
        let event = one_provider_event(
            ProviderId::Claude,
            SessionUpdate::ToolCall(
                ToolCall::new("toolu-review", "Review changes")
                    .kind(ToolKind::Think)
                    .status(ToolCallStatus::InProgress)
                    .content(vec![ToolCallContent::Content(Content::new(
                        ContentBlock::Text(TextContent::new("Inspect the authentication flow.")),
                    ))])
                    .raw_input(serde_json::json!({
                        "description": "Review changes",
                        "prompt": "Inspect the authentication flow.",
                        "subagent_type": "Explore",
                        "model": "sonnet",
                    }))
                    .meta(serde_json::Map::from_iter([(
                        "claudeCode".into(),
                        serde_json::json!({
                            "toolName": "Agent",
                            "subagent": true,
                        }),
                    )])),
            ),
        );

        assert!(matches!(
            event,
            Event::ToolCallUpdated(ToolActivity {
                kind: Some(kind),
                detail: Some(ToolDetail { content, .. }),
                ..
            }) if kind == "Task" && matches!(
                content.as_slice(),
                [ToolOutput::Task {
                    description,
                    prompt,
                    subagent_type,
                    model: Some(model),
                    ..
                }] if description == "Review changes"
                    && prompt == "Inspect the authentication flow."
                    && subagent_type == "Explore"
                    && model == "sonnet"
            )
        ));
    }

    fn cursor_notification_tool(method: &str, params: serde_json::Value) -> ToolActivity {
        let (event_tx, event_rx) = mpsc::sync_channel(4);
        let events = EventSender {
            provider: ProviderId::Cursor,
            event_tx,
            wake: Arc::new(|| {}),
            active_session: None,
        };
        normalize_cursor_notification(method, params, &events);
        match event_rx.recv().unwrap() {
            Event::ToolCallUpdated(tool) => tool,
            event => panic!("expected a tool card, got {event:?}"),
        }
    }

    #[test]
    fn cursor_task_accepts_documented_and_custom_subagent_types() {
        for name in [
            "unspecified",
            "computer_use",
            "explore",
            "video_review",
            "browser_use",
            "shell",
            "vm_setup_helper",
        ] {
            let tool = cursor_notification_tool(
                "cursor/task",
                serde_json::json!({
                    "toolCallId": "task-1",
                    "description": "Explore codebase",
                    "prompt": "Find where authentication is handled.",
                    "subagentType": name,
                }),
            );
            assert!(matches!(
                tool.detail.as_ref().unwrap().content.as_slice(),
                [ToolOutput::Task { subagent_type, .. }] if subagent_type == name
            ));
        }

        let tool = cursor_notification_tool(
            "cursor/task",
            serde_json::json!({
                "toolCallId": "task-2",
                "description": "Review changes",
                "prompt": "Look at the diff and report issues.",
                "subagentType": {"custom": "reviewer"},
                "model": "gpt-5",
                "agentId": "agent-9",
                "durationMs": 1200,
            }),
        );
        assert_eq!(tool.kind.as_deref(), Some("Task"));
        assert_eq!(tool.status.as_deref(), Some("Completed"));
        assert!(matches!(
            tool.detail.as_ref().unwrap().content.as_slice(),
            [ToolOutput::Task {
                subagent_type,
                model: Some(model),
                agent_id: Some(agent_id),
                duration_ms: Some(1200),
                ..
            }] if subagent_type == "reviewer" && model == "gpt-5" && agent_id == "agent-9"
        ));

        // Live `agent acp` sends `custom` as an object (verified 2026-08-11).
        let tool = cursor_notification_tool(
            "cursor/task",
            serde_json::json!({
                "toolCallId": "task-3",
                "description": "Reply READY and stop",
                "prompt": "Reply with the single word READY and stop.",
                "subagentType": {"custom": {"unspecified": {}}},
                "model": "claude-opus-5-thinking-high",
                "agentId": "83634e95-e3bc-4a12-8838-e8ae4b7d9906",
                "durationMs": 5195,
            }),
        );
        assert!(matches!(
            tool.detail.as_ref().unwrap().content.as_slice(),
            [ToolOutput::Task { subagent_type, .. }] if subagent_type == "unspecified"
        ));
    }

    #[test]
    fn cursor_task_v2_tool_calls_are_live_subagent_cards_with_output() {
        let event = one_provider_event(
            ProviderId::Cursor,
            SessionUpdate::ToolCall(
                ToolCall::new("task-live", "Task V2")
                    .status(ToolCallStatus::Completed)
                    .raw_input(serde_json::json!({
                        "description": "Explore sidebar",
                        "prompt": "Find the terminal toggle.",
                        "subagentType": "explore",
                        "model": "cursor-grok-4.5-high-fast",
                    }))
                    .raw_output(serde_json::json!({
                        "agentId": "agent-9",
                        "durationMs": 1200,
                        "output": "Found the toggle in workspace_view.rs",
                    })),
            ),
        );

        assert!(matches!(
            event,
            Event::ToolCallUpdated(ToolActivity {
                title: Some(title),
                status: Some(status),
                kind: Some(kind),
                detail: Some(ToolDetail { content, .. }),
                ..
            }) if title == "Subagent: Explore sidebar"
                && status == "Completed"
                && kind == "Task"
                && matches!(
                    content.as_slice(),
                    [
                        ToolOutput::Task {
                            prompt,
                            subagent_type,
                            model: Some(model),
                            agent_id: Some(agent_id),
                            duration_ms: Some(1200),
                            ..
                        },
                        ToolOutput::Log { label, text },
                    ] if prompt == "Find the terminal toggle."
                        && subagent_type == "explore"
                        && model == "cursor-grok-4.5-high-fast"
                        && agent_id == "agent-9"
                        && label == "Subagent output"
                        && text == "Found the toggle in workspace_view.rs"
                )
        ));
    }

    #[test]
    fn cursor_generate_image_without_a_file_path_stays_a_card() {
        let tool = cursor_notification_tool(
            "cursor/generate_image",
            serde_json::json!({
                "toolCallId": "image-1",
                "description": "Minimal flat app icon",
            }),
        );

        assert!(tool.paths.is_empty());
        assert!(matches!(
            tool.detail.as_ref().unwrap().content.as_slice(),
            [ToolOutput::GeneratedImage {
                description,
                file_path: None,
                reference_image_paths,
            }] if description == "Minimal flat app icon" && reference_image_paths.is_empty()
        ));
    }

    #[test]
    fn cursor_question_extensions_validate_and_return_multi_select_answers() {
        let (request, kind) = parse_cursor_interaction(
            9,
            CursorRequest {
                method: "cursor/ask_question".into(),
                params: serde_json::json!({
                    "toolCallId": "ask-1",
                    "title": "Choose",
                    "questions": [{
                        "id": "q",
                        "prompt": "Pick any",
                        "allowMultiple": true,
                        "options": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}]
                    }]
                }),
            },
        )
        .unwrap();
        assert!(matches!(
            request.kind,
            InteractionKind::Questions { questions, .. }
                if questions[0].allow_multiple && questions[0].options.len() == 2
        ));

        let (response_tx, response_rx) = async_channel::bounded(1);
        let pending = Mutex::new(HashMap::from([(
            9,
            PendingInteraction { kind, response_tx },
        )]));
        let (event_tx, _) = mpsc::sync_channel(4);
        respond_interaction(
            9,
            InteractionResponse::Answers(vec![QuestionAnswer {
                question_id: "q".into(),
                selected_option_ids: vec!["a".into(), "b".into()],
            }]),
            &pending,
            &EventSender {
                provider: ProviderId::Cursor,
                event_tx,
                wake: Arc::new(|| {}),
                active_session: None,
            },
        );
        assert_eq!(
            response_rx.try_recv().unwrap()["outcome"]["outcome"],
            "answered"
        );
    }

    #[test]
    fn cursor_plan_extensions_preserve_the_proposal_and_acceptance() {
        let (request, kind) = parse_cursor_interaction(
            10,
            CursorRequest {
                method: "cursor/create_plan".into(),
                params: serde_json::json!({
                    "toolCallId": "plan-1",
                    "name": "Fix sidebar",
                    "overview": "Keep every update visible",
                    "plan": "1. Normalize\n2. Render",
                    "todos": [{"id": "one", "content": "Normalize", "status": "pending"}],
                    "isProject": true,
                    "phases": [{
                        "name": "UI",
                        "todos": [{"id": "two", "content": "Render", "status": "pending"}]
                    }]
                }),
            },
        )
        .unwrap();
        assert!(matches!(
            request.kind,
            InteractionKind::Plan(PlanProposal {
                name: Some(name),
                is_project: Some(true),
                phases,
                ..
            }) if name == "Fix sidebar" && phases[0].name == "UI"
        ));

        let (response_tx, response_rx) = async_channel::bounded(1);
        let pending = Mutex::new(HashMap::from([(
            10,
            PendingInteraction { kind, response_tx },
        )]));
        let (event_tx, _) = mpsc::sync_channel(4);
        respond_interaction(
            10,
            InteractionResponse::PlanAccepted,
            &pending,
            &EventSender {
                provider: ProviderId::Cursor,
                event_tx,
                wake: Arc::new(|| {}),
                active_session: None,
            },
        );
        assert_eq!(
            response_rx.try_recv().unwrap()["outcome"]["outcome"],
            "accepted"
        );
    }

    #[test]
    fn acp_elicitation_forms_preserve_field_types_and_user_values() {
        let schema = ElicitationSchema::new()
            .property(
                "token",
                StringPropertySchema::new()
                    .title("API token")
                    .meta(serde_json::Map::from_iter([(
                        "codex".into(),
                        serde_json::json!({"isSecret": true}),
                    )])),
                true,
            )
            .property(
                "environment",
                StringPropertySchema::new()
                    .enum_values(vec!["dev".into(), "prod".into()])
                    .default_value("dev"),
                true,
            )
            .integer("retries", 0, 5, false)
            .boolean("confirm", true)
            .property(
                "scopes",
                MultiSelectPropertySchema::new(vec!["read".into(), "write".into()]),
                false,
            );
        let (request, kind) = parse_elicitation(
            11,
            CreateElicitationRequest::new(
                ElicitationFormMode::new(ElicitationSessionScope::new("fake-session"), schema),
                "Configure access",
            ),
        )
        .unwrap();
        let InteractionKind::Questions { title, questions } = request.kind else {
            panic!("expected form questions")
        };
        assert_eq!(title, "Configure access");
        assert!(questions.iter().any(|question| {
            question.id == "token"
                && question.required
                && question.secret
                && question.options.is_empty()
                && question.value_kind == QuestionValueKind::String
        }));
        assert!(questions.iter().any(|question| {
            question.id == "environment"
                && question.default_values == ["dev"]
                && question.options.len() == 2
        }));
        assert!(questions.iter().any(|question| {
            question.id == "scopes"
                && question.allow_multiple
                && question.value_kind == QuestionValueKind::StringArray
        }));

        let (response_tx, response_rx) = async_channel::bounded(1);
        let pending = Mutex::new(HashMap::from([(
            11,
            PendingElicitation { kind, response_tx },
        )]));
        let (event_tx, _) = mpsc::sync_channel(4);
        respond_elicitation(
            11,
            InteractionResponse::Answers(vec![
                QuestionAnswer {
                    question_id: "token".into(),
                    selected_option_ids: vec!["secret-value".into()],
                },
                QuestionAnswer {
                    question_id: "environment".into(),
                    selected_option_ids: vec!["prod".into()],
                },
                QuestionAnswer {
                    question_id: "retries".into(),
                    selected_option_ids: vec!["3".into()],
                },
                QuestionAnswer {
                    question_id: "confirm".into(),
                    selected_option_ids: vec!["true".into()],
                },
            ]),
            &pending,
            &EventSender {
                provider: ProviderId::Codex,
                event_tx,
                wake: Arc::new(|| {}),
                active_session: None,
            },
        );
        let ElicitationAction::Accept(response) = response_rx.try_recv().unwrap() else {
            panic!("expected accepted elicitation")
        };
        let content = response.content.unwrap();
        assert_eq!(
            content["token"],
            ElicitationContentValue::String("secret-value".into())
        );
        assert_eq!(content["retries"], ElicitationContentValue::Integer(3));
        assert_eq!(content["confirm"], ElicitationContentValue::Boolean(true));
        assert!(!content.contains_key("scopes"));
    }

    #[test]
    fn oversized_elicitation_answers_stay_pending() {
        let (_, kind) = parse_elicitation(
            12,
            CreateElicitationRequest::new(
                ElicitationFormMode::new(
                    ElicitationSessionScope::new("fake-session"),
                    ElicitationSchema::new().property("token", StringPropertySchema::new(), true),
                ),
                "Configure access",
            ),
        )
        .unwrap();
        let (response_tx, response_rx) = async_channel::bounded(1);
        let pending = Mutex::new(HashMap::from([(
            12,
            PendingElicitation { kind, response_tx },
        )]));
        let (event_tx, event_rx) = mpsc::sync_channel(4);

        assert!(respond_elicitation(
            12,
            InteractionResponse::Answers(vec![QuestionAnswer {
                question_id: "token".into(),
                selected_option_ids: vec!["x".repeat(MAX_DETAIL_BYTES + 1)],
            }]),
            &pending,
            &EventSender {
                provider: ProviderId::Codex,
                event_tx,
                wake: Arc::new(|| {}),
                active_session: None,
            },
        ));
        assert!(response_rx.try_recv().is_err());
        assert!(pending.lock().unwrap().contains_key(&12));
        assert!(matches!(event_rx.try_recv(), Ok(Event::Error(_))));
    }

    #[test]
    fn elicitation_strings_enforce_patterns_and_formats() {
        let email = ElicitationPropertySchema::from(
            StringPropertySchema::email().pattern(r"^[a-z]+@example\.com$"),
        );
        assert!(elicitation_value(&email, &["hello@example.com".into()]).is_ok());
        assert!(elicitation_value(&email, &["hello@elsewhere.test".into()]).is_err());

        let date_time = ElicitationPropertySchema::from(StringPropertySchema::date_time());
        assert!(elicitation_value(&date_time, &["2024-02-29T23:59:60Z".into()]).is_ok());
        assert!(elicitation_value(&date_time, &["2023-02-29T24:00:00Z".into()]).is_err());
        assert!(elicitation_value(&date_time, &["2024-01-01T00:00:é1234".into()]).is_err());
    }

    #[test]
    fn codex_goal_metadata_becomes_visible_state() {
        let event = one_event(SessionUpdate::SessionInfoUpdate(
            SessionInfoUpdate::new().meta(serde_json::Map::from_iter([(
                "goal".into(),
                serde_json::json!({
                    "objective": "Ship the editor",
                    "status": "active",
                    "iterations": 3,
                    "lastReason": "waiting for CI",
                    "tokenBudget": 5000,
                    "tokensUsed": 120,
                    "timeUsedSeconds": 9,
                    "controlMethod": "_session/goal"
                }),
            )])),
        ));

        assert!(matches!(
            event,
            Event::GoalUpdated(Some(GoalState {
                objective,
                status,
                iterations: Some(3),
                last_reason: Some(last_reason),
                token_budget: Some(5000),
                tokens_used: Some(120),
                time_used_seconds: Some(9),
            })) if objective == "Ship the editor"
                && status == "active"
                && last_reason == "waiting for CI"
        ));
    }

    #[test]
    fn bounded_diagnostics_keep_complete_lines_and_unicode_boundaries() {
        let output = Mutex::new(String::new());
        append_bounded(&output, "hello", 64);
        append_bounded(&output, "é", 64);
        assert_eq!(*output.lock().unwrap(), "hello\né\n");
    }

    #[test]
    fn bounded_json_keeps_unicode_boundaries() {
        let detail = bounded_json(&serde_json::json!("é".repeat(MAX_DETAIL_BYTES)));

        assert!(detail.len() <= MAX_DETAIL_BYTES + '…'.len_utf8());
        assert!(detail.ends_with('…'));
    }

    #[test]
    fn protocol_debug_labels_do_not_include_payload_data() {
        let message = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "update": { "sessionUpdate": "future_update", "secret": "do not log" }
            }
        });

        assert_eq!(
            protocol_label(&message).as_deref(),
            Some("session/update (future_update)")
        );
    }

    #[test]
    fn terminal_auth_reuses_the_agent_command_and_appends_the_method_contract() {
        let agent = AcpAgentConfig::new("/managed/editur")
            .args(["--agent-process", "claude", "8", "/project"])
            .env("BASE", "one");
        let method = AuthMethodTerminal::new("claude-ai-login", "Claude Subscription")
            .args(vec![
                "--cli".into(),
                "auth".into(),
                "login".into(),
                "--claudeai".into(),
            ])
            .env(HashMap::from([("AUTH".into(), "two".into())]));

        let login = terminal_auth_config(&agent, &method);

        assert_eq!(login.command(), Path::new("/managed/editur"));
        assert_eq!(
            login.arguments(),
            [
                "--agent-process",
                "claude",
                "8",
                "/project",
                "--cli",
                "auth",
                "login",
                "--claudeai",
            ]
        );
        assert_eq!(login.environment()["BASE"], "one");
        assert_eq!(login.environment()["AUTH"], "two");
    }

    #[test]
    fn tool_display_title_humanizes_codex_wait_and_keeps_real_titles() {
        let wait = ToolActivity {
            id: "wait-1".into(),
            title: Some("wait".into()),
            status: Some("Completed".into()),
            kind: Some("Other".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: Some(
                    serde_json::json!({
                        "agentsStates": [{"id": "a"}],
                        "status": "completed"
                    })
                    .to_string(),
                ),
                content: Vec::new(),
                output: None,
            }),
        };
        assert_eq!(wait.display_title(), "Waiting for agents");

        let bare = ToolActivity {
            id: "wait-2".into(),
            title: Some("wait".into()),
            status: Some("Completed".into()),
            kind: Some("Other".into()),
            paths: Vec::new(),
            detail: None,
        };
        assert_eq!(bare.display_title(), "Waiting");

        let spawn = ToolActivity {
            id: "spawn-1".into(),
            title: Some("spawnAgent".into()),
            status: Some("InProgress".into()),
            kind: Some("Task".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: Some(serde_json::json!({ "prompt": "Review authentication" }).to_string()),
                content: Vec::new(),
                output: None,
            }),
        };
        assert_eq!(spawn.display_title(), "Spawn agent · Review authentication");

        let command = ToolActivity {
            id: "bash-1".into(),
            title: Some("Bash".into()),
            status: Some("Completed".into()),
            kind: Some("Execute".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                // Single-field raw input is collapsed to the string by bounded_json.
                input: Some("cargo test".into()),
                content: Vec::new(),
                output: None,
            }),
        };
        assert_eq!(command.display_title(), "cargo test");

        let cursor_command = ToolActivity {
            id: "cursor-bash-1".into(),
            title: Some("Run Terminal Command V2".into()),
            status: Some("Completed".into()),
            kind: Some("Execute".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: Some(
                    serde_json::json!({
                        "command": "cargo test --lib devin",
                        "cwd": "/project",
                        "options": { "timeout": 300000 }
                    })
                    .to_string(),
                ),
                content: Vec::new(),
                output: None,
            }),
        };
        assert_eq!(cursor_command.display_title(), "cargo test --lib devin");

        let snake = ToolActivity {
            id: "git-1".into(),
            title: Some("::git-stage".into()),
            status: Some("Completed".into()),
            kind: Some("Other".into()),
            paths: Vec::new(),
            detail: None,
        };
        assert_eq!(snake.display_title(), "Git Stage");

        let human = ToolActivity {
            id: "read-1".into(),
            title: Some("Read src/app.rs".into()),
            status: Some("Completed".into()),
            kind: Some("Read".into()),
            paths: vec![ToolPath::from("src/app.rs")],
            detail: None,
        };
        assert_eq!(human.display_title(), "Read src/app.rs");
    }

    #[test]
    fn cursor_edit_title_includes_the_detected_file_name() {
        let edit = ToolActivity {
            id: "edit".into(),
            title: Some("edit_file_v2".into()),
            status: Some("Completed".into()),
            kind: Some("Edit".into()),
            paths: vec!["src/app.rs".into()],
            detail: None,
        };

        assert_eq!(edit.display_title(), "Edit app.rs");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn terminal_auth_shell_arguments_cannot_inject_commands() {
        assert_eq!(
            shell_quote(std::ffi::OsStr::new("it's; complicated")).unwrap(),
            "'it'\\''s; complicated'"
        );
        assert_eq!(apple_script_string("a \\\" b"), "\"a \\\\\\\" b\"");
    }
}
