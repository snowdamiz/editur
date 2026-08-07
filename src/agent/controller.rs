use std::{
    collections::{HashMap, HashSet},
    fmt::Write as _,
    fs,
    io::Write as _,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{Receiver, SyncSender},
    },
    thread,
};

use agent_client_protocol::schema::{ProtocolVersion, v1::*};
use agent_client_protocol::{AcpAgent, AcpAgentConfig, Agent, ConnectionTo, LineDirection};
use serde::Deserialize;
use sha2::{Digest, Sha256};

const EVENT_CAPACITY: usize = 512;
const COMMAND_CAPACITY: usize = 64;
const MAX_DETAIL_BYTES: usize = 64 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const MAX_CHOICES: usize = 128;
const MAX_PLAN_ITEMS: usize = 1_024;
const MAX_TOOL_PATHS: usize = 256;
const MAX_HIDDEN_SESSIONS: usize = 4_096;

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
pub struct AuthChoice {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
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
    SessionReady {
        current_mode: Option<String>,
        modes: Vec<ModeChoice>,
        config_options: Vec<ConfigChoice>,
    },
    SessionsUpdated(Vec<SessionChoice>),
    SessionLoading {
        title: Option<String>,
    },
    SessionLoaded {
        current_mode: Option<String>,
        modes: Vec<ModeChoice>,
        config_options: Vec<ConfigChoice>,
    },
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
    pub paths: Vec<PathBuf>,
    pub detail: Option<ToolDetail>,
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
        file_path: PathBuf,
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
}

pub struct AgentController {
    commands: async_channel::Sender<Command>,
    events: Receiver<Event>,
    worker: Option<thread::JoinHandle<()>>,
}

impl AgentController {
    pub fn start(project_root: PathBuf) -> Self {
        let history = session_history_path(&project_root);
        Self::start_launch(project_root, Launch::Managed, Arc::new(|| {}), history)
    }

    pub fn start_with_wake(project_root: PathBuf, wake: impl Fn() + Send + Sync + 'static) -> Self {
        let history = session_history_path(&project_root);
        Self::start_launch(project_root, Launch::Managed, Arc::new(wake), history)
    }

    #[doc(hidden)]
    pub fn start_process(project_root: PathBuf, command: PathBuf, args: Vec<String>) -> Self {
        let history = Some(project_root.join(".editur-test-hidden-sessions.json"));
        Self::start_launch(
            project_root,
            Launch::Process(AcpAgentConfig::new(command).args(args)),
            Arc::new(|| {}),
            history,
        )
    }

