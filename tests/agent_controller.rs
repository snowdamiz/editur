use std::time::{Duration, Instant};

use editur::agent::controller::{
    AgentController, AuthKind, Command, ConfigValue, ConnectionState, Event, InteractionResponse,
    PromptAttachment, QuestionAnswer,
};
use editur::agent::provider::ProviderId;

fn receive_until(
    controller: &AgentController,
    timeout: Duration,
    mut done: impl FnMut(&Event) -> bool,
) -> Vec<Event> {
    let deadline = Instant::now() + timeout;
    let mut events = Vec::new();
    while Instant::now() < deadline {
        if let Ok(event) = controller.events().recv_timeout(Duration::from_millis(100)) {
            let finished = done(&event);
            events.push(event);
            if finished {
                return events;
            }
        }
    }
    panic!("timed out waiting for controller event: {events:?}");
}

#[test]
fn image_prompt_reaches_the_agent_as_an_acp_image_block() {
    let project = tempfile::tempdir().unwrap();
    let image_path = project.path().join("reference.png");
    std::fs::write(&image_path, [137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 0]).unwrap();
    let controller = AgentController::start_process_for(
        ProviderId::Claude,
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    controller
        .send(Command::PromptWithAttachments {
            text: "image".into(),
            attachments: vec![PromptAttachment::from_path(&image_path).unwrap()],
        })
        .unwrap();
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });

    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::AssistantDelta(text) if text == "image/png:16"))
    );
}

#[test]
fn audio_and_text_files_use_their_advertised_acp_content_blocks() {
    let project = tempfile::tempdir().unwrap();
    let audio = project.path().join("sample.mp3");
    let notes = project.path().join("notes.md");
    std::fs::write(&audio, b"ID3sample audio").unwrap();
    std::fs::write(&notes, b"project notes").unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    controller
        .send(Command::PromptWithAttachments {
            text: "attachments".into(),
            attachments: vec![
                PromptAttachment::from_path(audio).unwrap(),
                PromptAttachment::from_path(notes).unwrap(),
            ],
        })
        .unwrap();
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });

    assert!(events.iter().any(
        |event| matches!(event, Event::AssistantDelta(text) if text == "audio:audio/mpeg,text:text/plain")
    ));
}

#[test]
fn authentication_required_is_a_status_not_a_transcript_error() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec!["--auth-required".into()],
    );
    let mut events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(
            event,
            Event::ConnectionChanged(
                editur::agent::controller::ConnectionState::AuthenticationRequired(_)
            )
        )
    });
    assert!(events.iter().any(|event| matches!(
        event,
        Event::ConnectionChanged(ConnectionState::AuthenticationRequired(methods))
            if matches!(methods.as_slice(), [method] if method.kind == AuthKind::Agent)
    )));

    controller
        .send(Command::Authenticate("cursor_login".into()))
        .unwrap();
    events.extend(receive_until(
        &controller,
        Duration::from_secs(5),
        |event| matches!(event, Event::SessionReady { .. }),
    ));

    assert!(!events.iter().any(|event| matches!(event, Event::Error(_))));
}

#[test]
fn terminal_authentication_kind_is_preserved_without_collecting_credentials() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process_for(
        ProviderId::Codex,
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec!["--terminal-auth".into()],
    );

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(
            event,
            Event::ConnectionChanged(ConnectionState::AuthenticationRequired(_))
        )
    });

    assert!(events.iter().any(|event| matches!(
        event,
        Event::ConnectionChanged(ConnectionState::AuthenticationRequired(methods))
            if matches!(methods.as_slice(), [method]
                if method.id == "terminal_login"
                    && method.kind == AuthKind::Terminal
                    && method.setup.as_deref() == Some("arguments: login"))
    )));
}

