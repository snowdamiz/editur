use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    hash::{DefaultHasher, Hash, Hasher},
    io::IsTerminal,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

use egui::{
    Align, Align2, Color32, CursorIcon, FontId, Id, Key, Label, Layout, RichText, ScrollArea,
    Sense, TextEdit, TextFormat, UiBuilder, ViewportId, text::LayoutJob,
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{Icon, Window, WindowId},
};

use crate::{
    agent::{
        controller::{
            AgentController, Command as AgentCommand, ConfigValue, ConnectionState, ContentRole,
            DisplayContent, Event as AgentEvent, InteractionKind, InteractionResponse,
            QuestionAnswer, SessionChoice, ToolOutput,
        },
        state::{AgentState, TranscriptItem},
    },
    buffer::Buffer,
    editor_surface::{DocumentMetrics, EDITOR_BACKGROUND, EditorSurface},
    file_io::{OpenTarget, ReconcileOutcome, SaveError, load_buffer, reconcile_buffer, safe_save},
    instance::{Claim, InstanceEvent, claim, open_running, spawn_listener},
    markdown,
    renderer::Renderer,
    search::{SearchController, SearchHit, SearchResults},
    syntax::{Highlighter, IncrementalHighlightCache, SyntaxManager, data_dir},
    tree::{TreeEntry, read_directory},
    tree_surface::{TreeRow, TreeSurface},
};

#[derive(Clone)]
enum PendingAction {
    Open(PathBuf),
    OpenTarget(OpenTarget),
    CloseTab(usize),
    Close,
}

#[derive(Clone, Copy)]
enum WindowAction {
    Close,
    Minimize,
    ToggleMaximize,
    Drag,
}

const TITLEBAR_HEIGHT: f32 = 34.0;
const TAB_WIDTH: f32 = 176.0;
const FIND_BAR_HEIGHT: f32 = 38.0;
const AGENT_HEADER_HEIGHT: f32 = TITLEBAR_HEIGHT;
const AGENT_COMPOSER_HEIGHT: f32 = 108.0;
const AGENT_COMPOSER_MAX_HEIGHT: f32 = 240.0;
const AGENT_MENU_WIDTH: f32 = 240.0;
const AGENT_MENU_ROW_HEIGHT: f32 = 32.0;
const AGENT_COMMAND_ROW_HEIGHT: f32 = 40.0;
const AGENT_SESSION_ROW_HEIGHT: f32 = 40.0;
const AGENT_FOLLOW_THRESHOLD: f32 = 48.0;
const AGENT_TRANSCRIPT_EDGE_PADDING: i8 = 14;
const AGENT_TRANSCRIPT_EDGE_FADE: f32 = 28.0;
const AGENT_DIFF_PREVIEW_ROWS: usize = 18;
const AGENT_DIFF_PREVIEW_HEAD: usize = 12;
const TITLEBAR_PAINT_KEY: u64 = 0xa000_0000_0000_0000;

fn agent_near_bottom(offset: f32, max_offset: f32) -> bool {
    max_offset - offset <= AGENT_FOLLOW_THRESHOLD
}

fn agent_transcript_fade_mesh(rect: egui::Rect) -> egui::Mesh {
    let mut mesh = egui::Mesh::default();
    let height = AGENT_TRANSCRIPT_EDGE_FADE.min(rect.height() * 0.5);
    let opaque = Color32::from_rgb(20, 20, 22);
    for (position, color) in [
        (rect.left_top(), opaque),
        (rect.right_top(), opaque),
        (
            egui::pos2(rect.left(), rect.top() + height),
            Color32::TRANSPARENT,
        ),
        (
            egui::pos2(rect.right(), rect.top() + height),
            Color32::TRANSPARENT,
        ),
        (
            egui::pos2(rect.left(), rect.bottom() - height),
            Color32::TRANSPARENT,
        ),
        (
            egui::pos2(rect.right(), rect.bottom() - height),
            Color32::TRANSPARENT,
        ),
        (rect.left_bottom(), opaque),
        (rect.right_bottom(), opaque),
    ] {
        mesh.colored_vertex(position, color);
    }
    mesh.indices
        .extend_from_slice(&[0, 1, 2, 2, 1, 3, 4, 5, 6, 6, 5, 7]);
    mesh
}

fn draw_agent_content(ui: &mut egui::Ui, content: &DisplayContent) {
    match content {
        DisplayContent::Image {
            mime_type,
            uri,
            encoded_bytes,
        } => {
            ui.label(format!(
                "Image · {mime_type} · {encoded_bytes} encoded bytes"
            ));
            if let Some(uri) = uri {
                ui.add(Label::new(RichText::new(uri).monospace().small()).wrap());
            }
        }
        DisplayContent::Audio {
            mime_type,
            encoded_bytes,
        } => {
            ui.label(format!(
                "Audio · {mime_type} · {encoded_bytes} encoded bytes"
            ));
        }
        DisplayContent::ResourceLink {
            name,
            title,
            uri,
            description,
            mime_type,
            size,
        } => {
            ui.label(title.as_deref().unwrap_or(name));
            ui.add(Label::new(RichText::new(uri).monospace().small()).wrap());
            if let Some(description) = description {
                ui.add(Label::new(description).wrap());
            }
            let metadata = [mime_type.clone(), size.map(|size| format!("{size} bytes"))]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            if !metadata.is_empty() {
                ui.label(RichText::new(metadata).small().weak());
            }
        }
        DisplayContent::TextResource {
            uri,
            mime_type,
            text,
        } => {
            ui.label(mime_type.as_deref().unwrap_or("Text resource"));
            ui.add(Label::new(RichText::new(uri).monospace().small()).wrap());
            ui.add(Label::new(RichText::new(text).monospace().small()).wrap());
        }
        DisplayContent::BlobResource {
            uri,
            mime_type,
            encoded_bytes,
        } => {
            ui.label(format!(
                "Binary resource · {} · {encoded_bytes} encoded bytes",
                mime_type.as_deref().unwrap_or("unknown type")
            ));
            ui.add(Label::new(RichText::new(uri).monospace().small()).wrap());
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AgentDiffKind {
    Context,
    Removed,
    Added,
    Omitted,
}

#[derive(Clone, Debug)]
struct AgentDiffLine {
    old_number: Option<usize>,
    new_number: Option<usize>,
    text: String,
    kind: AgentDiffKind,
    omitted: usize,
}

#[derive(Debug)]
struct AgentDiff {
    lines: Vec<AgentDiffLine>,
    preview: Option<Vec<AgentDiffLine>>,
    added: usize,
    removed: usize,
    old_line_count: usize,
    new_line_count: usize,
}

fn build_agent_diff(old_text: Option<&str>, new_text: &str) -> AgentDiff {
    let old = old_text.map_or_else(Vec::new, |text| text.lines().collect::<Vec<_>>());
    let new = new_text.lines().collect::<Vec<_>>();
    let mut lines = Vec::with_capacity(old.len() + new.len());

    if old_text.is_none() {
        for (index, text) in new.iter().enumerate() {
            lines.push(AgentDiffLine {
                old_number: None,
                new_number: Some(index + 1),
                text: (*text).to_owned(),
                kind: AgentDiffKind::Added,
                omitted: 0,
            });
        }
    } else if (old.len() + 1).saturating_mul(new.len() + 1) <= 250_000 {
        // ponytail: bounded LCS keeps normal tool diffs precise; large file replacements use the
        // linear fallback below instead of spending a frame on a quadratic comparison.
        let width = new.len() + 1;
        let mut common = vec![0_u32; (old.len() + 1) * width];
        for old_index in (0..old.len()).rev() {
            for new_index in (0..new.len()).rev() {
                common[old_index * width + new_index] = if old[old_index] == new[new_index] {
                    common[(old_index + 1) * width + new_index + 1] + 1
                } else {
                    common[(old_index + 1) * width + new_index]
                        .max(common[old_index * width + new_index + 1])
                };
            }
        }
        let (mut old_index, mut new_index) = (0, 0);
        while old_index < old.len() && new_index < new.len() {
            if old[old_index] == new[new_index] {
                lines.push(AgentDiffLine {
                    old_number: Some(old_index + 1),
                    new_number: Some(new_index + 1),
                    text: old[old_index].to_owned(),
                    kind: AgentDiffKind::Context,
                    omitted: 0,
                });
                old_index += 1;
                new_index += 1;
            } else if common[(old_index + 1) * width + new_index]
                >= common[old_index * width + new_index + 1]
            {
                lines.push(AgentDiffLine {
                    old_number: Some(old_index + 1),
                    new_number: None,
                    text: old[old_index].to_owned(),
                    kind: AgentDiffKind::Removed,
                    omitted: 0,
                });
                old_index += 1;
            } else {
                lines.push(AgentDiffLine {
                    old_number: None,
                    new_number: Some(new_index + 1),
                    text: new[new_index].to_owned(),
                    kind: AgentDiffKind::Added,
                    omitted: 0,
                });
                new_index += 1;
            }
        }
        while old_index < old.len() {
            lines.push(AgentDiffLine {
                old_number: Some(old_index + 1),
                new_number: None,
                text: old[old_index].to_owned(),
                kind: AgentDiffKind::Removed,
                omitted: 0,
            });
            old_index += 1;
        }
        while new_index < new.len() {
            lines.push(AgentDiffLine {
                old_number: None,
                new_number: Some(new_index + 1),
                text: new[new_index].to_owned(),
                kind: AgentDiffKind::Added,
                omitted: 0,
            });
            new_index += 1;
        }
    } else {
        let prefix = old
            .iter()
            .zip(&new)
            .take_while(|(before, after)| before == after)
            .count();
        let suffix = old[prefix..]
            .iter()
            .rev()
            .zip(new[prefix..].iter().rev())
            .take_while(|(before, after)| before == after)
            .count();
        for (index, text) in old.iter().take(prefix).enumerate() {
            lines.push(AgentDiffLine {
                old_number: Some(index + 1),
                new_number: Some(index + 1),
                text: (*text).to_owned(),
                kind: AgentDiffKind::Context,
                omitted: 0,
            });
        }
        for (index, text) in old[prefix..old.len() - suffix].iter().enumerate() {
            lines.push(AgentDiffLine {
                old_number: Some(prefix + index + 1),
                new_number: None,
                text: (*text).to_owned(),
                kind: AgentDiffKind::Removed,
                omitted: 0,
            });
        }
        for (index, text) in new[prefix..new.len() - suffix].iter().enumerate() {
            lines.push(AgentDiffLine {
                old_number: None,
                new_number: Some(prefix + index + 1),
                text: (*text).to_owned(),
                kind: AgentDiffKind::Added,
                omitted: 0,
            });
        }
        for index in 0..suffix {
            let old_index = old.len() - suffix + index;
            let new_index = new.len() - suffix + index;
            lines.push(AgentDiffLine {
                old_number: Some(old_index + 1),
                new_number: Some(new_index + 1),
                text: old[old_index].to_owned(),
                kind: AgentDiffKind::Context,
                omitted: 0,
            });
        }
    }

    let added = lines
        .iter()
        .filter(|line| line.kind == AgentDiffKind::Added)
        .count();
    let removed = lines
        .iter()
        .filter(|line| line.kind == AgentDiffKind::Removed)
        .count();
    let mut compact = Vec::with_capacity(lines.len());
    let mut start = 0;
    while start < lines.len() {
        if lines[start].kind != AgentDiffKind::Context {
            compact.push(lines[start].clone());
            start += 1;
            continue;
        }
        let mut end = start + 1;
        while end < lines.len() && lines[end].kind == AgentDiffKind::Context {
            end += 1;
        }
        if end - start > 6 {
            compact.extend_from_slice(&lines[start..start + 3]);
            compact.push(AgentDiffLine {
                old_number: None,
                new_number: None,
                text: String::new(),
                kind: AgentDiffKind::Omitted,
                omitted: end - start - 6,
            });
            compact.extend_from_slice(&lines[end - 3..end]);
        } else {
            compact.extend_from_slice(&lines[start..end]);
        }
        start = end;
    }

    let preview = agent_diff_preview(&compact);
    AgentDiff {
        lines: compact,
        preview,
        added,
        removed,
        old_line_count: old.len(),
        new_line_count: new.len(),
    }
}

#[derive(Clone, Default)]
struct AgentDiffCache {
    old_text: Option<String>,
    new_text: String,
    diff: Option<Arc<AgentDiff>>,
}

fn cached_agent_diff(
    ui: &mut egui::Ui,
    id: Id,
    old_text: Option<&str>,
    new_text: &str,
) -> Arc<AgentDiff> {
    let cache_id = id.with("model");
    let stale = ui.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<AgentDiffCache>(cache_id);
        cache.diff.is_none() || cache.old_text.as_deref() != old_text || cache.new_text != new_text
    });
    if stale {
        let diff = Arc::new(build_agent_diff(old_text, new_text));
        ui.data_mut(|data| {
            let cache = data.get_temp_mut_or_default::<AgentDiffCache>(cache_id);
            cache.old_text = old_text.map(str::to_owned);
            cache.new_text.clear();
            cache.new_text.push_str(new_text);
            cache.diff = Some(diff);
        });
    }
    ui.data_mut(|data| {
        Arc::clone(
            data.get_temp_mut_or_default::<AgentDiffCache>(cache_id)
                .diff
                .as_ref()
                .expect("agent diff was cached"),
        )
    })
}

fn agent_diff_preview(lines: &[AgentDiffLine]) -> Option<Vec<AgentDiffLine>> {
    if lines.len() <= AGENT_DIFF_PREVIEW_ROWS {
        return None;
    }
    let tail = AGENT_DIFF_PREVIEW_ROWS - AGENT_DIFF_PREVIEW_HEAD - 1;
    let mut preview = Vec::with_capacity(AGENT_DIFF_PREVIEW_ROWS);
    preview.extend_from_slice(&lines[..AGENT_DIFF_PREVIEW_HEAD]);
    preview.push(AgentDiffLine {
        old_number: None,
        new_number: None,
        text: String::new(),
        kind: AgentDiffKind::Omitted,
        omitted: lines.len() - AGENT_DIFF_PREVIEW_HEAD - tail,
    });
    preview.extend_from_slice(&lines[lines.len() - tail..]);
    Some(preview)
}

fn tool_contains_diff(tool: &crate::agent::controller::ToolActivity) -> bool {
    tool.detail.as_ref().is_some_and(|detail| {
        detail
            .content
            .iter()
            .any(|content| matches!(content, ToolOutput::Diff { .. }))
    })
}

fn agent_tool_status(status: Option<&str>) -> (&'static str, Color32) {
    match status.unwrap_or_default() {
        "Completed" => ("Done", Color32::from_rgb(105, 210, 157)),
        "InProgress" | "Pending" => ("Running", Color32::from_rgb(93, 204, 225)),
        "Failed" => ("Failed", Color32::from_rgb(238, 132, 139)),
        _ => ("", Color32::from_rgb(145, 153, 168)),
    }
}

fn paint_agent_disclosure(ui: &mut egui::Ui, openness: f32, response: &egui::Response) {
    let center = response.rect.center() + egui::vec2(0.0, 1.0);
    let rotation = egui::emath::Rot2::from_angle(openness * std::f32::consts::FRAC_PI_2);
    let points = [
        egui::vec2(-2.0, -3.0),
        egui::vec2(-2.0, 3.0),
        egui::vec2(2.5, 0.0),
    ]
    .into_iter()
    .map(|point| center + rotation * point)
    .collect();
    ui.painter().add(egui::Shape::convex_polygon(
        points,
        ui.style().interact(response).fg_stroke.color,
        egui::Stroke::NONE,
    ));
}

fn paint_cursor_mark(painter: &egui::Painter, rect: egui::Rect) {
    let center = rect.center();
    let top = center + egui::vec2(0.0, -7.0);
    let upper_right = center + egui::vec2(6.1, -3.5);
    let lower_right = center + egui::vec2(6.1, 3.5);
    let bottom = center + egui::vec2(0.0, 7.0);
    let lower_left = center + egui::vec2(-6.1, 3.5);
    let upper_left = center + egui::vec2(-6.1, -3.5);
    let color = Color32::from_rgb(238, 240, 246);
    for points in [
        vec![top, upper_right, upper_left],
        vec![upper_right, lower_right, bottom],
        vec![upper_left, center, bottom, lower_left],
    ] {
        painter.add(egui::Shape::convex_polygon(
            points,
            color,
            egui::Stroke::NONE,
        ));
    }
}

fn draw_cursor_identity(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(15.0, 18.0), Sense::hover());
        paint_cursor_mark(ui.painter(), rect);
        ui.label(
            RichText::new("Cursor")
                .size(13.0)
                .strong()
                .color(Color32::from_rgb(238, 240, 246)),
        );
    });
    ui.add_space(2.0);
}

#[derive(Clone, Default)]
struct AgentMarkdownCache {
    source: String,
    width: u32,
    galley: Option<Arc<egui::Galley>>,
}

fn agent_markdown_galley(ui: &mut egui::Ui, id: Id, source: &str, width: f32) -> Arc<egui::Galley> {
    let width_key = width.round().to_bits();
    let stale = ui.data_mut(|data| {
        let cache = data.get_temp_mut_or_default::<AgentMarkdownCache>(id);
        cache.galley.is_none() || cache.width != width_key || cache.source != source
    });
    if stale {
        let galley =
            ui.fonts_mut(|fonts| fonts.layout_job(markdown::compact_layout(source, width)));
        ui.data_mut(|data| {
            let cache = data.get_temp_mut_or_default::<AgentMarkdownCache>(id);
            cache.source.clear();
            cache.source.push_str(source);
            cache.width = width_key;
            cache.galley = Some(galley);
        });
    }
    ui.data_mut(|data| {
        Arc::clone(
            data.get_temp_mut_or_default::<AgentMarkdownCache>(id)
                .galley
                .as_ref()
                .expect("agent Markdown was cached"),
        )
    })
}

fn agent_collapsing_header(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    title: &str,
    status: Option<&str>,
    width: f32,
    default_open: bool,
    add_body: impl FnOnce(&mut egui::Ui),
) {
    let id = ui.make_persistent_id(id_salt);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    let title_line = title
        .lines()
        .next()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Tool activity");
    let frame = egui::Frame::new()
        .fill(Color32::from_rgb(25, 26, 29))
        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(42, 44, 50)))
        .corner_radius(5);
    let content_width = (width - frame.total_margin().sum().x).max(0.0);
    frame.show(ui, |ui| {
        ui.set_width(content_width);
        let card_right = ui.max_rect().right();
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 8,
                right: 8,
                top: 8,
                bottom: 6,
            })
            .show(ui, |ui| {
                let (label, color) = agent_tool_status(status);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.spacing_mut().icon_width = 24.0;
                    state.show_toggle_button(ui, paint_agent_disclosure);
                    let status_width = if label.is_empty() { 0.0 } else { 52.0 };
                    let status_spacing = if label.is_empty() {
                        0.0
                    } else {
                        ui.spacing().item_spacing.x
                    };
                    let title_width =
                        (ui.available_width() - status_width - status_spacing).max(0.0);
                    let response = ui
                        .allocate_ui_with_layout(
                            egui::vec2(title_width, 24.0),
                            Layout::left_to_right(Align::Center),
                            |ui| {
                                ui.set_width(title_width);
                                ui.add(
                                    Label::new(
                                        RichText::new(title_line)
                                            .size(13.5)
                                            .color(Color32::from_rgb(211, 216, 226)),
                                    )
                                    .truncate()
                                    .sense(Sense::click()),
                                )
                            },
                        )
                        .inner
                        .on_hover_text(title);
                    if response.clicked() {
                        state.toggle(ui);
                    }
                    if !label.is_empty() {
                        ui.add_sized(
                            egui::vec2(status_width, 24.0),
                            Label::new(RichText::new(label).size(11.5).color(color)),
                        );
                    }
                });
            });
        state.show_body_unindented(ui, |ui| {
            ui.set_width((card_right - ui.cursor().left()).max(0.0));
            ui.painter().hline(
                ui.available_rect_before_wrap().x_range(),
                ui.cursor().top(),
                egui::Stroke::new(1.0, Color32::from_rgb(43, 45, 51)),
            );
            if default_open {
                add_body(ui);
            } else {
                egui::Frame::new()
                    .inner_margin(egui::Margin::symmetric(9, 8))
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        ui.set_width(width);
                        ui.set_max_width(width);
                        add_body(ui);
                    });
            }
        });
    });
}

fn draw_agent_diff(ui: &mut egui::Ui, id: Id, path: &Path, old_text: Option<&str>, new_text: &str) {
    let diff = cached_agent_diff(ui, id, old_text, new_text);
    let expanded_id = id.with("expanded");
    let expanded = ui.data(|data| data.get_temp::<bool>(expanded_id).unwrap_or(false));
    let lines = (!expanded)
        .then_some(diff.preview.as_deref())
        .flatten()
        .unwrap_or(&diff.lines);
    let can_toggle = diff.lines.len() > AGENT_DIFF_PREVIEW_ROWS;
    let file_name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    ui.set_width(ui.available_width());
    egui::Frame::new()
        .fill(Color32::from_rgb(24, 26, 30))
        .show(ui, |ui| {
            egui::Frame::new()
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(file_name.as_ref())
                                .size(13.5)
                                .strong()
                                .color(Color32::from_rgb(225, 229, 237)),
                        )
                        .on_hover_text(path.display().to_string());
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(
                                RichText::new(format!("+{}  −{}", diff.added, diff.removed))
                                    .monospace()
                                    .size(11.5)
                                    .color(Color32::from_rgb(160, 170, 184)),
                            );
                            ui.label(
                                RichText::new(if old_text.is_some() {
                                    "MODIFIED"
                                } else {
                                    "NEW FILE"
                                })
                                .size(10.5)
                                .strong()
                                .color(Color32::from_rgb(100, 201, 220)),
                            );
                        });
                    });
                });
            ui.painter().hline(
                ui.available_rect_before_wrap().x_range(),
                ui.cursor().top(),
                egui::Stroke::new(1.0, Color32::from_rgb(45, 49, 57)),
            );
            let old_digits = diff.old_line_count.max(1).ilog10() as usize + 1;
            let new_digits = diff.new_line_count.max(1).ilog10() as usize + 1;
            let longest = lines
                .iter()
                .map(|line| line.text.chars().count())
                .max()
                .unwrap_or(0);
            let content_width = ui.available_width();
            let desired_width =
                content_width.max(38.0 + (old_digits + new_digits + longest).min(240) as f32 * 7.3);
            ScrollArea::horizontal()
                .id_salt(id)
                .max_width(content_width)
                .auto_shrink([false, true])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded)
                .show(ui, |ui| {
                    ui.set_width(desired_width);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    for line in lines {
                        let fill = match line.kind {
                            AgentDiffKind::Added => Color32::from_rgb(24, 48, 38),
                            AgentDiffKind::Removed => Color32::from_rgb(55, 31, 37),
                            AgentDiffKind::Omitted => Color32::from_rgb(27, 31, 37),
                            AgentDiffKind::Context => Color32::TRANSPARENT,
                        };
                        egui::Frame::new()
                            .fill(fill)
                            .inner_margin(egui::Margin::symmetric(9, 3))
                            .show(ui, |ui| {
                                ui.set_min_width(desired_width - 18.0);
                                if line.kind == AgentDiffKind::Omitted {
                                    ui.label(
                                        RichText::new(format!("⋯  {} lines hidden", line.omitted))
                                            .monospace()
                                            .size(11.5)
                                            .color(Color32::from_rgb(132, 142, 157)),
                                    );
                                    return;
                                }
                                let old_number = line.old_number.map_or_else(
                                    || " ".repeat(old_digits),
                                    |number| format!("{number:>old_digits$}"),
                                );
                                let new_number = line.new_number.map_or_else(
                                    || " ".repeat(new_digits),
                                    |number| format!("{number:>new_digits$}"),
                                );
                                let (sign, color) = match line.kind {
                                    AgentDiffKind::Added => ("+", Color32::from_rgb(143, 224, 177)),
                                    AgentDiffKind::Removed => {
                                        ("−", Color32::from_rgb(239, 157, 164))
                                    }
                                    AgentDiffKind::Context => {
                                        (" ", Color32::from_rgb(198, 204, 215))
                                    }
                                    AgentDiffKind::Omitted => unreachable!(),
                                };
                                let mut job = LayoutJob::default();
                                job.append(
                                    &format!("{old_number} {new_number}  "),
                                    0.0,
                                    TextFormat {
                                        font_id: FontId::monospace(12.0),
                                        color: Color32::from_rgb(111, 121, 137),
                                        ..TextFormat::default()
                                    },
                                );
                                job.append(
                                    &format!("{sign} {}", line.text),
                                    0.0,
                                    TextFormat {
                                        font_id: FontId::monospace(12.0),
                                        color,
                                        ..TextFormat::default()
                                    },
                                );
                                ui.add(
                                    Label::new(job)
                                        .wrap_mode(egui::TextWrapMode::Extend)
                                        .selectable(true),
                                );
                            });
                    }
                });
            if can_toggle {
                let label = if expanded {
                    "Collapse diff".to_owned()
                } else {
                    format!("Show all {} lines", diff.lines.len())
                };
                let toggle = ui.add_sized(
                    egui::vec2(ui.available_width(), 40.0),
                    egui::Button::new(
                        RichText::new(label)
                            .size(11.5)
                            .color(Color32::from_rgb(111, 194, 211)),
                    )
                    .frame(false),
                );
                if toggle.clicked() {
                    ui.data_mut(|data| data.insert_temp(expanded_id, !expanded));
                    ui.ctx().request_repaint();
                }
            }
        });
}

