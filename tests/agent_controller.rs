use std::time::{Duration, Instant};

use editur::agent::controller::{
    AgentController, Command, ConfigValue, Event, InteractionResponse, QuestionAnswer,
};

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
fn reconnecting_restores_the_newest_project_session() {
    let project = tempfile::tempdir().unwrap();
    let controller = AgentController::start_process(
        project.path().to_path_buf(),
        env!("CARGO_BIN_EXE_editur-fake-agent").into(),
        vec!["--sessions".into()],
    );
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionLoaded { .. })
    });

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
    let older = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::SessionLoaded { .. })
    });
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
