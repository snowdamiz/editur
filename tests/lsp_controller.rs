use std::{
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

use editur::lsp::{
    Command, Controller, DiagnosticSeverity, DocumentSnapshot, Event, RequestTag, ServerLaunch,
    ServerStatus,
};

fn receive_until(
    controller: &Controller,
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
    panic!("timed out waiting for LSP event: {events:?}");
}

#[test]
fn controller_initializes_syncs_and_shuts_down_one_document() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    let record = project.path().join("messages.jsonl");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec!["--record".into(), record.to_string_lossy().into_owned()],
        Arc::new(|| {}),
    );

    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "fn main() {}\n".into(),
            revision: 0,
        }))
        .unwrap();
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "fn main() {}\n".into(),
            revision: 0,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    controller
        .send(Command::Change {
            path: path.clone(),
            text: "fn main() { println!(\"💡\"); }\n".into(),
            revision: 1,
        })
        .unwrap();
    controller.send(Command::Save(path.clone())).unwrap();
    controller.send(Command::Close(path)).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Stopped))
    });

    let messages = std::fs::read_to_string(record)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let methods = messages
        .iter()
        .filter_map(|message| message.get("method").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    assert_eq!(
        methods,
        [
            "initialize",
            "initialized",
            "textDocument/didOpen",
            "textDocument/didChange",
            "textDocument/didSave",
            "textDocument/didClose",
            "shutdown",
            "exit",
        ]
    );
    let change = messages
        .iter()
        .find(|message| {
            message.get("method").and_then(serde_json::Value::as_str)
                == Some("textDocument/didChange")
        })
        .unwrap();
    assert_eq!(
        change.pointer("/params/textDocument/version"),
        Some(&1.into())
    );
    assert!(change.pointer("/params/contentChanges/0/range").is_some());
}

#[test]
fn restart_discards_messages_from_the_old_process() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    let record = project.path().join("messages.jsonl");
    std::fs::write(&path, "fn main() {}\n").unwrap();
    let command = PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp"));
    let args = vec![
        "--record".into(),
        record.to_string_lossy().into_owned(),
        "--late-response".into(),
    ];
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        command.clone(),
        args.clone(),
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "fn main() {}\n".into(),
            revision: 0,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });

    controller
        .send(Command::Restart(ServerLaunch::Custom {
            command: command.to_string_lossy().into_owned(),
            args,
        }))
        .unwrap();
    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    assert!(!events.iter().any(|event| matches!(
        event,
        Event::ProcessExited { error, .. } if error.contains("response ID 777")
    )));

    controller.send(Command::Close(path)).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Stopped))
    });
    let messages = std::fs::read_to_string(record).unwrap();
    assert_eq!(messages.matches("\"method\":\"initialize\"").count(), 2);
    assert_eq!(
        messages
            .matches("\"method\":\"textDocument/didOpen\"")
            .count(),
        2
    );
}

#[test]
fn versioned_diagnostics_are_normalized_to_current_utf8_ranges() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    std::fs::write(&path, "let 💡 = 1;\n").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec!["--diagnostics".into()],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "let 💡 = 1;\n".into(),
            revision: 0,
        }))
        .unwrap();

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::Diagnostics { .. })
    });
    let Event::Diagnostics {
        path: actual,
        revision,
        diagnostics,
        ..
    } = events.last().unwrap()
    else {
        unreachable!()
    };
    assert_eq!((actual, *revision), (&path, 0));
    assert!(matches!(diagnostics.as_slice(), [diagnostic]
        if diagnostic.range == (4..8)
            && diagnostic.severity == DiagnosticSeverity::Error
            && diagnostic.message == "fake error"));
}