fn split_workspace(
    content: egui::Rect,
    explorer_open: bool,
    explorer_width: f32,
    agent_open: bool,
    agent_width: f32,
) -> (Option<egui::Rect>, egui::Rect, egui::Rect) {
    let right_width = if agent_open {
        agent_width.max(320.0).min(content.width() * 0.52)
    } else {
        0.0
    };
    let explorer_width = explorer_width
        .max(120.0)
        .min((content.width() - right_width - 160.0).max(120.0));
    let explorer = explorer_open
        .then(|| content.with_max_x((content.left() + explorer_width).min(content.right())));
    let agent = content.with_min_x((content.right() - right_width).max(content.left()));
    let editor_right = if agent_open {
        agent.left()
    } else {
        content.right()
    };
    let editor = egui::Rect::from_min_max(
        egui::pos2(
            explorer.map_or(content.left(), |rect| rect.right()),
            content.top(),
        ),
        egui::pos2(editor_right, content.bottom()),
    );
    (explorer, editor, agent)
}

fn split_editor_column(rect: egui::Rect, find_open: bool) -> (egui::Rect, Option<egui::Rect>) {
    let editor_top = (rect.top() + TITLEBAR_HEIGHT).min(rect.bottom());
    let findbar = find_open.then(|| {
        egui::Rect::from_min_max(
            egui::pos2(
                rect.left(),
                (rect.bottom() - FIND_BAR_HEIGHT).max(editor_top),
            ),
            rect.right_bottom(),
        )
    });
    let editor = egui::Rect::from_min_max(
        egui::pos2(rect.left(), editor_top),
        egui::pos2(
            rect.right(),
            findbar.map_or(rect.bottom(), |findbar| findbar.top()),
        ),
    );
    (editor, findbar)
}

fn titlebar_drag_action(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    region: &'static str,
) -> Option<WindowAction> {
    if rect.width() <= 0.0 {
        return None;
    }
    let response = ui.interact(
        rect,
        Id::new(("titlebar_drag", region)),
        Sense::click_and_drag(),
    );
    if response.drag_started() {
        Some(WindowAction::Drag)
    } else if response.double_clicked() {
        Some(WindowAction::ToggleMaximize)
    } else {
        None
    }
}

fn agent_composer_height(text_height: f32, row_height: f32, sidebar_height: f32) -> f32 {
    let max_height =
        AGENT_COMPOSER_MAX_HEIGHT.min((sidebar_height * 0.45).max(AGENT_COMPOSER_HEIGHT));
    (AGENT_COMPOSER_HEIGHT + (text_height - row_height * 3.0).max(0.0)).min(max_height)
}

fn split_agent_sidebar(
    rect: egui::Rect,
    composer_height: f32,
) -> (egui::Rect, egui::Rect, egui::Rect) {
    let header = rect.with_max_y((rect.top() + AGENT_HEADER_HEIGHT).min(rect.bottom()));
    let composer = rect.with_min_y(
        (rect.bottom() - composer_height)
            .max(header.bottom())
            .min(rect.bottom()),
    );
    let transcript = egui::Rect::from_min_max(header.left_bottom(), composer.right_top());
    (header, transcript, composer)
}

fn agent_toggle_rect(header: egui::Rect) -> egui::Rect {
    #[cfg(target_os = "macos")]
    let controls_right = header.right();
    #[cfg(not(target_os = "macos"))]
    let controls_right = header.right() - 3.0 * 46.0;
    egui::Rect::from_center_size(
        egui::pos2(controls_right - 17.0, header.center().y),
        egui::vec2(34.0, header.height()),
    )
}

fn agent_new_session_rect(header: egui::Rect) -> egui::Rect {
    let toggle = agent_toggle_rect(header);
    egui::Rect::from_center_size(
        egui::pos2(toggle.center().x - 33.0, header.center().y),
        egui::vec2(32.0, 32.0),
    )
}

fn agent_sessions_rect(header: egui::Rect) -> egui::Rect {
    let new_session = agent_new_session_rect(header);
    egui::Rect::from_center_size(
        egui::pos2(new_session.center().x - 33.0, header.center().y),
        egui::vec2(32.0, 32.0),
    )
}

fn agent_composer_content(composer: egui::Rect) -> egui::Rect {
    composer.shrink2(egui::vec2(14.0, 10.0))
}

fn agent_menu_rect(
    transcript: egui::Rect,
    anchor: egui::Rect,
    item_count: usize,
    row_height: f32,
) -> egui::Rect {
    let width = AGENT_MENU_WIDTH.min((transcript.width() - 12.0).max(1.0));
    let desired_height = 16.0 + (item_count as f32 * row_height).min(280.0);
    let bottom = anchor.top() - 4.0;
    let height = desired_height.min((bottom - transcript.top() - 8.0).max(1.0));
    let left = anchor.left().clamp(
        transcript.left() + 6.0,
        (transcript.right() - width - 6.0).max(transcript.left() + 6.0),
    );
    egui::Rect::from_min_size(egui::pos2(left, bottom - height), egui::vec2(width, height))
}

fn agent_session_menu_rect(
    transcript: egui::Rect,
    anchor: egui::Rect,
    item_count: usize,
) -> egui::Rect {
    let width = AGENT_MENU_WIDTH.min((transcript.width() - 12.0).max(1.0));
    let top = anchor.bottom() + 4.0;
    let height = (16.0 + item_count as f32 * AGENT_SESSION_ROW_HEIGHT)
        .min(280.0)
        .min((transcript.bottom() - top - 8.0).max(1.0));
    let left = (anchor.right() - width).clamp(
        transcript.left() + 6.0,
        (transcript.right() - width - 6.0).max(transcript.left() + 6.0),
    );
    egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(width, height))
}

fn slash_command_query(prompt: &str) -> Option<&str> {
    prompt
        .strip_prefix('/')
        .filter(|query| !query.chars().any(char::is_whitespace))
}

fn command_matches(name: &str, query: &str) -> bool {
    name.to_ascii_lowercase()
        .starts_with(&query.to_ascii_lowercase())
}

