use std::{
    fs,
    hint::black_box,
    path::PathBuf,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use egui::{RawInput, Rect, Vec2, pos2};
use serde::Serialize;

use super::{
    ASSISTANT_IMAGE_CACHE_UNUSED_FRAMES, AssistantImagePathCache, EditorApp, WorkspaceFilePicker,
    assistant_embedded_image_texture,
};
use crate::{
    agent::{controller::ConnectionState, state::TranscriptItem},
    buffer::Buffer,
    devin::{Activity, DevinMessage, SessionSummary, StatusCategory},
    editor_surface::{DocumentMetrics, EditorSurface},
    file_io::OpenTarget,
    git::{
        controller::discover_repositories,
        state::GitAvailability,
        status::{BranchInfo, ChangeKind, GitEntry, GitStatusSnapshot, RepoInfo, RepositoryStatus},
    },
    keybindings::Command,
    search::SearchController,
    theme,
    vim::{VimSession, VimState, VimTextIndex},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Summary {
    median: Duration,
    p95: Duration,
    max: Duration,
}

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "lowercase")]
enum Decision {
    Green,
    Watch,
    Red,
}

#[derive(Debug, Serialize)]
struct BenchmarkRecord {
    scenario: &'static str,
    workload: String,
    scale: usize,
    samples: usize,
    median_ms: f64,
    p95_ms: f64,
    max_ms: f64,
    decision: Decision,
    trigger: &'static str,
}

#[derive(Debug, Serialize)]
struct MetricRecord {
    scenario: &'static str,
    workload: String,
    scale: usize,
    metric: &'static str,
    value: f64,
    unit: &'static str,
    decision: Decision,
    trigger: &'static str,
}

#[derive(Clone, Copy)]
struct Config {
    warmups: usize,
    samples: usize,
    quick: bool,
}

fn summarize(samples: impl IntoIterator<Item = Duration>) -> Summary {
    let mut samples = samples.into_iter().collect::<Vec<_>>();
    assert!(!samples.is_empty(), "a benchmark needs at least one sample");
    samples.sort_unstable();
    Summary {
        median: samples[samples.len() / 2],
        p95: samples[samples.len() * 95 / 100],
        max: samples[samples.len() - 1],
    }
}

#[expect(clippy::too_many_arguments)]
fn timed_record(
    scenario: &'static str,
    workload: impl Into<String>,
    scale: usize,
    summary: Summary,
    samples: usize,
    green_ms: f64,
    red_ms: f64,
    trigger: &'static str,
) -> BenchmarkRecord {
    let p95_ms = duration_ms(summary.p95);
    let decision = if p95_ms <= green_ms {
        Decision::Green
    } else if p95_ms <= red_ms {
        Decision::Watch
    } else {
        Decision::Red
    };
    BenchmarkRecord {
        scenario,
        workload: workload.into(),
        scale,
        samples,
        median_ms: duration_ms(summary.median),
        p95_ms,
        max_ms: duration_ms(summary.max),
        decision,
        trigger,
    }
}

fn duration_ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn config() -> Config {
    let quick = std::env::var_os("EDITUR_BENCH_QUICK").is_some();
    Config {
        warmups: if quick { 1 } else { 5 },
        samples: if quick { 3 } else { 30 },
        quick,
    }
}

fn sample(config: Config, mut run: impl FnMut()) -> Summary {
    for _ in 0..config.warmups {
        run();
    }
    summarize((0..config.samples).map(|_| {
        let started = Instant::now();
        run();
        started.elapsed()
    }))
}

fn emit(record: impl Serialize) {
    println!(
        "BENCH {}",
        serde_json::to_string(&record).expect("benchmark result serializes")
    );
}

fn input() -> RawInput {
    RawInput {
        screen_rect: Some(Rect::from_min_size(
            pos2(0.0, 0.0),
            Vec2::new(1_200.0, 800.0),
        )),
        ..RawInput::default()
    }
}

