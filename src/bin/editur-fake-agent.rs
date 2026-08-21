use std::{
    io::Write,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

#[cfg(unix)]
use std::io::{IsTerminal as _, Read as _};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthMethod, AuthMethodAgent, AuthMethodTerminal, AuthenticateRequest,
    AuthenticateResponse, CancelNotification, CloseSessionRequest, CloseSessionResponse,
    ContentBlock, ContentChunk, CreateElicitationRequest, Diff, ElicitationAction,
    ElicitationContentValue, ElicitationFormMode, ElicitationSchema, ElicitationSessionScope,
    EmbeddedResourceResource, InitializeRequest, InitializeResponse, ListSessionsRequest,
    ListSessionsResponse, LoadSessionRequest, LoadSessionResponse, NewSessionRequest,
    NewSessionResponse, PermissionOption, PermissionOptionKind, PromptCapabilities, PromptRequest,
    PromptResponse, RequestPermissionOutcome, RequestPermissionRequest, ResumeSessionRequest,
    ResumeSessionResponse, SessionAdditionalDirectoriesCapabilities, SessionCapabilities,
    SessionCloseCapabilities, SessionConfigOption, SessionConfigSelectOption, SessionInfo,
    SessionInfoUpdate, SessionListCapabilities, SessionMode, SessionModeState, SessionNotification,
    SessionResumeCapabilities, SessionUpdate, SetSessionConfigOptionRequest,
    SetSessionConfigOptionResponse, SetSessionModeRequest, SetSessionModeResponse, StopReason,
    StringPropertySchema, Terminal, TextContent, ToolCall, ToolCallContent, ToolCallLocation,
    ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Agent, ConnectionTo, Result, Stdio};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
struct CompatibilityFixture {
    initialize: InitializeResponse,
    new_session: NewSessionResponse,
}

#[derive(Default)]
struct AccountFixture {
    mode: Option<String>,
    handoff_file: Option<PathBuf>,
    workspace_file: Option<PathBuf>,
}

#[derive(Clone, Debug)]
struct CursorRequest {
    method: String,
    params: serde_json::Value,
}

impl agent_client_protocol::JsonRpcMessage for CursorRequest {
    fn matches_method(method: &str) -> bool {
        method == "cursor/ask_question"
    }

    fn method(&self) -> &str {
        &self.method
    }

    fn to_untyped_message(&self) -> Result<agent_client_protocol::UntypedMessage> {
        agent_client_protocol::UntypedMessage::new(&self.method, &self.params)
    }

