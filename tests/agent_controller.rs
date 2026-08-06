use std::time::{Duration, Instant};

use editur::agent::controller::{AgentController, Command, ConfigValue, Event};

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
            .as_deref()
            .is_some_and(|detail| detail.contains("after"))
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