#[test]
fn codex_api_key_auth_is_environment_owned_and_chatgpt_stays_agent_owned() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process_for(
        ProviderId::Codex,
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec!["--codex-auth".into()],
    );

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(
            event,
            Event::ConnectionChanged(ConnectionState::AuthenticationRequired(_))
        )
    });
    let methods = events
        .iter()
        .find_map(|event| match event {
            Event::ConnectionChanged(ConnectionState::AuthenticationRequired(methods)) => {
                Some(methods)
            }
            _ => None,
        })
        .unwrap();
    let api_key = methods
        .iter()
        .find(|method| method.id == "api-key")
        .unwrap();
    let chatgpt = methods
        .iter()
        .find(|method| method.id == "chat-gpt")
        .unwrap();

    assert_eq!(api_key.kind, AuthKind::Environment);
    assert_eq!(
        api_key.setup.as_deref(),
        Some("variables: CODEX_API_KEY, OPENAI_API_KEY")
    );
    assert_eq!(
        api_key.can_authenticate,
        ["CODEX_API_KEY", "OPENAI_API_KEY"]
            .iter()
            .any(|name| std::env::var_os(name).is_some_and(|value| !value.is_empty()))
    );
    assert_eq!(chatgpt.kind, AuthKind::Agent);
    assert!(chatgpt.can_authenticate);
    assert_eq!(
        methods
            .iter()
            .map(|method| method.id.as_str())
            .collect::<Vec<_>>(),
        ["api-key", "chat-gpt"]
    );

    controller
        .send(Command::Authenticate("chat-gpt".into()))
        .unwrap();
    let authenticated = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    assert!(
        !authenticated
            .iter()
            .any(|event| matches!(event, Event::Error(_)))
    );
}

#[test]
fn claude_subscription_auth_uses_the_adapters_terminal_login() {
    let project = tempfile::tempdir().unwrap();
    let auth_marker = project.path().join("claude-authenticated");
    let controller = AgentController::start_process_for(
        ProviderId::Claude,
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec![
            "--claude-auth".into(),
            auth_marker.to_string_lossy().into_owned(),
        ],
    );

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(
            event,
            Event::ConnectionChanged(ConnectionState::AuthenticationRequired(_))
        )
    });
    let methods = events
        .iter()
        .find_map(|event| match event {
            Event::ConnectionChanged(ConnectionState::AuthenticationRequired(methods)) => {
                Some(methods)
            }
            _ => None,
        })
        .unwrap();

    assert!(matches!(methods.as_slice(), [method]
        if method.id == "claude-ai-login"
            && method.name == "Claude Subscription"
            && method.kind == AuthKind::Terminal
            && method.setup.as_deref() == Some("arguments: --cli auth login --claudeai")
            && method.can_authenticate));

    controller
        .send(Command::Authenticate("claude-ai-login".into()))
        .unwrap();
    let completed = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    assert!(auth_marker.is_file());
    assert!(
        !completed
            .iter()
            .any(|event| matches!(event, Event::Error(_)))
    );
}

#[test]
fn history_and_cursor_only_controls_are_capability_events() {
    for (provider, args, expected_history, expected_allow_all) in [
        (ProviderId::Cursor, vec!["--sessions".into()], true, true),
        (ProviderId::Codex, Vec::new(), false, false),
    ] {
        let project = tempfile::tempdir().unwrap();
        let controller = AgentController::start_process_for(
            provider,
            project.path().to_path_buf(),
            env!("CARGO_BIN_EXE_editur-fake-agent").into(),
            args,
        );
        let events = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(
                event,
                Event::SessionReady { .. } | Event::SessionLoaded { .. }
            )
        });
        assert!(events.iter().any(|event| matches!(
            event,
            Event::Capabilities { history, allow_run_everything }
                if (*history, *allow_run_everything) == (expected_history, expected_allow_all)
        )));
    }
}

#[test]
fn pinned_codex_fixture_drives_history_and_standard_session_controls() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/codex-acp-1.1.14.json");
    let fixture_text = std::fs::read_to_string(&fixture).unwrap();
    for forbidden in ["/Users/", "/home/", "accountId", "apiKey", "sk-"] {
        assert!(!fixture_text.contains(forbidden));
    }
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process_for(
        ProviderId::Codex,
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec![
            "--codex-fixture".into(),
            fixture.to_string_lossy().into_owned(),
        ],
    );

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    assert!(events.iter().any(|event| matches!(
        event,
        Event::Capabilities {
            history: true,
            allow_run_everything: false
        }
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::SessionReady { config_options, .. }
            if config_options.iter().map(|option| option.id.as_str()).collect::<Vec<_>>()
                == ["mode", "collaboration_mode", "model", "reasoning_effort", "fast-mode"]
    )));
}