fn run_everything_state(commands: &[crate::agent::controller::CommandChoice]) -> Option<bool> {
    let command = commands
        .iter()
        .find(|command| matches!(command.name.as_str(), "run-everything" | "auto-run"))?;
    let description = command.description.to_ascii_lowercase();
    (!description.contains("disabled by admin") && !description.contains("checking"))
        .then(|| description.contains("currently enabled"))
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum AgentMenu {
    Sessions,
    Commands(String),
    Permissions,
    Mode,
    Config(String),
}

struct TreeState {
    root: PathBuf,
    children: HashMap<PathBuf, Vec<TreeEntry>>,
    expanded: HashSet<PathBuf>,
    selected: Option<PathBuf>,
    visible: Vec<TreeRow>,
    selected_index: Option<usize>,
}

impl TreeState {
    fn new(root: PathBuf, selected: Option<PathBuf>) -> Result<Self, String> {
        let root_entries = read_directory(&root)?;
        let mut state = Self {
            children: HashMap::from([(root.clone(), root_entries)]),
            root,
            expanded: HashSet::new(),
            selected,
            visible: Vec::new(),
            selected_index: None,
        };
        state.refresh_visible();
        Ok(state)
    }

    fn refresh_visible(&mut self) {
        self.visible.clear();
        Self::append_visible(
            &self.children,
            &self.expanded,
            &self.root,
            0,
            &mut self.visible,
        );
        self.selected_index = self.selected.as_ref().and_then(|selected| {
            self.visible
                .iter()
                .position(|row| &row.entry.path == selected)
        });
    }

    fn append_visible(
        children: &HashMap<PathBuf, Vec<TreeEntry>>,
        expanded: &HashSet<PathBuf>,
        directory: &Path,
        depth: usize,
        result: &mut Vec<TreeRow>,
    ) {
        let Some(entries) = children.get(directory) else {
            return;
        };
        for entry in entries {
            let mut hasher = DefaultHasher::new();
            entry.path.hash(&mut hasher);
            entry.name.hash(&mut hasher);
            result.push(TreeRow {
                entry: entry.clone(),
                label: entry.name.to_string_lossy().into_owned(),
                depth,
                directory: entry.is_dir,
                expanded: expanded.contains(&entry.path),
                revision: hasher.finish(),
            });
            if entry.is_dir && expanded.contains(&entry.path) {
                Self::append_visible(children, expanded, &entry.path, depth + 1, result);
            }
        }
    }

    fn toggle(&mut self, path: &Path) -> Result<(), String> {
        if self.expanded.remove(path) {
            self.refresh_visible();
            return Ok(());
        }
        if !self.children.contains_key(path) {
            self.children
                .insert(path.to_path_buf(), read_directory(path)?);
        }
        self.expanded.insert(path.to_path_buf());
        self.refresh_visible();
        Ok(())
    }

    fn collapse(&mut self, path: &Path) {
        if self.expanded.remove(path) {
            self.refresh_visible();
        }
    }

    fn select(&mut self, path: Option<PathBuf>) {
        self.selected_index = path
            .as_ref()
            .and_then(|path| self.visible.iter().position(|row| row.entry.path == *path));
        self.selected = path;
    }
}

#[derive(Default)]
struct HighlightCache {
    revision: u64,
    syntax: String,
    job: LayoutJob,
    valid: bool,
    incremental: IncrementalHighlightCache,
    find_revision: u64,
    find_query: String,
    find_selected: usize,
    find_job: LayoutJob,
    find_valid: bool,
    bracket_job: Option<LayoutJob>,
    galley_key: Option<GalleyKey>,
    presentation_revision: u64,
}

#[derive(Clone, PartialEq)]
struct GalleyKey {
    revision: u64,
    syntax: String,
    wrap_width: u32,
    find: Option<(String, usize)>,
    bracket_pair: Option<(std::ops::Range<usize>, std::ops::Range<usize>)>,
}

struct FileTab {
    buffer: Buffer,
    editor_surface: EditorSurface,
}

impl FileTab {
    fn new(buffer: Buffer) -> Self {
        Self {
            buffer,
            editor_surface: EditorSurface::default(),
        }
    }
}

pub struct EditorApp {
    tabs: Vec<FileTab>,
    active_tab: Option<usize>,
    tree: TreeState,
    tree_surface: TreeSurface,
    syntaxes: SyntaxManager,
    highlighter: Highlighter,
    highlight_cache: HighlightCache,
    search: SearchController,
    search_open: bool,
    search_query: String,
    search_selected: usize,
    focus_search: bool,
    find_open: bool,
    find_query: String,
    focus_find: bool,
    find_matches: Vec<std::ops::Range<usize>>,
    find_match_revision: u64,
    find_match_query: String,
    find_selected: usize,
    scroll_to_find_match: bool,
    bracket_pair: Option<(std::ops::Range<usize>, std::ops::Range<usize>)>,
    bracket_pair_key: Option<(u64, usize)>,
    markdown_preview: bool,
    markdown_layout: Option<((u64, u32), Arc<egui::Galley>)>,
    sidebar: bool,
    sidebar_width: f32,
    sidebar_dragging: bool,
    agent_sidebar: bool,
    agent_sidebar_width: f32,
    agent_sidebar_dragging: bool,
    agent_menu: Option<AgentMenu>,
    agent_menu_popup: Option<egui::Rect>,
    agent_menu_scroll_y: f32,
    agent_follow_transcript: bool,
    agent_prompt_history_index: Option<usize>,
    agent_prompt_history_draft: String,
    agent_run_everything: Option<bool>,
    scrollbar_activity: crate::scrollbar::Activity,
    agent: AgentState,
    agent_controller: Option<AgentController>,
    pending_agent_prompt: bool,
    last_agent_reconcile: Instant,
    focus_editor: bool,
    tree_focused: bool,
    cursor: (usize, usize),
    pending: Option<PendingAction>,
    conflict: bool,
    save_as: Option<String>,
    error: Option<String>,
    should_close: bool,
    window_action: Option<WindowAction>,
}

impl EditorApp {
    pub fn new(target: OpenTarget) -> Result<Self, String> {
        let buffer = target.file.as_deref().map_or(Ok(None), |path| {
            if target.create {
                Ok(Some(Buffer::new(path.to_path_buf())))
            } else {
                load_buffer(path).map(Some)
            }
        })?;
        let selected = buffer.as_ref().map(|buffer| buffer.path.clone());
        let tabs = buffer.into_iter().map(FileTab::new).collect::<Vec<_>>();
        let active_tab = (!tabs.is_empty()).then_some(0);
        let syntaxes = data_dir()
            .and_then(|directory| SyntaxManager::load(&directory))
            .or_else(|_| SyntaxManager::built_in())?;
        let search = SearchController::new(target.root.clone())?;
        Ok(Self {
            tabs,
            active_tab,
            tree: TreeState::new(target.root, selected)?,
            tree_surface: TreeSurface::default(),
            syntaxes,
            highlighter: Highlighter::new()?,
            highlight_cache: HighlightCache::default(),
            search,
            search_open: false,
            search_query: String::new(),
            search_selected: 0,
            focus_search: false,
            find_open: false,
            find_query: String::new(),
            focus_find: false,
            find_matches: Vec::new(),
            find_match_revision: u64::MAX,
            find_match_query: String::new(),
            find_selected: 0,
            scroll_to_find_match: false,
            bracket_pair: None,
            bracket_pair_key: None,
            markdown_preview: false,
            markdown_layout: None,
            sidebar: true,
            sidebar_width: 248.0,
            sidebar_dragging: false,
            agent_sidebar: false,
            agent_sidebar_width: 440.0,
            agent_sidebar_dragging: false,
            agent_menu: None,
            agent_menu_popup: None,
            agent_menu_scroll_y: 0.0,
            agent_follow_transcript: true,
            agent_prompt_history_index: None,
            agent_prompt_history_draft: String::new(),
            agent_run_everything: None,
            scrollbar_activity: crate::scrollbar::Activity::default(),
            agent: AgentState::default(),
            agent_controller: None,
            pending_agent_prompt: false,
            last_agent_reconcile: Instant::now(),
            focus_editor: target.file.is_some(),
            tree_focused: target.file.is_none(),
            cursor: (1, 1),
            pending: None,
            conflict: false,
            save_as: None,
            error: None,
            should_close: false,
            window_action: None,
        })
    }

    pub fn request_close(&mut self) {
        self.request(PendingAction::Close);
    }

    fn request_target(&mut self, target: OpenTarget) {
        if target.root == self.tree.root {
            if let Some(path) = target.file {
                self.open_tab(path, target.create);
            }
        } else {
            self.request(PendingAction::OpenTarget(target));
        }
    }

    fn buffer(&self) -> Option<&Buffer> {
        self.active_tab
            .and_then(|index| self.tabs.get(index))
            .map(|tab| &tab.buffer)
    }

    fn activate_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let changed = self.active_tab != Some(index);
        self.active_tab = Some(index);
        self.tree.select(Some(self.tabs[index].buffer.path.clone()));
        if changed {
            self.highlight_cache = HighlightCache::default();
            self.find_match_revision = u64::MAX;
            self.scroll_to_find_match = self.find_open;
            self.bracket_pair = None;
            self.bracket_pair_key = None;
            self.markdown_preview = false;
            self.markdown_layout = None;
            self.cursor = self.tabs[index]
                .buffer
                .line_column(self.tabs[index].editor_surface.cursor());
        }
        self.focus_editor = true;
        self.tree_focused = false;
    }

    fn open_tab(&mut self, path: PathBuf, create: bool) {
        if let Some(index) = self.tabs.iter().position(|tab| tab.buffer.path == path) {
            self.activate_tab(index);
            return;
        }
        let buffer = if create {
            Ok(Buffer::new(path.clone()))
        } else {
            load_buffer(&path)
        };
        match buffer {
            Ok(buffer) => {
                self.tabs.push(FileTab::new(buffer));
                self.activate_tab(self.tabs.len() - 1);
            }
            Err(error) => self.show_error(error),
        }
    }

    fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        let active = self.active_tab;
        self.tabs.remove(index);
        match active {
            Some(active) if active == index => {
                self.active_tab = None;
                if !self.tabs.is_empty() {
                    self.activate_tab(index.min(self.tabs.len() - 1));
                } else {
                    self.tree.select(None);
                    self.cursor = (1, 1);
                    self.markdown_preview = false;
                    self.bracket_pair = None;
                    self.bracket_pair_key = None;
                    self.markdown_layout = None;
                }
            }
            Some(active) if active > index => self.active_tab = Some(active - 1),
            _ => {}
        }
    }

    fn move_tab(&mut self, from: usize, to: usize) {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return;
        }
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active_tab = self.active_tab.map(|active| {
            if active == from {
                to
            } else if from < active && active <= to {
                active - 1
            } else if to <= active && active < from {
                active + 1
            } else {
                active
            }
        });
    }

    fn request(&mut self, action: PendingAction) {
        let dirty = match &action {
            PendingAction::Open(_) => None,
            PendingAction::CloseTab(index) => self
                .tabs
                .get(*index)
                .filter(|tab| tab.buffer.dirty)
                .map(|_| *index),
            PendingAction::OpenTarget(_) | PendingAction::Close => {
                self.tabs.iter().position(|tab| tab.buffer.dirty)
            }
        };
        if let Some(index) = dirty {
            self.activate_tab(index);
            self.pending = Some(action);
            return;
        }
        self.perform(action);
    }

    fn perform(&mut self, action: PendingAction) {
        match action {
            PendingAction::Open(path) => self.open_tab(path, false),
            PendingAction::OpenTarget(target) => match Self::new(target) {
                Ok(editor) => *self = editor,
                Err(error) => self.show_error(error),
            },
            PendingAction::CloseTab(index) => self.close_tab(index),
            PendingAction::Close => self.should_close = true,
        }
    }

    fn save(&mut self, destination: Option<PathBuf>) -> bool {
        let Some(index) = self.active_tab else {
            return true;
        };
        let buffer = &mut self.tabs[index].buffer;
        let save_as = destination.is_some();
        let path = destination.unwrap_or_else(|| buffer.path.clone());
        match safe_save(buffer, &path) {
            Ok(()) => true,
            Err(SaveError::Conflict) => {
                if save_as {
                    self.show_error(format!(
                        "cannot save as {} because it already exists or changed",
                        path.display()
                    ));
                } else {
                    self.conflict = true;
                }
                false
            }
            Err(error) => {
                self.show_error(format!("cannot save {}: {error}", path.display()));
                false
            }
        }
    }

    fn finish_pending(&mut self) {
        if let Some(action) = self.pending.take() {
            self.request(action);
        }
    }

    fn discard_pending(&mut self) {
        let Some(action) = self.pending.take() else {
            return;
        };
        match action {
            PendingAction::CloseTab(index) => self.close_tab(index),
            PendingAction::OpenTarget(_) | PendingAction::Close => {
                if let Some(index) = self.active_tab {
                    self.close_tab(index);
                }
                self.request(action);
            }
            PendingAction::Open(path) => self.open_tab(path, false),
        }
    }

    fn show_error(&mut self, error: String) {
        eprintln!("editur: {error}");
        self.error = Some(error);
    }

    pub fn ui(&mut self, root: &mut egui::Ui) {
        root.style_mut().animation_time = 0.0;
        self.scrollbar_activity.style_egui(root);
        let ctx = root.ctx().clone();
        self.poll_agent(&ctx);
        if self.agent.active {
            ctx.request_repaint_after(Duration::from_millis(500));
            if self.last_agent_reconcile.elapsed() >= Duration::from_millis(500) {
                self.reconcile_open_buffer();
                self.last_agent_reconcile = Instant::now();
            }
        }
        self.shortcuts(&ctx);
        if self.find_open {
            self.refresh_find_matches();
        }
        if self.search_open {
            self.search.poll(&self.search_query);
            let results = self.search.results();
            if search_needs_polling(&self.search_query, &results.query, results.complete) {
                ctx.request_repaint_after(Duration::from_millis(16));
            }
        }

        let window = root.max_rect();
        let agent_sidebar_at_frame_start = self.agent_sidebar;
        let (sidebar, editor_column, agent) = split_workspace(
            window,
            self.sidebar,
            self.sidebar_width,
            self.agent_sidebar,
            self.agent_sidebar_width,
        );
        let (editor, findbar) = split_editor_column(editor_column, self.find_open);
        if let Some(sidebar) = sidebar {
            root.scope_builder(
                UiBuilder::new().id_salt("sidebar").max_rect(sidebar),
                |ui| self.draw_sidebar(ui),
            );
        }
        root.scope_builder(
            UiBuilder::new().id_salt("editor_surface").max_rect(editor),
            |ui| self.draw_editor(ui),
        );
        if let Some(findbar) = findbar {
            root.scope_builder(
                UiBuilder::new()
                    .id_salt("file_search_bar")
                    .max_rect(findbar),
                |ui| self.draw_find(ui),
            );
        }
        if self.agent_sidebar {
            root.scope_builder(
                UiBuilder::new().id_salt("agent_sidebar").max_rect(agent),
                |ui| self.draw_agent_sidebar(ui),
            );
        }
        self.draw_titlebar(
            root,
            window.with_max_y((window.top() + TITLEBAR_HEIGHT).min(window.bottom())),
            editor,
            agent_sidebar_at_frame_start,
        );
        let divider_stroke_width = ctx.input(|input| input.physical_pixel_size());
        if let Some(sidebar) = sidebar {
            let divider = egui::Rect::from_center_size(
                egui::pos2(sidebar.right(), sidebar.center().y),
                egui::vec2(5.0, sidebar.height()),
            );
            let pointer = ctx.pointer_hover_pos();
            let hovered = pointer.is_some_and(|pointer| divider.contains(pointer));
            if hovered && ctx.input(|input| input.pointer.primary_pressed()) {
                self.sidebar_dragging = true;
            }
            if !ctx.input(|input| input.pointer.primary_down()) {
                self.sidebar_dragging = false;
            }
            if self.sidebar_dragging
                && let Some(pointer) = pointer
            {
                self.sidebar_width = (pointer.x - window.left()).clamp(120.0, 500.0);
                ctx.request_repaint();
            }
            if hovered || self.sidebar_dragging {
                ctx.set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            let active = hovered || self.sidebar_dragging;
            crate::renderer::mark_retained(
                root.painter(),
                divider,
                0x8000_0000_0000_0000,
                u64::from(divider.center().x.to_bits())
                    ^ u64::from(divider.height().to_bits()).rotate_left(32)
                    ^ ((active as u64) << 63),
            );
            root.painter().line_segment(
                [divider.center_top(), divider.center_bottom()],
                egui::Stroke::new(
                    divider_stroke_width,
                    if active {
                        Color32::from_rgb(86, 207, 225)
                    } else {
                        Color32::from_rgb(53, 53, 59)
                    },
                ),
            );
        }
        if self.agent_sidebar {
            let divider = egui::Rect::from_center_size(
                egui::pos2(agent.left(), agent.center().y),
                egui::vec2(5.0, agent.height()),
            );
            let pointer = ctx.pointer_hover_pos();
            let hovered = pointer.is_some_and(|pointer| divider.contains(pointer));
            if hovered && ctx.input(|input| input.pointer.primary_pressed()) {
                self.agent_sidebar_dragging = true;
            }
            if !ctx.input(|input| input.pointer.primary_down()) {
                self.agent_sidebar_dragging = false;
            }
            if self.agent_sidebar_dragging
                && let Some(pointer) = pointer
            {
                self.agent_sidebar_width = (window.right() - pointer.x).clamp(320.0, 720.0);
                ctx.request_repaint();
            }
            if hovered || self.agent_sidebar_dragging {
                ctx.set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            let active = hovered || self.agent_sidebar_dragging;
            crate::renderer::mark_retained(
                root.painter(),
                divider,
                0x9000_0000_0000_0000,
                u64::from(divider.center().x.to_bits())
                    ^ u64::from(divider.height().to_bits()).rotate_left(32)
                    ^ ((active as u64) << 63),
            );
            root.painter().line_segment(
                [divider.center_top(), divider.center_bottom()],
                egui::Stroke::new(
                    divider_stroke_width,
                    if active {
                        Color32::from_rgb(86, 207, 225)
                    } else {
                        Color32::from_rgb(53, 53, 59)
                    },
                ),
            );
        }
        self.draw_search(root);
        self.draw_dialogs(&ctx);
        self.draw_error(&ctx);
    }

    fn draw_titlebar(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        editor: egui::Rect,
        agent_sidebar_open: bool,
    ) {
        crate::renderer::mark_retained(
            ui.painter(),
            rect,
            TITLEBAR_PAINT_KEY,
            ui.ctx().cumulative_frame_nr(),
        );
        let editor_header =
            egui::Rect::from_min_max(egui::pos2(editor.left(), rect.top()), editor.right_top());
        ui.painter()
            .rect_filled(editor_header, 0.0, Color32::from_rgb(24, 24, 26));
        ui.painter().hline(
            editor_header.x_range(),
            editor_header.bottom() - 0.5,
            egui::Stroke::new(1.0, Color32::from_rgb(42, 42, 47)),
        );
        #[cfg(target_os = "macos")]
        let controls_left = editor_header.right();
        #[cfg(not(target_os = "macos"))]
        let controls_left = editor_header.right().min(rect.right() - 3.0 * 46.0);
        let agent_button = (!agent_sidebar_open).then(|| agent_toggle_rect(editor_header));

        let markdown = self
            .buffer()
            .is_some_and(|buffer| markdown::is_markdown(&buffer.path));
        let preview_button = markdown.then(|| {
            let right = agent_button.map_or(controls_left, |button| button.left());
            egui::Rect::from_min_max(
                egui::pos2(right - 66.0, rect.top()),
                egui::pos2(right, rect.bottom()),
            )
        });
        if let Some(button) = preview_button {
            let response = ui
                .interact(button, Id::new("markdown_preview_toggle"), Sense::click())
                .on_hover_text(if self.markdown_preview {
                    "Return to Markdown source"
                } else {
                    "Preview rendered Markdown"
                });
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    if self.markdown_preview {
                        "Edit Markdown"
                    } else {
                        "Preview Markdown"
                    },
                )
            });
            if response.hovered() || self.markdown_preview {
                ui.painter().rect_filled(
                    button.shrink2(egui::vec2(3.0, 3.0)),
                    4.0,
                    Color32::from_rgb(38, 38, 42),
                );
            }
            ui.painter().text(
                button.center(),
                Align2::CENTER_CENTER,
                if self.markdown_preview {
                    "Edit"
                } else {
                    "Preview"
                },
                FontId::proportional(12.0),
                Color32::from_rgb(174, 181, 194),
            );
            if response.clicked() {
                self.markdown_preview = !self.markdown_preview;
                self.focus_editor = !self.markdown_preview;
                ui.ctx().request_repaint();
            }
        }

        let controls_start = preview_button.map_or_else(
            || agent_button.map_or(controls_left, |button| button.left()),
            |button| button.left(),
        );
        #[cfg(target_os = "macos")]
        let tabs_left = if editor_header.left() == rect.left() {
            editor_header.left() + 72.0
        } else {
            editor_header.left()
        };
        #[cfg(not(target_os = "macos"))]
        let tabs_left = editor_header.left();
        let tabs_right = (controls_start - 8.0).max(tabs_left);
        let tabs_used_right = (tabs_left + self.tabs.len() as f32 * TAB_WIDTH).min(tabs_right);
        if tabs_used_right > tabs_left {
            self.draw_file_tabs(
                ui,
                egui::Rect::from_min_max(
                    egui::pos2(tabs_left, editor_header.top()),
                    egui::pos2(tabs_used_right, editor_header.bottom()),
                ),
            );
        }
        let drag_rect = egui::Rect::from_min_max(
            egui::pos2(tabs_used_right, editor_header.top()),
            egui::pos2(controls_start, editor_header.bottom()),
        );
        #[cfg(target_os = "macos")]
        let sidebar_drag_left = rect.left() + 72.0;
        #[cfg(not(target_os = "macos"))]
        let sidebar_drag_left = rect.left();
        let sidebar_drag_right = (editor_header.left() - 3.0).max(sidebar_drag_left);
        let sidebar_drag_rect = egui::Rect::from_min_max(
            egui::pos2(sidebar_drag_left, rect.top()),
            egui::pos2(sidebar_drag_right, rect.bottom()),
        );
        let agent_drag_rect = if agent_sidebar_open {
            let agent_header =
                egui::Rect::from_min_max(editor_header.right_top(), rect.right_bottom());
            egui::Rect::from_min_max(
                egui::pos2(agent_header.left() + 3.0, agent_header.top()),
                egui::pos2(
                    agent_sessions_rect(agent_header).left(),
                    agent_header.bottom(),
                ),
            )
        } else {
            egui::Rect::NOTHING
        };
        for (region, drag_rect) in [
            ("sidebar", sidebar_drag_rect),
            ("editor", drag_rect),
            ("agent", agent_drag_rect),
        ] {
            if let Some(action) = titlebar_drag_action(ui, drag_rect, region) {
                self.window_action = Some(action);
            }
        }

        if agent_button.is_some_and(|button| self.draw_agent_toggle(ui, button)) {
            self.agent_sidebar = true;
            self.agent_sidebar_dragging = false;
            self.open_agent(ui.ctx());
            ui.ctx().request_repaint();
        }

        #[cfg(target_os = "macos")]
        self.draw_macos_titlebar_controls(ui, rect);
        #[cfg(not(target_os = "macos"))]
        self.draw_windows_titlebar_controls(ui, rect);
    }

    fn draw_file_tabs(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let active = self.active_tab;
        let tabs = self
            .tabs
            .iter()
            .map(|tab| {
                let path = &tab.buffer.path;
                let label = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned();
                (label, path.display().to_string(), tab.buffer.dirty)
            })
            .collect::<Vec<_>>();
        let mut activate = None;
        let mut close = None;
        let mut reorder = None;
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("file_tabs")
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.set_clip_rect(rect);
                ScrollArea::horizontal()
                    .id_salt("file_tabs_scroll")
                    .max_width(rect.width())
                    .max_height(rect.height())
                    .auto_shrink([false, false])
                    .content_margin(egui::Margin::ZERO)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                        for (index, (label, path, dirty)) in tabs.iter().enumerate() {
                            let (_, tab) = ui.allocate_space(egui::vec2(TAB_WIDTH, rect.height()));
                            let selected = active == Some(index);
                            let response = ui
                                .interact(tab, Id::new(("file_tab", path)), Sense::click_and_drag())
                                .on_hover_text(path);
                            response.widget_info(|| {
                                egui::WidgetInfo::selected(
                                    egui::WidgetType::SelectableLabel,
                                    ui.is_enabled(),
                                    selected,
                                    label.clone(),
                                )
                            });
                            let hovered = ui.input(|input| {
                                input
                                    .pointer
                                    .hover_pos()
                                    .is_some_and(|pointer| tab.contains(pointer))
                            });
                            let dragging = response.dragged();
                            if selected || hovered || dragging {
                                ui.painter().rect_filled(
                                    tab,
                                    0.0,
                                    if dragging {
                                        Color32::from_rgb(39, 39, 43)
                                    } else if selected {
                                        Color32::from_rgb(31, 31, 34)
                                    } else {
                                        Color32::from_rgb(26, 26, 29)
                                    },
                                );
                            }
                            ui.painter().vline(
                                tab.right() - 0.5,
                                tab.y_range().shrink(5.0),
                                egui::Stroke::new(1.0, Color32::from_rgb(44, 44, 49)),
                            );
                            if selected {
                                ui.painter().hline(
                                    tab.x_range(),
                                    tab.bottom() - 1.0,
                                    egui::Stroke::new(2.0, Color32::from_rgb(86, 207, 225)),
                                );
                            }
                            let close_rect = egui::Rect::from_center_size(
                                egui::pos2(tab.right() - 14.0, tab.center().y),
                                egui::vec2(20.0, 20.0),
                            );
                            let text_rect = egui::Rect::from_min_max(
                                egui::pos2(tab.left() + 12.0, tab.top()),
                                egui::pos2(close_rect.left() - 8.0, tab.bottom()),
                            );
                            let text_color = if selected {
                                Color32::from_rgb(224, 228, 236)
                            } else {
                                Color32::from_rgb(145, 151, 164)
                            };
                            let galley = egui::WidgetText::from(
                                RichText::new(label)
                                    .font(FontId::proportional(12.0))
                                    .color(text_color),
                            )
                            .into_galley(
                                ui,
                                Some(egui::TextWrapMode::Truncate),
                                text_rect.width(),
                                egui::FontSelection::Default,
                            );
                            let text_position = egui::pos2(
                                text_rect.center().x - galley.size().x * 0.5,
                                text_rect.center().y - galley.size().y * 0.5,
                            );
                            ui.painter().galley(text_position, galley, text_color);
                            let show_close = hovered || (selected && !dirty);
                            if *dirty && !show_close {
                                ui.painter().circle_filled(
                                    close_rect.center(),
                                    3.0,
                                    Color32::from_rgb(245, 184, 77),
                                );
                            }
                            let close_response = ui
                                .interact(
                                    close_rect,
                                    Id::new(("file_tab_close", path)),
                                    Sense::click(),
                                )
                                .on_hover_text(format!("Close {label}"));
                            close_response.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::WidgetType::Button,
                                    ui.is_enabled(),
                                    format!("Close {label}"),
                                )
                            });
                            if show_close {
                                ui.painter().text(
                                    close_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "×",
                                    FontId::proportional(14.0),
                                    if close_response.hovered() {
                                        Color32::from_rgb(224, 228, 236)
                                    } else {
                                        Color32::from_rgb(174, 181, 194)
                                    },
                                );
                            }
                            if close_response.clicked() {
                                close = Some(index);
                            } else if response.clicked() {
                                activate = Some(index);
                            }
                            if dragging {
                                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                                if let Some(pointer) =
                                    ui.input(|input| input.pointer.interact_pos())
                                {
                                    let first_left = tab.left() - index as f32 * TAB_WIDTH;
                                    let target = ((pointer.x - first_left) / TAB_WIDTH)
                                        .floor()
                                        .clamp(0.0, (tabs.len() - 1) as f32)
                                        as usize;
                                    if target != index {
                                        reorder = Some((index, target));
                                    }
                                }
                            }
                            if selected {
                                response.scroll_to_me(Some(Align::Center));
                            }
                        }
                    });
            },
        );
        if self.pending.is_none() {
            if let Some(index) = close {
                self.request(PendingAction::CloseTab(index));
            } else if let Some((from, to)) = reorder {
                self.move_tab(from, to);
            } else if let Some(index) = activate {
                self.activate_tab(index);
            }
        }
    }

    fn draw_agent_toggle(&self, ui: &mut egui::Ui, button: egui::Rect) -> bool {
        let label = if self.agent_sidebar {
            "Close Cursor Agent"
        } else {
            "Open Cursor Agent"
        };
        let response = ui
            .interact(button, Id::new("agent_sidebar_toggle"), Sense::click())
            .on_hover_text(label);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        let icon_center = button.center()
            + if self.agent_sidebar {
                egui::vec2(-3.0, 0.0)
            } else {
                egui::Vec2::ZERO
            };
        let icon = egui::Rect::from_center_size(icon_center, egui::vec2(16.0, 13.0));
        let icon_color = if response.hovered() && self.agent_sidebar {
            Color32::from_rgb(151, 232, 242)
        } else if response.hovered() {
            Color32::from_rgb(218, 224, 235)
        } else if self.agent_sidebar {
            Color32::from_rgb(112, 215, 228)
        } else {
            Color32::from_rgb(155, 163, 177)
        };
        ui.painter().rect_stroke(
            icon,
            2.0,
            egui::Stroke::new(1.2, icon_color),
            egui::StrokeKind::Inside,
        );
        ui.painter().vline(
            icon.right() - 4.5,
            icon.y_range(),
            egui::Stroke::new(1.2, icon_color),
        );
        if self.agent_sidebar {
            ui.painter().rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(icon.right() - 4.5, icon.top()),
                    icon.right_bottom(),
                ),
                1.0,
                Color32::from_rgba_unmultiplied(86, 207, 225, 55),
            );
        }
        if self.agent.waiting_permission() {
            ui.painter().circle_filled(
                icon_center + egui::vec2(8.0, -7.0),
                3.0,
                Color32::from_rgb(245, 184, 77),
            );
        }
        response.clicked()
    }

    #[cfg(target_os = "macos")]
    fn draw_macos_titlebar_controls(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let pointer = ui.ctx().pointer_hover_pos();
        let button_centers =
            [17.0, 37.0, 57.0].map(|x| egui::pos2(rect.left() + x, rect.center().y));
        let hovered = button_centers
            .iter()
            .position(|center| pointer.is_some_and(|pointer| pointer.distance(*center) <= 10.0));
        let actions = [
            (WindowAction::Close, Color32::from_rgb(255, 95, 87), "×"),
            (WindowAction::Minimize, Color32::from_rgb(254, 188, 46), "−"),
            (
                WindowAction::ToggleMaximize,
                Color32::from_rgb(40, 200, 64),
                "+",
            ),
        ];
        for (index, ((action, color, symbol), center)) in
            actions.into_iter().zip(button_centers).enumerate()
        {
            let button = egui::Rect::from_center_size(center, egui::vec2(18.0, 24.0));
            if ui
                .interact(button, Id::new(("titlebar_button", index)), Sense::click())
                .clicked()
            {
                self.window_action = Some(action);
            }
            ui.painter().circle_filled(center, 6.0, color);
            if hovered == Some(index) {
                ui.painter().text(
                    center,
                    Align2::CENTER_CENTER,
                    symbol,
                    FontId::proportional(10.0),
                    Color32::from_black_alpha(150),
                );
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    fn draw_windows_titlebar_controls(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        let actions = [
            WindowAction::Minimize,
            WindowAction::ToggleMaximize,
            WindowAction::Close,
        ];
        for (index, action) in actions.into_iter().enumerate() {
            let button = egui::Rect::from_min_max(
                egui::pos2(rect.right() - (3 - index) as f32 * 46.0, rect.top()),
                egui::pos2(rect.right() - (2 - index) as f32 * 46.0, rect.bottom()),
            );
            let response = ui.interact(button, Id::new(("titlebar_button", index)), Sense::click());
            if response.hovered() {
                ui.painter().rect_filled(
                    button,
                    0.0,
                    if index == 2 {
                        Color32::from_rgb(196, 43, 28)
                    } else {
                        Color32::from_rgb(45, 45, 50)
                    },
                );
            }
            let center = button.center();
            let color = Color32::from_rgb(205, 208, 218);
            match index {
                0 => {
                    ui.painter().hline(
                        (center.x - 5.0)..=(center.x + 5.0),
                        center.y + 3.0,
                        egui::Stroke::new(1.0, color),
                    );
                }
                1 => {
                    ui.painter().rect_stroke(
                        egui::Rect::from_center_size(center, egui::vec2(9.0, 9.0)),
                        0.0,
                        egui::Stroke::new(1.0, color),
                        egui::StrokeKind::Inside,
                    );
                }
                _ => {
                    ui.painter().line_segment(
                        [
                            center + egui::vec2(-4.0, -4.0),
                            center + egui::vec2(4.0, 4.0),
                        ],
                        egui::Stroke::new(1.0, color),
                    );
                    ui.painter().line_segment(
                        [
                            center + egui::vec2(4.0, -4.0),
                            center + egui::vec2(-4.0, 4.0),
                        ],
                        egui::Stroke::new(1.0, color),
                    );
                }
            }
            if response.clicked() {
                self.window_action = Some(action);
            }
        }
    }

    fn draw_error(&mut self, ctx: &egui::Context) {
        let Some(error) = self.error.clone() else {
            return;
        };
        egui::Window::new("Error")
            .id(Id::new("error_banner"))
            .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 42.0))
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.colored_label(Color32::from_rgb(255, 125, 125), error);
                if ui.button("Dismiss").clicked() {
                    self.error = None;
                }
            });
    }

    fn take_window_action(&mut self) -> Option<WindowAction> {
        self.window_action.take()
    }

    fn shortcuts(&mut self, ctx: &egui::Context) {
        let (save, save_quit, project_search, find, sidebar, tree, editor, close) =
            ctx.input(|input| {
                let command = input.modifiers.command;
                (
                    command && input.key_pressed(Key::S),
                    command
                        && ((input.key_pressed(Key::Q) && input.key_down(Key::S))
                            || (input.key_pressed(Key::S) && input.key_down(Key::Q))),
                    command && input.modifiers.shift && input.key_pressed(Key::F),
                    command && !input.modifiers.shift && input.key_pressed(Key::F),
                    command && input.key_pressed(Key::B),
                    command && input.key_pressed(Key::Num1),
                    command && input.key_pressed(Key::Num2),
                    command && input.key_pressed(Key::W),
                )
            });
        if save_quit {
            if self.save(None) {
                self.request_close();
            }
        } else if save {
            self.save(None);
        }
        if project_search {
            self.search_open = true;
            self.find_open = false;
            self.focus_search = true;
            self.focus_editor = false;
            self.tree_focused = false;
            ctx.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
        }
        if find {
            self.markdown_preview = false;
            self.find_open = true;
            self.search_open = false;
            self.focus_find = true;
            self.scroll_to_find_match = !self.find_matches.is_empty();
            self.focus_editor = false;
            self.tree_focused = false;
            ctx.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
        }
        if sidebar {
            self.sidebar = !self.sidebar;
        }
        if tree {
            self.sidebar = true;
            self.focus_editor = false;
            self.tree_focused = true;
            ctx.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
        }
        if editor {
            self.markdown_preview = false;
            self.focus_editor = true;
            self.tree_focused = false;
        }
        if close {
            if let Some(index) = self.active_tab {
                self.request(PendingAction::CloseTab(index));
            } else {
                self.request_close();
            }
        }
    }

    fn refresh_find_matches(&mut self) {
        let Some(index) = self.active_tab else {
            self.find_matches.clear();
            self.find_selected = 0;
            return;
        };
        let buffer = &self.tabs[index].buffer;
        if self.find_match_revision == buffer.revision && self.find_match_query == self.find_query {
            return;
        }
        self.find_matches = match_spans(&buffer.text, &self.find_query);
        self.find_match_revision = buffer.revision;
        self.find_match_query.clone_from(&self.find_query);
        self.find_selected = self
            .find_selected
            .min(self.find_matches.len().saturating_sub(1));
        self.highlight_cache.find_valid = false;
    }

    fn draw_sidebar(&mut self, ui: &mut egui::Ui) {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, Color32::from_rgb(20, 20, 22));
        #[cfg(target_os = "macos")]
        ui.add_space(TITLEBAR_HEIGHT);
        self.draw_tree(ui);
    }

    fn draw_agent_sidebar(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(20, 20, 22));
        self.draw_agent(ui, rect);
    }

    fn open_agent(&mut self, ctx: &egui::Context) {
        let wake = ctx.clone();
        self.start_agent(move || wake.request_repaint());
    }

    fn start_agent(&mut self, wake: impl Fn() + Send + Sync + 'static) {
        if self.agent_controller.is_some() {
            return;
        }
        self.agent.session_ready = false;
        self.agent.active = false;
        self.agent.connection = ConnectionState::Starting;
        self.agent_run_everything = None;
        self.agent_controller = Some(AgentController::start_with_wake(
            self.tree.root.clone(),
            wake,
        ));
    }

    fn reconnect_agent(&mut self, ctx: &egui::Context) {
        self.agent_controller = None;
        self.open_agent(ctx);
    }

    fn poll_agent(&mut self, ctx: &egui::Context) {
        let mut events = Vec::new();
        if let Some(controller) = &self.agent_controller {
            for _ in 0..64 {
                let Ok(event) = controller.events().try_recv() else {
                    break;
                };
                events.push(event);
            }
        }
        if events.len() == 64 {
            ctx.request_repaint();
        }
        for event in events {
            if matches!(
                event,
                AgentEvent::SessionReady { .. } | AgentEvent::SessionLoading { .. }
            ) {
                self.agent_follow_transcript = true;
                self.agent_prompt_history_index = None;
                self.agent_prompt_history_draft.clear();
            }
            if let AgentEvent::CommandsUpdated(commands) = &event
                && let Some(enabled) = run_everything_state(commands)
            {
                self.agent_run_everything = Some(enabled);
            }
            let reconcile_path = match &event {
                AgentEvent::ToolCallUpdated(tool) => self
                    .tabs
                    .iter()
                    .any(|tab| tool.paths.iter().any(|path| path == &tab.buffer.path)),
                _ => false,
            };
            let turn_finished = matches!(event, AgentEvent::TurnFinished { .. });
            let refresh_project =
                turn_finished || matches!(event, AgentEvent::ProcessExited { .. });
            self.agent.apply(event);
            if reconcile_path || turn_finished {
                self.reconcile_open_buffer();
            }
            if refresh_project {
                self.refresh_after_agent();
            }
        }
    }

    fn reconcile_open_buffer(&mut self) {
        let mut active_reloaded = false;
        let mut conflict = None;
        let mut error = None;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            match reconcile_buffer(&mut tab.buffer) {
                Ok(ReconcileOutcome::Unchanged) => {}
                Ok(ReconcileOutcome::Reloaded) => {
                    let cursor = tab.editor_surface.cursor();
                    tab.editor_surface = EditorSurface::default();
                    tab.editor_surface.set_selection(cursor, cursor);
                    active_reloaded |= self.active_tab == Some(index);
                }
                Ok(ReconcileOutcome::Conflict) => {
                    conflict.get_or_insert(index);
                }
                Err(found) if error.is_none() => error = Some(found),
                Err(_) => {}
            };
        }
        if active_reloaded {
            self.highlight_cache.valid = false;
            self.find_match_revision = u64::MAX;
            self.bracket_pair = None;
            self.bracket_pair_key = None;
            self.markdown_layout = None;
        }
        if let Some(index) = conflict {
            self.activate_tab(index);
            self.conflict = true;
        }
        if let Some(error) = error
            && self.error.is_none()
        {
            self.show_error(error);
        }
    }

    fn refresh_after_agent(&mut self) {
        let changed = std::mem::take(&mut self.agent.changed_paths);
        let directories = if changed.is_empty() {
            self.tree.children.keys().cloned().collect::<HashSet<_>>()
        } else {
            changed
                .iter()
                .filter_map(|path| path.parent().map(Path::to_path_buf))
                .collect()
        };
        for directory in directories {
            let error = match self.tree.children.entry(directory) {
                std::collections::hash_map::Entry::Occupied(mut entry) => {
                    match read_directory(entry.key()) {
                        Ok(entries) => {
                            entry.insert(entries);
                            None
                        }
                        Err(error) => Some(error),
                    }
                }
                std::collections::hash_map::Entry::Vacant(_) => None,
            };
            if let Some(error) = error
                && self.error.is_none()
            {
                self.show_error(error);
            }
        }
        self.tree.refresh_visible();
        if !changed.is_empty() {
            match SearchController::new(self.tree.root.clone()) {
                Ok(search) => {
                    self.search = search;
                    if !self.search_query.trim().is_empty() {
                        let _ = self.search.set_query(&self.search_query);
                    }
                }
                Err(error) => self.show_error(error),
            }
        }
    }

    fn queue_agent_prompt(&mut self) {
        if self.buffer().is_some_and(|buffer| buffer.dirty) {
            self.pending_agent_prompt = true;
        } else {
            self.send_agent_prompt();
        }
    }

    fn send_agent_prompt(&mut self) {
        let Some(controller) = &self.agent_controller else {
            return;
        };
        let prompt = self.agent.prompt.trim().to_owned();
        if prompt.is_empty() || self.agent.active || !self.agent.session_ready {
            return;
        }
        self.agent.active = true;
        match controller.send(AgentCommand::Prompt(prompt)) {
            Ok(()) => {
                self.agent_prompt_history_index = None;
                self.agent_prompt_history_draft.clear();
            }
            Err(error) => {
                self.agent.active = false;
                self.show_error(error);
            }
        }
    }

    fn navigate_agent_prompt_history(&mut self, older: bool) -> bool {
        let history_len = self
            .agent
            .transcript
            .iter()
            .filter(|item| matches!(item, TranscriptItem::User(_)))
            .count();
        if history_len == 0 {
            return false;
        }
        let next = if older {
            if let Some(index) = self.agent_prompt_history_index {
                Some(index.saturating_sub(1))
            } else {
                self.agent_prompt_history_draft = self.agent.prompt.clone();
                Some(history_len - 1)
            }
        } else {
            match self.agent_prompt_history_index {
                Some(index) if index + 1 < history_len => Some(index + 1),
                Some(_) => None,
                None => return false,
            }
        };
        self.agent_prompt_history_index = next;
        self.agent.prompt = next.map_or_else(
            || self.agent_prompt_history_draft.clone(),
            |index| {
                self.agent
                    .transcript
                    .iter()
                    .filter_map(|item| match item {
                        TranscriptItem::User(prompt) => Some(prompt),
                        _ => None,
                    })
                    .nth(index)
                    .cloned()
                    .unwrap_or_default()
            },
        );
        true
    }

    fn draw_agent(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Body, FontId::proportional(15.0));
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Small, FontId::proportional(12.5));
        ui.style_mut()
            .text_styles
            .insert(egui::TextStyle::Button, FontId::proportional(13.5));
        let font_id = egui::TextStyle::Body.resolve(ui.style());
        let (text_height, row_height) = ui.fonts_mut(|fonts| {
            let row_height = fonts.row_height(&font_id);
            let text_height = fonts
                .layout(
                    self.agent.prompt.clone(),
                    font_id,
                    Color32::WHITE,
                    (rect.width() - 20.0).max(24.0),
                )
                .size()
                .y;
            (text_height, row_height)
        });
        let composer_height = agent_composer_height(text_height, row_height, rect.height());
        let (header, transcript, composer) = split_agent_sidebar(rect, composer_height);
        let menu_owns_wheel = self.agent_menu.is_some()
            && self.agent_menu_popup.is_some_and(|popup| {
                ui.input(|input| {
                    input
                        .pointer
                        .hover_pos()
                        .is_some_and(|pointer| popup.contains(pointer))
                })
            });
        let status = self.agent.connection.clone();
        let mut new_session = false;
        let mut session_menu_anchor = None;
        let mut session_menu_toggled = false;
        let painter = ui.painter().clone();
        painter.rect_filled(header, 0.0, Color32::from_rgb(24, 24, 26));
        painter.hline(
            header.x_range(),
            header.bottom() - 0.5,
            egui::Stroke::new(1.0, Color32::from_rgb(42, 42, 47)),
        );
        let title = self.agent.title.as_deref().unwrap_or("Agent");
        painter.text(
            egui::pos2(header.left() + 14.0, header.center().y - 0.5),
            Align2::LEFT_CENTER,
            title,
            FontId::proportional(13.0),
            Color32::from_rgb(223, 227, 235),
        );
        let agent_button = agent_toggle_rect(header);
        if self.draw_agent_toggle(ui, agent_button) {
            self.agent_sidebar = false;
            self.agent_sidebar_dragging = false;
            self.agent_menu = None;
            ui.ctx().request_repaint();
        }
        if self.agent.session_ready {
            let button = agent_new_session_rect(header);
            let response = ui
                .interact(button, Id::new("agent_new_session"), Sense::click())
                .on_hover_text("New Agent session");
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    "New Agent session",
                )
            });
            let icon_color = if response.hovered() {
                Color32::from_rgb(224, 228, 236)
            } else {
                Color32::from_rgb(172, 180, 194)
            };
            painter.hline(
                (button.center().x - 5.0)..=(button.center().x + 5.0),
                button.center().y,
                egui::Stroke::new(1.3, icon_color),
            );
            painter.vline(
                button.center().x,
                (button.center().y - 5.0)..=(button.center().y + 5.0),
                egui::Stroke::new(1.3, icon_color),
            );
            new_session = response.clicked();
        }
        if self.agent.session_ready && self.agent.sessions.is_some() {
            let button = agent_sessions_rect(header);
            let response = ui
                .interact(button, Id::new("agent_sessions"), Sense::click())
                .on_hover_text("Previous sessions");
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    "Previous sessions",
                )
            });
            let icon_color = if response.hovered() {
                Color32::from_rgb(224, 228, 236)
            } else {
                Color32::from_rgb(172, 180, 194)
            };
            let icon_center = button.center() + egui::vec2(3.0, 0.0);
            painter.circle_stroke(icon_center, 6.0, egui::Stroke::new(1.3, icon_color));
            painter.line_segment(
                [icon_center, icon_center + egui::vec2(0.0, -3.5)],
                egui::Stroke::new(1.3, icon_color),
            );
            painter.line_segment(
                [icon_center, icon_center + egui::vec2(3.0, 1.5)],
                egui::Stroke::new(1.3, icon_color),
            );
            if response.clicked() {
                let menu = AgentMenu::Sessions;
                self.agent_menu = (self.agent_menu.as_ref() != Some(&menu)).then_some(menu.clone());
                session_menu_toggled = true;
                if self.agent_menu.is_some()
                    && let Some(controller) = &self.agent_controller
                {
                    let _ = controller.send(AgentCommand::RefreshSessions);
                }
            }
            if matches!(self.agent_menu, Some(AgentMenu::Sessions)) {
                session_menu_anchor = Some(button);
            }
        }

        let mut reconnect = false;
        let mut authenticate = None;
        let mut permission_decisions = Vec::new();
        let mut interaction_responses = Vec::new();
        let transcript_content = transcript.shrink2(egui::vec2(16.0, 0.0));
        let transcript_width = transcript_content.width();
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("agent_transcript_region")
                .max_rect(transcript_content)
                .layout(Layout::top_down(Align::LEFT)),
            |ui| match &status {
                ConnectionState::Provisioning { downloaded, total } => {
                    ui.label(
                        RichText::new("Installing Cursor Agent")
                            .size(15.0)
                            .strong()
                            .color(Color32::from_rgb(224, 228, 236)),
                    );
                    let total = total
                        .map_or_else(|| "?".into(), |value| (value / 1_048_576).to_string());
                    ui.label(
                        RichText::new(format!(
                            "Downloading {} / {total} MiB…",
                            downloaded / 1_048_576
                        ))
                        .color(Color32::from_rgb(132, 141, 156)),
                    );
                }
                ConnectionState::Starting => {
                    ui.label(
                        RichText::new("Connecting to Cursor…")
                            .size(14.0)
                            .color(Color32::from_rgb(174, 181, 194)),
                    );
                }
                ConnectionState::AuthenticationRequired(methods) => {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(27, 27, 30))
                        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(45, 45, 50)))
                        .inner_margin(egui::Margin::same(14))
                        .corner_radius(7)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new("Connect Cursor")
                                    .size(15.0)
                                    .strong()
                                    .color(Color32::from_rgb(226, 230, 238)),
                            );
                            ui.add_space(3.0);
                            ui.add(
                                Label::new(
                                    RichText::new(
                                        "Sign in with your Cursor account to start an Agent session in this project.",
                                    )
                                    .color(Color32::from_rgb(142, 150, 164)),
                                )
                                .wrap(),
                            );
                            ui.add_space(10.0);
                            for method in methods {
                                let button = egui::Button::new(
                                    RichText::new(&method.name)
                                        .strong()
                                        .color(Color32::from_rgb(10, 27, 31)),
                                )
                                .fill(Color32::from_rgb(94, 210, 224))
                                .stroke(egui::Stroke::NONE)
                                .corner_radius(5)
                                .min_size(egui::vec2(ui.available_width(), 30.0));
                                if ui.add(button).clicked() {
                                    authenticate = Some(method.id.clone());
                                }
                                if let Some(description) = &method.description {
                                    ui.add(
                                        Label::new(RichText::new(description).small().weak())
                                            .wrap(),
                                    );
                                }
                            }
                        });
                }
                ConnectionState::Failed(error) => {
                    egui::Frame::new()
                        .fill(Color32::from_rgb(34, 27, 31))
                        .stroke(egui::Stroke::new(1.0, Color32::from_rgb(78, 45, 51)))
                        .inner_margin(egui::Margin::same(14))
                        .corner_radius(7)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(
                                RichText::new("Cursor Agent unavailable")
                                    .strong()
                                    .color(Color32::from_rgb(237, 191, 194)),
                            );
                            ui.add(Label::new(RichText::new(error).weak()).wrap());
                            ui.add_space(8.0);
                            reconnect = ui.button("Try again").clicked();
                        });
                }
                ConnectionState::Disconnected => {
                    ui.label(RichText::new("Cursor Agent is offline.").weak());
                    reconnect = ui.button("Connect").clicked();
                }
                ConnectionState::Ready if self.agent.transcript.is_empty() => {
                    ui.add_space((ui.available_height() * 0.3).min(110.0));
                    ui.with_layout(Layout::top_down(Align::Center), |ui| {
                        ui.label(
                            RichText::new("Start a task")
                                .size(18.0)
                                .strong()
                                .color(Color32::from_rgb(210, 215, 225)),
                        );
                        ui.add_space(3.0);
                        ui.add(
                            Label::new(
                                RichText::new(
                                    "Ask Cursor to edit, explain, or run commands in this project.",
                                )
                                .size(13.5)
                                .color(Color32::from_rgb(112, 121, 136)),
                            )
                            .wrap(),
                        );
                    });
                }
                ConnectionState::Ready => {
                    let scroll_delta = if !menu_owns_wheel && ui.rect_contains_pointer(ui.max_rect()) {
                        ui.input(|input| input.smooth_scroll_delta.y)
                    } else {
                        0.0
                    };
                    let manual_scroll = scroll_delta != 0.0;
                    let scrolling_up = scroll_delta > 0.0;
                    if scrolling_up {
                        self.agent_follow_transcript = false;
                    }
                    let output = ScrollArea::vertical()
                        .id_salt("agent_transcript")
                        .auto_shrink([false, false])
                        .scroll_source(egui::scroll_area::ScrollSource {
                            mouse_wheel: !menu_owns_wheel,
                            ..Default::default()
                        })
                        .content_margin(egui::Margin::symmetric(
                            0,
                            AGENT_TRANSCRIPT_EDGE_PADDING,
                        ))
                        .stick_to_bottom(self.agent_follow_transcript)
                        .show(ui, |ui| {
                            ui.set_width(transcript_width);
                            ui.set_max_width(transcript_width);
                            for (item_index, item) in self.agent.transcript.iter_mut().enumerate() {
                                match item {
                                    TranscriptItem::User(text) => {
                                        ui.label(
                                            RichText::new("YOU")
                                                .size(11.5)
                                                .strong()
                                                .color(Color32::from_rgb(146, 157, 174)),
                                        );
                                        egui::Frame::new()
                                            .fill(Color32::from_rgb(31, 37, 44))
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                Color32::from_rgb(49, 59, 68),
                                            ))
                                            .inner_margin(egui::Margin::same(12))
                                            .corner_radius(8)
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.add(Label::new(text.as_str()).wrap());
                                            });
                                    }
                                    TranscriptItem::Assistant(text) => {
                                        draw_cursor_identity(ui);
                                        let width = ui.available_width();
                                        let galley = agent_markdown_galley(
                                            ui,
                                            Id::new(("agent_markdown", item_index)),
                                            text,
                                            width,
                                        );
                                        ui.add(Label::new(galley).wrap());
                                    }
                                    TranscriptItem::Thought(text) => {
                                        egui::CollapsingHeader::new("Thinking")
                                            .id_salt(("thought", item_index))
                                            .show(ui, |ui| {
                                                ui.add(
                                                    Label::new(
                                                        RichText::new(text.as_str()).weak().italics(),
                                                    )
                                                    .wrap(),
                                                );
                                            });
                                    }
                                    TranscriptItem::Content { role, content } => {
                                        match role {
                                            ContentRole::Assistant => draw_cursor_identity(ui),
                                            ContentRole::User => {
                                                ui.label(RichText::new("You").small().strong());
                                            }
                                            ContentRole::Thought => {
                                                ui.label(RichText::new("Thinking").small().strong());
                                            }
                                        }
                                        draw_agent_content(ui, content);
                                    }
                                    TranscriptItem::Plan(plan) => {
                                        egui::CollapsingHeader::new("Plan")
                                            .id_salt(("plan", item_index))
                                            .default_open(true)
                                            .show(ui, |ui| {
                                                for item in plan {
                                                    ui.add(
                                                        Label::new(format!(
                                                            "{}  {}",
                                                            item.status, item.content
                                                        ))
                                                        .wrap(),
                                                    );
                                                }
                                            });
                                    }
                                    TranscriptItem::Tool(tool) => {
                                        let title =
                                            tool.title.as_deref().unwrap_or("Tool activity");
                                        let contains_diff = tool_contains_diff(tool);
                                        agent_collapsing_header(
                                            ui,
                                            ("tool", &tool.id, contains_diff),
                                            title,
                                            tool.status.as_deref(),
                                            transcript_width,
                                            contains_diff,
                                            |ui| {
                                                for path in &tool.paths {
                                                    ui.add(
                                                        Label::new(
                                                            RichText::new(path.display().to_string())
                                                                .monospace()
                                                                .size(12.5),
                                                        )
                                                        .truncate(),
                                                    );
                                                }
                                                if let Some(detail) = &tool.detail {
                                                    for (content_index, content) in
                                                        detail.content.iter().enumerate()
                                                    {
                                                        match content {
                                                            ToolOutput::Text(text) => {
                                                                ui.add(Label::new(text).wrap());
                                                            }
                                                            ToolOutput::Content(content) => {
                                                                draw_agent_content(ui, content);
                                                            }
                                                            ToolOutput::Diff {
                                                                path,
                                                                old_text,
                                                                new_text,
                                                            } => {
                                                                draw_agent_diff(
                                                                    ui,
                                                                    Id::new((
                                                                        "agent_diff",
                                                                        &tool.id,
                                                                        content_index,
                                                                    )),
                                                                    path,
                                                                    old_text.as_deref(),
                                                                    new_text,
                                                                );
                                                            }
                                                            ToolOutput::Terminal(id) => {
                                                                ui.label(format!("Terminal {id}"));
                                                            }
                                                            ToolOutput::Todo {
                                                                id,
                                                                content,
                                                                status,
                                                            } => {
                                                                ui.add(
                                                                    Label::new(format!(
                                                                        "{status}  {content} ({id})"
                                                                    ))
                                                                    .wrap(),
                                                                );
                                                            }
                                                            ToolOutput::Task {
                                                                description,
                                                                prompt,
                                                                subagent_type,
                                                                model,
                                                                agent_id,
                                                                duration_ms,
                                                            } => {
                                                                ui.label(
                                                                    RichText::new(description)
                                                                        .strong(),
                                                                );
                                                                ui.add(Label::new(prompt).wrap());
                                                                let metadata = [
                                                                    Some(subagent_type.clone()),
                                                                    model.clone(),
                                                                    agent_id.clone(),
                                                                    duration_ms.map(|duration| {
                                                                        format!("{duration} ms")
                                                                    }),
                                                                ]
                                                                .into_iter()
                                                                .flatten()
                                                                .collect::<Vec<_>>()
                                                                .join(" · ");
                                                                ui.label(
                                                                    RichText::new(metadata)
                                                                        .small()
                                                                        .weak(),
                                                                );
                                                            }
                                                            ToolOutput::GeneratedImage {
                                                                description,
                                                                file_path,
                                                                reference_image_paths,
                                                            } => {
                                                                ui.add(
                                                                    Label::new(description).wrap(),
                                                                );
                                                                ui.label(
                                                                    RichText::new(
                                                                        file_path
                                                                            .display()
                                                                            .to_string(),
                                                                    )
                                                                    .monospace()
                                                                    .small(),
                                                                );
                                                                for reference in
                                                                    reference_image_paths
                                                                {
                                                                    ui.label(
                                                                        RichText::new(format!(
                                                                            "Reference: {}",
                                                                            reference.display()
                                                                        ))
                                                                        .monospace()
                                                                        .small()
                                                                        .weak(),
                                                                    );
                                                                }
                                                            }
                                                        }
                                                    }
                                                    if detail.content.is_empty()
                                                        && let Some(text) = detail
                                                            .output
                                                            .as_deref()
                                                            .or(detail.input.as_deref())
                                                    {
                                                        ui.add(
                                                            Label::new(
                                                                RichText::new(text)
                                                                    .monospace()
                                                                    .size(12.5)
                                                                    .color(Color32::from_rgb(
                                                                        174, 181, 194,
                                                                    )),
                                                            )
                                                            .wrap()
                                                            .selectable(true),
                                                        );
                                                    }
                                                }
                                            },
                                        );
                                    }
                                    TranscriptItem::Permission(card) => {
                                        if let Some(selected) = card.selected.as_ref().and_then(
                                            |selected| {
                                                card.options
                                                    .iter()
                                                    .find(|option| &option.id == selected)
                                            },
                                        ) {
                                            let (status, color) = match selected.kind.as_str() {
                                                "AllowAlways" => (
                                                    "Allowed globally",
                                                    Color32::from_rgb(123, 205, 158),
                                                ),
                                                "AllowOnce" => (
                                                    "Allowed once",
                                                    Color32::from_rgb(123, 205, 158),
                                                ),
                                                "RejectAlways" => (
                                                    "Rejected globally",
                                                    Color32::from_rgb(224, 137, 145),
                                                ),
                                                "RejectOnce" => (
                                                    "Rejected",
                                                    Color32::from_rgb(224, 137, 145),
                                                ),
                                                _ => (selected.name.as_str(), Color32::GRAY),
                                            };
                                            egui::Frame::new()
                                                .fill(Color32::from_rgb(28, 28, 31))
                                                .stroke(egui::Stroke::new(
                                                    1.0,
                                                    Color32::from_rgb(48, 48, 53),
                                                ))
                                                .inner_margin(egui::Margin::same(8))
                                                .corner_radius(7)
                                                .show(ui, |ui| {
                                                    ui.set_width(ui.available_width().min(284.0));
                                                    ui.label(
                                                        RichText::new(status)
                                                            .small()
                                                            .strong()
                                                            .color(color),
                                                    );
                                                    ui.add(Label::new(&card.action).wrap());
                                                });
                                            continue;
                                        }
                                        egui::Frame::new()
                                            .fill(Color32::from_rgb(31, 30, 27))
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                Color32::from_rgb(73, 62, 42),
                                            ))
                                            .inner_margin(egui::Margin::same(10))
                                            .corner_radius(8)
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width().min(278.0));
                                                ui.label(
                                                    RichText::new("Permission required")
                                                        .strong()
                                                        .color(Color32::from_rgb(235, 204, 137)),
                                                );
                                                ui.add_space(3.0);
                                                ui.add(Label::new(&card.action).wrap());
                                                ui.add_space(8.0);
                                                ui.horizontal_wrapped(|ui| {
                                                    ui.spacing_mut().item_spacing.x = 6.0;
                                                    for option in &card.options {
                                                        let label = match option.kind.as_str() {
                                                            "AllowOnce" => "Allow once",
                                                            "AllowAlways" => "Always allow",
                                                            "RejectOnce" => "Reject",
                                                            "RejectAlways" => "Always reject",
                                                            _ => &option.name,
                                                        };
                                                        let (fill, stroke, text_color) =
                                                            match option.kind.as_str() {
                                                                "AllowAlways" => (
                                                                    Color32::from_rgb(37, 55, 57),
                                                                    Color32::from_rgb(62, 120, 126),
                                                                    Color32::from_rgb(210, 237, 240),
                                                                ),
                                                                "RejectOnce" | "RejectAlways" => (
                                                                    Color32::from_rgb(42, 32, 35),
                                                                    Color32::from_rgb(84, 53, 60),
                                                                    Color32::from_rgb(225, 190, 195),
                                                                ),
                                                                _ => (
                                                                    Color32::from_rgb(43, 43, 48),
                                                                    Color32::from_rgb(67, 67, 74),
                                                                    Color32::from_rgb(230, 230, 234),
                                                                ),
                                                            };
                                                        let response = ui
                                                            .add(
                                                                egui::Button::new(
                                                                    RichText::new(label)
                                                                        .strong()
                                                                        .color(text_color),
                                                                )
                                                                .min_size(egui::vec2(0.0, 30.0))
                                                                .fill(fill)
                                                                .stroke(egui::Stroke::new(
                                                                    1.0, stroke,
                                                                ))
                                                                .corner_radius(6),
                                                            )
                                                            .on_hover_text(
                                                                if option.kind == "AllowAlways" {
                                                                    "Remember this permission globally"
                                                                } else {
                                                                    &option.name
                                                                },
                                                            );
                                                        if response.clicked() {
                                                            permission_decisions.push((
                                                                card.request_id,
                                                                option.id.clone(),
                                                            ));
                                                        }
                                                    }
                                                });
                                            });
                                    }
                                    TranscriptItem::Interaction(card) => {
                                        egui::Frame::new()
                                            .fill(Color32::from_rgb(31, 35, 42))
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                Color32::from_rgb(56, 68, 82),
                                            ))
                                            .inner_margin(egui::Margin::same(10))
                                            .corner_radius(7)
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                match &card.request.kind {
                                                    InteractionKind::Questions {
                                                        title,
                                                        questions,
                                                    } => {
                                                        ui.label(
                                                            RichText::new(title).strong().color(
                                                                Color32::from_rgb(176, 216, 233),
                                                            ),
                                                        );
                                                        for question in questions {
                                                            ui.add_space(6.0);
                                                            ui.add(
                                                                Label::new(
                                                                    RichText::new(
                                                                        &question.prompt,
                                                                    )
                                                                    .strong(),
                                                                )
                                                                .wrap(),
                                                            );
                                                            let selected = card
                                                                .selections
                                                                .entry(question.id.clone())
                                                                .or_default();
                                                            for option in &question.options {
                                                                let is_selected = selected
                                                                    .contains(&option.id);
                                                                if ui
                                                                    .add_enabled(
                                                                        !card.answered,
                                                                        egui::Button::selectable(
                                                                            is_selected,
                                                                            &option.label,
                                                                        ),
                                                                    )
                                                                    .clicked()
                                                                {
                                                                    if question.allow_multiple {
                                                                        if is_selected {
                                                                            selected.retain(|id| {
                                                                                id != &option.id
                                                                            });
                                                                        } else {
                                                                            selected
                                                                                .push(option.id.clone());
                                                                        }
                                                                    } else {
                                                                        selected.clear();
                                                                        selected.push(
                                                                            option.id.clone(),
                                                                        );
                                                                    }
                                                                }
                                                            }
                                                        }
                                                        if !card.answered {
                                                            ui.horizontal(|ui| {
                                                                let complete = questions.iter().all(
                                                                    |question| {
                                                                        card.selections
                                                                            .get(&question.id)
                                                                            .is_some_and(|answer| {
                                                                                !answer.is_empty()
                                                                            })
                                                                    },
                                                                );
                                                                if ui
                                                                    .add_enabled(
                                                                        complete,
                                                                        egui::Button::new(
                                                                            "Submit answers",
                                                                        ),
                                                                    )
                                                                    .clicked()
                                                                {
                                                                    interaction_responses.push((
                                                                        card.request.request_id,
                                                                        InteractionResponse::Answers(
                                                                            questions
                                                                                .iter()
                                                                                .map(|question| {
                                                                                    QuestionAnswer {
                                                                                        question_id: question.id.clone(),
                                                                                        selected_option_ids: card.selections[&question.id].clone(),
                                                                                    }
                                                                                })
                                                                                .collect(),
                                                                        ),
                                                                    ));
                                                                }
                                                                if ui.button("Skip").clicked() {
                                                                    interaction_responses.push((
                                                                        card.request.request_id,
                                                                        InteractionResponse::Skipped,
                                                                    ));
                                                                }
                                                            });
                                                        }
                                                    }
                                                    InteractionKind::Plan(plan) => {
                                                        ui.label(
                                                            RichText::new(
                                                                plan.name
                                                                    .as_deref()
                                                                    .unwrap_or("Proposed plan"),
                                                            )
                                                            .strong()
                                                            .color(Color32::from_rgb(
                                                                176, 216, 233,
                                                            )),
                                                        );
                                                        if let Some(overview) = &plan.overview {
                                                            ui.add(
                                                                Label::new(overview).wrap(),
                                                            );
                                                        }
                                                        if !plan.plan.is_empty() {
                                                            ui.add(
                                                                Label::new(&plan.plan).wrap(),
                                                            );
                                                        }
                                                        for todo in &plan.todos {
                                                            ui.add(
                                                                Label::new(format!(
                                                                    "{}  {}",
                                                                    todo.status, todo.content
                                                                ))
                                                                .wrap(),
                                                            );
                                                        }
                                                        for phase in &plan.phases {
                                                            ui.label(
                                                                RichText::new(&phase.name).strong(),
                                                            );
                                                            for todo in &phase.todos {
                                                                ui.add(
                                                                    Label::new(format!(
                                                                        "{}  {}",
                                                                        todo.status, todo.content
                                                                    ))
                                                                    .wrap(),
                                                                );
                                                            }
                                                        }
                                                        if let Some(is_project) = plan.is_project {
                                                            ui.label(
                                                                RichText::new(if is_project {
                                                                    "Project plan"
                                                                } else {
                                                                    "Session plan"
                                                                })
                                                                .small()
                                                                .weak(),
                                                            );
                                                        }
                                                        if !card.answered {
                                                            ui.horizontal(|ui| {
                                                                if ui.button("Accept").clicked() {
                                                                    interaction_responses.push((
                                                                        card.request.request_id,
                                                                        InteractionResponse::PlanAccepted,
                                                                    ));
                                                                }
                                                                if ui.button("Reject").clicked() {
                                                                    interaction_responses.push((
                                                                        card.request.request_id,
                                                                        InteractionResponse::PlanRejected,
                                                                    ));
                                                                }
                                                            });
                                                        }
                                                    }
                                                }
                                            });
                                    }
                                    TranscriptItem::Error(error) => {
                                        egui::Frame::new()
                                            .fill(Color32::from_rgb(38, 28, 32))
                                            .inner_margin(egui::Margin::same(10))
                                            .corner_radius(6)
                                            .show(ui, |ui| {
                                                ui.set_width(ui.available_width());
                                                ui.add(
                                                    Label::new(
                                                        RichText::new(error.as_str()).color(
                                                            Color32::from_rgb(235, 145, 150),
                                                        ),
                                                    )
                                                    .wrap(),
                                                );
                                            });
                                    }
                                    TranscriptItem::Truncated => {
                                        ui.label(
                                            RichText::new("Earlier output was truncated.")
                                                .small()
                                                .weak(),
                                        );
                                    }
                                }
                                ui.add_space(16.0);
                            }
                        });
                    let max_offset =
                        (output.content_size.y - output.inner_rect.height()).max(0.0);
                    let near_bottom = agent_near_bottom(output.state.offset.y, max_offset);
                    let at_bottom = (max_offset - output.state.offset.y).abs() <= 0.5;
                    self.agent_follow_transcript = !scrolling_up
                        && (self.agent_follow_transcript
                            || at_bottom
                            || (manual_scroll && near_bottom));
                    if self.agent_follow_transcript
                        && (output.state.offset.y - max_offset).abs() > 0.5
                    {
                        let mut state = output.state;
                        state.offset.y = max_offset;
                        state.store(ui.ctx(), output.id);
                        ui.ctx().request_repaint();
                    }
                    ui.painter().add(egui::Shape::mesh(agent_transcript_fade_mesh(
                        output.inner_rect,
                    )));
                    if !self.agent_follow_transcript {
                        let button = egui::Rect::from_min_size(
                            egui::pos2(
                                output.inner_rect.right() - 32.0,
                                output.inner_rect.bottom() - 32.0,
                            ),
                            egui::vec2(26.0, 26.0),
                        );
                        let jump = ui
                            .put(
                                button,
                                egui::Button::new("")
                                .fill(Color32::from_rgb(40, 43, 49))
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    Color32::from_rgb(65, 70, 80),
                                ))
                                .corner_radius(6),
                            )
                            .on_hover_text("Jump to latest");
                        let center = jump.rect.center();
                        let stroke = egui::Stroke::new(1.5, Color32::from_rgb(190, 198, 211));
                        ui.painter().line_segment(
                            [
                                egui::pos2(center.x - 4.0, center.y - 2.0),
                                egui::pos2(center.x, center.y + 2.0),
                            ],
                            stroke,
                        );
                        ui.painter().line_segment(
                            [
                                egui::pos2(center.x, center.y + 2.0),
                                egui::pos2(center.x + 4.0, center.y - 2.0),
                            ],
                            stroke,
                        );
                        if jump.clicked() {
                            let mut state = output.state;
                            state.offset.y = max_offset;
                            state.store(ui.ctx(), output.id);
                            self.agent_follow_transcript = true;
                            ui.ctx().request_repaint();
                        }
                    }
                }
            },
        );

        let mut mode_change = None;
        let mut config_changes = Vec::new();
        let mut run_everything_change = None;
        let mut session_load = None;
        let mut session_remove = None;
        let has_config_mode = self.agent.config_options.iter().any(|option| {
            option.id.eq_ignore_ascii_case("mode") || option.name.eq_ignore_ascii_case("mode")
        });

        let mut send = false;
        let mut cancel = false;
        let mut submit_shortcut = false;
        let mut prompt_changed = false;
        let mut history_navigated = false;
        let composer_enabled = self.agent.session_ready && !self.agent.active;
        let composer_hint = if self.agent.session_ready {
            "Ask Cursor Agent…"
        } else {
            "Connect Cursor to start…"
        };
        let mut open_menu = self.agent_menu.clone();
        if !self.agent.session_ready || self.agent.active {
            open_menu = None;
        }
        let mut menu_anchor = session_menu_anchor;
        let mut menu_toggled = session_menu_toggled;
        ui.painter()
            .rect_filled(composer, 0.0, Color32::from_rgb(24, 24, 27));
        ui.painter().hline(
            composer.x_range(),
            composer.top() + 0.5,
            egui::Stroke::new(1.0, Color32::from_rgb(48, 48, 54)),
        );
        let composer_content = agent_composer_content(composer);
        let activity_height = if self.agent.active { 22.0 } else { 0.0 };
        if self.agent.active {
            let activity = composer_content.with_max_y(composer_content.top() + activity_height);
            ui.scope_builder(
                UiBuilder::new()
                    .id_salt("agent_activity")
                    .max_rect(activity)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.add(
                        egui::Spinner::new()
                            .size(12.0)
                            .color(Color32::from_rgb(91, 196, 216)),
                    );
                    ui.label(
                        RichText::new("Working")
                            .size(11.5)
                            .color(Color32::from_rgb(151, 164, 181)),
                    );
                },
            );
        }
        let footer = egui::Rect::from_min_max(
            egui::pos2(composer_content.left(), composer_content.bottom() - 30.0),
            composer_content.right_bottom(),
        );
        let input_rect = egui::Rect::from_min_max(
            egui::pos2(
                composer_content.left(),
                composer_content.top() + activity_height,
            ),
            egui::pos2(composer_content.right(), footer.top() - 4.0),
        );
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("agent_composer_region")
                .max_rect(input_rect)
                .layout(Layout::top_down(Align::LEFT)),
            |ui| {
                ScrollArea::vertical()
                    .id_salt("agent_prompt_scroll")
                    .max_height(input_rect.height())
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false])
                    .scroll_source(egui::scroll_area::ScrollSource {
                        mouse_wheel: !menu_owns_wheel,
                        ..Default::default()
                    })
                    .content_margin(0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let prompt_id = Id::new("agent_prompt");
                        let cursor_at_start = egui::TextEdit::load_state(ui.ctx(), prompt_id)
                            .and_then(|state| state.cursor.char_range())
                            .is_some_and(|range| {
                                range.primary.index == egui::text::CharIndex(0)
                                    && range.secondary.index == egui::text::CharIndex(0)
                            });
                        let history_key = (ui.memory(|memory| memory.has_focus(prompt_id))
                            && cursor_at_start)
                            .then(|| {
                                ui.input(|input| {
                                    if input.modifiers == egui::Modifiers::NONE
                                        && input.key_pressed(Key::ArrowUp)
                                    {
                                        Some((Key::ArrowUp, true))
                                    } else if input.modifiers == egui::Modifiers::NONE
                                        && input.key_pressed(Key::ArrowDown)
                                    {
                                        Some((Key::ArrowDown, false))
                                    } else {
                                        None
                                    }
                                })
                            })
                            .flatten();
                        if let Some((key, older)) = history_key
                            && self.navigate_agent_prompt_history(older)
                        {
                            history_navigated = true;
                            ui.memory_mut(|memory| {
                                memory.move_focus(egui::FocusDirection::None);
                            });
                            ui.input_mut(|input| {
                                input.consume_key(egui::Modifiers::NONE, key);
                            });
                        }
                        let input = ui.add_enabled(
                            composer_enabled,
                            TextEdit::multiline(&mut self.agent.prompt)
                                .id(prompt_id)
                                .hint_text(composer_hint)
                                .desired_rows(3)
                                .desired_width(f32::INFINITY)
                                .return_key(egui::KeyboardShortcut::new(
                                    egui::Modifiers::SHIFT,
                                    Key::Enter,
                                ))
                                .frame(egui::Frame::NONE),
                        );
                        if history_navigated
                            && let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), prompt_id)
                        {
                            state
                                .cursor
                                .set_char_range(Some(egui::text::CCursorRange::one(
                                    egui::text::CCursor::new(0),
                                )));
                            egui::TextEdit::store_state(ui.ctx(), prompt_id, state);
                        }
                        let input_changed = input.changed();
                        if input_changed {
                            self.agent_prompt_history_index = None;
                            self.agent_prompt_history_draft.clear();
                        }
                        prompt_changed = history_navigated || input_changed;
                        submit_shortcut = input.has_focus()
                            && ui.input(|input| {
                                !input.modifiers.shift && input.key_pressed(Key::Enter)
                            });
                    });
            },
        );
        if history_navigated {
            ui.memory_mut(|memory| memory.request_focus(Id::new("agent_prompt")));
        }
        if composer_enabled
            && let Some(query) = slash_command_query(&self.agent.prompt)
            && (prompt_changed || matches!(open_menu, Some(AgentMenu::Commands(_))))
        {
            open_menu = Some(AgentMenu::Commands(query.to_owned()));
            menu_anchor = Some(input_rect);
        } else if prompt_changed && matches!(open_menu, Some(AgentMenu::Commands(_))) {
            open_menu = None;
        }
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("agent_composer_footer")
                .max_rect(footer)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                ui.add_enabled_ui(!self.agent.active, |ui| {
                    let run_everything = self
                        .agent_run_everything
                        .or_else(|| run_everything_state(&self.agent.commands))
                        .unwrap_or(false);
                    let menu = AgentMenu::Permissions;
                    let selector = agent_selector_button(
                        ui,
                        if run_everything { "Allow all" } else { "Ask" },
                        "Permissions",
                    );
                    if selector.clicked() {
                        open_menu = (open_menu.as_ref() != Some(&menu)).then_some(menu.clone());
                        menu_toggled = true;
                    }
                    if open_menu.as_ref() == Some(&menu) {
                        menu_anchor = Some(selector.rect);
                    }
                    if !has_config_mode && !self.agent.modes.is_empty() {
                        let current = self.agent.current_mode.as_deref().unwrap_or_default();
                        let current_name = self
                            .agent
                            .modes
                            .iter()
                            .find(|mode| mode.id == current)
                            .map_or(current, |mode| mode.name.as_str());
                        let menu = AgentMenu::Mode;
                        let selector = agent_selector_button(ui, current_name, "Mode");
                        if selector.clicked() {
                            open_menu = (open_menu.as_ref() != Some(&menu)).then_some(menu.clone());
                            menu_toggled = true;
                        }
                        if open_menu.as_ref() == Some(&menu) {
                            menu_anchor = Some(selector.rect);
                        }
                    }
                    for option in &self.agent.config_options {
                        match &option.value {
                            ConfigValue::Select(current) => {
                                let current_name = option
                                    .options
                                    .iter()
                                    .find(|value| value.id == *current)
                                    .map_or_else(
                                        || {
                                            if is_model_config(&option.id, &option.name) {
                                                model_display_name(current, current)
                                            } else {
                                                Cow::Borrowed(current.as_str())
                                            }
                                        },
                                        |value| {
                                            if is_model_config(&option.id, &option.name) {
                                                model_display_name(&value.id, &value.name)
                                            } else {
                                                Cow::Borrowed(value.name.as_str())
                                            }
                                        },
                                    );
                                let menu = AgentMenu::Config(option.id.clone());
                                let selector = agent_selector_button(
                                    ui,
                                    &current_name,
                                    option.description.as_deref().unwrap_or(&option.name),
                                );
                                if selector.clicked() {
                                    open_menu =
                                        (open_menu.as_ref() != Some(&menu)).then_some(menu.clone());
                                    menu_toggled = true;
                                }
                                if open_menu.as_ref() == Some(&menu) {
                                    menu_anchor = Some(selector.rect);
                                }
                            }
                            ConfigValue::Boolean(current) => {
                                let selected =
                                    ui.selectable_label(*current, &option.name).on_hover_text(
                                        option.description.as_deref().unwrap_or(&option.name),
                                    );
                                if selected.clicked() {
                                    config_changes
                                        .push((option.id.clone(), ConfigValue::Boolean(!current)));
                                }
                            }
                        }
                    }
                    if let Some(usage) = &self.agent.usage {
                        let cost = usage
                            .cost
                            .as_deref()
                            .map_or(String::new(), |cost| format!(" · {cost}"));
                        ui.label(
                            RichText::new(format!("{} / {}{cost}", usage.used, usage.size))
                                .size(10.0)
                                .weak(),
                        )
                        .on_hover_text("Context usage");
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if self.agent.active {
                        let response = ui
                            .add(
                                egui::Button::new("")
                                    .fill(Color32::from_rgb(65, 65, 72))
                                    .corner_radius(6)
                                    .min_size(egui::vec2(32.0, 30.0)),
                            )
                            .on_hover_text("Stop");
                        ui.painter().rect_filled(
                            egui::Rect::from_center_size(
                                response.rect.center(),
                                egui::vec2(8.0, 8.0),
                            ),
                            1.5,
                            Color32::from_rgb(230, 232, 237),
                        );
                        cancel = response.clicked();
                    } else {
                        let ready =
                            self.agent.session_ready && !self.agent.prompt.trim().is_empty();
                        let (fill, color) = agent_send_button_colors(ready);
                        let response = ui
                            .add_enabled(
                                ready,
                                egui::Button::new("")
                                    .fill(fill)
                                    .stroke(egui::Stroke::NONE)
                                    .corner_radius(6)
                                    .min_size(egui::vec2(32.0, 30.0)),
                            )
                            .on_hover_text("Send (Enter)");
                        let center = response.rect.center();
                        ui.painter().line_segment(
                            [
                                egui::pos2(center.x, center.y + 5.0),
                                egui::pos2(center.x, center.y - 5.0),
                            ],
                            egui::Stroke::new(1.5, color),
                        );
                        ui.painter().line_segment(
                            [
                                egui::pos2(center.x - 4.0, center.y - 1.0),
                                egui::pos2(center.x, center.y - 5.0),
                            ],
                            egui::Stroke::new(1.5, color),
                        );
                        ui.painter().line_segment(
                            [
                                egui::pos2(center.x, center.y - 5.0),
                                egui::pos2(center.x + 4.0, center.y - 1.0),
                            ],
                            egui::Stroke::new(1.5, color),
                        );
                        send = response.clicked();
                    }
                });
            },
        );

        let mut menu_popup = None;
        if let (Some(menu), Some(anchor)) = (open_menu.as_ref(), menu_anchor) {
            let item_count = match menu {
                AgentMenu::Sessions => self
                    .agent
                    .sessions
                    .as_ref()
                    .map(|sessions| sessions.len().max(1)),
                AgentMenu::Commands(query) => Some(
                    self.agent
                        .commands
                        .iter()
                        .filter(|command| command_matches(&command.name, query))
                        .count()
                        .max(1),
                ),
                AgentMenu::Permissions => Some(2),
                AgentMenu::Mode => Some(self.agent.modes.len()),
                AgentMenu::Config(id) => self
                    .agent
                    .config_options
                    .iter()
                    .find(|option| option.id == *id)
                    .map(|option| option.options.len()),
            };
            if let Some(item_count) = item_count {
                if self.agent_menu.as_ref() != Some(menu) {
                    self.agent_menu_scroll_y = 0.0;
                }
                let row_height = match menu {
                    AgentMenu::Commands(_) => AGENT_COMMAND_ROW_HEIGHT,
                    AgentMenu::Sessions => AGENT_SESSION_ROW_HEIGHT,
                    _ => AGENT_MENU_ROW_HEIGHT,
                };
                let popup = if matches!(menu, AgentMenu::Sessions) {
                    agent_session_menu_rect(transcript, anchor, item_count)
                } else {
                    agent_menu_rect(transcript, anchor, item_count, row_height)
                };
                menu_popup = Some(popup);
                let max_scroll =
                    (item_count as f32 * row_height - (popup.height() - 16.0)).max(0.0);
                let wheel_delta = ui.input(|input| {
                    input
                        .pointer
                        .hover_pos()
                        .filter(|pointer| popup.contains(*pointer))
                        .map_or(0.0, |_| input.smooth_scroll_delta.y)
                });
                if wheel_delta != 0.0 {
                    self.agent_menu_scroll_y =
                        (self.agent_menu_scroll_y - wheel_delta).clamp(0.0, max_scroll);
                    ui.input_mut(|input| input.smooth_scroll_delta.y = 0.0);
                    ui.ctx().request_repaint();
                }
                self.agent_menu_scroll_y = self.agent_menu_scroll_y.min(max_scroll);
                let mut selected = false;
                let mut scroll_y = self.agent_menu_scroll_y;
                ui.scope_builder(
                    UiBuilder::new()
                        .id_salt("agent_menu")
                        .max_rect(popup)
                        .layout(Layout::top_down(Align::LEFT)),
                    |ui| {
                        ui.painter().add(egui::Shadow {
                            offset: [0, 4],
                            blur: 16,
                            spread: 0,
                            color: Color32::from_black_alpha(120),
                        }
                        .as_shape(popup, 9));
                        ui.set_clip_rect(popup);
                        ui.painter()
                            .rect_filled(popup, 9.0, Color32::from_rgb(26, 27, 31));
                        ui.painter().rect_stroke(
                            popup,
                            9.0,
                            egui::Stroke::new(1.0, Color32::from_rgb(57, 61, 70)),
                            egui::StrokeKind::Inside,
                        );
                        ui.scope_builder(
                            UiBuilder::new()
                                .id_salt("agent_menu_content")
                                .max_rect(popup.shrink2(egui::vec2(6.0, 8.0)))
                                .layout(Layout::top_down_justified(Align::LEFT)),
                            |ui| {
                                let list_height = ui.available_height();
                                let output = ScrollArea::vertical()
                                    .id_salt(("agent_menu_values", menu))
                                    .max_height(list_height)
                                    .auto_shrink([false, false])
                                    .scroll_source(egui::scroll_area::ScrollSource::SCROLL_BAR)
                                    .vertical_scroll_offset(scroll_y)
                                    .show(ui, |ui| {
                                        ui.spacing_mut().interact_size.y = row_height;
                                        ui.spacing_mut().item_spacing.y = 0.0;
                                        match menu {
                                        AgentMenu::Sessions => {
                                            if let Some(sessions) = &self.agent.sessions {
                                                if sessions.is_empty() {
                                                    ui.add_sized(
                                                        [
                                                            ui.available_width(),
                                                            AGENT_SESSION_ROW_HEIGHT,
                                                        ],
                                                        Label::new(
                                                            RichText::new("No previous sessions")
                                                                .weak(),
                                                        ),
                                                    );
                                                }
                                                for session in sessions {
                                                    let (open, remove) =
                                                        agent_session_row(ui, session);
                                                    if open {
                                                        session_load = Some(session.id.clone());
                                                        selected = true;
                                                    }
                                                    if remove {
                                                        session_remove = Some(session.id.clone());
                                                    }
                                                }
                                            }
                                        }
                                        AgentMenu::Commands(query) => {
                                            let mut matches = 0;
                                            for command in &self.agent.commands {
                                                if !command_matches(&command.name, query) {
                                                    continue;
                                                }
                                                matches += 1;
                                                let label =
                                                    command.input_hint.as_ref().map_or_else(
                                                        || format!("/{}", command.name),
                                                        |hint| format!("/{} {hint}", command.name),
                                                    );
                                                if ui
                                                    .selectable_label(false, label)
                                                    .on_hover_text(&command.description)
                                                    .clicked()
                                                {
                                                    self.agent.prompt =
                                                        format!("/{} ", command.name);
                                                    self.agent_prompt_history_index = None;
                                                    self.agent_prompt_history_draft.clear();
                                                    ui.memory_mut(|memory| {
                                                        memory
                                                            .request_focus(Id::new("agent_prompt"));
                                                    });
                                                    selected = true;
                                                }
                                            }
                                            if matches == 0 {
                                                ui.add_sized(
                                                    [
                                                        ui.available_width(),
                                                        AGENT_COMMAND_ROW_HEIGHT,
                                                    ],
                                                    Label::new(
                                                        RichText::new("No matching commands")
                                                            .weak(),
                                                    ),
                                                );
                                            }
                                        }
                                        AgentMenu::Permissions => {
                                            for (enabled, label, description) in [
                                                (
                                                    false,
                                                    "Ask",
                                                    "Ask before tools that are not already allowed",
                                                ),
                                                (
                                                    true,
                                                    "Allow all",
                                                    "Approve every Cursor tool unless explicitly denied",
                                            ),
                                        ] {
                                            if agent_menu_option(
                                                ui,
                                                label,
                                                self.agent_run_everything.unwrap_or(false)
                                                    == enabled,
                                                row_height,
                                            )
                                                .on_hover_text(description)
                                                .clicked()
                                                {
                                                    run_everything_change = Some(enabled);
                                                    selected = true;
                                                }
                                            }
                                        }
                                        AgentMenu::Mode => {
                                            let current = self
                                                .agent
                                                .current_mode
                                                .as_deref()
                                                .unwrap_or_default();
                                            for mode in &self.agent.modes {
                                                let response = agent_menu_option(
                                                    ui,
                                                    &mode.name,
                                                    current == mode.id,
                                                    row_height,
                                                );
                                                let response = if let Some(description) = mode
                                                    .description
                                                    .as_deref()
                                                    .filter(|description| !description.is_empty())
                                                {
                                                    response.on_hover_text(description)
                                                } else {
                                                    response
                                                };
                                                if response.clicked() {
                                                    mode_change = Some(mode.id.clone());
                                                    selected = true;
                                                }
                                            }
                                        }
                                        AgentMenu::Config(id) => {
                                            if let Some(option) = self
                                                .agent
                                                .config_options
                                                .iter()
                                                .find(|option| option.id == *id)
                                                && let ConfigValue::Select(current) = &option.value
                                            {
                                                for value in &option.options {
                                                    let label = if is_model_config(
                                                        &option.id,
                                                        &option.name,
                                                    ) {
                                                        model_display_name(&value.id, &value.name)
                                                    } else {
                                                        Cow::Borrowed(value.name.as_str())
                                                    };
                                                    let response = agent_menu_option(
                                                        ui,
                                                        &label,
                                                        value.id == *current,
                                                        row_height,
                                                    );
                                                    let response = if let Some(description) =
                                                        value.description.as_deref().filter(
                                                            |description| !description.is_empty(),
                                                        ) {
                                                        response.on_hover_text(description)
                                                    } else {
                                                        response
                                                    };
                                                    if response.clicked() {
                                                        config_changes.push((
                                                            option.id.clone(),
                                                            ConfigValue::Select(value.id.clone()),
                                                        ));
                                                        selected = true;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    });
                                scroll_y = output.state.offset.y;
                            },
                        );
                    },
                );
                self.agent_menu_scroll_y = scroll_y;
                let close = selected
                    || ui.input(|input| input.key_pressed(Key::Escape))
                    || (!menu_toggled
                        && ui.input(|input| {
                            input.pointer.any_click()
                                && input
                                    .pointer
                                    .interact_pos()
                                    .is_some_and(|position| !popup.contains(position))
                        }));
                if close {
                    open_menu = None;
                }
            } else {
                open_menu = None;
            }
        } else if open_menu.is_some() {
            open_menu = None;
        }
        if self.agent_menu != open_menu {
            self.agent_menu_scroll_y = 0.0;
        }
        if open_menu.is_none() {
            menu_popup = None;
        }
        self.agent_menu_popup = menu_popup;
        self.agent_menu = open_menu;

        for (request_id, option_id) in permission_decisions {
            if self.agent.decide_permission(request_id, &option_id)
                && let Some(controller) = &self.agent_controller
            {
                let _ = controller.send(AgentCommand::DecidePermission {
                    request_id,
                    option_id,
                });
            }
        }
        for (request_id, response) in interaction_responses {
            if self.agent.answer_interaction(request_id)
                && let Some(controller) = &self.agent_controller
            {
                let _ = controller.send(AgentCommand::RespondInteraction {
                    request_id,
                    response,
                });
            }
        }
        if let Some(enabled) = run_everything_change
            && let Some(controller) = &self.agent_controller
        {
            match controller.send(AgentCommand::SetRunEverything(enabled)) {
                Ok(()) => {
                    self.agent_run_everything = Some(enabled);
                }
                Err(error) => self.show_error(error),
            }
        }
        if let Some(controller) = &self.agent_controller {
            if let Some(session_id) = session_remove {
                let _ = controller.send(AgentCommand::RemoveSession(session_id));
            }
            if let Some(session_id) = session_load {
                let _ = controller.send(AgentCommand::LoadSession(session_id));
            }
            if let Some(mode) = mode_change {
                let _ = controller.send(AgentCommand::SetMode(mode));
            }
            for (id, value) in config_changes {
                let _ = controller.send(AgentCommand::SetConfig { id, value });
            }
            if cancel {
                let _ = controller.send(AgentCommand::Cancel);
            }
        }
        if send || submit_shortcut {
            self.queue_agent_prompt();
        }
        if new_session && let Some(controller) = &self.agent_controller {
            let _ = controller.send(AgentCommand::NewSession);
        }
        if let Some(method) = authenticate
            && let Some(controller) = &self.agent_controller
        {
            let _ = controller.send(AgentCommand::Authenticate(method));
        }
        if reconnect {
            self.reconnect_agent(ui.ctx());
        }
    }

    fn draw_find(&mut self, ui: &mut egui::Ui) {
        if !self.find_open {
            return;
        }
        let ctx = ui.ctx().clone();
        let query_focused = ctx.memory(|memory| memory.has_focus(Id::new("file_search_query")));
        let (enter, backwards, mut close) = ctx.input(|input| {
            (
                query_focused && input.key_pressed(Key::Enter),
                input.modifiers.shift,
                input.key_pressed(Key::Escape),
            )
        });
        let mut query_changed = false;
        let mut previous = false;
        let mut next = false;
        let count = if self.find_matches.is_empty() {
            "0 / 0".to_owned()
        } else {
            format!("{} / {}", self.find_selected + 1, self.find_matches.len())
        };
        let rect = ui.max_rect();
        ui.painter()
            .rect_filled(rect, 0.0, Color32::from_rgb(26, 26, 28));
        ui.painter().hline(
            rect.x_range(),
            rect.top(),
            egui::Stroke::new(1.0, Color32::from_rgb(53, 53, 59)),
        );
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(8.0, 6.0)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                let input_width = (ui.available_width() - 142.0).max(40.0);
                let response = egui::Frame::new()
                    .fill(Color32::from_rgb(31, 31, 34))
                    .inner_margin(egui::Margin::symmetric(6, 3))
                    .corner_radius(4)
                    .show(ui, |ui| {
                        ui.set_width((input_width - 12.0).max(28.0));
                        ui.add_sized(
                            egui::vec2(ui.available_width(), 20.0),
                            TextEdit::singleline(&mut self.find_query)
                                .id(Id::new("file_search_query"))
                                .font(FontId::proportional(13.0))
                                .hint_text("Find in current file…")
                                .frame(egui::Frame::NONE),
                        )
                    })
                    .inner;
                ui.add(Label::new(
                    RichText::new(&count).monospace().size(11.0).weak(),
                ));
                previous = chevron_icon_button(ui, true, "Previous match (Shift+Enter)").clicked();
                next = chevron_icon_button(ui, false, "Next match (Enter)").clicked();
                close |= close_icon_button(ui).clicked();
                if self.focus_find {
                    response.request_focus();
                    self.focus_find = false;
                }
                query_changed = response.changed();
            },
        );
        if query_changed {
            self.find_selected = 0;
            self.find_match_revision = u64::MAX;
            self.refresh_find_matches();
            self.scroll_to_find_match = !self.find_matches.is_empty();
            ctx.request_repaint();
        } else if (enter || previous || next) && !self.find_matches.is_empty() {
            self.find_selected = next_find_match(
                self.find_selected,
                self.find_matches.len(),
                previous || (enter && backwards),
            );
            self.highlight_cache.find_valid = false;
            self.scroll_to_find_match = true;
            ctx.request_repaint();
        }
        if close {
            self.find_open = false;
            self.focus_editor = self.active_tab.is_some();
        }
    }

    fn draw_search(&mut self, root: &mut egui::Ui) {
        if !self.search_open {
            return;
        }
        let ctx = root.ctx().clone();
        let results = self.search.results();
        let hit_count = results.files.len() + results.contents.len();
        if hit_count == 0 {
            self.search_selected = 0;
        } else {
            self.search_selected = self.search_selected.min(hit_count - 1);
        }

        let (down, up, enter, escape) = ctx.input(|input| {
            (
                input.key_pressed(Key::ArrowDown),
                input.key_pressed(Key::ArrowUp),
                input.key_pressed(Key::Enter),
                input.key_pressed(Key::Escape),
            )
        });
        let (selection, scroll_to_selection) =
            search_selection_after_navigation(self.search_selected, hit_count, down, up);
        self.search_selected = selection;

        let mut query_changed = false;
        let mut selected_path = enter
            .then(|| search_hit(results, self.search_selected).map(|hit| hit.path.clone()))
            .flatten();
        let empty_query = self.search_query.trim().is_empty();
        let palette_size = egui::vec2(680.0, if empty_query { 185.0 } else { 430.0 });
        let screen = ctx.content_rect();
        let palette_rect = egui::Rect::from_min_size(
            egui::pos2(
                screen.center().x - palette_size.x / 2.0,
                screen.top() + 48.0,
            ),
            palette_size,
        );
        let palette_frame = egui::Frame::window(&ctx.style_of(ctx.theme()))
            .fill(Color32::from_rgb(24, 24, 26))
            .stroke(egui::Stroke::new(
                1.0,
                Color32::from_rgba_unmultiplied(255, 255, 255, 22),
            ))
            .inner_margin(14)
            .corner_radius(12)
            .shadow(egui::Shadow {
                offset: [0, 8],
                blur: 28,
                spread: 2,
                color: Color32::from_black_alpha(150),
            });
        let mut palette = root.new_child(
            UiBuilder::new()
                .id_salt("project_search")
                .layer_id(egui::LayerId::new(
                    egui::Order::Foreground,
                    Id::new("project_search"),
                ))
                .max_rect(palette_rect)
                .layout(Layout::top_down(Align::Min)),
        );
        palette.set_clip_rect(screen);
        palette.interact(
            palette_rect,
            Id::new("project_search_surface"),
            Sense::click(),
        );
        palette
            .painter()
            .add(palette_frame.paint(palette_rect.shrink(15.0)));
        palette.scope_builder(
            UiBuilder::new()
                .max_rect(palette_rect.shrink(15.0))
                .layout(Layout::top_down(Align::Min)),
            |ui| {
                ui.visuals_mut().selection.bg_fill = Color32::from_rgb(30, 83, 94);
                ui.visuals_mut().selection.stroke.color = Color32::from_rgb(126, 228, 239);
                ui.visuals_mut().widgets.hovered.weak_bg_fill = Color32::from_rgb(35, 35, 39);
                ui.visuals_mut().widgets.hovered.bg_fill = Color32::from_rgb(35, 35, 39);
                ui.spacing_mut().item_spacing.y = 4.0;

                ui.horizontal(|ui| {
                    ui.label(RichText::new("Search project").size(15.0).strong());
                    ui.with_layout(Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("Up/Down navigate   Enter open   Esc close")
                                .small()
                                .weak(),
                        );
                    });
                });
                ui.add_space(8.0);
                let response = egui::Frame::new()
                    .fill(Color32::from_rgb(33, 33, 37))
                    .inner_margin(egui::Margin::symmetric(10, 7))
                    .corner_radius(7)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.add_sized(
                                egui::vec2(ui.available_width(), 26.0),
                                TextEdit::singleline(&mut self.search_query)
                                    .id(Id::new("project_search_query"))
                                    .hint_text("Search files and contents…")
                                    .desired_width(f32::INFINITY)
                                    .frame(egui::Frame::NONE),
                            )
                        })
                        .inner
                    })
                    .inner;
                if self.focus_search {
                    response.request_focus();
                    self.focus_search = false;
                }
                query_changed = response.changed();
                ui.add_space(8.0);

                ScrollArea::vertical()
                    .id_salt("project_search_results")
                    .auto_shrink([false, false])
                    .max_height(if empty_query { 54.0 } else { 300.0 })
                    .show(ui, |ui| {
                        if self.search_query.trim().is_empty() {
                            ui.add_space(20.0);
                            ui.vertical_centered(|ui| {
                                ui.label(RichText::new("Find anything in this project").strong());
                                ui.label(
                                    RichText::new("Type a filename or text from a file")
                                        .small()
                                        .weak(),
                                );
                            });
                            return;
                        }
                        if results.query != self.search_query.trim() {
                            ui.label(RichText::new("Searching…").weak());
                            return;
                        }

                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("FILES")
                                    .size(11.0)
                                    .strong()
                                    .color(Color32::from_rgb(145, 153, 169)),
                            );
                            ui.label(
                                RichText::new(results.files.len().to_string())
                                    .small()
                                    .weak(),
                            );
                        });
                        for (index, hit) in results.files.iter().enumerate() {
                            let mut job = LayoutJob::default();
                            job.wrap.max_width = ui.available_width() - 18.0;
                            job.wrap.break_anywhere = true;
                            job.append(
                                &format!("  {}", hit.relative),
                                0.0,
                                TextFormat {
                                    font_id: FontId::monospace(13.5),
                                    color: Color32::from_rgb(205, 210, 220),
                                    ..TextFormat::default()
                                },
                            );
                            let response =
                                search_result_row(ui, self.search_selected == index, job, 30.0);
                            if scroll_to_selection && self.search_selected == index {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                selected_path = Some(hit.path.clone());
                            }
                        }

                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("FILE CONTENT")
                                    .size(11.0)
                                    .strong()
                                    .color(Color32::from_rgb(145, 153, 169)),
                            );
                            ui.label(
                                RichText::new(results.contents.len().to_string())
                                    .small()
                                    .weak(),
                            );
                        });
                        for (offset, hit) in results.contents.iter().enumerate() {
                            let index = results.files.len() + offset;
                            let response = search_result_row(
                                ui,
                                self.search_selected == index,
                                content_result_job(
                                    hit,
                                    self.search_query.trim(),
                                    ui.available_width() - 18.0,
                                ),
                                44.0,
                            );
                            if scroll_to_selection && self.search_selected == index {
                                response.scroll_to_me(None);
                            }
                            if response.clicked() {
                                selected_path = Some(hit.path.clone());
                            }
                        }
                        if hit_count == 0 && results.complete {
                            ui.label(RichText::new("No matches").weak());
                        }
                    });
                ui.add_space(4.0);
                let status = if results.complete {
                    format!("{} files indexed", results.indexed_files)
                } else {
                    format!("Indexing… {} files ready", results.indexed_files)
                };
                ui.label(RichText::new(status).small().weak());
            },
        );

        if query_changed {
            self.search_selected = 0;
            if let Err(error) = self.search.set_query(&self.search_query) {
                self.show_error(error);
            }
        }
        if escape {
            self.search_open = false;
            self.focus_editor = self.active_tab.is_some();
        } else if let Some(path) = selected_path {
            self.search_open = false;
            self.request(PendingAction::Open(path));
        }
    }

    fn draw_tree(&mut self, ui: &mut egui::Ui) {
        let scroll_to_selected = self.tree_focused
            && !ui.ctx().egui_wants_keyboard_input()
            && self.pending.is_none()
            && self.tree_keyboard(ui);
        let output = self.tree_surface.show(
            ui,
            &self.tree.visible,
            self.tree.selected_index,
            scroll_to_selected,
        );
        if output.response.clicked() {
            self.tree_focused = true;
        }
        if let Some(index) = output.clicked.filter(|_| self.pending.is_none()) {
            let entry = self.tree.visible[index].entry.clone();
            self.tree_focused = true;
            self.focus_editor = false;
            ui.memory_mut(|memory| memory.surrender_focus(Id::new("editor")));
            self.tree.select(Some(entry.path.clone()));
            if entry.is_dir {
                if let Err(error) = self.tree.toggle(&entry.path) {
                    self.show_error(error);
                }
            } else {
                self.request(PendingAction::Open(entry.path));
            }
        }
    }

    fn tree_keyboard(&mut self, ui: &egui::Ui) -> bool {
        let entry_count = self.tree.visible.len();
        if entry_count == 0 {
            return false;
        }
        let (down, up, right, left, enter) = ui.input(|input| {
            (
                input.key_pressed(Key::ArrowDown),
                input.key_pressed(Key::ArrowUp),
                input.key_pressed(Key::ArrowRight),
                input.key_pressed(Key::ArrowLeft),
                input.key_pressed(Key::Enter),
            )
        });
        if !(down || up || right || left || enter) {
            return false;
        }
        let current = self.tree.selected_index.unwrap_or(0).min(entry_count - 1);
        let next = if down {
            Some((current + 1).min(entry_count - 1))
        } else if up {
            Some(current.saturating_sub(1))
        } else {
            None
        };
        if let Some(next) = next {
            let path = self.tree.visible[next].entry.path.clone();
            self.tree.select(Some(path));
        }
        let entry = self.tree.visible[next.unwrap_or(current)].entry.clone();
        if right && entry.is_dir {
            if !self.tree.expanded.contains(&entry.path)
                && let Err(error) = self.tree.toggle(&entry.path)
            {
                self.show_error(error);
            }
        } else if left && entry.is_dir {
            self.tree.collapse(&entry.path);
        } else if enter {
            if entry.is_dir {
                if let Err(error) = self.tree.toggle(&entry.path) {
                    self.show_error(error);
                }
            } else {
                self.request(PendingAction::Open(entry.path.clone()));
            }
        }
        next.is_some()
    }

    fn draw_editor(&mut self, ui: &mut egui::Ui) {
        let active_tab = self.active_tab;
        let markdown =
            active_tab.is_some_and(|index| markdown::is_markdown(&self.tabs[index].buffer.path));
        if !markdown {
            self.markdown_preview = false;
        } else if self.markdown_preview {
            let buffer = &self.tabs[active_tab.expect("checked above")].buffer;
            draw_markdown_preview(ui, &buffer.text, buffer.revision, &mut self.markdown_layout);
            return;
        }
        let find_open = self.find_open;
        let find_query = &self.find_query;
        let find_matches = &self.find_matches;
        let find_selected = self.find_selected;
        let bracket_pair = self.bracket_pair.clone();
        let scroll_character = self
            .scroll_to_find_match
            .then(|| find_matches.get(find_selected).cloned())
            .flatten()
            .map(|span| {
                active_tab.map_or(0, |index| {
                    self.tabs[index].buffer.text[..span.start].chars().count()
                })
            });
        let Some(index) = active_tab else {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, EDITOR_BACKGROUND);
            ui.centered_and_justified(|ui| {
                ui.label(RichText::new("Select a file to begin editing").weak());
            });
            return;
        };
        let tab = &mut self.tabs[index];
        let buffer = &mut tab.buffer;
        if buffer.large_file_warning {
            ui.colored_label(
                Color32::YELLOW,
                "Large file: syntax highlighting is disabled above 5 MiB.",
            );
        }

        let syntax = self
            .syntaxes
            .detect(&buffer.path, buffer.large_file_warning);
        let revision = buffer.revision;
        let syntax_name = syntax.name.clone();
        let cache = &mut self.highlight_cache;
        let highlighter = &self.highlighter;
        let syntaxes = &self.syntaxes;
        let large_file = buffer.large_file_warning;
        let mut highlight_error = None;
        let wrap_width = ui.available_width().max(1.0);
        if !cache.valid || cache.revision != revision || cache.syntax != syntax_name {
            if large_file {
                cache.job = plain_text_job(&buffer.text, wrap_width);
                cache.incremental = IncrementalHighlightCache::default();
            } else {
                match highlighter.highlight_job_incremental(
                    &buffer.text,
                    syntax,
                    syntaxes.set(),
                    wrap_width,
                    &mut cache.incremental,
                ) {
                    Ok(job) => cache.job = job,
                    Err(error) => {
                        highlight_error = Some(error);
                        cache.job = plain_text_job(&buffer.text, wrap_width);
                    }
                }
            }
            cache.revision = revision;
            cache.syntax.clone_from(&syntax_name);
            cache.valid = true;
            cache.find_valid = false;
        }
        let job = if find_open && !find_matches.is_empty() {
            if !cache.find_valid
                || cache.find_revision != revision
                || cache.find_query != *find_query
                || cache.find_selected != find_selected
            {
                cache.find_job = find_highlighted_job(&cache.job, find_matches, find_selected);
                cache.find_revision = revision;
                cache.find_query.clone_from(find_query);
                cache.find_selected = find_selected;
                cache.find_valid = true;
            }
            &cache.find_job
        } else {
            &cache.job
        };
        let galley_key = GalleyKey {
            revision,
            syntax: syntax_name,
            wrap_width: wrap_width.round().to_bits(),
            find: (find_open && !find_matches.is_empty())
                .then(|| (find_query.clone(), find_selected)),
            bracket_pair: bracket_pair.clone(),
        };
        if cache.galley_key.as_ref() != Some(&galley_key) {
            cache.galley_key = Some(galley_key);
            cache.presentation_revision = cache.presentation_revision.wrapping_add(1);
            cache.bracket_job = bracket_pair
                .as_ref()
                .map(|pair| bracket_highlighted_job(job, pair));
        }
        let job = presentation_job(job, cache.bracket_job.as_ref());
        let document = DocumentMetrics {
            revision: cache.presentation_revision,
            line_count: buffer.line_count(),
        };
        let output = tab.editor_surface.show_document(
            ui,
            &mut buffer.text,
            job,
            document,
            self.focus_editor,
            scroll_character,
        );
        if self.scroll_to_find_match {
            self.scroll_to_find_match = false;
        }
        self.focus_editor = false;
        if output.response.has_focus() {
            self.tree_focused = false;
        }
        if output.changed {
            buffer.mark_changed();
            self.highlight_cache.valid = false;
        }
        self.cursor = buffer.line_column(output.cursor);
        let bracket_pair_key = (buffer.revision, output.cursor);
        if self.bracket_pair_key != Some(bracket_pair_key) {
            let pair = (!buffer.large_file_warning)
                .then(|| match_bracket_pair(buffer, output.cursor))
                .flatten();
            self.bracket_pair_key = Some(bracket_pair_key);
            if self.bracket_pair != pair {
                self.bracket_pair = pair;
                ui.ctx().request_repaint();
            }
        }
        if let Some(error) = highlight_error {
            self.show_error(error);
        }
    }

    fn draw_dialogs(&mut self, ctx: &egui::Context) {
        if self.pending_agent_prompt {
            let mut save_and_run = false;
            let mut cancel = false;
            egui::Window::new("Save before running Agent")
                .id(Id::new("agent_save_dialog"))
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Cursor Agent reads files from disk. Save the current buffer first?");
                    ui.horizontal(|ui| {
                        save_and_run = ui.button("Save and Run").clicked();
                        cancel = ui.button("Cancel").clicked();
                    });
                });
            if save_and_run && self.save(None) {
                self.pending_agent_prompt = false;
                self.send_agent_prompt();
            } else if cancel {
                self.pending_agent_prompt = false;
            }
        }
        if self.pending.is_some() && !self.conflict && self.save_as.is_none() {
            let dialog_frame = egui::Frame::window(&ctx.style_of(ctx.theme()))
                .fill(Color32::from_rgb(28, 28, 31))
                .stroke(egui::Stroke::new(
                    1.0,
                    Color32::from_rgba_unmultiplied(255, 255, 255, 22),
                ))
                .inner_margin(18)
                .corner_radius(12)
                .shadow(egui::Shadow {
                    offset: [0, 8],
                    blur: 28,
                    spread: 2,
                    color: Color32::from_black_alpha(150),
                });
            egui::Window::new("Unsaved changes")
                .id(Id::new("unsaved_dialog"))
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .title_bar(false)
                .fade_in(false)
                .fixed_size(egui::vec2(360.0, 134.0))
                .collapsible(false)
                .resizable(false)
                .frame(dialog_frame)
                .show(ctx, |ui| {
                    ui.label(
                        RichText::new("Unsaved changes")
                            .size(16.0)
                            .strong()
                            .color(Color32::from_rgb(226, 230, 238)),
                    );
                    ui.add_space(5.0);
                    ui.label(
                        RichText::new("Save your changes before continuing?")
                            .color(Color32::from_rgb(145, 151, 164)),
                    );
                    ui.add_space(18.0);
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let save = egui::Button::new(
                            RichText::new("Save")
                                .strong()
                                .color(Color32::from_rgb(9, 28, 32)),
                        )
                        .fill(Color32::from_rgb(94, 210, 224))
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(6)
                        .min_size(egui::vec2(72.0, 32.0));
                        if ui.add(save).clicked() && self.save(None) {
                            self.finish_pending();
                        }
                        let discard = egui::Button::new(
                            RichText::new("Discard").color(Color32::from_rgb(224, 156, 160)),
                        )
                        .fill(Color32::from_rgb(40, 34, 37))
                        .stroke(egui::Stroke::NONE)
                        .corner_radius(6)
                        .min_size(egui::vec2(78.0, 32.0));
                        if ui.add(discard).clicked() {
                            self.discard_pending();
                        }
                        let cancel = egui::Button::new("Cancel")
                            .fill(Color32::from_rgb(39, 39, 43))
                            .stroke(egui::Stroke::NONE)
                            .corner_radius(6)
                            .min_size(egui::vec2(72.0, 32.0));
                        if ui.add(cancel).clicked() {
                            self.pending = None;
                            self.tree
                                .select(self.buffer().map(|buffer| buffer.path.clone()));
                        }
                    });
                });
        }
        if self.conflict {
            egui::Window::new("File changed on disk")
                .id(Id::new("conflict_dialog"))
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("The file changed outside Editur. It was not overwritten.");
                    ui.horizontal(|ui| {
                        if ui.button("Reload").clicked()
                            && let Some(index) = self.active_tab
                        {
                            let path = self.tabs[index].buffer.path.clone();
                            match load_buffer(&path) {
                                Ok(buffer) => {
                                    self.tabs[index] = FileTab::new(buffer);
                                    self.highlight_cache.valid = false;
                                    self.conflict = false;
                                    if self.pending.is_some() {
                                        self.finish_pending();
                                    }
                                }
                                Err(error) => self.show_error(error),
                            }
                        }
                        if ui.button("Save As…").clicked() {
                            let suggestion = self.buffer().map_or_else(String::new, |buffer| {
                                format!("{}.editur-copy", buffer.path.display())
                            });
                            self.save_as = Some(suggestion);
                            self.conflict = false;
                        }
                        if ui.button("Cancel").clicked() {
                            self.conflict = false;
                            self.pending = None;
                        }
                    });
                });
        }
        if self.save_as.is_some() {
            let mut save_clicked = false;
            let mut cancel_clicked = false;
            egui::Window::new("Save As")
                .id(Id::new("save_as_dialog"))
                .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .collapsible(false)
                .resizable(false)
                .show(ctx, |ui| {
                    ui.label("Destination path");
                    if let Some(path) = self.save_as.as_mut() {
                        ui.text_edit_singleline(path);
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Save").clicked() {
                            save_clicked = true;
                        }
                        if ui.button("Cancel").clicked() {
                            cancel_clicked = true;
                        }
                    });
                });
            if save_clicked {
                if let Some(path) = self.save_as.clone()
                    && self.save(Some(PathBuf::from(path)))
                {
                    self.save_as = None;
                    self.finish_pending();
                }
            } else if cancel_clicked {
                self.save_as = None;
                self.conflict = true;
            }
        }
    }
}