fn app_fixture() -> (tempfile::TempDir, EditorApp) {
    let directory = tempfile::tempdir().expect("temporary benchmark project");
    let app = EditorApp::new(OpenTarget {
        root: directory.path().canonicalize().expect("benchmark root"),
        file: None,
        create: false,
    })
    .expect("benchmark app");
    (directory, app)
}

fn frame_summary(config: Config, mut draw: impl FnMut(&mut egui::Ui)) -> Summary {
    let context = theme::test_context();
    sample(config, || {
        let output = context.run_ui(input(), |ui| draw(ui));
        black_box(output.shapes.len());
    })
}

#[test]
#[ignore = "manual release-mode performance benchmark"]
fn benchmark_vim_motions() {
    let config = config();
    let sizes: &[usize] = if config.quick {
        &[100 * 1024, 1024 * 1024]
    } else {
        &[100 * 1024, 1024 * 1024, 10 * 1024 * 1024]
    };
    let line = "fn café(value: usize) -> usize { let s = \"value\"; [value] } // benchmark\n";
    for &size in sizes {
        let mut text = line.repeat(size / line.len() + 1);
        text.truncate(text.floor_char_boundary(size));
        let character_len = text.chars().count();
        for (name, command, cursor) in [
            ("word-forward", Command::VimWordForward, 0),
            ("word-back", Command::VimWordBack, character_len),
            ("move-down", Command::VimMoveDown, character_len / 2),
            ("paragraph-forward", Command::VimParagraphForward, 0),
            ("paragraph-back", Command::VimParagraphBack, character_len),
        ] {
            let mut buffer = Buffer::new("benchmark.txt".into());
            buffer.text.clone_from(&text);
            buffer.mark_changed();
            let mut state = VimState::default();
            let mut editor = EditorSurface::default();
            let mut session = VimSession::default();
            let summary = sample(config, || {
                editor.set_selection(cursor, cursor);
                let (text, line_starts, line_byte_starts, character_len) = buffer.vim_parts();
                let index = VimTextIndex::new(line_starts, line_byte_starts, character_len);
                black_box(state.execute_indexed(command, &mut editor, text, &mut session, index));
            });
            emit(timed_record(
                "vim",
                format!("{size}-bytes-{name}"),
                size,
                summary,
                config.samples,
                4.0,
                8.0,
                "local motion p95 should stay below the input budget",
            ));
        }

        let middle_byte = text.floor_char_boundary(text.len() / 2);
        let object_line = text[middle_byte..].find("fn café").unwrap() + middle_byte;
        for (name, object, byte) in [
            ("inner-word", 'w', object_line + 3),
            (
                "inner-quote",
                '"',
                text[object_line..].find('"').unwrap() + object_line + 2,
            ),
            (
                "inner-pair",
                '(',
                text[object_line..].find('(').unwrap() + object_line + 2,
            ),
        ] {
            let cursor = text[..byte].chars().count();
            let mut buffer = Buffer::new("benchmark.txt".into());
            buffer.text.clone_from(&text);
            buffer.mark_changed();
            let mut state = VimState::default();
            let mut editor = EditorSurface::default();
            let mut session = VimSession::default();
            let summary = sample(config, || {
                editor.set_selection(cursor, cursor);
                let (text, line_starts, line_byte_starts, character_len) = buffer.vim_parts();
                let index = VimTextIndex::new(line_starts, line_byte_starts, character_len);
                state.execute_indexed(
                    Command::VimOperatorYank,
                    &mut editor,
                    text,
                    &mut session,
                    index,
                );
                state.execute_indexed(
                    Command::VimTextInner,
                    &mut editor,
                    text,
                    &mut session,
                    index,
                );
                black_box(state.provide_character_indexed(
                    object,
                    &mut editor,
                    text,
                    &mut session,
                    index,
                ));
            });
            emit(timed_record(
                "vim",
                format!("{size}-bytes-{name}"),
                size,
                summary,
                config.samples,
                4.0,
                8.0,
                "local text object p95 should stay below the input budget",
            ));
        }
    }
}