#[test]
fn completion_filters_snippets_and_preserves_the_request_context() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    std::fs::write(&path, "foo.").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec![],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "foo.".into(),
            revision: 4,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    let tag = RequestTag {
        path,
        revision: 4,
        cursor: 4,
    };
    controller
        .send(Command::Complete {
            tag: tag.clone(),
            trigger: Some(".".into()),
        })
        .unwrap();

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::Completion { .. })
    });
    let Event::Completion {
        tag: actual, items, ..
    } = events.last().unwrap()
    else {
        unreachable!()
    };
    assert_eq!(actual, &tag);
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].label, "print");
    assert_eq!(items[0].detail.as_deref(), Some("Function details"));
    assert_eq!(items[0].edit.as_ref().unwrap().range, 0..3);
    assert_eq!(items[0].edit.as_ref().unwrap().new_text, "println");
    assert_eq!(items[1].insert_text, "fallback");
}

#[test]
fn feature_errors_do_not_terminate_the_server() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    std::fs::write(&path, "pri").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec!["--feature-errors".into()],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "pri".into(),
            revision: 0,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    controller
        .send(Command::Complete {
            tag: RequestTag {
                path: path.clone(),
                revision: 0,
                cursor: 3,
            },
            trigger: None,
        })
        .unwrap();

    let events = receive_until(
        &controller,
        Duration::from_secs(5),
        |event| matches!(event, Event::ServerMessage(message) if message.contains("request cancelled")),
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, Event::ProcessExited { .. }))
    );
    controller.send(Command::Close(path)).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Stopped))
    });
}

#[test]
fn hover_normalizes_markdown_and_the_optional_range() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    std::fs::write(&path, "foo").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec![],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "foo".into(),
            revision: 2,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    let tag = RequestTag {
        path,
        revision: 2,
        cursor: 1,
    };
    controller.send(Command::Hover(tag.clone())).unwrap();

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::Hover { .. })
    });
    let Event::Hover {
        tag: actual,
        content,
    } = events.last().unwrap()
    else {
        unreachable!()
    };
    assert_eq!(actual, &tag);
    let content = content.as_ref().unwrap();
    assert_eq!(content.text, "**fake hover**");
    assert!(content.markdown);
    assert_eq!(content.range, Some(0..3));
}

#[test]
fn multiple_definition_locations_remain_local_and_ordered() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    std::fs::write(&path, "foo\nbar\n").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec![],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "foo\nbar\n".into(),
            revision: 0,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    let tag = RequestTag {
        path: path.clone(),
        revision: 0,
        cursor: 1,
    };
    controller.send(Command::Definition(tag.clone())).unwrap();

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::Definitions { .. })
    });
    let Event::Definitions {
        tag: actual,
        locations,
        ..
    } = events.last().unwrap()
    else {
        unreachable!()
    };
    assert_eq!(actual, &tag);
    assert_eq!(locations.len(), 2);
    assert_eq!(locations[0].path, path);
    assert_eq!((locations[0].line, locations[0].character), (0, 0));
    assert_eq!((locations[1].line, locations[1].character), (1, 0));
}

#[test]
fn one_definition_location_is_normalized_directly() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    std::fs::write(&path, "foo").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec!["--one-definition".into()],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "foo".into(),
            revision: 0,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    controller
        .send(Command::Definition(RequestTag {
            path,
            revision: 0,
            cursor: 1,
        }))
        .unwrap();

    let events = receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::Definitions { .. })
    });
    assert!(
        matches!(events.last(), Some(Event::Definitions { locations, .. }) if locations.len() == 1)
    );
}

#[test]
fn feature_responses_are_dropped_after_the_document_changes() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    std::fs::write(&path, "foo.").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec!["--delay-features".into()],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "foo.".into(),
            revision: 0,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    controller
        .send(Command::Complete {
            tag: RequestTag {
                path: path.clone(),
                revision: 0,
                cursor: 4,
            },
            trigger: None,
        })
        .unwrap();
    controller
        .send(Command::Hover(RequestTag {
            path: path.clone(),
            revision: 0,
            cursor: 2,
        }))
        .unwrap();
    controller
        .send(Command::Definition(RequestTag {
            path: path.clone(),
            revision: 0,
            cursor: 2,
        }))
        .unwrap();
    controller
        .send(Command::Change {
            path,
            text: "foo.bar".into(),
            revision: 1,
        })
        .unwrap();

    let deadline = Instant::now() + Duration::from_millis(750);
    while Instant::now() < deadline {
        if let Ok(event) = controller.events().recv_timeout(Duration::from_millis(50)) {
            assert!(
                !matches!(
                    event,
                    Event::Completion { .. } | Event::Hover { .. } | Event::Definitions { .. }
                ),
                "stale event: {event:?}"
            );
        }
    }
}

