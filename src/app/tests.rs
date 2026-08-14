use crate::pane::allowed_tab_drop_zone;

#[cfg(unix)]
use super::picker_breadcrumb_segments;
use super::{
    AGENT_COMPOSER_HEIGHT, AGENT_MENU_ROW_HEIGHT, AgentFilePicker, AgenticDiff, CompletionPopup,
    DropZone, EditorApp, FileChange, LspDiagnosticsState, PANE_TAB_HEIGHT, PaneId, PaneLayout,
    PendingAction, RESIZE_SETTLE_DELAY, SIDEBAR_SETTINGS_ROW_HEIGHT, SettingsSection, TAB_CLOSE,
    TAB_DRAG_GHOST_PAINT_KEY, TAB_MAX_WIDTH, TAB_MIN_WIDTH, TITLEBAR_HEIGHT, TITLEBAR_PAINT_KEY,
    TabDrop, TreeState, UPDATE_BUTTON_SIZE, WINDOW_CORNER_RADIUS, agent_at_bottom,
    agent_collapsing_header, agent_composer_content, agent_composer_height,
    agent_dense_disclosure_row, agent_dense_tool, agent_diff_cache_count, agent_diff_preview,
    agent_empty_state_rect, agent_markdown_galley, agent_mention_matches, agent_mention_query,
    agent_menu_rect, agent_new_session_rect, agent_search_matches, agent_selector_button,
    agent_send_button_colors, agent_toggle_rect, build_agent_diff, cached_agent_diff, child_path,
    collect_agent_mentions, completion_word_range, copy_tree_entry, defer_resize,
    diagnostic_highlighted_job, disable_transient_egui_debug_overlays, draw_agent_changed_files,
    draw_agent_diff, draw_editor_empty_state, draw_provider_selector_identity,
    draw_sidebar_toggle_icon, draw_tab_drag_ghost, editor_background, editor_column_content,
    file_result_job, find_highlighted_job, install_repaint_wake, launch_in_current_process,
    load_agent_image_preview_bytes, match_bracket_pair, match_spans, model_display_name,
    next_find_match, pane_header_and_content, plain_text_job, presentation_job, project_chooser_ui,
    provider_selector_visible, repaint_deadline, repaint_delay_after_texture_update,
    resize_divider_stroke, run_everything_state, search_group_header, search_needs_polling,
    search_selection_after_navigation, settings_ui_scale_slider, should_show_project_chooser,
    skip_transition_render, slash_command_query, split_agent_sidebar, split_agentic_diff,
    split_agentic_workspace, split_bottom_panel, split_pane_content, split_workspace,
    stable_tab_drop_zone, tab_width, unique_copy_path,
};

#[test]
fn composer_mentions_only_use_the_active_at_token() {
    assert_eq!(
        agent_mention_query("Review @src/agent/sta"),
        Some("src/agent/sta")
    );
    assert_eq!(agent_mention_query("Attach @"), Some(""));
    assert_eq!(agent_mention_query("mail dev@example.com"), None);
    assert_eq!(agent_mention_query("Review @src then explain"), None);
    assert_eq!(agent_mention_query("Review @src "), None);
}

#[test]
fn composer_mentions_find_files_and_folders_but_skip_build_outputs() {
    let project = tempfile::tempdir().unwrap();
    fs::create_dir_all(project.path().join("src/agent")).unwrap();
    fs::create_dir_all(project.path().join("target/debug")).unwrap();
    fs::write(project.path().join("src/agent/state.rs"), "state").unwrap();
    fs::write(project.path().join("target/debug/state.rs"), "build").unwrap();

    let entries = collect_agent_mentions(project.path());
    let state = agent_mention_matches(&entries, "state.rs");
    let agent = agent_mention_matches(&entries, "agent");

    assert_eq!(state.len(), 1);
    assert_eq!(state[0].relative, "src/agent/state.rs");
    assert!(
        agent
            .iter()
            .any(|entry| entry.relative == "src/agent" && entry.is_dir)
    );
}

#[test]
fn composer_mention_menu_uses_compact_rows_without_bottom_slack() {
    let entry = super::AgentMentionEntry {
        path: "src".into(),
        relative: "src".into(),
        is_dir: true,
    };
    let mut row_height = 0.0;
    let _ = theme::test_context().run_ui(RawInput::default(), |ui| {
        row_height = super::agent_mention_row(ui, &entry, false).rect.height();
    });
    let transcript = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(400.0, 400.0));
    let anchor = Rect::from_min_size(pos2(0.0, 390.0), Vec2::new(100.0, 10.0));
    let popup = agent_menu_rect(transcript, anchor, 5, row_height, 4.0);

    assert_eq!(row_height, super::AGENT_MENTION_ROW_HEIGHT);
    assert_eq!(popup.height(), 8.0 + 5.0 * row_height);
}

#[test]
fn agent_search_covers_output_paths_and_diffs_but_not_thoughts() {
    use crate::agent::controller::{ToolActivity, ToolDetail, ToolOutput};
    use crate::agent::state::TranscriptItem;
    use std::collections::VecDeque;

    let transcript = VecDeque::from([
        TranscriptItem::Thought("private renderer guess".into()),
        TranscriptItem::Assistant("Updated the renderer.".into()),
        TranscriptItem::Tool(ToolActivity {
            id: "edit".into(),
            title: Some("Edit src/agent/state.rs".into()),
            status: Some("Completed".into()),
            kind: Some("Edit".into()),
            paths: vec!["src/agent/state.rs".into()],
            detail: Some(ToolDetail {
                input: None,
                content: vec![ToolOutput::Diff {
                    path: "src/agent/state.rs".into(),
                    old_text: Some("old extension".into()),
                    new_text: "new extension".into(),
                }],
                output: None,
            }),
        }),
    ]);

    let changed_paths = std::collections::HashMap::new();
    assert_eq!(
        agent_search_matches(&transcript, &changed_paths, "renderer"),
        vec![1]
    );
    assert_eq!(
        agent_search_matches(&transcript, &changed_paths, "state.rs"),
        vec![2, 2]
    );
    assert_eq!(
        agent_search_matches(&transcript, &changed_paths, "extension"),
        vec![2, 2]
    );
    assert!(agent_search_matches(&transcript, &changed_paths, "private").is_empty());
}

#[test]
fn agent_search_returns_every_occurrence_in_transcript_order() {
    use crate::agent::state::TranscriptItem;
    use std::collections::VecDeque;

    let transcript = VecDeque::from([
        TranscriptItem::User("page".into()),
        TranscriptItem::Assistant("page after page".into()),
    ]);

    assert_eq!(
        agent_search_matches(&transcript, &std::collections::HashMap::new(), "page"),
        vec![0, 1, 1]
    );
}

#[test]
fn completion_replaces_the_word_around_the_cursor() {
    assert_eq!(completion_word_range("let pri_value = 1", 7), 4..13);
    assert_eq!(completion_word_range("café", 4), 0..4);
    assert_eq!(completion_word_range("value.", 6), 6..6);
}

#[test]
fn duplicate_paths_use_the_first_available_copy_name() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("main.rs");
    fs::write(&source, "fn main() {}\n").unwrap();
    fs::write(temp.path().join("main copy.rs"), "occupied\n").unwrap();

    assert_eq!(
        unique_copy_path(&source, temp.path()).unwrap(),
        temp.path().join("main copy 2.rs")
    );
}

#[test]
fn copied_folders_keep_nested_files() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    let destination = temp.path().join("destination");
    fs::create_dir_all(source.join("nested")).unwrap();
    fs::write(source.join("nested/file.txt"), "copied").unwrap();

    copy_tree_entry(&source, &destination).unwrap();

    assert_eq!(
        fs::read_to_string(destination.join("nested/file.txt")).unwrap(),
        "copied"
    );
}

#[test]
fn tree_names_cannot_escape_the_selected_folder() {
    let temp = tempfile::tempdir().unwrap();

    assert!(child_path(temp.path(), "../outside").is_err());
}

#[test]
fn foreground_menu_mesh_is_not_cached_as_retained_content() {
    let menu_color = theme::color::sentinel();
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    crate::theme::apply(&context);
    let menu_rect = Rect::from_min_size(pos2(100.0, 100.0), egui::vec2(220.0, 24.0));
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1_000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| {
            app.ui(root);
            context
                .layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    Id::new("test_context_menu"),
                ))
                .rect_filled(menu_rect, 0.0, menu_color);
        },
    );
    let mut retained = None;
    let mut menu_mesh = false;
    for primitive in context.tessellate(output.shapes, output.pixels_per_point) {
        match &primitive.primitive {
            egui::epaint::Primitive::Callback(_) => {
                retained = crate::renderer::retained_paint(&primitive.primitive).unwrap();
            }
            egui::epaint::Primitive::Mesh(mesh) => {
                if mesh
                    .vertices
                    .iter()
                    .any(|vertex| vertex.color == menu_color)
                {
                    menu_mesh = true;
                    assert_eq!(retained, None, "foreground menu inherited retained paint");
                }
                retained = None;
            }
        }
    }
    assert!(menu_mesh);
}
use crate::{
    agent::controller::{
        AuthChoice, AuthKind, CommandChoice, ConfigChoice, ConfigValue, ConfigValueChoice,
        ConnectionState, ModeChoice, PermissionChoice, SessionChoice, ToolActivity, ToolDetail,
    },
    agent::provider::ProviderId,
    agent::state::{PermissionCard, TranscriptItem},
    buffer::Buffer,
    file_io::OpenTarget,
    lsp::{CompletionItem, DefinitionLocation, Diagnostic, DiagnosticSeverity, RequestTag},
    settings::Settings,
    syntax::{Highlighter, SyntaxManager},
    theme,
};

#[test]
fn completion_enter_is_consumed_and_the_edit_is_one_undo_step() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("current.txt");
    fs::write(&file, "pri").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file.clone()),
        create: false,
    })
    .unwrap();
    app.tabs[0].editor_surface.set_selection(3, 3);
    app.lsp_completion = Some(CompletionPopup {
        tag: RequestTag {
            path: file,
            revision: 0,
            cursor: 3,
        },
        items: vec![CompletionItem {
            label: "print".into(),
            kind: Some(3),
            detail: None,
            insert_text: "print".into(),
            edit: None,
        }],
        selected: 0,
        anchor: Rect::NOTHING,
        bounds: Rect::EVERYTHING,
    });
    let context = theme::test_context();
    let _ = context.run_ui(
        RawInput {
            events: vec![Event::Key {
                key: Key::Enter,
                physical_key: Some(Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..RawInput::default()
        },
        |_root| assert!(app.handle_lsp_popup_keys(&context)),
    );

    assert_eq!(app.tabs[0].buffer.text, "print");
    assert!(app.tabs[0].buffer.dirty);
    let tab = &mut app.tabs[0];
    assert!(tab.editor_surface.undo(&mut tab.buffer.text));
    assert_eq!(tab.buffer.text, "pri");
}

#[test]
fn diagnostics_preserve_existing_presentation_backgrounds() {
    let base = plain_text_job("alpha", 200.0);
    let find = find_highlighted_job(&base, std::slice::from_ref(&(0..5)), 0);
    let shown = diagnostic_highlighted_job(
        &find,
        &[Diagnostic {
            range: 1..3,
            line: 0,
            severity: DiagnosticSeverity::Error,
            source: None,
            code: None,
            message: "error".into(),
        }],
        false,
    );

    assert!(shown.sections.iter().any(|section| {
        section.format.background != Color32::TRANSPARENT
            && section.format.underline != egui::Stroke::NONE
    }));
    let underlined = shown
        .sections
        .iter()
        .filter(|section| section.format.underline != egui::Stroke::NONE)
        .map(|section| section.byte_range.start.0..section.byte_range.end.0)
        .collect::<Vec<_>>();
    assert_eq!(underlined.len(), 1);
    assert_eq!(underlined[0], 1..3);
}

#[test]
fn one_definition_opens_the_file_and_clamps_its_utf16_position() {
    let temp = tempfile::tempdir().unwrap();
    let current = temp.path().join("current.txt");
    let target = temp.path().join("target.rs");
    fs::write(&current, "current").unwrap();
    fs::write(&target, "a💡b\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(current),
        create: false,
    })
    .unwrap();

    app.navigate_to_definition(DefinitionLocation {
        path: target.clone(),
        line: 0,
        character: 3,
        end_line: 0,
        end_character: 4,
    });

    let tab = &app.tabs[app.active_tab.unwrap()];
    assert_eq!(tab.buffer.path, target);
    assert_eq!(tab.editor_surface.cursor(), 2);
}

#[test]
fn settings_search_filters_unmatched_sections_and_server_rows() {
    fn contains_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, expected)),
            _ => false,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("README.md");
    fs::write(&file, "keep me").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file),
        create: false,
    })
    .unwrap();
    app.settings = Settings::default();
    app.settings_error = None;
    app.settings_open = true;
    app.settings_search = "rust".into();
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1_200.0, 800.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(
        output
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "Rust"))
    );
    assert!(!output.shapes.iter().any(|shape| {
        contains_text(&shape.shape, "Enable language servers")
            || contains_text(&shape.shape, "TypeScript / JavaScript")
    }));
}

#[test]
fn settings_navigation_has_no_button_backgrounds() {
    fn collect(shape: &Shape, labels: &mut Vec<Rect>, fills: &mut Vec<(Rect, Color32)>) {
        match shape {
            Shape::Text(text)
                if text.pos.x < 270.0
                    && matches!(
                        text.galley.text(),
                        "←  Back to app"
                            | "Back to app"
                            | "Appearance"
                            | "Keybindings"
                            | "Language Servers"
                    ) =>
            {
                labels.push(text.visual_bounding_rect());
            }
            Shape::Rect(rect)
                if [
                    theme::surface().input,
                    theme::state::selected(),
                    theme::state::hover(),
                ]
                .contains(&rect.fill) =>
            {
                fills.push((rect.rect, rect.fill));
            }
            Shape::Vec(shapes) => {
                shapes
                    .iter()
                    .for_each(|shape| collect(shape, labels, fills));
            }
            _ => {}
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.settings_open = true;
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1_200.0, 800.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let (mut labels, mut fills) = (Vec::new(), Vec::new());
    output
        .shapes
        .iter()
        .for_each(|shape| collect(&shape.shape, &mut labels, &mut fills));

    assert!(
        labels.len() >= 2,
        "the rail has to name Back and at least one section"
    );
    assert!(
        labels
            .iter()
            .all(|label| fills.iter().all(|(fill, _)| !fill.contains_rect(*label))),
        "navigation rows stay text, never chip-backed"
    );
}

#[test]
fn settings_navigation_tabs_use_compact_vertical_rhythm() {
    fn text_center(shape: &Shape, expected: &str) -> Option<f32> {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => {
                Some(text.visual_bounding_rect().center().y)
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_center(shape, expected)),
            _ => None,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.settings_open = true;
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1_200.0, 800.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let centers = ["Appearance", "Keybindings", "Language Servers"].map(|label| {
        output
            .shapes
            .iter()
            .find_map(|shape| text_center(&shape.shape, label))
            .expect(label)
    });

    assert!(
        centers.windows(2).all(|pair| pair[1] - pair[0] <= 40.0),
        "settings tabs are too far apart: {centers:?}"
    );
}

#[test]
fn settings_server_labels_align_with_their_mode_control() {
    fn text_rect(shape: &Shape, expected: &str) -> Option<Rect> {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => {
                Some(text.visual_bounding_rect())
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_rect(shape, expected)),
            _ => None,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.settings_open = true;
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1_200.0, 800.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let find = |expected| {
        output
            .shapes
            .iter()
            .find_map(|shape| text_rect(&shape.shape, expected))
            .expect(expected)
    };
    let language = find("Rust");
    let server = find("rust-analyzer");
    let mode = find("Auto");
    let label_center = (language.top() + server.bottom()) * 0.5;

    assert!(
        (label_center - mode.center().y).abs() < 1.0,
        "label center {label_center}, mode center {}",
        mode.center().y
    );
}

#[test]
fn appearance_rows_keep_label_and_choices_on_one_line_in_declared_order() {
    fn text_rect(shape: &Shape, expected: &str) -> Option<Rect> {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => {
                Some(text.visual_bounding_rect())
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_rect(shape, expected)),
            _ => None,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.settings = Settings::default();
    app.settings_open = true;
    app.settings_section = SettingsSection::Appearance;
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1_200.0, 800.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let find = |expected| {
        output
            .shapes
            .iter()
            .find_map(|shape| text_rect(&shape.shape, expected))
            .expect(expected)
    };
    let title = find("Theme");
    let detail = find("Dark, light, or follow the system.");
    let dark = find("Dark");
    let light = find("Light");
    let system = find("System");
    let line_wrapping = find("Line wrapping");
    let no_wrap = find("No wrap");
    let wrap = find("Wrap");
    let label_center = (title.top() + detail.bottom()) * 0.5;

    assert!((label_center - dark.center().y).abs() < 3.0);
    assert!(dark.right() < light.left());
    assert!(light.right() < system.left());
    assert!(title.right() < dark.left());
    assert!(line_wrapping.right() < no_wrap.left());
    assert!(no_wrap.right() < wrap.left());
}

#[test]
fn appearance_settings_offer_the_dense_agent_toggle() {
    fn contains_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, expected)),
            _ => false,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.settings = Settings::default();
    app.settings_open = true;
    app.settings_section = SettingsSection::Appearance;

    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1_200.0, 800.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(
        output
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "Dense Agent"))
    );
}

#[test]
fn dragging_a_tab_down_previews_a_horizontal_split() {
    let pane = Rect::from_min_size(pos2(100.0, 100.0), Vec2::new(600.0, 400.0));

    assert_eq!(
        allowed_tab_drop_zone(pane, pos2(400.0, 480.0)),
        DropZone::Bottom
    );
}

#[test]
fn split_preview_stays_stable_near_a_drop_zone_boundary() {
    let pane = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 600.0));

    assert_eq!(
        stable_tab_drop_zone(pane, pos2(215.0, 300.0), Some(DropZone::Left)),
        DropZone::Left
    );
    assert_eq!(
        stable_tab_drop_zone(pane, pos2(280.0, 300.0), Some(DropZone::Left)),
        DropZone::Center
    );
}

#[test]
fn moving_the_drag_ghost_invalidates_its_retained_geometry() {
    let context = theme::test_context();
    let draw = |pointer| {
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 600.0))),
                events: vec![Event::PointerMoved(pointer)],
                ..RawInput::default()
            },
            |_| draw_tab_drag_ghost(&context, "moving.rs"),
        );
        context
            .tessellate(output.shapes, output.pixels_per_point)
            .into_iter()
            .find_map(|primitive| {
                crate::renderer::retained_paint(&primitive.primitive)
                    .ok()
                    .flatten()
                    .filter(|paint| paint.key == TAB_DRAG_GHOST_PAINT_KEY)
            })
            .expect("retained drag ghost")
            .revision
    };

    let before = draw(pos2(100.0, 100.0));
    let moved = draw(pos2(300.0, 240.0));

    assert_ne!(before, moved);
}

#[test]
fn bottom_split_divides_the_target_pane_horizontally() {
    let mut layout = PaneLayout::default();
    let lower = layout.split(PaneId(0), DropZone::Bottom).unwrap();
    let available = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 600.0));

    assert_eq!(
        layout.rects(available),
        [
            (PaneId(0), available.with_max_y(300.0)),
            (lower, available.with_min_y(300.0)),
        ]
    );
}

#[test]
fn dragging_a_vertical_pane_border_resizes_both_panes() {
    let mut layout = PaneLayout::default();
    let right = layout.split(PaneId(0), DropZone::Right).unwrap();
    let available = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 600.0));
    let handle = layout.split_handles(available)[0];

    assert!(layout.resize(handle.id, handle.bounds, pos2(600.0, 300.0)));
    assert_eq!(
        layout.rects(available),
        [
            (PaneId(0), available.with_max_x(600.0)),
            (right, available.with_min_x(600.0)),
        ]
    );
}

#[test]
fn adjacent_resize_keeps_non_neighboring_panes_the_same_width() {
    let mut layout = PaneLayout::default();
    let right = layout.split(PaneId(0), DropZone::Right).unwrap();
    let middle = layout.split(right, DropZone::Left).unwrap();
    let available = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1200.0, 600.0));
    let root_handle = layout.split_handles(available)[0];

    assert!(layout.resize_adjacent(root_handle.id, available, pos2(700.0, 300.0)));

    let rects = layout.rects(available);
    assert_eq!(
        rects[1],
        (
            middle,
            Rect::from_min_max(pos2(700.0, 0.0), pos2(900.0, 600.0))
        )
    );
    assert_eq!(rects[2].1.width(), 300.0);
}

#[test]
fn inserting_at_a_divider_places_an_equal_pane_between_two_existing_panes() {
    let mut layout = PaneLayout::default();
    let right = layout.split(PaneId(0), DropZone::Right).unwrap();
    let available = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1200.0, 600.0));
    let divider = layout.split_handles(available)[0];

    let middle = layout.insert_at_split(divider.id).unwrap();

    let rects = layout.rects(available);
    assert_eq!(
        rects.iter().map(|(pane, _)| *pane).collect::<Vec<_>>(),
        [PaneId(0), middle, right]
    );
    assert!(
        rects
            .iter()
            .all(|(_, rect)| (rect.width() - 400.0).abs() < 0.01)
    );
}

#[test]
fn dragging_a_pane_border_in_the_app_updates_the_split() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["first.rs", "second.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "text\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(paths[1].clone(), false);
    app.drop_tab(1, PaneId(0), DropZone::Right);
    let context = theme::test_context();
    let draw = |app: &mut EditorApp, events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let _ = draw(&mut app, Vec::new());
    let divider = context
        .read_response(Id::new(("pane_split_divider", 0)))
        .expect("pane divider")
        .rect
        .center();
    let _ = draw(
        &mut app,
        vec![
            Event::PointerMoved(divider),
            Event::PointerButton {
                pos: divider,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    let moved = divider + Vec2::new(100.0, 0.0);
    let _ = draw(&mut app, vec![Event::PointerMoved(moved)]);
    let _ = draw(
        &mut app,
        vec![Event::PointerButton {
            pos: moved,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );

    let available = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 600.0));
    assert!(app.pane_layout.rects(available)[0].1.width() > 400.0);
}

#[test]
fn top_row_panes_use_the_titlebar_without_losing_editor_height() {
    let titlebar = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, TITLEBAR_HEIGHT));
    let editor = Rect::from_min_size(pos2(240.0, TITLEBAR_HEIGHT), Vec2::new(760.0, 666.0));
    let pane = editor.with_max_y(367.0);

    let (header, content) = pane_header_and_content(titlebar, editor, pane);

    assert_eq!(
        (header, content.top()),
        (titlebar.with_min_x(240.0), pane.top())
    );
}

#[test]
fn terminal_panel_uses_the_bottom_of_the_non_sidebar_workspace() {
    let window = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let (sidebar, editor, agent) = split_workspace(window, true, 240.0, true, 320.0);
    let workspace = Rect::from_min_max(editor.left_top(), window.right_bottom());

    let (main, terminal) = split_bottom_panel(workspace, true, 240.0);
    let terminal = terminal.unwrap();
    let editor = editor.with_max_y(main.bottom());
    let agent = agent.with_max_y(main.bottom());

    assert_eq!(main, workspace.with_max_y(460.0));
    assert_eq!(terminal, workspace.with_min_y(460.0));
    assert_eq!(sidebar.unwrap().bottom(), window.bottom());
    assert_eq!(terminal.left(), editor.left());
    assert_eq!(terminal.right(), agent.right());
    assert_eq!(terminal.top(), agent.bottom());
    assert_eq!(
        split_bottom_panel(workspace, false, 240.0),
        (workspace, None)
    );
}

#[test]
fn terminal_panel_stays_beside_the_agentic_session_rail() {
    let window = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let (sessions, agent) = split_agentic_workspace(window, true, 248.0);

    let (_, terminal) = split_bottom_panel(agent, true, 240.0);

    assert_eq!(terminal.unwrap().left(), sessions.unwrap().right());
    assert_eq!(terminal.unwrap().right(), window.right());
}

#[test]
fn lower_row_panes_keep_their_header_inside_the_pane() {
    let titlebar = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, TITLEBAR_HEIGHT));
    let editor = Rect::from_min_size(pos2(240.0, TITLEBAR_HEIGHT), Vec2::new(760.0, 666.0));
    let pane = editor.with_min_y(367.0);

    let (header, content) = pane_header_and_content(titlebar, editor, pane);

    assert_eq!(
        (header, content.top()),
        (
            pane.with_max_y(pane.top() + PANE_TAB_HEIGHT),
            pane.top() + PANE_TAB_HEIGHT,
        )
    );
}

#[test]
fn narrow_panes_merge_tabs_instead_of_splitting_below_the_usable_width() {
    let pane = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(399.0, 600.0));

    assert_eq!(
        allowed_tab_drop_zone(pane, pos2(1.0, 300.0)),
        DropZone::Center
    );
}

#[test]
fn short_panes_merge_tabs_instead_of_splitting_below_the_usable_height() {
    let pane = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(600.0, 399.0));

    assert_eq!(
        allowed_tab_drop_zone(pane, pos2(300.0, 398.0)),
        DropZone::Center
    );
}

#[test]
fn portrait_panes_fall_back_to_a_row_split_near_a_blocked_side_edge() {
    let pane = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(399.0, 1000.0));

    assert_eq!(
        allowed_tab_drop_zone(pane, pos2(0.1, 999.0)),
        DropZone::Bottom
    );
}

#[test]
fn ultrawide_panes_fall_back_to_a_column_split_near_a_blocked_bottom_edge() {
    let pane = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 399.0));

    assert_eq!(
        allowed_tab_drop_zone(pane, pos2(1.0, 398.9)),
        DropZone::Left
    );
}

#[test]
fn portrait_layouts_can_grow_to_eight_rows_when_space_allows() {
    let available = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(900.0, 1600.0));
    let mut layout = PaneLayout::default();
    let mut rows = vec![PaneId(0)];
    for _ in 0..3 {
        let mut next = Vec::with_capacity(rows.len() * 2);
        for pane in rows {
            let rect = layout
                .rects(available)
                .into_iter()
                .find_map(|(id, rect)| (id == pane).then_some(rect))
                .unwrap();
            let zone = allowed_tab_drop_zone(rect, rect.center_bottom() - Vec2::Y);
            let added = layout.split(pane, zone).unwrap();
            next.extend([pane, added]);
        }
        rows = next;
    }

    assert_eq!(
        layout
            .rects(available)
            .into_iter()
            .map(|(_, rect)| rect.height())
            .collect::<Vec<_>>(),
        vec![200.0; 8]
    );
}

#[test]
fn ultrawide_layouts_can_grow_to_eight_columns_when_space_allows() {
    let available = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1600.0, 900.0));
    let mut layout = PaneLayout::default();
    let mut columns = vec![PaneId(0)];
    for _ in 0..3 {
        let mut next = Vec::with_capacity(columns.len() * 2);
        for pane in columns {
            let rect = layout
                .rects(available)
                .into_iter()
                .find_map(|(id, rect)| (id == pane).then_some(rect))
                .unwrap();
            let zone = allowed_tab_drop_zone(rect, rect.right_center() - Vec2::X);
            let added = layout.split(pane, zone).unwrap();
            next.extend([pane, added]);
        }
        columns = next;
    }

    assert_eq!(
        layout
            .rects(available)
            .into_iter()
            .map(|(_, rect)| rect.width())
            .collect::<Vec<_>>(),
        vec![200.0; 8]
    );
}

#[test]
fn dropping_a_tab_at_the_bottom_moves_it_into_a_new_pane() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["first.rs", "second.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "text\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(paths[1].clone(), false);

    app.drop_tab(1, PaneId(0), DropZone::Bottom);

    assert_eq!(
        (
            app.tabs[0].pane,
            app.tabs[1].pane,
            app.pane_layout.rects(Rect::EVERYTHING).len(),
        ),
        (PaneId(0), PaneId(1), 2)
    );
}

#[test]
fn dropping_a_file_tree_path_opens_it_in_a_new_pane() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = root.join("first.rs");
    let second = root.join("second.rs");
    fs::write(&first, "first\n").unwrap();
    fs::write(&second, "second\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(first),
        create: false,
    })
    .unwrap();

    app.drop_path(second.clone(), PaneId(0), DropZone::Right);

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tabs[1].buffer.path, second);
    assert_eq!(app.tabs[1].pane, PaneId(1));
}

#[test]
fn drag_preview_restructures_without_committing_the_layout() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["first.rs", "second.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "text\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(paths[1].clone(), false);
    let available = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 600.0));

    let (preview, dragged) = app
        .tab_drag_preview(
            available,
            &paths[1],
            TabDrop {
                target: PaneId(0),
                zone: DropZone::Bottom,
                preview: Rect::NOTHING,
            },
        )
        .unwrap();

    assert_eq!(preview.len(), 2);
    assert_eq!(preview[1].0, dragged);
    assert_eq!(app.pane_layout.rects(available).len(), 1);
}

#[test]
fn split_preview_does_not_render_duplicate_tab_controls() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["first.rs", "second.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "text\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(paths[1].clone(), false);
    let context = theme::test_context();
    disable_transient_egui_debug_overlays(&context);
    let draw = |app: &mut EditorApp, events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let _ = draw(&mut app, Vec::new());
    let tab = context
        .read_response(Id::new(("file_tab", paths[1].display().to_string())))
        .unwrap()
        .rect
        .center();
    let editor = context.read_response(Id::new("editor")).unwrap().rect;
    let target = pos2(editor.left() + 4.0, editor.center().y);
    let _ = draw(
        &mut app,
        vec![
            Event::PointerMoved(tab),
            Event::PointerButton {
                pos: tab,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    let _ = draw(&mut app, vec![Event::PointerMoved(target)]);
    let output = draw(&mut app, Vec::new());

    assert!(!output.shapes.iter().any(|shape| has_id_clash(&shape.shape)));
}

#[test]
fn dragged_pane_preview_renders_the_file_contents() {
    fn contains_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(expected),
            Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, expected)),
            _ => false,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("preview.rs");
    fs::write(&path, "dragged_preview_contents_42\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(path.clone()),
        create: false,
    })
    .unwrap();
    let context = theme::test_context();

    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 600.0))),
            ..RawInput::default()
        },
        |root| {
            let editor = root.max_rect();
            app.draw_editor_pane(root, PaneId(99), editor, false, Some(&path), true);
        },
    );

    assert!(
        output
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "dragged_preview_contents_42"))
    );
}

#[test]
fn moving_the_last_tab_out_of_a_pane_collapses_the_empty_split() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["first.rs", "second.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "text\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(paths[1].clone(), false);
    app.drop_tab(1, PaneId(0), DropZone::Bottom);

    app.drop_tab(1, PaneId(0), DropZone::Center);

    assert_eq!(app.pane_layout.rects(Rect::EVERYTHING).len(), 1);
}

#[test]
fn every_markdown_pane_has_its_own_preview_toggle() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["first.md", "second.md"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "# Preview\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(paths[1].clone(), false);
    app.drop_tab(1, PaneId(0), DropZone::Bottom);
    let context = theme::test_context();

    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!([PaneId(0), PaneId(1)].into_iter().all(|pane| {
        context
            .read_response(Id::new(("markdown_preview_toggle", pane.0)))
            .is_some()
    }));
}

#[test]
fn provider_selector_requires_two_available_providers() {
    assert!(!provider_selector_visible(&[]));
    assert!(!provider_selector_visible(&[ProviderId::Cursor]));
    assert!(provider_selector_visible(&[
        ProviderId::Cursor,
        ProviderId::Codex,
    ]));
}

#[test]
fn provider_selector_uses_a_drawn_chevron() {
    let mut selector = Rect::NOTHING;
    let output = theme::test_context().run_ui(RawInput::default(), |ui| {
        selector = draw_provider_selector_identity(ui, ProviderId::Cursor, true).rect;
    });
    let glyph = output.shapes.iter().any(|shape| match &shape.shape {
        Shape::Text(text) => text.galley.text() == "⌄",
        _ => false,
    });
    let label = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.text() == "Cursor" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .expect("provider label");
    let trailing = selector.intersect(Rect::everything_right_of(label.right()));
    let chevron = crate::icons::probe::bounds(&output.shapes, trailing, theme::text().primary)
        .expect("drawn chevron");
    let chevron_left = chevron.left();
    let chevron_right = chevron.right();

    assert!(!glyph);
    assert!(chevron.width() > 4.0);
    assert!(chevron_left - label.right() >= 10.0);
    assert!(chevron_right - chevron_left >= 8.0);
}

