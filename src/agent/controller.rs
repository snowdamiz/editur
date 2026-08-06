use std::{
    collections::{HashMap, HashSet},
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

const EVENT_CAPACITY: usize = 512;
const COMMAND_CAPACITY: usize = 64;
const MAX_DETAIL_BYTES: usize = 16 * 1024;
const MAX_DIAGNOSTIC_BYTES: usize = 64 * 1024;
const MAX_CHOICES: usize = 128;
const MAX_PLAN_ITEMS: usize = 1_024;
const MAX_TOOL_PATHS: usize = 256;

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

#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    ConnectionChanged(ConnectionState),
    SessionReady {
        current_mode: Option<String>,
        modes: Vec<ModeChoice>,
        config_options: Vec<ConfigChoice>,
    },
    ModeChanged(String),
    ConfigOptionsUpdated(Vec<ConfigChoice>),
    UserMessage(String),
    AssistantDelta(String),
    PlanUpdated(Vec<PlanItem>),
    ToolCallUpdated(ToolActivity),
    PermissionRequested(PermissionRequest),
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
    pub detail: Option<String>,
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
pub enum Command {
    Connect,
    Authenticate(String),
    NewSession,
    SetMode(String),
    SetConfig {
        id: String,
        value: ConfigValue,
    },
    Prompt(String),
    DecidePermission {
        request_id: u64,
        option_id: String,
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
        Self::start_launch(project_root, Launch::Managed, Arc::new(|| {}))
    }

    pub fn start_with_wake(project_root: PathBuf, wake: impl Fn() + Send + Sync + 'static) -> Self {
        Self::start_launch(project_root, Launch::Managed, Arc::new(wake))
    }

    #[doc(hidden)]
    pub fn start_process(project_root: PathBuf, command: PathBuf, args: Vec<String>) -> Self {
        Self::start_launch(
            project_root,
            Launch::Process(AcpAgentConfig::new(command).args(args)),
            Arc::new(|| {}),
        )
    }

    fn start_launch(
        project_root: PathBuf,
        launch: Launch,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        let (command_tx, command_rx) = async_channel::bounded(COMMAND_CAPACITY);
        let debug_commands = command_tx.clone();
        let (event_tx, event_rx) = std::sync::mpsc::sync_channel(EVENT_CAPACITY);
        let event_tx = EventSender { event_tx, wake };
        let worker = thread::Builder::new()
            .name("editur-agent".into())
            .spawn(move || run_thread(project_root, launch, command_rx, debug_commands, event_tx))
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
    ));
    if shutdown.load(Ordering::Acquire) {
        send_event(
            &events,
            Event::ConnectionChanged(ConnectionState::Disconnected),
        );
    } else if let Err(error) = result {
        send_event(
            &events,
            Event::ProcessExited {
                error: error.to_string(),
                diagnostics: diagnostics
                    .lock()
                    .map(|text| text.clone())
                    .unwrap_or_default(),
            },
        );
    }
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
) -> agent_client_protocol::Result<()> {
    let active = Arc::new(AtomicBool::new(false));
    let permissions = Arc::new(Mutex::new(HashMap::new()));
    let next_permission = Arc::new(AtomicU64::new(1));
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
        .on_receive_request(
            {
                let events = events.clone();
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
                            action: bounded_json(&request.tool_call),
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
        .connect_with(agent, |connection: ConnectionTo<Agent>| {
            let events = events.clone();
            let active = Arc::clone(&active);
            let permissions = Arc::clone(&permissions);
            let shutdown = Arc::clone(&shutdown);
            async move {
                let initialized = connection
                    .send_request(
                        InitializeRequest::new(ProtocolVersion::V1)
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
                let mut session_id = match new_session(&connection, &project_root, &events).await {
                    Ok(session_id) => Some(session_id),
                    Err(error) if !auth.is_empty() => {
                        send_event(
                            &events,
                            Event::ConnectionChanged(ConnectionState::AuthenticationRequired(
                                auth.clone(),
                            )),
                        );
                        send_event(&events, Event::Error(error.to_string()));
                        None
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
                            match new_session(&connection, &project_root, &events).await {
                                Ok(session) => session_id = Some(session),
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
                                connection.send_notification(CancelNotification::new(session))?;
                            }
                        }
                        Command::Shutdown => {
                            shutdown.store(true, Ordering::Release);
                            permissions
                                .lock()
                                .expect("permission lock poisoned")
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
    send_event(
        events,
        Event::SessionReady {
            current_mode: response
                .modes
                .as_ref()
                .map(|modes| modes.current_mode_id.0.to_string()),
            modes: response.modes.as_ref().map_or_else(Vec::new, |modes| {
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
            config_options: response
                .config_options
                .as_deref()
                .map_or_else(Vec::new, normalize_config_options),
        },
    );
    send_event(events, Event::ConnectionChanged(ConnectionState::Ready));
    Ok(response.session_id)
}

struct PendingPermission {
    allowed: HashSet<String>,
    decision_tx: async_channel::Sender<PendingDecision>,
}

enum PendingDecision {
    Selected(String),
    Cancelled,
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
        SessionUpdate::AgentMessageChunk(chunk) => {
            if let ContentBlock::Text(text) = chunk.content {
                send_event(events, Event::AssistantDelta(text.text));
            }
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
        _ => {}
    }
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
) -> Option<String> {
    if input.is_none() && content.is_empty() && output.is_none() {
        return None;
    }
    Some(bounded_json(&serde_json::json!({
        "input": input,
        "content": content,
        "output": output,
    })))
}

fn bounded_json(value: &impl serde::Serialize) -> String {
    let mut detail = serde_json::to_string(value).unwrap_or_else(|_| "<unavailable>".into());
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
    use super::{MAX_DETAIL_BYTES, append_bounded, bounded_json, protocol_label};
    use std::sync::Mutex;

    #[test]
    fn bounded_diagnostics_keep_complete_lines_and_unicode_boundaries() {
        let output = Mutex::new(String::new());
        append_bounded(&output, "hello", 64);
        append_bounded(&output, "é", 64);
        assert_eq!(*output.lock().unwrap(), "hello\né\n");
    }

    #[test]
    fn bounded_json_keeps_unicode_boundaries() {
        let detail = bounded_json(&"é".repeat(MAX_DETAIL_BYTES));

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