    fn start_launch(
        project_root: PathBuf,
        launch: Launch,
        wake: Arc<dyn Fn() + Send + Sync>,
        history: Option<PathBuf>,
    ) -> Self {
        let (command_tx, command_rx) = async_channel::bounded(COMMAND_CAPACITY);
        let debug_commands = command_tx.clone();
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(EVENT_CAPACITY);
        let event_tx = EventSender { event_tx, wake };
        let worker = thread::Builder::new()
            .name("editur-agent".into())
            .spawn(move || {
                run_thread(
                    project_root,
                    launch,
                    command_rx,
                    debug_commands,
                    event_tx,
                    history,
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

#[derive(Clone)]
struct EventSender {
    event_tx: SyncSender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

fn run_thread(
    project_root: PathBuf,
    launch: Launch,
    commands: async_channel::Receiver<Command>,
    debug_commands: async_channel::Sender<Command>,
    events: EventSender,
    history: Option<PathBuf>,
) {
    let (config, _managed_tree) = match launch {
        Launch::Managed => match managed_config(&project_root, &events) {
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
    let agent = AcpAgent::new(config).with_debug(move |line, direction| {
        if direction == LineDirection::Stderr {
            append_bounded(&debug_diagnostics, line, MAX_DIAGNOSTIC_BYTES);
        } else if direction == LineDirection::Stdout {
            match serde_json::from_str::<serde_json::Value>(line) {
                Ok(message) if protocol_debug => {
                    if let Some(label) = protocol_label(&message) {
                        eprintln!("editur: ACP <- {label}");
                    }
                }
                Ok(_) => {}
                Err(_) => {
                    let _ = debug_commands.try_send(Command::TransportFailed(
                        "Cursor Agent wrote malformed JSON to stdout".into(),
                    ));
                }
            }
        }
    });
    let shutdown = Arc::new(AtomicBool::new(false));
    let result = async_io::block_on(run_connection(
        agent,
        project_root,
        commands,
        events.clone(),
        Arc::clone(&shutdown),
        history,
    ));
    if shutdown.load(Ordering::Acquire) {
        send_event(
            &events,
            Event::ConnectionChanged(ConnectionState::Disconnected),
        );
    } else if let Err(error) = result {
        let diagnostics = diagnostics
            .lock()
            .map(|text| text.clone())
            .unwrap_or_default();
        send_event(
            &events,
            Event::ProcessExited {
                error: connection_error(&error, &diagnostics),
                diagnostics,
            },
        );
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

fn managed_config(
    project_root: &std::path::Path,
    events: &EventSender,
) -> Result<(AcpAgentConfig, ManagedTree), String> {
    let manifest = super::provision::embedded_manifest()?;
    let data_dir = crate::syntax::data_dir()?;
    super::provision::ensure(&manifest, &data_dir, |progress| {
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
    agent: AcpAgent,
    project_root: PathBuf,
    commands: async_channel::Receiver<Command>,
    events: EventSender,
    shutdown: Arc<AtomicBool>,
    history: Option<PathBuf>,
) -> agent_client_protocol::Result<()> {
    let active = Arc::new(AtomicBool::new(false));
    let auto_approve_permissions = Arc::new(AtomicBool::new(false));
    let permissions = Arc::new(Mutex::new(HashMap::new()));
    let interactions = Arc::new(Mutex::new(HashMap::new()));
    let next_permission = Arc::new(AtomicU64::new(1));
    let mut hidden_sessions = HiddenSessions::load(history);
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
                    normalize_cursor_notification(
                        notification.method(),
                        notification.params().clone(),
                        &events,
                    );
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
                let initialized = connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
                            .client_capabilities(
                                ClientCapabilities::new().session(
                                    ClientSessionCapabilities::new().config_options(
                                        SessionConfigOptionsCapabilities::new()
                                            .boolean(BooleanConfigOptionCapabilities::new()),
                                    ),
                                ),
                            )
                            .client_info(Implementation::new("editur", env!("CARGO_PKG_VERSION"))),
                    )
                    .block_task()
                    .await?;
                if initialized.protocol_version != ProtocolVersion::V1 {
                    return Err(agent_client_protocol::Error::invalid_request()
                        .data("Cursor Agent does not support stable ACP v1"));
                }
                let auth = initialized
                    .auth_methods
                    .iter()
                    .take(MAX_CHOICES)
                    .map(|method| AuthChoice {
                        id: method.id().0.to_string(),
                        name: method.name().to_owned(),
                        description: method.description().map(str::to_owned),
                    })
                    .collect::<Vec<_>>();
                let supports_history = initialized.agent_capabilities.load_session
                    && initialized
                        .agent_capabilities
                        .session_capabilities
                        .list
                        .is_some();
                let (mut session_id, mut sessions) = match start_session(
                    &connection,
                    &project_root,
                    &events,
                    supports_history,
                    &hidden_sessions.ids,
                )
                .await
                {
                    Ok((session_id, sessions)) => (Some(session_id), sessions),
                    Err(_) if !auth.is_empty() => {
                        send_event(
                            &events,
                            Event::ConnectionChanged(ConnectionState::AuthenticationRequired(
                                auth.clone(),
                            )),
                        );
                        (None, Vec::new())
                    }
                    Err(error) => return Err(error),
                };
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
                            let authenticated = connection
                                .send_request(AuthenticateRequest::new(method.id.clone()))
                                .block_task()
                                .await;
                            if let Err(error) = authenticated {
                                send_event(
                                    &events,
                                    Event::Error(format!("authentication failed: {error}")),
                                );
                                continue;
                            }
                            match start_session(
                                &connection,
                                &project_root,
                                &events,
                                supports_history,
                                &hidden_sessions.ids,
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
                                        Event::Error(format!("cannot start session: {error}")),
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
                                    Ok(session) => session_id = Some(session),
                                    Err(error) => send_event(
                                        &events,
                                        Event::Error(format!("cannot start session: {error}")),
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
                                        Event::Error(format!("cannot list sessions: {error}")),
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
                                    send_event(
                                        &events,
                                        Event::Error(format!("cannot load session: {error}")),
                                    );
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
                                    Event::Error(format!("cannot set session mode: {error}")),
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
                                    Event::Error(format!("cannot set session option: {error}")),
                                ),
                            }
                        }
                        Command::SetRunEverything(enabled) => {
                            auto_approve_permissions.store(enabled, Ordering::Release);
                        }
                        Command::Prompt(text) => {
                            let Some(session) = session_id.clone() else {
                                send_event(
                                    &events,
                                    Event::Error("authenticate before sending a prompt".into()),
                                );
                                continue;
                            };
                            if text.trim().is_empty() || active.swap(true, Ordering::AcqRel) {
                                send_event(
                                    &events,
                                    Event::Error(
                                        "only one non-empty prompt can run at a time".into(),
                                    ),
                                );
                                continue;
                            }
                            send_event(&events, Event::UserMessage(text.clone()));
                            let events_for_result = events.clone();
                            let active_for_result = Arc::clone(&active);
                            connection
                                .send_request(PromptRequest::new(
                                    session,
                                    vec![ContentBlock::Text(TextContent::new(text))],
                                ))
                                .on_receiving_result(async move |result| {
                                    active_for_result.store(false, Ordering::Release);
                                    let cancelled = match result {
                                        Ok(response) => {
                                            response.stop_reason == StopReason::Cancelled
                                        }
                                        Err(error) => {
                                            send_event(
                                                &events_for_result,
                                                Event::Error(format!("agent turn failed: {error}")),
                                            );
                                            false
                                        }
                                    };
                                    send_event(
                                        &events_for_result,
                                        Event::TurnFinished { cancelled },
                                    );
                                    Ok(())
                                })?;
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
                    }
                }
                shutdown.store(true, Ordering::Release);
                Ok(())
            }
        })
        .await
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
    send_event(events, Event::ConnectionChanged(ConnectionState::Ready));
    Ok(response.session_id)
}

async fn start_session(
    connection: &ConnectionTo<Agent>,
    project_root: &std::path::Path,
    events: &EventSender,
    supports_history: bool,
    hidden_sessions: &HashSet<String>,
) -> agent_client_protocol::Result<(SessionId, Vec<SessionChoice>)> {
    let sessions = if supports_history {
        list_sessions(connection, project_root, events, hidden_sessions)
            .await
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    if let Some(session) = sessions.first()
        && let Ok(session_id) = load_session(connection, project_root, session, events).await
    {
        return Ok((session_id, sessions));
    }
    new_session(connection, project_root, events)
        .await
        .map(|session_id| (session_id, sessions))
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

fn session_history_path(project_root: &std::path::Path) -> Option<PathBuf> {
    let digest = Sha256::digest(project_root.as_os_str().as_encoded_bytes());
    let mut name = String::with_capacity(digest.len() * 2 + 5);
    for byte in digest {
        write!(&mut name, "{byte:02x}").expect("writing to a String cannot fail");
    }
    name.push_str(".json");
    crate::syntax::data_dir()
        .ok()
        .map(|directory| directory.join("agents/session-history").join(name))
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
    subagent_type: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    duration_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorImageUpdate {
    tool_call_id: String,
    description: String,
    file_path: PathBuf,
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
                paths: Vec::new(),
                detail: Some(ToolDetail {
                    input: None,
                    content: vec![ToolOutput::Task {
                        description: update.description,
                        prompt: update.prompt,
                        subagent_type: update.subagent_type,
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
                paths: vec![update.file_path.clone()],
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

fn tool_paths(locations: &[ToolCallLocation], content: &[ToolCallContent]) -> Vec<PathBuf> {
    let mut paths = locations
        .iter()
        .take(MAX_TOOL_PATHS)
        .map(|location| location.path.clone())
        .collect::<Vec<_>>();
    for item in content {
        if paths.len() == MAX_TOOL_PATHS {
            break;
        }
        if let ToolCallContent::Diff(diff) = item
            && !paths.contains(&diff.path)
        {
            paths.push(diff.path.clone());
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
    let _ = events.event_tx.send(event);
    (events.wake)();
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
                event_tx,
                wake: Arc::new(|| {}),
            },
        );
        event_rx.recv().expect("update should be visible")
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
            event_tx,
            wake: Arc::new(|| {}),
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
                event_tx,
                wake: Arc::new(|| {}),
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
                event_tx,
                wake: Arc::new(|| {}),
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
}
