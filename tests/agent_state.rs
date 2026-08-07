use editur::agent::{
    controller::{
        ConnectionState, Event, PermissionChoice, PermissionRequest, PlanItem, ToolActivity,
        ToolDetail, ToolOutput,
    },
    state::{AgentState, TranscriptItem},
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
fn state_enforces_one_turn_orders_streams_and_answers_permission_once() {
    let mut state = AgentState {
        prompt: "ship it".into(),
        ..AgentState::default()
    };
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
fn streamed_unicode_is_trimmed_only_at_character_boundaries() {
    let mut state = AgentState::default();
    state.apply(Event::AssistantDelta("a".repeat(64 * 1024 - 1)));
    state.apply(Event::AssistantDelta("é".into()));

    assert!(matches!(
        state.transcript.back(),
        Some(TranscriptItem::Assistant(text)) if text.len() <= 64 * 1024 && text.is_char_boundary(text.len())
    ));
}

#[test]
fn raw_tool_details_are_evicted_before_visible_messages() {
    let mut state = AgentState::default();
    state.apply(Event::AssistantDelta("keep me".into()));
    for id in 0..17 {
        state.apply(Event::ToolCallUpdated(ToolActivity {
            id: id.to_string(),
            title: Some("tool".into()),
            status: Some("Completed".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: None,
                content: vec![ToolOutput::Text("x".repeat(64 * 1024))],
                output: None,
            }),
        }));
    }

    assert!(
        state
            .transcript
            .iter()
            .any(|item| matches!(item, TranscriptItem::Assistant(text) if text == "keep me"))
    );
    assert!(
        state
            .transcript
            .iter()
            .any(|item| { matches!(item, TranscriptItem::Tool(tool) if tool.detail.is_none()) })
    );
}

#[test]
fn split_tool_updates_preserve_input_and_structured_output() {
    let mut state = AgentState::default();
    state.apply(Event::ToolCallUpdated(ToolActivity {
        id: "tool".into(),
        title: Some("Edit".into()),
        status: Some("InProgress".into()),
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
fn zero_detail_tools_and_changed_paths_are_bounded() {
    let mut state = AgentState::default();
    for id in 0..5_000 {
        state.apply(Event::ToolCallUpdated(ToolActivity {
            id: id.to_string(),
            title: None,
            status: None,
            paths: vec![format!("/tmp/{id}").into()],
            detail: None,
        }));
    }

    assert!(state.transcript.len() <= 2_049);
    assert!(state.changed_paths.len() <= 4_096);
    assert!(matches!(
        state.transcript.front(),
        Some(TranscriptItem::Truncated)
    ));
}