#[test]
fn pinned_claude_fixture_drives_shared_history_attachments_and_session_controls() {
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/claude-agent-acp-0.66.0.json");
    let fixture_text = std::fs::read_to_string(&fixture).unwrap();
    for forbidden in [
        "/Users/",
        "/home/",
        "accountId",
        "organizationId",
        "apiKey",
        "sk-ant-",
    ] {
        assert!(!fixture_text.contains(forbidden));
    }
    let project = tempfile::tempdir().unwrap();
    let image_path = project.path().join("fixture.png");
    std::fs::write(&image_path, [137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 0]).unwrap();
    let controller = AgentController::start_process_for(
        ProviderId::Claude,
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec![
            "--claude-fixture".into(),
            fixture.to_string_lossy().into_owned(),
        ],
    );

    let ready = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    assert!(ready.iter().any(|event| matches!(
        event,
        Event::Capabilities {
            history: true,
            allow_run_everything: false
        }
    )));
    assert!(ready.iter().any(|event| matches!(
        event,
        Event::SessionReady { config_options, .. }
            if config_options.iter().map(|option| option.id.as_str()).collect::<Vec<_>>()
                == ["mode", "model", "effort", "fast", "agent"]
    )));

    controller
        .send(Command::PromptWithAttachments {
            text: "image".into(),
            attachments: vec![PromptAttachment::from_path(&image_path).unwrap()],
        })
        .unwrap();
    let prompt = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });
    assert!(
        prompt
            .iter()
            .any(|event| matches!(event, Event::AssistantDelta(text) if text == "image/png:16"))
    );
}

#[test]
fn reconnecting_restores_the_newest_project_session() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec!["--sessions".into()],
    );
    let events = receive_until(
        &controller,
        Duration::from_secs(5),
        |event| matches!(event, Event::ActiveSessionChanged(id) if id == "newest-session"),
    );

    assert!(events.iter().any(|event| matches!(
        event,
        Event::SessionsUpdated(sessions)
            if sessions.len() == 2 && sessions[0].title.as_deref() == Some("Newest task")
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::UserMessage(text) if text == "restored prompt"
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        Event::AssistantDelta(text) if text == "restored reply"
    )));

    controller
        .send(Command::LoadSession("older-session".into()))
        .unwrap();
    let older = receive_until(
        &controller,
        Duration::from_secs(5),
        |event| matches!(event, Event::ActiveSessionChanged(id) if id == "older-session"),
    );
    assert!(older.iter().any(|event| matches!(
        event,
        Event::AssistantDelta(text) if text == "older reply"
    )));

    controller
        .send(Command::Prompt("follow up".into()))
        .unwrap();
    let follow_up = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });
    assert!(follow_up.iter().any(|event| matches!(
        event,
        Event::AssistantDelta(text) if text == "first "
    )));
}

#[test]
fn reconnecting_can_restore_the_providers_active_session() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process_resuming(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec!["--sessions".into()],
        "older-session".into(),
    );
    let events = receive_until(
        &controller,
        Duration::from_secs(5),
        |event| matches!(event, Event::ActiveSessionChanged(id) if id == "older-session"),
    );

    assert!(events.iter().any(|event| matches!(
        event,
        Event::AssistantDelta(text) if text == "older reply"
    )));
}

#[test]
fn removed_sessions_stay_out_of_history_after_reconnecting() {
    let project = tempfile::tempdir().unwrap();
    let start = || {
        AgentController::start_process(
            project.path().to_path_buf(),
            env!("CARGO_BIN_EXE_editur-fake-agent").into(),
            vec!["--sessions".into()],
        )
    };
    let controller = start();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionLoaded { .. })
    });

    controller
        .send(Command::RemoveSession("older-session".into()))
        .unwrap();
    let removed = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionsUpdated(_))
    });
    assert!(removed.iter().any(|event| matches!(
        event,
        Event::SessionsUpdated(sessions)
            if sessions.iter().all(|session| session.id != "older-session")
    )));
    drop(controller);

    let reopened = start();
    let restored = receive_until(&reopened, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionLoaded { .. })
    });
    assert!(restored.iter().any(|event| matches!(
        event,
        Event::SessionsUpdated(sessions)
            if sessions.len() == 1 && sessions[0].id == "newest-session"
    )));
}

