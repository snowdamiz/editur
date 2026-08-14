use editur::agent::{
    controller::{
        ConnectionState, ContentRole, DisplayContent, Event, PermissionChoice, PermissionRequest,
        PlanItem, SessionTranscriptMessage, ToolActivity, ToolDetail, ToolOutput,
    },
    state::{AgentState, FileChange, TranscriptItem},
};

#[test]
fn plans_are_replaced_only_within_the_current_user_turn() {
    let mut state = AgentState::default();
    let plan = |content: &str| {
        Event::PlanUpdated(vec![PlanItem {
            content: content.into(),
            status: "Pending".into(),
        }])
    };

    state.apply(Event::UserMessage("first".into()));
    state.apply(plan("first draft"));
    state.apply(plan("first final"));
    state.apply(Event::TurnFinished { cancelled: false });
    state.apply(Event::UserMessage("second".into()));
    state.apply(plan("second"));

    let plans = state
        .transcript
        .iter()
        .filter_map(|item| match item {
            TranscriptItem::Plan(items) => Some(items[0].content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(plans, ["first final", "second"]);
}

#[test]
fn a_successful_new_session_clears_the_previous_transcript() {
    let mut state = AgentState::default();
    state.apply(Event::AssistantDelta("old session".into()));

    state.apply(Event::SessionReady {
        current_mode: None,
        modes: Vec::new(),
        config_options: Vec::new(),
    });

    assert!(state.transcript.is_empty());
}

#[test]
fn loading_a_session_keeps_the_replayed_transcript() {
    let mut state = AgentState::default();
    state.apply(Event::AssistantDelta("old session".into()));

    state.apply(Event::SessionLoading {
        title: Some("Restored task".into()),
    });
    state.apply(Event::UserMessage("restored prompt".into()));
    state.apply(Event::AssistantDelta("restored reply".into()));
    state.apply(Event::SessionLoaded {
        current_mode: None,
        modes: Vec::new(),
        config_options: Vec::new(),
    });

    assert_eq!(state.title.as_deref(), Some("Restored task"));
    assert_eq!(state.transcript.len(), 2);
    assert!(state.session_ready);
    assert!(!state.active);
}

#[test]
fn imported_sessions_restore_thoughts_and_tool_cards() {
    let mut state = AgentState::default();
    state.apply(Event::SessionTranscriptLoaded(vec![
        SessionTranscriptMessage::User("fix it".into()),
        SessionTranscriptMessage::Thought("checking".into()),
        SessionTranscriptMessage::Tool(ToolActivity {
            id: "tool-1".into(),
            title: Some("Bash".into()),
            status: Some("Completed".into()),
            kind: Some("Execute".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: Some("cargo test".into()),
                content: Vec::new(),
                output: Some("passed".into()),
            }),
        }),
        SessionTranscriptMessage::Assistant("done".into()),
    ]));

    assert!(matches!(state.transcript[0], TranscriptItem::User(ref text) if text == "fix it"));
    assert!(matches!(state.transcript[1], TranscriptItem::Thought(ref text) if text == "checking"));
    assert!(matches!(
        state.transcript[2],
        TranscriptItem::Tool(ref tool)
            if tool.id == "tool-1"
                && tool.detail.as_ref().and_then(|detail| detail.output.as_deref()) == Some("passed")
    ));
    assert!(matches!(state.transcript[3], TranscriptItem::Assistant(ref text) if text == "done"));
}

#[test]
fn imported_session_images_reach_the_transcript() {
    let mut state = AgentState::default();
    let bytes = std::sync::Arc::<[u8]>::from([1, 2, 3]);

    state.apply(Event::SessionTranscriptLoaded(vec![
        SessionTranscriptMessage::Content {
            role: ContentRole::User,
            content: DisplayContent::Image {
                mime_type: "image/png".into(),
                uri: None,
                encoded_bytes: 4,
                data: Some(bytes.clone()),
            },
        },
    ]));

    assert!(matches!(
        state.transcript.front(),
        Some(TranscriptItem::Content {
            role: ContentRole::User,
            content: DisplayContent::Image { data: Some(data), .. },
        }) if std::sync::Arc::ptr_eq(data, &bytes)
    ));
}

#[test]
fn imported_session_diffs_restore_tool_line_stats() {
    let mut state = AgentState::default();
    state.apply(Event::SessionTranscriptLoaded(vec![
        SessionTranscriptMessage::Tool(ToolActivity {
            id: "cursor-edit".into(),
            title: Some("Edit app.rs".into()),
            status: Some("Completed".into()),
            kind: Some("Edit".into()),
            paths: vec!["/w/app.rs".into()],
            detail: Some(ToolDetail {
                input: None,
                content: vec![ToolOutput::Diff {
                    path: "/w/app.rs".into(),
                    old_text: Some("keep\nremove\n".into()),
                    new_text: "keep\nadd one\nadd two\n".into(),
                }],
                output: None,
            }),
        }),
    ]));

    assert_eq!(
        state.tool_change("cursor-edit"),
        Some(FileChange {
            added: 2,
            removed: 1,
        })
    );
}

#[test]
fn failed_session_load_restores_the_active_transcript() {
    let mut state = AgentState::default();
    state.apply(Event::ConnectionChanged(ConnectionState::Ready));
    state.apply(Event::SessionTitleUpdated(Some("Current task".into())));
    state.apply(Event::UserMessage("keep this".into()));
    state.apply(Event::AssistantDelta("still here".into()));
    state.apply(Event::TurnFinished { cancelled: false });
    let transcript = state.transcript.clone();

    state.apply(Event::SessionLoading {
        title: Some("Missing task".into()),
    });
    state.apply(Event::SessionLoadFailed);
    state.apply(Event::ConnectionChanged(ConnectionState::Ready));

    assert_eq!(state.transcript, transcript);
    assert_eq!(state.title.as_deref(), Some("Current task"));
    assert!(state.session_ready);
}

#[test]
fn state_enforces_one_turn_orders_streams_and_answers_permission_once() {
    let mut state = AgentState::default();
    state.prompt = "ship it".into();
    state.apply(Event::ConnectionChanged(ConnectionState::Ready));
    state.apply(Event::SessionReady {
        current_mode: None,
        modes: Vec::new(),
        config_options: Vec::new(),
    });
    assert!(state.can_send(true));
    state.apply(Event::UserMessage("ship it".into()));
    state.apply(Event::AssistantDelta("one ".into()));
    state.apply(Event::AssistantDelta("two".into()));
    state.apply(Event::UsageUpdated {
        used: 25,
        size: 100,
        cost: Some("0.01 USD".into()),
    });
    assert!(!state.can_send(true));
    assert_eq!(
        state
            .usage
            .as_ref()
            .map(|usage| (usage.used, usage.size, usage.cost.as_deref())),
        Some((25, 100, Some("0.01 USD")))
    );

    state.apply(Event::PermissionRequested(PermissionRequest {
        request_id: 7,
        tool_call_id: "tool".into(),
        action: "run command".into(),
        options: vec![PermissionChoice {
            id: "allow_once".into(),
            name: "Allow once".into(),
            kind: "AllowOnce".into(),
        }],
    }));
    assert!(state.decide_permission(7, "allow_once"));
    assert!(!state.decide_permission(7, "allow_once"));
    assert!(matches!(
        state.transcript.iter().find(|item| matches!(item, TranscriptItem::Assistant(_))),
        Some(TranscriptItem::Assistant(text)) if text == "one two"
    ));

    state.apply(Event::ProcessExited {
        error: "gone".into(),
        diagnostics: String::new(),
    });
    assert!(
        state
            .transcript
            .iter()
            .any(|item| matches!(item, TranscriptItem::Assistant(text) if text == "one two"))
    );
    state.apply(Event::SessionReady {
        current_mode: None,
        modes: Vec::new(),
        config_options: Vec::new(),
    });
    assert!(state.session_ready);
    assert!(state.transcript.is_empty());
}

#[test]
fn finished_turns_finalize_tools_that_never_completed() {
    let running_tool = |id: &str| {
        Event::ToolCallUpdated(ToolActivity {
            id: id.into(),
            title: Some("Edit File".into()),
            status: Some("InProgress".into()),
            kind: None,
            paths: Vec::new(),
            detail: None,
        })
    };
    let tool_status = |state: &AgentState, id: &str| {
        state
            .transcript
            .iter()
            .find_map(|item| match item {
                TranscriptItem::Tool(tool) if tool.id == id => tool.status.clone(),
                _ => None,
            })
            .expect("tool should keep a status")
    };

    let mut state = AgentState::default();
    state.apply(Event::UserMessage("edit".into()));
    state.apply(running_tool("failed-edit"));
    state.apply(Event::ToolCallUpdated(ToolActivity {
        id: "done-edit".into(),
        title: None,
        status: Some("Completed".into()),
        kind: None,
        paths: Vec::new(),
        detail: None,
    }));
    state.apply(Event::Error(
        "agent turn failed: RetriableError: [canceled] http/2 stream closed".into(),
    ));
    state.apply(Event::TurnFinished { cancelled: false });
    assert_eq!(tool_status(&state, "failed-edit"), "Failed");
    assert_eq!(tool_status(&state, "done-edit"), "Completed");

    let mut state = AgentState::default();
    state.apply(Event::UserMessage("edit".into()));
    state.apply(running_tool("cancelled-edit"));
    state.apply(Event::TurnFinished { cancelled: true });
    assert_eq!(tool_status(&state, "cancelled-edit"), "Cancelled");

    let mut state = AgentState::default();
    state.apply(Event::UserMessage("edit".into()));
    state.apply(running_tool("orphaned-edit"));
    state.apply(Event::ProcessExited {
        error: "gone".into(),
        diagnostics: String::new(),
    });
    assert_eq!(tool_status(&state, "orphaned-edit"), "Failed");
}

#[test]
fn streamed_unicode_is_retained_without_truncation() {
    let mut state = AgentState::default();
    state.apply(Event::AssistantDelta("a".repeat(64 * 1024 - 1)));
    state.apply(Event::AssistantDelta("é".into()));

    assert!(matches!(
        state.transcript.back(),
        Some(TranscriptItem::Assistant(text)) if text.len() == 64 * 1024 + 1 && text.ends_with('é')
    ));
}

#[test]
fn transcript_retains_every_large_tool_card() {
    let mut state = AgentState::default();
    state.apply(Event::AssistantDelta("keep me".into()));
    for id in 0..17 {
        state.apply(Event::ToolCallUpdated(ToolActivity {
            id: id.to_string(),
            title: Some("tool".into()),
            status: Some("Completed".into()),
            kind: None,
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: None,
                content: vec![ToolOutput::Text("x".repeat(64 * 1024))],
                output: None,
            }),
        }));
    }

    assert_eq!(state.transcript.len(), 18);
    assert!(matches!(
        state.transcript.front(),
        Some(TranscriptItem::Assistant(text)) if text == "keep me"
    ));
}