#[test]
fn full_sync_and_split_server_writes_are_honored() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    let record = project.path().join("messages.jsonl");
    std::fs::write(&path, "old").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec![
            "--full-sync".into(),
            "--split-writes".into(),
            "--record".into(),
            record.to_string_lossy().into_owned(),
        ],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "old".into(),
            revision: 0,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    controller
        .send(Command::Change {
            path: path.clone(),
            text: "new".into(),
            revision: 1,
        })
        .unwrap();
    controller.send(Command::Save(path.clone())).unwrap();
    controller.send(Command::Close(path)).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Stopped))
    });

    let change = std::fs::read_to_string(record)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|message| {
            message.get("method").and_then(serde_json::Value::as_str)
                == Some("textDocument/didChange")
        })
        .unwrap();
    assert_eq!(
        change
            .pointer("/params/contentChanges/0/text")
            .and_then(serde_json::Value::as_str),
        Some("new")
    );
    assert!(change.pointer("/params/contentChanges/0/range").is_none());
}

#[test]
fn save_before_initialization_is_sent_after_the_document_opens() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    let record = project.path().join("messages.jsonl");
    std::fs::write(&path, "fn main() {}").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec![
            "--delay-initialize".into(),
            "--record".into(),
            record.to_string_lossy().into_owned(),
        ],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "fn main() {}".into(),
            revision: 0,
        }))
        .unwrap();
    controller.send(Command::Save(path.clone())).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    controller.send(Command::Close(path)).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Stopped))
    });

    let methods = std::fs::read_to_string(record)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter_map(|message| {
            message
                .get("method")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
        })
        .collect::<Vec<_>>();
    let open = methods
        .iter()
        .position(|method| method == "textDocument/didOpen")
        .unwrap();
    let save = methods
        .iter()
        .position(|method| method == "textDocument/didSave")
        .unwrap();
    assert!(open < save);
}

#[test]
fn conservative_server_requests_receive_bounded_responses() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    let record = project.path().join("messages.jsonl");
    std::fs::write(&path, "fn main() {}").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec![
            "--server-requests".into(),
            "--record".into(),
            record.to_string_lossy().into_owned(),
        ],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "fn main() {}".into(),
            revision: 0,
        }))
        .unwrap();
    receive_until(
        &controller,
        Duration::from_secs(5),
        |event| matches!(event, Event::ServerMessage(message) if message == "fake server message"),
    );
    controller.send(Command::Close(path)).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Stopped))
    });

    let messages = std::fs::read_to_string(record)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .collect::<Vec<_>>();
    let response = |id: i64| {
        messages
            .iter()
            .find(|message| {
                message.get("id") == Some(&id.into()) && message.get("method").is_none()
            })
            .unwrap()
    };
    assert_eq!(
        response(900)
            .get("result")
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        Some(2)
    );
    assert_eq!(
        response(903).pointer("/result/applied"),
        Some(&false.into())
    );
    assert_eq!(response(904).pointer("/error/code"), Some(&(-32601).into()));
}

