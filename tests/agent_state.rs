use editur::agent::{
    controller::{ConnectionState, Event, PermissionChoice, PermissionRequest, ToolActivity},
    state::{AgentState, TranscriptItem},
};

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
    assert!(
        state
            .transcript
            .iter()
            .any(|item| matches!(item, TranscriptItem::Assistant(text) if text == "one two"))
    );
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
            detail: Some("x".repeat(64 * 1024)),
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