#[test]
fn edit_tool_tracks_its_own_diff_totals() {
    let mut state = AgentState::default();
    state.apply(diff_event(
        "edit",
        "/w/lib.rs",
        Some("keep\nremove\n"),
        "keep\nadd one\nadd two\n",
    ));

    assert_eq!(
        state.tool_change("edit"),
        Some(FileChange {
            added: 2,
            removed: 1,
        })
    );
}

#[test]
fn tool_kind_survives_updates_that_omit_it() {
    let mut state = AgentState::default();
    state.apply(Event::ToolCallUpdated(ToolActivity {
        id: "task".into(),
        title: Some("Subagent: explore".into()),
        status: Some("InProgress".into()),
        kind: Some("Task".into()),
        paths: Vec::new(),
        detail: None,
    }));
    state.apply(Event::ToolCallUpdated(ToolActivity {
        id: "task".into(),
        title: None,
        status: Some("Completed".into()),
        kind: None,
        paths: Vec::new(),
        detail: None,
    }));

    assert!(matches!(
        state.transcript.back(),
        Some(TranscriptItem::Tool(tool))
            if tool.kind.as_deref() == Some("Task")
                && tool.status.as_deref() == Some("Completed")
    ));
}

#[test]
fn split_tool_updates_preserve_input_and_structured_output() {
    let mut state = AgentState::default();
    state.apply(Event::ToolCallUpdated(ToolActivity {
        id: "tool".into(),
        title: Some("Edit".into()),
        status: Some("InProgress".into()),
        kind: None,
        paths: Vec::new(),
        detail: Some(ToolDetail {
            input: Some("command".into()),
            content: Vec::new(),
            output: None,
        }),
    }));
    state.apply(Event::ToolCallUpdated(ToolActivity {
        id: "tool".into(),
        title: None,
        status: Some("Completed".into()),
        kind: None,
        paths: Vec::new(),
        detail: Some(ToolDetail {
            input: None,
            content: vec![ToolOutput::Text("done".into())],
            output: Some("result".into()),
        }),
    }));

    assert!(matches!(
        state.transcript.back(),
        Some(TranscriptItem::Tool(ToolActivity {
            detail: Some(ToolDetail { input: Some(input), content, output: Some(output) }),
            ..
        })) if input == "command" && output == "result"
            && matches!(content.as_slice(), [ToolOutput::Text(text)] if text == "done")
    ));
}