#[test]
fn stale_listed_session_is_removed_without_a_transcript_error() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec!["--sessions".into(), "--stale-session".into()],
    );
    receive_until(
        &controller,
        Duration::from_secs(5),
        |event| matches!(event, Event::ActiveSessionChanged(id) if id == "newest-session"),
    );

    controller
        .send(Command::LoadSession("stale-session".into()))
        .unwrap();
    let events = receive_until(
        &controller,
        Duration::from_secs(5),
        |event| matches!(event, Event::SessionsUpdated(sessions) if sessions.iter().all(|session| session.id != "stale-session")),
    );

    assert!(!events.iter().any(|event| matches!(event, Event::Error(_))));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ConnectionChanged(ConnectionState::Ready)))
    );
}

#[test]
fn fake_agent_streams_a_prompt_and_keeps_the_session_for_a_follow_up() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    controller.send(Command::Prompt("first".into())).unwrap();
    let first = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });
    controller.send(Command::Prompt("second".into())).unwrap();
    let second = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });

    let text = |events: &[Event]| {
        events
            .iter()
            .filter_map(|event| match event {
                Event::AssistantDelta(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>()
    };
    assert_eq!(
        (text(&first), text(&second)),
        ("first reply".into(), "second reply".into())
    );
    controller.send(Command::Shutdown).unwrap();
}

#[test]
fn advertised_session_controls_round_trip_exact_values() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    let ready = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    assert!(ready.iter().any(|event| matches!(
        event,
        Event::SessionReady { current_mode: Some(mode), modes, config_options }
            if mode == "ask" && modes.iter().any(|mode| mode.id == "agent")
                && config_options.iter().any(|option| option.id == "model")
                && config_options.iter().any(|option| {
                    option.id == "thoughts" && option.value == ConfigValue::Boolean(false)
                })
    )));

    controller.send(Command::SetMode("agent".into())).unwrap();
    receive_until(
        &controller,
        Duration::from_secs(5),
        |event| matches!(event, Event::ModeChanged(mode) if mode == "agent"),
    );
    controller
        .send(Command::SetConfig {
            id: "model".into(),
            value: ConfigValue::Select("fast".into()),
        })
        .unwrap();
    let updated = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::ConfigOptionsUpdated(_))
    });
    assert!(updated.iter().any(|event| matches!(
        event,
        Event::ConfigOptionsUpdated(options)
            if options.iter().any(|option| option.id == "model"
                && option.value == ConfigValue::Select("fast".into()))
    )));
}

#[test]
fn rejected_control_does_not_end_the_connection() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    controller.send(Command::SetMode("invalid".into())).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::Error(_))
    });
    controller
        .send(Command::Prompt("still alive".into()))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });
}

#[test]
fn rejected_prompt_reports_an_error_and_finishes_the_turn() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    controller.send(Command::Prompt("error".into())).unwrap();
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });

    assert!(events.iter().any(|event| matches!(event, Event::Error(_))));
}

#[test]
fn split_tool_updates_render_supplied_details_and_unknown_notifications_are_ignored() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    controller.send(Command::Prompt("tool".into())).unwrap();
    let tool_events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });
    let tools = tool_events
        .iter()
        .filter_map(|event| match event {
            Event::ToolCallUpdated(tool) => Some(tool),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tools.len(), 2);
    assert_eq!(tools[0].status.as_deref(), Some("InProgress"));
    assert_eq!(tools[1].status.as_deref(), Some("Completed"));
    assert!(
        tools[1]
            .detail
            .as_ref()
            .is_some_and(|detail| detail.content.iter().any(|content| matches!(
                content,
                editur::agent::controller::ToolOutput::Diff { new_text, .. }
                    if new_text.contains("after")
            )))
    );

    controller.send(Command::Prompt("unknown".into())).unwrap();
    let unknown = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });
    assert!(
        unknown
            .iter()
            .any(|event| matches!(event, Event::AssistantDelta(text) if text == "unknown ignored"))
    );
}