#[test]
fn provider_selector_stays_open_in_ide_and_agentic_layouts() {
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(expected),
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }

    for agentic_mode in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root: temp.path().canonicalize().unwrap(),
            file: None,
            create: false,
        })
        .unwrap();
        app.agentic_mode = agentic_mode;
        app.agent_sidebar = !agentic_mode;
        app.available_providers = vec![ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude];
        app.agent.connection = ConnectionState::Ready;
        app.agent.session_ready = true;
        let context = theme::test_context();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
        let draw = |app: &mut EditorApp, events| {
            context.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..RawInput::default()
                },
                |root| app.ui(root),
            )
        };

        let _ = draw(&mut app, Vec::new());
        let anchor = app.provider_menu_anchor.expect("provider selector");
        let selector = anchor.center();
        let _ = draw(
            &mut app,
            vec![
                Event::PointerMoved(selector),
                Event::PointerButton {
                    pos: selector,
                    button: PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::NONE,
                },
            ],
        );
        let _ = draw(
            &mut app,
            vec![Event::PointerButton {
                pos: selector,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
        );
        let output = draw(&mut app, Vec::new());

        assert_eq!(
            app.agent_menu,
            Some(super::AgentMenu::Providers),
            "provider menu closed in agentic_mode={agentic_mode}"
        );
        let popup = app.agent_menu_popup.expect("provider menu");
        assert_eq!(popup.left(), anchor.left());
        assert_eq!(popup.top(), anchor.bottom() + 6.0);
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| has_text(&shape.shape, "Claude")),
            "Claude missing in agentic_mode={agentic_mode}"
        );
    }
}

#[test]
fn provider_selector_and_provider_specific_controls_follow_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    assert!(app.available_providers.is_empty());
    assert!(app.agent_controllers.is_empty());
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    let context = theme::test_context();
    let draw = |app: &mut EditorApp| {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    };

    app.available_providers = vec![ProviderId::Cursor];
    draw(&mut app);
    assert!(app.provider_menu_anchor.is_none());

    app.available_providers = vec![ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude];
    draw(&mut app);
    assert!(app.provider_menu_anchor.is_some());

    app.agent_menu = Some(super::AgentMenu::Providers);
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let text_shapes = output.shapes.iter().filter_map(|shape| match &shape.shape {
        Shape::Text(text) => Some(text.galley.text()),
        _ => None,
    });
    let text = text_shapes.collect::<Vec<_>>();
    assert!(text.contains(&"Cursor"));
    assert!(text.contains(&"Codex"));
    assert!(text.contains(&"Claude"));
    for removed in [
        "Cursor's ACP coding agent",
        "OpenAI Codex through the canonical ACP adapter",
        "Anthropic Claude through the canonical ACP adapter",
        "Installs on first use",
        "◎",
        "✦",
    ] {
        assert!(
            !text.contains(&removed),
            "provider menu still paints {removed}"
        );
    }
    let popup = app.agent_menu_popup.unwrap();
    assert!(popup.top() >= app.provider_menu_anchor.unwrap().bottom());
    assert!(
        popup.height() <= 150.0,
        "provider menu is not compact: {popup:?}"
    );

    app.agent_menu = Some(super::AgentMenu::Permissions);
    draw(&mut app);
    assert!(app.agent_menu_popup.is_none());

    app.agent.allow_run_everything = true;
    app.agent_menu = Some(super::AgentMenu::Permissions);
    draw(&mut app);
    assert!(app.agent_menu_popup.is_some());
}

#[test]
fn selected_provider_drives_agent_identity_and_failure_copy() {
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }

    fn has_provider_logo(shape: &Shape) -> bool {
        match shape {
            Shape::Mesh(mesh) => mesh.texture_id != egui::TextureId::default(),
            Shape::Vec(shapes) => shapes.iter().any(has_provider_logo),
            _ => false,
        }
    }

    for (provider, name) in [(ProviderId::Codex, "Codex"), (ProviderId::Claude, "Claude")] {
        let temp = tempfile::tempdir().unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root: temp.path().canonicalize().unwrap(),
            file: None,
            create: false,
        })
        .unwrap();
        app.agent_sidebar = true;
        app.available_providers = vec![ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude];
        app.selected_provider = provider;
        app.agent.connection = ConnectionState::Ready;
        app.agent.session_ready = true;
        app.agent
            .transcript
            .push_back(TranscriptItem::Assistant("Provider response".into()));
        let context = theme::test_context();
        let input = || RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        };
        let output = context.run_ui(input(), |root| app.ui(root));
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| has_text(&shape.shape, name))
        );
        assert!(!output.shapes.iter().any(|shape| {
            has_text(
                &shape.shape,
                if provider == ProviderId::Claude {
                    "Codex"
                } else {
                    "Claude"
                },
            )
        }));
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| has_provider_logo(&shape.shape)),
            "{name} response is missing its provider logo"
        );

        app.agent.connection = ConnectionState::Failed("adapter stopped".into());
        app.agent.diagnostics = Some(format!("{name} diagnostics were suppressed."));
        let output = context.run_ui(input(), |root| app.ui(root));
        for expected in [
            format!("{name} Agent unavailable"),
            "adapter stopped".into(),
            format!("{name} diagnostics were suppressed."),
            "Retry".into(),
        ] {
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|shape| has_text(&shape.shape, &expected)),
                "missing failure-state text: {expected}"
            );
        }
        assert!(app.provider_menu_anchor.is_some());
        assert_eq!(app.selected_provider, provider);
    }
}

#[test]
fn connecting_states_show_identity_and_a_progress_bar() {
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }

    fn thin_bar(shape: &Shape, fill: Color32, found: &mut Option<Rect>) {
        match shape {
            Shape::Rect(rect) if rect.fill == fill && rect.rect.height() <= 4.0 => {
                *found = Some(rect.rect);
            }
            Shape::Vec(shapes) => {
                for shape in shapes {
                    thin_bar(shape, fill, found);
                }
            }
            _ => {}
        }
    }

    fn find_bar(output: &egui::FullOutput, fill: Color32) -> Option<Rect> {
        let mut found = None;
        for shape in &output.shapes {
            thin_bar(&shape.shape, fill, &mut found);
        }
        found
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    let context = theme::test_context();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1000.0, 700.0),
        )),
        time: Some(0.5),
        ..RawInput::default()
    };

    app.agent.connection = ConnectionState::Starting;
    let output = context.run_ui(input(), |root| app.ui(root));
    for expected in ["Starting Cursor Agent", "Connecting to this project…"] {
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| has_text(&shape.shape, expected)),
            "missing connecting text: {expected}"
        );
    }
    assert!(
        find_bar(&output, theme::accent()).is_some(),
        "connecting state should animate a progress sweep"
    );

    app.agent.connection = ConnectionState::Provisioning {
        downloaded: 12 * 1_048_576,
        total: Some(48 * 1_048_576),
    };
    let output = context.run_ui(input(), |root| app.ui(root));
    for expected in ["Installing Cursor Agent", "Downloading 12 of 48 MiB"] {
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| has_text(&shape.shape, expected)),
            "missing provisioning text: {expected}"
        );
    }
    let track = find_bar(&output, theme::border::hairline_color())
        .expect("download should draw a progress track");
    let bar = find_bar(&output, theme::accent())
        .expect("download should draw a determinate progress fill");
    let fraction = bar.width() / track.width();
    assert!(
        (fraction - 0.25).abs() < 0.05,
        "a quarter of the download should fill about a quarter of the bar, got {fraction}"
    );
}

#[test]
fn claude_missing_credentials_show_setup_and_retry_without_subscription_login() {
    fn collect_text(shape: &Shape, text: &mut Vec<String>) {
        match shape {
            Shape::Text(value) => text.push(value.galley.text().to_owned()),
            Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_text(shape, text);
                }
            }
            _ => {}
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.available_providers = vec![ProviderId::Cursor, ProviderId::Codex, ProviderId::Claude];
    app.selected_provider = ProviderId::Claude;
    app.agent.connection = ConnectionState::AuthenticationRequired(vec![AuthChoice {
        id: "anthropic-environment".into(),
        name: "Anthropic API or commercial cloud credentials".into(),
        description: Some(
            "Configure credentials outside Editur, then retry the Claude provider.".into(),
        ),
        kind: AuthKind::Environment,
        setup: Some(
            "variables: ANTHROPIC_API_KEY or supported commercial cloud credentials".into(),
        ),
        can_authenticate: false,
    }]);
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let mut text = Vec::new();
    for shape in &output.shapes {
        collect_text(&shape.shape, &mut text);
    }
    let joined = text.join("\n");

    for expected in [
        "Connect Claude",
        "Configure API or supported commercial cloud credentials outside Editur, then retry this provider.",
        "Anthropic API or commercial cloud credentials",
        "variables: ANTHROPIC_API_KEY or supported commercial cloud credentials",
        "Retry",
    ] {
        assert!(
            text.iter().any(|text| text == expected),
            "missing {expected}"
        );
    }
    assert!(!joined.contains("subscription"));
    assert!(!joined.contains("secret"));
    assert!(!joined.contains("Authenticate"));
}

#[test]
fn provider_switch_preserves_each_providers_session_and_the_composer_draft() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.selected_provider = ProviderId::Cursor;
    app.agent.prompt = "keep this draft".into();
    app.agent.session_id = Some("cursor-session".into());
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("cursor output".into()));

    app.select_provider_state(ProviderId::Codex);
    assert_eq!(app.selected_provider, ProviderId::Codex);
    assert_eq!(app.agent.prompt, "keep this draft");
    assert!(app.agent.session_id.is_none());
    assert!(app.agent.transcript.is_empty());

    app.agent.prompt = "new draft".into();
    app.agent.session_id = Some("codex-session".into());
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("codex output".into()));
    app.select_provider_state(ProviderId::Cursor);

    assert_eq!(app.agent.prompt, "new draft");
    assert_eq!(app.agent.session_id.as_deref(), Some("cursor-session"));
    assert!(matches!(
        app.agent.transcript.back(),
        Some(TranscriptItem::Assistant(text)) if text == "cursor output"
    ));

    app.select_provider_state(ProviderId::Codex);
    assert_eq!(app.agent.session_id.as_deref(), Some("codex-session"));
    assert!(matches!(
        app.agent.transcript.back(),
        Some(TranscriptItem::Assistant(text)) if text == "codex output"
    ));
}

#[test]
fn an_unbundled_provider_cannot_be_selected() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.available_providers = vec![ProviderId::Cursor, ProviderId::Codex];
    let context = theme::test_context();

    app.request_provider_switch(ProviderId::Claude, &context);

    assert_eq!(app.selected_provider, ProviderId::Cursor);
}
use egui::{
    Color32, CursorIcon, DroppedFile, Event, HoveredFile, Id, Key, Modifiers, MouseWheelUnit,
    PointerButton, Pos2, RawInput, Rect, TouchPhase, Vec2, epaint::Shape, pos2,
};
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::{
    fs,
    time::{Duration, Instant},
};

#[test]
fn ui_scale_slider_commits_only_after_release() {
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(400.0, 80.0));
    let mut value = 100;
    let pointer = {
        let mut draw = |events| {
            let mut slider = Rect::NOTHING;
            let _ = context.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..RawInput::default()
                },
                |ui| {
                    let (response, _) = settings_ui_scale_slider(ui, &mut value);
                    slider = response.rect;
                },
            );
            slider
        };
        let slider = draw(Vec::new());
        let pointer = pos2(slider.left() + 180.0, slider.center().y);
        let _ = draw(vec![
            Event::PointerMoved(pointer),
            Event::PointerButton {
                pos: pointer,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        pointer
    };
    let held = value;
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            events: vec![Event::PointerButton {
                pos: pointer,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: Modifiers::NONE,
            }],
            ..RawInput::default()
        },
        |ui| {
            settings_ui_scale_slider(ui, &mut value);
        },
    );

    assert_eq!((held, value), (100, 200));
}

#[test]
fn ui_scale_change_in_agentic_mode_rebuilds_hidden_editor_text() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("cached-tree-entry.rs");
    fs::write(&file, "cached editor line\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file),
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1_000.0, 700.0),
        )),
        ..RawInput::default()
    };
    let draw = |app: &mut EditorApp| context.run_ui(input(), |root| app.ui(root));
    fn galley(output: &egui::FullOutput, expected: &str) -> std::sync::Arc<egui::Galley> {
        fn find(shape: &Shape, expected: &str) -> Option<std::sync::Arc<egui::Galley>> {
            match shape {
                Shape::Text(text) if text.galley.text() == expected => {
                    Some(std::sync::Arc::clone(&text.galley))
                }
                Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, expected)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, expected))
            .unwrap_or_else(|| panic!("missing rendered text {expected:?}"))
    }

    let before = draw(&mut app);
    let before_tree = galley(&before, "cached-tree-entry.rs");
    let before_editor = galley(&before, "cached editor line");

    app.agentic_mode = true;
    app.settings.appearance.ui_scale_percent = 150;
    let _ = draw(&mut app); // UI zoom becomes active at the start of the next pass.
    let scaled_agent = draw(&mut app);
    assert_eq!(scaled_agent.pixels_per_point, 1.5);

    app.agentic_mode = false;
    let returned = draw(&mut app);
    let returned_tree = galley(&returned, "cached-tree-entry.rs");
    let returned_editor = galley(&returned, "cached editor line");

    assert!(!std::sync::Arc::ptr_eq(&before_tree, &returned_tree));
    assert!(!std::sync::Arc::ptr_eq(&before_editor, &returned_editor));
}

#[test]
fn ui_scale_change_rebuilds_hidden_markdown_preview() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("preview.md");
    fs::write(&file, "cached preview paragraph\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file),
        create: false,
    })
    .unwrap();
    app.tabs[0].markdown_preview = true;
    let context = theme::test_context();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1_000.0, 700.0),
        )),
        ..RawInput::default()
    };
    let draw = |app: &mut EditorApp| context.run_ui(input(), |root| app.ui(root));
    let preview = |output: &egui::FullOutput| {
        fn find(shape: &Shape) -> Option<std::sync::Arc<egui::Galley>> {
            match shape {
                Shape::Text(text) if text.galley.text() == "cached preview paragraph\n" => {
                    Some(std::sync::Arc::clone(&text.galley))
                }
                Shape::Vec(shapes) => shapes.iter().find_map(find),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape))
            .expect("rendered Markdown preview")
    };

    let before = preview(&draw(&mut app));
    app.agentic_mode = true;
    app.settings.appearance.ui_scale_percent = 150;
    let _ = draw(&mut app);
    let _ = draw(&mut app);
    app.agentic_mode = false;
    let returned = preview(&draw(&mut app));

    assert!(!std::sync::Arc::ptr_eq(&before, &returned));
}

fn has_id_clash(shape: &Shape) -> bool {
    match shape {
        Shape::Text(text) => text.galley.text().contains("use of widget ID"),
        Shape::Rect(rect) => rect.stroke.color == Color32::RED,
        Shape::Vec(shapes) => shapes.iter().any(has_id_clash),
        _ => false,
    }
}

#[test]
fn graphical_launch_runs_in_the_current_process() {
    assert!(launch_in_current_process(false, false, false, false));
}

#[test]
fn dock_launch_without_a_path_shows_the_project_chooser() {
    assert!(should_show_project_chooser(false, true));
}

#[test]
fn cli_or_explicit_path_launch_skips_the_project_chooser() {
    assert_eq!(
        (
            should_show_project_chooser(false, false),
            should_show_project_chooser(true, true),
        ),
        (false, false)
    );
}

#[test]
fn active_sidebar_toggle_icons_are_white_and_do_not_shift() {
    let button = Rect::from_center_size(pos2(50.0, 17.0), Vec2::new(34.0, 34.0));
    let draw = |open| {
        let context = theme::test_context();
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(100.0, 34.0))),
                ..RawInput::default()
            },
            |ui| {
                let response = ui.interact(
                    button,
                    Id::new(("sidebar_toggle_icon", open)),
                    egui::Sense::click(),
                );
                draw_sidebar_toggle_icon(ui, button, &response, open);
            },
        );
        output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Rect(rect) if rect.rect.size() == Vec2::new(16.0, 13.0) => {
                    Some((rect.rect.center(), rect.stroke.color))
                }
                _ => None,
            })
            .expect("sidebar toggle icon")
    };
    let inactive = draw(false);
    let active = draw(true);

    assert_eq!(active.0 - button.center(), inactive.0 - button.center());
    assert_eq!(active.1, theme::text().primary);
}

#[test]
fn agent_toggle_is_a_sparkle_rather_than_a_panel_glyph() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();

    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    let button = context
        .read_response(Id::new("agent_sidebar_toggle"))
        .expect("agent toggle")
        .rect;
    let sparkle = crate::icons::probe::bounds(&output.shapes, button, theme::text().muted)
        .expect("sparkle glyph inside the agent toggle");
    assert!(sparkle.width() <= crate::icons::GRID + 1.0);
    assert!(!output.shapes.iter().any(|shape| match &shape.shape {
        Shape::Rect(rect) => {
            button.contains_rect(rect.rect) && rect.rect.size() == Vec2::new(16.0, 13.0)
        }
        _ => false,
    }));
}

#[test]
fn settings_row_sits_at_the_bottom_of_the_file_tree_sidebar() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let screen = Rect::from_min_size(Default::default(), Vec2::new(1000.0, 700.0));

    let output = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    let sidebar = split_workspace(screen, true, app.sidebar_width, false, 0.0)
        .0
        .expect("file tree sidebar");
    let settings = context
        .read_response(Id::new("settings_toggle"))
        .expect("settings row")
        .rect;
    assert_eq!(settings.bottom(), sidebar.bottom());
    assert_eq!(settings.x_range(), sidebar.x_range());
    assert_eq!(settings.height(), SIDEBAR_SETTINGS_ROW_HEIGHT);
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "Settings"))
    );
    assert!(
        output.shapes.iter().any(|shape| match shape.shape {
            Shape::LineSegment { points, .. } =>
                (points[0].y - settings.top()).abs() <= 1.0
                    && (points[1].y - settings.top()).abs() <= 1.0
                    && points[0].x <= settings.left() + 1.0
                    && points[1].x >= settings.right() - 1.0,
            _ => false,
        }),
        "a hairline separates the settings row from the tree"
    );
}

#[test]
fn settings_row_sits_at_the_bottom_of_the_agentic_sessions_rail() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    let context = theme::test_context();
    let screen = Rect::from_min_size(Default::default(), Vec2::new(1000.0, 700.0));

    let output = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    let sessions = split_agentic_workspace(screen, true, app.sidebar_width)
        .0
        .expect("agentic sessions rail");
    let settings = context
        .read_response(Id::new("settings_toggle"))
        .expect("settings row")
        .rect;
    assert_eq!(settings.bottom(), sessions.bottom());
    assert_eq!(settings.x_range(), sessions.x_range());
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "Settings"))
    );
}

#[test]
fn update_button_appears_on_the_settings_row_only_when_an_update_is_detected() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let screen = Rect::from_min_size(Default::default(), Vec2::new(1000.0, 700.0));
    let input = || RawInput {
        screen_rect: Some(screen),
        ..RawInput::default()
    };

    let _ = context.run_ui(input(), |root| app.ui(root));
    assert!(
        context.read_response(Id::new("settings_update")).is_none(),
        "the button may only appear once an update is detected"
    );

    app.update_available
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let output = context.run_ui(input(), |root| app.ui(root));

    let row = context
        .read_response(Id::new("settings_toggle"))
        .expect("settings row")
        .rect;
    let button = context
        .read_response(Id::new("settings_update"))
        .expect("update button")
        .rect;
    assert_eq!(button.size(), Vec2::splat(UPDATE_BUTTON_SIZE));
    assert_eq!(button.right(), row.right() - theme::space::MEDIUM);
    assert_eq!(button.center().y, row.center().y);
    fn is_accent_circle(shape: &Shape, center: egui::Pos2) -> bool {
        match shape {
            Shape::Circle(circle) => {
                circle.center == center
                    && circle.radius == UPDATE_BUTTON_SIZE * 0.5
                    && circle.fill == theme::accent()
            }
            Shape::Vec(shapes) => shapes.iter().any(|shape| is_accent_circle(shape, center)),
            _ => false,
        }
    }
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| is_accent_circle(&shape.shape, button.center())),
        "the update button is a solid accent circle"
    );
}

#[test]
fn settings_toggle_hover_brightens_the_icon_without_a_background() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let screen = Rect::from_min_size(Default::default(), Vec2::new(1000.0, 700.0));
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let button = context
        .read_response(Id::new("settings_toggle"))
        .expect("settings toggle")
        .rect;
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            events: vec![Event::PointerMoved(button.center())],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(!output.shapes.iter().any(|shape| match &shape.shape {
        Shape::Rect(rect) => rect.rect == button && rect.fill == theme::state::hover(),
        _ => false,
    }));
    assert!(crate::icons::probe::bounds(&output.shapes, button, theme::text().primary).is_some());
}

#[test]
fn agent_opens_on_the_right_without_replacing_the_explorer() {
    let content = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let (explorer, editor, agent) = split_workspace(content, true, 240.0, true, 340.0);

    let explorer = explorer.expect("explorer remains visible");
    assert_eq!(explorer.left(), content.left());
    assert_eq!(agent.right(), content.right());
    assert_eq!(explorer.right(), editor.left());
    assert_eq!(editor.right(), agent.left());
}

#[test]
fn open_agent_toggle_belongs_to_the_agent_header() {
    let window = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let (_, _, sidebar) = split_workspace(window, true, 248.0, true, 360.0);
    let (header, _, _) = split_agent_sidebar(sidebar, AGENT_COMPOSER_HEIGHT);
    let toggle = agent_toggle_rect(header);
    let new_session = agent_new_session_rect(header);

    assert!(header.contains_rect(toggle));
    assert_eq!(toggle.height(), header.height());
    assert!(toggle.center().x > header.center().x);
    assert_eq!(toggle.center().x - new_session.center().x, 33.0);
    assert_eq!(toggle.center().y, new_session.center().y);
}

#[test]
fn agent_header_is_title_only_and_working_follows_the_latest_output() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.title = Some("Landing Page Builder".into());
    app.selected_provider = ProviderId::Cursor;
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("Latest output".into()));
    let context = theme::test_context();
    let draw = |app: &mut EditorApp| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    fn text(shape: &Shape, expected: &str) -> Option<(Rect, f32)> {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => Some((
                Rect::from_min_size(text.pos, text.galley.size()),
                text.galley.job.sections[0].format.font_id.size,
            )),
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text(shape, expected)),
            _ => None,
        }
    }
    let find = |output: &egui::FullOutput, expected| {
        output
            .shapes
            .iter()
            .find_map(|shape| text(&shape.shape, expected))
    };

    let idle = draw(&mut app);
    let title = find(&idle, "Landing Page Builder").unwrap();
    assert_eq!(title.1, 13.0);
    assert_eq!(title.0.center().y, TITLEBAR_HEIGHT * 0.5);
    assert!(find(&idle, "Ready").is_none());
    assert!(find(&idle, "Cursor is working").is_none());

    app.agent.active = true;
    let active = draw(&mut app);
    let latest = find(&active, "Latest output").unwrap().0;
    let working = find(&active, "Cursor is working").unwrap().0;
    assert!(working.top() > latest.bottom());
    assert!(working.bottom() < 592.0);
    assert!(context.read_response(Id::new("agent_working")).is_some());
}

#[test]
fn session_title_is_the_history_selector() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.history_available = true;
    app.agent.title = Some("Landing Page Builder".into());
    app.agent.sessions = Some(Vec::new());
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let draw = |app: &mut EditorApp, events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let output = draw(&mut app, Vec::new());
    let selector = context
        .read_response(Id::new("agent_session_selector"))
        .expect("session title selector")
        .rect;
    assert!(context.read_response(Id::new("agent_sessions")).is_none());
    let title = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.text() == "Landing Page Builder" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .expect("session title");
    assert!(selector.contains_rect(title));
    let trailing = selector.intersect(Rect::everything_right_of(title.right()));
    assert!(
        crate::icons::probe::bounds(&output.shapes, trailing, theme::text().primary).is_some(),
        "session title needs a drawn chevron"
    );

    let pointer = selector.center();
    let _ = draw(
        &mut app,
        vec![
            Event::PointerMoved(pointer),
            Event::PointerButton {
                pos: pointer,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    let _ = draw(
        &mut app,
        vec![Event::PointerButton {
            pos: pointer,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    assert_eq!(app.agent_menu, Some(super::AgentMenu::Sessions));
}

#[test]
fn dense_agent_keeps_responses_and_changed_files_visible() {
    fn contains_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(expected),
            Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, expected)),
            _ => false,
        }
    }

    for agentic_mode in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root: temp.path().canonicalize().unwrap(),
            file: None,
            create: false,
        })
        .unwrap();
        app.agentic_mode = agentic_mode;
        app.agent_sidebar = !agentic_mode;
        app.settings.appearance.dense_agent = true;
        app.agent.connection = ConnectionState::Ready;
        app.agent.session_ready = true;
        app.agent
            .transcript
            .push_back(TranscriptItem::User("Make it compact".into()));
        app.agent
            .transcript
            .push_back(TranscriptItem::Assistant("Inspecting the renderer".into()));
        app.agent
            .transcript
            .push_back(TranscriptItem::Tool(ToolActivity {
                id: "dense-read".into(),
                title: Some("Read src/app.rs".into()),
                status: Some("Completed".into()),
                kind: Some("Read".into()),
                paths: vec!["src/app.rs".into()],
                detail: None,
            }));
        app.agent
            .transcript
            .push_back(TranscriptItem::Assistant("Now the implementation".into()));
        app.agent.changed_paths.insert(
            "src/app.rs".into(),
            FileChange {
                added: 1,
                removed: 0,
            },
        );
        let context = theme::test_context();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1_200.0, 760.0));
        let mut draw = |events| {
            context.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..RawInput::default()
                },
                |root| app.ui(root),
            )
        };

        let output = draw(Vec::new());

        assert!(
            output
                .shapes
                .iter()
                .any(|shape| contains_text(&shape.shape, "Explored 1 file"))
        );
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| contains_text(&shape.shape, "Inspecting the renderer"))
        );
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| contains_text(&shape.shape, "Now the implementation"))
        );
        assert!(
            !output
                .shapes
                .iter()
                .any(|shape| contains_text(&shape.shape, "Read src/app.rs"))
        );
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| contains_text(&shape.shape, "1 file changed"))
        );
    }
}

#[test]
fn dense_agent_responses_use_primary_text() {
    fn text_color(shape: &Shape, expected: &str) -> Option<Color32> {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => {
                Some(text.galley.job.sections[0].format.color)
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_color(shape, expected)),
            _ => None,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.settings.appearance.dense_agent = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("Midway update".into()));

    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1_000.0, 760.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert_eq!(
        output
            .shapes
            .iter()
            .find_map(|shape| text_color(&shape.shape, "Midway update")),
        Some(theme::text().primary)
    );
}

#[test]
fn dense_agent_labels_only_the_final_response_in_each_turn() {
    fn text_rects(shape: &Shape, expected: &str, rects: &mut Vec<Rect>) {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => {
                rects.push(Rect::from_min_size(text.pos, text.galley.size()));
            }
            Shape::Vec(shapes) => shapes
                .iter()
                .for_each(|shape| text_rects(shape, expected, rects)),
            _ => {}
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.settings.appearance.dense_agent = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.selected_provider = ProviderId::Cursor;
    app.agent
        .transcript
        .push_back(TranscriptItem::User("First turn".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("Midway update".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::Thought("Working".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("First final".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::User("Second turn".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("Second final".into()));

    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1_000.0, 900.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let rects = |expected| {
        let mut rects = Vec::new();
        output
            .shapes
            .iter()
            .for_each(|shape| text_rects(&shape.shape, expected, &mut rects));
        rects
    };
    let identities = rects("Cursor");
    let midway = rects("Midway update")[0];
    let first_final = rects("First final")[0];
    let second_final = rects("Second final")[0];
    let has_identity_above = |response: Rect| {
        identities.iter().any(|identity| {
            identity.bottom() <= response.top()
                && response.top() - identity.bottom() <= theme::space::MEDIUM
        })
    };

    assert!(!has_identity_above(midway));
    assert!(
        has_identity_above(first_final),
        "identities={identities:?}, first_final={first_final:?}"
    );
    assert!(
        has_identity_above(second_final),
        "identities={identities:?}, second_final={second_final:?}"
    );
}

#[test]
fn dense_work_summary_combines_edits_exploration_commands_and_diff_totals() {
    use std::collections::{HashMap, VecDeque};

    let tool = |id: &str, title: &str, kind: &str| {
        TranscriptItem::Tool(ToolActivity {
            id: id.into(),
            title: Some(title.into()),
            status: Some("Completed".into()),
            kind: Some(kind.into()),
            paths: Vec::new(),
            detail: None,
        })
    };
    let transcript = VecDeque::from([
        tool("edit", "Edit", "Edit"),
        tool("read", "Read", "Read"),
        tool("search", "Grep", "Search"),
        tool("command", "Run tests", "Execute"),
    ]);
    let changes = HashMap::from([(
        "edit".into(),
        FileChange {
            added: 22,
            removed: 11,
        },
    )]);

    let clusters = super::dense_agent_work_clusters(&transcript, &changes, None);
    let summary = clusters[0].as_ref().unwrap();

    assert_eq!(
        summary.label,
        "Edited 1 file, explored 1 file, 1 search, ran 1 command"
    );
    assert_eq!(
        summary.change,
        Some(FileChange {
            added: 22,
            removed: 11
        })
    );
}

#[test]
fn dense_work_summary_is_one_bold_inline_hover_target() {
    fn text(shape: &Shape, expected: &str) -> Option<(Rect, egui::FontFamily, Color32)> {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => Some((
                Rect::from_min_size(text.pos, text.galley.size()),
                text.galley.job.sections[0].format.font_id.family.clone(),
                text.galley.job.sections[0].format.color,
            )),
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text(shape, expected)),
            _ => None,
        }
    }
    fn has_hover_fill(shape: &Shape, row: Rect) -> bool {
        match shape {
            Shape::Rect(rect) => rect.rect == row && rect.fill == theme::state::hover(),
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_hover_fill(shape, row)),
            _ => false,
        }
    }

    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 120.0));
    let draw = |events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |ui| {
                agent_dense_disclosure_row(
                    ui,
                    Id::new("dense_summary_style"),
                    "Edited app.rs",
                    Some(FileChange {
                        added: 44,
                        removed: 5,
                    }),
                    false,
                    false,
                );
            },
        )
    };

    let _ = draw(Vec::new());
    let row = context
        .read_response(Id::new("dense_summary_style"))
        .expect("dense summary row")
        .rect;
    let hovered = draw(vec![Event::PointerMoved(row.center())]);
    let label = hovered
        .shapes
        .iter()
        .find_map(|shape| text(&shape.shape, "Edited app.rs"))
        .expect("summary label");
    let added = hovered
        .shapes
        .iter()
        .find_map(|shape| text(&shape.shape, "+44"))
        .expect("added count");
    let removed = hovered
        .shapes
        .iter()
        .find_map(|shape| text(&shape.shape, "−5"))
        .expect("removed count");
    let chevron = crate::icons::probe::bounds(&hovered.shapes, row, theme::text().primary)
        .expect("hovered summary chevron");

    assert_eq!(label.1, theme::typography::strong_family());
    assert_eq!(label.2, theme::text().primary);
    assert!((added.0.left() - label.0.right()) <= theme::space::MEDIUM);
    assert!(
        (chevron.left() - removed.0.right()) <= theme::space::MEDIUM,
        "diff-to-chevron gap was {}",
        chevron.left() - removed.0.right()
    );
    assert!(added.0.left() >= label.0.right());
    assert!(chevron.left() >= removed.0.right());
    assert!(
        !hovered
            .shapes
            .iter()
            .any(|shape| has_hover_fill(&shape.shape, row))
    );
}