pub fn launch(target: OpenTarget, started: Instant) -> Result<(), String> {
    if launch_in_current_process(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
        cfg!(target_os = "macos") && std::env::var_os("__CFBundleIdentifier").is_some(),
    ) {
        return run(target, started);
    }
    if open_running(&target)? {
        return Ok(());
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate the Editur executable: {error}"))?;
    let path = target.file.as_ref().unwrap_or(&target.root);
    let mut command = Command::new(executable);
    #[cfg(target_os = "macos")]
    command
        .env_remove("__CFBundleIdentifier")
        .env_remove("XPC_SERVICE_NAME");
    command
        .arg("--resident")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot start the editor resident: {error}"))
}

#[doc(hidden)]
pub fn quit_running() -> Result<(), String> {
    if crate::instance::quit_running()? {
        Ok(())
    } else {
        Err("save or discard changes in the running editor before restarting".into())
    }
}

const fn launch_in_current_process(
    stdin_terminal: bool,
    stdout_terminal: bool,
    stderr_terminal: bool,
    macos_bundle_launch: bool,
) -> bool {
    !macos_bundle_launch && !(stdin_terminal || stdout_terminal || stderr_terminal)
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

pub fn run(target: OpenTarget, started: Instant) -> Result<(), String> {
    let listener = match claim(&target)? {
        Claim::Primary(listener) => listener,
        Claim::Forwarded => return Ok(()),
    };
    let mut event_loop = EventLoop::<InstanceEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    winit::platform::macos::EventLoopBuilderExtMacOS::with_default_menu(&mut event_loop, false);
    let event_loop = event_loop
        .build()
        .map_err(|error| format!("cannot create event loop: {error}"))?;
    let event_proxy = event_loop.create_proxy();
    spawn_listener(listener, event_proxy.clone())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let editor_started = Instant::now();
    let editor = EditorApp::new(target)?;
    if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
        eprintln!(
            "editur: editor state initialized in {:.2?}",
            editor_started.elapsed()
        );
    }
    let mut shell = Shell::new(editor, started, event_proxy);
    event_loop
        .run_app(&mut shell)
        .map_err(|error| format!("window event loop failed: {error}"))?;
    shell.fatal.map_or(Ok(()), Err)
}

struct Shell {
    editor: EditorApp,
    window: Option<Window>,
    renderer: Option<Renderer>,
    egui: Option<egui_winit::State>,
    repaint_at: Option<Instant>,
    pending_resize: Option<winit::dpi::PhysicalSize<u32>>,
    fatal: Option<String>,
    clipboard: Option<arboard::Clipboard>,
    modifiers: ModifiersState,
    started: Instant,
    first_frame_logged: bool,
    event_proxy: EventLoopProxy<InstanceEvent>,
}

fn repaint_deadline(delay: Duration, now: Instant) -> Option<Instant> {
    (delay != Duration::MAX).then(|| now + delay)
}

fn queue_resize(
    pending: &mut Option<winit::dpi::PhysicalSize<u32>>,
    size: winit::dpi::PhysicalSize<u32>,
) {
    *pending = Some(size);
}

fn repaint_delay_after_texture_update(delay: Duration, textures_updated: bool) -> Duration {
    if textures_updated {
        Duration::ZERO
    } else {
        delay
    }
}

fn install_repaint_wake(context: &egui::Context, wake: impl Fn() + Send + Sync + 'static) {
    context.set_request_repaint_callback(move |info| {
        if info.delay.is_zero() {
            wake();
        }
    });
}

fn disable_transient_egui_debug_overlays(context: &egui::Context) {
    #[cfg(debug_assertions)]
    context.all_styles_mut(|style| style.debug.warn_if_rect_changes_id = false);
    #[cfg(not(debug_assertions))]
    let _ = context;
}

fn system_clipboard(
    clipboard: &mut Option<arboard::Clipboard>,
) -> Result<&mut arboard::Clipboard, String> {
    if clipboard.is_none() {
        *clipboard = Some(
            arboard::Clipboard::new()
                .map_err(|error| format!("system clipboard is unavailable: {error}"))?,
        );
    }
    Ok(clipboard.as_mut().expect("clipboard was initialized"))
}

impl Shell {
    fn new(
        editor: EditorApp,
        started: Instant,
        event_proxy: EventLoopProxy<InstanceEvent>,
    ) -> Self {
        Self {
            editor,
            window: None,
            renderer: None,
            egui: None,
            repaint_at: None,
            pending_resize: None,
            fatal: None,
            clipboard: None,
            modifiers: ModifiersState::default(),
            started,
            first_frame_logged: false,
            event_proxy,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        self.fatal = Some(error);
        event_loop.exit();
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer), Some(state)) = (
            self.window.as_ref(),
            self.renderer.as_mut(),
            self.egui.as_mut(),
        ) else {
            return;
        };
        if let Some(size) = self.pending_resize.take() {
            #[cfg(target_os = "macos")]
            if let Err(error) = renderer.resize(window, size) {
                self.fail(event_loop, error);
                return;
            }
            #[cfg(not(target_os = "macos"))]
            renderer.resize(size);
        }
        let input = state.take_egui_input(window);
        let context = state.egui_ctx().clone();
        let output = context.run_ui(input, |root| self.editor.ui(root));
        if let Some(action) = self.editor.take_window_action() {
            match action {
                WindowAction::Close => self.editor.request_close(),
                WindowAction::Minimize => window.set_minimized(true),
                WindowAction::ToggleMaximize => window.set_maximized(!window.is_maximized()),
                WindowAction::Drag => {
                    if let Err(error) = window.drag_window() {
                        self.editor
                            .show_error(format!("cannot drag window: {error}"));
                    }
                }
            }
        }
        for command in &output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command
                && let Err(error) = system_clipboard(&mut self.clipboard).and_then(|clipboard| {
                    clipboard.set_text(text).map_err(|error| error.to_string())
                })
            {
                self.editor
                    .show_error(format!("cannot copy to system clipboard: {error}"));
            }
        }
        state.handle_platform_output_with_event_loop(window, event_loop, output.platform_output);
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        let textures_updated = !output.textures_delta.set.is_empty();
        #[cfg(target_os = "linux")]
        window.pre_present_notify();
        if let Err(error) =
            renderer.render(output.pixels_per_point, &primitives, &output.textures_delta)
        {
            self.fail(event_loop, error);
            return;
        }
        if !self.first_frame_logged {
            window.focus_window();
            if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
                eprintln!(
                    "editur: first editable frame in {:.2?}",
                    self.started.elapsed()
                );
            }
            self.first_frame_logged = true;
        }
        if self.editor.should_close {
            event_loop.exit();
            return;
        }
        let delay = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .map_or(Duration::MAX, |output| output.repaint_delay);
        let delay = repaint_delay_after_texture_update(delay, textures_updated);
        let now = Instant::now();
        if let Some(repaint_at) = repaint_deadline(delay, now) {
            self.repaint_at = Some(repaint_at);
            if delay.is_zero() {
                event_loop.set_control_flow(ControlFlow::Poll);
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(repaint_at));
            }
        } else {
            self.repaint_at = None;
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

impl ApplicationHandler<InstanceEvent> for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (pixels, width, height) = application_icon_rgba();
        let icon = match Icon::from_rgba(pixels.to_vec(), width, height) {
            Ok(icon) => icon,
            Err(error) => {
                self.fail(event_loop, format!("cannot load application icon: {error}"));
                return;
            }
        };
        let attributes = Window::default_attributes()
            .with_title("Editur")
            .with_inner_size(LogicalSize::new(1180, 760))
            .with_min_inner_size(LogicalSize::new(520, 320))
            .with_window_icon(Some(icon))
            .with_decorations(false);
        #[cfg(target_os = "macos")]
        let attributes = attributes.with_transparent(true);
        let window_started = Instant::now();
        #[cfg(target_os = "macos")]
        let window = create_macos_window_without_native_title(event_loop, attributes);
        #[cfg(not(target_os = "macos"))]
        let window = event_loop.create_window(attributes);
        let window = match window {
            Ok(window) => window,
            Err(error) => {
                self.fail(event_loop, format!("cannot create window: {error}"));
                return;
            }
        };
        let window_time = window_started.elapsed();
        let renderer_started = Instant::now();
        let renderer = match Renderer::new(&window) {
            Ok(renderer) => renderer,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        let renderer_time = renderer_started.elapsed();
        if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
            eprintln!(
                "editur: {} adapter: {}",
                renderer.backend_name(),
                renderer.adapter_name()
            );
            eprintln!("editur: window created in {window_time:.2?}");
            eprintln!("editur: renderer initialized in {renderer_time:.2?}");
        }
        let context = egui::Context::default();
        disable_transient_egui_debug_overlays(&context);
        let event_proxy = self.event_proxy.clone();
        install_repaint_wake(&context, move || {
            let _ = event_proxy.send_event(InstanceEvent::Wake);
        });
        let state = egui_winit::State::new(
            context,
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        #[cfg(target_os = "macos")]
        {
            activate_macos_application();
            window.focus_window();
        }
        self.egui = Some(state);
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.redraw(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if window.id() != window_id {
            return;
        }
        if let WindowEvent::ModifiersChanged(modifiers) = &event {
            self.modifiers = modifiers.state();
        }
        if let WindowEvent::KeyboardInput { event: key, .. } = &event
            && key.state.is_pressed()
            && key.physical_key == PhysicalKey::Code(KeyCode::KeyV)
            && (self.modifiers.control_key() || self.modifiers.super_key())
        {
            match system_clipboard(&mut self.clipboard)
                .and_then(|clipboard| clipboard.get_text().map_err(|error| error.to_string()))
            {
                Ok(text) => {
                    if let Some(state) = &mut self.egui {
                        state.set_clipboard_text(text);
                    }
                }
                Err(error) => self
                    .editor
                    .show_error(format!("cannot paste from system clipboard: {error}")),
            }
        }
        if !matches!(event, WindowEvent::RedrawRequested)
            && self
                .egui
                .as_mut()
                .is_some_and(|state| state.on_window_event(window, &event).repaint)
        {
            window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => {
                self.editor.request_close();
                if self.editor.should_close {
                    event_loop.exit();
                } else {
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                queue_resize(&mut self.pending_resize, size);
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                queue_resize(&mut self.pending_resize, window.inner_size());
                window.request_redraw();
            }
            WindowEvent::Focused(true) => {
                self.editor.reconcile_open_buffer();
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if self
            .repaint_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.repaint_at = None;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: InstanceEvent) {
        match event {
            InstanceEvent::Open(target, reply) => {
                self.editor.request_target(target);
                if let Some(window) = &self.window {
                    window.set_visible(true);
                    #[cfg(target_os = "macos")]
                    activate_macos_application();
                    window.focus_window();
                    window.request_redraw();
                }
                let _ = reply.send(true);
            }
            InstanceEvent::Quit(reply) => {
                let clean = self.editor.tabs.iter().all(|tab| !tab.buffer.dirty);
                let _ = reply.send(clean);
                if !clean {
                    self.editor
                        .show_error("Save or discard changes before updating Editur.".into());
                    if let Some(window) = &self.window {
                        window.set_visible(true);
                        window.focus_window();
                        window.request_redraw();
                    }
                }
            }
            InstanceEvent::Wake => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            InstanceEvent::Exit => event_loop.exit(),
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn create_macos_window_without_native_title(
    event_loop: &ActiveEventLoop,
    attributes: winit::window::WindowAttributes,
) -> Result<Window, winit::error::OsError> {
    use objc::{
        class,
        runtime::{self, Imp, Method, Object, Sel},
        sel, sel_impl,
    };

    unsafe extern "C" fn ignore_title(_: *mut Object, _: Sel, _: *mut Object) {}

    struct RestoreMethod {
        method: *mut Method,
        implementation: Imp,
    }

    impl Drop for RestoreMethod {
        fn drop(&mut self) {
            unsafe {
                runtime::method_setImplementation(self.method, self.implementation);
            }
        }
    }

    unsafe {
        // macOS 26 blocks for roughly two seconds when winit sets a title on a borderless window.
        // Editur draws its own titlebar, so suppress only that synchronous creation-time call.
        let method = runtime::class_getInstanceMethod(class!(NSWindow), sel!(setTitle:));
        if method.is_null() {
            return event_loop.create_window(attributes);
        }
        let replacement: Imp = std::mem::transmute(
            ignore_title as unsafe extern "C" fn(*mut Object, Sel, *mut Object),
        );
        let restore = RestoreMethod {
            method: method.cast_mut(),
            implementation: runtime::method_setImplementation(method.cast_mut(), replacement),
        };
        let window = event_loop.create_window(attributes);
        drop(restore);
        window
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
fn activate_macos_application() {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};

    unsafe {
        let application: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let modern: objc::runtime::BOOL =
            msg_send![application, respondsToSelector: sel!(activate)];
        if modern == objc::runtime::YES {
            let _: () = msg_send![application, activate];
        } else {
            let _: () = msg_send![application, activateIgnoringOtherApps: objc::runtime::YES];
        }
    }
}

fn application_icon_rgba() -> (&'static [u8], u32, u32) {
    (include_bytes!("../assets/icons/editur-64.rgba"), 64, 64)
}

fn search_result_row(
    ui: &mut egui::Ui,
    selected: bool,
    job: LayoutJob,
    min_height: f32,
) -> egui::Response {
    let mut row = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(9, 6))
        .corner_radius(6)
        .begin(ui);
    row.content_ui
        .set_min_width(row.content_ui.available_width());
    row.content_ui.set_min_height(min_height);
    row.content_ui.add(Label::new(job).wrap());
    let response = row.allocate_space(ui).interact(Sense::click());
    row.frame.fill = if selected {
        Color32::from_rgb(30, 74, 84)
    } else if response.hovered() {
        Color32::from_rgb(35, 35, 39)
    } else {
        Color32::TRANSPARENT
    };
    row.paint(ui);
    response
}

fn search_hit(results: &SearchResults, index: usize) -> Option<&SearchHit> {
    results.files.get(index).or_else(|| {
        results
            .contents
            .get(index.saturating_sub(results.files.len()))
    })
}

fn agent_selector_button(ui: &mut egui::Ui, label: &str, tooltip: &str) -> egui::Response {
    let idle_color = Color32::from_rgb(166, 175, 190);
    let idle_galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), FontId::proportional(12.0), idle_color);
    let width = (idle_galley.size().x + 20.0).max(40.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 30.0), Sense::click());
    let visual_rect = rect.translate(egui::vec2(0.0, 4.0));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let text_color = if response.hovered() {
        Color32::from_rgb(218, 224, 235)
    } else {
        idle_color
    };
    let galley = if response.hovered() {
        ui.painter()
            .layout_no_wrap(label.to_owned(), FontId::proportional(12.0), text_color)
    } else {
        idle_galley
    };
    ui.painter().galley(
        egui::pos2(
            visual_rect.left(),
            visual_rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        text_color,
    );
    let tip = egui::pos2(visual_rect.right() - 5.5, visual_rect.center().y + 2.0);
    let arrow_color = if response.hovered() {
        Color32::from_rgb(230, 235, 243)
    } else {
        Color32::from_rgb(190, 199, 214)
    };
    let stroke = egui::Stroke::new(1.4, arrow_color);
    ui.painter()
        .line_segment([tip + egui::vec2(-3.5, -3.0), tip], stroke);
    ui.painter()
        .line_segment([tip, tip + egui::vec2(3.5, -3.0)], stroke);
    response.on_hover_text(tooltip)
}

fn agent_send_button_colors(ready: bool) -> (Color32, Color32) {
    if ready {
        (
            Color32::from_rgb(94, 210, 224),
            Color32::from_rgb(9, 28, 32),
        )
    } else {
        (
            Color32::from_rgb(35, 43, 47),
            Color32::from_rgb(92, 140, 149),
        )
    }
}

fn is_model_config(id: &str, name: &str) -> bool {
    id.eq_ignore_ascii_case("model") || name.eq_ignore_ascii_case("model")
}

fn model_display_name<'a>(id: &str, name: &'a str) -> Cow<'a, str> {
    let machine_like = name.eq_ignore_ascii_case(id)
        || (!name.chars().any(char::is_whitespace) && name.contains(['-', '_']));
    if !machine_like {
        return Cow::Borrowed(name);
    }
    let mut words: Vec<String> = Vec::new();
    for part in name.split(['-', '_']).filter(|part| !part.is_empty()) {
        if part.bytes().all(|byte| byte.is_ascii_digit())
            && let Some(version) = words.last_mut().filter(|word| {
                word.bytes()
                    .all(|byte| byte.is_ascii_digit() || byte == b'.')
            })
        {
            version.push('.');
            version.push_str(part);
            continue;
        }
        let lower = part.to_ascii_lowercase();
        words.push(match lower.as_str() {
            "gpt" | "glm" | "llm" | "oss" => lower.to_ascii_uppercase(),
            _ => {
                let mut chars = lower.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_ascii_uppercase().to_string() + chars.as_str()
                })
            }
        });
    }
    Cow::Owned(words.join(" "))
}

fn agent_menu_option(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    row_height: f32,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    let fill = if selected {
        Color32::from_rgb(38, 55, 62)
    } else if response.hovered() {
        Color32::from_rgb(39, 40, 46)
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 5.0, fill);
    }
    let color = if selected {
        Color32::from_rgb(213, 231, 235)
    } else if response.hovered() {
        Color32::from_rgb(230, 233, 240)
    } else {
        Color32::from_rgb(184, 191, 203)
    };
    ui.painter().text(
        egui::pos2(rect.left() + 9.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(12.5),
        color,
    );
    if selected {
        let center = egui::pos2(rect.right() - 12.0, rect.center().y);
        let stroke = egui::Stroke::new(1.4, Color32::from_rgb(105, 205, 220));
        ui.painter().line_segment(
            [
                center + egui::vec2(-4.0, 0.0),
                center + egui::vec2(-1.0, 3.0),
            ],
            stroke,
        );
        ui.painter().line_segment(
            [
                center + egui::vec2(-1.0, 3.0),
                center + egui::vec2(4.0, -3.0),
            ],
            stroke,
        );
    }
    response
}

fn agent_session_row(ui: &mut egui::Ui, session: &SessionChoice) -> (bool, bool) {
    let label = session
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("Untitled session");
    let (_, row) = ui.allocate_space(egui::vec2(ui.available_width(), AGENT_SESSION_ROW_HEIGHT));
    let remove = egui::Rect::from_min_max(
        egui::pos2(row.right() - AGENT_SESSION_ROW_HEIGHT, row.top()),
        row.right_bottom(),
    );
    let open = row.with_max_x(remove.left());
    let details = session.updated_at.as_ref().map_or_else(
        || session.id.clone(),
        |updated| format!("{updated}\n{}", session.id),
    );
    let open_response = ui
        .interact(
            open,
            Id::new(("agent_session_open", &session.id)),
            Sense::click(),
        )
        .on_hover_text(details);
    open_response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let remove_label = format!("Remove {label} from history");
    let remove_response = ui
        .interact(
            remove,
            Id::new(("agent_session_remove", &session.id)),
            Sense::click(),
        )
        .on_hover_text(&remove_label);
    remove_response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            remove_label.clone(),
        )
    });
    if open_response.hovered() {
        ui.painter()
            .rect_filled(open, 4.0, Color32::from_white_alpha(10));
    }
    if remove_response.hovered() {
        ui.painter()
            .rect_filled(remove, 4.0, Color32::from_rgb(63, 37, 42));
    }
    let color = Color32::from_rgb(206, 211, 221);
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), FontId::proportional(12.0), color);
    ui.painter().with_clip_rect(open.shrink(7.0)).galley(
        egui::pos2(open.left() + 7.0, open.center().y - galley.size().y * 0.5),
        galley,
        color,
    );
    ui.painter().text(
        remove.center(),
        Align2::CENTER_CENTER,
        "×",
        FontId::proportional(16.0),
        if remove_response.hovered() {
            Color32::from_rgb(236, 145, 150)
        } else {
            Color32::from_rgb(137, 145, 159)
        },
    );
    (open_response.clicked(), remove_response.clicked())
}