#[test]
fn transcript_tools_are_retained_while_changed_paths_stay_bounded() {
    let mut state = AgentState::default();
    for id in 0..5_000 {
        state.apply(Event::ToolCallUpdated(ToolActivity {
            id: id.to_string(),
            title: None,
            status: None,
            kind: Some("Edit".into()),
            paths: vec![format!("/tmp/{id}").into()],
            detail: None,
        }));
    }

    assert_eq!(state.transcript.len(), 5_000);
    assert!(state.changed_paths.len() <= 4_096);
}

#[test]
fn only_modifying_tools_mark_files_as_changed() {
    let mut state = AgentState::default();
    let tool = |id: &str, kind: &str, path: &str| ToolActivity {
        id: id.into(),
        title: None,
        status: Some("Completed".into()),
        kind: Some(kind.into()),
        paths: vec![path.into()],
        detail: None,
    };
    state.apply(Event::ToolCallUpdated(tool("read", "Read", "/w/read.rs")));
    state.apply(Event::ToolCallUpdated(tool(
        "search",
        "Search",
        "/w/found.rs",
    )));
    state.apply(Event::ToolCallUpdated(tool("edit", "Edit", "/w/edited.rs")));
    state.apply(Event::ToolCallUpdated(tool(
        "delete",
        "Delete",
        "/w/deleted.rs",
    )));

    assert_eq!(
        {
            let mut paths = state
                .changed_paths
                .keys()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>();
            paths.sort();
            paths
        },
        ["/w/deleted.rs", "/w/edited.rs"]
    );
    assert_eq!(state.refresh_queue.len(), 4, "refresh stays conservative");
}