#[test]
#[ignore = "manual release-mode performance benchmark"]
fn benchmark_editor_scroll() {
    let config = config();
    let scales: &[usize] = if config.quick {
        &[10_000, 100_000]
    } else {
        &[10_000, 100_000, 250_000]
    };
    for &scale in scales {
        let mut text = "fn scroll_frame() { let value = 42; }\n".repeat(scale);
        let job = egui::text::LayoutJob::simple(
            text.clone(),
            theme::typography::code_editor(),
            theme::syntax().foreground,
            f32::INFINITY,
        );
        let document = DocumentMetrics {
            revision: 1,
            line_count: scale + 1,
            character_len: text.chars().count(),
        };
        let context = theme::test_context();
        let mut editor = EditorSurface::default();
        let _ = context.run_ui(input(), |ui| {
            editor.show_document(ui, &mut text, &job, document, false, None);
        });
        let summary = sample(config, || {
            let mut frame_input = input();
            frame_input.events = vec![
                egui::Event::PointerMoved(pos2(600.0, 400.0)),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: Vec2::new(0.0, -96.0),
                    phase: egui::TouchPhase::Move,
                    modifiers: egui::Modifiers::NONE,
                },
            ];
            let output = context.run_ui(frame_input, |ui| {
                black_box(editor.show_document(ui, &mut text, &job, document, false, None));
            });
            black_box(output.shapes.len());
        });
        emit(timed_record(
            "ui",
            "editor-scroll",
            scale,
            summary,
            config.samples,
            4.0,
            8.0,
            "editor scroll p95 should react inside half a 60 Hz frame",
        ));
    }
}