fn chevron_icon_button(ui: &mut egui::Ui, upward: bool, label: &str) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 5.0, Color32::from_white_alpha(14));
    }
    let center = rect.center();
    let direction = if upward { -1.0 } else { 1.0 };
    let tip = center + egui::vec2(0.0, 3.0 * direction);
    let stroke = egui::Stroke::new(1.5, Color32::from_rgb(174, 181, 197));
    ui.painter()
        .line_segment([center + egui::vec2(-4.0, -2.0 * direction), tip], stroke);
    ui.painter()
        .line_segment([tip, center + egui::vec2(4.0, -2.0 * direction)], stroke);
    response.on_hover_text(label)
}

fn close_icon_button(ui: &mut egui::Ui) -> egui::Response {
    let label = "Close (Esc)";
    let (rect, response) = ui.allocate_exact_size(egui::vec2(30.0, 26.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 5.0, Color32::from_white_alpha(14));
    }
    let center = rect.center();
    let stroke = egui::Stroke::new(1.5, Color32::from_rgb(174, 181, 197));
    ui.painter().line_segment(
        [
            center + egui::vec2(-3.5, -3.5),
            center + egui::vec2(3.5, 3.5),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            center + egui::vec2(-3.5, 3.5),
            center + egui::vec2(3.5, -3.5),
        ],
        stroke,
    );
    response.on_hover_text(label)
}