#[test]
fn a_diff_marks_its_file_changed_even_when_the_kind_arrives_earlier() {
    let mut state = AgentState::default();
    state.apply(Event::ToolCallUpdated(ToolActivity {
        id: "edit".into(),
        title: None,
        status: Some("InProgress".into()),
        kind: Some("Other".into()),
        paths: vec!["/w/read-location.rs".into()],
        detail: None,
    }));
    state.apply(Event::ToolCallUpdated(ToolActivity {
        id: "edit".into(),
        title: None,
        status: Some("Completed".into()),
        kind: None,
        paths: Vec::new(),
        detail: Some(ToolDetail {
            input: None,
            content: vec![ToolOutput::Diff {
                path: "/w/patched.rs".into(),
                old_text: Some("old".into()),
                new_text: "new".into(),
            }],
            output: None,
        }),
    }));

    assert_eq!(
        state
            .changed_paths
            .keys()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>(),
        ["/w/patched.rs"]
    );
}

#[test]
fn diff_line_stats_accumulate_without_double_counting_streamed_updates() {
    let mut state = AgentState::default();
    let diff_update = |id: &str, old_text: &str, new_text: &str| {
        Event::ToolCallUpdated(ToolActivity {
            id: id.into(),
            title: None,
            status: Some("InProgress".into()),
            kind: Some("Edit".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: None,
                content: vec![ToolOutput::Diff {
                    path: "/w/streamed.rs".into(),
                    old_text: Some(old_text.into()),
                    new_text: new_text.into(),
                }],
                output: None,
            }),
        })
    };
    // The same tool streams a growing diff: its contribution is replaced.
    state.apply(diff_update(
        "edit-1",
        "kept\nremoved\n",
        "kept\nadded one\n",
    ));
    state.apply(diff_update(
        "edit-1",
        "kept\nremoved\n",
        "kept\nadded one\nadded two\n",
    ));
    // A second tool touching the same file adds to the total.
    state.apply(diff_update("edit-2", "kept\n", "kept\nlater\n"));

    assert_eq!(
        state
            .changed_paths
            .get(std::path::Path::new("/w/streamed.rs")),
        Some(&FileChange {
            added: 3,
            removed: 1
        })
    );
}