#[test]
fn dense_agent_expands_active_work_then_collapses_it_when_the_turn_finishes() {
    fn contains_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(expected),
            Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, expected)),
            _ => false,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.settings.appearance.dense_agent = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.active = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::User("Inspect this".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("Looking now".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::Tool(ToolActivity {
            id: "active-search".into(),
            title: Some("Grepped renderer in app.rs".into()),
            status: Some("Completed".into()),
            kind: Some("Search".into()),
            paths: Vec::new(),
            detail: None,
        }));
    let context = theme::test_context();
    let draw = |app: &mut EditorApp| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1_000.0, 760.0),
                )),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let active = draw(&mut app);
    assert!(
        active
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "Grepped renderer in app.rs"))
    );

    app.agent.active = false;
    let completed = draw(&mut app);
    assert!(
        !completed
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "Grepped renderer in app.rs"))
    );
    assert!(
        completed
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "Explored 1 search"))
    );
}

#[test]
fn dense_work_items_have_no_extra_gap_between_rows() {
    let tool = |id: &str, title: &str| {
        TranscriptItem::Tool(ToolActivity {
            id: id.into(),
            title: Some(title.into()),
            status: Some("Completed".into()),
            kind: Some("Read".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: None,
                content: vec![crate::agent::controller::ToolOutput::Text("details".into())],
                output: None,
            }),
        })
    };
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.settings.appearance.dense_agent = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.active = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::User("Inspect this".into()));
    app.agent
        .transcript
        .push_back(tool("dense-first", "Read first.rs"));
    app.agent
        .transcript
        .push_back(tool("dense-second", "Read second.rs"));
    let context = theme::test_context();
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1_000.0, 760.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let first = context
        .read_response(Id::new(("dense_agent_tool", 1)))
        .expect("first dense work row")
        .rect;
    let second = context
        .read_response(Id::new(("dense_agent_tool", 2)))
        .expect("second dense work row")
        .rect;

    assert!(second.top() - first.bottom() <= theme::space::TIGHT);
}

#[test]
fn dense_work_rows_are_short_and_use_text_only_hover() {
    fn text_color(shape: &Shape, expected: &str) -> Option<Color32> {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => {
                Some(text.galley.job.sections[0].format.color)
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_color(shape, expected)),
            _ => None,
        }
    }
    fn has_hover_fill(shape: &Shape, row: Rect) -> bool {
        match shape {
            Shape::Rect(rect) => rect.rect == row && rect.fill == theme::state::hover(),
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_hover_fill(shape, row)),
            _ => false,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.settings.appearance.dense_agent = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.active = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::User("Inspect this".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::Tool(ToolActivity {
            id: "dense-hover".into(),
            title: Some("Read compact.rs".into()),
            status: Some("Completed".into()),
            kind: Some("Read".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: None,
                content: vec![crate::agent::controller::ToolOutput::Text("details".into())],
                output: None,
            }),
        }));
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1_000.0, 760.0));
    let mut draw = |events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let _ = draw(Vec::new());
    let row = context
        .read_response(Id::new(("dense_agent_tool", 1)))
        .expect("dense work row")
        .rect;
    let hovered = draw(vec![Event::PointerMoved(row.center())]);

    assert!(row.height() <= 30.0, "row height was {}", row.height());
    assert_eq!(
        hovered
            .shapes
            .iter()
            .find_map(|shape| text_color(&shape.shape, "Read compact.rs")),
        Some(theme::text().primary)
    );
    assert!(
        !hovered
            .shapes
            .iter()
            .any(|shape| has_hover_fill(&shape.shape, row))
    );
}

#[test]
fn dense_tool_titles_stay_on_one_line_at_the_minimum_sidebar_width() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent_sidebar_width = 320.0;
    app.settings.appearance.dense_agent = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.active = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::User("Fix the shell startup".into()));
    let command = "const patch = '*** Begin Patch *** Update File: /Users/example/.zshrc @@ remove a very long stale shell startup entry *** End Patch'";
    app.agent
        .transcript
        .push_back(TranscriptItem::Tool(ToolActivity {
            id: "narrow-command".into(),
            title: Some(command.into()),
            status: Some("Completed".into()),
            kind: Some("Execute".into()),
            paths: Vec::new(),
            detail: None,
        }));

    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1_000.0, 760.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let rows = output.shapes.iter().find_map(|shape| match &shape.shape {
        Shape::Text(text) if text.galley.text() == command => Some(text.galley.rows.len()),
        _ => None,
    });

    assert_eq!(rows, Some(1));
}

#[test]
fn dense_agent_tool_rows_expand_to_their_compact_details() {
    fn contains_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(expected),
            Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, expected)),
            _ => false,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.settings.appearance.dense_agent = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::User("Tighten the renderer".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::Tool(ToolActivity {
            id: "dense-edit".into(),
            title: Some("Edit src/app.rs".into()),
            status: Some("Completed".into()),
            kind: Some("Edit".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: None,
                content: vec![crate::agent::controller::ToolOutput::Text(
                    "compact edit details".into(),
                )],
                output: None,
            }),
        }));
    app.agent.tool_changes.insert(
        "dense-edit".into(),
        FileChange {
            added: 44,
            removed: 5,
        },
    );
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1_000.0, 760.0));
    let mut draw = |events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let collapsed = draw(Vec::new());
    assert!(
        !collapsed
            .shapes
            .iter()
            .any(|shape| { contains_text(&shape.shape, "compact edit details") })
    );
    assert!(
        collapsed
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "Edited app.rs"))
    );
    assert!(
        collapsed
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "+44"))
    );
    assert!(
        collapsed
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "−5"))
    );
    let cluster = context
        .read_response(Id::new(("dense_agent_work", 1, false)))
        .expect("dense work cluster")
        .rect;
    let _ = draw(vec![
        Event::PointerMoved(cluster.center()),
        Event::PointerButton {
            pos: cluster.center(),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    let _ = draw(vec![Event::PointerButton {
        pos: cluster.center(),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    let row = context
        .read_response(Id::new(("dense_agent_tool", 1)))
        .expect("dense tool disclosure")
        .rect;
    let _ = draw(vec![
        Event::PointerMoved(row.center()),
        Event::PointerButton {
            pos: row.center(),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    let expanded = draw(vec![Event::PointerButton {
        pos: row.center(),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);

    assert!(
        expanded
            .shapes
            .iter()
            .any(|shape| { contains_text(&shape.shape, "compact edit details") })
    );
    assert!(
        expanded
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "+44"))
    );
    assert!(
        expanded
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "−5"))
    );
}

#[test]
fn dense_agent_removes_redundant_speaker_labels() {
    fn contains_exact_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes
                .iter()
                .any(|shape| contains_exact_text(shape, expected)),
            _ => false,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.settings.appearance.dense_agent = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::User("Compact this".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("On it".into()));

    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1_000.0, 760.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(
        !output
            .shapes
            .iter()
            .any(|shape| contains_exact_text(&shape.shape, "YOU"))
    );
}

#[test]
fn agentic_toggle_labels_share_sidebar_edge_padding() {
    fn label_rect(shape: &Shape, expected: &str) -> Option<Rect> {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| label_rect(shape, expected)),
            _ => None,
        }
    }
    let draw = |agentic_mode, expected| {
        let temp = tempfile::tempdir().unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root: temp.path().canonicalize().unwrap(),
            file: None,
            create: false,
        })
        .unwrap();
        app.agentic_mode = agentic_mode;
        let context = theme::test_context();
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
        let button = context
            .read_response(Id::new("agentic_mode_toggle"))
            .expect("agentic mode toggle")
            .rect;
        let label = output
            .shapes
            .iter()
            .find_map(|shape| label_rect(&shape.shape, expected))
            .expect("agentic mode toggle label");
        button.right() - label.right()
    };

    let agent_padding = draw(false, "Agent");
    let ide_padding = draw(true, "IDE");
    assert!((agent_padding - ide_padding).abs() < 0.1);
    assert!(agent_padding >= 12.0);
}

#[test]
fn agentic_toggle_hover_brightens_text_without_a_background() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let button = context
        .read_response(Id::new("agentic_mode_toggle"))
        .expect("agentic mode toggle")
        .rect;
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            events: vec![Event::PointerMoved(button.center())],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn inspect(shape: &Shape, button: Rect, bright: &mut bool, background: &mut bool) {
        match shape {
            Shape::Text(text) if text.galley.text() == "Agent" => {
                *bright = text.galley.job.sections[0].format.color == theme::text().primary;
            }
            Shape::Rect(rect)
                if rect.fill == theme::state::selected()
                    && button.contains(rect.rect.left_top())
                    && button.contains(rect.rect.right_bottom()) =>
            {
                *background = true;
            }
            Shape::Vec(shapes) => shapes
                .iter()
                .for_each(|shape| inspect(shape, button, bright, background)),
            _ => {}
        }
    }
    let (mut bright, mut background) = (false, false);
    output
        .shapes
        .iter()
        .for_each(|shape| inspect(&shape.shape, button, &mut bright, &mut background));

    assert!(bright);
    assert!(!background);
}

#[test]
fn disabled_send_button_is_neutral_instead_of_blue_with_a_gray_arrow() {
    let (ready_fill, ready_icon) = agent_send_button_colors(true);
    let (disabled_fill, disabled_icon) = agent_send_button_colors(false);

    assert_eq!(ready_fill, theme::accent());
    assert_eq!(ready_icon, theme::text().on_accent);
    assert_eq!(disabled_fill, theme::state::hover());
    assert_eq!(disabled_icon, theme::text_disabled());
}

#[test]
fn agentic_transcript_uses_neutral_dark_surfaces() {
    fn inspect(shape: &Shape) {
        match shape {
            Shape::Rect(rect) => {
                // Only opaque paint is a surface. Translucent state
                // overlays and subtle semantic rails are meant to carry a
                // hue, and they read against whatever is beneath them.
                for (role, color) in [("fill", rect.fill), ("stroke", rect.stroke.color)] {
                    let [red, green, blue, alpha] = color.to_array();
                    if alpha == 255 && red.max(green).max(blue) <= 100 {
                        let spread = red.max(green).max(blue) - red.min(green).min(blue);
                        assert!(
                            spread <= 6,
                            "{role} is a tinted dark instead of neutral: {color:?}"
                        );
                    }
                }
            }
            Shape::Vec(shapes) => shapes.iter().for_each(inspect),
            _ => {}
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.sidebar = false;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::User("Use one neutral palette".into()));
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("Understood".into()));
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    output.shapes.iter().for_each(|shape| inspect(&shape.shape));
}

#[test]
fn a_composer_selector_reads_as_a_control_rather_than_as_bare_text() {
    fn draw(context: &egui::Context, pointer: Option<egui::Pos2>) -> (egui::FullOutput, Rect) {
        let mut rect = None;
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(200.0, 60.0))),
                events: pointer.into_iter().map(Event::PointerMoved).collect(),
                ..RawInput::default()
            },
            |ui| {
                rect = Some(agent_selector_button(ui, "Ask", "Permissions").rect);
            },
        );
        (output, rect.unwrap())
    }
    fn selector_text(shape: &Shape) -> Option<Color32> {
        match shape {
            Shape::Text(text) if text.galley.text() == "Ask" => {
                Some(text.galley.job.sections[0].format.color)
            }
            Shape::Vec(shapes) => shapes.iter().find_map(selector_text),
            _ => None,
        }
    }
    fn hover_surface(shape: &Shape, rect: Rect) -> bool {
        match shape {
            Shape::Rect(fill) => {
                fill.fill == theme::state::hover() && fill.rect.contains(rect.center())
            }
            Shape::Vec(shapes) => shapes.iter().any(|shape| hover_surface(shape, rect)),
            _ => false,
        }
    }
    let context = theme::test_context();
    let idle = draw(&context, None);
    let hovered = draw(&context, Some(pos2(10.0, 15.0)));
    let text = |output: &egui::FullOutput| {
        output
            .shapes
            .iter()
            .find_map(|shape| selector_text(&shape.shape))
            .unwrap()
    };

    assert_eq!(text(&idle.0), theme::text().secondary);
    assert_eq!(text(&hovered.0), theme::text().primary);
    assert!(
        !idle
            .0
            .shapes
            .iter()
            .any(|shape| hover_surface(&shape.shape, idle.1)),
        "an untouched selector is quiet"
    );
    assert!(
        hovered
            .0
            .shapes
            .iter()
            .any(|shape| hover_surface(&shape.shape, hovered.1)),
        "a hovered selector has to show that it is a control"
    );
    assert!(
        idle.1.height() == theme::control::COMPACT,
        "selectors sit on the control scale: {:?}",
        idle.1
    );
}

#[test]
fn transcript_search_is_command_only_in_the_agentic_view() {
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    for agentic_mode in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root: temp.path().canonicalize().unwrap(),
            file: None,
            create: false,
        })
        .unwrap();
        app.agentic_mode = agentic_mode;
        app.agent_sidebar = !agentic_mode;
        app.agent.connection = ConnectionState::Ready;
        app.agent.session_ready = true;
        let context = theme::test_context();
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..RawInput::default()
            },
            |root| app.ui(root),
        );

        assert!(context.read_response(Id::new("agent_search")).is_none());
        app.open_find(&context);
        assert_eq!(
            app.agent_find.open, agentic_mode,
            "Cmd/Ctrl+F routed to the transcript with agentic_mode={agentic_mode}"
        );
    }
}

#[test]
fn agent_header_button_hover_brightens_the_icon_without_adding_a_surface() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.history_available = true;
    app.agent.sessions = Some(Vec::new());
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let (_, _, sidebar) = split_workspace(
        screen,
        app.sidebar,
        app.sidebar_width,
        true,
        app.agent_sidebar_width,
    );
    let button = agent_new_session_rect(sidebar.with_max_y(TITLEBAR_HEIGHT));
    let context = theme::test_context();
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            events: vec![Event::PointerMoved(button.center())],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn has_hover_surface(shape: &Shape, button: Rect) -> bool {
        match shape {
            Shape::Rect(rect) => {
                rect.rect == button
                    && rect.fill == theme::composite(theme::state::hover(), theme::surface().chrome)
            }
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_hover_surface(shape, button)),
            _ => false,
        }
    }
    fn has_bright_icon(shape: &Shape, button: Rect) -> bool {
        match shape {
            Shape::LineSegment { points, stroke } => {
                stroke.color == theme::text().primary
                    && points.iter().all(|point| button.contains(*point))
            }
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_bright_icon(shape, button)),
            _ => false,
        }
    }

    assert!(
        !output
            .shapes
            .iter()
            .any(|shape| has_hover_surface(&shape.shape, button))
    );
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_bright_icon(&shape.shape, button))
    );
}

#[test]
fn collapsed_agent_leaves_no_rail_or_empty_space() {
    let content = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let (_, editor, agent) = split_workspace(content, true, 240.0, false, 340.0);

    assert_eq!(agent.width(), 0.0);
    assert_eq!(editor.right(), content.right());
}

#[test]
fn titlebar_starts_a_fresh_paint_batch_after_window_resize() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let mut draw = |width| {
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(width, 700.0))),
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
        context.tessellate(output.shapes, output.pixels_per_point)
    };

    let _ = draw(1000.0);
    let resized = draw(1200.0);
    let titlebar_marker = resized
        .iter()
        .position(|primitive| {
            crate::renderer::retained_paint(&primitive.primitive)
                .ok()
                .flatten()
                .is_some_and(|paint| paint.key == TITLEBAR_PAINT_KEY)
        })
        .expect("titlebar paint boundary");
    let titlebar_right = resized[titlebar_marker + 1..]
        .iter()
        .find_map(|primitive| match &primitive.primitive {
            egui::epaint::Primitive::Mesh(mesh) => mesh
                .vertices
                .iter()
                .map(|vertex| vertex.pos.x)
                .reduce(f32::max),
            egui::epaint::Primitive::Callback(_) => None,
        })
        .expect("titlebar mesh");

    assert!(titlebar_right >= 1200.0);
}

#[test]
fn height_only_resize_invalidates_retained_workspace_geometry() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    let context = theme::test_context();
    let mut draw = |height| {
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, height),
                )),
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        [0x8000_0000_0000_0000, 0x9000_0000_0000_0000].map(|key| {
            primitives
                .iter()
                .find_map(|primitive| {
                    crate::renderer::retained_paint(&primitive.primitive)
                        .ok()
                        .flatten()
                        .filter(|paint| paint.key == key)
                })
                .expect("retained workspace paint boundary")
                .revision
        })
    };

    let before = draw(700.0);
    let resized = draw(800.0);

    for (before, resized) in before.into_iter().zip(resized) {
        assert_ne!(before, resized);
    }
}

#[test]
fn closing_the_open_agent_sidebar_does_not_reopen_it_in_the_same_frame() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    let context = theme::test_context();
    let button = agent_toggle_rect(Rect::from_min_size(
        pos2(640.0, 0.0),
        Vec2::new(360.0, TITLEBAR_HEIGHT),
    ));
    let mut draw = |events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let _ = draw(Vec::new());
    let _ = draw(vec![
        Event::PointerMoved(button.center()),
        Event::PointerButton {
            pos: button.center(),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    let output = draw(vec![Event::PointerButton {
        pos: button.center(),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    assert!(!app.agent_sidebar);
    assert!(!output.shapes.iter().any(|shape| has_id_clash(&shape.shape)));
}

#[test]
fn tab_controls_do_not_emit_red_debug_overlays_when_the_editor_width_changes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = (0..4)
        .map(|index| root.join(format!("tab-{index}.md")))
        .collect::<Vec<_>>();
    for path in &paths {
        fs::write(path, "# title\n\nbody\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    for path in paths.iter().skip(1) {
        app.open_tab(path.clone(), false);
    }
    app.agent_sidebar = true;
    let context = theme::test_context();
    disable_transient_egui_debug_overlays(&context);
    let draw = |app: &mut EditorApp| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let _ = draw(&mut app);
    app.agent_sidebar = false;
    let output = draw(&mut app);

    assert!(!output.shapes.iter().any(|shape| has_id_clash(&shape.shape)));
}

#[test]
fn switching_tabs_preserves_each_tabs_highlight_cache() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = root.join("first.rs");
    let second = root.join("second.rs");
    fs::write(&first, "fn first() {}\n").unwrap();
    fs::write(&second, "fn second() {}\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(first),
        create: false,
    })
    .unwrap();
    app.open_tab(second, false);
    let context = theme::test_context();
    app.activate_tab(0);
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(900.0, 700.0))),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let allocation = app.tabs[0].highlight_cache.job.text.as_ptr();

    app.activate_tab(1);
    app.activate_tab(0);

    assert_eq!(allocation, app.tabs[0].highlight_cache.job.text.as_ptr());
}

#[test]
fn editor_column_fills_the_window_without_a_statusbar() {
    let window = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let (explorer, editor_column, agent) = split_workspace(window, true, 240.0, true, 340.0);
    let editor = editor_column_content(editor_column);

    let explorer = explorer.unwrap();
    assert_eq!(explorer.y_range(), window.y_range());
    assert_eq!(agent.y_range(), window.y_range());
    assert_eq!(editor.top(), window.top() + TITLEBAR_HEIGHT);
    assert_eq!(editor.bottom(), window.bottom());
}

#[test]
fn in_file_find_bar_uses_only_the_bottom_of_its_pane() {
    let pane = Rect::from_min_size(pos2(620.0, 34.0), Vec2::new(380.0, 666.0));

    let (editor, findbar) = split_pane_content(pane, true);
    let findbar = findbar.expect("open find bar");
    assert_eq!(findbar.x_range(), pane.x_range());
    assert_eq!(editor.bottom(), findbar.top());
    assert_eq!(findbar.bottom(), pane.bottom());

    let (editor, findbar) = split_pane_content(pane, false);
    assert!(findbar.is_none());
    assert_eq!(editor, pane);
}

#[test]
fn agent_layout_keeps_the_composer_inside_the_sidebar() {
    let sidebar = Rect::from_min_size(pos2(640.0, 34.0), Vec2::new(360.0, 641.0));
    let (header, transcript, composer) = split_agent_sidebar(sidebar, AGENT_COMPOSER_HEIGHT);

    assert_eq!(header.left(), sidebar.left());
    assert_eq!(header.height(), TITLEBAR_HEIGHT);
    assert_eq!(transcript.x_range(), sidebar.x_range());
    assert_eq!(composer.x_range(), sidebar.x_range());
    assert_eq!(header.bottom(), transcript.top());
    assert_eq!(transcript.bottom(), composer.top());
    assert_eq!(composer.bottom(), sidebar.bottom());
    assert!(agent_toggle_rect(header).width() >= 32.0);
    assert!(agent_new_session_rect(header).size().min_elem() >= 32.0);
}

#[test]
fn tool_card_toggle_settles_layout_before_the_frame_is_presented() {
    let context = theme::test_context();
    let draw = |events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(400.0, 160.0))),
                events,
                ..RawInput::default()
            },
            |ui| {
                agent_collapsing_header(
                    ui,
                    "settled-tool-card",
                    "Edit src/lib.rs",
                    Some("Completed"),
                    None,
                    360.0,
                    None,
                    true,
                    false,
                    |ui| {
                        ui.label("expanded body");
                    },
                );
            },
        )
    };
    let pointer = pos2(17.0, 20.0);
    let _ = draw(Vec::new());
    let _ = draw(vec![
        Event::PointerMoved(pointer),
        Event::PointerButton {
            pos: pointer,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    let output = draw(vec![Event::PointerButton {
        pos: pointer,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);

    assert_eq!(output.platform_output.num_completed_passes, 2);
}

#[test]
fn transcript_following_does_not_treat_one_pixel_short_as_the_bottom() {
    assert!(!agent_at_bottom(999.0, 1_000.0));
}

#[test]
fn repeated_agent_transcript_controls_have_unique_widget_ids() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.transcript.extend([
        TranscriptItem::Thought("first thought".into()),
        TranscriptItem::Thought("second thought".into()),
        TranscriptItem::Plan(Vec::new()),
        TranscriptItem::Plan(Vec::new()),
    ]);
    app.agent.transcript.extend((0..2).map(|index| {
        TranscriptItem::Tool(ToolActivity {
            id: format!("read-{index}"),
            title: Some("Read File".into()),
            status: Some("Completed".into()),
            kind: None,
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: Some(format!("input-{index}")),
                content: vec![crate::agent::controller::ToolOutput::Diff {
                    path: format!("file-{index}.rs").into(),
                    old_text: Some("before".into()),
                    new_text: "after".into(),
                }],
                output: Some(format!("output-{index}")),
            }),
        })
    }));
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn has_filled_disclosure_triangle(shape: &Shape) -> bool {
        match shape {
            Shape::Path(path) => {
                path.closed
                    && path.points.len() == 3
                    && path.visual_bounding_rect().width() < 20.0
                    && path.visual_bounding_rect().height() < 20.0
            }
            Shape::Vec(shapes) => shapes.iter().any(has_filled_disclosure_triangle),
            _ => false,
        }
    }
    assert!(!output.shapes.iter().any(|shape| has_id_clash(&shape.shape)));
    assert!(
        !output
            .shapes
            .iter()
            .any(|shape| has_filled_disclosure_triangle(&shape.shape))
    );
}

#[test]
fn agent_diff_keeps_line_numbers_and_compacts_distant_context() {
    let before = "first\nsecond\nthird\nfourth\nfifth\nsixth\nseventh\neighth\nninth\ntenth\n";
    let after =
        "first\nsecond changed\nthird\nfourth\nfifth\nsixth\nseventh\neighth\nninth\ntenth\nlast\n";

    let diff = build_agent_diff(Some(before), after);

    assert_eq!(diff.removed, 1);
    assert_eq!(diff.added, 2);
    assert!(diff.lines.iter().any(|line| {
        line.old_number == Some(2) && line.new_number.is_none() && line.text == "second"
    }));
    assert!(diff.lines.iter().any(|line| {
        line.old_number.is_none() && line.new_number == Some(2) && line.text == "second changed"
    }));
    assert!(diff.lines.iter().any(|line| line.omitted == 2));
    assert!(diff.lines.iter().any(|line| {
        line.old_number.is_none() && line.new_number == Some(11) && line.text == "last"
    }));
}

#[test]
fn agent_diff_preview_bounds_long_changed_runs() {
    let after = (1..=100)
        .map(|line| format!("changed line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let diff = build_agent_diff(None, &after);
    let preview = agent_diff_preview(&diff.lines).expect("long diff preview");

    assert_eq!(preview.len(), 18);
    assert_eq!(preview[0].new_number, Some(1));
    assert_eq!(preview[11].new_number, Some(12));
    assert_eq!(preview[12].kind, super::AgentDiffKind::Omitted);
    assert_eq!(preview[12].omitted, 83);
    assert_eq!(preview.last().and_then(|line| line.new_number), Some(100));
}

#[test]
fn unchanged_agent_markdown_and_diffs_reuse_their_frame_work() {
    let context = theme::test_context();
    let syntaxes = SyntaxManager::built_in().unwrap();
    let highlighter = Highlighter::new().unwrap();
    let draw = |new_text: &str| {
        let mut cached = None;
        let _ = context.run_ui(RawInput::default(), |ui| {
            let markdown = agent_markdown_galley(
                ui,
                Id::new("cached_markdown"),
                "**fast**",
                320.0,
                &highlighter,
                &syntaxes,
                false,
                None,
            );
            let diff = cached_agent_diff(ui, Id::new("cached_diff"), Some("before"), new_text);
            cached = Some((markdown, diff));
        });
        cached.unwrap()
    };

    let first = draw("after");
    let unchanged = draw("after");
    let changed = draw("changed");

    assert!(std::sync::Arc::ptr_eq(&first.0, &unchanged.0));
    assert!(std::sync::Arc::ptr_eq(&first.1, &unchanged.1));
    assert!(!std::sync::Arc::ptr_eq(&unchanged.1, &changed.1));
}

#[test]
fn agent_diff_caches_are_reused_across_session_reloads() {
    use crate::agent::controller::{ToolActivity, ToolDetail, ToolOutput};

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    let load = |app: &mut EditorApp, session: &str| {
        app.agent.session_id = Some(session.into());
        app.agent.transcript.clear();
        for index in 0..8 {
            app.agent
                .transcript
                .push_back(TranscriptItem::Tool(ToolActivity {
                    id: format!("{session}-{index}"),
                    title: Some(format!("Tool {index}")),
                    status: Some("Completed".into()),
                    kind: None,
                    paths: Vec::new(),
                    detail: Some(ToolDetail {
                        input: None,
                        content: vec![ToolOutput::Diff {
                            path: format!("file-{index}.rs").into(),
                            old_text: Some("before\n".into()),
                            new_text: format!("after {session}\n").into(),
                        }],
                        output: None,
                    }),
                }));
        }
    };
    let context = theme::test_context();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1000.0, 700.0),
        )),
        ..RawInput::default()
    };

    load(&mut app, "first");
    let _ = context.run_ui(input(), |root| app.ui(root));
    let first = agent_diff_cache_count(&context);
    load(&mut app, "second");
    let _ = context.run_ui(input(), |root| app.ui(root));

    assert_eq!(agent_diff_cache_count(&context), first);
}

#[test]
fn ui_scale_change_rebuilds_cached_agent_text() {
    let context = theme::test_context();
    let syntaxes = SyntaxManager::built_in().unwrap();
    let highlighter = Highlighter::new().unwrap();
    let draw = || {
        let mut cached = None;
        let _ = context.run_ui(RawInput::default(), |ui| {
            cached = Some((
                agent_markdown_galley(
                    ui,
                    Id::new("scaled_agent_markdown"),
                    "**cached agent text**",
                    320.0,
                    &highlighter,
                    &syntaxes,
                    false,
                    None,
                ),
                super::agent_code_galley(
                    ui,
                    Id::new("scaled_agent_code"),
                    std::path::Path::new("cached.rs"),
                    "fn cached_agent_code() {}",
                    320.0,
                    &highlighter,
                    &syntaxes,
                    None,
                ),
            ));
        });
        cached.unwrap()
    };

    let before = draw();
    context.set_zoom_factor(1.5);
    let after = draw();

    assert!(!std::sync::Arc::ptr_eq(&before.0, &after.0));
    assert!(!std::sync::Arc::ptr_eq(&before.1, &after.1));
}

#[test]
fn agent_markdown_syntax_highlights_fenced_code() {
    let mut galley = None;
    let syntaxes = SyntaxManager::built_in().unwrap();
    let highlighter = Highlighter::new().unwrap();
    let _ = theme::test_context().run_ui(RawInput::default(), |ui| {
        galley = Some(agent_markdown_galley(
            ui,
            Id::new("highlighted_markdown"),
            "```rust\nfn main() { let value = \"ok\"; }\n```",
            320.0,
            &highlighter,
            &syntaxes,
            false,
            None,
        ));
    });
    let galley = galley.unwrap();
    let color_at = |needle: &str| {
        let offset = galley.text().find(needle).unwrap();
        galley
            .job
            .sections
            .iter()
            .find(|section| section.byte_range.contains(&offset.into()))
            .unwrap()
            .format
            .color
    };

    assert_ne!(color_at("fn"), color_at("main"));
}

#[test]
fn agent_tool_file_output_uses_detected_syntax() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::Tool(ToolActivity {
            id: "read-rust".into(),
            title: Some("Read main.rs".into()),
            status: Some("Completed".into()),
            kind: None,
            paths: vec!["main.rs".into()],
            detail: Some(ToolDetail {
                input: None,
                content: vec![
                    crate::agent::controller::ToolOutput::Text(
                        "fn read_value() { let value = \"ok\"; }".into(),
                    ),
                    crate::agent::controller::ToolOutput::Diff {
                        path: "main.rs".into(),
                        old_text: Some("fn before() {}".into()),
                        new_text: "fn after() {}".into(),
                    },
                ],
                output: None,
            }),
        }));
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn code_colors(shape: &Shape) -> Option<(Color32, Color32)> {
        match shape {
            Shape::Text(text) if text.galley.text().contains("read_value") => {
                let color_at = |needle: &str| {
                    let offset = text.galley.text().find(needle).unwrap();
                    text.galley
                        .job
                        .sections
                        .iter()
                        .find(|section| section.byte_range.contains(&offset.into()))
                        .unwrap()
                        .format
                        .color
                };
                Some((color_at("fn"), color_at("read_value")))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(code_colors),
            _ => None,
        }
    }
    let colors = output
        .shapes
        .iter()
        .find_map(|shape| code_colors(&shape.shape))
        .expect("rendered tool output");

    assert_ne!(colors.0, colors.1);
}

#[test]
fn read_and_edit_tool_cards_do_not_repeat_their_paths_in_the_body() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    for (index, action) in ["Read", "Edit"].into_iter().enumerate() {
        let path = format!("/workspace/src/file-{index}.rs");
        app.agent
            .transcript
            .push_back(TranscriptItem::Tool(ToolActivity {
                id: format!("{action}-{index}"),
                title: Some(format!("{action} {path}")),
                status: Some("Completed".into()),
                kind: None,
                paths: vec![path.into()],
                detail: Some(ToolDetail {
                    input: None,
                    content: vec![crate::agent::controller::ToolOutput::Diff {
                        path: format!("changed-{index}.rs").into(),
                        old_text: None,
                        new_text: "fn changed() {}".into(),
                    }],
                    output: None,
                }),
            }));
    }
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn path_count(shape: &Shape) -> usize {
        match shape {
            Shape::Text(text) => {
                usize::from(text.galley.text().starts_with("/workspace/src/file-"))
            }
            Shape::Vec(shapes) => shapes.iter().map(path_count).sum(),
            _ => 0,
        }
    }

    assert_eq!(
        output
            .shapes
            .iter()
            .map(|shape| path_count(&shape.shape))
            .sum::<usize>(),
        2
    );
}

