use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    io::{Read as _, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender},
    },
    thread,
};

use agent_client_protocol::schema::{ProtocolVersion, v1::*};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo, LineDirection};
use base64::Engine as _;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub use super::provider::{AuthChoice, AuthKind};
use super::provider::{
    ProviderExtensions, ProviderId, authentication_required_choices, descriptor,
    normalize_auth_methods, visible_diagnostics,
};

const EVENT_CAPACITY: usize = 512;
const COMMAND_CAPACITY: usize = 64;
const MAX_DETAIL_BYTES: usize = 64 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const MAX_CHOICES: usize = 128;
const MAX_PLAN_ITEMS: usize = 1_024;
const MAX_TOOL_PATHS: usize = 256;
const MAX_HIDDEN_SESSIONS: usize = 4_096;
/// Consecutive automatic resumes after retriable transport drops, per user turn.
const MAX_TURN_RESUMES: u64 = 2;
/// Prompt sent to resume a turn after the provider's upstream connection dropped.
pub const TURN_RESUME_PROMPT: &str = "Continue from where you left off.";
pub const MAX_PROMPT_ATTACHMENTS: usize = 8;
pub const MAX_PROMPT_ATTACHMENT_BYTES: u64 = 10 * 1024 * 1024;
pub const MAX_PROMPT_ATTACHMENT_TOTAL_BYTES: u64 = 20 * 1024 * 1024;

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
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ContentRole {
    User,
    Assistant,
    Thought,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DisplayContent {
    Image {
        mime_type: String,
        uri: Option<String>,
        encoded_bytes: usize,
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
    },
    SessionReady {
        current_mode: Option<String>,
        modes: Vec<ModeChoice>,
        config_options: Vec<ConfigChoice>,
    },
    SessionsUpdated(Vec<SessionChoice>),
    SessionLoading {
        title: Option<String>,
    },
    SessionLoadFailed,
    SessionLoaded {
        current_mode: Option<String>,
        modes: Vec<ModeChoice>,
        config_options: Vec<ConfigChoice>,
    },
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
pub struct PlanItem {
    pub content: String,
    pub status: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
            self.detail.as_ref().and_then(|detail| detail.input.as_deref()),
        )
    }
}

