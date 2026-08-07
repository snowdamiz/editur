use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthMethod, AuthMethodAgent, AuthenticateRequest, AuthenticateResponse,
    CancelNotification, ContentBlock, ContentChunk, Diff, InitializeRequest, InitializeResponse,
    ListSessionsRequest, ListSessionsResponse, LoadSessionRequest, LoadSessionResponse,
    NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest,
    PromptResponse, RequestPermissionOutcome, RequestPermissionRequest, SessionCapabilities,
    SessionConfigOption, SessionConfigSelectOption, SessionInfo, SessionListCapabilities,
    SessionMode, SessionModeState, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, StopReason, TextContent, ToolCall, ToolCallLocation, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Agent, ConnectionTo, Result, Stdio};

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

fn main() {
    #[cfg(windows)]
    if run_windows_job_fixture() {
        return;
    }
    let mut authentication_required = false;
    let mut sessions_supported = false;
    let mut address_file = None;
    for argument in std::env::args_os().skip(1) {
        match argument.to_str() {
            Some("--auth-required") => authentication_required = true,
            Some("--sessions") => sessions_supported = true,
            _ if address_file.is_none() => address_file = Some(argument),
            _ => {}
        }
    }
    let listener = address_file.map(|address_file| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake marker");
        std::fs::write(address_file, listener.local_addr().unwrap().to_string())
            .expect("write fake marker");
        listener
    });
    let result = async_io::block_on(run(authentication_required, sessions_supported));
    drop(listener);
    if let Err(error) = result {
        eprintln!("fake ACP agent: {error}");
        std::process::exit(1);
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

async fn run(authentication_required: bool, sessions_supported: bool) -> Result<()> {
    let prompts = Arc::new(AtomicUsize::new(0));
    let authenticated = Arc::new(AtomicBool::new(!authentication_required));
    let boolean_config_options = Arc::new(AtomicBool::new(false));
    let (cancel_tx, cancel_rx) = async_channel::unbounded();
    Agent
        .builder()
        .name("editur-fake-agent")
        .on_receive_request(
            {
                let boolean_config_options = Arc::clone(&boolean_config_options);
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
                    let mut response = InitializeResponse::new(request.protocol_version);
                    if sessions_supported {
                        response = response.agent_capabilities(
                            AgentCapabilities::new()
                                .load_session(true)
                                .session_capabilities(
                                    SessionCapabilities::new()
                                        .list(SessionListCapabilities::new()),
                                ),
                        );
                    }
                    if authentication_required {
                        response = response.auth_methods(vec![AuthMethod::Agent(
                            AuthMethodAgent::new("cursor_login", "Cursor Login"),
                        )]);
                    }
                    responder.respond(response)
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let authenticated = Arc::clone(&authenticated);
                async move |request: AuthenticateRequest, responder, _connection| {
                    if request.method_id.0.as_ref() != "cursor_login" {
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
                let boolean_config_options = Arc::clone(&boolean_config_options);
                async move |request: NewSessionRequest, responder, _connection| {
                    if !authenticated.load(Ordering::Acquire) {
                        return responder.respond_with_result(Err(
                            agent_client_protocol::Error::auth_required(),
                        ));
                    }
                if !request.cwd.is_absolute() {
                    return responder
                        .respond_with_result(Err(agent_client_protocol::Error::invalid_params()));
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
            async move |request: ListSessionsRequest, responder, _connection| {
                assert!(
                    sessions_supported
                        && request.cwd.as_ref().is_some_and(|cwd| cwd.is_absolute())
                );
                let cwd = request.cwd.expect("validated cwd");
                responder.respond(ListSessionsResponse::new(vec![
                    SessionInfo::new("older-session", cwd.clone())
                        .title("Older task")
                        .updated_at("2026-08-06T12:00:00Z"),
                    SessionInfo::new("newest-session", cwd)
                        .title("Newest task")
                        .updated_at("2026-08-07T12:00:00Z"),
                ]))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            {
                let boolean_config_options = Arc::clone(&boolean_config_options);
                async move |request: LoadSessionRequest,
                            responder,
                            connection: ConnectionTo<agent_client_protocol::Client>| {
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
                            request.session_id,
                            SessionUpdate::AgentMessageChunk(ContentChunk::new(
                                ContentBlock::Text(TextContent::new(reply)),
                            )),
                        ))?;
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
                async move |request: PromptRequest,
                            responder,
                            connection: ConnectionTo<agent_client_protocol::Client>| {
                    let cancel_rx = cancel_rx.clone();
                    let turn = prompts.fetch_add(1, Ordering::Relaxed) + 1;
                    let task_connection = connection.clone();
                    connection.spawn(async move {
                        if prompt_text(&request) == "wait" {
                            let _ = cancel_rx.recv().await;
                            return responder.respond(PromptResponse::new(StopReason::Cancelled));
                        }
                        if prompt_text(&request) == "exit" {
                            eprintln!("fake diagnostic before exit");
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