#[test]
fn subagent_cards_are_distinct_and_collapse_their_prompt() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::Tool(ToolActivity {
            id: "task-1".into(),
            title: Some("Subagent: Review changes".into()),
            status: Some("InProgress".into()),
            kind: Some("Task".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: None,
                content: vec![crate::agent::controller::ToolOutput::Task {
                    description: "Review changes".into(),
                    prompt: "dig through the auth flow and report issues".into(),
                    subagent_type: "reviewer".into(),
                    model: Some("gpt-5".into()),
                    agent_id: Some("agent-9".into()),
                    duration_ms: Some(1200),
                }],
                output: None,
            }),
        }));
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn collect_text(shape: &Shape, texts: &mut String) {
        match shape {
            Shape::Text(text) => {
                texts.push_str(text.galley.text());
                texts.push('\n');
            }
            Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect_text(shape, texts)),
            _ => {}
        }
    }
    let mut texts = String::new();
    for shape in &output.shapes {
        collect_text(&shape.shape, &mut texts);
    }

    assert!(texts.contains("Subagent: Review changes"), "{texts}");
    assert!(texts.contains("SUBAGENT"), "{texts}");
    assert!(texts.contains("reviewer · gpt-5 · 1.2 s"), "{texts}");
    assert!(texts.contains("Running"), "{texts}");
    assert!(texts.contains("Prompt"), "{texts}");
    assert!(!texts.contains("dig through the auth flow"), "{texts}");
}

#[test]
fn dense_subagent_rows_show_their_status() {
    let output = theme::test_context().run_ui(RawInput::default(), |ui| {
        agent_dense_tool(
            ui,
            Id::new("dense_subagent_status"),
            "Subagent: Review changes",
            Some("InProgress"),
            None,
            None,
            false,
            |_| {},
        );
    });
    fn contains_running(shape: &Shape) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == "Running",
            Shape::Vec(shapes) => shapes.iter().any(contains_running),
            _ => false,
        }
    }

    assert!(
        output
            .shapes
            .iter()
            .any(|shape| contains_running(&shape.shape))
    );
}

#[test]
fn agent_paths_open_in_the_editor_at_the_reported_line() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("touched.rs");
    fs::write(&path, "one\ntwo\nthree\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: None,
        create: false,
    })
    .unwrap();

    app.open_agent_path(path.clone(), Some(3));

    let tab = app
        .tabs
        .iter()
        .find(|tab| tab.buffer.path == path)
        .expect("agent path opens a tab");
    assert_eq!(tab.editor_surface.selection(), 8..8);
    assert_eq!(app.lsp_scroll_to, Some((path, 8)));
}

#[test]
fn agent_paths_resolve_relative_to_the_workspace_root() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("relative.txt"), "content").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: root.clone(),
        file: None,
        create: false,
    })
    .unwrap();

    app.open_agent_path("relative.txt".into(), None);

    assert!(
        app.tabs
            .iter()
            .any(|tab| tab.buffer.path == root.join("relative.txt"))
    );
}

#[test]
fn missing_agent_paths_report_an_error_instead_of_opening_a_tab() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let tabs_before = app.tabs.len();

    app.open_agent_path("missing.txt".into(), None);

    assert_eq!(app.tabs.len(), tabs_before);
}

#[test]
fn changed_file_diffs_open_as_a_diff_tab_in_ide_mode() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("lib.rs");
    fs::write(&path, "new\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: None,
        create: false,
    })
    .unwrap();
    app.agent
        .baselines
        .insert(path.clone(), Some("old\n".to_owned()));

    app.open_agent_diff(path.clone());

    let tab = app
        .tabs
        .iter()
        .find(|tab| tab.buffer.path == path)
        .expect("the diff opens the file's tab");
    assert_eq!(tab.agent_diff, Some(Some("old\n".to_owned())));
    assert!(app.agentic_diffs.is_empty());
}

#[test]
fn changed_file_diffs_without_a_baseline_fall_back_to_a_plain_open() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("lib.rs");
    fs::write(&path, "new\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: None,
        create: false,
    })
    .unwrap();

    app.open_agent_diff(path.clone());

    let tab = app
        .tabs
        .iter()
        .find(|tab| tab.buffer.path == path)
        .expect("the file still opens");
    assert_eq!(tab.agent_diff, None);
}

#[test]
fn changed_file_diffs_open_the_side_panel_in_agentic_mode() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("lib.rs"), "new\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: root.clone(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    // Baselines are keyed by the path exactly as the agent reported it,
    // which may be relative to the workspace root.
    app.agent
        .baselines
        .insert("lib.rs".into(), Some("old\n".to_owned()));
    let tabs_before = app.tabs.len();

    app.open_agent_diff("lib.rs".into());

    let panel = app
        .agentic_diffs
        .get(app.active_agentic_diff)
        .expect("the side panel opens");
    assert_eq!(panel.path, root.join("lib.rs"));
    assert_eq!(panel.baseline, Some("old\n".to_owned()));
    assert_eq!(panel.text, "new\n");
    assert_eq!(app.tabs.len(), tabs_before, "no editor tab is opened");
}

#[test]
fn missing_changed_file_does_not_open_its_retained_agent_diff() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: root.clone(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent
        .apply(crate::agent::controller::Event::ToolCallUpdated(
            ToolActivity {
                id: "edit-index".into(),
                title: Some("Edit index.html".into()),
                status: Some("Completed".into()),
                kind: Some("Edit".into()),
                paths: vec!["index.html".into()],
                detail: Some(ToolDetail {
                    input: None,
                    content: vec![crate::agent::controller::ToolOutput::Diff {
                        path: "index.html".into(),
                        old_text: Some("old\n".into()),
                        new_text: "<main>retained</main>\n".into(),
                    }],
                    output: None,
                }),
            },
        ));

    app.open_agent_diff("index.html".into());

    assert!(app.agentic_diffs.is_empty());
}

#[test]
fn agentic_diff_panel_keeps_multiple_open_files_as_tabs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    for file in ["first.rs", "second.rs"] {
        fs::write(root.join(file), format!("new {file}\n")).unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;

    app.open_agent_diff("first.rs".into());
    app.open_agent_diff("second.rs".into());

    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1200.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }

    assert!(["first.rs", "second.rs"].into_iter().all(|expected| {
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, expected))
    }));
}

#[test]
fn agentic_diff_tabs_switch_and_close_files_independently() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("first.rs"), "first diff body\n").unwrap();
    fs::write(root.join("second.rs"), "second diff body\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: root.clone(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .baselines
        .insert("first.rs".into(), Some("old first\n".into()));
    app.agent
        .baselines
        .insert("second.rs".into(), Some("old second\n".into()));
    app.open_agent_diff("first.rs".into());
    app.open_agent_diff("second.rs".into());
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1200.0, 700.0));
    let draw = |app: &mut EditorApp, events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    let _ = draw(&mut app, Vec::new());
    let first = context
        .read_response(Id::new(("agentic_diff_tab", root.join("first.rs"))))
        .expect("first diff tab")
        .rect
        .center();
    let _ = draw(
        &mut app,
        vec![
            Event::PointerMoved(first),
            Event::PointerButton {
                pos: first,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    let _ = draw(
        &mut app,
        vec![Event::PointerButton {
            pos: first,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    let output = draw(&mut app, Vec::new());
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(expected),
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }

    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "first diff body"))
    );

    let close_first = context
        .read_response(Id::new(("agentic_diff_tab_close", root.join("first.rs"))))
        .expect("first diff close button")
        .rect
        .center();
    let _ = draw(
        &mut app,
        vec![
            Event::PointerMoved(close_first),
            Event::PointerButton {
                pos: close_first,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    let _ = draw(
        &mut app,
        vec![Event::PointerButton {
            pos: close_first,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    let output = draw(&mut app, Vec::new());
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "second diff body"))
    );
}

#[test]
fn the_agentic_diff_panel_renders_beside_the_conversation() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: root.clone(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agentic_diffs.push(AgenticDiff {
        path: root.join("lib.rs"),
        baseline: Some("old\n".to_owned()),
        text: "new\n".to_owned(),
    });
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1200.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn find_text(shape: &Shape, expected: &str) -> Option<Rect> {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| find_text(shape, expected)),
            _ => None,
        }
    }
    let find = |expected: &str| {
        output
            .shapes
            .iter()
            .find_map(|shape| find_text(&shape.shape, expected))
    };

    // The file header lives in the titlebar strip beside the session
    // title, so the two columns wear one continuous header.
    for label in ["lib.rs", "MODIFIED"] {
        let rect = find(label).unwrap_or_else(|| panic!("{label} is not on screen"));
        assert!(
            (rect.center().y - TITLEBAR_HEIGHT * 0.5).abs() <= 1.0,
            "{label} sits at {:?} instead of the titlebar strip",
            rect.center()
        );
    }
}

#[test]
fn the_diff_panel_yields_when_the_column_is_too_narrow() {
    let wide = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 600.0));
    let (conversation, panel) = split_agentic_diff(wide, true);
    let panel = panel.expect("a wide column fits the panel");
    assert!((panel.width() - 550.0).abs() < 0.5);
    assert!((conversation.right() - panel.left()).abs() < 0.5);

    let narrow = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(600.0, 600.0));
    let (conversation, panel) = split_agentic_diff(narrow, true);
    assert!(panel.is_none());
    assert_eq!(conversation, narrow);
}

#[test]
fn changed_files_render_as_a_summary_card_with_line_stats() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: root.clone(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("done".into()));
    app.agent.changed_paths.insert(
        root.join("src/lib.rs"),
        FileChange {
            added: 2,
            removed: 1,
        },
    );
    app.agent
        .changed_paths
        .insert(root.join("README.md"), FileChange::default());
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn collect_text(shape: &Shape, texts: &mut String) {
        match shape {
            Shape::Text(text) => {
                texts.push_str(text.galley.text());
                texts.push('\n');
            }
            Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect_text(shape, texts)),
            _ => {}
        }
    }
    let mut texts = String::new();
    for shape in &output.shapes {
        collect_text(&shape.shape, &mut texts);
    }

    assert!(texts.contains("2 files changed"), "{texts}");
    assert!(texts.contains("lib.rs"), "{texts}");
    assert!(texts.contains("README.md"), "{texts}");
    assert!(texts.contains("+2"), "{texts}");
    assert!(texts.contains("−1"), "{texts}");
}

#[test]
fn deleted_changed_file_keeps_its_counts_but_cannot_be_opened() {
    fn find_text(shape: &Shape, expected: &str) -> Option<Rect> {
        match shape {
            Shape::Text(text) if text.galley.text().contains(expected) => {
                Some(text.visual_bounding_rect())
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| find_text(shape, expected)),
            _ => None,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let path = root.join("deleted.html");
    let changed = std::collections::HashMap::from([(
        path.clone(),
        FileChange {
            added: 2,
            removed: 1,
        },
    )]);
    let context = theme::test_context();
    let draw = |events| {
        let mut clicked = None;
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(500.0, 180.0))),
                events,
                ..RawInput::default()
            },
            |ui| clicked = draw_agent_changed_files(ui, &root, &changed, None),
        );
        (output, clicked)
    };

    let (output, _) = draw(Vec::new());
    let text = output
        .shapes
        .iter()
        .filter_map(|shape| match &shape.shape {
            Shape::Text(text) => Some(text.galley.text()),
            _ => None,
        })
        .collect::<String>();
    assert!(text.contains("Deleted"), "{text}");
    assert!(text.contains("+2"), "{text}");
    assert!(text.contains("−1"), "{text}");

    let row = output
        .shapes
        .iter()
        .find_map(|shape| find_text(&shape.shape, "deleted.html"))
        .expect("deleted file row")
        .center();
    let _ = draw(vec![
        Event::PointerMoved(row),
        Event::PointerButton {
            pos: row,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    let (_, clicked) = draw(vec![Event::PointerButton {
        pos: row,
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    }]);
    assert_eq!(clicked, None);
}

#[test]
fn individual_edit_cards_render_their_line_stats() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .apply(crate::agent::controller::Event::ToolCallUpdated(
            ToolActivity {
                id: "edit-lib".into(),
                title: Some("Edit src/lib.rs".into()),
                status: Some("Completed".into()),
                kind: Some("Edit".into()),
                paths: vec!["src/lib.rs".into()],
                detail: Some(ToolDetail {
                    input: None,
                    content: vec![crate::agent::controller::ToolOutput::Diff {
                        path: "src/lib.rs".into(),
                        old_text: Some("keep\nremove\n".into()),
                        new_text: "keep\nadd one\nadd two\n".into(),
                    }],
                    output: None,
                }),
            },
        ));
    // The closing summary has the same counts; remove it so this check can
    // only be satisfied by the individual tool card header.
    app.agent.changed_paths.clear();
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn collect_text(shape: &Shape, texts: &mut String) {
        match shape {
            Shape::Text(text) => {
                texts.push_str(text.galley.text());
                texts.push('\n');
            }
            Shape::Vec(shapes) => shapes.iter().for_each(|shape| collect_text(shape, texts)),
            _ => {}
        }
    }
    let mut texts = String::new();
    for shape in &output.shapes {
        collect_text(&shape.shape, &mut texts);
    }

    assert!(texts.contains("+2"), "{texts}");
    assert!(texts.contains("−1"), "{texts}");
}

#[test]
#[ignore = "timing spike, run manually"]
fn timing_spike_for_agent_sidebar_costs() {
    let source = (0..12_000)
        .map(|index| {
            format!(
                "<div class=\"row-{index}\"><span>cell {index}</span>\
                     <script>var value_{index} = {index} * 2;</script></div>\n"
            )
        })
        .collect::<String>();
    let syntaxes = SyntaxManager::built_in().unwrap();
    let highlighter = Highlighter::new().unwrap();
    let syntax = syntaxes.detect(std::path::Path::new("docs/index.html"), false);

    let start = std::time::Instant::now();
    let job = highlighter
        .highlight_job(&source, syntax, syntaxes.set(), f32::INFINITY)
        .unwrap();
    eprintln!(
        "highlight {} lines: {:?} ({} sections)",
        source.lines().count(),
        start.elapsed(),
        job.sections.len()
    );

    let start = std::time::Instant::now();
    let bundle = crate::agent::provision::embedded_bundle();
    eprintln!(
        "embedded_bundle: {:?} ok={}",
        start.elapsed(),
        bundle.is_ok()
    );

    // Every line changes, like a full-file rewrite: no context compaction,
    // so the expanded card carries added+removed rows for the whole file.
    let new_text = source.replace("cell", "unit");
    let start = std::time::Instant::now();
    let diff = build_agent_diff(Some(&source), &new_text);
    eprintln!(
        "diff {} lines: {:?} (+{} -{}, {} rows)",
        source.lines().count(),
        start.elapsed(),
        diff.added,
        diff.removed,
        diff.lines.len()
    );

    let start = std::time::Instant::now();
    let job = crate::markdown::compact_layout(&source, 700.0, |_, code| {
        highlighter
            .highlight_job(code, syntaxes.plain_text(), syntaxes.set(), 700.0)
            .ok()
    });
    eprintln!(
        "markdown layout: {:?} ({} sections)",
        start.elapsed(),
        job.sections.len()
    );

    let context = theme::test_context();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(700.0, 700.0))),
        ..RawInput::default()
    };
    // Force the card open: the complaint is scrolling with a large diff
    // expanded, so steady-state frames must pay the expanded cost.
    context.data_mut(|data| data.insert_temp(Id::new("spike_diff").with("expanded"), true));
    let render = |label: &str| {
        let start = std::time::Instant::now();
        let _ = context.run_ui(input(), |ui| {
            draw_agent_diff(
                ui,
                Id::new("spike_diff"),
                std::path::Path::new("docs/index.html"),
                Some(&source),
                &new_text,
                &highlighter,
                &syntaxes,
                None,
                false,
            );
        });
        eprintln!("{label}: {:?}", start.elapsed());
    };
    render("expanded diff first frame");
    for frame in 0..5 {
        render(&format!("expanded diff steady frame {frame}"));
    }
    let render_search = |label: &str| {
        let start = std::time::Instant::now();
        let _ = context.run_ui(input(), |ui| {
            draw_agent_diff(
                ui,
                Id::new("spike_diff"),
                std::path::Path::new("docs/index.html"),
                Some(&source),
                &new_text,
                &highlighter,
                &syntaxes,
                Some(("unit", Some(3))),
                false,
            );
        });
        eprintln!("{label}: {:?}", start.elapsed());
    };
    for frame in 0..3 {
        render_search(&format!("expanded diff search frame {frame}"));
    }
}

#[test]
fn huge_diff_rows_skip_syntax_highlighting_but_previews_stay_colored() {
    let syntaxes = SyntaxManager::built_in().unwrap();
    let highlighter = Highlighter::new().unwrap();
    let source = (0..1_500)
        .map(|index| format!("<span class=\"cell\">item {index}</span>\n"))
        .collect::<String>();
    let new_text = source.replace("item", "unit");
    fn code_row_sections(output: &egui::FullOutput, needle: &str) -> Option<usize> {
        fn visit(shape: &Shape, needle: &str) -> Option<usize> {
            match shape {
                Shape::Text(text) if text.galley.text().contains(needle) => {
                    Some(text.galley.job.sections.len())
                }
                Shape::Vec(shapes) => shapes.iter().find_map(|shape| visit(shape, needle)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| visit(&shape.shape, needle))
    }
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(700.0, 700.0))),
        ..RawInput::default()
    };
    let render = |context: &egui::Context, id: &str| {
        context.run_ui(input(), |ui| {
            draw_agent_diff(
                ui,
                Id::new(id),
                std::path::Path::new("page.html"),
                Some(&source),
                &new_text,
                &highlighter,
                &syntaxes,
                None,
                false,
            );
        })
    };
    // Expanded: 1500 changed lines per side is past the highlight cap, so
    // a code row is gutter + sign + one plain code section.
    let context = theme::test_context();
    context.data_mut(|data| data.insert_temp(Id::new("huge").with("expanded"), true));
    let expanded = render(&context, "huge");
    let sections =
        code_row_sections(&expanded, "item 0").expect("expanded diff renders its first rows");
    assert!(
        sections <= 3,
        "a capped diff row must not carry syntax sections, found {sections}"
    );
    // Collapsed: the preview only needs a handful of lines, so it keeps
    // real syntax colors (many sections per row).
    let context = theme::test_context();
    let preview = render(&context, "preview");
    let sections = code_row_sections(&preview, "item 0").expect("preview renders its first rows");
    assert!(
        sections > 3,
        "the collapsed preview should stay syntax highlighted, found {sections}"
    );
}

#[test]
fn agent_diff_uses_detected_syntax() {
    let syntaxes = SyntaxManager::built_in().unwrap();
    let highlighter = Highlighter::new().unwrap();
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(600.0, 300.0))),
            ..RawInput::default()
        },
        |ui| {
            draw_agent_diff(
                ui,
                Id::new("highlighted_diff"),
                std::path::Path::new("main.rs"),
                Some("fn before() {}"),
                "fn after() {}",
                &highlighter,
                &syntaxes,
                None,
                false,
            );
        },
    );
    fn code_colors(shape: &Shape) -> Option<(Color32, Color32)> {
        match shape {
            Shape::Text(text) if text.galley.text().contains("before") => {
                let color_at = |needle: &str| {
                    let offset = text.galley.text().find(needle).unwrap();
                    text.galley
                        .job
                        .sections
                        .iter()
                        .find(|section| section.byte_range.contains(&offset.into()))
                        .unwrap()
                        .format
                        .color
                };
                Some((color_at("fn"), color_at("before")))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(code_colors),
            _ => None,
        }
    }
    let colors = output
        .shapes
        .iter()
        .find_map(|shape| code_colors(&shape.shape))
        .expect("rendered removed diff line");

    assert_ne!(colors.0, colors.1);
}

#[test]
fn agent_file_edits_are_collapsed_by_default() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::Tool(ToolActivity {
            id: "edit".into(),
            title: Some(format!("Edit {}", "/very/long/path".repeat(20))),
            status: Some("InProgress".into()),
            kind: None,
            paths: Vec::new(),
            detail: None,
        }));
    let context = theme::test_context();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1100.0, 700.0),
        )),
        ..RawInput::default()
    };
    let _ = context.run_ui(input(), |root| app.ui(root));
    let Some(TranscriptItem::Tool(tool)) = app.agent.transcript.back_mut() else {
        unreachable!();
    };
    let old_text = (1..=953)
        .map(|line| format!("old {line}: {}", "wide content ".repeat(20)))
        .collect::<Vec<_>>()
        .join("\n");
    let new_text = (1..=1383)
        .map(|line| format!("new {line}: {}", "wide content ".repeat(20)))
        .collect::<Vec<_>>()
        .join("\n");
    tool.status = Some("Completed".into());
    tool.detail = Some(ToolDetail {
        input: Some("raw edit request".into()),
        content: vec![crate::agent::controller::ToolOutput::Diff {
            path: "sample.rs".into(),
            old_text: Some(old_text.into()),
            new_text: new_text.into(),
        }],
        output: Some("generic completion summary".into()),
    });
    let output = context.run_ui(input(), |root| app.ui(root));
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(expected),
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }

    assert!(
        output
            .shapes
            .iter()
            .all(|shape| !has_text(&shape.shape, "old 1:")),
        "a file edit painted its diff before the user expanded it"
    );
}

#[test]
fn agentic_empty_state_centers_at_every_available_height() {
    for height in [90.0, 136.0, 700.0] {
        let region = Rect::from_min_size(pos2(12.0, 34.0), Vec2::new(800.0, height));
        assert_eq!(
            agent_empty_state_rect(region, true).center(),
            region.center()
        );
    }
}

#[test]
fn sidebar_empty_state_is_centered_in_the_available_transcript() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 900.0));
    let (_, _, sidebar) = split_workspace(
        screen,
        app.sidebar,
        app.sidebar_width,
        true,
        app.agent_sidebar_width,
    );
    let (_, transcript, _) = split_agent_sidebar(sidebar, AGENT_COMPOSER_HEIGHT);
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let empty = context
        .read_response(Id::new("agent_empty_state"))
        .expect("empty state region")
        .rect;
    assert!((empty.center().y - transcript.center().y).abs() <= 1.0);
    let heading = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.text() == "Start a task" => Some(text),
            _ => None,
        })
        .expect("empty state heading");
    assert_eq!(
        heading.galley.job.sections[0].format.color,
        theme::text().primary
    );
}

#[test]
fn agentic_composer_is_darker_than_the_agent_canvas() {
    assert_eq!(
        super::agentic_composer_fill(),
        theme::mix(theme::surface().editor, theme::surface().input, 0.7)
    );
}

#[test]
fn agent_transcript_constrains_response_and_tool_text_to_the_sidebar() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    let command = "`python3 -c \"\nfrom pathlib import Path\nhtml = Path('/a/very/long/project/path/index.html').read_text()\nassert 'Install Editur' in html\n\"`";
    let response = "Building a static landing page that matches Editur's cyan-on-charcoal visual identity and remains readable inside the agent sidebar.";
    app.agent.transcript.extend([
        TranscriptItem::Tool(ToolActivity {
            id: "grep".into(),
            title: Some(command.into()),
            status: Some("Completed".into()),
            kind: None,
            paths: Vec::new(),
            detail: None,
        }),
        TranscriptItem::Assistant(response.into()),
    ]);
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(760.0, 700.0))),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn text_metrics(shape: &Shape, expected: &str) -> Option<(Rect, usize)> {
        match shape {
            Shape::Text(text) if text.galley.text().trim_end() == expected => Some((
                Rect::from_min_size(text.pos, text.galley.size()),
                text.galley.rows.len(),
            )),
            Shape::Vec(shapes) => shapes
                .iter()
                .find_map(|shape| text_metrics(shape, expected)),
            _ => None,
        }
    }
    fn tool_card_rect(shape: &Shape) -> Option<Rect> {
        match shape {
            Shape::Rect(rect) if rect.fill == theme::surface().raised => Some(rect.rect),
            Shape::Vec(shapes) => shapes.iter().find_map(tool_card_rect),
            _ => None,
        }
    }
    let metrics = |expected| {
        output.shapes.iter().find_map(|shape| {
            text_metrics(&shape.shape, expected).map(|(rect, rows)| (rect, rows, shape.clip_rect))
        })
    };

    let (command_rect, command_rows, command_clip) =
        metrics("`python3 -c \"").expect("compact command title");
    let card_rect = output
        .shapes
        .iter()
        .find_map(|shape| tool_card_rect(&shape.shape))
        .expect("tool card");
    assert_eq!(command_rows, 1);
    assert!(command_rect.right() <= command_clip.right());
    assert!(
        card_rect.bottom() - command_rect.bottom() <= command_rect.top() - card_rect.top() + 0.5,
        "card={card_rect:?} command={command_rect:?}"
    );
    let (response_rect, response_rows, response_clip) = metrics(response).expect("Cursor response");
    assert!(
        command_rect.left() <= response_rect.left() + 48.0,
        "command={command_rect:?} response={response_rect:?}"
    );
    assert!(response_rows > 1);
    assert!(response_rect.right() <= response_clip.right());
}

#[test]
fn agent_output_keeps_scrollable_clearance_above_the_composer_in_both_layouts() {
    fn text_rect(shape: &Shape, expected: &str) -> Option<Rect> {
        match shape {
            Shape::Text(text) if text.galley.text().contains(expected) => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_rect(shape, expected)),
            _ => None,
        }
    }

    for agentic_mode in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root: temp.path().canonicalize().unwrap(),
            file: None,
            create: false,
        })
        .unwrap();
        app.agentic_mode = agentic_mode;
        app.agent_sidebar = !agentic_mode;
        app.agent.connection = ConnectionState::Ready;
        app.agent.session_ready = true;
        for index in 0..60 {
            app.agent
                .transcript
                .push_back(TranscriptItem::Assistant(format!("Reply number {index}")));
        }
        app.agent
            .transcript
            .push_back(TranscriptItem::Assistant("End of transcript".into()));
        let context = theme::test_context();
        let input = || RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        };

        let _ = context.run_ui(input(), |root| app.ui(root));
        let output = context.run_ui(input(), |root| app.ui(root));
        let (text, clip) = output
            .shapes
            .iter()
            .find_map(|shape| {
                text_rect(&shape.shape, "End of transcript").map(|text| (text, shape.clip_rect))
            })
            .expect("newest output visible at the bottom");

        assert!(
            clip.bottom() - text.bottom() >= theme::space::LARGE - 1.0,
            "end clearance must scroll with the output in agentic_mode={agentic_mode}: \
                 text={text:?} clip={clip:?}"
        );
    }
}

#[test]
fn agent_transcript_culls_offscreen_items_and_restores_them_when_scrolled_into_view() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    for index in 0..120 {
        app.agent
            .transcript
            .push_back(TranscriptItem::Assistant(format!(
                "Reply number {index} ends here"
            )));
    }
    fn contains_text(shape: &Shape, needle: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(needle),
            Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, needle)),
            _ => false,
        }
    }
    fn text_pos(shape: &Shape, needle: &str) -> Option<Pos2> {
        match shape {
            Shape::Text(text) if text.galley.text().contains(needle) => Some(text.pos),
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_pos(shape, needle)),
            _ => None,
        }
    }
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(760.0, 700.0))),
        ..RawInput::default()
    };
    let context = theme::test_context();
    // First frame measures every item; the second starts from cached
    // heights with the view pinned to the newest reply.
    let _ = context.run_ui(input(), |root| app.ui(root));
    assert_eq!(
        app.agent_transcript_rendered, 120,
        "the measuring frame lays out every item"
    );
    let output = context.run_ui(input(), |root| app.ui(root));
    let last = output
        .shapes
        .iter()
        .find_map(|shape| text_pos(&shape.shape, "Reply number 119 ends here"))
        .expect("newest reply visible at the bottom");
    assert!(
        app.agent_transcript_rendered < 60,
        "items far above the viewport must be culled into spacers, \
             but {} of 120 were laid out",
        app.agent_transcript_rendered
    );
    // Wheel-scroll back to the top over the transcript; culled items must
    // rematerialize once they enter the viewport.
    let mut oldest_visible = false;
    for _ in 0..12 {
        let mut scroll = input();
        scroll.events = vec![
            Event::PointerMoved(last),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, 1_000_000.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ];
        let output = context.run_ui(scroll, |root| app.ui(root));
        if output
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "Reply number 0 ends here"))
        {
            oldest_visible = true;
            assert!(
                app.agent_transcript_rendered < 60,
                "culling must keep working at the top of the transcript"
            );
            break;
        }
    }
    assert!(
        oldest_visible,
        "scrolling up must bring culled items back into the transcript"
    );
}

#[test]
fn scrolling_to_the_top_lazily_loads_earlier_transcript_pages() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    for id in 0..2_050 {
        app.agent
            .apply(crate::agent::controller::Event::ToolCallUpdated(
                ToolActivity {
                    id: id.to_string(),
                    title: Some(format!("Tool {id}")),
                    status: Some("Completed".into()),
                    kind: None,
                    paths: Vec::new(),
                    detail: None,
                },
            ));
    }
    assert!(app.agent.has_earlier_transcript());

    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1_000.0, 700.0));
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            events: vec![
                Event::PointerMoved(pos2(800.0, 300.0)),
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, 1_000_000.0),
                    phase: TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                },
            ],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(!app.agent.has_earlier_transcript());
    assert!(app.agent.has_later_transcript());
}

#[test]
fn completed_permission_cards_use_the_dense_transcript_gap() {
    fn gap(dense: bool) -> f32 {
        let temp = tempfile::tempdir().unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root: temp.path().canonicalize().unwrap(),
            file: None,
            create: false,
        })
        .unwrap();
        app.agent_sidebar = true;
        app.settings.appearance.dense_agent = dense;
        app.agent.connection = ConnectionState::Ready;
        app.agent.session_ready = true;
        app.agent.transcript.extend([
            TranscriptItem::Permission(PermissionCard {
                request_id: 1,
                tool_call_id: "permission".into(),
                action: "Edit protected.rs".into(),
                options: vec![PermissionChoice {
                    id: "once".into(),
                    name: "Allow once".into(),
                    kind: "AllowOnce".into(),
                }],
                selected: Some("once".into()),
            }),
            TranscriptItem::Assistant("After permission".into()),
        ]);
        let output = theme::test_context().run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1_000.0, 700.0),
                )),
                ..RawInput::default()
            },
            |ui| app.ui(ui),
        );
        fn text_rect(shape: &Shape, expected: &str) -> Option<Rect> {
            match shape {
                Shape::Text(text) if text.galley.text() == expected => {
                    Some(Rect::from_min_size(text.pos, text.galley.size()))
                }
                Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_rect(shape, expected)),
                _ => None,
            }
        }
        fn card_rect(shape: &Shape, action: Rect) -> Option<Rect> {
            match shape {
                Shape::Rect(rect)
                    if rect.fill == theme::surface().raised
                        && rect.rect.contains(action.center()) =>
                {
                    Some(rect.rect)
                }
                Shape::Vec(shapes) => shapes.iter().find_map(|shape| card_rect(shape, action)),
                _ => None,
            }
        }
        let action = output
            .shapes
            .iter()
            .find_map(|shape| text_rect(&shape.shape, "Edit protected.rs"))
            .expect("permission action");
        let card = output
            .shapes
            .iter()
            .find_map(|shape| card_rect(&shape.shape, action))
            .expect("completed permission card");
        let next = output
            .shapes
            .iter()
            .find_map(|shape| text_rect(&shape.shape, "After permission"))
            .expect("following reply");
        next.top() - card.bottom()
    }

    assert!((gap(false) - gap(true) - 8.0).abs() <= 0.5);
}

#[test]
fn collapsed_dense_work_clusters_keep_their_trailing_gap_when_culled() {
    assert_eq!(
        super::dense_agent_gap_after_item(8.0, true, false, true),
        8.0
    );
    assert_eq!(
        super::dense_agent_gap_after_item(8.0, true, true, true),
        0.0
    );
}

#[test]
fn switching_sessions_remeasures_the_new_transcript_before_culling() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.session_id = Some("first-session".into());
    app.agent
        .transcript
        .extend((0..120).map(|index| TranscriptItem::Assistant(format!("First reply {index}"))));
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(760.0, 700.0));
    let draw = |app: &mut EditorApp, events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |ui| app.ui(ui),
        )
    };

    let _ = draw(&mut app, Vec::new());
    let _ = draw(&mut app, Vec::new());
    assert!(app.agent_transcript_rendered < 60);

    app.agent.session_id = Some("second-session".into());
    app.agent.transcript = (0..120)
        .map(|index| TranscriptItem::Assistant(format!("Second reply {index}")))
        .collect();
    let _ = draw(&mut app, vec![Event::PointerMoved(pos2(700.0, 350.0))]);

    assert_eq!(app.agent_transcript_rendered, 120);
}