#[test]
fn permission_request_waits_for_each_exact_supplied_choice() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    for selected in ["allow_once", "reject_once"] {
        controller
            .send(Command::Prompt("permission".into()))
            .unwrap();
        let request = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::PermissionRequested(_))
        })
        .into_iter()
        .find_map(|event| match event {
            Event::PermissionRequested(request) => Some(request),
            _ => None,
        })
        .unwrap();
        assert_eq!(request.action, "Run a sensitive command");
        assert_eq!(
            request
                .options
                .iter()
                .map(|option| option.id.as_str())
                .collect::<Vec<_>>(),
            ["allow_once", "reject_once"]
        );
        controller
            .send(Command::DecidePermission {
                request_id: request.request_id,
                option_id: selected.into(),
            })
            .unwrap();
        let events = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::TurnFinished { .. })
        });
        assert!(
            events
                .iter()
                .any(|event| matches!(event, Event::AssistantDelta(text) if text == selected))
        );
    }
    controller.send(Command::Shutdown).unwrap();
}

#[test]
fn run_everything_changes_without_starting_an_agent_turn() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    controller.send(Command::SetRunEverything(true)).unwrap();
    controller.send(Command::Prompt("first".into())).unwrap();
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });

    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::UserMessage(text) if text == "first"))
    );
    let reply = events
        .iter()
        .filter_map(|event| match event {
            Event::AssistantDelta(text) => Some(text.as_str()),
            _ => None,
        })
        .collect::<String>();
    assert_eq!(reply, "first reply");
    assert!(!events.iter().any(|event| matches!(event, Event::Error(_))));
    controller.send(Command::Shutdown).unwrap();
}

#[test]
fn run_everything_auto_approves_acp_permissions_with_allow_always() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    controller.send(Command::SetRunEverything(true)).unwrap();
    controller
        .send(Command::Prompt("permission-always".into()))
        .unwrap();
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(
            event,
            Event::PermissionRequested(_) | Event::TurnFinished { .. }
        )
    });

    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::PermissionRequested(_))),
        "Yolo mode leaked a permission request to the UI: {events:?}"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::AssistantDelta(text) if text == "allow_always"))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::TurnFinished { .. }))
    );
    controller.send(Command::Shutdown).unwrap();
}

#[test]
fn cursor_question_extension_round_trips_rich_answers() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    controller
        .send(Command::Prompt("cursor-question".into()))
        .unwrap();
    let request = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::InteractionRequested(_))
    })
    .into_iter()
    .find_map(|event| match event {
        Event::InteractionRequested(request) => Some(request),
        _ => None,
    })
    .unwrap();
    controller
        .send(Command::RespondInteraction {
            request_id: request.request_id,
            response: InteractionResponse::Answers(vec![QuestionAnswer {
                question_id: "q".into(),
                selected_option_ids: vec!["a".into(), "b".into()],
            }]),
        })
        .unwrap();
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });

    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::AssistantDelta(text) if text == "answered"))
    );
}

#[test]
fn cursor_extension_notifications_reach_structured_tool_cards() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    controller
        .send(Command::Prompt("cursor-notification".into()))
        .unwrap();
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });

    assert!(events.iter().any(|event| matches!(
        event,
        Event::ToolCallUpdated(tool)
            if tool.id == "todos-1" && tool.detail.as_ref().is_some_and(|detail| matches!(
                detail.content.as_slice(),
                [editur::agent::controller::ToolOutput::Todo { content, .. }]
                    if content == "Ship it"
            ))
    )));
}

#[test]
fn cursor_extensions_are_ignored_for_a_non_cursor_provider() {
    for provider in [ProviderId::Codex, ProviderId::Claude] {
        let project = tempfile::tempdir().unwrap();
        let controller = AgentController::start_process_for(
            provider,
            project.path().to_path_buf(),
            env!("CARGO_BIN_EXE_editur-fake-agent").into(),
            Vec::new(),
        );
        receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::SessionReady { .. })
        });

        controller
            .send(Command::Prompt("cursor-question".into()))
            .unwrap();
        let question = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::TurnFinished { .. })
        });
        assert!(
            !question
                .iter()
                .any(|event| { matches!(event, Event::InteractionRequested(_) | Event::Error(_)) })
        );
        assert!(
            question
                .iter()
                .any(|event| matches!(event, Event::AssistantDelta(text) if text == "cancelled"))
        );

        controller
            .send(Command::Prompt("cursor-notification".into()))
            .unwrap();
        let notification = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::TurnFinished { .. })
        });
        assert!(
            !notification
                .iter()
                .any(|event| matches!(event, Event::ToolCallUpdated(_)))
        );
    }
}