/// A file the tool touched, with the optional line number the agent reported
/// for that location.
#[derive(Clone, Debug, Eq, PartialEq)]
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDetail {
    pub input: Option<String>,
    pub content: Vec<ToolOutput>,
    pub output: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolOutput {
    Text(String),
    Content(DisplayContent),
    Diff {
        path: PathBuf,
        old_text: Option<String>,
        new_text: String,
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
        duration_ms: Option<u64>,
    },
    GeneratedImage {
        description: String,
        file_path: Option<PathBuf>,
        reference_image_paths: Vec<PathBuf>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionRequest {
    pub request_id: u64,
    pub tool_call_id: String,
    pub action: String,
    pub options: Vec<PermissionChoice>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PermissionChoice {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InteractionRequest {
    pub request_id: u64,
    pub tool_call_id: String,
    pub kind: InteractionKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum InteractionKind {
    Questions {
        title: String,
        questions: Vec<Question>,
    },
    Plan(PlanProposal),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Question {
    pub id: String,
    pub prompt: String,
    pub options: Vec<QuestionOption>,
    pub allow_multiple: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlanProposal {
    pub name: Option<String>,
    pub overview: Option<String>,
    pub plan: String,
    pub todos: Vec<PlanItem>,
    pub is_project: Option<bool>,
    pub phases: Vec<PlanPhase>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    Connect,
    Authenticate(String),
    NewSession,
    RefreshSessions,
    LoadSession(String),
    RemoveSession(String),
    SetMode(String),
    SetConfig {
        id: String,
        value: ConfigValue,
    },
    SetRunEverything(bool),
    Prompt(String),
    PromptWithAttachments {
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

pub struct AgentController {
    commands: async_channel::Sender<Command>,
    events: Receiver<Event>,
    worker: Option<thread::JoinHandle<()>>,
}

impl AgentController {
    pub fn start(provider: ProviderId, project_root: PathBuf) -> Self {
        Self::start_launch(
            provider,
            managed_session_startup(provider, &project_root, None),
            project_root,
            Launch::Managed,
            Arc::new(|| {}),
        )
    }

    pub fn start_with_wake(
        provider: ProviderId,
        project_root: PathBuf,
        preferred_session: Option<String>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self::start_launch(
            provider,
            managed_session_startup(provider, &project_root, preferred_session),
            project_root,
            Launch::Managed,
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
        Self::start_launch(
            provider,
            SessionStartup {
                history,
                active_session: None,
                preferred_session: None,
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
        Self::start_launch(
            ProviderId::Cursor,
            SessionStartup {
                history,
                active_session: None,
                preferred_session: Some(preferred_session),
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
        let event_tx = EventSender {
            provider,
            event_tx,
            wake,
            active_session: session_startup.active_session.clone(),
        };
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
                )
            })
            .expect("failed to start Editur agent controller thread");
        Self {
            commands: command_tx,
            events: event_rx,
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
        if let Some(worker) = self.worker.take() {
            loop {
                match self.commands.try_send(Command::Shutdown) {
                    Ok(()) | Err(async_channel::TrySendError::Closed(_)) => break,
                    Err(async_channel::TrySendError::Full(_)) => {
                        self.events.try_iter().for_each(drop);
                        if worker.is_finished() {
                            break;
                        }
                        thread::park_timeout(std::time::Duration::from_millis(1));
                    }
                }
            }
            while !worker.is_finished() {
                self.events.try_iter().for_each(drop);
                thread::park_timeout(std::time::Duration::from_millis(1));
            }
            let _ = worker.join();
        }
    }
}

enum Launch {
    Managed,
    Process(AcpAgentConfig),
}

struct SessionStartup {
    history: Option<PathBuf>,
    active_session: Option<PathBuf>,
    preferred_session: Option<String>,
}

#[derive(Clone)]
struct EventSender {
    provider: ProviderId,
    event_tx: SyncSender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
    active_session: Option<PathBuf>,
}

fn run_thread(
    provider: ProviderId,
    project_root: PathBuf,
    launch: Launch,
    commands: async_channel::Receiver<Command>,
    debug_commands: async_channel::Sender<Command>,
    events: EventSender,
    session_startup: SessionStartup,
) {
    let system_terminal = matches!(launch, Launch::Managed);
    let (config, _managed_tree) = match launch {
        Launch::Managed => match managed_config(provider, &project_root, &events) {
            Ok(config) => (config.0, Some(config.1)),
            Err(error) => {
                send_event(
                    &events,
                    Event::ConnectionChanged(ConnectionState::Failed(error)),
                );
                return;
            }
        },
        Launch::Process(config) => (config, None),
    };
    send_event(&events, Event::ConnectionChanged(ConnectionState::Starting));
    let diagnostics = Arc::new(Mutex::new(String::new()));
    let debug_diagnostics = Arc::clone(&diagnostics);
    let protocol_debug = std::env::var("EDITUR_LOG").as_deref() == Ok("debug");
    let auth_config = config.clone();
    let internal_commands = debug_commands.clone();
    let agent = AcpAgent::new(config).with_debug(move |line, direction| {
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
    let shutdown = Arc::new(AtomicBool::new(false));
    let result = async_io::block_on(run_connection(
        (agent, auth_config, system_terminal, internal_commands),
        provider,
        project_root,
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
    provider: ProviderId,
    project_root: &std::path::Path,
    events: &EventSender,
) -> Result<(AcpAgentConfig, ManagedTree), String> {
    let data_dir = crate::data_dir()?;
    super::provider::prepare(provider, &data_dir, |progress| {
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
        .arg(provider.as_str())
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

async fn run_connection(
    (agent, auth_config, system_terminal, internal_commands): (
        AcpAgent,
        AcpAgentConfig,
        bool,
        async_channel::Sender<Command>,
    ),
    provider: ProviderId,
    project_root: PathBuf,
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
    let next_permission = Arc::new(AtomicU64::new(1));
    let mut hidden_sessions = HiddenSessions::load(session_startup.history);
    let preferred_session = session_startup.preferred_session;
    agent_client_protocol::Client
        .builder()
        .name("editur")
        .on_receive_notification(
            {
                let events = events.clone();
                async move |notification: SessionNotification, _connection| {
                    normalize_update(notification.update, &events);
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
            let shutdown = Arc::clone(&shutdown);
            async move {
                let client_capabilities = ClientCapabilities::new().session(
                    ClientSessionCapabilities::new().config_options(
                        SessionConfigOptionsCapabilities::new()
                            .boolean(BooleanConfigOptionCapabilities::new()),
                    ),
                );
                let client_capabilities = if provider == ProviderId::Claude {
                    client_capabilities.auth(AuthCapabilities::new().terminal(true))
                } else {
                    client_capabilities
                };
                let initialized = connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(client_capabilities)
                            .client_info(Implementation::new("editur", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
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
                send_event(
                    &events,
                    Event::Capabilities {
                        history: supports_history,
                        allow_run_everything: extensions == ProviderExtensions::Cursor,
                    },
                );
                let (mut session_id, mut sessions) = match start_session(
                    &connection,
                    &project_root,
                    &events,
                    supports_history,
                    &hidden_sessions.ids,
                    preferred_session.as_deref(),
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
                            let authenticated = connection
                                .send_request(AuthenticateRequest::new(method.id.clone()))
                                .block_task()
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
                                preferred_session.as_deref(),
                            )
                            .await
                            {
                                Ok((session, listed)) => {
                                    session_id = Some(session);
                                    sessions = listed;
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
                                match new_session(&connection, &project_root, &events).await {
                                    Ok(session) => {
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
                            if supports_history {
                                match list_sessions(
                                    &connection,
                                    &project_root,
                                    &events,
                                    &hidden_sessions.ids,
                                )
                                .await
                                {
                                    Ok(listed) => sessions = listed,
                                    Err(error) => send_event(
                                        &events,
                                        Event::Error(acp_error(
                                            provider,
                                            "cannot list sessions",
                                            &error,
                                        )),
                                    ),
                                }
                            }
                        }
                        Command::RemoveSession(id) => {
                            if !sessions.iter().any(|session| session.id == id) {
                                send_event(&events, Event::Error("unknown session".into()));
                                continue;
                            }
                            match hidden_sessions.hide(id.clone()) {
                                Ok(()) => {
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
                            let Some(session) = sessions.iter().find(|session| session.id == id)
                            else {
                                send_event(&events, Event::Error("unknown session".into()));
                                continue;
                            };
                            match load_session(&connection, &project_root, session, &events).await {
                                Ok(loaded) => session_id = Some(loaded),
                                Err(error) => {
                                    send_event(&events, Event::SessionLoadFailed);
                                    if session_not_found(&error) {
                                        if let Err(error) = hidden_sessions.hide(id.clone()) {
                                            send_event(&events, Event::Error(error));
                                        }
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
                            match connection
                                .send_request(SetSessionModeRequest::new(session, mode_id.clone()))
                                .block_task()
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
                            let response = connection
                                .send_request(SetSessionConfigOptionRequest::new(
                                    session, id, value,
                                ))
                                .block_task()
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
                        Command::Prompt(text) => {
                            turn_resume.attempts.store(0, Ordering::Release);
                            send_prompt(
                                &connection,
                                &events,
                                &active,
                                session_id.clone(),
                                text,
                                Vec::new(),
                                attachment_support,
                                turn_resume.clone(),
                            )?;
                        }
                        Command::PromptWithAttachments { text, attachments } => {
                            turn_resume.attempts.store(0, Ordering::Release);
                            send_prompt(
                                &connection,
                                &events,
                                &active,
                                session_id.clone(),
                                text,
                                attachments,
                                attachment_support,
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
                                Vec::new(),
                                attachment_support,
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
                            respond_interaction(request_id, response, &interactions, &events);
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
                                preferred_session.as_deref(),
                            )
                            .await
                            {
                                Ok((session, listed)) => {
                                    session_id = Some(session);
                                    sessions = listed;
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

#[expect(clippy::too_many_arguments)]
fn send_prompt(
    connection: &ConnectionTo<Agent>,
    events: &EventSender,
    active: &Arc<AtomicBool>,
    session: Option<SessionId>,
    text: String,
    attachments: Vec<PromptAttachment>,
    attachment_support: AttachmentSupport,
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
    send_event(events, Event::UserMessage(text));
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
                    let message =
                        acp_error(events_for_result.provider, "agent turn failed", &error);
                    let resuming = resume.enabled
                        && is_retriable_transport_error(&error)
                        && resume.attempts.fetch_add(1, Ordering::AcqRel) < MAX_TURN_RESUMES
                        && resume.commands.try_send(Command::ResumeTurn).is_ok();
                    send_event(
                        &events_for_result,
                        Event::Error(if resuming {
                            format!(
                                "{message}\n\nThe connection dropped mid-turn; progress is \
                                 preserved in this session — resuming automatically."
                            )
                        } else {
                            message
                        }),
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
                let data = base64::engine::general_purpose::STANDARD.encode(bytes);
                displays.push(DisplayContent::Image {
                    mime_type: mime_type.into(),
                    uri: Some(uri.clone()),
                    encoded_bytes: data.len(),
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

async fn new_session(
    connection: &ConnectionTo<Agent>,
    project_root: &std::path::Path,
    events: &EventSender,
) -> agent_client_protocol::Result<SessionId> {
    let response = connection
        .send_request(NewSessionRequest::new(project_root))
        .block_task()
        .await?;
    let session_id = response.session_id;
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

async fn start_session(
    connection: &ConnectionTo<Agent>,
    project_root: &std::path::Path,
    events: &EventSender,
    supports_history: bool,
    hidden_sessions: &HashSet<String>,
    preferred_session: Option<&str>,
) -> agent_client_protocol::Result<(SessionId, Vec<SessionChoice>)> {
    let sessions = if supports_history {
        list_sessions(connection, project_root, events, hidden_sessions)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if let Some(session) = preferred_session
        .and_then(|id| sessions.iter().find(|session| session.id == id))
        .or_else(|| sessions.first())
        && let Ok(session_id) = load_session(connection, project_root, session, events).await
    {
        return Ok((session_id, sessions));
    }
    let session_id = new_session(connection, project_root, events).await?;
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
) -> agent_client_protocol::Result<Vec<SessionChoice>> {
    let response = connection
        .send_request(ListSessionsRequest::new().cwd(project_root))
        .block_task()
        .await?;
    let mut sessions = response
        .sessions
        .into_iter()
        .filter(|session| {
            session.cwd == project_root && !hidden_sessions.contains(session.session_id.0.as_ref())
        })
        .map(|session| SessionChoice {
            id: session.session_id.0.to_string(),
            title: session.title,
            updated_at: session.updated_at,
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    sessions.truncate(MAX_CHOICES);
    send_event(events, Event::SessionsUpdated(sessions.clone()));
    Ok(sessions)
}

struct HiddenSessions {
    path: Option<PathBuf>,
    ids: HashSet<String>,
}

impl HiddenSessions {
    fn load(path: Option<PathBuf>) -> Self {
        let ids = path
            .as_ref()
            .and_then(|path| {
                fs::symlink_metadata(path)
                    .ok()
                    .filter(|metadata| {
                        metadata.is_file()
                            && !metadata.file_type().is_symlink()
                            && metadata.len() <= MAX_DETAIL_BYTES as u64
                    })
                    .and_then(|_| fs::read(path).ok())
            })
            .and_then(|bytes| serde_json::from_slice::<Vec<String>>(&bytes).ok())
            .unwrap_or_default()
            .into_iter()
            .take(MAX_HIDDEN_SESSIONS)
            .collect();
        Self { path, ids }
    }

    fn hide(&mut self, id: String) -> Result<(), String> {
        if self.ids.len() >= MAX_HIDDEN_SESSIONS && !self.ids.contains(&id) {
            return Err("too many sessions have been removed from history".into());
        }
        let Some(path) = &self.path else {
            return Err("cannot determine where to save session history".into());
        };
        if !self.ids.insert(id.clone()) {
            return Ok(());
        }
        if let Err(error) = save_hidden_sessions(path, &self.ids) {
            self.ids.remove(&id);
            return Err(error);
        }
        Ok(())
    }
}

fn session_history_path(provider: ProviderId, project_root: &std::path::Path) -> Option<PathBuf> {
    crate::data_dir()
        .ok()
        .map(|directory| session_history_path_in(&directory, provider, project_root))
}

fn managed_session_startup(
    provider: ProviderId,
    project_root: &Path,
    preferred_session: Option<String>,
) -> SessionStartup {
    let history = session_history_path(provider, project_root);
    let active_session = crate::data_dir()
        .ok()
        .map(|directory| active_session_path_in(&directory, provider, project_root));
    let preferred_session =
        preferred_session.or_else(|| active_session.as_deref().and_then(load_active_session));
    SessionStartup {
        history,
        active_session,
        preferred_session,
    }
}

fn session_history_path_in(data_dir: &Path, provider: ProviderId, project_root: &Path) -> PathBuf {
    let digest = Sha256::digest(project_root.as_os_str().as_encoded_bytes());
    let mut name = String::with_capacity(digest.len() * 2 + 5);
    for byte in digest {
        write!(&mut name, "{byte:02x}").expect("writing to a String cannot fail");
    }
    name.push_str(".json");
    let destination = super::provision::provider_root(data_dir, provider)
        .join("session-history")
        .join(&name);
    if provider == ProviderId::Cursor && !destination.exists() {
        let legacy = data_dir.join("agents/session-history").join(name);
        let migratable = fs::symlink_metadata(&legacy).ok().is_some_and(|metadata| {
            metadata.is_file()
                && !metadata.file_type().is_symlink()
                && metadata.len() <= MAX_DETAIL_BYTES as u64
        });
        if migratable
            && destination
                .parent()
                .is_some_and(|parent| fs::create_dir_all(parent).is_ok())
        {
            let _ = fs::rename(legacy, &destination);
        }
    }
    destination
}

fn active_session_path_in(data_dir: &Path, provider: ProviderId, project_root: &Path) -> PathBuf {
    let history = session_history_path_in(data_dir, provider, project_root);
    super::provision::provider_root(data_dir, provider)
        .join("active-session")
        .join(
            history
                .file_name()
                .expect("session history has a file name"),
        )
}

fn load_active_session(path: &Path) -> Option<String> {
    HiddenSessions::load(Some(path.to_path_buf()))
        .ids
        .into_iter()
        .next()
}

fn save_active_session(path: &Path, id: &str) -> Result<(), String> {
    save_hidden_sessions(path, &HashSet::from([id.to_owned()]))
}

fn save_hidden_sessions(path: &std::path::Path, ids: &HashSet<String>) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "session history path has no parent directory".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create session history directory: {error}"))?;
    let mut ids = ids.iter().collect::<Vec<_>>();
    ids.sort_unstable();
    let bytes = serde_json::to_vec(&ids)
        .map_err(|error| format!("cannot encode session history: {error}"))?;
    if bytes.len() > MAX_DETAIL_BYTES {
        return Err("too many sessions have been removed from history".into());
    }
    let mut staged = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| format!("cannot stage session history: {error}"))?;
    staged
        .write_all(&bytes)
        .and_then(|()| staged.flush())
        .and_then(|()| staged.as_file().sync_all())
        .map_err(|error| format!("cannot write session history: {error}"))?;
    staged
        .persist(path)
        .map_err(|error| format!("cannot save session history: {}", error.error))?;
    Ok(())
}

async fn load_session(
    connection: &ConnectionTo<Agent>,
    project_root: &std::path::Path,
    session: &SessionChoice,
    events: &EventSender,
) -> agent_client_protocol::Result<SessionId> {
    send_event(
        events,
        Event::SessionLoading {
            title: session.title.clone(),
        },
    );
    let session_id = SessionId::new(session.id.clone());
    let response = connection
        .send_request(LoadSessionRequest::new(session_id.clone(), project_root))
        .block_task()
        .await?;
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

struct PendingInteraction {
    kind: PendingInteractionKind,
    response_tx: async_channel::Sender<serde_json::Value>,
}

enum PendingInteractionKind {
    Questions(HashMap<String, (HashSet<String>, bool)>),
    Plan,
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

fn normalize_update(update: SessionUpdate, events: &EventSender) {
    match update {
        SessionUpdate::UserMessageChunk(chunk) => {
            normalize_content(ContentRole::User, chunk.content, events)
        }
        SessionUpdate::AgentMessageChunk(chunk) => {
            normalize_content(ContentRole::Assistant, chunk.content, events)
        }
        SessionUpdate::AgentThoughtChunk(chunk) => {
            normalize_content(ContentRole::Thought, chunk.content, events)
        }
        SessionUpdate::Plan(plan) => send_event(
            events,
            Event::PlanUpdated(
                plan.entries
                    .into_iter()
                    .take(MAX_PLAN_ITEMS)
                    .map(|entry| PlanItem {
                        content: entry.content,
                        status: format!("{:?}", entry.status),
                    })
                    .collect(),
            ),
        ),
        SessionUpdate::ToolCall(tool) => send_event(
            events,
            Event::ToolCallUpdated(ToolActivity {
                id: tool.tool_call_id.0.to_string(),
                title: Some(tool.title),
                status: Some(format!("{:?}", tool.status)),
                kind: Some(format!("{:?}", tool.kind)),
                paths: tool_paths(&tool.locations, &tool.content),
                detail: tool_detail(
                    tool.raw_input.as_ref(),
                    &tool.content,
                    tool.raw_output.as_ref(),
                ),
            }),
        ),
        SessionUpdate::ToolCallUpdate(update) => {
            let fields = update.fields;
            let content = fields.content.as_deref().unwrap_or_default();
            let locations = fields.locations.as_deref().unwrap_or_default();
            send_event(
                events,
                Event::ToolCallUpdated(ToolActivity {
                    id: update.tool_call_id.0.to_string(),
                    title: fields.title,
                    status: fields.status.map(|status| format!("{status:?}")),
                    kind: fields.kind.map(|kind| format!("{kind:?}")),
                    paths: tool_paths(locations, content),
                    detail: tool_detail(
                        fields.raw_input.as_ref(),
                        content,
                        fields.raw_output.as_ref(),
                    ),
                }),
            );
        }
        SessionUpdate::UsageUpdate(usage) => send_event(
            events,
            Event::UsageUpdated {
                used: usage.used,
                size: usage.size,
                cost: usage
                    .cost
                    .map(|cost| format!("{} {}", cost.amount, cost.currency)),
            },
        ),
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
                        .take(MAX_PLAN_ITEMS)
                        .map(|todo| ToolOutput::Todo {
                            id: todo.id,
                            content: todo.content,
                            status: todo.status,
                        })
                        .collect(),
                    output: None,
                }),
            })
        }
        "cursor/task" => {
            serde_json::from_value::<CursorTaskUpdate>(params).map(|update| ToolActivity {
                id: update.tool_call_id,
                title: Some(format!("Subagent: {}", update.description)),
                status: None,
                kind: Some("Task".into()),
                paths: Vec::new(),
                detail: Some(ToolDetail {
                    input: None,
                    content: vec![ToolOutput::Task {
                        description: update.description,
                        prompt: update.prompt,
                        subagent_type: update.subagent_type.into(),
                        model: update.model,
                        agent_id: update.agent_id,
                        duration_ms: update.duration_ms,
                    }],
                    output: None,
                }),
            })
        }
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
                        description: update.description,
                        file_path: update.file_path,
                        reference_image_paths: update
                            .reference_image_paths
                            .into_iter()
                            .take(MAX_TOOL_PATHS)
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

enum NormalizedContent {
    Text(String),
    Display(DisplayContent),
}

fn normalize_display_content(content: ContentBlock) -> Option<NormalizedContent> {
    Some(match content {
        ContentBlock::Text(text) => NormalizedContent::Text(text.text),
        ContentBlock::Image(image) => NormalizedContent::Display(DisplayContent::Image {
            mime_type: image.mime_type,
            uri: image.uri,
            encoded_bytes: image.data.len(),
        }),
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
                    text: resource.text,
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
        .filter_map(|content| match content {
            ToolCallContent::Content(content) => {
                match normalize_display_content(content.content.clone())? {
                    NormalizedContent::Text(text) => Some(ToolOutput::Text(text)),
                    NormalizedContent::Display(content) => Some(ToolOutput::Content(content)),
                }
            }
            ToolCallContent::Diff(diff) => Some(ToolOutput::Diff {
                path: diff.path.clone(),
                old_text: diff.old_text.clone(),
                new_text: diff.new_text.clone(),
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
    let mut detail = value
        .as_object()
        .filter(|fields| fields.len() == 1)
        .and_then(|fields| fields.values().next())
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            serde_json::to_string_pretty(value).unwrap_or_else(|_| "<unavailable>".into())
        });
    if detail.len() > MAX_DETAIL_BYTES {
        let mut end = MAX_DETAIL_BYTES;
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
        "spawn_agent" | "spawn_agents" => input_string(&["prompt", "description", "task"])
            .map(|prompt| format!("Spawn agent · {}", first_line(&prompt)))
            .or_else(|| Some("Spawn agent".into())),
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
        "edit" | "write" | "write_file" | "edit_file" | "search_replace" => path_label
            .clone()
            .or_else(input_path_label)
            .map(|path| format!("Edit {path}"))
            .or_else(|| Some("Edit file".into())),
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
            chars.next().map(|first| {
                format!("{}{}", first.to_ascii_uppercase(), chars.as_str())
            })
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
    use std::sync::{Arc, Mutex, mpsc};

    fn one_event(update: SessionUpdate) -> Event {
        let (event_tx, event_rx) = mpsc::sync_channel(4);
        normalize_update(
            update,
            &EventSender {
                provider: ProviderId::Cursor,
                event_tx,
                wake: Arc::new(|| {}),
                active_session: None,
            },
        );
        event_rx.recv().expect("update should be visible")
    }

    #[test]
    fn hidden_session_history_is_provider_scoped_and_migrates_cursor_once() {
        let data = tempfile::tempdir().unwrap();
        let project = Path::new("/work/project");
        let codex = session_history_path_in(data.path(), ProviderId::Codex, project);
        let legacy = data
            .path()
            .join("agents/session-history")
            .join(codex.file_name().unwrap());
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, br#"["cursor-session"]"#).unwrap();

        let cursor = session_history_path_in(data.path(), ProviderId::Cursor, project);
        let claude = session_history_path_in(data.path(), ProviderId::Claude, project);

        assert_ne!(cursor, codex);
        assert_ne!(claude, cursor);
        assert_ne!(claude, codex);
        assert_eq!(
            std::fs::read_to_string(&cursor).unwrap(),
            r#"["cursor-session"]"#
        );
        assert!(!legacy.exists());

        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, br#"["later"]"#).unwrap();
        assert_eq!(
            session_history_path_in(data.path(), ProviderId::Cursor, project),
            cursor
        );
        assert_eq!(
            std::fs::read_to_string(&cursor).unwrap(),
            r#"["cursor-session"]"#
        );
        assert!(legacy.exists());
    }

    #[test]
    fn active_session_is_restored_per_provider_and_project() {
        let data = tempfile::tempdir().unwrap();
        let project = Path::new("/work/project");
        let cursor = active_session_path_in(data.path(), ProviderId::Cursor, project);
        let codex = active_session_path_in(data.path(), ProviderId::Codex, project);

        save_active_session(&cursor, "cursor-session").unwrap();
        save_active_session(&codex, "codex-session").unwrap();

        assert_eq!(
            load_active_session(&cursor).as_deref(),
            Some("cursor-session")
        );
        assert_eq!(
            load_active_session(&codex).as_deref(),
            Some("codex-session")
        );
        assert_ne!(cursor, codex);
        assert_ne!(
            cursor,
            active_session_path_in(data.path(), ProviderId::Cursor, Path::new("/work/other"))
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
                },
            }
        );
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
            .args(["--agent-process", "claude", "/project"])
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