#[test]
fn switching_sessions_keeps_latest_reply_visible_on_pointer_frames() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.session_id = Some("first-session".into());
    app.agent
        .transcript
        .extend((0..120).map(|index| TranscriptItem::Assistant(format!("First reply {index}"))));
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(760.0, 700.0));
    let draw = |app: &mut EditorApp, events, time| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                time: Some(time),
                ..RawInput::default()
            },
            |ui| app.ui(ui),
        )
    };

    fn contains_text(shape: &Shape, needle: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(needle),
            Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, needle)),
            _ => false,
        }
    }

    let _ = draw(&mut app, Vec::new(), 0.0);
    let _ = draw(&mut app, Vec::new(), 1.0);
    let _ = draw(
        &mut app,
        vec![
            Event::PointerMoved(pos2(700.0, 350.0)),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, 1_000_000.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
        2.0,
    );
    assert!(!app.agent_follow_transcript);
    let _ = draw(
        &mut app,
        vec![Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: Vec2::ZERO,
            phase: TouchPhase::End,
            modifiers: Modifiers::NONE,
        }],
        2.1,
    );

    app.agent.session_id = Some("second-session".into());
    app.agent.transcript = (0..120)
        .map(|index| TranscriptItem::Assistant(format!("Second reply {index}")))
        .collect();
    app.agent_follow_transcript = true;

    for time in [3.0, 4.0, 5.0] {
        let output = draw(
            &mut app,
            vec![Event::PointerMoved(pos2(700.0, 350.0))],
            time,
        );
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| contains_text(&shape.shape, "Second reply 119")),
            "latest reply vanished on a pointer-only frame"
        );
    }
}

#[test]
fn agent_diff_body_culls_rows_far_outside_the_viewport() {
    let temp = tempfile::tempdir().unwrap();
    let app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let new_text = (0..2000)
        .map(|index| format!("diff row {index} payload"))
        .collect::<Vec<_>>()
        .join("\n");
    let diff = super::build_agent_diff(None, &new_text);
    fn contains_text(shape: &Shape, needle: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(needle),
            Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, needle)),
            _ => false,
        }
    }
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(500.0, 400.0))),
        ..RawInput::default()
    };
    let context = theme::test_context();
    let run = |events: Vec<Event>| {
        let mut frame = input();
        frame.events = events;
        context.run_ui(frame, |ui| {
            super::draw_agent_diff_body(
                ui,
                Id::new("diff_cull"),
                std::path::Path::new("notes.txt"),
                &diff,
                None,
                &new_text,
                &app.highlighter,
                &app.syntaxes,
            );
        })
    };
    let rendered_rows = || {
        context
            .data(|data| data.get_temp::<usize>(Id::new("diff_cull").with("rendered_rows")))
            .expect("diff body records its rendered row count")
    };
    // Row height is learned from the first rendered row, so even the
    // first frame only lays out the visible band.
    let first = run(Vec::new());
    assert!(
        first
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "diff row 0 payload")),
        "the first rows start visible"
    );
    let output = run(Vec::new());
    assert!(
        rendered_rows() < 100,
        "rows far below the viewport must be culled into spacers, \
             but {} of 2000 were laid out",
        rendered_rows()
    );
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "diff row 0 payload")),
        "rows inside the viewport keep rendering"
    );
    // Wheel to the bottom: culled rows must rematerialize exactly where
    // their spacers put them.
    let mut bottom_visible = false;
    for _ in 0..12 {
        let output = run(vec![
            Event::PointerMoved(pos2(250.0, 200.0)),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -1_000_000.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ]);
        if output
            .shapes
            .iter()
            .any(|shape| contains_text(&shape.shape, "diff row 1999 payload"))
        {
            bottom_visible = true;
            assert!(
                rendered_rows() < 100,
                "culling must keep working at the bottom of the diff"
            );
            break;
        }
    }
    assert!(
        bottom_visible,
        "scrolling down must bring culled rows back into view"
    );
}

#[test]
fn find_navigation_scrolls_the_matching_diff_row_into_view() {
    use crate::agent::controller::ToolOutput;

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    // One tool card whose (search-expanded) diff hides the only match at
    // row 250, followed by enough replies that the view rests far below.
    let new_text = (0..300)
        .map(|index| {
            if index == 250 {
                "the needle line".to_owned()
            } else {
                format!("filler line {index}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    app.agent
        .transcript
        .push_back(TranscriptItem::Tool(ToolActivity {
            id: "edit".into(),
            title: Some("Edit big.txt".into()),
            status: Some("Completed".into()),
            kind: Some("Edit".into()),
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: None,
                content: vec![ToolOutput::Diff {
                    path: "big.txt".into(),
                    old_text: None,
                    new_text: new_text.clone().into(),
                }],
                output: None,
            }),
        }));
    for index in 0..40 {
        app.agent
            .transcript
            .push_back(TranscriptItem::Assistant(format!("reply {index}")));
    }
    fn text_pos(shape: &Shape, needle: &str) -> Option<Pos2> {
        match shape {
            Shape::Text(text) if text.galley.text().contains(needle) => Some(text.pos),
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_pos(shape, needle)),
            _ => None,
        }
    }
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(760.0, 700.0))),
        ..RawInput::default()
    };
    let context = theme::test_context();
    let _ = context.run_ui(input(), |root| app.ui(root));
    // Navigate to the match the way the find bar would.
    app.agent_find.query = "needle".to_owned();
    app.agent_find.matches =
        super::agent_search_matches(&app.agent.transcript, &app.agent.changed_paths, "needle");
    assert_eq!(app.agent_find.matches, vec![0], "the diff holds the match");
    app.agent_find.selected = 0;
    app.agent_find.scroll_to_match = true;
    app.agent_follow_transcript = false;
    let mut match_visible = false;
    for _ in 0..12 {
        let output = context.run_ui(input(), |root| app.ui(root));
        if let Some(pos) = output
            .shapes
            .iter()
            .find_map(|shape| text_pos(&shape.shape, "the needle line"))
            && (0.0..700.0).contains(&pos.y)
        {
            match_visible = true;
            break;
        }
    }
    assert!(
        match_visible,
        "navigating to a match inside a diff must scroll its row into view"
    );
    assert!(
        !app.agent_find.scroll_to_match,
        "the pending scroll must be consumed once the row is in view"
    );
}

#[test]
fn detail_less_tool_cards_are_not_expandable() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::Tool(ToolActivity {
            id: "edit".into(),
            title: Some("Edit /tmp/validate.py".into()),
            status: Some("Completed".into()),
            kind: Some("Edit".into()),
            paths: vec!["/tmp/validate.py".into()],
            detail: None,
        }));

    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(760.0, 700.0))),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let title = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.text() == "/tmp/validate.py" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .expect("tool title");
    let card = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Rect(rect)
                if rect.fill == theme::surface().raised && rect.rect.contains(title.center()) =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .expect("tool card");
    let has_disclosure = output.shapes.iter().any(|shape| match &shape.shape {
        Shape::Path(path) if !path.closed && path.points.len() == 3 => {
            let center = path.visual_bounding_rect().center();
            card.contains(center) && center.x < title.left()
        }
        _ => false,
    });

    assert!(!has_disclosure, "an empty tool card offered a disclosure");
}

#[test]
fn tool_card_disclosure_is_optically_centered_and_radius_is_compact() {
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(400.0, 100.0))),
            ..RawInput::default()
        },
        |ui| {
            agent_collapsing_header(
                ui,
                "tool-card",
                "python3 -c",
                Some("Completed"),
                None,
                340.0,
                None,
                true,
                false,
                |_| {},
            );
        },
    );
    let mut title_center = None;
    let mut chevron_bounds = Rect::NOTHING;
    let mut chevron_paths = 0;
    let mut filled_triangle = false;
    let mut card_radius = None;
    let mut done_text = false;
    let mut completion_circle = false;
    let mut completion_check = false;
    let completion_color = theme::ink(theme::semantic().success);
    for clipped in output.shapes {
        match clipped.shape {
            Shape::Text(text) if text.galley.text() == "python3 -c" => {
                title_center = Some(text.pos.y + text.galley.size().y * 0.5);
            }
            Shape::Text(text) if text.galley.text() == "Done" => done_text = true,
            Shape::Circle(circle)
                if circle.fill == Color32::TRANSPARENT
                    && circle.stroke.color == completion_color =>
            {
                completion_circle = true;
            }
            Shape::Path(path)
                if !path.closed
                    && path.stroke.color == egui::epaint::ColorMode::Solid(completion_color) =>
            {
                completion_check = true;
            }
            Shape::Path(path) if !path.closed && path.points.len() == 3 => {
                chevron_bounds = chevron_bounds.union(path.visual_bounding_rect());
                chevron_paths += 1;
            }
            Shape::Path(path)
                if path.closed
                    && path.points.len() == 3
                    && path.visual_bounding_rect().width() < 20.0 =>
            {
                filled_triangle = true;
            }
            Shape::Rect(rect)
                if rect.rect.width() > 300.0
                    && rect.rect.height() > 30.0
                    && rect.fill != Color32::TRANSPARENT =>
            {
                card_radius = Some(rect.corner_radius.nw);
            }
            _ => {}
        }
    }

    let title_center = title_center.unwrap();
    assert!(
        (chevron_bounds.center().y - title_center).abs() < 1.5,
        "chevron={chevron_bounds:?}, title={title_center}"
    );
    assert_eq!(chevron_paths, 1, "the disclosure is one mitred path");
    assert!(!filled_triangle);
    assert!(card_radius.unwrap() <= theme::radius::CONTROL);
    assert!(!done_text);
    assert!(completion_circle);
    assert!(completion_check);
}

#[test]
fn overflowing_tool_path_marquees_on_hover_without_moving_the_action() {
    let context = theme::test_context();
    let path = "/Users/example/Documents/project/src/a-very-long-file-name.rs";
    let draw = |time| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(300.0, 100.0))),
                events: vec![Event::PointerMoved(pos2(120.0, 20.0))],
                time: Some(time),
                ..RawInput::default()
            },
            |ui| {
                agent_collapsing_header(
                    ui,
                    "marquee-tool",
                    &format!("Edit {path}"),
                    Some("Completed"),
                    None,
                    260.0,
                    None,
                    false,
                    false,
                    |_| {},
                );
            },
        )
    };
    fn positions(shape: &Shape, path: &str) -> (Option<f32>, Option<f32>) {
        match shape {
            Shape::Text(text) if text.galley.text().trim() == "Edit" => (Some(text.pos.x), None),
            Shape::Text(text) if text.galley.text() == path => (None, Some(text.pos.x)),
            Shape::Vec(shapes) => shapes.iter().fold((None, None), |found, shape| {
                let next = positions(shape, path);
                (found.0.or(next.0), found.1.or(next.1))
            }),
            _ => (None, None),
        }
    }
    let locate = |output: &egui::FullOutput| {
        output.shapes.iter().fold((None, None), |found, shape| {
            let next = positions(&shape.shape, path);
            (found.0.or(next.0), found.1.or(next.1))
        })
    };
    let _ = draw(0.0);
    let before = locate(&draw(0.1));
    let after = locate(&draw(2.1));
    let (before_action, before_path) = (before.0.unwrap(), before.1.unwrap());
    let (after_action, after_path) = (after.0.unwrap(), after.1.unwrap());

    assert!(
        (before_action - after_action).abs() < 0.1 && after_path < before_path - 5.0,
        "action {before_action}->{after_action}, path {before_path}->{after_path}"
    );
}

#[test]
fn agent_assistant_responses_render_compact_markdown() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.transcript.push_back(TranscriptItem::Assistant(
            "# Result\n\n- **Done**\n- Run `cargo-test-with-an-unbroken-argument-that-is-much-wider-than-the-agent-sidebar`."
                .into(),
        ));
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(760.0, 700.0))),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn markdown_metrics(shape: &Shape) -> Option<(Rect, f32)> {
        match shape {
            Shape::Text(text)
                if text.galley.text()
                    == "Result\n\n  • Done\n  • Run cargo-test-with-an-unbroken-argument-that-is-much-wider-than-the-agent-sidebar." =>
            {
                Some((
                    Rect::from_min_size(text.pos, text.galley.size()),
                    text.galley
                        .job
                        .sections
                        .iter()
                        .map(|section| section.format.font_id.size)
                        .fold(0.0, f32::max),
                ))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(markdown_metrics),
            _ => None,
        }
    }
    let (rect, max_font_size, clip) = output
        .shapes
        .iter()
        .find_map(|shape| {
            markdown_metrics(&shape.shape)
                .map(|(rect, max_font_size)| (rect, max_font_size, shape.clip_rect))
        })
        .expect("rendered Markdown response");

    assert!((19.5..=20.0).contains(&max_font_size));
    assert!(rect.right() <= clip.right());
    let cursor_metrics = output
        .shapes
        .iter()
        .find_map(|clipped| match &clipped.shape {
            Shape::Text(text) if text.galley.text() == "Cursor" => Some((
                Rect::from_min_size(text.pos, text.galley.size()),
                text.galley.job.sections[0].format.font_id.size,
                text.galley.job.sections[0].format.color,
            )),
            _ => None,
        });
    let cursor_paths = output
        .shapes
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            Shape::Path(path)
                if path.closed
                    && path.fill == theme::text().primary
                    && (3..=4).contains(&path.points.len()) =>
            {
                Some(path.visual_bounding_rect())
            }
            _ => None,
        });
    let (cursor_facets, cursor_mark) = cursor_paths
        .fold((0, Rect::NOTHING), |(count, bounds), path| {
            (count + 1, bounds.union(path))
        });
    let (cursor_text, cursor_size, cursor_color) = cursor_metrics.expect("Cursor identity");
    assert_eq!(cursor_color, theme::text().primary);
    assert!(cursor_size >= 13.0);
    assert_eq!(cursor_facets, 3);
    assert!(cursor_mark.height() >= 15.5);
    assert!(rect.top() - cursor_text.union(cursor_mark).bottom() >= 5.0);
}

#[test]
fn agent_permission_and_metadata_labels_explain_what_they_show() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.transcript.extend([
        TranscriptItem::Tool(ToolActivity {
            id: "grep".into(),
            title: Some("grep".into()),
            status: Some("InProgress".into()),
            kind: None,
            paths: Vec::new(),
            detail: Some(ToolDetail {
                input: None,
                content: Vec::new(),
                output: Some("{\"totalMatches\":192,\"truncated\":true}".into()),
            }),
        }),
        TranscriptItem::Permission(PermissionCard {
            request_id: 1,
            tool_call_id: "write".into(),
            action: "Edit .github/workflows/release.yml".into(),
            options: vec![
                PermissionChoice {
                    id: "once".into(),
                    name: "Allow once".into(),
                    kind: "AllowOnce".into(),
                },
                PermissionChoice {
                    id: "always".into(),
                    name: "Allow always".into(),
                    kind: "AllowAlways".into(),
                },
                PermissionChoice {
                    id: "reject".into(),
                    name: "Reject".into(),
                    kind: "RejectOnce".into(),
                },
            ],
            selected: None,
        }),
    ]);
    let context = theme::test_context();
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            time: Some(0.0),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }
    let has = |output: &egui::FullOutput, expected| {
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, expected))
    };
    fn permission_width(shape: &Shape) -> Option<f32> {
        match shape {
            Shape::Rect(rect) if rect.fill == theme::callout(theme::semantic().warning).fill => {
                Some(rect.rect.width())
            }
            Shape::Vec(shapes) => shapes.iter().find_map(permission_width),
            _ => None,
        }
    }

    assert!(!has(&output, "Summary"));
    assert!(!has(&output, "Input"));
    assert!(has(&output, "Always allow"));
    assert!(!has(&output, "Always allow globally"));
    assert!(!has(
        &output,
        "Cursor saves global choices in ~/.cursor/cli-config.json."
    ));
    let width = output
        .shapes
        .iter()
        .find_map(|shape| permission_width(&shape.shape))
        .expect("permission card surface");
    assert!(width <= 300.0, "permission card was {width}px wide");

    assert!(app.agent.decide_permission(1, "always"));
    let resolved = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            time: Some(1.0),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    assert!(has(&resolved, "Allowed globally"));
    assert!(!has(&resolved, "Always allow globally"));
}

#[test]
fn agent_composer_grows_with_wrapped_text_and_stops_at_its_cap() {
    assert_eq!(agent_composer_height(28.0, 14.0, 700.0), 108.0);
    assert_eq!(agent_composer_height(84.0, 14.0, 700.0), 150.0);
    assert_eq!(agent_composer_height(1_400.0, 14.0, 700.0), 240.0);
}

#[test]
fn dropping_an_image_over_the_composer_shows_a_square_thumbnail() {
    let temp = tempfile::tempdir().unwrap();
    let image = temp.path().join("reference.png");
    fs::write(&image, include_bytes!("../../assets/icons/editur.png")).unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let pointer = pos2(800.0, 650.0);
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            events: vec![Event::PointerMoved(pointer)],
            hovered_files: vec![HoveredFile {
                path: Some(image.clone()),
                ..HoveredFile::default()
            }],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            events: vec![Event::PointerMoved(pointer)],
            dropped_files: vec![DroppedFile {
                path: Some(image),
                ..DroppedFile::default()
            }],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn contains_filename(shape: &Shape) -> bool {
        match shape {
            Shape::Text(text) => {
                text.galley.text().contains("reference.png")
                    && text.pos.x > 560.0
                    && text.pos.y > 520.0
            }
            Shape::Vec(shapes) => shapes.iter().any(contains_filename),
            _ => false,
        }
    }
    fn contains_thumbnail(shape: &Shape) -> bool {
        fn is_thumbnail(bounds: Rect) -> bool {
            bounds.left() > 560.0
                && bounds.top() > 520.0
                && (40.0..=56.0).contains(&bounds.width())
                && (40.0..=56.0).contains(&bounds.height())
        }
        match shape {
            Shape::Rect(rect) if rect.brush.is_some() => is_thumbnail(rect.rect),
            Shape::Mesh(mesh) if !mesh.vertices.is_empty() => {
                let bounds = mesh.vertices.iter().fold(Rect::NOTHING, |bounds, vertex| {
                    bounds.union(Rect::from_min_max(vertex.pos, vertex.pos))
                });
                is_thumbnail(bounds)
            }
            Shape::Vec(shapes) => shapes.iter().any(contains_thumbnail),
            _ => false,
        }
    }

    assert!(
        !output
            .shapes
            .iter()
            .any(|shape| contains_filename(&shape.shape)),
        "image attachments should not render a filename badge"
    );
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| contains_thumbnail(&shape.shape)),
        "the dropped image should render as a small thumbnail"
    );
}

#[test]
fn embedded_session_image_bytes_decode_to_a_thumbnail() {
    let context = theme::test_context();

    let preview = load_agent_image_preview_bytes(
        &context,
        "detected-session-image",
        include_bytes!("../../assets/icons/editur.png"),
    );

    assert!(preview.is_some());
}

#[test]
fn prompt_image_square_uses_a_center_crop() {
    let landscape = super::agent_image_cover_uv(Vec2::new(200.0, 100.0), Vec2::splat(72.0));
    let portrait = super::agent_image_cover_uv(Vec2::new(100.0, 200.0), Vec2::splat(72.0));

    assert_eq!(
        landscape,
        Rect::from_min_max(pos2(0.25, 0.0), pos2(0.75, 1.0))
    );
    assert_eq!(
        portrait,
        Rect::from_min_max(pos2(0.0, 0.25), pos2(1.0, 0.75))
    );
}

#[test]
fn embedded_session_image_is_painted_in_the_transcript() {
    use crate::agent::controller::DisplayContent;

    let context = theme::test_context();
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(800.0, 700.0))),
            ..RawInput::default()
        },
        |ui| {
            super::draw_agent_content(
                ui,
                &DisplayContent::Image {
                    mime_type: "image/png".into(),
                    uri: None,
                    encoded_bytes: 0,
                    data: Some(std::sync::Arc::from(
                        include_bytes!("../../assets/icons/editur.png").as_slice(),
                    )),
                },
                None,
            );
        },
    );

    let bounds = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                Some(mesh.vertices.iter().fold(Rect::NOTHING, |bounds, vertex| {
                    bounds.union(Rect::from_min_max(vertex.pos, vertex.pos))
                }))
            }
            Shape::Rect(rect) if rect.brush.is_some() => Some(rect.rect),
            _ => None,
        })
        .expect("embedded image texture");

    assert!(bounds.width() <= 240.0 && bounds.height() <= 180.0);
}

#[test]
fn user_prompt_image_is_a_square_inside_the_prompt_and_opens_a_lightbox() {
    use crate::agent::controller::{ContentRole, DisplayContent};
    use crate::agent::state::TranscriptItem;

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent
        .transcript
        .push_back(TranscriptItem::User("Review this image".into()));
    app.agent.transcript.push_back(TranscriptItem::Content {
        role: ContentRole::User,
        content: DisplayContent::Image {
            mime_type: "image/png".into(),
            uri: None,
            encoded_bytes: 0,
            data: Some(std::sync::Arc::from(
                include_bytes!("../../assets/icons/editur.png").as_slice(),
            )),
        },
    });
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    {
        let mut draw = |events| {
            context.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..RawInput::default()
                },
                |root| app.ui(root),
            )
        };
        fn image_bounds(output: &egui::FullOutput) -> Vec<Rect> {
            fn collect(shape: &Shape, bounds: &mut Vec<Rect>) {
                match shape {
                    Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                        bounds.push(mesh.vertices.iter().fold(Rect::NOTHING, |bounds, vertex| {
                            bounds.union(Rect::from_min_max(vertex.pos, vertex.pos))
                        }));
                    }
                    Shape::Rect(rect) if rect.brush.is_some() => bounds.push(rect.rect),
                    Shape::Vec(shapes) => {
                        for shape in shapes {
                            collect(shape, bounds);
                        }
                    }
                    _ => {}
                }
            }
            let mut bounds = Vec::new();
            for shape in &output.shapes {
                collect(&shape.shape, &mut bounds);
            }
            bounds
        }

        fn text_bounds(output: &egui::FullOutput, needle: &str) -> Rect {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    Shape::Text(text) if text.galley.text() == needle => {
                        Some(Rect::from_min_size(text.pos, text.galley.size()))
                    }
                    _ => None,
                })
                .expect("prompt text")
        }

        fn prompt_surfaces(output: &egui::FullOutput) -> Vec<Rect> {
            fn collect(shape: &Shape, bounds: &mut Vec<Rect>) {
                match shape {
                    Shape::Rect(rect) if rect.fill == theme::surface().input => {
                        bounds.push(rect.rect);
                    }
                    Shape::Vec(shapes) => {
                        for shape in shapes {
                            collect(shape, bounds);
                        }
                    }
                    _ => {}
                }
            }
            let mut bounds = Vec::new();
            for shape in &output.shapes {
                collect(&shape.shape, &mut bounds);
            }
            bounds
        }

        let initial = draw(Vec::new());
        let thumbnail = image_bounds(&initial)
            .into_iter()
            .find(|rect| rect.width() <= 96.0 && rect.height() <= 96.0)
            .expect("prompt image thumbnail");
        assert!((thumbnail.width() - thumbnail.height()).abs() <= 0.5);
        let prompt_text = text_bounds(&initial, "Review this image");
        assert!(prompt_surfaces(&initial).iter().any(|rect| {
            rect.contains(prompt_text.center()) && rect.contains(thumbnail.center())
        }));
        let _ = draw(vec![
            Event::PointerMoved(thumbnail.center()),
            Event::PointerButton {
                pos: thumbnail.center(),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        let _ = draw(vec![Event::PointerButton {
            pos: thumbnail.center(),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        let lightbox = draw(Vec::new());

        context
            .read_response(Id::new("agent_image_lightbox_close"))
            .expect("lightbox close button");
        let lightbox_images = image_bounds(&lightbox);
        assert!(
            lightbox_images
                .iter()
                .any(|rect| rect.width() > 240.0 || rect.height() > 180.0),
            "lightbox should render a larger image, got {lightbox_images:?}"
        );

        let _ = draw(vec![Event::Key {
            key: Key::Escape,
            physical_key: Some(Key::Escape),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }]);
    }
    assert!(app.agent_image_lightbox.is_none());
}

#[test]
fn agent_originated_image_starts_collapsed() {
    use crate::agent::controller::{ContentRole, DisplayContent};
    use crate::agent::state::TranscriptItem;

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.transcript.push_back(TranscriptItem::Content {
        role: ContentRole::Assistant,
        content: DisplayContent::Image {
            mime_type: "image/png".into(),
            uri: None,
            encoded_bytes: 0,
            data: Some(std::sync::Arc::from(
                include_bytes!("../../assets/icons/editur.png").as_slice(),
            )),
        },
    });
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    {
        let mut draw = |events| {
            context.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    events,
                    ..RawInput::default()
                },
                |root| app.ui(root),
            )
        };
        let output = draw(Vec::new());

        assert!(!output.shapes.iter().any(|shape| match &shape.shape {
            Shape::Mesh(mesh) => mesh.texture_id != egui::TextureId::default(),
            Shape::Rect(rect) => rect.brush.is_some(),
            _ => false,
        }));
        let header = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Text(text) if text.galley.text() == "Image" => {
                    Some(Rect::from_min_size(text.pos, text.galley.size()))
                }
                _ => None,
            })
            .expect("collapsed image disclosure");
        let _ = draw(vec![
            Event::PointerMoved(header.center()),
            Event::PointerButton {
                pos: header.center(),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        let expanded = draw(vec![Event::PointerButton {
            pos: header.center(),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
        let thumbnail = expanded
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Mesh(mesh) if mesh.texture_id != egui::TextureId::default() => {
                    Some(mesh.vertices.iter().fold(Rect::NOTHING, |bounds, vertex| {
                        bounds.union(Rect::from_min_max(vertex.pos, vertex.pos))
                    }))
                }
                Shape::Rect(rect) if rect.brush.is_some() => Some(rect.rect),
                _ => None,
            })
            .expect("expanded image thumbnail");
        let _ = draw(vec![
            Event::PointerMoved(thumbnail.center()),
            Event::PointerButton {
                pos: thumbnail.center(),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        let _ = draw(vec![Event::PointerButton {
            pos: thumbnail.center(),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }]);
    }
    assert!(app.agent_image_lightbox.is_some());
}

#[test]
fn image_picker_button_is_visible_in_both_agent_composers() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent_sidebar = true;
    let context = theme::test_context();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1000.0, 700.0),
        )),
        ..RawInput::default()
    };
    let sidebar = context.run_ui(input(), |root| app.ui(root));
    app.agent_sidebar = false;
    app.agentic_mode = true;
    let agentic = context.run_ui(input(), |root| app.ui(root));
    // The attach control is the icon module's plus: two crossed strokes of
    // equal length, which no other glyph in the composer draws.
    fn has_plus(shape: &Shape) -> bool {
        match shape {
            Shape::Path(path) => {
                path.points.len() == 2
                    && (path.points[0].x - path.points[1].x).abs() < 0.1
                    && (path.points[0].y - path.points[1].y).abs() > 8.0
            }
            Shape::Vec(shapes) => shapes.iter().any(has_plus),
            _ => false,
        }
    }

    assert!(
        [sidebar, agentic]
            .iter()
            .all(|output| output.shapes.iter().any(|shape| has_plus(&shape.shape)))
    );
}

#[test]
fn custom_file_picker_loads_one_directory_at_a_time_and_preserves_selection() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let child = root.join("child");
    fs::create_dir(&child).unwrap();
    let root_file = root.join("root.txt");
    let nested_file = child.join("nested.txt");
    fs::write(&root_file, "root").unwrap();
    fs::write(&nested_file, "nested").unwrap();

    let mut picker = AgentFilePicker::open(root.clone()).unwrap();
    assert_eq!(
        picker
            .entries
            .iter()
            .map(|entry| entry.path.clone())
            .collect::<Vec<_>>(),
        [child.clone(), root_file.clone()]
    );
    picker.toggle(root_file.clone());
    picker.navigate(child).unwrap();

    assert_eq!(picker.entries[0].path, nested_file);
    assert!(picker.selected.contains(&root_file));
}

#[test]
fn custom_file_picker_filters_the_current_folder_case_insensitively() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("docs")).unwrap();
    fs::write(root.join("Reference.PNG"), "image").unwrap();
    fs::write(root.join("notes.txt"), "notes").unwrap();
    let mut picker = AgentFilePicker::open(root).unwrap();

    picker.query = "png".into();

    assert_eq!(
        picker
            .visible_entries()
            .iter()
            .map(|entry| entry.name.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["Reference.PNG"]
    );
}

#[test]
fn custom_file_picker_renders_project_files_without_a_native_dialog() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::write(root.join("reference.png"), "image").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.open_agent_file_picker();
    let context = theme::test_context();
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }

    assert!(
        context
            .read_response(Id::new("agent_file_picker"))
            .is_some()
    );
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "reference.png"))
    );
}

#[test]
fn custom_file_picker_stays_fixed_while_the_pointer_moves() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("folder")).unwrap();
    fs::write(root.join("reference.png"), "image").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.open_agent_file_picker();
    let context = theme::test_context();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1000.0, 700.0),
        )),
        ..RawInput::default()
    };
    // The modal's anchored area learns its size over the first frames, so
    // it only settles onto the screen center from the third frame on.
    for _ in 0..2 {
        let _ = context.run_ui(input(), |root| app.ui(root));
    }
    let mut positions = Vec::new();

    for pointer in [pos2(500.0, 350.0), pos2(650.0, 300.0), pos2(400.0, 240.0)] {
        let mut input = input();
        input.events.push(Event::PointerMoved(pointer));
        let _ = context.run_ui(input, |root| app.ui(root));
        positions.push(
            context
                .read_response(Id::new("agent_file_picker"))
                .unwrap()
                .rect,
        );
    }

    assert!(
        positions.windows(2).all(|pair| pair[0] == pair[1]),
        "picker moved between pointer repaints: {positions:?}"
    );
    assert_eq!(
        positions[0].center(),
        pos2(500.0, 350.0),
        "the picker centers on the screen: {positions:?}"
    );
    assert!(
        (positions[0].width() - 760.0).abs() <= 4.0 && (positions[0].height() - 560.0).abs() <= 4.0,
        "the picker keeps its fixed size: {positions:?}"
    );
}

#[test]
fn the_project_folder_picker_lists_only_directories() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join("crates")).unwrap();
    fs::write(root.join("notes.txt"), "notes").unwrap();

    let picker = AgentFilePicker::open_directories(root).unwrap();

    assert_eq!(
        picker
            .visible_entries()
            .iter()
            .map(|entry| entry.name.to_string_lossy().into_owned())
            .collect::<Vec<_>>(),
        ["crates"]
    );
}

#[test]
fn hidden_entries_stay_out_of_the_picker_until_the_toggle() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    fs::create_dir(root.join(".config")).unwrap();
    fs::create_dir(root.join("src")).unwrap();
    let mut picker = AgentFilePicker::open_directories(root).unwrap();

    let names = |picker: &AgentFilePicker| {
        picker
            .visible_entries()
            .iter()
            .map(|entry| entry.name.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(names(&picker), ["src"]);
    assert_eq!(picker.hidden_entries(), 1);

    picker.show_hidden = true;

    assert_eq!(names(&picker), [".config", "src"]);
    assert_eq!(picker.hidden_entries(), 0);
}

#[test]
fn the_picker_walks_its_own_history_with_back_and_forward() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let child = root.join("child");
    fs::create_dir(&child).unwrap();
    let mut picker = AgentFilePicker::open_directories(root.clone()).unwrap();
    assert!(!picker.can_go_back() && !picker.can_go_forward());

    picker.navigate(child.clone()).unwrap();
    assert!(picker.can_go_back());

    picker.go_back();
    assert_eq!(picker.directory, root);
    assert!(picker.can_go_forward());

    picker.go_forward();
    assert_eq!(picker.directory, child);
    assert!(picker.can_go_back() && !picker.can_go_forward());
}