#[test]
#[ignore = "manual release-mode performance benchmark"]
fn benchmark_ui_histories_and_lists() {
    let config = config();
    let scales: &[usize] = if config.quick {
        &[100, 1_000]
    } else {
        &[100, 1_000, 10_000]
    };

    for &scale in scales {
        let (_directory, mut app) = app_fixture();
        app.devin_state.messages = (0..scale)
            .map(|index| DevinMessage {
                id: format!("message-{index}"),
                timestamp: index.to_string(),
                role: if index % 2 == 0 { "user" } else { "devin" }.into(),
                text: format!("Message {index} with **markdown** and `code`."),
                attachment_ids: Vec::new(),
            })
            .collect();
        app.devin_state.activity = (0..scale / 10)
            .map(|index| Activity {
                id: format!("activity-{index}"),
                timestamp: (index * 10 + 5).to_string(),
                category: "code".into(),
                summary: format!("Changed file {index}"),
                path: Some(format!("src/file-{index}.rs")),
                ..Activity::default()
            })
            .collect();
        let summary = frame_summary(config, |ui| app.benchmark_draw_devin_stream(ui));
        emit(timed_record(
            "ui",
            "devin-transcript",
            scale,
            summary,
            config.samples,
            8.0,
            16.7,
            "transcript frame p95 should remain inside one 60 Hz frame",
        ));

        let (_directory, mut app) = app_fixture();
        app.devin_state.connection = crate::devin::ConnectionState::Connected;
        app.devin_state.sessions = (0..scale)
            .map(|index| SessionSummary {
                id: format!("session-{index}"),
                title: format!("Session {index}"),
                status: "running".into(),
                category: StatusCategory::Active,
                repository: Some("openai/editur".into()),
                updated_at: Some("2026-08-19T12:00:00Z".into()),
                ..SessionSummary::default()
            })
            .collect();
        app.devin_state.sessions_revision = 1;
        let summary = frame_summary(config, |ui| app.benchmark_draw_devin_sessions(ui));
        emit(timed_record(
            "ui",
            "devin-session-list",
            scale,
            summary,
            config.samples,
            8.0,
            16.7,
            "session-list frame p95 should remain inside one 60 Hz frame",
        ));

        let (_directory, mut app) = app_fixture();
        let repository = app.tree.root.clone();
        let entries = (0..scale)
            .map(|index| GitEntry {
                path: format!("src/generated/file-{index:05}.rs").into(),
                orig_path: None,
                index: (index % 3 == 0).then_some(ChangeKind::Modified),
                worktree: Some(ChangeKind::Modified),
            })
            .collect();
        app.git_state.availability = GitAvailability::Ready;
        app.git_state.selected_repository = Some(repository.clone());
        app.git_state.snapshot = Some(GitStatusSnapshot {
            generation: 1,
            repositories: vec![RepositoryStatus {
                root: repository,
                info: RepoInfo {
                    branch: BranchInfo::Named {
                        name: "main".into(),
                        upstream: None,
                        ahead: 0,
                        behind: 0,
                        unborn: false,
                    },
                    last_commit_subject: None,
                },
                entries,
            }],
        });
        let summary = frame_summary(config, |ui| app.benchmark_draw_source_control(ui));
        emit(timed_record(
            "ui",
            "source-control-list",
            scale,
            summary,
            config.samples,
            8.0,
            16.7,
            "source-control frame p95 should remain inside one 60 Hz frame",
        ));

        let entries = (0..scale)
            .map(|index| crate::tree::TreeEntry {
                name: format!("file-{index:05}.rs").into(),
                path: PathBuf::from(format!("/benchmark/file-{index:05}.rs")),
                is_dir: false,
                is_symlink: false,
            })
            .collect();
        let mut picker = WorkspaceFilePicker::with_entries(PathBuf::from("/benchmark"), entries);
        let summary = frame_summary(config, |ui| {
            black_box(EditorApp::file_picker_dialog(
                ui.ctx(),
                &mut picker,
                None,
                0,
            ));
        });
        emit(timed_record(
            "ui",
            "file-picker-list",
            scale,
            summary,
            config.samples,
            8.0,
            16.7,
            "file-picker frame p95 should remain inside one 60 Hz frame",
        ));
    }

    let (_directory, mut app) = app_fixture();
    app.agent.connection = ConnectionState::Ready;
    app.agent.session_ready = true;
    for index in 0..2_048 {
        app.agent.transcript.push_back(if index % 2 == 0 {
            TranscriptItem::User(format!("Question {index}"))
        } else {
            TranscriptItem::Assistant(format!("Answer {index} with **markdown**."))
        });
    }
    let summary = frame_summary(config, |ui| app.draw_agent(ui, ui.max_rect()));
    emit(timed_record(
        "ui",
        "agent-transcript",
        2_048,
        summary,
        config.samples,
        8.0,
        16.7,
        "culled Agent transcript metadata should fit inside one 60 Hz frame",
    ));
}

#[test]
fn devin_transcript_culls_rows_after_their_first_measurement() {
    let (_directory, mut app) = app_fixture();
    app.devin_state.messages = (0..1_000)
        .map(|index| DevinMessage {
            id: format!("message-{index}"),
            timestamp: index.to_string(),
            role: "devin".into(),
            text: format!("Message {index}"),
            attachment_ids: Vec::new(),
        })
        .collect();
    let context = theme::test_context();
    for _ in 0..3 {
        let _ = context.run_ui(input(), |ui| app.benchmark_draw_devin_stream(ui));
    }

    assert!(
        app.devin_stream_rendered < 100,
        "only visible transcript rows should be laid out after measurement"
    );
}