#[test]
fn diff_line_stats_and_their_tool_survive_large_transcripts() {
    let mut state = AgentState::default();
    state.apply(Event::ToolCallUpdated(ToolActivity {
        id: "early-edit".into(),
        title: None,
        status: Some("Completed".into()),
        kind: Some("Edit".into()),
        paths: Vec::new(),
        detail: Some(ToolDetail {
            input: None,
            content: vec![ToolOutput::Diff {
                path: "/w/early.rs".into(),
                old_text: Some("a\n".into()),
                new_text: "a\nb\nc\n".into(),
            }],
            output: None,
        }),
    }));
    for id in 0..40 {
        state.apply(Event::ToolCallUpdated(ToolActivity {
            id: format!("big-{id}"),
            title: None,
            status: Some("Completed".into()),
            kind: None,
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: None,
                content: vec![ToolOutput::Text("x".repeat(60_000))],
                output: None,
            }),
        }));
    }

    assert!(
        state
            .transcript
            .iter()
            .any(|item| matches!(item, TranscriptItem::Tool(tool) if tool.id == "early-edit"))
    );
    assert_eq!(
        state.changed_paths.get(std::path::Path::new("/w/early.rs")),
        Some(&FileChange {
            added: 2,
            removed: 0
        })
    );
}

fn diff_event(id: &str, path: &str, old_text: Option<&str>, new_text: &str) -> Event {
    Event::ToolCallUpdated(ToolActivity {
        id: id.into(),
        title: None,
        status: Some("Completed".into()),
        kind: Some("Edit".into()),
        paths: Vec::new(),
        detail: Some(ToolDetail {
            input: None,
            content: vec![ToolOutput::Diff {
                path: path.into(),
                old_text: old_text.map(Into::into),
                new_text: new_text.into(),
            }],
            output: None,
        }),
    })
}

#[test]
fn the_first_diff_per_file_records_the_session_baseline() {
    let mut state = AgentState::default();

    state.apply(diff_event(
        "edit-1",
        "/w/lib.rs",
        Some("original\n"),
        "first pass\n",
    ));
    state.apply(diff_event(
        "edit-2",
        "/w/lib.rs",
        Some("first pass\n"),
        "second pass\n",
    ));
    state.apply(diff_event("create", "/w/new.rs", None, "created\n"));

    assert_eq!(
        state.baselines.get(std::path::Path::new("/w/lib.rs")),
        Some(&Some("original\n".to_owned())),
        "the earliest pre-edit content wins"
    );
    assert_eq!(
        state.baselines.get(std::path::Path::new("/w/new.rs")),
        Some(&None),
        "a created file records that there was no file before"
    );
}

#[test]
fn oversized_baselines_are_skipped_so_memory_stays_bounded() {
    let mut state = AgentState::default();
    let huge = "x".repeat(1024 * 1024 + 1);

    state.apply(diff_event("edit", "/w/huge.rs", Some(&huge), "tiny\n"));

    assert!(state.baselines.is_empty());
    assert!(
        state
            .changed_paths
            .contains_key(std::path::Path::new("/w/huge.rs")),
        "line stats are still tracked"
    );
}

#[test]
fn a_new_session_clears_recorded_baselines() {
    let mut state = AgentState::default();
    state.apply(diff_event("edit", "/w/lib.rs", Some("original\n"), "new\n"));

    state.apply(Event::SessionReady {
        current_mode: None,
        modes: Vec::new(),
        config_options: Vec::new(),
    });

    assert!(state.baselines.is_empty());
}

#[test]
fn a_failed_session_load_restores_the_previous_baselines() {
    let mut state = AgentState::default();
    state.apply(diff_event("edit", "/w/lib.rs", Some("original\n"), "new\n"));

    state.apply(Event::SessionLoading { title: None });
    state.apply(Event::SessionLoadFailed);

    assert_eq!(
        state.baselines.get(std::path::Path::new("/w/lib.rs")),
        Some(&Some("original\n".to_owned()))
    );
}
