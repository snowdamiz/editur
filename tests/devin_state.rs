use editur::devin::{
    CredentialSource, DevinEvent, DevinMessage, DevinState, SessionDetail, SessionSummary,
};

fn session(id: &str, title: &str) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        title: title.into(),
        status: "running".into(),
        ..SessionSummary::default()
    }
}

fn message(id: &str, timestamp: &str, text: &str) -> DevinMessage {
    DevinMessage {
        id: id.into(),
        timestamp: timestamp.into(),
        role: "user".into(),
        text: text.into(),
        attachment_ids: Vec::new(),
    }
}

#[test]
fn search_results_replace_the_session_list() {
    let mut state = DevinState::default();
    state.apply(DevinEvent::SessionsLoaded {
        sessions: vec![session("old", "Old task")],
        next_cursor: Some("page-2".into()),
        append: false,
    });
    state.apply(DevinEvent::SessionsLoaded {
        sessions: vec![session("new", "New task")],
        next_cursor: None,
        append: false,
    });

    assert_eq!(state.sessions, vec![session("new", "New task")]);
}

#[test]
fn selected_session_pages_ignore_stale_generations_and_deduplicate_messages() {
    let mut state = DevinState::default();
    let stale = state.select("first".into());
    let current = state.select("second".into());
    state.apply(DevinEvent::SessionLoaded {
        session_id: "first".into(),
        generation: stale,
        detail: SessionDetail {
            summary: session("first", "Stale"),
            ..SessionDetail::default()
        },
    });
    state.apply(DevinEvent::MessagesLoaded {
        session_id: "second".into(),
        generation: current,
        messages: vec![
            message("two", "2026-08-15T12:02:00Z", "second"),
            message("one", "2026-08-15T12:01:00Z", "first"),
            message("one", "2026-08-15T12:01:00Z", "duplicate"),
        ],
        next_cursor: Some("next".into()),
        replace: false,
    });

    assert_eq!(
        state
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "two"]
    );
}

#[test]
fn numeric_upstream_timestamps_sort_chronologically() {
    let mut state = DevinState::default();
    let generation = state.select("session".into());
    state.apply(DevinEvent::MessagesLoaded {
        session_id: "session".into(),
        generation,
        messages: vec![
            message("later", "10", "later"),
            message("earlier", "2", "earlier"),
        ],
        next_cursor: None,
        replace: true,
    });

    assert_eq!(state.messages[0].id, "earlier");
}

#[test]
fn created_sessions_are_selectable_and_disconnect_clears_remote_state() {
    let mut state = DevinState {
        busy: true,
        ..DevinState::default()
    };
    state.apply(DevinEvent::SessionCreated(session(
        "created",
        "Created task",
    )));
    state.apply(DevinEvent::CredentialsChanged(Some(
        CredentialSource::Keyring,
    )));

    assert_eq!(state.sessions[0].id, "created");
    assert!(!state.busy);

    state.apply(DevinEvent::CredentialsChanged(None));

    assert!(state.sessions.is_empty());
    assert!(state.selected_session.is_none());
}