#[test]
#[ignore = "manual release-mode performance benchmark"]
fn benchmark_image_cache_retention() {
    let config = config();
    let scales: &[usize] = if config.quick {
        &[50]
    } else {
        &[50, 200, 1_000]
    };
    let mut pixels = image::RgbaImage::new(128, 128);
    for (x, y, pixel) in pixels.enumerate_pixels_mut() {
        *pixel = image::Rgba([x as u8, y as u8, 128, 255]);
    }
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("benchmark PNG");

    for &scale in scales {
        let context = theme::test_context();
        let sources = (0..scale)
            .map(|_| Arc::<[u8]>::from(png.clone()))
            .collect::<Vec<_>>();
        let _ = context.run_ui(input(), |ui| {
            for source in &sources {
                black_box(assistant_embedded_image_texture(ui, source));
            }
        });
        drop(sources);
        for _ in 0..=ASSISTANT_IMAGE_CACHE_UNUSED_FRAMES {
            let _ = context.run_ui(input(), |_| {});
        }
        let manager = context.tex_manager();
        let manager = manager.read();
        let retained = manager
            .allocated()
            .filter(|(_, metadata)| metadata.name.starts_with("agent_embedded_image_"))
            .count();
        let bytes = manager
            .allocated()
            .filter(|(_, metadata)| metadata.name.starts_with("agent_embedded_image_"))
            .map(|(_, metadata)| metadata.bytes_used())
            .sum::<usize>();
        emit(MetricRecord {
            scenario: "memory",
            workload: "embedded-image-cache-count".into(),
            scale,
            metric: "retained_textures",
            value: retained as f64,
            unit: "textures",
            decision: if retained == 0 {
                Decision::Green
            } else {
                Decision::Red
            },
            trigger: "closed image sources should not leave permanent texture entries",
        });
        emit(MetricRecord {
            scenario: "memory",
            workload: "embedded-image-cache-bytes".into(),
            scale,
            metric: "retained_texture_bytes",
            value: bytes as f64,
            unit: "bytes",
            decision: if bytes == 0 {
                Decision::Green
            } else {
                Decision::Red
            },
            trigger: "closed image sources should not retain decoded texture memory",
        });
    }
}

#[test]
fn image_cache_reuses_recent_texture_and_remains_bounded() {
    let context = theme::test_context();
    let pixels = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 255]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgba8(pixels)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("test PNG");
    let source = Arc::<[u8]>::from(png);
    let mut original = None;
    let _ = context.run_ui(input(), |ui| {
        original = assistant_embedded_image_texture(ui, &source).map(|texture| texture.id());
    });
    let _ = context.run_ui(input(), |_| {});
    let mut reused = None;
    let _ = context.run_ui(input(), |ui| {
        reused = assistant_embedded_image_texture(ui, &source).map(|texture| texture.id());
    });
    assert_eq!(reused, original, "one offscreen frame should not re-decode");

    let sources = (0..65)
        .map(|_| Arc::<[u8]>::from(source.as_ref()))
        .collect::<Vec<_>>();
    let _ = context.run_ui(input(), |ui| {
        for source in &sources {
            assert!(assistant_embedded_image_texture(ui, source).is_some());
        }
    });

    let retained = context
        .tex_manager()
        .read()
        .allocated()
        .filter(|(_, metadata)| metadata.name.starts_with("agent_embedded_image_"))
        .count();
    assert!(retained <= 64, "image cache retained {retained} textures");
}

#[test]
fn image_path_metadata_is_rechecked_at_a_bounded_interval() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("preview.png");
    fs::write(&path, b"first").unwrap();
    let now = Instant::now();
    let mut cache = AssistantImagePathCache::default();
    let first = cache.fingerprint_at(&path, 640, now);
    fs::write(&path, b"second-is-longer").unwrap();

    assert_eq!(cache.fingerprint_at(&path, 640, now), first);
    assert_ne!(
        cache.fingerprint_at(&path, 640, now + Duration::from_secs(2)),
        first
    );
}