#[test]
fn missing_sync_and_malformed_output_fail_only_the_controller() {
    for arguments in [
        vec!["--missing-sync".to_owned()],
        vec!["--malformed".to_owned()],
        vec!["--oversized".to_owned()],
        vec![
            "--exit-on-open".to_owned(),
            "--stderr-lines".to_owned(),
            "1000".to_owned(),
        ],
    ] {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("main.rs");
        std::fs::write(&path, "fn main() {}").unwrap();
        let controller = Controller::start_process(
            project.path().to_path_buf(),
            PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
            arguments.clone(),
            Arc::new(|| {}),
        );
        controller
            .send(Command::Open(DocumentSnapshot {
                path,
                language_id: "rust".into(),
                text: "fn main() {}".into(),
                revision: 0,
            }))
            .unwrap();
        let events = receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::ProcessExited { .. })
        });
        let Event::ProcessExited { stderr, .. } = events.last().unwrap() else {
            unreachable!()
        };
        assert!(stderr.len() <= 64 * 1024);
        if arguments.first().map(String::as_str) == Some("--exit-on-open") {
            assert!(stderr.contains("fake stderr line 000999"));
            assert!(!stderr.contains("fake stderr line 000000"));
        }
    }
}

#[test]
fn closing_the_last_document_clears_a_failed_server_status() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    std::fs::write(&path, "fn main() {}").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec!["--malformed".into()],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "fn main() {}".into(),
            revision: 0,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::ProcessExited { .. })
    });

    controller.send(Command::Close(path)).unwrap();
    receive_until(&controller, Duration::from_secs(1), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Stopped))
    });
}

#[test]
fn old_versioned_diagnostics_are_dropped_but_new_versionless_results_are_current() {
    for (argument, expect_diagnostic) in [
        ("--stale-diagnostics", false),
        ("--versionless-diagnostics", true),
    ] {
        let project = tempfile::tempdir().unwrap();
        let path = project.path().join("main.rs");
        std::fs::write(&path, "let 💡 = 1;").unwrap();
        let controller = Controller::start_process(
            project.path().to_path_buf(),
            PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
            vec![argument.into()],
            Arc::new(|| {}),
        );
        controller
            .send(Command::Open(DocumentSnapshot {
                path: path.clone(),
                language_id: "rust".into(),
                text: "let 💡 = 1;".into(),
                revision: 0,
            }))
            .unwrap();
        receive_until(&controller, Duration::from_secs(5), |event| {
            matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
        });
        controller
            .send(Command::Change {
                path: path.clone(),
                text: "let 💡 = 2;".into(),
                revision: 1,
            })
            .unwrap();
        controller.send(Command::Save(path)).unwrap();
        let deadline = Instant::now() + Duration::from_millis(750);
        let mut found = false;
        while Instant::now() < deadline {
            if let Ok(event) = controller.events().recv_timeout(Duration::from_millis(50))
                && matches!(event, Event::Diagnostics { revision: 1, .. })
            {
                found = true;
            }
        }
        assert_eq!(found, expect_diagnostic, "mode {argument}");
    }
}

#[cfg(unix)]
#[test]
fn graceful_shutdown_terminates_a_server_descendant() {
    let project = tempfile::tempdir().unwrap();
    let path = project.path().join("main.rs");
    let pid_file = project.path().join("descendant.pid");
    std::fs::write(&path, "fn main() {}").unwrap();
    let controller = Controller::start_process(
        project.path().to_path_buf(),
        PathBuf::from(env!("CARGO_BIN_EXE_editur-fake-lsp")),
        vec![
            "--descendant-pid".into(),
            pid_file.to_string_lossy().into_owned(),
        ],
        Arc::new(|| {}),
    );
    controller
        .send(Command::Open(DocumentSnapshot {
            path: path.clone(),
            language_id: "rust".into(),
            text: "fn main() {}".into(),
            revision: 0,
        }))
        .unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Ready(_)))
    });
    let pid = std::fs::read_to_string(&pid_file)
        .unwrap()
        .parse::<i32>()
        .unwrap();
    controller.send(Command::Close(path)).unwrap();
    receive_until(&controller, Duration::from_secs(5), |event| {
        matches!(event, Event::StateChanged(ServerStatus::Stopped))
    });

    unsafe extern "C" {
        fn kill(pid: i32, signal: i32) -> i32;
    }
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && unsafe { kill(pid, 0) } == 0 {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_ne!(
        unsafe { kill(pid, 0) },
        0,
        "descendant {pid} survived shutdown"
    );
}