fn plain_text_job(text: &str, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: FontId::monospace(14.0),
            color: Color32::LIGHT_GRAY,
            ..TextFormat::default()
        },
    );
    job
}

fn draw_markdown_preview(
    ui: &mut egui::Ui,
    source: &str,
    revision: u64,
    cache: &mut Option<((u64, u32), Arc<egui::Galley>)>,
) {
    let rect = ui.available_rect_before_wrap();
    ui.painter()
        .rect_filled(rect, 0.0, Color32::from_rgb(24, 24, 26));
    let content_width = (rect.width() - 64.0).clamp(1.0, 860.0);
    let side = ((rect.width() - content_width) * 0.5).max(0.0);
    let key = (revision, content_width.round().to_bits());
    if cache.as_ref().is_none_or(|(current, _)| *current != key) {
        let job = markdown::layout(source, content_width);
        *cache = Some((key, ui.fonts_mut(|fonts| fonts.layout_job(job))));
    }
    let galley = Arc::clone(&cache.as_ref().expect("Markdown layout was cached").1);
    ScrollArea::vertical()
        .id_salt("markdown_preview")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.add_space(24.0);
            ui.horizontal(|ui| {
                ui.add_space(side);
                ui.vertical(|ui| {
                    ui.set_width(content_width);
                    ui.add(Label::new(galley).selectable(true).wrap());
                });
            });
            ui.add_space(32.0);
        });
}