#[test]
fn navigating_somewhere_new_after_going_back_drops_the_forward_trail() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = root.join("first");
    let second = root.join("second");
    fs::create_dir(&first).unwrap();
    fs::create_dir(&second).unwrap();
    let mut picker = AgentFilePicker::open_directories(root).unwrap();

    picker.navigate(first).unwrap();
    picker.go_back();
    picker.navigate(second.clone()).unwrap();

    assert_eq!(picker.directory, second);
    assert!(!picker.can_go_forward(), "a new branch replaces the future");
}

#[test]
fn reloading_reads_the_directory_again_without_touching_history_or_the_query() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let child = root.join("child");
    fs::create_dir(&child).unwrap();
    let mut picker = AgentFilePicker::open_directories(root.clone()).unwrap();
    picker.navigate(child.clone()).unwrap();
    picker.query = "late".into();
    fs::create_dir(child.join("latecomer")).unwrap();

    picker.reload();

    assert_eq!(picker.entries[0].path, child.join("latecomer"));
    assert_eq!(picker.query, "late", "a refresh must not clear the filter");
    assert!(picker.can_go_back(), "a refresh is not a navigation");
    assert!(!picker.can_go_forward());
}

#[test]
fn navigating_to_the_current_directory_does_not_stack_history() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let mut picker = AgentFilePicker::open_directories(root.clone()).unwrap();

    picker.navigate(root).unwrap();

    assert!(
        !picker.can_go_back(),
        "re-entering the same folder is a reload"
    );
}

#[test]
#[cfg(unix)]
fn breadcrumbs_keep_the_trail_tail_and_expose_the_overflow_ancestor() {
    let deep = PathBuf::from("/Users/example/dev/editur/src");

    let (overflow, segments) = picker_breadcrumb_segments(&deep, 3);

    assert_eq!(overflow.as_deref(), Some(Path::new("/Users/example")));
    assert_eq!(
        segments,
        [
            ("dev".to_owned(), PathBuf::from("/Users/example/dev")),
            (
                "editur".to_owned(),
                PathBuf::from("/Users/example/dev/editur")
            ),
            (
                "src".to_owned(),
                PathBuf::from("/Users/example/dev/editur/src")
            ),
        ]
    );

    let (overflow, segments) = picker_breadcrumb_segments(Path::new("/"), 3);
    assert_eq!(overflow, None);
    assert_eq!(segments, [("/".to_owned(), PathBuf::from("/"))]);
}

#[test]
fn arrow_keys_walk_the_list_and_enter_descends_into_the_highlighted_folder() {
    let current = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let destination_root = destination.path().canonicalize().unwrap();
    fs::create_dir(destination_root.join("alpha")).unwrap();
    fs::create_dir(destination_root.join("beta")).unwrap();
    let original_root = current.path().canonicalize().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: original_root.clone(),
        file: None,
        create: false,
    })
    .unwrap();
    app.project_folder_picker =
        Some(AgentFilePicker::open_directories(destination_root.clone()).unwrap());
    let context = theme::test_context();
    let mut press = |key| {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events: vec![Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    };

    press(Key::ArrowDown);
    press(Key::Enter);

    let picker = app.project_folder_picker.as_ref().unwrap();
    assert_eq!(
        picker.directory,
        destination_root.join("alpha"),
        "Enter on a highlighted folder descends instead of confirming"
    );
    assert_eq!(app.tree.root, original_root, "the project must not switch");
}

#[test]
fn command_brackets_walk_the_picker_history_and_the_toolbar_offers_the_buttons() {
    let current = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let destination_root = destination.path().canonicalize().unwrap();
    fs::create_dir(destination_root.join("alpha")).unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: current.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.project_folder_picker =
        Some(AgentFilePicker::open_directories(destination_root.clone()).unwrap());
    let context = theme::test_context();
    fn press(context: &egui::Context, app: &mut EditorApp, key: Key, modifiers: Modifiers) {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                modifiers,
                events: vec![Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers,
                }],
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    }

    press(&context, &mut app, Key::ArrowDown, Modifiers::NONE);
    press(&context, &mut app, Key::Enter, Modifiers::NONE);
    assert_eq!(
        app.project_folder_picker.as_ref().unwrap().directory,
        destination_root.join("alpha")
    );

    press(&context, &mut app, Key::OpenBracket, Modifiers::COMMAND);
    assert_eq!(
        app.project_folder_picker.as_ref().unwrap().directory,
        destination_root,
        "Cmd+[ steps back through the picker's history"
    );

    press(&context, &mut app, Key::CloseBracket, Modifiers::COMMAND);
    assert_eq!(
        app.project_folder_picker.as_ref().unwrap().directory,
        destination_root.join("alpha"),
        "Cmd+] retraces the step Back undid"
    );

    for id in ["picker_nav_back", "picker_nav_forward", "picker_nav_up"] {
        assert!(
            context.read_response(Id::new(id)).is_some(),
            "the toolbar offers {id}"
        );
    }
}

#[test]
fn the_project_picker_offers_recent_projects_in_the_rail() {
    let current = tempfile::tempdir().unwrap();
    let recent = tempfile::tempdir().unwrap();
    let recent_root = recent.path().canonicalize().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: current.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.recent_projects = vec![recent_root.clone()];

    app.add_project_via_dialog();

    assert_eq!(
        app.project_folder_picker.as_ref().unwrap().recent,
        std::slice::from_ref(&recent_root)
    );
    let context = theme::test_context();
    let mut frame = || {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    // The modal's anchored area settles over its first frames; widgets
    // outside the estimated rect are culled until then.
    frame();
    frame();
    let output = frame();
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }
    let recent_name = recent_root.file_name().unwrap().to_string_lossy();
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "RECENT")),
        "the rail labels its recent section"
    );
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, recent_name.as_ref())),
        "the recent project appears by name"
    );
}

#[test]
fn adding_a_project_opens_the_custom_folder_picker_rather_than_a_native_dialog() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();

    app.add_project_via_dialog();

    assert!(app.project_folder_picker.is_some());
    let context = theme::test_context();
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    assert!(
        context
            .read_response(Id::new("project_folder_picker"))
            .is_some(),
        "the folder picker dialog is not on screen"
    );
}

#[test]
fn opening_a_folder_from_the_picker_switches_the_project() {
    let current = tempfile::tempdir().unwrap();
    let destination = tempfile::tempdir().unwrap();
    let destination_root = destination.path().canonicalize().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: current.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.project_folder_picker =
        Some(AgentFilePicker::open_directories(destination_root.clone()).unwrap());

    let _ = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            events: vec![Event::Key {
                key: Key::Enter,
                physical_key: Some(Key::Enter),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(app.project_folder_picker.is_none());
    assert_eq!(app.tree.root, destination_root);
}

#[test]
fn agent_composer_scrolls_after_reaching_its_height_cap() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.prompt = (0..80)
        .map(|line| format!("composer overflow line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    let prompt = app.agent.prompt.clone();
    let context = theme::test_context();
    let mut draw = |events, time| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                time: Some(time),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    let text_metrics = |output: &egui::FullOutput| {
        fn find(shape: &Shape, prompt: &str) -> Option<(f32, f32)> {
            match shape {
                Shape::Text(text) if text.galley.text() == prompt => {
                    Some((text.pos.y, text.galley.size().y))
                }
                Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, prompt)),
                _ => None,
            }
        }
        output.shapes.iter().find_map(|clipped| {
            find(&clipped.shape, &prompt).map(|(y, height)| (y, height, clipped.clip_rect.height()))
        })
    };

    let before = draw(Vec::new(), 0.0);
    let (before_y, text_height, clip_height) = text_metrics(&before).unwrap();
    assert!(text_height > clip_height);
    let _ = draw(
        vec![
            Event::PointerMoved(pos2(820.0, 560.0)),
            Event::MouseWheel {
                unit: MouseWheelUnit::Line,
                delta: Vec2::new(0.0, -8.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
        1.0,
    );
    let after = draw(Vec::new(), 2.0);
    let (after_y, _, _) = text_metrics(&after).unwrap();

    assert!(after_y < before_y);
}

#[test]
fn composer_enter_submits_and_shift_enter_inserts_a_newline() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("current.rs");
    fs::write(&file, "fn main() {}\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file),
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.prompt = "ship it".into();
    app.tabs[app.active_tab.unwrap()].buffer.mark_changed();
    let context = theme::test_context();
    fn draw(
        context: &egui::Context,
        app: &mut EditorApp,
        events: Vec<Event>,
        modifiers: Modifiers,
    ) {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                modifiers,
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    }

    draw(&context, &mut app, Vec::new(), Modifiers::NONE);
    context.memory_mut(|memory| memory.request_focus(Id::new("agent_prompt")));
    draw(
        &context,
        &mut app,
        vec![Event::Key {
            key: Key::Enter,
            physical_key: Some(Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::SHIFT,
        }],
        Modifiers::SHIFT,
    );
    assert!(!app.pending_agent_prompt);
    assert_eq!(app.agent.prompt, "ship it\n");

    app.agent.prompt = "ship it".into();
    draw(&context, &mut app, Vec::new(), Modifiers::NONE);
    context.memory_mut(|memory| memory.request_focus(Id::new("agent_prompt")));
    draw(
        &context,
        &mut app,
        vec![Event::Key {
            key: Key::Enter,
            physical_key: Some(Key::Enter),
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        }],
        Modifiers::NONE,
    );

    assert!(app.pending_agent_prompt);
    assert_eq!(app.agent.prompt, "ship it");
}

#[test]
fn composer_arrows_at_the_start_cycle_prompt_history_and_restore_the_draft() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = root.join("a.rs");
    let second = root.join("b.rs");
    fs::write(&first, "one\n").unwrap();
    fs::write(&second, "two\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: None,
        create: false,
    })
    .unwrap();
    app.tree.select(Some(second.clone()));
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.transcript.extend([
        TranscriptItem::User("first prompt".into()),
        TranscriptItem::User("second prompt".into()),
    ]);
    app.agent.prompt = "current draft".into();
    let context = theme::test_context();
    fn draw(context: &egui::Context, app: &mut EditorApp, key: Option<Key>) {
        let events = key
            .map(|key| {
                vec![Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }]
            })
            .unwrap_or_default();
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    }

    draw(&context, &mut app, None);
    let id = Id::new("agent_prompt");
    let mut state = egui::TextEdit::load_state(&context, id).unwrap();
    state
        .cursor
        .set_char_range(Some(egui::text::CCursorRange::one(
            egui::text::CCursor::new(0),
        )));
    egui::TextEdit::store_state(&context, id, state);
    context.memory_mut(|memory| memory.request_focus(id));

    draw(&context, &mut app, Some(Key::ArrowUp));
    assert_eq!(app.agent.prompt, "second prompt");
    assert_eq!(app.tree.selected.as_ref(), Some(&second));
    assert_eq!(app.agent_prompt_history_index, Some(1));
    let cursor = egui::TextEdit::load_state(&context, id)
        .and_then(|state| state.cursor.char_range())
        .unwrap();
    assert_eq!(cursor.primary.index, egui::text::CharIndex(0));
    draw(&context, &mut app, Some(Key::ArrowUp));
    assert_eq!(app.agent.prompt, "first prompt");
    draw(&context, &mut app, Some(Key::ArrowDown));
    assert_eq!(app.agent.prompt, "second prompt");
    draw(&context, &mut app, Some(Key::ArrowDown));
    assert_eq!(app.agent.prompt, "current draft");
}

#[test]
fn transcript_following_only_resticks_at_the_bottom() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.transcript.extend((0..80).map(|line| {
        TranscriptItem::Assistant(format!(
            "transcript overflow line {line}: enough text to wrap in the narrow sidebar"
        ))
    }));
    let context = theme::test_context();
    fn draw(
        context: &egui::Context,
        app: &mut EditorApp,
        events: Vec<Event>,
        time: f64,
    ) -> egui::FullOutput {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                time: Some(time),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    }

    let _ = draw(&context, &mut app, Vec::new(), 0.0);
    let _ = draw(&context, &mut app, Vec::new(), 1.0);
    assert!(app.agent_follow_transcript);
    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("followed output".into()));
    let _ = draw(&context, &mut app, Vec::new(), 1.5);
    let followed = draw(&context, &mut app, Vec::new(), 1.6);
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let (_, _, agent) = split_workspace(
        screen,
        app.sidebar,
        app.sidebar_width,
        true,
        app.agent_sidebar_width,
    );
    let transcript_gutter = pos2(agent.left() + 4.0, 250.0);
    fn text_rect(shape: &Shape, label: &str) -> Option<Rect> {
        match shape {
            Shape::Text(text) if text.galley.text().trim_end() == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_rect(shape, label)),
            _ => None,
        }
    }
    assert!(followed.shapes.iter().any(|shape| {
        text_rect(&shape.shape, "followed output")
            .is_some_and(|text| shape.clip_rect.intersects(text))
    }));
    let _ = draw(
        &context,
        &mut app,
        vec![
            Event::PointerMoved(transcript_gutter),
            Event::MouseWheel {
                unit: MouseWheelUnit::Line,
                delta: Vec2::new(0.0, 1.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
        2.0,
    );
    assert!(!app.agent_follow_transcript);

    app.agent
        .transcript
        .push_back(TranscriptItem::Assistant("new output".into()));
    let _ = draw(&context, &mut app, Vec::new(), 3.0);
    assert!(!app.agent_follow_transcript);
    let _ = draw(
        &context,
        &mut app,
        vec![
            Event::PointerMoved(transcript_gutter),
            Event::MouseWheel {
                unit: MouseWheelUnit::Line,
                delta: Vec2::new(0.0, -200.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
        4.0,
    );
    let _ = draw(&context, &mut app, Vec::new(), 5.0);
    assert!(app.agent_follow_transcript);
}

#[test]
fn agentic_transcript_scrolls_from_the_side_gutters() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.transcript.extend(
        (0..80).map(|line| TranscriptItem::Assistant(format!("transcript overflow line {line}"))),
    );
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1600.0, 700.0));
    let (_, agent) = split_agentic_workspace(screen, app.sidebar, app.sidebar_width);
    let pointer = pos2(agent.left() + 10.0, 250.0);

    for time in [0.0, 1.0] {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    }
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            time: Some(2.0),
            events: vec![
                Event::PointerMoved(pointer),
                Event::MouseWheel {
                    unit: MouseWheelUnit::Line,
                    delta: Vec2::new(0.0, 1.0),
                    phase: TouchPhase::Move,
                    modifiers: Modifiers::NONE,
                },
            ],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(!app.agent_follow_transcript);
}

#[test]
fn agent_menu_hugs_its_selector_without_wasted_bottom_space() {
    let sidebar = Rect::from_min_size(pos2(640.0, 34.0), Vec2::new(360.0, 641.0));
    let (_, transcript, composer) = split_agent_sidebar(sidebar, AGENT_COMPOSER_HEIGHT);
    let content = agent_composer_content(composer);
    let selector = Rect::from_min_size(
        pos2(content.left(), content.bottom() - 30.0),
        Vec2::new(76.0, 30.0),
    );
    let menu = agent_menu_rect(transcript, selector, 3, AGENT_MENU_ROW_HEIGHT, 8.0);

    assert_eq!(
        content.bottom(),
        composer.bottom() - (theme::space::LARGE - 5.0)
    );
    assert_eq!(composer.right() - content.right(), theme::space::MEDIUM);
    assert_eq!(menu.bottom(), selector.top() - 4.0);
    assert_eq!(menu.height(), 16.0 + 3.0 * AGENT_MENU_ROW_HEIGHT);
    assert!(menu.top() >= transcript.top());
}

#[test]
fn slash_command_query_tracks_only_the_command_token() {
    assert_eq!(slash_command_query("/"), Some(""));
    assert_eq!(slash_command_query("/sim"), Some("sim"));
    assert_eq!(slash_command_query("/simplify "), None);
    assert_eq!(slash_command_query("explain /sim"), None);
}

#[test]
fn cursor_run_everything_command_controls_the_composer_toggle() {
    let mut commands = vec![CommandChoice {
        name: "run-everything".into(),
        description: "Toggle Run Everything (currently disabled)".into(),
        input_hint: None,
    }];
    assert_eq!(run_everything_state(&commands), Some(false));

    commands[0].description = "Toggle Run Everything (currently enabled)".into();
    assert_eq!(run_everything_state(&commands), Some(true));

    commands[0].description = "Run Everything is disabled by admin settings".into();
    assert_eq!(run_everything_state(&commands), None);
}

#[test]
fn model_names_are_human_readable_without_rewriting_curated_labels() {
    assert_eq!(
        model_display_name("cursor/grok-4.5", "grok-4.5"),
        "Grok 4.5"
    );
    assert_eq!(
        model_display_name("gpt-5.6-sol", "gpt-5.6-sol"),
        "GPT 5.6 Sol"
    );
    assert_eq!(
        model_display_name("claude-opus-4-8", "claude-opus-4-8"),
        "Claude Opus 4.8"
    );
    assert_eq!(
        model_display_name("claude-sonnet-4-5", "Claude Sonnet 4.5"),
        "Claude Sonnet 4.5"
    );
}

#[test]
fn composer_combines_model_and_thinking_in_one_selector() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.allow_run_everything = true;
    app.agent.current_mode = Some("agent".into());
    app.agent.modes = vec![ModeChoice {
        id: "agent".into(),
        name: "Agent".into(),
        description: None,
    }];
    app.agent.config_options = vec![
        ConfigChoice {
            id: "model".into(),
            name: "Model".into(),
            description: None,
            value: ConfigValue::Select("auto".into()),
            options: vec![ConfigValueChoice {
                id: "auto".into(),
                name: "Auto".into(),
                description: None,
            }],
        },
        ConfigChoice {
            id: "thinking".into(),
            name: "Thinking".into(),
            description: None,
            value: ConfigValue::Boolean(true),
            options: Vec::new(),
        },
        ConfigChoice {
            id: "effort".into(),
            name: "Effort".into(),
            description: None,
            value: ConfigValue::Select("extra-high".into()),
            options: vec![ConfigValueChoice {
                id: "extra-high".into(),
                name: "Extra High".into(),
                description: None,
            }],
        },
        ConfigChoice {
            id: "fast".into(),
            name: "Fast".into(),
            description: None,
            value: ConfigValue::Select("true".into()),
            options: vec![
                ConfigValueChoice {
                    id: "false".into(),
                    name: "Off".into(),
                    description: None,
                },
                ConfigValueChoice {
                    id: "true".into(),
                    name: "On".into(),
                    description: None,
                },
            ],
        },
        ConfigChoice {
            id: "mode".into(),
            name: "Mode".into(),
            description: None,
            value: ConfigValue::Select("agent".into()),
            options: vec![ConfigValueChoice {
                id: "agent".into(),
                name: "Agent".into(),
                description: None,
            }],
        },
    ];
    let (_, fast_enabled, next_fast) =
        super::fast_mode_config(&app.agent.config_options).expect("Cursor fast parameter");
    assert!(fast_enabled);
    assert_eq!(next_fast, ConfigValue::Select("false".into()));
    let context = theme::test_context();
    let draw_at = |app: &mut EditorApp, width: f32| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(width, 700.0))),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    let draw = |app: &mut EditorApp| draw_at(app, 1000.0);
    fn text_rect(shape: &Shape, label: &str) -> Option<Rect> {
        match shape {
            Shape::Text(text) if text.galley.text() == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_rect(shape, label)),
            _ => None,
        }
    }
    fn text_color(shape: &Shape, label: &str) -> Option<Color32> {
        match shape {
            Shape::Text(text) if text.galley.text() == label => {
                Some(text.galley.job.sections[0].format.color)
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_color(shape, label)),
            _ => None,
        }
    }
    fn has_divider_between(shape: &Shape, popup: Rect, top: f32, bottom: f32) -> bool {
        match shape {
            Shape::LineSegment { points, stroke } => {
                stroke.color == theme::border::hairline_color()
                    && points[0].y > top
                    && points[0].y < bottom
                    && (points[0].y - points[1].y).abs() <= 1.0
                    && points[1].x - points[0].x >= popup.width() - 40.0
            }
            Shape::Vec(shapes) => shapes
                .iter()
                .any(|shape| has_divider_between(shape, popup, top, bottom)),
            _ => false,
        }
    }

    let output = draw(&mut app);
    for label in ["Ask", "Agent", "Auto · Extra High"] {
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| text_rect(&shape.shape, label).is_some()),
            "missing {label} selector"
        );
    }
    let find = |label| {
        output
            .shapes
            .iter()
            .find_map(|shape| text_rect(&shape.shape, label))
            .unwrap()
    };
    // The attach control is an icon now, and an unhovered icon button paints
    // only its glyph, so the control box is that glyph's 28 px target.
    let attach = context
        .read_response(Id::new("agent_attach"))
        .expect("the composer lost its attach control")
        .rect;
    let permissions = find("Ask");
    let mode = find("Agent");
    let model = find("Auto · Extra High");
    for selector in [permissions, mode, model] {
        assert!(
            (selector.center().y - attach.center().y).abs() <= 2.0,
            "selector {selector:?} is off the attach control's line at {attach:?}"
        );
    }
    fn composer_surface(shape: &Shape) -> Option<(Rect, u8)> {
        match shape {
            Shape::Rect(rect) if rect.fill == super::agentic_composer_fill() => {
                Some((rect.rect, rect.corner_radius.nw))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(composer_surface),
            _ => None,
        }
    }
    let (composer, composer_radius) = output
        .shapes
        .iter()
        .find_map(|shape| composer_surface(&shape.shape))
        .expect("agentic composer panel");
    assert_eq!(composer_radius, 10);
    // The visible glyph, not the button's invisible hit target, is what
    // has to line up with the composer's inset.
    let glyph_left = attach.center().x - crate::icons::GRID * 0.5;
    let left_gap = glyph_left - composer.left();
    let bottom_gap = composer.bottom() - attach.bottom();
    assert!((left_gap - theme::space::MEDIUM).abs() <= 2.0);
    assert!((bottom_gap - (theme::space::SMALL - 5.0)).abs() <= 2.0);
    assert!(
        mode.left() > permissions.right() && model.left() > mode.right(),
        "selectors overlap: permissions={permissions:?}, mode={mode:?}, model={model:?}"
    );
    assert!(
        output
            .shapes
            .iter()
            .all(|shape| text_rect(&shape.shape, "Fast").is_none())
    );
    let model_selector = context
        .read_response(Id::new("agent_model_selector"))
        .expect("model selector")
        .rect;
    assert!(
        crate::icons::probe::bounds(&output.shapes, model_selector, theme::accent()).is_some(),
        "Fast mode needs a bolt indicator"
    );

    app.agentic_mode = false;
    app.agent_sidebar_width = 320.0;
    let output = draw(&mut app);
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let agent = split_workspace(
        screen,
        app.sidebar,
        app.sidebar_width,
        true,
        app.agent_sidebar_width,
    )
    .2;
    let send_left = agent.right() - theme::space::MEDIUM - theme::control::STANDARD;
    let model_selector = context
        .read_response(Id::new("agent_model_selector"))
        .expect("model selector at minimum sidebar width")
        .rect;
    assert!(
        model_selector.right() <= send_left - theme::space::SMALL,
        "model selector {model_selector:?} collides with Send at x={send_left}"
    );
    assert!(
        output
            .shapes
            .iter()
            .all(|shape| text_rect(&shape.shape, "Fast").is_none())
    );
    app.agentic_mode = true;

    app.agent_menu = Some(super::AgentMenu::Permissions);
    assert!(
        draw(&mut app)
            .shapes
            .iter()
            .any(|shape| text_rect(&shape.shape, "Allow all").is_some())
    );

    app.agent_menu = Some(super::AgentMenu::Config("model".into()));
    let output = draw(&mut app);
    let popup = app.agent_menu_popup.expect("model and thinking menu");
    for label in ["Model", "Thinking", "Effort", "Speed", "Auto", "Extra High"] {
        let row = output
            .shapes
            .iter()
            .find_map(|shape| text_rect(&shape.shape, label))
            .unwrap_or_else(|| panic!("missing {label} in combined menu"));
        assert!(popup.contains(row.center()));
    }
    for header in ["Effort", "Speed", "Model"] {
        assert_eq!(
            output
                .shapes
                .iter()
                .find_map(|shape| text_color(&shape.shape, header)),
            Some(theme::text().primary)
        );
    }
    let effort_header = output
        .shapes
        .iter()
        .find_map(|shape| text_rect(&shape.shape, "Effort"))
        .unwrap();
    let effort_option = output
        .shapes
        .iter()
        .find_map(|shape| text_rect(&shape.shape, "Extra High"))
        .unwrap();
    assert!(output.shapes.iter().any(|shape| has_divider_between(
        &shape.shape,
        popup,
        effort_header.center().y,
        effort_option.center().y,
    )));
    assert!(output.shapes.iter().all(|shape| match &shape.shape {
        Shape::Rect(rect) => {
            rect.fill != theme::surface().sunken || !rect.rect.contains(effort_header.center())
        }
        _ => true,
    }));
    let fast_mode = output
        .shapes
        .iter()
        .find_map(|shape| text_rect(&shape.shape, "Fast mode"))
        .expect("Fast mode toggle label");
    assert!(popup.contains(fast_mode.center()));
    assert!(
        output
            .shapes
            .iter()
            .filter(|shape| match &shape.shape {
                Shape::Circle(circle) => popup.contains(circle.center),
                _ => false,
            })
            .count()
            >= 2
    );
}

#[test]
fn config_menu_hover_does_not_paint_a_transient_tooltip() {
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text().contains(expected),
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.config_options = vec![ConfigChoice {
        id: "effort".into(),
        name: "Thinking level".into(),
        description: None,
        value: ConfigValue::Select("high".into()),
        options: vec![ConfigValueChoice {
            id: "high".into(),
            name: "High".into(),
            description: Some("Transient tooltip text".into()),
        }],
    }];
    app.agent_menu = Some(super::AgentMenu::Config("effort".into()));
    let context = theme::test_context();
    context.all_styles_mut(|style| {
        style.interaction.tooltip_delay = 0.0;
        style.interaction.show_tooltips_only_when_still = false;
    });
    let draw = |app: &mut EditorApp, events, time| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                time: Some(time),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let _ = draw(&mut app, Vec::new(), 0.0);
    let popup = app.agent_menu_popup.expect("config menu");
    let hovered = draw(&mut app, vec![Event::PointerMoved(popup.center())], 1.0);
    let hovered_again = draw(&mut app, Vec::new(), 2.0);

    assert!(
        [&hovered, &hovered_again].iter().all(|output| output
            .shapes
            .iter()
            .all(|shape| !has_text(&shape.shape, "Transient tooltip text"))),
        "popup option descriptions must not flash as floating tooltips"
    );
}

#[test]
fn agent_history_menu_lists_restorable_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.history_available = true;
    app.agent.sessions = Some(vec![SessionChoice {
        id: "session-1".into(),
        title: Some("Previous landing page".into()),
        updated_at: Some("2026-08-07T12:00:00Z".into()),
        started_in_editur: true,
    }]);
    app.agent_menu = Some(super::AgentMenu::Sessions);
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    fn has_text(shape: &Shape, label: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == label,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, label)),
            _ => false,
        }
    }
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "Previous landing page"))
    );
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "×"))
    );
}

#[test]
fn agent_history_marks_only_sessions_started_outside_editur_with_the_provider() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.selected_provider = ProviderId::Codex;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.history_available = true;
    app.agent.sessions = Some(vec![
        SessionChoice {
            id: "provider-session".into(),
            title: Some("Started in Codex".into()),
            updated_at: None,
            started_in_editur: false,
        },
        SessionChoice {
            id: "editur-session".into(),
            title: Some("Started in Editur".into()),
            updated_at: None,
            started_in_editur: true,
        },
    ]);
    app.agent_menu = Some(super::AgentMenu::Sessions);
    let context = theme::test_context();
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(
        context
            .read_response(Id::new((
                "agent_session_origin",
                "provider-session",
                "codex",
            )))
            .is_some()
    );
    assert!(
        context
            .read_response(Id::new(
                ("agent_session_origin", "editur-session", "codex",)
            ))
            .is_none()
    );
}

#[test]
fn session_history_does_not_overscroll_past_its_last_row() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.history_available = true;
    app.agent.sessions = Some(
        (0..24)
            .map(|index| SessionChoice {
                id: format!("session-{index:02}"),
                title: Some(format!("Session {index:02}")),
                updated_at: None,
                started_in_editur: true,
            })
            .collect(),
    );
    app.agent_menu = Some(super::AgentMenu::Sessions);
    let context = theme::test_context();
    let mut draw = |events, time| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                time: Some(time),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    let has_text = |output: &egui::FullOutput, label: &str| {
        fn find(shape: &Shape, label: &str) -> bool {
            match shape {
                Shape::Text(text) => text.galley.text() == label,
                Shape::Vec(shapes) => shapes.iter().any(|shape| find(shape, label)),
                _ => false,
            }
        }
        output.shapes.iter().any(|shape| find(&shape.shape, label))
    };

    let _ = draw(Vec::new(), 0.0);
    let overscrolled = draw(
        vec![
            Event::PointerMoved(pos2(800.0, 120.0)),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(0.0, -2_000.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
        1.0,
    );

    assert!(has_text(&overscrolled, "Session 23"));
}

#[test]
fn slash_prompt_opens_an_opaque_filtered_bounded_command_menu() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.commands = (0..32)
        .map(|index| CommandChoice {
            name: if index == 0 {
                "needle".into()
            } else {
                format!("command-{index:02}")
            },
            description: String::new(),
            input_hint: None,
        })
        .collect();
    let context = theme::test_context();
    fn draw(
        context: &egui::Context,
        app: &mut EditorApp,
        events: Vec<Event>,
        time: f64,
    ) -> egui::FullOutput {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                time: Some(time),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    }
    fn popup_rect(output: &egui::FullOutput) -> Option<Rect> {
        fn find(shape: &Shape) -> Option<Rect> {
            match shape {
                Shape::Rect(rect)
                    if rect.fill == theme::surface().raised && rect.rect.width() > 100.0 =>
                {
                    Some(rect.rect)
                }
                Shape::Vec(shapes) => shapes.iter().find_map(find),
                _ => None,
            }
        }
        output.shapes.iter().find_map(|shape| find(&shape.shape))
    }
    fn has_text(output: &egui::FullOutput, expected: &str) -> bool {
        fn find(shape: &Shape, expected: &str) -> bool {
            match shape {
                Shape::Text(text) => text.galley.text() == expected,
                Shape::Vec(shapes) => shapes.iter().any(|shape| find(shape, expected)),
                _ => false,
            }
        }
        output
            .shapes
            .iter()
            .any(|shape| find(&shape.shape, expected))
    }

    let empty = draw(&context, &mut app, Vec::new(), 0.0);
    assert!(!has_text(&empty, "Commands"));
    context.memory_mut(|memory| memory.request_focus(Id::new("agent_prompt")));
    let output = draw(&context, &mut app, vec![Event::Text("/".into())], 1.0);
    assert_eq!(app.agent.prompt, "/");
    assert!(matches!(
        app.agent_menu,
        Some(super::AgentMenu::Commands(_))
    ));
    let full = popup_rect(&output).expect("command popup on its first frame");
    assert_eq!(full.height(), 296.0);

    let filtered = popup_rect(&draw(
        &context,
        &mut app,
        vec![Event::Text("needle".into())],
        2.0,
    ))
    .expect("filtered command popup");
    assert_eq!(filtered.height(), 56.0);
    assert_eq!(filtered.bottom(), full.bottom());
}