#[test]
fn standard_acp_behavior_is_shared_by_all_provider_ids() {
    for provider in [ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude] {
        let project = tempfile::tempdir().unwrap();
        let controller = AgentController::start_process_for(
            provider,
            project.path().to_path_buf(),
            env!("CARGO_BIN_EXE_editur-fake-agent").into(),
            Vec::new(),
        );
        receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::SessionReady { .. })
        });

        controller.send(Command::Prompt("first".into())).unwrap();
        let first = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::TurnFinished { .. })
        });
        assert!(
            first
                .iter()
                .any(|event| matches!(event, Event::AssistantDelta(text) if text == "first "))
        );

        controller
            .send(Command::Prompt("follow-up".into()))
            .unwrap();
        let follow_up = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::TurnFinished { .. })
        });
        assert!(
            follow_up
                .iter()
                .any(|event| matches!(event, Event::AssistantDelta(text) if text == "second "))
        );

        controller.send(Command::Prompt("tool".into())).unwrap();
        let events = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::TurnFinished { .. })
        });
        assert!(events.iter().any(|event| matches!(
            event,
            Event::ToolCallUpdated(tool) if tool.id == "fake-edit"
        )));

        controller
            .send(Command::Prompt("permission".into()))
            .unwrap();
        let permission = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::PermissionRequested(_))
        })
        .into_iter()
        .find_map(|event| match event {
            Event::PermissionRequested(request) => Some(request),
            _ => None,
        })
        .unwrap();
        controller
            .send(Command::DecidePermission {
                request_id: permission.request_id,
                option_id: "allow_once".into(),
            })
            .unwrap();
        let decided = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::TurnFinished { .. })
        });
        assert!(
            decided
                .iter()
                .any(|event| matches!(event, Event::AssistantDelta(text) if text == "allow_once"))
        );

        controller.send(Command::Prompt("wait".into())).unwrap();
        receive_until(
            &controller,
            Duration::from_secs(5),
            |event| matches!(event, Event::UserMessage(text) if text == "wait"),
        );
        controller.send(Command::Cancel).unwrap();
        let cancelled = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::TurnFinished { .. })
        });
        assert!(
            cancelled
                .iter()
                .any(|event| matches!(event, Event::TurnFinished { cancelled: true }))
        );

        controller.send(Command::Shutdown).unwrap();
        receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(
                event,
                Event::ConnectionChanged(ConnectionState::Disconnected)
            )
        });
    }
}

#[test]
fn malformed_permission_choices_are_cancelled_without_partial_ui() {
    for prompt in ["permission-overflow", "permission-empty"] {
        let project = tempfile::tempdir().unwrap();
        let controller = AgentController::start_process(
            project.path().to_path_buf(),
            env!("CARGO_BIN_EXE_editur-fake-agent").into(),
            Vec::new(),
        );
        receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::SessionReady { .. })
        });
        controller.send(Command::Prompt(prompt.into())).unwrap();
        let events = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(
                event,
                Event::PermissionRequested(_) | Event::Error(_) | Event::TurnFinished { .. }
            )
        });
        assert!(
            events.iter().any(
                |event| matches!(event, Event::Error(error) if error.contains("permission choices"))
            ),
            "malformed permission request was not rejected: {events:?}"
        );
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, Event::PermissionRequested(_)))
        );
        let finished = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::TurnFinished { .. })
        });
        assert!(
            finished
                .iter()
                .any(|event| matches!(event, Event::TurnFinished { cancelled: true }))
        );
        controller.send(Command::Shutdown).unwrap();
    }
}

#[test]
fn cancelling_a_pending_permission_finishes_the_turn_as_cancelled() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    controller
        .send(Command::Prompt("permission".into()))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::PermissionRequested(_))
    });

    controller.send(Command::Cancel).unwrap();
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::TurnFinished { .. })
    });

    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::TurnFinished { cancelled: true }))
    );
    controller.send(Command::Shutdown).unwrap();
}