#[test]
#[ignore = "manual release-mode performance benchmark"]
fn benchmark_workspace_index_and_repository_discovery() {
    let config = config();
    let scales: &[usize] = if config.quick {
        &[1_000, 10_000]
    } else {
        &[10_000, 100_000]
    };
    for &scale in scales {
        let directory = tempfile::tempdir().expect("workspace fixture");
        let mut current_directory = usize::MAX;
        let mut path = directory.path().to_path_buf();
        for index in 0..scale {
            let group = index / 1_000;
            if group != current_directory {
                path = directory.path().join(format!("group-{group:04}"));
                fs::create_dir(&path).expect("fixture directory");
                if group % 25 == 0 {
                    fs::create_dir(path.join(".git")).expect("repository marker");
                }
                current_directory = group;
            }
            fs::File::create(path.join(format!("needle-{index:06}.rs"))).expect("fixture file");
        }

        let mut search = SearchController::new(directory.path().to_path_buf())
            .expect("search benchmark controller");
        let started = Instant::now();
        search.set_query("needle").expect("start benchmark query");
        let mut first_update = None;
        let completion = loop {
            if search.poll("needle") {
                first_update.get_or_insert_with(|| started.elapsed());
                if search.results().complete {
                    break started.elapsed();
                }
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "search benchmark timed out"
            );
            thread::sleep(Duration::from_millis(1));
        };
        emit(timed_record(
            "workspace",
            "search-first-update",
            scale,
            summarize([first_update.unwrap_or(completion)]),
            1,
            500.0,
            2_000.0,
            "workspace search should publish usable results promptly",
        ));
        emit(timed_record(
            "workspace",
            "search-complete",
            scale,
            summarize([completion]),
            1,
            2_000.0,
            5_000.0,
            "workspace indexing should finish without a long startup stall",
        ));

        let query_samples = if config.quick { 3 } else { 5 };
        let summary = summarize((0..query_samples).map(|index| {
            let query = format!("needle-{:02}", index + 10);
            let started = Instant::now();
            search.set_query(&query).expect("switch benchmark query");
            loop {
                if search.poll(&query) && search.results().complete {
                    break started.elapsed();
                }
                assert!(
                    started.elapsed() < Duration::from_secs(10),
                    "query switch timed out"
                );
                thread::sleep(Duration::from_millis(1));
            }
        }));
        emit(timed_record(
            "workspace",
            "search-query-switch",
            scale,
            summary,
            query_samples,
            100.0,
            250.0,
            "search query cancellation and replacement should stay interactive",
        ));

        let discovery_samples = if config.quick { 1 } else { 3 };
        let summary = summarize((0..discovery_samples).map(|_| {
            let started = Instant::now();
            black_box(
                discover_repositories(directory.path()).expect("repository discovery benchmark"),
            );
            started.elapsed()
        }));
        emit(timed_record(
            "workspace",
            "repository-discovery",
            scale,
            summary,
            discovery_samples,
            500.0,
            2_000.0,
            "repository discovery should not monopolize source control refresh",
        ));
    }
}

#[test]
fn benchmark_summary_reports_median_p95_and_maximum() {
    let summary = summarize([
        Duration::from_millis(5),
        Duration::from_millis(1),
        Duration::from_millis(4),
        Duration::from_millis(2),
        Duration::from_millis(3),
    ]);

    assert_eq!(
        summary,
        Summary {
            median: Duration::from_millis(3),
            p95: Duration::from_millis(5),
            max: Duration::from_millis(5),
        }
    );
}

#[test]
fn benchmark_record_is_machine_readable_and_classifies_latency() {
    let record = timed_record(
        "vim",
        "1-mib-word-forward",
        1_048_576,
        Summary {
            median: Duration::from_millis(3),
            p95: Duration::from_millis(6),
            max: Duration::from_millis(9),
        },
        30,
        4.0,
        8.0,
        "local motion p95",
    );
    let value = serde_json::to_value(record).expect("serializable benchmark result");

    assert_eq!(value["decision"], "watch");
}