    fn parse_message(method: &str, params: &impl serde::Serialize) -> Result<Self> {
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
struct CodexExtensionRequest {
    method: String,
    params: serde_json::Value,
}

impl agent_client_protocol::JsonRpcMessage for CodexExtensionRequest {
    fn matches_method(method: &str) -> bool {
        matches!(method, "_session/steering" | "_session/goal")
    }

    fn method(&self) -> &str {
        &self.method
    }

    fn to_untyped_message(&self) -> Result<agent_client_protocol::UntypedMessage> {
        agent_client_protocol::UntypedMessage::new(&self.method, &self.params)
    }

    fn parse_message(method: &str, params: &impl serde::Serialize) -> Result<Self> {
        Ok(Self {
            method: method.into(),
            params: serde_json::to_value(params)?,
        })
    }
}

impl agent_client_protocol::JsonRpcRequest for CodexExtensionRequest {
    type Response = serde_json::Value;
}

fn main() {
    if run_performance_fixture() {
        return;
    }
    #[cfg(unix)]
    if run_cursor_cloud_fixture() {
        return;
    }
    if run_descendant_child() {
        return;
    }
    if run_claude_login_fixture() {
        return;
    }
    #[cfg(windows)]
    if run_windows_job_fixture() {
        return;
    }
    let mut authentication_required = false;
    let mut terminal_auth = false;
    let mut codex_auth = false;
    let mut claude_auth = false;
    let mut claude_auth_file = None;
    let mut sessions_supported = false;
    let mut stale_session = false;
    let mut paged_sessions = false;
    let mut codex_extensions = false;
    let mut session_lifecycle_file = None;
    let mut additional_directories_file = None;
    let mut address_file = None;
    let mut descendant_file = None;
    let mut compatibility_fixture = None;
    let mut account_fixture = AccountFixture::default();
    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.to_str() {
            Some("--auth-required") => authentication_required = true,
            Some("--terminal-auth") => {
                authentication_required = true;
                terminal_auth = true;
            }
            Some("--codex-auth") => {
                authentication_required = true;
                codex_auth = true;
            }
            Some("--claude-auth") => {
                authentication_required = true;
                claude_auth = true;
                claude_auth_file = Some(arguments.next().expect("Claude auth marker path"));
            }
            Some("--sessions") => sessions_supported = true,
            Some("--stale-session") => stale_session = true,
            Some("--paged-sessions") => paged_sessions = true,
            Some("--codex-extensions") => codex_extensions = true,
            Some("--session-lifecycle") => {
                session_lifecycle_file = Some(arguments.next().expect("session lifecycle marker"));
            }
            Some("--additional-directories") => {
                additional_directories_file =
                    Some(arguments.next().expect("additional directories marker"));
            }
            Some("--codex-fixture" | "--claude-fixture") => {
                let path = arguments.next().expect("compatibility fixture path");
                compatibility_fixture = Some(
                    serde_json::from_slice(
                        &std::fs::read(path).expect("read ACP compatibility fixture"),
                    )
                    .expect("parse ACP compatibility fixture"),
                );
            }
            Some("--descendant") => {
                descendant_file = Some(arguments.next().expect("descendant marker path"));
            }
            Some("--account-fixture") => {
                account_fixture.mode = Some(
                    arguments
                        .next()
                        .expect("account fixture mode")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            Some("--cursor-cloud-fixture") => {}
            Some("--handoff-file") => {
                account_fixture.handoff_file =
                    Some(arguments.next().expect("handoff marker path").into());
            }
            Some("--workspace-file") => {
                account_fixture.workspace_file =
                    Some(arguments.next().expect("workspace marker path").into());
            }
            _ if address_file.is_none() => address_file = Some(argument),
            _ => {}
        }
    }
    let _descendant = descendant_file.map(|address_file| {
        std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--descendant-child")
            .arg(address_file)
            .spawn()
            .expect("spawn fake descendant")
    });
    let listener = address_file.map(|address_file| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake marker");
        std::fs::write(address_file, listener.local_addr().unwrap().to_string())
            .expect("write fake marker");
        listener
    });
    let result = async_io::block_on(run(
        authentication_required,
        terminal_auth,
        codex_auth,
        (claude_auth, claude_auth_file),
        sessions_supported,
        stale_session,
        paged_sessions,
        codex_extensions,
        session_lifecycle_file,
        additional_directories_file,
        compatibility_fixture,
        account_fixture,
    ));
    drop(listener);
    if let Err(error) = result {
        eprintln!("fake ACP agent: {error}");
        std::process::exit(1);
    }
}

#[cfg(unix)]
fn run_cursor_cloud_fixture() -> bool {
    if !std::env::args().any(|argument| argument == "--cursor-cloud-fixture")
        || !std::io::stdout().is_terminal()
    {
        return false;
    }
    assert!(
        std::process::Command::new("stty")
            .args(["raw", "-echo"])
            .status()
            .unwrap()
            .success()
    );

    let mut stdout = std::io::stdout().lock();
    stdout
        .write_all(b"Plan, search, build anything")
        .and_then(|()| stdout.flush())
        .unwrap();
    let mut stdin = std::io::stdin().lock();
    let mut byte = [0_u8; 1];
    stdin.read_exact(&mut byte).unwrap();
    assert_eq!(byte, [b'&']);
    stdout
        .write_all(b"\r\n^ Move to cloud agent")
        .and_then(|()| stdout.flush())
        .unwrap();

    let mut prompt = Vec::new();
    loop {
        stdin.read_exact(&mut byte).unwrap();
        if byte[0] == b'\r' {
            break;
        }
        prompt.push(byte[0]);
    }
    assert_eq!(prompt, b"\x1b[200~Fix cloud\x1b[201~");
    stdout
        .write_all(b"\r\nOpen Cloud Agent in: https://cursor.com/agents/bc-fixture\r\n")
        .and_then(|()| stdout.flush())
        .unwrap();
    true
}

fn run_performance_fixture() -> bool {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    if let Some(path) = arguments
        .windows(2)
        .find(|arguments| arguments[0] == "--performance-pid-file")
        .map(|arguments| &arguments[1])
    {
        std::fs::write(path, std::process::id().to_string()).expect("write performance pid");
    }
    if let Some(index) = arguments
        .iter()
        .position(|argument| argument == "--stall-before-protocol-ms")
    {
        let milliseconds = arguments
            .get(index + 1)
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(2_000)
            .min(10_000);
        std::thread::sleep(std::time::Duration::from_millis(milliseconds));
        return true;
    }
    let frame = arguments.iter().enumerate().find_map(|(index, argument)| {
        let terminated = match argument.as_str() {
            "--raw-frame-bytes" => true,
            "--raw-frame-unterminated-bytes" => false,
            _ => return None,
        };
        arguments
            .get(index + 1)
            .and_then(|value| value.parse::<usize>().ok())
            .map(|bytes| (bytes.min(128 * 1024 * 1024), terminated))
    });
    let Some((bytes, terminated)) = frame else {
        return false;
    };
    const PREFIX: &[u8] =
        b"{\"jsonrpc\":\"2.0\",\"method\":\"benchmark/frame\",\"params\":{\"payload\":\"";
    let suffix: &[u8] = if terminated { b"\"}}\n" } else { b"" };
    let payload_bytes = bytes.saturating_sub(PREFIX.len() + suffix.len());
    let mut stdout = std::io::stdout().lock();
    stdout.write_all(PREFIX).expect("write raw-frame prefix");
    let chunk = [b'x'; 64 * 1024];
    for length in (0..payload_bytes).step_by(chunk.len()) {
        stdout
            .write_all(&chunk[..chunk.len().min(payload_bytes - length)])
            .expect("write raw-frame payload");
        stdout.flush().expect("flush raw-frame payload");
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    stdout.write_all(suffix).expect("write raw-frame suffix");
    stdout.flush().expect("flush raw frame");
    true
}

fn run_claude_login_fixture() -> bool {
    let arguments = std::env::args_os().skip(1).collect::<Vec<_>>();
    if !arguments.windows(4).any(|arguments| {
        arguments
            == [
                std::ffi::OsStr::new("--cli"),
                std::ffi::OsStr::new("auth"),
                std::ffi::OsStr::new("login"),
                std::ffi::OsStr::new("--claudeai"),
            ]
    }) {
        return false;
    }
    let marker = arguments
        .windows(2)
        .find(|arguments| arguments[0] == "--claude-auth")
        .map(|arguments| &arguments[1])
        .expect("Claude auth marker argument");
    std::fs::write(marker, b"authenticated").expect("write Claude auth marker");
    true
}

fn run_descendant_child() -> bool {
    let mut arguments = std::env::args_os().skip(1);
    if arguments.next().as_deref() != Some(std::ffi::OsStr::new("--descendant-child")) {
        return false;
    }
    let address_file = arguments.next().expect("descendant marker path");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind descendant marker");
    std::fs::write(address_file, listener.local_addr().unwrap().to_string())
        .expect("write descendant marker");
    loop {
        std::thread::park();
    }
}

#[cfg(windows)]
fn run_windows_job_fixture() -> bool {
    let mut arguments = std::env::args_os().skip(1);
    match arguments
        .next()
        .as_deref()
        .and_then(std::ffi::OsStr::to_str)
    {
        Some("--job-parent") => {
            let name = arguments.next().expect("job name");
            let address_file = arguments.next().expect("address file");
            editur::agent::join_windows_job(&name.to_string_lossy()).expect("join Windows job");
            let mut descendant = std::process::Command::new(std::env::current_exe().unwrap())
                .arg("--job-child")
                .arg(address_file)
                .spawn()
                .expect("spawn fake descendant");
            let _ = descendant.wait();
            true
        }
        Some("--job-child") => {
            let address_file = arguments.next().expect("address file");
            let listener =
                std::net::TcpListener::bind("127.0.0.1:0").expect("bind descendant marker");
            std::fs::write(address_file, listener.local_addr().unwrap().to_string())
                .expect("write descendant marker");
            loop {
                std::thread::park();
            }
        }
        _ => false,
    }
}

#[expect(clippy::too_many_arguments)]
async fn run(
    authentication_required: bool,
    terminal_auth: bool,
    codex_auth: bool,
    (claude_auth, claude_auth_file): (bool, Option<std::ffi::OsString>),
    sessions_supported: bool,
    stale_session: bool,
    paged_sessions: bool,
    codex_extensions: bool,
    session_lifecycle_file: Option<std::ffi::OsString>,
    additional_directories_file: Option<std::ffi::OsString>,
    compatibility_fixture: Option<CompatibilityFixture>,
    account_fixture: AccountFixture,
) -> Result<()> {
    let compatibility_fixture = compatibility_fixture.map(Arc::new);
    let sessions_supported = sessions_supported
        || paged_sessions
        || compatibility_fixture.is_some()
        || session_lifecycle_file.is_some()
        || additional_directories_file.is_some();
    let session_lifecycle_file = session_lifecycle_file.map(PathBuf::from).map(Arc::new);
    let additional_directories_file = additional_directories_file.map(PathBuf::from).map(Arc::new);
    let prompts = Arc::new(AtomicUsize::new(0));
    let always_drop_transport = Arc::new(AtomicBool::new(false));
    let authenticated = Arc::new(AtomicBool::new(!authentication_required));
    let claude_auth_file = claude_auth_file.map(PathBuf::from).map(Arc::new);
    let boolean_config_options = Arc::new(AtomicBool::new(false));
    let account_fixture = Arc::new(account_fixture);
    let (cancel_tx, cancel_rx) = async_channel::unbounded();
    Agent
        .builder()
        .name("editur-fake-agent")
        .on_receive_request(
            {
                let boolean_config_options = Arc::clone(&boolean_config_options);
                let compatibility_fixture = compatibility_fixture.clone();
                    let session_lifecycle = session_lifecycle_file.is_some();
                    let additional_directories = additional_directories_file.is_some();
                async move |request: InitializeRequest, responder, _connection| {
                    boolean_config_options.store(
                        request
                            .client_capabilities
                            .session
                            .as_ref()
                            .and_then(|session| session.config_options.as_ref())
                            .and_then(|options| options.boolean.as_ref())
                            .is_some(),
                        Ordering::Release,
                    );
                    if let Some(fixture) = &compatibility_fixture {
                        return responder.respond(fixture.initialize.clone());
                    }
                    let mut capabilities = AgentCapabilities::new().prompt_capabilities(
                        PromptCapabilities::new()
                            .image(true)
                            .audio(true)
                            .embedded_context(true),
                    );
                    let mut response = InitializeResponse::new(request.protocol_version);
                    if sessions_supported {
                        capabilities = capabilities.load_session(true).session_capabilities(
                            SessionCapabilities::new()
                                .list(SessionListCapabilities::new())
                                .close(session_lifecycle.then(SessionCloseCapabilities::new))
                                .resume(
                                    additional_directories.then(SessionResumeCapabilities::new),
                                )
                                .additional_directories(additional_directories.then(
                                    SessionAdditionalDirectoriesCapabilities::new,
                                )),
                        );
                    }
                    if authentication_required {
                        response = response.auth_methods(if codex_auth {
                            let mut meta = serde_json::Map::new();
                            meta.insert(
                                "api-key".into(),
                                serde_json::json!({"provider": "openai"}),
                            );
                            vec![
                                AuthMethod::Agent(
                                    AuthMethodAgent::new("api-key", "API Key")
                                        .description("Use an API key to authenticate")
                                        .meta(meta),
                                ),
                                AuthMethod::Agent(
                                    AuthMethodAgent::new("chat-gpt", "ChatGPT")
                                        .description("Use ChatGPT to authenticate"),
                                ),
                            ]
                        } else if claude_auth {
                            if request.client_capabilities.auth.terminal {
                                vec![AuthMethod::Terminal(
                                    AuthMethodTerminal::new(
                                        "claude-ai-login",
                                        "Claude Subscription",
                                    )
                                    .description("Use Claude subscription")
                                    .args(vec![
                                        "--cli".into(),
                                        "auth".into(),
                                        "login".into(),
                                        "--claudeai".into(),
                                    ]),
                                )]
                            } else {
                                Vec::new()
                            }
                        } else {
                            vec![if terminal_auth {
                                AuthMethod::Terminal(
                                    AuthMethodTerminal::new("terminal_login", "Terminal Login")
                                        .args(vec!["login".into()]),
                                )
                            } else {
                                AuthMethod::Agent(AuthMethodAgent::new(
                                    "cursor_login",
                                    "Cursor Login",
                                ))
                            }]
                        });
                    }
                    response = response.agent_capabilities(capabilities);
                    if codex_extensions {
                        response = response.meta(serde_json::Map::from_iter([
                            (
                                "steering".into(),
                                serde_json::json!({"supported": true}),
                            ),
                            (
                                "goal".into(),
                                serde_json::json!({
                                    "version": 1,
                                    "controlMethod": "_session/goal",
                                    "actions": ["set", "pause", "resume", "clear"]
                                }),
                            ),
                        ]));
                    }
                    responder.respond(response)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: CodexExtensionRequest,
                        responder,
                        connection: ConnectionTo<agent_client_protocol::Client>| {
                let session_id = request
                    .params
                    .get("sessionId")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(agent_client_protocol::Error::invalid_params)?
                    .to_owned();
                match request.method.as_str() {
                    "_session/steering" => {
                        let text = request.params["prompt"]
                            .as_array()
                            .and_then(|blocks| blocks.first())
                            .and_then(|block| block.get("text"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default();
                        connection.send_notification(SessionNotification::new(
                            session_id.clone(),
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(format!("steered:{text}"))),
                            )),
                        ))?;
                        responder.respond(serde_json::json!({"outcome": "injected"}))
                    }
                    "_session/goal" => {
                        let action = request.params["action"].as_str().unwrap_or_default();
                        let goal = if action == "clear" {
                            serde_json::Value::Null
                        } else {
                            serde_json::json!({
                                "objective": request.params["objective"]
                                    .as_str()
                                    .unwrap_or("Ship the editor"),
                                "status": if action == "pause" { "paused" } else { "active" },
                                "controlMethod": "_session/goal"
                            })
                        };
                        connection.send_notification(SessionNotification::new(
                            session_id,
                            SessionUpdate::SessionInfoUpdate(
                                SessionInfoUpdate::new().meta(serde_json::Map::from_iter([(
                                    "goal".into(),
                                    goal,
                                )])),
                            ),
                        ))?;
                        responder.respond(serde_json::json!({}))
                    }
                    _ => responder.respond_with_result(Err(
                        agent_client_protocol::Error::method_not_found(),
                    )),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let marker = additional_directories_file.clone();
                async move |request: ResumeSessionRequest, responder, _connection| {
                    if let Some(path) = marker.as_deref() {
                        let value = request
                            .additional_directories
                            .iter()
                            .map(|path| path.to_string_lossy())
                            .collect::<Vec<_>>()
                            .join("\n");
                        std::fs::write(path, value)
                            .map_err(|_| agent_client_protocol::Error::internal_error())?;
                    }
                    responder.respond(ResumeSessionResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let authenticated = Arc::clone(&authenticated);
                async move |request: AuthenticateRequest, responder, _connection| {
                    if !matches!(request.method_id.0.as_ref(), "cursor_login" | "api-key" | "chat-gpt") {
                        return responder.respond_with_result(Err(
                            agent_client_protocol::Error::invalid_params(),
                        ));
                    }
                    authenticated.store(true, Ordering::Release);
                    responder.respond(AuthenticateResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let authenticated = Arc::clone(&authenticated);
                let claude_auth_file = claude_auth_file.clone();
                let boolean_config_options = Arc::clone(&boolean_config_options);
                let compatibility_fixture = compatibility_fixture.clone();
                async move |request: NewSessionRequest, responder, _connection| {
                    if !authenticated.load(Ordering::Acquire)
                        && !claude_auth_file.as_ref().is_some_and(|path| path.exists())
                    {
                        return responder.respond_with_result(Err(
                            agent_client_protocol::Error::auth_required(),
                        ));
                    }
                if !request.cwd.is_absolute() {
                    return responder
                        .respond_with_result(Err(agent_client_protocol::Error::invalid_params()));
                }
                if let Some(fixture) = &compatibility_fixture {
                    return responder.respond(fixture.new_session.clone());
                }
                responder.respond(
                    NewSessionResponse::new("fake-session")
                        .modes(SessionModeState::new(
                            "ask",
                            vec![
                                SessionMode::new("ask", "Ask"),
                                SessionMode::new("agent", "Agent"),
                            ],
                        ))
                        .config_options(config_options(
                            "balanced",
                            false,
                            boolean_config_options.load(Ordering::Acquire),
                        )),
                )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
            let has_compatibility_fixture = compatibility_fixture.is_some();
            async move |request: ListSessionsRequest, responder, _connection| {
                assert!(
                    sessions_supported
                        && request.cwd.as_ref().is_some_and(|cwd| cwd.is_absolute())
                );
                if has_compatibility_fixture {
                    return responder.respond(ListSessionsResponse::new(Vec::new()));
                }
                let cwd = request.cwd.expect("validated cwd");
                if paged_sessions {
                    let (id, title, next_cursor) = if request.cursor.is_none() {
                        (
                            "newest-session",
                            "Newest task",
                            Some("second-page".to_owned()),
                        )
                    } else {
                        ("older-session", "Older task", None)
                    };
                    return responder.respond(
                        ListSessionsResponse::new(vec![
                            SessionInfo::new(id, cwd).title(title),
                        ])
                        .next_cursor(next_cursor),
                    );
                }
                let mut sessions = vec![
                    SessionInfo::new("older-session", cwd.clone())
                        .title("Older task")
                        .updated_at("2026-08-06T12:00:00Z"),
                    SessionInfo::new("newest-session", cwd.clone())
                        .title("Newest task")
                        .updated_at("2026-08-07T12:00:00Z"),
                ];
                if stale_session {
                    sessions.push(
                        SessionInfo::new("stale-session", cwd)
                            .title("Missing task")
                            .updated_at("2026-08-05T12:00:00Z"),
                    );
                }
                responder.respond(ListSessionsResponse::new(sessions))
            }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let session_lifecycle_file = session_lifecycle_file.clone();
                async move |request: CloseSessionRequest, responder, _connection| {
                    if let Some(path) = session_lifecycle_file.as_deref() {
                        std::fs::write(path, request.session_id.0.as_bytes()).map_err(|_| {
                            agent_client_protocol::Error::internal_error()
                        })?;
                    }
                    responder.respond(CloseSessionResponse::new())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let boolean_config_options = Arc::clone(&boolean_config_options);
                let session_lifecycle = session_lifecycle_file.is_some();
                async move |request: LoadSessionRequest,
                            responder,
                            connection: ConnectionTo<agent_client_protocol::Client>| {
                        if stale_session && request.session_id.0.as_ref() == "stale-session" {
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::invalid_params().data(
                                    serde_json::json!({
                                        "message": "Session stale-session not found"
                                    }),
                                ),
                            ));
                        }
                        if !sessions_supported
                            || !matches!(
                                request.session_id.0.as_ref(),
                                "newest-session" | "older-session"
                            )
                            || !request.cwd.is_absolute()
                        {
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::invalid_params(),
                            ));
                        }
                        let (prompt, reply) = if request.session_id.0.as_ref() == "newest-session" {
                            ("restored prompt", "restored reply")
                        } else {
                            ("older prompt", "older reply")
                        };
                        connection.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::UserMessageChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(prompt)),
                            )),
                        ))?;
                        connection.send_notification(SessionNotification::new(
                            request.session_id.clone(),
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(reply)),
                            )),
                        ))?;
                        if session_lifecycle && request.session_id.0.as_ref() == "older-session" {
                            let delayed = connection.clone();
                            let session_id = request.session_id.clone();
                            connection.spawn(async move {
                                async_io::Timer::after(std::time::Duration::from_millis(40)).await;
                                delayed.send_notification(SessionNotification::new(
                                    "newest-session",
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new("stale previous reply")),
                                    )),
                                ))?;
                                delayed.send_notification(SessionNotification::new(
                                    session_id,
                                    SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                        ContentBlock::Text(TextContent::new("settled current reply")),
                                    )),
                                ))
                            })?;
                        }
                        responder.respond(
                            LoadSessionResponse::new()
                                .modes(SessionModeState::new(
                                    "ask",
                                    vec![
                                        SessionMode::new("ask", "Ask"),
                                        SessionMode::new("agent", "Agent"),
                                    ],
                                ))
                                .config_options(config_options(
                                    "balanced",
                                    false,
                                    boolean_config_options.load(Ordering::Acquire),
                                )),
                        )
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: SetSessionModeRequest, responder, _connection| {
                if !matches!(request.mode_id.0.as_ref(), "ask" | "agent") {
                    return responder
                        .respond_with_result(Err(agent_client_protocol::Error::invalid_params()));
                }
                responder.respond(SetSessionModeResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let boolean_config_options = Arc::clone(&boolean_config_options);
                async move |request: SetSessionConfigOptionRequest, responder, _connection| {
                let supports_boolean = boolean_config_options.load(Ordering::Acquire);
                let options = match request.config_id.0.as_ref() {
                    "model" => {
                        let Some(model) = request.value.as_value_id() else {
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::invalid_params(),
                            ));
                        };
                        config_options(model.0.as_ref(), false, supports_boolean)
                    }
                    "thoughts" => {
                        if !supports_boolean {
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::invalid_params(),
                            ));
                        }
                        let Some(thoughts) = request.value.as_bool() else {
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::invalid_params(),
                            ));
                        };
                        config_options("balanced", thoughts, true)
                    }
                    _ => {
                        return responder.respond_with_result(Err(
                            agent_client_protocol::Error::invalid_params(),
                        ));
                    }
                };
                responder.respond(SetSessionConfigOptionResponse::new(options))
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let prompts = Arc::clone(&prompts);
                let always_drop_transport = Arc::clone(&always_drop_transport);
                let account_fixture = Arc::clone(&account_fixture);
                async move |request: PromptRequest,
                            responder,
                            connection: ConnectionTo<agent_client_protocol::Client>| {
                    let cancel_rx = cancel_rx.clone();
                    let always_drop_transport = Arc::clone(&always_drop_transport);
                    let turn = prompts.fetch_add(1, Ordering::Relaxed) + 1;
                    let task_connection = connection.clone();
                    let account_fixture = Arc::clone(&account_fixture);
                    connection.spawn(async move {
                        if account_fixture.mode.as_deref() == Some("a")
                            && prompt_text(&request) == "account-failover"
                        {
                            if let Some(path) = &account_fixture.workspace_file {
                                std::fs::write(path, b"partial workspace change").map_err(|_| {
                                    agent_client_protocol::Error::internal_error()
                                })?;
                            }
                            stream_text(&task_connection, &request, "partial account A output")?;
                            task_connection.send_notification(SessionNotification::new(
                                request.session_id.clone(),
                                SessionUpdate::ToolCall(
                                    ToolCall::new("account-a-edit", "Edit workspace")
                                        .status(ToolCallStatus::Completed)
                                        .locations(vec![ToolCallLocation::new(
                                            account_fixture
                                                .workspace_file
                                                .as_deref()
                                                .unwrap_or_else(|| std::path::Path::new("workspace")),
                                        )]),
                                ),
                            ))?;
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::internal_error().data(
                                    serde_json::json!({
                                        "codexErrorInfo": "usageLimitExceeded"
                                    }),
                                ),
                            ));
                        }
                        if account_fixture.mode.as_deref() == Some("exhausted") {
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::internal_error().data(
                                    serde_json::json!({
                                        "codexErrorInfo": "usageLimitExceeded"
                                    }),
                                ),
                            ));
                        }
                        if account_fixture.mode.as_deref() == Some("b")
                            && prompt_text(&request)
                                .starts_with("<!-- editur-account-handoff:v1 ")
                        {
                            if let Some(path) = &account_fixture.handoff_file {
                                std::fs::write(path, prompt_text(&request)).map_err(|_| {
                                    agent_client_protocol::Error::internal_error()
                                })?;
                            }
                            stream_text(&task_connection, &request, "account B completed handoff")?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "transport-drop-loop" {
                            always_drop_transport.store(true, Ordering::Release);
                        }
                        if prompt_text(&request) == "transport-drop"
                            || always_drop_transport.load(Ordering::Acquire)
                        {
                            task_connection.send_notification(SessionNotification::new(
                                request.session_id.clone(),
                                SessionUpdate::ToolCall(
                                    ToolCall::new("drop-edit", "Edit File")
                                        .status(ToolCallStatus::InProgress),
                                ),
                            ))?;
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::internal_error().data(
                                    "Error: RetriableError: [canceled] http/2 stream closed \
                                     with error code CANCEL (0x8)",
                                ),
                            ));
                        }
                        if prompt_text(&request) == "wait" {
                            let _ = cancel_rx.recv().await;
                            return responder.respond(PromptResponse::new(StopReason::Cancelled));
                        }
                        if prompt_text(&request) == "exit" {
                            eprintln!("fake diagnostic before exit");
                            std::process::exit(23);
                        }
                        if prompt_text(&request) == "exit-secret" {
                            eprintln!("CODEX_API_KEY=super-secret-test-value");
                            std::process::exit(23);
                        }
                        if prompt_text(&request) == "error" {
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::invalid_request(),
                            ));
                        }
                        if prompt_text(&request) == "malformed" {
                            {
                                let mut stdout = std::io::stdout().lock();
                                stdout.write_all(b"{not-json}\n").map_err(|_| {
                                    agent_client_protocol::Error::internal_error()
                                })?;
                                stdout.flush().map_err(|_| {
                                    agent_client_protocol::Error::internal_error()
                                })?;
                            }
                            std::future::pending::<()>().await;
                        }
                        if prompt_text(&request) == "image" {
                            let summary = request
                                .prompt
                                .iter()
                                .find_map(|content| match content {
                                    ContentBlock::Image(image) => {
                                        Some(format!("{}:{}", image.mime_type, image.data.len()))
                                    }
                                    _ => None,
                                })
                                .unwrap_or_else(|| "missing image".into());
                            stream_text(&task_connection, &request, &summary)?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "attachments" {
                            let summary = request
                                .prompt
                                .iter()
                                .filter_map(|content| match content {
                                    ContentBlock::Audio(audio) => {
                                        Some(format!("audio:{}", audio.mime_type))
                                    }
                                    ContentBlock::Resource(resource) => match &resource.resource {
                                        EmbeddedResourceResource::TextResourceContents(text) => {
                                            Some(format!(
                                                "text:{}",
                                                text.mime_type.as_deref().unwrap_or("unknown")
                                            ))
                                        }
                                        EmbeddedResourceResource::BlobResourceContents(blob) => {
                                            Some(format!(
                                                "blob:{}",
                                                blob.mime_type.as_deref().unwrap_or("unknown")
                                            ))
                                        }
                                        _ => None,
                                    },
                                    _ => None,
                                })
                                .collect::<Vec<_>>()
                                .join(",");
                            stream_text(&task_connection, &request, &summary)?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "unknown" {
                            {
                                let mut stdout = std::io::stdout().lock();
                                stdout
                                    .write_all(
                                        b"{\"jsonrpc\":\"2.0\",\"method\":\"future/notification\",\"params\":{}}\n",
                                    )
                                    .map_err(|_| agent_client_protocol::Error::internal_error())?;
                                stdout
                                    .flush()
                                    .map_err(|_| agent_client_protocol::Error::internal_error())?;
                            }
                            stream_text(&task_connection, &request, "unknown ignored")?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "memory-stress" {
                            let payload = "x".repeat(64 * 1024);
                            for index in 0..300 {
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::ToolCall(
                                        ToolCall::new(
                                            format!("memory-stress-{index}"),
                                            "Memory stress",
                                        )
                                        .status(ToolCallStatus::Completed)
                                        .raw_input(serde_json::json!({"payload": payload})),
                                    ),
                                ))?;
                            }
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "tool" {
                            task_connection.send_notification(SessionNotification::new(
                                request.session_id.clone(),
                                SessionUpdate::ToolCall(
                                    ToolCall::new("fake-edit", "Edit fake.rs")
                                        .status(ToolCallStatus::InProgress)
                                        .locations(vec![ToolCallLocation::new("/tmp/fake.rs")])
                                        .raw_input(serde_json::json!({
                                            "command": "replace",
                                            "cwd": "/tmp"
                                        })),
                                ),
                            ))?;
                            task_connection.send_notification(SessionNotification::new(
                                request.session_id.clone(),
                                SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
                                    "fake-edit",
                                    ToolCallUpdateFields::new()
                                        .status(ToolCallStatus::Completed)
                                        .content(vec![Diff::new("/tmp/fake.rs", "after")
                                            .old_text("before".to_owned())
                                            .into()]),
                                )),
                            ))?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "codex-progress" {
                            task_connection.send_notification(SessionNotification::new(
                                request.session_id.clone(),
                                SessionUpdate::ToolCall(
                                    ToolCall::new("progress-tool", "Run checks")
                                        .status(ToolCallStatus::InProgress)
                                        .content(vec![ToolCallContent::Terminal(Terminal::new(
                                            "terminal-1",
                                        ))]),
                                ),
                            ))?;
                            for meta in [
                                serde_json::json!({
                                    "terminal_output_delta": {
                                        "terminal_id": "terminal-1",
                                        "data": "compiling\n"
                                    }
                                }),
                                serde_json::json!({
                                    "mcp_output_delta": {"data": "checking service"}
                                }),
                            ] {
                                task_connection.send_notification(SessionNotification::new(
                                    request.session_id.clone(),
                                    SessionUpdate::ToolCallUpdate(
                                        ToolCallUpdate::new(
                                            "progress-tool",
                                            ToolCallUpdateFields::new(),
                                        )
                                        .meta(meta.as_object().cloned()),
                                    ),
                                ))?;
                            }
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "elicitation" {
                            let response = task_connection
                                .send_request(CreateElicitationRequest::new(
                                    ElicitationFormMode::new(
                                        ElicitationSessionScope::new(request.session_id.clone()),
                                        ElicitationSchema::new()
                                            .property(
                                                "name",
                                                StringPropertySchema::new().title("Name"),
                                                true,
                                            )
                                            .boolean("confirm", true),
                                    ),
                                    "Configure",
                                ))
                                .block_task()
                                .await?;
                            let summary = match response.action {
                                ElicitationAction::Accept(action) => {
                                    let content = action.content.unwrap_or_default();
                                    match (content.get("name"), content.get("confirm")) {
                                        (
                                            Some(ElicitationContentValue::String(name)),
                                            Some(ElicitationContentValue::Boolean(confirm)),
                                        ) => format!("{name}:{confirm}"),
                                        _ => "invalid elicitation".into(),
                                    }
                                }
                                _ => "declined elicitation".into(),
                            };
                            stream_text(&task_connection, &request, &summary)?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "cursor-question" {
                            let response = task_connection
                                .send_request(CursorRequest {
                                    method: "cursor/ask_question".into(),
                                    params: serde_json::json!({
                                        "toolCallId": "ask-1",
                                        "title": "Choose",
                                        "questions": [{
                                            "id": "q",
                                            "prompt": "Pick any",
                                            "allowMultiple": true,
                                            "options": [
                                                {"id": "a", "label": "A"},
                                                {"id": "b", "label": "B"}
                                            ]
                                        }]
                                    }),
                                })
                                .block_task()
                                .await?;
                            stream_text(
                                &task_connection,
                                &request,
                                response["outcome"]["outcome"]
                                    .as_str()
                                    .unwrap_or("invalid"),
                            )?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "cursor-notification" {
                            task_connection.send_notification(
                                agent_client_protocol::UntypedMessage::new(
                                    "cursor/update_todos",
                                    serde_json::json!({
                                        "toolCallId": "todos-1",
                                        "merge": true,
                                        "todos": [{
                                            "id": "a",
                                            "content": "Ship it",
                                            "status": "in_progress"
                                        }]
                                    }),
                                )?,
                            )?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "cursor-task" {
                            task_connection.send_notification(
                                agent_client_protocol::UntypedMessage::new(
                                    "cursor/task",
                                    serde_json::json!({
                                        "toolCallId": "task-1",
                                        "description": "Review changes",
                                        "prompt": "Look at the diff and report issues.",
                                        "subagentType": {"custom": "reviewer"},
                                        "model": "gpt-5",
                                        "agentId": "agent-9",
                                        "durationMs": 1200
                                    }),
                                )?,
                            )?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "cursor-image" {
                            task_connection.send_notification(
                                agent_client_protocol::UntypedMessage::new(
                                    "cursor/generate_image",
                                    serde_json::json!({
                                        "toolCallId": "image-1",
                                        "description": "Minimal flat app icon"
                                    }),
                                )?,
                            )?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if prompt_text(&request) == "cursor-plan" {
                            let response = task_connection
                                .send_request(CursorRequest {
                                    method: "cursor/create_plan".into(),
                                    params: serde_json::json!({
                                        "toolCallId": "plan-1",
                                        "name": "Fix sidebar",
                                        "overview": "Keep every update visible",
                                        "plan": "1. Normalize\n2. Render",
                                        "todos": [{
                                            "id": "one",
                                            "content": "Normalize",
                                            "status": "pending"
                                        }],
                                        "isProject": false
                                    }),
                                })
                                .block_task()
                                .await?;
                            stream_text(
                                &task_connection,
                                &request,
                                response["outcome"]["outcome"]
                                    .as_str()
                                    .unwrap_or("invalid"),
                            )?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if let Some(state) = prompt_text(&request).strip_prefix("/run-everything ") {
                            stream_text(
                                &task_connection,
                                &request,
                                &format!("run everything {state}"),
                            )?;
                            return responder.respond(PromptResponse::new(StopReason::EndTurn));
                        }
                        if matches!(
                            prompt_text(&request),
                            "permission"
                                | "permission-always"
                                | "permission-empty"
                                | "permission-overflow"
                        ) {
                            let options = match prompt_text(&request) {
                                "permission-empty" => Vec::new(),
                                "permission-overflow" => (0..129)
                                    .map(|index| {
                                        PermissionOption::new(
                                            format!("choice-{index}"),
                                            format!("Choice {index}"),
                                            PermissionOptionKind::AllowOnce,
                                        )
                                    })
                                    .collect(),
                                "permission-always" => vec![
                                    PermissionOption::new(
                                        "allow_once",
                                        "Allow once",
                                        PermissionOptionKind::AllowOnce,
                                    ),
                                    PermissionOption::new(
                                        "allow_always",
                                        "Allow always",
                                        PermissionOptionKind::AllowAlways,
                                    ),
                                    PermissionOption::new(
                                        "reject_once",
                                        "Reject once",
                                        PermissionOptionKind::RejectOnce,
                                    ),
                                ],
                                _ => vec![
                                    PermissionOption::new(
                                        "allow_once",
                                        "Allow once",
                                        PermissionOptionKind::AllowOnce,
                                    ),
                                    PermissionOption::new(
                                        "reject_once",
                                        "Reject once",
                                        PermissionOptionKind::RejectOnce,
                                    ),
                                ],
                            };
                            let response = task_connection
                                .send_request(RequestPermissionRequest::new(
                                    request.session_id.clone(),
                                    ToolCallUpdate::new(
                                        "fake-tool",
                                        ToolCallUpdateFields::new()
                                            .title("Run a sensitive command")
                                            .raw_input(serde_json::json!({"command": "fake"})),
                                    ),
                                    options,
                                ))
                                .block_task()
                                .await?;
                            return match response.outcome {
                                RequestPermissionOutcome::Selected(selected) => {
                                    stream_text(
                                        &task_connection,
                                        &request,
                                        selected.option_id.0.as_ref(),
                                    )?;
                                    responder.respond(PromptResponse::new(StopReason::EndTurn))
                                }
                                RequestPermissionOutcome::Cancelled => responder
                                    .respond(PromptResponse::new(StopReason::Cancelled)),
                                _ => responder.respond(PromptResponse::new(StopReason::Cancelled)),
                            };
                        }
                        let prefix = if turn == 1 { "first " } else { "second " };
                        stream_text(&task_connection, &request, prefix)?;
                        stream_text(&task_connection, &request, "reply")?;
                        responder.respond(PromptResponse::new(StopReason::EndTurn))
                    })?;
                    Ok(())
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |notification: CancelNotification, _connection| {
                cancel_tx
                    .send(notification.session_id)
                    .await
                    .map_err(|_| agent_client_protocol::Error::internal_error())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(Stdio::new())
        .await
}

fn config_options(model: &str, thoughts: bool, supports_boolean: bool) -> Vec<SessionConfigOption> {
    let mut options = vec![SessionConfigOption::select(
        "model",
        "Model",
        model.to_owned(),
        vec![
            SessionConfigSelectOption::new("balanced", "Balanced"),
            SessionConfigSelectOption::new("fast", "Fast"),
        ],
    )];
    if supports_boolean {
        options.push(SessionConfigOption::boolean(
            "thoughts",
            "Show thoughts",
            thoughts,
        ));
    }
    options
}

fn prompt_text(request: &PromptRequest) -> &str {
    request
        .prompt
        .iter()
        .find_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .unwrap_or_default()
}

fn stream_text(
    connection: &ConnectionTo<agent_client_protocol::Client>,
    request: &PromptRequest,
    text: &str,
) -> Result<()> {
    connection.send_notification(SessionNotification::new(
        request.session_id.clone(),
        SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::Text(TextContent::new(
            text,
        )))),
    ))
}