fn search_selection_after_navigation(
    selected: usize,
    hit_count: usize,
    down: bool,
    up: bool,
) -> (usize, bool) {
    if hit_count == 0 {
        return (0, false);
    }
    let next = if down {
        (selected + 1).min(hit_count - 1)
    } else if up {
        selected.saturating_sub(1)
    } else {
        selected.min(hit_count - 1)
    };
    (next, next != selected)
}

fn search_needs_polling(query: &str, result_query: &str, complete: bool) -> bool {
    !query.trim().is_empty() && (result_query != query || !complete)
}

fn next_find_match(selected: usize, match_count: usize, backwards: bool) -> usize {
    if match_count == 0 {
        0
    } else if backwards {
        selected.checked_sub(1).unwrap_or(match_count - 1)
    } else {
        (selected + 1) % match_count
    }
}

fn match_bracket_pair(
    buffer: &Buffer,
    cursor_character: usize,
) -> Option<(std::ops::Range<usize>, std::ops::Range<usize>)> {
    let text = &buffer.text;
    let cursor_byte = buffer.byte_index(cursor_character);
    let (byte, bracket) = text[..cursor_byte]
        .char_indices()
        .next_back()
        .filter(|(_, character)| is_bracket(*character))
        .or_else(|| {
            text[cursor_byte..]
                .char_indices()
                .next()
                .map(|(offset, character)| (cursor_byte + offset, character))
                .filter(|(_, character)| is_bracket(*character))
        })?;
    let bracket_range = byte..byte + bracket.len_utf8();

    if is_opening_bracket(bracket) {
        let mut stack = vec![bracket];
        let rest = byte + bracket.len_utf8();
        for (offset, candidate) in text[rest..].char_indices() {
            let candidate_byte = rest + offset;
            if is_opening_bracket(candidate) {
                stack.push(candidate);
            } else if is_closing_bracket(candidate) {
                if matching_bracket(*stack.last()?) != candidate {
                    return None;
                }
                stack.pop();
                if stack.is_empty() {
                    return Some((
                        bracket_range,
                        candidate_byte..candidate_byte + candidate.len_utf8(),
                    ));
                }
            }
        }
    } else {
        let mut stack = vec![bracket];
        for (candidate_byte, candidate) in text[..byte].char_indices().rev() {
            if is_closing_bracket(candidate) {
                stack.push(candidate);
            } else if is_opening_bracket(candidate) {
                if matching_bracket(candidate) != *stack.last()? {
                    return None;
                }
                stack.pop();
                if stack.is_empty() {
                    return Some((
                        candidate_byte..candidate_byte + candidate.len_utf8(),
                        bracket_range,
                    ));
                }
            }
        }
    }
    None
}

const fn is_opening_bracket(character: char) -> bool {
    matches!(character, '(' | '[' | '{')
}

const fn is_closing_bracket(character: char) -> bool {
    matches!(character, ')' | ']' | '}')
}

const fn is_bracket(character: char) -> bool {
    is_opening_bracket(character) || is_closing_bracket(character)
}

const fn matching_bracket(character: char) -> char {
    match character {
        '(' => ')',
        '[' => ']',
        '{' => '}',
        ')' => '(',
        ']' => '[',
        '}' => '{',
        _ => character,
    }
}

fn content_result_job(hit: &SearchHit, query: &str, wrap_width: f32) -> LayoutJob {
    let font_id = FontId::monospace(13.0);
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.append(
        &format!("  {}:{}\n", hit.relative, hit.line.unwrap_or(1)),
        0.0,
        TextFormat {
            font_id: font_id.clone(),
            color: Color32::from_rgb(163, 172, 190),
            ..TextFormat::default()
        },
    );
    let normal = TextFormat {
        font_id: font_id.clone(),
        color: Color32::from_rgb(205, 210, 220),
        ..TextFormat::default()
    };
    let highlighted = TextFormat {
        font_id,
        color: Color32::from_rgb(128, 232, 242),
        background: Color32::from_rgb(30, 79, 89),
        ..TextFormat::default()
    };
    let mut cursor = 0;
    for span in match_spans(&hit.preview, query) {
        job.append(&hit.preview[cursor..span.start], 0.0, normal.clone());
        job.append(&hit.preview[span.clone()], 0.0, highlighted.clone());
        cursor = span.end;
    }
    job.append(&hit.preview[cursor..], 0.0, normal);
    job
}

fn find_highlighted_job(
    base: &LayoutJob,
    matches: &[std::ops::Range<usize>],
    active: usize,
) -> LayoutJob {
    if matches.is_empty() {
        return base.clone();
    }
    let mut highlighted = base.clone();
    highlighted.text.clear();
    highlighted.sections.clear();
    let mut match_index = 0;
    for section in &base.sections {
        let section_start = section.byte_range.start.0;
        let section_end = section.byte_range.end.0;
        while match_index < matches.len() && matches[match_index].end <= section_start {
            match_index += 1;
        }
        let mut current_match = match_index;
        let mut cursor = section_start;
        let mut leading_space = section.leading_space;
        while current_match < matches.len() && matches[current_match].start < section_end {
            let start = matches[current_match].start.max(section_start);
            let end = matches[current_match].end.min(section_end);
            if cursor < start {
                highlighted.append(
                    &base.text[cursor..start],
                    leading_space,
                    section.format.clone(),
                );
                leading_space = 0.0;
            }
            if start < end {
                let mut format = section.format.clone();
                format.background = if current_match == active {
                    Color32::from_rgb(42, 117, 128)
                } else {
                    Color32::from_rgb(30, 79, 89)
                };
                highlighted.append(&base.text[start..end], leading_space, format);
                leading_space = 0.0;
                cursor = end;
            }
            if matches[current_match].end <= section_end {
                current_match += 1;
            } else {
                break;
            }
        }
        if cursor < section_end {
            highlighted.append(
                &base.text[cursor..section_end],
                leading_space,
                section.format.clone(),
            );
        }
        while match_index < matches.len() && matches[match_index].end <= section_end {
            match_index += 1;
        }
    }
    highlighted
}

