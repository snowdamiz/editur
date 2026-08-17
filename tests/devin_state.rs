use editur::devin::{
    Attachment, Blueprint, ConnectionState, CredentialSource, DevinError, DevinEvent, DevinMessage,
    DevinResourceKind, DevinSection, DevinState, KnowledgeNote, LoadState, PageUpdate,
    SessionDetail, SessionFilters, SessionSummary, StatusCategory,
};

fn session(id: &str, title: &str) -> SessionSummary {
    SessionSummary {
        id: id.into(),
        title: title.into(),
        status: "running".into(),
        ..SessionSummary::default()
    }
}

#[test]
fn one_resource_failure_does_not_disable_sessions_or_other_resources() {
    let mut state = DevinState {
        connection: ConnectionState::Connected,
        sessions: vec![session("one", "Still usable")],
        ..DevinState::default()
    };

    state.apply(DevinEvent::ResourceFailed {
        section: DevinSection::Knowledge,
        error: DevinError {
            message: "Knowledge is temporarily unavailable".into(),
            retry_after_seconds: None,
        },
    });

    assert_eq!(state.connection, ConnectionState::Connected);
    assert_eq!(state.sessions.len(), 1);
    assert_eq!(state.resources.knowledge.status, LoadState::Failed);
    assert_eq!(
        state.resources.knowledge.error.as_deref(),
        Some("Knowledge is temporarily unavailable")
    );
}

#[test]
fn a_forbidden_secret_list_does_not_hide_available_blueprints() {
    let mut state = DevinState::default();
    state.apply(DevinEvent::BlueprintsLoaded(vec![Blueprint {
        id: "blueprint-1".into(),
        name: "Organization".into(),
        ..Blueprint::default()
    }]));
    state.apply(DevinEvent::ResourceKindForbidden(
        DevinResourceKind::Secrets,
    ));

    assert_eq!(state.resources.blueprints.status, LoadState::Loaded);
    assert_eq!(state.resources.blueprints.items.len(), 1);
    assert_eq!(state.resources.secrets.status, LoadState::Forbidden);
}

#[test]
fn a_loaded_resource_detail_updates_only_its_list_row() {
    let mut state = DevinState::default();
    state.apply(DevinEvent::KnowledgeLoaded(vec![
        KnowledgeNote {
            id: "one".into(),
            name: "One".into(),
            ..KnowledgeNote::default()
        },
        KnowledgeNote {
            id: "two".into(),
            name: "Two".into(),
            ..KnowledgeNote::default()
        },
    ]));

    state.apply(DevinEvent::KnowledgeDetailLoaded(KnowledgeNote {
        id: "one".into(),
        name: "One".into(),
        content: "Full content".into(),
        ..KnowledgeNote::default()
    }));

    assert_eq!(state.resources.knowledge.items.len(), 2);
    assert_eq!(state.resources.knowledge.items[0].content, "Full content");
    assert_eq!(state.resources.knowledge.items[1].id, "two");
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
        total: Some(2),
        has_next: true,
        append: false,
    });
    state.apply(DevinEvent::SessionsLoaded {
        sessions: vec![session("new", "New task")],
        next_cursor: None,
        total: Some(1),
        has_next: false,
        append: false,
    });

    assert_eq!(state.sessions, vec![session("new", "New task")]);
    assert_eq!(state.sessions_total, Some(1));
    assert!(!state.sessions_has_next);
}

#[test]
fn selected_detail_reconciles_the_matching_session_list_row() {
    let mut state = DevinState {
        sessions: vec![session("one", "Old title")],
        ..DevinState::default()
    };
    let generation = state.select("one".into());
    let detail_summary = SessionSummary {
        id: "one".into(),
        title: "Current title".into(),
        status: "suspended".into(),
        status_detail: Some("user_request".into()),
        category: StatusCategory::Completed,
        archived: true,
        ..SessionSummary::default()
    };

    state.apply(DevinEvent::SessionLoaded {
        session_id: "one".into(),
        generation,
        detail: SessionDetail {
            summary: detail_summary.clone(),
            ..SessionDetail::default()
        },
    });

    assert_eq!(state.sessions, [detail_summary]);
}

#[test]
fn organization_switch_clears_only_organization_owned_remote_state() {
    let mut state = DevinState {
        sessions: vec![session("old", "Old task")],
        sessions_cursor: Some("next".into()),
        sessions_total: Some(3),
        sessions_has_next: true,
        filters: SessionFilters {
            origin: "slack".into(),
            ..SessionFilters::default()
        },
        ..DevinState::default()
    };

    state.apply(DevinEvent::OrganizationSwitching);

    assert!(state.sessions.is_empty());
    assert_eq!(state.sessions_cursor, None);
    assert_eq!(state.sessions_total, None);
    assert!(!state.sessions_has_next);
    assert!(state.filters.is_empty());
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
        update: PageUpdate::History,
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
        update: PageUpdate::Initial,
    });

    assert_eq!(state.messages[0].id, "earlier");
}

#[test]
fn background_refresh_never_consumes_or_clears_the_history_cursor() {
    let mut state = DevinState::default();
    let generation = state.select("one".into());
    let page =
        |id: &str, timestamp: &str, cursor: Option<&str>, update| DevinEvent::MessagesLoaded {
            session_id: "one".into(),
            generation,
            messages: vec![message(id, timestamp, id)],
            next_cursor: cursor.map(str::to_owned),
            update,
        };

    state.apply(page("newest", "3", Some("older-1"), PageUpdate::Initial));
    state.apply(page("older", "2", Some("older-2"), PageUpdate::History));
    state.apply(page("oldest", "1", None, PageUpdate::History));
    for _ in 0..3 {
        state.apply(page("newest", "3", None, PageUpdate::Refresh));
    }
    state.apply(page("arrived", "4", None, PageUpdate::Refresh));

    assert_eq!(state.messages_cursor, None);
    assert_eq!(
        state
            .messages
            .iter()
            .map(|message| message.id.as_str())
            .collect::<Vec<_>>(),
        ["oldest", "older", "newest", "arrived"]
    );

    let other_generation = state.select("two".into());
    assert_ne!(generation, other_generation);
    state.apply(page("stale", "5", Some("stale"), PageUpdate::History));
    assert!(state.messages.is_empty());
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

#[test]
fn a_created_batch_is_kept_for_the_explicit_gather_action() {
    let mut state = DevinState::default();
    state.apply(DevinEvent::BatchCreated(vec!["one".into(), "two".into()]));

    assert_eq!(state.last_created_batch, ["one", "two"]);
}

#[test]
fn signed_attachment_urls_are_redacted_from_debug_output() {
    let attachment = Attachment {
        id: "one".into(),
        name: "mock.png".into(),
        url: Some("https://storage.example/mock?secret-signature".into()),
        ..Attachment::default()
    };

    assert!(!format!("{attachment:?}").contains("secret-signature"));
}

#[test]
fn prompt_and_message_content_are_redacted_from_debug_output() {
    let private = "private-prompt-content";
    let summary = SessionSummary {
        prompt: Some(private.into()),
        structured_output: Some(serde_json::json!({"secret": private})),
        ..SessionSummary::default()
    };
    let message = message("one", "1", private);

    assert!(!format!("{summary:?}").contains(private));
    assert!(!format!("{message:?}").contains(private));
}