#[test]
fn model_menu_scrolls_when_the_pointer_is_over_it() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agent_sidebar = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.config_options = vec![ConfigChoice {
        id: "model-id".into(),
        name: "Model".into(),
        description: None,
        value: ConfigValue::Select("model-00".into()),
        options: (0..24)
            .map(|index| ConfigValueChoice {
                id: format!("model-{index:02}"),
                name: format!("model-{index:02}"),
                description: None,
            })
            .collect(),
    }];
    app.agent
        .transcript
        .extend((0..40).map(|index| TranscriptItem::Assistant(format!("transcript-{index:02}"))));
    app.agent_follow_transcript = false;
    app.agent_menu = Some(super::AgentMenu::Config("model-id".into()));
    let context = theme::test_context();
    let mut draw = |events, time| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                time: Some(time),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    let text_y = |output: &egui::FullOutput, label: &str| {
        fn find(shape: &Shape, label: &str) -> Option<f32> {
            match shape {
                Shape::Text(text) if text.galley.text().trim_end() == label => Some(text.pos.y),
                Shape::Vec(shapes) => shapes.iter().find_map(|shape| find(shape, label)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, label))
    };
    fn popup_rect(output: &egui::FullOutput) -> Option<Rect> {
        fn find(shape: &Shape) -> Option<Rect> {
            match shape {
                Shape::Rect(rect) if rect.fill == theme::surface().raised => Some(rect.rect),
                Shape::Vec(shapes) => shapes.iter().find_map(find),
                _ => None,
            }
        }
        output.shapes.iter().find_map(|shape| find(&shape.shape))
    }
    fn menu_text(output: &egui::FullOutput, popup: Rect, label: &str) -> Option<(f32, f32)> {
        fn find(shape: &Shape, clip: Rect, popup: Rect, label: &str) -> Option<(f32, f32)> {
            match shape {
                Shape::Text(text) if text.galley.text().trim_end() == label => {
                    let rect = Rect::from_min_size(text.pos, text.galley.size());
                    (popup.contains(text.pos) && clip.intersects(rect))
                        .then(|| (text.pos.y, text.galley.job.sections[0].format.font_id.size))
                }
                Shape::Vec(shapes) => shapes
                    .iter()
                    .find_map(|shape| find(shape, clip, popup, label)),
                _ => None,
            }
        }
        output
            .shapes
            .iter()
            .find_map(|shape| find(&shape.shape, shape.clip_rect, popup, label))
    }
    fn has_selected_surface(shape: &Shape) -> bool {
        match shape {
            Shape::Rect(rect) => rect.fill == theme::state::selected(),
            Shape::Vec(shapes) => shapes.iter().any(has_selected_surface),
            _ => false,
        }
    }

    let _ = draw(Vec::new(), 0.0);
    let before = draw(vec![Event::PointerMoved(pos2(780.0, 500.0))], 1.0);
    let before_popup = popup_rect(&before).expect("model popup");
    assert_eq!(
        menu_text(&before, before_popup, "Model 00").map(|(_, size)| size),
        Some(theme::typography::SMALL_SIZE)
    );
    assert!(
        before
            .shapes
            .iter()
            .any(|shape| has_selected_surface(&shape.shape))
    );
    let transcript_y = text_y(&before, "transcript-00").expect("visible transcript");
    let after = draw(
        vec![
            Event::PointerMoved(pos2(780.0, 500.0)),
            Event::MouseWheel {
                unit: MouseWheelUnit::Line,
                delta: Vec2::new(0.0, -6.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
        2.0,
    );
    let after_popup = popup_rect(&after).expect("scrolled model popup");
    assert!(menu_text(&after, after_popup, "Model 00").is_none());
    assert!(menu_text(&after, after_popup, "Model 10").is_some());
    assert_eq!(text_y(&after, "transcript-00"), Some(transcript_y));
}

#[test]
fn immediate_repaint_gets_a_followup_event_loop_deadline() {
    let now = Instant::now();

    assert_eq!(repaint_deadline(Duration::ZERO, now), Some(now));
}

#[test]
fn immediate_background_repaint_wakes_the_event_loop() {
    let context = theme::test_context();
    let (wake, woken) = std::sync::mpsc::channel();
    install_repaint_wake(&context, move || {
        let _ = wake.send(());
    });
    let worker_context = context.clone();

    std::thread::spawn(move || worker_context.request_repaint())
        .join()
        .unwrap();

    woken.recv_timeout(Duration::from_millis(100)).unwrap();
}

#[test]
fn texture_upload_forces_a_followup_repaint() {
    assert_eq!(
        repaint_delay_after_texture_update(Duration::MAX, true),
        Duration::ZERO
    );
}

#[test]
fn saved_window_geometry_uses_the_opening_display_scale() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("state/window.json");
    let geometry = super::WindowGeometry {
        position: Some((120, 80)),
        size: (1440, 900),
    };

    super::save_window_geometry(&path, geometry).unwrap();
    let attributes = super::load_window_geometry(&path)
        .unwrap()
        .apply(winit::window::Window::default_attributes(), 2.0);

    assert_eq!(
        (attributes.position, attributes.inner_size),
        (
            Some(winit::dpi::Position::Logical(
                winit::dpi::LogicalPosition::new(60.0, 40.0)
            )),
            Some(winit::dpi::Size::Logical(winit::dpi::LogicalSize::new(
                720.0, 450.0
            ))),
        )
    );
}

#[cfg(target_os = "macos")]
#[test]
fn saved_startup_geometry_is_not_adjusted_for_the_opening_display() {
    let attributes = super::WindowGeometry {
        position: Some((120, 20)),
        size: (1440, 1260),
    }
    .apply(winit::window::Window::default_attributes(), 1.0);

    let attributes = super::fit_startup_window_attributes(
        attributes,
        Some(super::startup_display_bounds(super::DisplayBounds {
            position: winit::dpi::PhysicalPosition::new(0, 0),
            size: winit::dpi::PhysicalSize::new(2048, 1280),
            scale_factor: 1.0,
        })),
        true,
    );

    assert_eq!(
        (attributes.position, attributes.inner_size),
        (
            Some(winit::dpi::Position::Logical(
                winit::dpi::LogicalPosition::new(120.0, 20.0)
            )),
            Some(winit::dpi::Size::Logical(winit::dpi::LogicalSize::new(
                1440.0, 1260.0
            ))),
        )
    );
}

#[test]
fn rapid_resizes_keep_only_the_latest_surface_size() {
    let mut pending = None;
    let mut redraw_at = None;
    let started = Instant::now();
    defer_resize(
        &mut pending,
        &mut redraw_at,
        winit::dpi::PhysicalSize::new(800, 600),
        started,
    );
    defer_resize(
        &mut pending,
        &mut redraw_at,
        winit::dpi::PhysicalSize::new(1920, 1080),
        started + Duration::from_millis(10),
    );

    assert_eq!(
        (pending, redraw_at),
        (
            Some(winit::dpi::PhysicalSize::new(1920, 1080)),
            Some(started + Duration::from_millis(10) + RESIZE_SETTLE_DELAY),
        )
    );
}

#[test]
fn maximize_skips_the_stale_frame_unless_it_contains_texture_updates() {
    assert!(skip_transition_render(true, false));
    assert!(!skip_transition_render(true, true));
    assert!(!skip_transition_render(false, false));
}

#[test]
fn project_search_polls_only_while_a_nonempty_query_is_pending() {
    assert!(!search_needs_polling("", "", false));
    assert!(search_needs_polling("needle", "", false));
    assert!(search_needs_polling("needle", "needle", false));
    assert!(!search_needs_polling("needle", "needle", true));
}

#[test]
fn macos_bundle_launch_skips_the_slow_launchservices_process() {
    assert!(!launch_in_current_process(false, false, false, true));
}

#[test]
fn terminal_launch_preserves_the_detached_cli() {
    assert!(!launch_in_current_process(true, false, false, false));
}

#[test]
fn file_tree_rebuilds_its_cached_rows_only_when_expansion_changes() {
    let temp = tempfile::tempdir().unwrap();
    let directory = temp.path().join("src");
    fs::create_dir(&directory).unwrap();
    fs::write(directory.join("main.rs"), "").unwrap();
    fs::write(temp.path().join("README.md"), "").unwrap();
    let mut tree = TreeState::new(temp.path().to_path_buf(), None).unwrap();

    assert_eq!(tree.visible.len(), 2);
    tree.toggle(&directory).unwrap();
    assert_eq!(tree.visible.len(), 3);
    tree.collapse(&directory);
    assert_eq!(tree.visible.len(), 2);
}

#[test]
fn a_palette_group_announces_itself_only_when_it_has_results() {
    let context = theme::test_context();
    let empty = context.run_ui(RawInput::default(), |ui| {
        search_group_header(ui, "FILES", 0);
    });
    let nonempty = context.run_ui(RawInput::default(), |ui| {
        search_group_header(ui, "FILES", 3);
    });
    let painted = |shapes: &[egui::epaint::ClippedShape], text: &str| {
        shapes.iter().any(|shape| match &shape.shape {
            Shape::Text(painted) => painted.galley.text() == text,
            _ => false,
        })
    };
    assert!(painted(&empty.shapes, "FILES"));
    assert!(painted(&empty.shapes, "0"));
    assert!(painted(&nonempty.shapes, "3"));
    // The empty group still has a header when asked; the caller is what
    // decides whether to ask. The invariant is that the count is always
    // present when the header is.
    assert_eq!(
        file_result_job("src/app.rs", "app", 200.0)
            .sections
            .iter()
            .filter(|section| section.format.color == theme::accent())
            .count(),
        1
    );
}

#[test]
fn finds_every_case_insensitive_ascii_match_for_palette_highlighting() {
    assert_eq!(
        match_spans("Cargo cargo CARGO", "cargo"),
        [0..5, 6..11, 12..17]
    );
    assert!(match_spans("Cargo", "").is_empty());
}

#[test]
fn large_file_plain_layout_preserves_text_without_syntax_sections() {
    let job = plain_text_job("one\ntwo", 320.0);

    assert_eq!((job.text, job.sections.len()), ("one\ntwo".into(), 1));
}

#[test]
fn plain_layout_uses_the_active_theme_foreground() {
    let job = plain_text_job("large document", 320.0);

    assert_eq!(job.sections[0].format.color, theme::syntax().foreground);
}

#[test]
fn open_editor_rehighlights_when_palette_changes() {
    let _flag = theme::PALETTE_TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    theme::set_light(false);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("Cargo.toml");
    fs::write(&path, "[package]\nname = \"editur\"\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(path),
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(900.0, 500.0))),
        ..RawInput::default()
    };
    app.settings.appearance.theme = crate::settings::ThemePreference::Dark;
    let _ = context.run_ui(input(), |root| app.ui(root));
    app.settings.appearance.theme = crate::settings::ThemePreference::Light;
    let output = context.run_ui(input(), |root| app.ui(root));
    let foreground = output.shapes.iter().find_map(|shape| match &shape.shape {
        Shape::Text(text) => {
            let offset = text.galley.text().find("name")?;
            text.galley
                .job
                .sections
                .iter()
                .find(|section| section.byte_range.contains(&offset.into()))
                .map(|section| section.format.color)
        }
        _ => None,
    });
    let expected = theme::syntax().foreground;
    theme::set_light(false);

    assert_eq!(foreground, Some(expected));
}

#[test]
fn project_search_scrolls_only_when_keyboard_navigation_moves_selection() {
    assert_eq!(
        search_selection_after_navigation(12, 30, false, false),
        (12, false)
    );
    assert_eq!(
        search_selection_after_navigation(12, 30, true, false),
        (13, true)
    );
    assert_eq!(
        search_selection_after_navigation(12, 30, false, true),
        (11, true)
    );
    assert_eq!(
        search_selection_after_navigation(0, 30, false, true),
        (0, false)
    );
}

#[test]
fn project_search_is_full_size_and_opaque_on_its_first_frame() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let input = |time| RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1000.0, 700.0),
        )),
        time: Some(time),
        ..RawInput::default()
    };
    fn contains_opaque_palette(shape: &Shape, height: std::ops::Range<f32>) -> bool {
        match shape {
            Shape::Rect(rect) => {
                rect.rect.width() > 650.0
                    && height.contains(&rect.rect.height())
                    && rect.fill == theme::surface().raised
            }
            Shape::Vec(shapes) => shapes
                .iter()
                .any(|shape| contains_opaque_palette(shape, height.clone())),
            _ => false,
        }
    }

    let _ = context.run_ui(input(0.0), |root| app.ui(root));
    app.search_open = true;
    let first = context.run_ui(input(1.0), |root| app.ui(root));
    assert!(
        first
            .shapes
            .iter()
            .any(|shape| { contains_opaque_palette(&shape.shape, 150.0..250.0) })
    );

    app.search_query = ".e".into();
    let expanded = context.run_ui(input(1.016), |root| app.ui(root));
    assert!(
        expanded
            .shapes
            .iter()
            .any(|shape| { contains_opaque_palette(&shape.shape, 400.0..460.0) })
    );
}

#[test]
fn in_file_search_navigation_wraps_in_both_directions() {
    assert_eq!(next_find_match(0, 3, false), 1);
    assert_eq!(next_find_match(2, 3, false), 0);
    assert_eq!(next_find_match(0, 3, true), 2);
    assert_eq!(next_find_match(0, 0, false), 0);
}

#[test]
fn bracket_pair_matching_respects_nested_pairs_on_either_side_of_the_cursor() {
    let mut buffer = Buffer::new("nested.rs".into());
    buffer.text = "fn call(value: [u8; 2]) { values[index] }".into();
    buffer.mark_changed();
    let text = &buffer.text;
    let opening = text.find('[').unwrap();
    let closing = text[opening..].find(']').unwrap() + opening;

    assert_eq!(
        match_bracket_pair(&buffer, text[..opening].chars().count()),
        Some((opening..opening + 1, closing..closing + 1))
    );
    assert_eq!(
        match_bracket_pair(&buffer, text[..closing + 1].chars().count()),
        Some((opening..opening + 1, closing..closing + 1))
    );
}

#[test]
fn editor_header_omits_language_and_vim_badges() {
    fn contains_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| contains_text(shape, expected)),
            _ => false,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("main.rs");
    fs::write(&file, "fn main() {}").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file),
        create: false,
    })
    .unwrap();
    app.settings.keybindings.active_profile = "vim".into();
    app.lsp_status.insert(
        crate::lsp::PresetId::RustAnalyzer,
        crate::lsp::ServerStatus::Ready(crate::lsp::ServerCapabilities {
            sync: crate::lsp::SyncKind::Incremental,
            open_close: true,
            save_include_text: None,
            completion: true,
            completion_triggers: Vec::new(),
            hover: true,
            definition: true,
        }),
    );

    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1_000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    for badge in ["Rust", "NORMAL"] {
        assert!(
            !output
                .shapes
                .iter()
                .any(|shape| contains_text(&shape.shape, badge)),
            "unexpected editor badge {badge}"
        );
    }
}

#[test]
fn unchanged_editor_presentation_borrows_the_highlighted_document() {
    let job = plain_text_job("large document", 800.0);

    assert!(std::ptr::eq(presentation_job(&job, None), &job));
}

#[test]
fn sidebar_divider_uses_the_horizontal_resize_cursor() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.settings.appearance.ui_scale_percent = 200;
    let context = theme::test_context();
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            events: vec![Event::PointerMoved(pos2(248.0, 100.0))],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert_eq!(
        output.platform_output.cursor_icon,
        CursorIcon::ResizeHorizontal
    );
    fn active_divider_width(shape: &Shape) -> Option<f32> {
        match shape {
            Shape::LineSegment { stroke, .. } if stroke.color == theme::accent() => {
                Some(stroke.width)
            }
            Shape::Vec(shapes) => shapes.iter().find_map(active_divider_width),
            _ => None,
        }
    }
    fn contains_editor_header(shape: &Shape) -> bool {
        match shape {
            Shape::Rect(rect) => {
                rect.fill == theme::surface().chrome
                    && rect.rect.contains(pos2(500.0, 20.0))
                    && rect.rect.height() == TITLEBAR_HEIGHT
            }
            Shape::Vec(shapes) => shapes.iter().any(contains_editor_header),
            _ => false,
        }
    }
    let divider_index = output
        .shapes
        .iter()
        .position(|shape| active_divider_width(&shape.shape).is_some())
        .expect("active sidebar divider");
    let header_index = output
        .shapes
        .iter()
        .position(|shape| contains_editor_header(&shape.shape))
        .expect("editor header");
    let width = output
        .shapes
        .iter()
        .find_map(|shape| active_divider_width(&shape.shape))
        .expect("active sidebar divider");
    assert_eq!(width, 0.5);
    assert!(divider_index > header_index);
}

#[test]
fn active_resize_dividers_change_color_without_becoming_thicker() {
    let context = theme::test_context();
    context.set_pixels_per_point(2.0);
    let mut strokes = None;
    let _ = context.run_ui(RawInput::default(), |_| {
        strokes = Some((
            resize_divider_stroke(&context, true),
            resize_divider_stroke(&context, false),
        ));
    });
    let (active, inactive) = strokes.unwrap();

    assert_eq!(active, egui::Stroke::new(0.5, theme::accent()));
    assert_eq!(
        inactive,
        egui::Stroke::new(0.5, super::theme::border::strong_color())
    );
}

#[test]
fn unoccupied_titlebar_regions_start_a_window_drag() {
    for start in [pos2(400.0, 17.0), pos2(600.0, 17.0)] {
        let temp = tempfile::tempdir().unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root: temp.path().canonicalize().unwrap(),
            file: None,
            create: false,
        })
        .unwrap();
        app.agent_sidebar = true;
        let context = theme::test_context();
        let mut draw = |events| {
            let _ = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(
                        pos2(0.0, 0.0),
                        Vec2::new(1000.0, 700.0),
                    )),
                    events,
                    ..RawInput::default()
                },
                |root| app.ui(root),
            );
        };
        draw(Vec::new());
        draw(vec![
            Event::PointerMoved(start),
            Event::PointerButton {
                pos: start,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ]);
        draw(vec![Event::PointerMoved(start + Vec2::new(10.0, 0.0))]);

        assert!(
            matches!(app.take_window_action(), Some(super::WindowAction::Drag)),
            "header at {start:?} did not start a window drag"
        );
    }
}

#[test]
fn sidebar_width_tracks_the_pointer_without_accumulating_drag_delta() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let mut draw = |events| {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    };
    draw(Vec::new());
    draw(vec![
        Event::PointerMoved(pos2(249.0, 100.0)),
        Event::PointerButton {
            pos: pos2(249.0, 100.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    draw(vec![Event::PointerMoved(pos2(300.0, 100.0))]);
    draw(vec![Event::PointerMoved(pos2(320.0, 100.0))]);

    assert_eq!(app.sidebar_width, 320.0);
}

#[test]
fn sidebar_resize_paints_the_current_pointer_position_in_the_same_frame() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let mut draw = |events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    let _ = draw(Vec::new());
    let _ = draw(vec![
        Event::PointerMoved(pos2(249.0, 100.0)),
        Event::PointerButton {
            pos: pos2(249.0, 100.0),
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    let output = draw(vec![Event::PointerMoved(pos2(320.0, 100.0))]);
    let divider_x = output.shapes.iter().find_map(|shape| match shape.shape {
        Shape::LineSegment { points, stroke } if stroke.color == theme::accent() => {
            Some(points[0].x)
        }
        _ => None,
    });

    assert_eq!(divider_x, Some(320.0));
}

#[test]
fn pane_resize_paints_the_current_pointer_position_in_the_same_frame() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.pane_layout.split(PaneId(0), DropZone::Right);
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let (_, editor, _) = split_workspace(
        screen,
        app.sidebar,
        app.sidebar_width,
        app.agent_sidebar,
        app.agent_sidebar_width,
    );
    let start = app.pane_layout.split_handles(editor_column_content(editor))[0]
        .hit_rect
        .center();
    let context = theme::test_context();
    let mut draw = |events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    let _ = draw(Vec::new());
    let _ = draw(vec![
        Event::PointerMoved(start),
        Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    let output = draw(vec![Event::PointerMoved(pos2(700.0, start.y))]);
    let divider_x = output.shapes.iter().find_map(|shape| match shape.shape {
        Shape::LineSegment { points, stroke }
            if stroke.color == theme::accent() && points[0].x == points[1].x =>
        {
            Some(points[0].x)
        }
        _ => None,
    });

    assert_eq!(divider_x, Some(700.0));
}

#[test]
fn terminal_resize_paints_the_current_pointer_position_in_the_same_frame() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    app.terminal.open(&app.tree.root, &context).unwrap();
    app.terminal_open = true;
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let (_, editor, _) = split_workspace(
        screen,
        app.sidebar,
        app.sidebar_width,
        app.agent_sidebar,
        app.agent_sidebar_width,
    );
    let workspace = Rect::from_min_max(editor.left_top(), screen.right_bottom());
    let (_, terminal) = split_bottom_panel(workspace, true, app.terminal_height);
    let start = terminal.unwrap().center_top();
    let mut draw = |events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    let _ = draw(Vec::new());
    let _ = draw(vec![
        Event::PointerMoved(start),
        Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    let output = draw(vec![Event::PointerMoved(pos2(start.x, 400.0))]);
    let divider_y = output.shapes.iter().find_map(|shape| match shape.shape {
        Shape::LineSegment { points, stroke }
            if stroke.color == theme::accent() && points[0].y == points[1].y =>
        {
            Some(points[0].y)
        }
        _ => None,
    });

    assert_eq!(divider_y, Some(400.0));
}

#[test]
fn agentic_sidebar_resize_paints_the_current_pointer_position_in_the_same_frame() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let start = split_agentic_workspace(screen, true, app.sidebar_width)
        .0
        .expect("agentic sidebar")
        .right_center();
    let context = theme::test_context();
    let mut draw = |events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    let _ = draw(Vec::new());
    let _ = draw(vec![
        Event::PointerMoved(start),
        Event::PointerButton {
            pos: start,
            button: PointerButton::Primary,
            pressed: true,
            modifiers: Modifiers::NONE,
        },
    ]);
    let output = draw(vec![Event::PointerMoved(pos2(340.0, start.y))]);
    let divider_x = output.shapes.iter().find_map(|shape| match shape.shape {
        Shape::LineSegment { points, stroke }
            if stroke.color == theme::accent() && points[0].x == points[1].x =>
        {
            Some(points[0].x)
        }
        _ => None,
    });

    assert_eq!(divider_x, Some(340.0));
}

#[cfg(target_os = "macos")]
#[test]
fn file_tree_sidebar_cannot_shrink_past_its_titlebar_controls() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let draw = |app: &mut EditorApp, events| {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    };

    draw(&mut app, Vec::new());
    draw(
        &mut app,
        vec![
            Event::PointerMoved(pos2(249.0, 100.0)),
            Event::PointerButton {
                pos: pos2(249.0, 100.0),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    draw(&mut app, vec![Event::PointerMoved(pos2(100.0, 100.0))]);
    draw(
        &mut app,
        vec![Event::PointerButton {
            pos: pos2(100.0, 100.0),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    draw(&mut app, Vec::new());

    let terminal = context
        .read_response(Id::new("terminal_toggle"))
        .expect("terminal toggle")
        .rect;
    let agentic = context
        .read_response(Id::new("agentic_mode_toggle"))
        .expect("agentic mode toggle")
        .rect;

    assert!(
        terminal.right() <= app.sidebar_width,
        "terminal={terminal:?}, agentic={agentic:?}, width={}",
        app.sidebar_width
    );
    assert!(agentic.left() >= terminal.right());
    assert!(
        agentic.right() <= app.sidebar_width,
        "terminal={terminal:?}, agentic={agentic:?}, width={}",
        app.sidebar_width
    );
}

#[test]
fn in_file_search_highlights_every_match_and_distinguishes_the_active_one() {
    let text = "needle then needle";
    let base = egui::text::LayoutJob::simple(
        text.into(),
        crate::theme::typography::code_editor(),
        egui::Color32::WHITE,
        400.0,
    );
    let highlighted = find_highlighted_job(&base, &match_spans(text, "needle"), 1);
    let backgrounds: Vec<_> = highlighted
        .sections
        .iter()
        .filter_map(|section| {
            (section.format.background != egui::Color32::TRANSPARENT)
                .then_some(section.format.background)
        })
        .collect();

    assert_eq!(highlighted.text, text);
    assert_eq!(backgrounds.len(), 2);
    assert_ne!(backgrounds[0], backgrounds[1]);
}

#[test]
fn arrow_keys_navigate_only_the_focused_pane() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = root.join("a.rs");
    let second = root.join("b.rs");
    fs::write(&first, "one\ntwo\n").unwrap();
    fs::write(&second, "three\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(first.clone()),
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let draw = |app: &mut EditorApp, events| {
        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1000.0, 700.0),
            )),
            events,
            ..RawInput::default()
        };
        let _ = context.run_ui(input, |root| app.ui(root));
    };
    let arrow_down = || Event::Key {
        key: Key::ArrowDown,
        physical_key: Some(Key::ArrowDown),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };

    draw(&mut app, Vec::new());
    draw(&mut app, vec![arrow_down()]);
    assert_eq!(app.tree.selected.as_ref(), Some(&first));
    assert!(context.memory(|memory| memory.has_focus(Id::new("editor"))));

    app.tree_focused = true;
    context.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
    draw(&mut app, vec![arrow_down()]);
    assert_eq!(app.tree.selected.as_ref(), Some(&second));
}

#[test]
fn command_f_and_command_shift_f_open_their_own_searches() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("current.rs");
    fs::write(&file, "needle here\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file),
        create: false,
    })
    .unwrap();
    app.sidebar = false;
    let context = theme::test_context();
    let command = Modifiers {
        command: true,
        ..Modifiers::NONE
    };
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(
            Default::default(),
            Vec2::new(1000.0, 700.0),
        )),
        modifiers: command,
        events: vec![Event::Key {
            key: Key::F,
            physical_key: Some(Key::F),
            pressed: true,
            repeat: false,
            modifiers: command,
        }],
        ..RawInput::default()
    };

    let _ = context.run_ui(input, |root| app.ui(root));

    assert!(app.pane_find.get(&PaneId(0)).is_some_and(|find| find.open));
    assert!(!app.search_open);
    assert!(!app.sidebar);
    assert!(
        context.memory(|memory| { memory.has_focus(Id::new(("file_search_query", PaneId(0).0))) })
    );

    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(
            Default::default(),
            Vec2::new(1000.0, 700.0),
        )),
        events: vec![
            Event::Key {
                key: Key::F,
                physical_key: Some(Key::F),
                pressed: false,
                repeat: false,
                modifiers: command,
            },
            Event::Text("needle".into()),
        ],
        ..RawInput::default()
    };
    let _ = context.run_ui(input, |root| app.ui(root));
    let find = app.pane_find.get(&PaneId(0)).unwrap();
    assert_eq!(find.query, "needle");
    assert_eq!(find.matches.as_slice(), std::slice::from_ref(&(0..6)));

    let command_shift = Modifiers {
        command: true,
        shift: true,
        ..Modifiers::NONE
    };
    let input = RawInput {
        screen_rect: Some(Rect::from_min_size(
            Default::default(),
            Vec2::new(1000.0, 700.0),
        )),
        modifiers: command_shift,
        events: vec![Event::Key {
            key: Key::F,
            physical_key: Some(Key::F),
            pressed: true,
            repeat: false,
            modifiers: command_shift,
        }],
        ..RawInput::default()
    };
    let _ = context.run_ui(input, |root| app.ui(root));

    assert!(app.search_open);
    assert!(!app.pane_find.get(&PaneId(0)).is_some_and(|find| find.open));
    assert!(context.memory(|memory| memory.has_focus(Id::new("project_search_query"))));
}

#[test]
fn command_f_opens_find_inside_the_active_pane() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["left.rs", "right.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "needle\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.sidebar = false;
    app.open_tab(paths[1].clone(), false);
    app.drop_tab(1, PaneId(0), DropZone::Right);
    let context = theme::test_context();
    let command = Modifiers {
        command: true,
        ..Modifiers::NONE
    };

    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1000.0, 700.0),
            )),
            modifiers: command,
            events: vec![Event::Key {
                key: Key::F,
                physical_key: Some(Key::F),
                pressed: true,
                repeat: false,
                modifiers: command,
            }],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    let query = context
        .read_response(Id::new(("file_search_query", PaneId(1).0)))
        .expect("find query in the active pane");
    assert!(query.rect.left() >= 500.0);
}

#[test]
fn escape_closes_find_in_the_active_pane() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("current.rs");
    fs::write(&file, "needle\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file),
        create: false,
    })
    .unwrap();
    app.settings = Settings::default();
    app.rebuild_keybinding_resolver().unwrap();
    app.sidebar = false;
    let context = theme::test_context();
    let screen = Rect::from_min_size(Default::default(), Vec2::new(1000.0, 700.0));
    let command = Modifiers {
        command: true,
        ..Modifiers::NONE
    };

    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            modifiers: command,
            events: vec![Event::Key {
                key: Key::F,
                physical_key: Some(Key::F),
                pressed: true,
                repeat: false,
                modifiers: command,
            }],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            events: vec![Event::Key {
                key: Key::Escape,
                physical_key: Some(Key::Escape),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(!app.pane_find.get(&PaneId(0)).is_some_and(|find| find.open));
}

#[test]
fn each_pane_keeps_its_own_find_footer_open() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["left.rs", "right.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "needle\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.sidebar = false;
    app.open_tab(paths[1].clone(), false);
    app.drop_tab(1, PaneId(0), DropZone::Right);
    let context = theme::test_context();
    let command = Modifiers {
        command: true,
        ..Modifiers::NONE
    };
    let draw = |app: &mut EditorApp| {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Default::default(),
                    Vec2::new(1000.0, 700.0),
                )),
                modifiers: command,
                events: vec![Event::Key {
                    key: Key::F,
                    physical_key: Some(Key::F),
                    pressed: true,
                    repeat: false,
                    modifiers: command,
                }],
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let _ = draw(&mut app);
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1000.0, 700.0),
            )),
            events: vec![Event::Key {
                key: Key::F,
                physical_key: Some(Key::F),
                pressed: false,
                repeat: false,
                modifiers: command,
            }],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    app.activate_tab(0);
    let output = draw(&mut app);
    fn find_hints(shape: &Shape) -> usize {
        match shape {
            Shape::Text(text) if text.galley.text() == "Find in current file…" => 1,
            Shape::Vec(shapes) => shapes.iter().map(find_hints).sum(),
            _ => 0,
        }
    }

    assert_eq!(
        output
            .shapes
            .iter()
            .map(|shape| find_hints(&shape.shape))
            .sum::<usize>(),
        2
    );
}

#[test]
fn single_editor_pane_has_no_focus_outline() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let file = root.join("only.rs");
    fs::write(&file, "fn main() {}\n").unwrap();

    for file in [None, Some(file)] {
        let mut app = EditorApp::new(OpenTarget {
            root: root.clone(),
            file,
            create: false,
        })
        .unwrap();
        app.sidebar = false;
        let output = theme::test_context().run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    Default::default(),
                    Vec2::new(1000.0, 700.0),
                )),
                ..RawInput::default()
            },
            |root| app.ui(root),
        );

        assert!(!output.shapes.iter().any(|shape| match &shape.shape {
            Shape::Rect(rect) => rect.stroke.color == theme::border::focus_color(),
            _ => false,
        }));
    }
}

#[test]
fn editor_pane_does_not_paint_over_the_window_right_or_bottom_border() {
    fn paints_outer_editor_edge(shape: &Shape, window: Rect) -> bool {
        match shape {
            Shape::LineSegment { points, stroke }
                if stroke.color == super::theme::border::strong_color() =>
            {
                points
                    .iter()
                    .all(|point| (point.x - (window.right() - 0.5)).abs() < 0.01)
                    || points
                        .iter()
                        .all(|point| (point.y - (window.bottom() - 0.5)).abs() < 0.01)
            }
            Shape::Vec(shapes) => shapes
                .iter()
                .any(|shape| paints_outer_editor_edge(shape, window)),
            _ => false,
        }
    }

    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let window = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(window),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(
        !output
            .shapes
            .iter()
            .any(|shape| paints_outer_editor_edge(&shape.shape, window))
    );
}