fn bracket_highlighted_job(
    base: &LayoutJob,
    pair: &(std::ops::Range<usize>, std::ops::Range<usize>),
) -> LayoutJob {
    let spans = [pair.0.clone(), pair.1.clone()];
    let mut highlighted = find_highlighted_job(base, &spans, usize::MAX);
    for section in &mut highlighted.sections {
        let range = section.byte_range.start.0..section.byte_range.end.0;
        if spans
            .iter()
            .any(|span| range.start < span.end && span.start < range.end)
        {
            section.format.background = Color32::from_rgb(50, 57, 72);
            section.format.underline = egui::Stroke::new(1.0, Color32::from_rgb(86, 207, 225));
        }
    }
    highlighted
}

fn presentation_job<'a>(
    base: &'a LayoutJob,
    bracket_overlay: Option<&'a LayoutJob>,
) -> &'a LayoutJob {
    bracket_overlay.unwrap_or(base)
}

fn match_spans(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut text_lower = text.to_owned();
    text_lower.make_ascii_lowercase();
    let mut query_lower = query.to_owned();
    query_lower.make_ascii_lowercase();
    let mut spans = Vec::new();
    let mut cursor = 0;
    while let Some(offset) = text_lower[cursor..].find(&query_lower) {
        let start = cursor + offset;
        let end = start + query_lower.len();
        spans.push(start..end);
        cursor = end;
    }
    spans
}

#[cfg(test)]
mod tests {
    use super::{
        AGENT_COMPOSER_HEIGHT, AGENT_MENU_ROW_HEIGHT, EditorApp, PendingAction, TAB_WIDTH,
        TITLEBAR_HEIGHT, TITLEBAR_PAINT_KEY, TreeState, agent_collapsing_header,
        agent_composer_content, agent_composer_height, agent_diff_preview, agent_markdown_galley,
        agent_menu_rect, agent_near_bottom, agent_new_session_rect, agent_selector_button,
        agent_send_button_colors, agent_sessions_rect, agent_toggle_rect,
        agent_transcript_fade_mesh, build_agent_diff, cached_agent_diff,
        disable_transient_egui_debug_overlays, find_highlighted_job, install_repaint_wake,
        launch_in_current_process, match_bracket_pair, match_spans, model_display_name,
        next_find_match, plain_text_job, presentation_job, queue_resize, repaint_deadline,
        repaint_delay_after_texture_update, run_everything_state, search_needs_polling,
        search_selection_after_navigation, slash_command_query, split_agent_sidebar,
        split_editor_column, split_workspace,
    };
    use crate::{
        agent::controller::{
            CommandChoice, ConfigChoice, ConfigValue, ConfigValueChoice, ConnectionState,
            ModeChoice, PermissionChoice, SessionChoice, ToolActivity, ToolDetail,
        },
        agent::state::{PermissionCard, TranscriptItem},
        buffer::Buffer,
        file_io::OpenTarget,
    };
    use egui::{
        Color32, CursorIcon, Event, Id, Key, Modifiers, MouseWheelUnit, PointerButton, RawInput,
        Rect, TouchPhase, Vec2, epaint::Shape, pos2,
    };
    use std::{
        fs,
        time::{Duration, Instant},
    };

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
        let sessions = agent_sessions_rect(header);

        assert!(header.contains_rect(toggle));
        assert_eq!(toggle.height(), header.height());
        assert!(toggle.center().x > header.center().x);
        assert_eq!(toggle.center().x - new_session.center().x, 33.0);
        assert_eq!(new_session.center().x - sessions.center().x, 33.0);
        assert_eq!(toggle.center().y, new_session.center().y);
        assert_eq!(new_session.center().y, sessions.center().y);
    }

    #[test]
    fn agent_header_is_title_only_and_working_moves_to_the_composer() {
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
        let context = egui::Context::default();
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
        fn text(shape: &Shape, expected: &str) -> Option<(f32, f32)> {
            match shape {
                Shape::Text(text) if text.galley.text() == expected => {
                    Some((text.pos.y, text.galley.job.sections[0].format.font_id.size))
                }
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
        assert_eq!(find(&idle, "Landing Page Builder").unwrap().1, 13.0);
        assert!(find(&idle, "Ready").is_none());
        assert!(find(&idle, "Working").is_none());

        app.agent.active = true;
        let active = draw(&mut app);
        assert!(find(&active, "Working").unwrap().0 >= 592.0);
    }

    #[test]
    fn disabled_send_button_is_neutral_instead_of_blue_with_a_gray_arrow() {
        let (ready_fill, ready_icon) = agent_send_button_colors(true);
        let (disabled_fill, disabled_icon) = agent_send_button_colors(false);

        assert_eq!(ready_fill, Color32::from_rgb(94, 210, 224));
        assert_eq!(ready_icon, Color32::from_rgb(9, 28, 32));
        assert_eq!(disabled_fill, Color32::from_rgb(35, 43, 47));
        assert_eq!(disabled_icon, Color32::from_rgb(92, 140, 149));
    }

    #[test]
    fn composer_selector_hover_brightens_foreground_without_adding_a_surface() {
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
        fn selector_text(shape: &Shape) -> Option<(Color32, f32, f32)> {
            match shape {
                Shape::Text(text) if text.galley.text() == "Ask" => Some((
                    text.galley.job.sections[0].format.color,
                    text.pos.y,
                    text.galley.size().y,
                )),
                Shape::Vec(shapes) => shapes.iter().find_map(selector_text),
                _ => None,
            }
        }
        fn has_hover_surface(shape: &Shape) -> bool {
            match shape {
                Shape::Rect(rect) => rect.fill == Color32::from_white_alpha(10),
                Shape::Vec(shapes) => shapes.iter().any(has_hover_surface),
                _ => false,
            }
        }
        let context = egui::Context::default();
        let idle = draw(&context, None);
        let hovered = draw(&context, Some(pos2(10.0, 15.0)));
        let text = |output: &egui::FullOutput| {
            output
                .shapes
                .iter()
                .find_map(|shape| selector_text(&shape.shape))
                .unwrap()
        };

        assert_eq!(text(&idle.0).0, Color32::from_rgb(166, 175, 190));
        assert_eq!(text(&hovered.0).0, Color32::from_rgb(218, 224, 235));
        assert_eq!(
            text(&idle.0).1 + text(&idle.0).2 * 0.5,
            idle.1.center().y + 4.0
        );
        assert!(
            !hovered
                .0
                .shapes
                .iter()
                .any(|shape| has_hover_surface(&shape.shape))
        );
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
        let context = egui::Context::default();
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
                    rect.rect == button && rect.fill == Color32::from_rgb(36, 36, 40)
                }
                Shape::Vec(shapes) => shapes.iter().any(|shape| has_hover_surface(shape, button)),
                _ => false,
            }
        }
        fn has_bright_icon(shape: &Shape, button: Rect) -> bool {
            match shape {
                Shape::LineSegment { points, stroke } => {
                    stroke.color == Color32::from_rgb(224, 228, 236)
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
        let history_center = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::Circle(circle)
                    if circle.radius == 6.0
                        && circle.stroke.color == Color32::from_rgb(172, 180, 194) =>
                {
                    Some(circle.center)
                }
                _ => None,
            });
        let plus_center = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::LineSegment { points, stroke }
                    if stroke.color == Color32::from_rgb(224, 228, 236)
                        && points[0].y == points[1].y =>
                {
                    Some(Rect::from_points(points).center())
                }
                _ => None,
            });
        let toggle_center = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::Rect(rect)
                    if rect.stroke.color == Color32::from_rgb(112, 215, 228)
                        && rect.rect.size() == Vec2::new(16.0, 13.0) =>
                {
                    Some(rect.rect.center())
                }
                _ => None,
            });
        let history_center = history_center.expect("history icon");
        let plus_center = plus_center.expect("new-session icon");
        let toggle_center = toggle_center.expect("sidebar icon");
        assert!(plus_center.x - history_center.x <= 30.5);
        assert!(toggle_center.x - plus_center.x <= 30.5);
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
        let context = egui::Context::default();
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
        let context = egui::Context::default();
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
        let context = egui::Context::default();
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
        let context = egui::Context::default();
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
    fn editor_column_fills_the_window_without_a_statusbar() {
        let window = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
        let (explorer, editor_column, agent) = split_workspace(window, true, 240.0, true, 340.0);
        let (editor, findbar) = split_editor_column(editor_column, false);

        let explorer = explorer.unwrap();
        assert!(findbar.is_none());
        assert_eq!(explorer.y_range(), window.y_range());
        assert_eq!(agent.y_range(), window.y_range());
        assert_eq!(editor.top(), window.top() + TITLEBAR_HEIGHT);
        assert_eq!(editor.bottom(), window.bottom());
    }

    #[test]
    fn in_file_find_bar_uses_the_bottom_of_the_editor_column_only_while_open() {
        let column = Rect::from_min_size(pos2(240.0, 0.0), Vec2::new(760.0, 700.0));

        let (editor, findbar) = split_editor_column(column, true);
        let findbar = findbar.expect("open find bar");
        assert_eq!(findbar.x_range(), column.x_range());
        assert_eq!(editor.bottom(), findbar.top());
        assert_eq!(findbar.bottom(), column.bottom());

        let (editor, findbar) = split_editor_column(column, false);
        assert!(findbar.is_none());
        assert_eq!(editor.bottom(), column.bottom());
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
    fn agent_transcript_fades_cover_both_scroll_edges() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), Vec2::new(300.0, 400.0));
        let mesh = agent_transcript_fade_mesh(rect);

        assert_eq!(mesh.vertices.len(), 8);
        assert_eq!(mesh.indices.len(), 12);
        assert_eq!(mesh.vertices[0].pos, rect.left_top());
        assert_eq!(mesh.vertices[0].color.a(), 255);
        assert_eq!(mesh.vertices[2].pos.y, rect.top() + 28.0);
        assert_eq!(mesh.vertices[2].color.a(), 0);
        assert_eq!(mesh.vertices[4].pos.y, rect.bottom() - 28.0);
        assert_eq!(mesh.vertices[4].color.a(), 0);
        assert_eq!(mesh.vertices[6].pos, rect.left_bottom());
        assert_eq!(mesh.vertices[6].color.a(), 255);
    }

    #[test]
    fn transcript_following_reengages_within_the_near_bottom_threshold() {
        assert!(agent_near_bottom(952.0, 1_000.0));
        assert!(!agent_near_bottom(951.0, 1_000.0));
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
        let output = egui::Context::default().run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(
                    pos2(0.0, 0.0),
                    Vec2::new(1000.0, 700.0),
                )),
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
        assert!(!output.shapes.iter().any(|shape| has_id_clash(&shape.shape)));
    }

    #[test]
    fn agent_diff_keeps_line_numbers_and_compacts_distant_context() {
        let before = "first\nsecond\nthird\nfourth\nfifth\nsixth\nseventh\neighth\nninth\ntenth\n";
        let after = "first\nsecond changed\nthird\nfourth\nfifth\nsixth\nseventh\neighth\nninth\ntenth\nlast\n";

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
        let context = egui::Context::default();
        let draw = |new_text: &str| {
            let mut cached = None;
            let _ = context.run_ui(RawInput::default(), |ui| {
                let markdown =
                    agent_markdown_galley(ui, Id::new("cached_markdown"), "**fast**", 320.0);
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
    fn agent_diff_is_open_and_readable_without_expanding_the_tool() {
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
                paths: Vec::new(),
                detail: None,
            }));
        let context = egui::Context::default();
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
                old_text: Some(old_text),
                new_text,
            }],
            output: Some("generic completion summary".into()),
        });
        let output = context.run_ui(input(), |root| app.ui(root));
        fn readable_removed_line(shape: &Shape) -> bool {
            match shape {
                Shape::Text(text) if text.galley.text().contains("old 1:") => text
                    .galley
                    .job
                    .sections
                    .iter()
                    .any(|section| section.format.font_id.size == 12.0),
                Shape::Vec(shapes) => shapes.iter().any(readable_removed_line),
                _ => false,
            }
        }
        fn removed_surface(shape: &Shape) -> bool {
            match shape {
                Shape::Rect(rect) => rect.fill == Color32::from_rgb(55, 31, 37),
                Shape::Vec(shapes) => shapes.iter().any(removed_surface),
                _ => false,
            }
        }
        fn diff_surface_right(shape: &Shape) -> Option<f32> {
            match shape {
                Shape::Rect(rect) if rect.fill == Color32::from_rgb(24, 26, 30) => {
                    Some(rect.rect.right())
                }
                Shape::Vec(shapes) => shapes
                    .iter()
                    .filter_map(diff_surface_right)
                    .max_by(f32::total_cmp),
                _ => None,
            }
        }
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
                .any(|shape| readable_removed_line(&shape.shape))
        );
        assert!(
            output
                .shapes
                .iter()
                .any(|shape| removed_surface(&shape.shape))
        );
        let diff_right = output
            .shapes
            .iter()
            .filter_map(|shape| diff_surface_right(&shape.shape))
            .max_by(f32::total_cmp)
            .expect("diff surface");
        assert!(diff_right <= 1_100.0 - 16.0, "diff right: {diff_right}");
        for redundant in [
            "Input",
            "Output",
            "raw edit request",
            "generic completion summary",
        ] {
            assert!(
                !output
                    .shapes
                    .iter()
                    .any(|shape| has_text(&shape.shape, redundant)),
                "structured tool rendered redundant {redundant:?} detail"
            );
        }
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
                paths: Vec::new(),
                detail: None,
            }),
            TranscriptItem::Assistant(response.into()),
        ]);
        let output = egui::Context::default().run_ui(
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
                Shape::Rect(rect) if rect.fill == Color32::from_rgb(25, 26, 29) => Some(rect.rect),
                Shape::Vec(shapes) => shapes.iter().find_map(tool_card_rect),
                _ => None,
            }
        }
        let metrics = |expected| {
            output.shapes.iter().find_map(|shape| {
                text_metrics(&shape.shape, expected)
                    .map(|(rect, rows)| (rect, rows, shape.clip_rect))
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
            card_rect.bottom() - command_rect.bottom()
                <= command_rect.top() - card_rect.top() + 0.5,
            "card={card_rect:?} command={command_rect:?}"
        );
        let (response_rect, response_rows, response_clip) =
            metrics(response).expect("Cursor response");
        assert!(
            command_rect.left() <= response_rect.left() + 48.0,
            "command={command_rect:?} response={response_rect:?}"
        );
        assert!(response_rows > 1);
        assert!(response_rect.right() <= response_clip.right());
    }

    #[test]
    fn tool_card_disclosure_is_optically_centered_and_radius_is_compact() {
        let output = egui::Context::default().run_ui(
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
                    340.0,
                    false,
                    |_| {},
                );
            },
        );
        let mut title_center = None;
        let mut arrow_center = None;
        let mut card_radius = None;
        for clipped in output.shapes {
            match clipped.shape {
                Shape::Text(text) if text.galley.text() == "python3 -c" => {
                    title_center = Some(text.pos.y + text.galley.size().y * 0.5);
                }
                Shape::Path(path)
                    if path.closed
                        && path.points.len() == 3
                        && path.visual_bounding_rect().width() < 20.0 =>
                {
                    arrow_center = Some(path.visual_bounding_rect().center().y);
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

        let arrow_center = arrow_center.unwrap();
        let title_center = title_center.unwrap();
        assert!(
            (arrow_center - title_center - 1.0).abs() < 0.1,
            "arrow={arrow_center}, title={title_center}"
        );
        assert!(card_radius.unwrap() <= 5);
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
        let output = egui::Context::default().run_ui(
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
                        == "Result\n\n• Done\n• Run cargo-test-with-an-unbroken-argument-that-is-much-wider-than-the-agent-sidebar.\n" =>
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

        assert!((18.0..=19.0).contains(&max_font_size));
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
                        && path.fill == Color32::from_rgb(238, 240, 246)
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
        assert_eq!(cursor_color, Color32::from_rgb(238, 240, 246));
        assert!(cursor_size >= 13.0);
        assert_eq!(cursor_facets, 3);
        assert!(cursor_mark.height() >= 14.0);
        assert!(rect.top() - cursor_text.union(cursor_mark).bottom() >= 2.0);
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
        let context = egui::Context::default();
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
                Shape::Rect(rect) if rect.fill == Color32::from_rgb(31, 30, 27) => {
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
        let context = egui::Context::default();
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
                find(&clipped.shape, &prompt)
                    .map(|(y, height)| (y, height, clipped.clip_rect.height()))
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
        let context = egui::Context::default();
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
        let context = egui::Context::default();
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
    fn transcript_pauses_following_for_manual_scroll_and_resticks_near_the_bottom() {
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
        let context = egui::Context::default();
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
                Event::PointerMoved(pos2(820.0, 250.0)),
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
                Event::PointerMoved(pos2(820.0, 250.0)),
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
    fn agent_menu_hugs_its_selector_without_wasted_bottom_space() {
        let sidebar = Rect::from_min_size(pos2(640.0, 34.0), Vec2::new(360.0, 641.0));
        let (_, transcript, composer) = split_agent_sidebar(sidebar, AGENT_COMPOSER_HEIGHT);
        let content = agent_composer_content(composer);
        let selector = Rect::from_min_size(
            pos2(content.left(), content.bottom() - 30.0),
            Vec2::new(76.0, 30.0),
        );
        let menu = agent_menu_rect(transcript, selector, 3, AGENT_MENU_ROW_HEIGHT);

        assert_eq!(content.bottom(), composer.bottom() - 10.0);
        assert_eq!(composer.right() - content.right(), 14.0);
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
    fn composer_shows_permissions_mode_and_model_selectors() {
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
        app.agent.current_mode = Some("agent".into());
        app.agent.modes = vec![ModeChoice {
            id: "agent".into(),
            name: "Agent".into(),
            description: None,
        }];
        app.agent.config_options = vec![ConfigChoice {
            id: "model-id".into(),
            name: "Model".into(),
            description: None,
            value: ConfigValue::Select("auto".into()),
            options: vec![ConfigValueChoice {
                id: "auto".into(),
                name: "Auto".into(),
                description: None,
            }],
        }];
        let context = egui::Context::default();
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
        fn has_text(shape: &Shape, label: &str) -> bool {
            match shape {
                Shape::Text(text) => text.galley.text() == label,
                Shape::Vec(shapes) => shapes.iter().any(|shape| has_text(shape, label)),
                _ => false,
            }
        }

        let output = draw(&mut app);
        for label in ["Ask", "Agent", "Auto"] {
            assert!(
                output
                    .shapes
                    .iter()
                    .any(|shape| has_text(&shape.shape, label)),
                "missing {label} selector"
            );
        }

        app.agent_menu = Some(super::AgentMenu::Permissions);
        assert!(
            draw(&mut app)
                .shapes
                .iter()
                .any(|shape| has_text(&shape.shape, "Allow all"))
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
        app.agent.sessions = Some(vec![SessionChoice {
            id: "session-1".into(),
            title: Some("Previous landing page".into()),
            updated_at: Some("2026-08-07T12:00:00Z".into()),
        }]);
        app.agent_menu = Some(super::AgentMenu::Sessions);
        let output = egui::Context::default().run_ui(
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
        app.agent.sessions = Some(
            (0..24)
                .map(|index| SessionChoice {
                    id: format!("session-{index:02}"),
                    title: Some(format!("Session {index:02}")),
                    updated_at: None,
                })
                .collect(),
        );
        app.agent_menu = Some(super::AgentMenu::Sessions);
        let context = egui::Context::default();
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
        let context = egui::Context::default();
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
                    Shape::Rect(rect) if rect.fill == Color32::from_rgb(26, 27, 31) => {
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
        app.agent.transcript.extend(
            (0..40).map(|index| TranscriptItem::Assistant(format!("transcript-{index:02}"))),
        );
        app.agent_follow_transcript = false;
        app.agent_menu = Some(super::AgentMenu::Config("model-id".into()));
        let context = egui::Context::default();
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
                    Shape::Rect(rect) if rect.fill == Color32::from_rgb(26, 27, 31) => {
                        Some(rect.rect)
                    }
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
                Shape::Rect(rect) => rect.fill == Color32::from_rgb(38, 55, 62),
                Shape::Vec(shapes) => shapes.iter().any(has_selected_surface),
                _ => false,
            }
        }

        let _ = draw(Vec::new(), 0.0);
        let before = draw(vec![Event::PointerMoved(pos2(780.0, 500.0))], 1.0);
        let before_popup = popup_rect(&before).expect("model popup");
        assert_eq!(
            menu_text(&before, before_popup, "Model 00").map(|(_, size)| size),
            Some(12.5)
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
        let context = egui::Context::default();
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
    fn rapid_resizes_keep_only_the_latest_surface_size() {
        let mut pending = None;
        queue_resize(&mut pending, winit::dpi::PhysicalSize::new(800, 600));
        queue_resize(&mut pending, winit::dpi::PhysicalSize::new(1920, 1080));

        assert_eq!(pending, Some(winit::dpi::PhysicalSize::new(1920, 1080)));
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
        let context = egui::Context::default();
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
                        && rect.fill == Color32::from_rgb(24, 24, 26)
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
        let context = egui::Context::default();
        context.set_pixels_per_point(2.0);
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
                Shape::LineSegment { stroke, .. }
                    if stroke.color == Color32::from_rgb(86, 207, 225) =>
                {
                    Some(stroke.width)
                }
                Shape::Vec(shapes) => shapes.iter().find_map(active_divider_width),
                _ => None,
            }
        }
        fn contains_editor_header(shape: &Shape) -> bool {
            match shape {
                Shape::Rect(rect) => {
                    rect.fill == Color32::from_rgb(24, 24, 26)
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
    fn every_titlebar_region_starts_a_window_drag() {
        for start in [pos2(100.0, 17.0), pos2(400.0, 17.0), pos2(600.0, 17.0)] {
            let temp = tempfile::tempdir().unwrap();
            let mut app = EditorApp::new(OpenTarget {
                root: temp.path().canonicalize().unwrap(),
                file: None,
                create: false,
            })
            .unwrap();
            app.agent_sidebar = true;
            let context = egui::Context::default();
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
        let context = egui::Context::default();
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
    fn in_file_search_highlights_every_match_and_distinguishes_the_active_one() {
        let text = "needle then needle";
        let base = egui::text::LayoutJob::simple(
            text.into(),
            egui::FontId::monospace(14.0),
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
        let context = egui::Context::default();
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
        let context = egui::Context::default();
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

        assert!(app.find_open);
        assert!(!app.search_open);
        assert!(!app.sidebar);
        assert!(context.memory(|memory| memory.has_focus(Id::new("file_search_query"))));

        let input = RawInput {
            screen_rect: Some(Rect::from_min_size(
                Default::default(),
                Vec2::new(1000.0, 700.0),
            )),
            events: vec![Event::Text("needle".into())],
            ..RawInput::default()
        };
        let _ = context.run_ui(input, |root| app.ui(root));
        assert_eq!(app.find_query, "needle");
        assert_eq!(app.find_matches.len(), 1);
        assert_eq!(app.find_matches[0], 0..6);

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
        assert!(!app.find_open);
        assert!(context.memory(|memory| memory.has_focus(Id::new("project_search_query"))));
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
        let context = egui::Context::default();
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
    fn active_tab_underline_spans_the_full_tab_width() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let file = root.join("current.rs");
        fs::write(&file, "text\n").unwrap();
        let mut app = EditorApp::new(OpenTarget {
            root,
            file: Some(file),
            create: false,
        })
        .unwrap();
        let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(1000.0, 700.0));
        let context = egui::Context::default();
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..RawInput::default()
            },
            |root| app.ui(root),
        );
        fn cyan_underline(shape: &Shape) -> Option<[egui::Pos2; 2]> {
            match shape {
                Shape::LineSegment { points, stroke }
                    if stroke.width == 2.0 && stroke.color == Color32::from_rgb(86, 207, 225) =>
                {
                    Some(*points)
                }
                Shape::Vec(shapes) => shapes.iter().find_map(cyan_underline),
                _ => None,
            }
        }
        let underline = output
            .shapes
            .iter()
            .find_map(|shape| cyan_underline(&shape.shape))
            .expect("active tab underline");
        let (_, editor, _) = split_workspace(screen, true, 248.0, false, 360.0);

        assert_eq!(
            underline,
            [
                pos2(editor.left(), TITLEBAR_HEIGHT - 1.0),
                pos2(editor.left() + TAB_WIDTH, TITLEBAR_HEIGHT - 1.0),
            ]
        );
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
        let context = egui::Context::default();
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
        let context = egui::Context::default();
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
        let context = egui::Context::default();
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
}
