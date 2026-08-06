use std::{
    io::Write,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use agent_client_protocol::schema::v1::{
    CancelNotification, ContentBlock, ContentChunk, Diff, InitializeRequest, InitializeResponse,
    NewSessionRequest, NewSessionResponse, PermissionOption, PermissionOptionKind, PromptRequest,
    PromptResponse, RequestPermissionOutcome, RequestPermissionRequest, SessionConfigOption,
    SessionConfigSelectOption, SessionMode, SessionModeState, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, StopReason, TextContent, ToolCall, ToolCallLocation, ToolCallStatus,
    ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Agent, ConnectionTo, Result, Stdio};

fn main() {
    #[cfg(windows)]
    if run_windows_job_fixture() {
        return;
    }
    let listener = std::env::args_os().nth(1).map(|address_file| {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind fake marker");
        std::fs::write(address_file, listener.local_addr().unwrap().to_string())
            .expect("write fake marker");
        listener
    });
    let result = async_io::block_on(run());
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

async fn run() -> Result<()> {
    let prompts = Arc::new(AtomicUsize::new(0));
    let (cancel_tx, cancel_rx) = async_channel::unbounded();
    Agent
        .builder()
        .name("editur-fake-agent")
        .on_receive_request(
            async move |request: InitializeRequest, responder, _connection| {
                responder.respond(InitializeResponse::new(request.protocol_version))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |request: NewSessionRequest, responder, _connection| {
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
                        .config_options(config_options("balanced", false)),
                )
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
            async move |request: SetSessionConfigOptionRequest, responder, _connection| {
                let options = match request.config_id.0.as_ref() {
                    "model" => {
                        let Some(model) = request.value.as_value_id() else {
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::invalid_params(),
                            ));
                        };
                        config_options(model.0.as_ref(), false)
                    }
                    "thoughts" => {
                        let Some(thoughts) = request.value.as_bool() else {
                            return responder.respond_with_result(Err(
                                agent_client_protocol::Error::invalid_params(),
                            ));
                        };
                        config_options("balanced", thoughts)
                    }
                    _ => {
                        return responder.respond_with_result(Err(
                            agent_client_protocol::Error::invalid_params(),
                        ));
                    }
                };
                responder.respond(SetSessionConfigOptionResponse::new(options))
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
                        if matches!(
                            prompt_text(&request),
                            "permission" | "permission-empty" | "permission-overflow"
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

fn config_options(model: &str, thoughts: bool) -> Vec<SessionConfigOption> {
    vec![
        SessionConfigOption::select(
            "model",
            "Model",
            model.to_owned(),
            vec![
                SessionConfigSelectOption::new("balanced", "Balanced"),
                SessionConfigSelectOption::new("fast", "Fast"),
            ],
        ),
        SessionConfigOption::boolean("thoughts", "Show thoughts", thoughts),
    ]
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