#[test]
fn scrolling_an_inactive_pane_focuses_it() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["left.rs", "right.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "one\ntwo\nthree\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.sidebar = false;
    app.open_tab(paths[1].clone(), false);
    app.drop_tab(1, PaneId(0), DropZone::Right);
    let context = theme::test_context();
    let screen = Rect::from_min_size(Default::default(), Vec2::new(1000.0, 700.0));
    let draw = |app: &mut EditorApp, events| {
        context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };

    let _ = draw(&mut app, Vec::new());
    let output = draw(
        &mut app,
        vec![
            Event::PointerMoved(pos2(250.0, 300.0)),
            Event::MouseWheel {
                unit: MouseWheelUnit::Line,
                delta: Vec2::new(0.0, -1.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
    );

    assert_eq!(app.active_pane, PaneId(0));
    let focus = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Rect(rect)
                if rect.stroke.color == theme::border::focus_color()
                    && rect.stroke.width == 1.0 =>
            {
                Some(rect)
            }
            _ => None,
        })
        .expect("focused pane outline");
    assert_eq!(
        focus.corner_radius,
        egui::CornerRadius {
            nw: WINDOW_CORNER_RADIUS,
            ne: 0,
            sw: WINDOW_CORNER_RADIUS,
            se: 0,
        }
    );
}

#[cfg(unix)]
#[test]
fn terminal_focus_suppresses_the_editor_pane_outline() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["left.rs", "right.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "text\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root: root.clone(),
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.sidebar = false;
    app.open_tab(paths[1].clone(), false);
    app.drop_tab(1, PaneId(0), DropZone::Right);
    let context = theme::test_context();
    app.terminal.open(&root, &context).unwrap();
    app.terminal_open = true;

    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(app.terminal.focused(&context));
    assert!(!output.shapes.iter().any(|shape| {
        matches!(
            &shape.shape,
            Shape::Rect(rect)
                if rect.stroke.color == theme::border::focus_color()
                    && rect.stroke.width == 1.0
        )
    }));
}

#[test]
fn opening_files_keeps_dirty_buffers_and_reuses_existing_tabs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = root.join("first.rs");
    let second = root.join("second.rs");
    fs::write(&first, "one\n").unwrap();
    fs::write(&second, "two\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(first.clone()),
        create: false,
    })
    .unwrap();
    app.tabs[0].buffer.text = "changed\n".into();
    app.tabs[0].buffer.mark_changed();

    app.request(PendingAction::Open(second.clone()));

    assert!(app.pending.is_none());
    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tabs[app.active_tab.unwrap()].buffer.path, second);

    app.request(PendingAction::Open(first.clone()));

    assert_eq!(app.tabs.len(), 2);
    assert_eq!(app.tabs[app.active_tab.unwrap()].buffer.path, first);
    assert_eq!(app.tabs[app.active_tab.unwrap()].buffer.text, "changed\n");
}

#[test]
fn dirty_tab_close_waits_for_an_explicit_discard() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("current.rs");
    fs::write(&file, "before\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file),
        create: false,
    })
    .unwrap();
    app.tabs[0].buffer.mark_changed();

    app.request(PendingAction::CloseTab(0));

    assert!(app.pending.is_some());
    assert_eq!(app.tabs.len(), 1);

    app.discard_pending();

    assert!(app.tabs.is_empty());
    assert!(app.active_tab.is_none());
}

#[test]
fn clicking_a_header_tab_switches_the_editor() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let first = root.join("first.rs");
    let second = root.join("second.rs");
    fs::write(&first, "one\n").unwrap();
    fs::write(&second, "two\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(first.clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(second.clone(), false);
    app.activate_tab(0);
    let context = theme::test_context();
    let draw = |app: &mut EditorApp, events| {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    };
    draw(&mut app, Vec::new());
    let second_tab = context
        .read_response(Id::new(("file_tab", second.display().to_string())))
        .expect("second tab")
        .rect;
    assert_eq!(
        (second_tab.top(), second_tab.bottom()),
        (0.0, TITLEBAR_HEIGHT)
    );
    let second_tab = second_tab.center();
    draw(
        &mut app,
        vec![
            Event::PointerMoved(second_tab),
            Event::PointerButton {
                pos: second_tab,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    draw(
        &mut app,
        vec![Event::PointerButton {
            pos: second_tab,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );

    assert_eq!(app.buffer().unwrap().path, second);
}

#[test]
fn file_tree_toggle_collapses_the_tree_without_overlapping_tabs() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let file = root.join("current.rs");
    fs::write(&file, "text\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(file.clone()),
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let draw = |app: &mut EditorApp, events| {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    };

    draw(&mut app, Vec::new());
    let open_button = context
        .read_response(Id::new("file_tree_toggle"))
        .expect("file tree toggle")
        .rect;
    draw(
        &mut app,
        vec![
            Event::PointerMoved(open_button.center()),
            Event::PointerButton {
                pos: open_button.center(),
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    draw(
        &mut app,
        vec![Event::PointerButton {
            pos: open_button.center(),
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );
    draw(&mut app, Vec::new());
    let agentic_button = context
        .read_response(Id::new("agentic_mode_toggle"))
        .expect("agentic mode toggle")
        .rect;
    let tab = context
        .read_response(Id::new(("file_tab", file.display().to_string())))
        .expect("file tab")
        .rect;

    assert!(!app.sidebar);
    assert!(tab.left() >= agentic_button.right());
}

#[test]
fn agentic_mode_replaces_the_editor_with_project_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.history_available = true;
    app.agent.sessions = Some(vec![SessionChoice {
        id: "session-1".into(),
        title: Some("Build the agentic workspace".into()),
        updated_at: None,
        started_in_editur: true,
    }]);
    let context = theme::test_context();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let mode_toggle = context
        .read_response(Id::new("agentic_mode_toggle"))
        .expect("agentic mode toggle")
        .rect;
    let (sessions, agent) = split_agentic_workspace(screen, true, app.sidebar_width);
    let sessions = sessions.expect("open agentic sidebar");
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }
    fn has_background(shape: &Shape, expected: Rect) -> bool {
        match shape {
            Shape::Rect(rect) => rect.rect == expected && rect.fill == editor_background(),
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_background(shape, expected)),
            _ => false,
        }
    }

    assert!(
        context
            .read_response(Id::new(("agent_session_open", "session-1")))
            .is_some()
    );
    assert_eq!(mode_toggle.right(), sessions.right());
    assert!(mode_toggle.width() >= 44.0);
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "IDE"))
    );
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_background(&shape.shape, agent))
    );
}

#[cfg(target_os = "macos")]
#[test]
fn agentic_session_sidebar_keeps_titlebar_controls_inside_at_minimum_window_width() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(520.0, 700.0));
    let context = theme::test_context();

    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    let sessions = split_agentic_workspace(screen, true, app.sidebar_width)
        .0
        .expect("agentic session sidebar");
    let terminal = context
        .read_response(Id::new("terminal_toggle"))
        .expect("terminal toggle")
        .rect;
    let mode = context
        .read_response(Id::new("agentic_mode_toggle"))
        .expect("agentic mode toggle")
        .rect;

    assert!(terminal.right() <= sessions.right());
    assert!(mode.left() >= terminal.right());
    assert!(mode.right() <= sessions.right());
}

#[test]
fn sidebar_visibility_is_shared_by_ide_and_agentic_modes() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("ide-sidebar-entry.txt"), "text\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.sidebar = false;
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.history_available = true;
    app.agent.sessions = Some(vec![SessionChoice {
        id: "hidden-session".into(),
        title: Some("Agent sidebar entry".into()),
        updated_at: None,
        started_in_editur: true,
    }]);
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1000.0, 700.0),
        )),
        ..RawInput::default()
    };
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }
    let context = theme::test_context();
    let agentic = context.run_ui(input(), |root| app.ui(root));
    app.agentic_mode = false;
    let ide = context.run_ui(input(), |root| app.ui(root));

    assert!(
        !agentic
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "Agent sidebar entry"))
    );
    assert!(
        !ide.shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "ide-sidebar-entry.txt"))
    );
}

#[test]
fn agentic_session_rail_lists_workspaces_before_sessions() {
    let temp = tempfile::tempdir().unwrap();
    let recent = temp.path().join("other-project");
    fs::create_dir_all(&recent).unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.available_providers = vec![ProviderId::Cursor, ProviderId::Codex];
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.recent_projects = vec![recent.clone()];
    let project = app
        .tree
        .root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn text_rect(shape: &Shape, expected: &str) -> Option<Rect> {
        match shape {
            Shape::Text(text) if text.galley.text() == expected => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            Shape::Vec(shapes) => shapes.iter().find_map(|shape| text_rect(shape, expected)),
            _ => None,
        }
    }

    let find = |expected| {
        output
            .shapes
            .iter()
            .find_map(|shape| text_rect(&shape.shape, expected))
    };
    let provider = find("Cursor").expect("provider");
    let projects_header = find("WORKSPACES").expect("workspaces header");
    let project = find(&project).expect("open project row");
    let recent_row = find("other-project").expect("recent project row");
    let sessions_header = find("SESSIONS").expect("sessions header");

    assert!(projects_header.top() >= provider.bottom());
    assert!(project.top() >= projects_header.bottom());
    assert!(recent_row.top() >= project.bottom());
    assert!(sessions_header.top() >= recent_row.bottom());
}

#[test]
fn agentic_workspace_row_shows_its_branch_and_pull_request() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.git_workspace_status = Some(crate::projects::GitWorkspaceStatus {
        branch: "codex/session-workspaces".into(),
        pull_request: Some(crate::projects::PullRequestStatus {
            number: 42,
            title: "Attach sessions to worktrees".into(),
            url: "https://github.com/editur/editur/pull/42".into(),
            state: "OPEN".into(),
            is_draft: false,
            review_decision: "APPROVED".into(),
            merge_state_status: "CLEAN".into(),
        }),
    });
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1_000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }

    for expected in ["WORKSPACES", "codex/session-workspaces", "PR #42 · Ready"] {
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| has_text(&shape.shape, expected)),
            "missing {expected:?}"
        );
    }
}

#[test]
fn agentic_workspace_branch_only_row_has_no_empty_third_line() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.git_workspace_status = Some(crate::projects::GitWorkspaceStatus {
        branch: "heads/release".into(),
        pull_request: None,
    });
    let root = app.tree.root.clone();
    let context = theme::test_context();
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1_000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |ui| app.ui(ui),
    );
    let row = context
        .read_response(Id::new(("agentic_project", root)))
        .expect("selected workspace row")
        .rect;
    let branch = output
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            Shape::Text(text) if text.galley.text() == "heads/release" => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
        .expect("branch label");

    assert!(row.bottom() - branch.bottom() <= 8.0);
}

#[test]
fn switching_projects_keeps_the_agentic_workspace_open() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: first.canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;

    app.perform(PendingAction::OpenTarget(OpenTarget {
        root: second.canonicalize().unwrap(),
        file: None,
        create: false,
    }));

    assert_eq!(app.tree.root, second.canonicalize().unwrap());
    assert!(app.agentic_mode);
    assert!(app.agent_boot_pending);
}

#[test]
fn the_editor_project_menu_offers_recents_and_a_folder_chooser() {
    let temp = tempfile::tempdir().unwrap();
    let recent = temp.path().join("neighbor");
    fs::create_dir_all(&recent).unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.recent_projects = vec![recent];
    app.project_menu = true;
    let context = theme::test_context();
    // New egui areas spend their first frame sizing themselves invisibly,
    // so the menu only paints on the second frame.
    let mut draw = || {
        context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                ..RawInput::default()
            },
            |root| app.ui(root),
        )
    };
    draw();
    let output = draw();
    fn collect_texts(shape: &Shape, texts: &mut Vec<String>) {
        match shape {
            Shape::Text(text) => texts.push(text.galley.text().to_owned()),
            Shape::Vec(shapes) => {
                for shape in shapes {
                    collect_texts(shape, texts);
                }
            }
            _ => {}
        }
    }

    let mut texts = Vec::new();
    for shape in &output.shapes {
        collect_texts(&shape.shape, &mut texts);
    }
    assert!(texts.iter().any(|text| text == "neighbor"), "{texts:?}");
    assert!(texts.iter().any(|text| text == "Open Folder…"), "{texts:?}");
    assert!(app.project_menu);
}

#[cfg(target_os = "macos")]
#[test]
fn opening_the_project_menu_survives_the_root_click() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    let screen = Some(Rect::from_min_size(
        pos2(0.0, 0.0),
        Vec2::new(1000.0, 700.0),
    ));
    let root_header = pos2(100.0, TITLEBAR_HEIGHT + 10.0);
    let mut draw = |events| {
        let _ = context.run_ui(
            RawInput {
                screen_rect: screen,
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    };

    draw(Vec::new());
    draw(vec![
        Event::PointerMoved(root_header),
        Event::PointerButton {
            pos: root_header,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        },
    ]);
    draw(vec![Event::PointerButton {
        pos: root_header,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    }]);

    assert!(app.project_menu);
}

#[test]
fn agentic_session_remove_control_stays_hidden_until_the_row_is_hovered() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.history_available = true;
    app.agent.sessions = Some(vec![SessionChoice {
        id: "session-1".into(),
        title: Some("Polish the agentic workspace".into()),
        updated_at: None,
        started_in_editur: true,
    }]);
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn has_remove(shape: &Shape) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == "×",
            Shape::Vec(shapes) => shapes.iter().any(has_remove),
            _ => false,
        }
    }

    assert!(!output.shapes.iter().any(|shape| has_remove(&shape.shape)));
}

#[test]
fn agentic_session_rows_are_compact() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.history_available = true;
    app.agent.sessions = Some(vec![SessionChoice {
        id: "session-1".into(),
        title: Some("Compact session".into()),
        updated_at: None,
        started_in_editur: true,
    }]);
    let context = theme::test_context();
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(
        context
            .read_response(Id::new(("agent_session_open", "session-1")))
            .expect("session row")
            .rect
            .height()
            <= 32.0
    );
}

#[test]
fn agentic_sidebar_marks_the_controller_active_session_as_selected() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    app.agent.history_available = true;
    app.agent.sessions = Some(vec![SessionChoice {
        id: "session-1".into(),
        title: Some("Current session".into()),
        updated_at: None,
        started_in_editur: true,
    }]);
    app.agent
        .apply(crate::agent::controller::Event::ActiveSessionChanged(
            "session-1".into(),
        ));
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn has_selected_fill(shape: &Shape) -> bool {
        match shape {
            Shape::Rect(rect) => rect.fill == theme::state::selected(),
            Shape::Vec(shapes) => shapes.iter().any(has_selected_fill),
            _ => false,
        }
    }

    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_selected_fill(&shape.shape))
    );
}

#[test]
fn agentic_composer_leaves_clearance_above_the_window_edge() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let output = theme::test_context().run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn composer_rect(shape: &Shape) -> Option<Rect> {
        match shape {
            Shape::Rect(rect) if rect.fill == super::agentic_composer_fill() => Some(rect.rect),
            Shape::Vec(shapes) => shapes.iter().find_map(composer_rect),
            _ => None,
        }
    }
    let composer = output
        .shapes
        .iter()
        .find_map(|shape| composer_rect(&shape.shape))
        .expect("agentic composer panel");

    assert!(screen.bottom() - composer.bottom() >= super::AGENTIC_COMPOSER_BOTTOM_MARGIN);
}

#[test]
fn narrow_file_tree_keeps_the_agentic_toggle_out_of_the_tab_strip() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let file = root.join("current.rs");
    fs::write(&file, "text\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(file.clone()),
        create: false,
    })
    .unwrap();
    app.sidebar_width = 120.0;
    let context = theme::test_context();
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    fn has_text(shape: &Shape, expected: &str) -> bool {
        match shape {
            Shape::Text(text) => text.galley.text() == expected,
            Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, expected)),
            _ => false,
        }
    }
    let agentic_button = context
        .read_response(Id::new("agentic_mode_toggle"))
        .expect("agentic mode toggle")
        .rect;
    let tab = context
        .read_response(Id::new(("file_tab", file.display().to_string())))
        .expect("file tab")
        .rect;

    assert!(tab.left() >= agentic_button.right());
    assert!(
        output
            .shapes
            .iter()
            .any(|shape| has_text(&shape.shape, "Agent"))
    );
}

#[test]
fn the_active_tab_continues_the_document_without_an_accent_border() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let file = root.join("current.rs");
    fs::write(&file, "text\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(file.clone()),
        create: false,
    })
    .unwrap();
    let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
    let context = theme::test_context();
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(screen),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let tab = context
        .read_response(Id::new(("file_tab", file.display().to_string())))
        .expect("file tab")
        .rect;

    let accent_border = output.shapes.iter().any(|shape| {
        matches!(
            &shape.shape,
            Shape::LineSegment { points, stroke }
                if *points == [
                    pos2(tab.left(), tab.top() + 1.0),
                    pos2(tab.right(), tab.top() + 1.0),
                ] && stroke.color == theme::accent()
        )
    });
    assert!(!accent_border, "the active tab has no blue top border");
    let continuous = output.shapes.iter().any(|shape| match &shape.shape {
        Shape::Rect(rect) => rect.rect == tab && rect.fill == theme::surface().editor,
        _ => false,
    });
    assert!(continuous, "the active tab carries the document's own fill");
}

#[test]
fn the_empty_editor_names_the_project_and_lists_only_bound_keys() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let app = EditorApp::new(OpenTarget {
        root: root.clone(),
        file: None,
        create: false,
    })
    .unwrap();
    let hints = app.keybinding_hints();
    assert!(hints.iter().all(|(_, chord)| !chord.is_empty()));
    assert!(
        hints.iter().all(|(label, _)| *label != "Open agent"),
        "a command with no chord in this profile has no hint to show: {hints:?}"
    );
    assert_eq!(hints.len(), 4, "{hints:?}");

    let project = root.file_name().unwrap().to_string_lossy().into_owned();
    let context = theme::test_context();
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(900.0, 600.0))),
            ..RawInput::default()
        },
        |ui| draw_editor_empty_state(ui, &project, &hints[..2]),
    );

    let painted = |text: &str| {
        output.shapes.iter().any(|shape| match &shape.shape {
            Shape::Text(painted) => painted.galley.text() == text,
            _ => false,
        })
    };
    assert!(painted(&project), "the window says what is open");
    for (label, chord) in &hints[..2] {
        assert!(painted(label), "{label} is missing");
        assert!(painted(chord), "{chord} is missing");
    }
    assert!(
        !painted(hints[3].0),
        "a command that was not passed must not appear"
    );
}

#[test]
fn the_empty_editor_and_the_project_chooser_paint_the_shipped_logo() {
    fn has_logo_texture(shape: &Shape) -> bool {
        match shape {
            Shape::Mesh(mesh) => mesh.texture_id != egui::TextureId::default(),
            Shape::Vec(shapes) => shapes.iter().any(has_logo_texture),
            _ => false,
        }
    }

    let context = theme::test_context();
    let input = || RawInput {
        screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(900.0, 600.0))),
        ..RawInput::default()
    };
    let empty_state = context.run_ui(input(), |ui| {
        draw_editor_empty_state(ui, "project", &[]);
    });
    assert!(
        empty_state
            .shapes
            .iter()
            .any(|shape| has_logo_texture(&shape.shape)),
        "the empty editor redraws the mark instead of showing the shipped logo"
    );

    let chooser = context.run_ui(input(), |ui| {
        let _ = project_chooser_ui(ui, None);
    });
    assert!(
        chooser
            .shapes
            .iter()
            .any(|shape| has_logo_texture(&shape.shape)),
        "the project chooser redraws the mark instead of showing the shipped logo"
    );
}

#[test]
fn the_pane_header_states_what_is_wrong_in_the_color_of_the_problem() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let file = root.join("broken.rs");
    fs::write(&file, "text\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(file.clone()),
        create: false,
    })
    .unwrap();
    let diagnostic = |severity| crate::lsp::Diagnostic {
        range: 0..1,
        line: 0,
        severity,
        source: None,
        code: None,
        message: "broken".to_owned(),
    };
    app.lsp_diagnostics.insert(
        file,
        LspDiagnosticsState {
            revision: 0,
            stale: false,
            generation: 1,
            diagnostics: vec![
                diagnostic(crate::lsp::DiagnosticSeverity::Error),
                diagnostic(crate::lsp::DiagnosticSeverity::Error),
                diagnostic(crate::lsp::DiagnosticSeverity::Warning),
            ],
        },
    );
    let context = theme::test_context();
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    let pill = |name: &str, text: &str| {
        let rect = context
            .read_response(Id::new((name, app.active_pane.0)))
            .unwrap_or_else(|| panic!("{name} pill"))
            .rect;
        output.shapes.iter().find_map(|shape| match &shape.shape {
            Shape::Text(painted) if painted.galley.text() == text && rect.contains(painted.pos) => {
                Some(painted.fallback_color)
            }
            _ => None,
        })
    };
    assert_eq!(
        pill("pane_errors", "2"),
        Some(theme::callout(theme::semantic().danger).text)
    );
    assert_eq!(
        pill("pane_warnings", "1"),
        Some(theme::callout(theme::semantic().warning).text)
    );
}

#[test]
fn a_tab_is_as_wide_as_its_name_between_a_floor_and_a_ceiling() {
    let context = theme::test_context();
    let mut measured = Vec::new();
    let _ = context.run_ui(RawInput::default(), |ui| {
        measured = ["a.rs", "settings.rs", &"n".repeat(120)]
            .map(|label| tab_width(ui, label))
            .to_vec();
    });

    let [short, middle, long] = measured[..] else {
        unreachable!()
    };
    assert_eq!(short, TAB_MIN_WIDTH);
    assert_eq!(long, TAB_MAX_WIDTH);
    assert!(middle > short && middle < long, "{middle}");
}

#[test]
fn a_tab_label_starts_at_the_same_inset_however_wide_its_neighbours_are() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let names = ["a.rs", "a_considerably_longer_module_name.rs"];
    let paths = names.map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "text\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(paths[1].clone(), false);
    let context = theme::test_context();
    let output = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1200.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    let insets = names.map(|name| {
        let tab = context
            .read_response(Id::new(("file_tab", root_path(&app, name))))
            .expect("tab")
            .rect;
        let label = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                Shape::Text(text) if text.galley.text() == name && tab.contains(text.pos) => {
                    Some(text.pos.x)
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("{name} has no label in its tab"));
        let close = context
            .read_response(Id::new(("file_tab_close", root_path(&app, name))))
            .expect("close control")
            .rect;
        assert!(tab.contains_rect(close), "{name} loses its close target");
        assert_eq!(close.width(), TAB_CLOSE);
        label - tab.left()
    });

    assert_eq!(insets[0], insets[1]);
}

fn root_path(app: &EditorApp, name: &str) -> String {
    app.tree.root.join(name).display().to_string()
}

#[test]
fn tab_strip_keeps_manual_horizontal_scroll() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = (0..8)
        .map(|index| root.join(format!("file-{index}.rs")))
        .collect::<Vec<_>>();
    for path in &paths {
        fs::write(path, "text\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    for path in paths.iter().skip(1) {
        app.open_tab(path.clone(), false);
    }
    app.activate_tab(0);
    let context = theme::test_context();
    let draw = |app: &mut EditorApp, events, time| {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                time: Some(time),
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    };
    draw(&mut app, Vec::new(), 0.0);
    let first_id = Id::new(("file_tab", paths[0].display().to_string()));
    let before = context.read_response(first_id).expect("first tab").rect;
    draw(
        &mut app,
        vec![
            Event::PointerMoved(before.center()),
            Event::MouseWheel {
                unit: MouseWheelUnit::Point,
                delta: Vec2::new(-500.0, 0.0),
                phase: TouchPhase::Move,
                modifiers: Modifiers::NONE,
            },
        ],
        1.0,
    );
    for time in 2..6 {
        draw(&mut app, Vec::new(), time as f64);
    }
    let after = context.read_response(first_id).expect("first tab").rect;

    assert!(after.left() < before.left() - 100.0);
}

#[test]
fn reordering_tabs_keeps_the_active_file_selected() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["a.rs", "b.rs", "c.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, path.file_name().unwrap().as_encoded_bytes()).unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(paths[1].clone(), false);
    app.open_tab(paths[2].clone(), false);
    app.activate_tab(1);

    app.move_tab(0, 2);

    assert_eq!(
        app.tabs
            .iter()
            .map(|tab| tab.buffer.path.clone())
            .collect::<Vec<_>>(),
        [paths[1].clone(), paths[2].clone(), paths[0].clone()]
    );
    assert_eq!(app.buffer().unwrap().path, paths[1]);
}

#[test]
fn dragging_a_header_tab_reorders_open_files() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["a.rs", "b.rs", "c.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, path.file_name().unwrap().as_encoded_bytes()).unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(paths[1].clone(), false);
    app.open_tab(paths[2].clone(), false);
    let context = theme::test_context();
    let draw = |app: &mut EditorApp, events| {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    };
    draw(&mut app, Vec::new());
    let first = context
        .read_response(Id::new(("file_tab", paths[0].display().to_string())))
        .expect("first tab")
        .rect
        .center();
    let third = context
        .read_response(Id::new(("file_tab", paths[2].display().to_string())))
        .expect("third tab")
        .rect
        .center();

    draw(
        &mut app,
        vec![
            Event::PointerMoved(first),
            Event::PointerButton {
                pos: first,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    draw(&mut app, vec![Event::PointerMoved(third)]);
    draw(
        &mut app,
        vec![Event::PointerButton {
            pos: third,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );

    assert_eq!(app.tabs[2].buffer.path, paths[0]);
}

#[test]
fn dragging_a_tab_into_the_editor_creates_a_split_pane() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().canonicalize().unwrap();
    let paths = ["a.rs", "b.rs"].map(|name| root.join(name));
    for path in &paths {
        fs::write(path, "text\n").unwrap();
    }
    let mut app = EditorApp::new(OpenTarget {
        root,
        file: Some(paths[0].clone()),
        create: false,
    })
    .unwrap();
    app.open_tab(paths[1].clone(), false);
    let context = theme::test_context();
    let draw = |app: &mut EditorApp, events| {
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                events,
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
    };
    draw(&mut app, Vec::new());
    let tab = context
        .read_response(Id::new(("file_tab", paths[1].display().to_string())))
        .unwrap()
        .rect
        .center();
    let editor = context.read_response(Id::new("editor")).unwrap().rect;
    let target = pos2(editor.center().x, editor.bottom() - 4.0);

    draw(
        &mut app,
        vec![
            Event::PointerMoved(tab),
            Event::PointerButton {
                pos: tab,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: Modifiers::NONE,
            },
        ],
    );
    draw(&mut app, vec![Event::PointerMoved(target)]);
    draw(
        &mut app,
        vec![Event::PointerButton {
            pos: target,
            button: PointerButton::Primary,
            pressed: false,
            modifiers: Modifiers::NONE,
        }],
    );

    assert_eq!(
        (app.tabs[1].pane, app.pane_layout.rects(editor).len()),
        (PaneId(1), 2)
    );
}

#[test]
fn command_s_q_saves_before_quitting() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("current.rs");
    fs::write(&file, "before\n").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file.clone()),
        create: false,
    })
    .unwrap();
    let buffer = &mut app.tabs[app.active_tab.unwrap()].buffer;
    buffer.text = "after\n".into();
    buffer.mark_changed();
    let command = Modifiers {
        command: true,
        ..Modifiers::NONE
    };
    let context = theme::test_context();
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            modifiers: command,
            events: [Key::S, Key::Q]
                .into_iter()
                .map(|key| Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers: command,
                })
                .collect(),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert!(app.should_close);
    assert_eq!(fs::read_to_string(file).unwrap(), "after\n");
}

#[test]
fn normalized_paste_runs_once_and_an_empty_profile_can_disable_it() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("current.rs");
    fs::write(&file, "base").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file),
        create: false,
    })
    .unwrap();
    app.settings = Settings::default();
    app.rebuild_keybinding_resolver().unwrap();
    let context = theme::test_context();
    let command = Modifiers {
        command: true,
        ..Modifiers::NONE
    };
    let paste = || RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1000.0, 700.0),
        )),
        events: vec![
            Event::Paste("X".into()),
            Event::Key {
                key: Key::V,
                physical_key: Some(Key::V),
                pressed: true,
                repeat: false,
                modifiers: command,
            },
        ],
        ..RawInput::default()
    };

    let _ = context.run_ui(paste(), |root| app.ui(root));
    assert_eq!(app.tabs[0].buffer.text, "Xbase");

    let empty = app
        .settings
        .keybindings
        .create_profile("Empty", None, crate::keybindings::Behavior::Standard)
        .unwrap();
    app.settings.keybindings.set_active(&empty).unwrap();
    app.rebuild_keybinding_resolver().unwrap();
    let _ = context.run_ui(paste(), |root| app.ui(root));

    assert_eq!(app.tabs[0].buffer.text, "Xbase");
}

#[test]
fn agentic_mode_leaves_copy_events_for_selectable_diff_text() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    app.agentic_mode = true;
    let context = theme::test_context();
    let mut copy_reaches_widgets = false;
    let _ = context.run_ui(
        RawInput {
            events: vec![Event::Copy],
            ..RawInput::default()
        },
        |_root| {
            app.shortcuts(&context);
            copy_reaches_widgets = context.input(|input| input.events.contains(&Event::Copy));
        },
    );

    assert!(copy_reaches_widgets);
}

#[cfg(unix)]
#[test]
fn terminal_focus_leaves_paste_events_for_the_terminal_surface() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: None,
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    app.terminal.open(&app.tree.root, &context).unwrap();
    app.terminal_open = true;
    let _ = context.run_ui(
        RawInput {
            screen_rect: Some(Rect::from_min_size(
                pos2(0.0, 0.0),
                Vec2::new(1000.0, 700.0),
            )),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    let mut paste_reaches_terminal = false;
    let _ = context.run_ui(
        RawInput {
            events: vec![Event::Paste("hello".into())],
            ..RawInput::default()
        },
        |_root| {
            app.shortcuts(&context);
            paste_reaches_terminal =
                context.input(|input| input.events.contains(&Event::Paste("hello".into())));
        },
    );

    assert!(paste_reaches_terminal);
}

#[cfg(unix)]
#[test]
fn terminal_focus_leaves_tab_for_shell_completion_even_with_a_stale_editor_popup() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("current.txt");
    fs::write(&file, "pri").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file.clone()),
        create: false,
    })
    .unwrap();
    let context = theme::test_context();
    app.terminal.open(&app.tree.root, &context).unwrap();
    app.terminal_open = true;
    context.memory_mut(|memory| {
        memory.request_focus(Id::new(("terminal_surface", 1_u64)));
    });
    app.lsp_completion = Some(CompletionPopup {
        tag: RequestTag {
            path: file,
            revision: 0,
            cursor: 3,
        },
        items: vec![CompletionItem {
            label: "print".into(),
            kind: None,
            detail: None,
            insert_text: "print".into(),
            edit: None,
        }],
        selected: 0,
        anchor: Rect::NOTHING,
        bounds: Rect::EVERYTHING,
    });
    let tab = Event::Key {
        key: Key::Tab,
        physical_key: Some(Key::Tab),
        pressed: true,
        repeat: false,
        modifiers: Modifiers::NONE,
    };

    let _ = context.run_ui(
        RawInput {
            events: vec![tab.clone()],
            ..RawInput::default()
        },
        |_root| {
            app.shortcuts(&context);
            assert!(context.input(|input| input.events.contains(&tab)));
        },
    );
}

#[test]
fn vim_operator_scope_updates_between_events_in_one_frame() {
    let temp = tempfile::tempdir().unwrap();
    let file = temp.path().join("current.rs");
    fs::write(&file, "one two").unwrap();
    let mut app = EditorApp::new(OpenTarget {
        root: temp.path().canonicalize().unwrap(),
        file: Some(file),
        create: false,
    })
    .unwrap();
    app.settings
        .keybindings
        .set_active(crate::keybindings::BUILTIN_VIM)
        .unwrap();
    app.rebuild_keybinding_resolver().unwrap();
    let context = theme::test_context();
    let screen = Some(Rect::from_min_size(
        pos2(0.0, 0.0),
        Vec2::new(1000.0, 700.0),
    ));
    let _ = context.run_ui(
        RawInput {
            screen_rect: screen,
            events: [Key::C, Key::I, Key::W]
                .into_iter()
                .map(|key| Event::Key {
                    key,
                    physical_key: Some(key),
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                })
                .collect(),
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    assert_eq!(app.tabs[0].vim.mode(), crate::vim::VimMode::Insert);

    let _ = context.run_ui(
        RawInput {
            screen_rect: screen,
            events: vec![Event::Text("X".into())],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );
    let _ = context.run_ui(
        RawInput {
            screen_rect: screen,
            events: vec![Event::Key {
                key: Key::Escape,
                physical_key: Some(Key::Escape),
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            ..RawInput::default()
        },
        |root| app.ui(root),
    );

    assert_eq!(app.tabs[0].buffer.text, "X two");
    assert_eq!(app.tabs[0].vim.mode(), crate::vim::VimMode::Normal);
}