#[test]
fn unexpected_agent_exit_preserves_bounded_stderr_diagnostics() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    controller.send(Command::Prompt("exit".into())).unwrap();

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::ProcessExited { .. })
    });

    assert!(events.iter().any(|event| matches!(
        event,
        Event::ProcessExited { diagnostics, .. } if diagnostics.contains("fake diagnostic")
    )));
    assert!(
        events.iter().any(|event| matches!(
            event,
            Event::ProcessExited { error, .. }
                if error.contains("Process exited")
                    && !error.contains("spawned_at")
                    && !error.contains("fake diagnostic")
        )),
        "process exit should be concise and omit SDK internals: {events:?}"
    );
}

#[test]
fn codex_stderr_never_copies_provider_payloads_into_events() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process_for(
        ProviderId::Codex,
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    controller
        .send(Command::Prompt("exit-secret".into()))
        .unwrap();

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::ProcessExited { .. })
    });

    assert!(
        events
            .iter()
            .all(|event| { !format!("{event:?}").contains("super-secret-test-value") })
    );
    assert!(events.iter().any(|event| matches!(
        event,
        Event::ProcessExited { diagnostics, .. }
            if diagnostics == "Codex stderr suppressed to protect authentication and protocol data."
    )));
}

#[test]
fn claude_stderr_never_copies_provider_payloads_into_events() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process_for(
        ProviderId::Claude,
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    controller
        .send(Command::Prompt("exit-secret".into()))
        .unwrap();

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::ProcessExited { .. })
    });

    assert!(
        events
            .iter()
            .all(|event| { !format!("{event:?}").contains("super-secret-test-value") })
    );
    assert!(events.iter().any(|event| matches!(
        event,
        Event::ProcessExited { diagnostics, .. }
            if diagnostics == "Claude stderr suppressed to protect authentication and protocol data."
    )));
}

#[test]
fn malformed_agent_stdout_becomes_a_connection_error() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        Vec::new(),
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    controller
        .send(Command::Prompt("malformed".into()))
        .unwrap();

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::ProcessExited { .. })
    });

    assert!(
        events
            .iter()
            .any(|event| matches!(event, Event::ProcessExited { error, .. }
        if error.to_ascii_lowercase().contains("json")))
    );
}

#[test]
fn clean_shutdown_waits_for_the_fake_process_to_exit() {
    let project = tempfile::tempdir().unwrap();
    let address_file = project.path().join("fake-address");
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec![address_file.to_string_lossy().into_owned()],
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });

    controller.send(Command::Shutdown).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(
            event,
            Event::ConnectionChanged(editur::agent::controller::ConnectionState::Disconnected)
        )
    });

    let address = std::fs::read_to_string(address_file).unwrap();
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        if std::net::TcpListener::bind(&address).is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fake process still owns its marker socket"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
#[test]
fn unix_shutdown_terminates_the_descendant_tree() {
    let project = tempfile::tempdir().unwrap();
    let address_file = project.path().join("descendant-address");
    let controller = AgentController::start_process_for(
        ProviderId::Claude,
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec![
            "--descendant".into(),
            address_file.to_string_lossy().into_owned(),
        ],
    );
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionReady { .. })
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while !address_file.is_file() {
        assert!(Instant::now() < deadline, "fake descendant did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let address = std::fs::read_to_string(&address_file).unwrap();

    controller.send(Command::Shutdown).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(
            event,
            Event::ConnectionChanged(editur::agent::controller::ConnectionState::Disconnected)
        )
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::net::TcpListener::bind(&address).is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fake descendant survived Unix process-group shutdown"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(windows)]
#[test]
fn windows_kill_on_close_job_terminates_the_descendant_tree() {
    let project = tempfile::tempdir().unwrap();
    let address_file = project.path().join("descendant-address");
    let (job_name, job) = editur::agent::new_windows_job().unwrap();
    let mut parent = std::process::Command::new(env!("CARGO_BIN_EXE_editur-fake-agent"))
        .arg("--job-parent")
        .arg(&job_name)
        .arg(&address_file)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !address_file.is_file() {
        assert!(Instant::now() < deadline, "fake descendant did not start");
        std::thread::sleep(Duration::from_millis(10));
    }
    let address = std::fs::read_to_string(&address_file).unwrap();
    assert!(std::net::TcpListener::bind(&address).is_err());

    drop(job);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if std::net::TcpListener::bind(&address).is_ok() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "fake descendant survived job closure"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(parent.wait().unwrap().code().is_some());
}
