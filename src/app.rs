use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::{BufRead, Cursor, IsTerminal, Seek},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};

mod agent_diff;
mod agent_text;
mod agent_view;
mod commands;
mod devin_view;
mod language_server;
mod layout;
mod runtime;
mod settings_ui;
mod settings_view;
mod source_control_view;
mod window_state;
mod workspace;
mod workspace_view;

use agent_diff::*;
use agent_text::*;
use devin_view::{
    DevinAdvancedDraft, DevinLifecycle, DevinResourceDraft, DevinScope, DevinView,
    PendingDevinMessage,
};
use layout::*;
use settings_ui::*;
use window_state::*;

use runtime::draw_editor_empty_state;
pub use runtime::{choose_project, launch, quit_running, run, should_choose_project};
#[cfg(test)]
use runtime::{
    editor_watermark_color, launch_in_current_process, project_chooser_ui,
    should_show_project_chooser,
};

use egui::{
    Align, Align2, Color32, CursorIcon, Id, Key, Label, Layout, RichText, ScrollArea, Sense,
    TextEdit, TextFormat, UiBuilder, ViewportId, text::LayoutJob,
};
use winit::{
    application::ApplicationHandler,
    dpi::LogicalSize,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    keyboard::{KeyCode, ModifiersState, PhysicalKey},
    window::{Icon as WindowIcon, Window, WindowId},
};

use crate::{
    agent::{
        controller::{
            AgentController, AuthKind, Command as AgentCommand, ConfigChoice, ConfigValue,
            ConnectionState, ContentRole, DisplayContent, Event as AgentEvent, GoalAction,
            InteractionKind, InteractionResponse, MAX_PROMPT_ATTACHMENT_TOTAL_BYTES,
            MAX_PROMPT_ATTACHMENTS, PromptAttachment, QuestionAnswer, SessionChoice, ToolOutput,
        },
        provider::{
            ProviderDescriptor, ProviderIcon, ProviderId, catalog as provider_catalog,
            descriptor as provider_descriptor,
        },
        state::{AgentState, FileChange, TranscriptItem},
    },
    buffer::{Buffer, LARGE_FILE_BYTES},
    components::{
        chevron_icon_button, chip, chip_width, close_icon_button, icon_button, segment,
        selectable_content_row, selectable_row,
    },
    data_dir,
    devin::{
        ConnectionState as DevinConnectionState, CreateSessionRequest, CredentialSource,
        CrudAction, DevinCommand, DevinController, DevinEvent, DevinSection, DevinState, LoadState,
        RepositoryState, ResourceMutation, SecretInput, SessionFilters, StatusCategory,
    },
    dialog::{Dialog, Outcome, Severity},
    editor_surface::{
        DocumentMetrics, EditorShowOptions, EditorSurface, TextInputMode, editor_background,
    },
    file_io::{
        OpenTarget, ReconcileOutcome, SaveError, child_path, copy_tree_entry, load_buffer,
        reconcile_buffer, resolve_target, reveal_in_file_manager, safe_save, unique_copy_path,
    },
    git::{
        controller::{DiffArea, GitCommand, GitController, GitEvent},
        state::{GitAvailability, GitState},
        status::{BranchInfo, ChangeKind, GitEntry, RepositoryStatus},
    },
    icons::{self, Icon},
    instance::{Claim, InstanceEvent, claim, open_running, spawn_listener},
    keybindings::{
        BUILTIN_VIM, BUILTIN_VSCODE, Behavior as KeybindingBehavior, BindingRule, BindingSource,
        CATALOG as KEYBINDING_CATALOG, Command as KeybindingCommand, InputStroke,
        Platform as KeybindingPlatform, Resolver, Scope, Stroke as BindingStroke,
        rules_have_sequence_conflict,
    },
    lsp::{
        Command as LspCommand, CompletionItem, Controller as LspController, DefinitionLocation,
        PresetId, RequestTag, ServerCapabilities, ServerLaunch, ServerStatus,
        catalog as lsp_catalog, preset_for_path,
    },
    markdown,
    pane::{
        DropZone, PaneId, PaneLayout, TabDrop, paint_pane_resize_handles, resize_divider_stroke,
        resize_dragged_pane_handle, stable_tab_drop_zone, tab_drop_preview,
    },
    renderer::Renderer,
    search::{SearchController, SearchHit, SearchResults},
    settings::{
        self, DensityPreference, LineHeightPreference, LineWrapPreference, ServerMode,
        ServerOverride, Settings, ThemePreference, UI_SCALE_MAX_PERCENT, UI_SCALE_MIN_PERCENT,
        UI_SCALE_STEP_PERCENT,
    },
    syntax::{Highlighter, IncrementalHighlightCache, SyntaxManager},
    terminal::TerminalPanel,
    theme,
    tree::{TreeEntry, read_directory},
    tree_surface::{TreeRow, TreeSurface},
    vim::{
        ExCommand, SearchDirection as VimSearchDirection, VimMode, VimRequest, VimSession,
        VimState, parse_ex,
    },
};

#[derive(Clone)]
enum PendingAction {
    Open(PathBuf),
    OpenTarget(OpenTarget),
    CloseTab(usize),
    Close,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum SidebarPane {
    #[default]
    Files,
    SourceControl,
}

#[derive(Clone)]
struct GitDiscardRequest {
    repository: PathBuf,
    paths: Vec<PathBuf>,
    untracked: usize,
}

#[derive(Clone, Copy)]
enum WindowAction {
    Close,
    Minimize,
    ToggleMaximize,
    Drag,
}

const TITLEBAR_HEIGHT: f32 = theme::chrome::TITLEBAR;
/// A tab is as wide as its own label needs, inside a range that keeps a strip
/// of them scannable: never so narrow that the close control crowds the name,
/// never so wide that one long filename pushes its neighbours off screen.
const TAB_MIN_WIDTH: f32 = 120.0;
const TAB_MAX_WIDTH: f32 = 240.0;
const TAB_CLOSE: f32 = 20.0;
/// The leading slot the unsaved dot occupies, so the label sits in the same
/// place whether the buffer is dirty or clean.
const TAB_DOT: f32 = 12.0;
const FIND_BAR_HEIGHT: f32 = theme::chrome::FIND;

fn key_character(key: Key, modifiers: egui::Modifiers) -> Option<char> {
    if modifiers.ctrl || modifiers.alt || modifiers.command || modifiers.mac_cmd {
        return None;
    }
    let letter = match key {
        Key::A => 'a',
        Key::B => 'b',
        Key::C => 'c',
        Key::D => 'd',
        Key::E => 'e',
        Key::F => 'f',
        Key::G => 'g',
        Key::H => 'h',
        Key::I => 'i',
        Key::J => 'j',
        Key::K => 'k',
        Key::L => 'l',
        Key::M => 'm',
        Key::N => 'n',
        Key::O => 'o',
        Key::P => 'p',
        Key::Q => 'q',
        Key::R => 'r',
        Key::S => 's',
        Key::T => 't',
        Key::U => 'u',
        Key::V => 'v',
        Key::W => 'w',
        Key::X => 'x',
        Key::Y => 'y',
        Key::Z => 'z',
        _ => '\0',
    };
    if letter != '\0' {
        return Some(if modifiers.shift {
            letter.to_ascii_uppercase()
        } else {
            letter
        });
    }
    Some(match (key, modifiers.shift) {
        (Key::Space, _) => ' ',
        (Key::Quote, false) => '\'',
        (Key::Quote, true) => '"',
        (Key::Comma, false) => ',',
        (Key::Comma, true) => '<',
        (Key::Period, false) => '.',
        (Key::Period, true) => '>',
        (Key::Slash, false) => '/',
        (Key::Slash, true) => '?',
        (Key::OpenBracket, false) => '[',
        (Key::OpenBracket, true) => '{',
        (Key::CloseBracket, false) => ']',
        (Key::CloseBracket, true) => '}',
        (Key::Minus, false) => '-',
        (Key::Minus, true) => '_',
        (Key::Equals, false) => '=',
        (Key::Equals, true) => '+',
        (Key::Semicolon, false) => ';',
        (Key::Semicolon, true) => ':',
        (Key::Backslash, false) => '\\',
        (Key::Backslash, true) => '|',
        (Key::Backtick, false) => '`',
        (Key::Backtick, true) => '~',
        (Key::Num0, false) => '0',
        (Key::Num1, false) => '1',
        (Key::Num2, false) => '2',
        (Key::Num3, false) => '3',
        (Key::Num4, false) => '4',
        (Key::Num5, false) => '5',
        (Key::Num6, false) => '6',
        (Key::Num7, false) => '7',
        (Key::Num8, false) => '8',
        (Key::Num9, false) => '9',
        (Key::Num1, true) => '!',
        (Key::Num3, true) => '#',
        (Key::Num4, true) => '$',
        (Key::Num5, true) => '%',
        (Key::Num6, true) => '^',
        (Key::Num7, true) => '&',
        (Key::Num8, true) => '*',
        (Key::Num9, true) => '(',
        (Key::Num0, true) => ')',
        (Key::Colon, _) => ':',
        (Key::Questionmark, _) => '?',
        (Key::OpenCurlyBracket, _) => '{',
        (Key::CloseCurlyBracket, _) => '}',
        (Key::Plus, _) => '+',
        (Key::Pipe, _) => '|',
        (Key::Exclamationmark, _) => '!',
        _ => return None,
    })
}

fn text_scope_owns_printable(scopes: &[Scope], behavior: KeybindingBehavior) -> bool {
    if scopes.iter().any(|scope| {
        matches!(
            scope,
            Scope::VimNormal | Scope::VimVisual | Scope::VimOperator
        )
    }) {
        return false;
    }
    scopes.iter().any(|scope| {
        matches!(
            scope,
            Scope::VimInsert
                | Scope::VimReplace
                | Scope::Find
                | Scope::ProjectSearch
                | Scope::Agent
                | Scope::Devin
                | Scope::Terminal
                | Scope::Settings
        ) || (*scope == Scope::DocumentEditor && behavior == KeybindingBehavior::Standard)
    })
}

fn primary_modifiers() -> egui::Modifiers {
    if KeybindingPlatform::current() == KeybindingPlatform::Macos {
        egui::Modifiers {
            mac_cmd: true,
            command: true,
            ..egui::Modifiers::NONE
        }
    } else {
        egui::Modifiers {
            ctrl: true,
            command: true,
            ..egui::Modifiers::NONE
        }
    }
}
const WINDOW_CORNER_RADIUS: u8 = theme::radius::WINDOW;
const ASSISTANT_HEADER_HEIGHT: f32 = TITLEBAR_HEIGHT;
const AGENT_FIND_HEIGHT: f32 = FIND_BAR_HEIGHT;
const ASSISTANT_COMPOSER_HEIGHT: f32 = 108.0;
const ASSISTANT_COMPOSER_MAX_HEIGHT: f32 = 240.0;
const ASSISTANT_ATTACHMENT_ROW_HEIGHT: f32 = 56.0;
const AGENT_MENU_WIDTH: f32 = 240.0;
const AGENT_PROVIDER_MENU_WIDTH: f32 = 240.0;
const AGENT_MENU_ROW_HEIGHT: f32 = 32.0;
const AGENT_PROVIDER_ROW_HEIGHT: f32 = 44.0;
const AGENT_COMMAND_ROW_HEIGHT: f32 = 40.0;
const AGENT_MENTION_ROW_HEIGHT: f32 = 32.0;
const AGENT_SESSION_ROW_HEIGHT: f32 = 40.0;
const AGENT_TRANSCRIPT_TOP_PADDING: i8 = 14;
const AGENT_DIFF_PREVIEW_ROWS: usize = 18;
const AGENT_DIFF_PREVIEW_HEAD: usize = 12;
const AGENTIC_CONTENT_WIDTH: f32 = 860.0;
const AGENT_PANE_COMPACT_WIDTH: f32 = 480.0;
const AGENTIC_COMPOSER_RADIUS: u8 = 10;
const AGENT_EMPTY_STATE_HEIGHT: f32 = 88.0;
/// Keep the panel flush with the transcript while retaining bottom window
/// clearance. The strip stays taller by the bottom margin so the inner text
/// area matches the docked composer.
const AGENTIC_COMPOSER_TOP_MARGIN: f32 = 0.0;
const AGENTIC_COMPOSER_BOTTOM_MARGIN: f32 = theme::space::LARGE;
const AGENTIC_MODE_TOGGLE_WIDTH: f32 = 48.0;
const AGENTIC_DIFF_MIN_CONVERSATION: f32 = 420.0;
const AGENTIC_DIFF_MIN_PANEL: f32 = 320.0;
const SIDEBAR_MIN_WIDTH: f32 = if cfg!(target_os = "macos") {
    72.0 + 3.0 * 32.0 + AGENTIC_MODE_TOGGLE_WIDTH
} else {
    120.0
};
/// The Settings row pinned under the file tree and the agentic sessions rail.
const SIDEBAR_SETTINGS_ROW_HEIGHT: f32 = 40.0;
/// Diameter of the circular update button on the settings row.
const UPDATE_BUTTON_SIZE: f32 = 22.0;
/// How far past the visible transcript viewport items are still rendered
/// rather than replaced by a spacer of their cached height.
const AGENT_CULL_MARGIN: f32 = 200.0;
/// Above this many highlighted lines per file side, an expanded diff renders
/// as plain diff-inked text: syntect over a wholly rewritten file costs
/// seconds on the frame that expands it.
const AGENT_DIFF_HIGHLIGHT_MAX_LINES: usize = 1_000;
const PANE_TAB_HEIGHT: f32 = theme::chrome::HEADER;
const TERMINAL_DEFAULT_HEIGHT: f32 = 240.0;
const TERMINAL_MIN_HEIGHT: f32 = 120.0;
const WORKSPACE_MIN_HEIGHT: f32 = 160.0;
const TITLEBAR_PAINT_KEY: u64 = 0xa000_0000_0000_0000;
const TAB_DRAG_GHOST_PAINT_KEY: u64 = 0xb000_0000_0000_0000;

fn agent_at_bottom(offset: f32, max_offset: f32) -> bool {
    (max_offset - offset).abs() <= 0.5
}

#[derive(Clone)]
enum AssistantImageSource {
    Bytes(Arc<[u8]>),
    Path(PathBuf),
}

fn draw_agent_content(
    ui: &mut egui::Ui,
    content: &DisplayContent,
    search: Option<(&str, Option<usize>)>,
) -> Option<AssistantImageSource> {
    let mut opened = None;
    match content {
        DisplayContent::Image {
            mime_type,
            uri,
            encoded_bytes,
            data,
        } => {
            if let Some(data) = data
                && assistant_embedded_image_preview(ui, data)
                    .is_some_and(|preview| preview.clicked())
            {
                opened = Some(AssistantImageSource::Bytes(Arc::clone(data)));
            }
            agent_search_label(
                ui,
                &format!("Image · {mime_type} · {encoded_bytes} encoded bytes"),
                theme::typography::body(),
                theme::text().primary,
                search,
            );
            if let Some(uri) = uri {
                agent_search_label(
                    ui,
                    uri,
                    theme::typography::code_small(),
                    theme::text().primary,
                    search,
                );
            }
        }
        DisplayContent::Audio {
            mime_type,
            encoded_bytes,
        } => {
            agent_search_label(
                ui,
                &format!("Audio · {mime_type} · {encoded_bytes} encoded bytes"),
                theme::typography::body(),
                theme::text().primary,
                search,
            );
        }
        DisplayContent::ResourceLink {
            name,
            title,
            uri,
            description,
            mime_type,
            size,
        } => {
            agent_search_label(
                ui,
                title.as_deref().unwrap_or(name),
                theme::typography::body(),
                theme::text().primary,
                search,
            );
            agent_search_label(
                ui,
                uri,
                theme::typography::code_small(),
                theme::text().primary,
                search,
            );
            if let Some(description) = description {
                agent_search_label(
                    ui,
                    description,
                    theme::typography::body(),
                    theme::text().primary,
                    search,
                );
            }
            let metadata = [mime_type.clone(), size.map(|size| format!("{size} bytes"))]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            if !metadata.is_empty() {
                agent_search_label(
                    ui,
                    &metadata,
                    theme::typography::small(),
                    theme::text().muted,
                    search,
                );
            }
        }
        DisplayContent::TextResource {
            uri,
            mime_type,
            text,
        } => {
            agent_search_label(
                ui,
                mime_type.as_deref().unwrap_or("Text resource"),
                theme::typography::body(),
                theme::text().primary,
                search,
            );
            agent_search_label(
                ui,
                uri,
                theme::typography::code_small(),
                theme::text().primary,
                search,
            );
            agent_search_label(
                ui,
                text,
                theme::typography::code_small(),
                theme::text().primary,
                search,
            );
        }
        DisplayContent::BlobResource {
            uri,
            mime_type,
            encoded_bytes,
        } => {
            agent_search_label(
                ui,
                &format!(
                    "Binary resource · {} · {encoded_bytes} encoded bytes",
                    mime_type.as_deref().unwrap_or("unknown type")
                ),
                theme::typography::body(),
                theme::text().primary,
                search,
            );
            agent_search_label(
                ui,
                uri,
                theme::typography::code_small(),
                theme::text().primary,
                search,
            );
        }
    }
    opened
}

fn tool_contains_diff(tool: &crate::agent::controller::ToolActivity) -> bool {
    tool.detail.as_ref().is_some_and(|detail| {
        detail
            .content
            .iter()
            .any(|content| matches!(content, ToolOutput::Diff { .. }))
    })
}

/// A tool card counts as a subagent when the ACP tool call advertised the
/// `Task` kind or when a `cursor/task` notification supplied task content.
fn tool_is_subagent(tool: &crate::agent::controller::ToolActivity) -> bool {
    tool.kind.as_deref() == Some("Task")
        || tool.detail.as_ref().is_some_and(|detail| {
            detail
                .content
                .iter()
                .any(|content| matches!(content, ToolOutput::Task { .. }))
        })
}

fn agent_task_duration(duration_ms: u64) -> String {
    if duration_ms < 1_000 {
        format!("{duration_ms} ms")
    } else {
        format!("{:.1} s", duration_ms as f64 / 1_000.0)
    }
}

/// A clickable path label; every path the agent surfaces routes through this
/// so one click opens the file in the editor.
fn agent_path_link(
    ui: &mut egui::Ui,
    label: &str,
    search: Option<(&str, Option<usize>)>,
) -> egui::Response {
    ui.add(
        Label::new(agent_text_job(
            label,
            ui.available_width(),
            theme::typography::code_small(),
            theme::text().primary,
            search,
        ))
        .truncate()
        .sense(Sense::click()),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

const ASSISTANT_IMAGE_PREVIEW_EDGE: u32 = 640;
const ASSISTANT_IMAGE_LIGHTBOX_EDGE: u32 = 4_096;
const ASSISTANT_IMAGE_PREVIEW_MAX_BYTES: u64 = 16 * 1024 * 1024;
const ASSISTANT_IMAGE_THUMBNAIL_SIZE: egui::Vec2 = egui::vec2(240.0, 180.0);
const ASSISTANT_PROMPT_IMAGE_THUMBNAIL_SIZE: egui::Vec2 = egui::vec2(72.0, 72.0);

#[derive(Clone)]
enum AssistantImagePreview {
    Unavailable,
    Loaded(egui::TextureHandle),
}

fn assistant_image_thumbnail(ui: &mut egui::Ui, texture: &egui::TextureHandle) -> egui::Response {
    let source_size = texture.size_vec2();
    let bounds =
        ASSISTANT_IMAGE_THUMBNAIL_SIZE.min(egui::vec2(ui.available_width(), f32::INFINITY));
    let scale = (bounds.x / source_size.x)
        .min(bounds.y / source_size.y)
        .min(1.0);
    let response = ui
        .add(
            egui::Image::from_texture((texture.id(), source_size))
                .fit_to_exact_size(source_size * scale)
                .corner_radius(6)
                .sense(Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.painter().rect_stroke(
        response.rect,
        6,
        theme::border::hairline(),
        egui::StrokeKind::Inside,
    );
    response
}

fn assistant_image_cover_uv(source_size: egui::Vec2, target_size: egui::Vec2) -> egui::Rect {
    let source_aspect = source_size.x / source_size.y;
    let target_aspect = target_size.x / target_size.y;
    if source_aspect > target_aspect {
        let width = target_aspect / source_aspect;
        egui::Rect::from_min_max(
            egui::pos2((1.0 - width) / 2.0, 0.0),
            egui::pos2((1.0 + width) / 2.0, 1.0),
        )
    } else {
        let height = source_aspect / target_aspect;
        egui::Rect::from_min_max(
            egui::pos2(0.0, (1.0 - height) / 2.0),
            egui::pos2(1.0, (1.0 + height) / 2.0),
        )
    }
}

fn assistant_embedded_image_texture(
    ui: &mut egui::Ui,
    data: &Arc<[u8]>,
) -> Option<egui::TextureHandle> {
    let cache_id = Id::new(("agent_embedded_image", data.as_ptr() as usize, data.len()));
    let cached = ui.data(|state| state.get_temp::<AssistantImagePreview>(cache_id));
    let cached = cached.unwrap_or_else(|| {
        let name = format!(
            "agent_embedded_image_{:x}_{}",
            data.as_ptr() as usize,
            data.len()
        );
        let loaded = load_assistant_image_preview_bytes(ui.ctx(), &name, data).map_or(
            AssistantImagePreview::Unavailable,
            AssistantImagePreview::Loaded,
        );
        ui.data_mut(|state| state.insert_temp(cache_id, loaded.clone()));
        loaded
    });
    match cached {
        AssistantImagePreview::Unavailable => None,
        AssistantImagePreview::Loaded(texture) => Some(texture),
    }
}

fn assistant_embedded_image_preview(ui: &mut egui::Ui, data: &Arc<[u8]>) -> Option<egui::Response> {
    let texture = assistant_embedded_image_texture(ui, data)?;
    Some(assistant_image_thumbnail(ui, &texture))
}

fn assistant_prompt_image_preview(ui: &mut egui::Ui, data: &Arc<[u8]>) -> Option<egui::Response> {
    let texture = assistant_embedded_image_texture(ui, data)?;
    let edge = ASSISTANT_PROMPT_IMAGE_THUMBNAIL_SIZE
        .x
        .min(ui.available_width());
    let size = egui::Vec2::splat(edge);
    let source_size = texture.size_vec2();
    let response = ui
        .add(
            egui::Image::from_texture((texture.id(), source_size))
                .fit_to_exact_size(size)
                .maintain_aspect_ratio(false)
                .uv(assistant_image_cover_uv(source_size, size))
                .corner_radius(8)
                .sense(Sense::click()),
        )
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    ui.painter().rect_stroke(
        response.rect,
        8,
        theme::border::hairline(),
        egui::StrokeKind::Inside,
    );
    Some(response)
}

/// Inline preview for a generated image. The decode happens once on first
/// expand and is cached (including failures) so a frame never re-reads disk.
fn agent_generated_image_preview(ui: &mut egui::Ui, path: &Path) -> Option<egui::Response> {
    let cache_id = Id::new(("agent_generated_image", path));
    let cached = ui.data(|data| data.get_temp::<AssistantImagePreview>(cache_id));
    let cached = cached.unwrap_or_else(|| {
        let loaded = load_assistant_image_preview(ui.ctx(), path).map_or(
            AssistantImagePreview::Unavailable,
            AssistantImagePreview::Loaded,
        );
        ui.data_mut(|data| data.insert_temp(cache_id, loaded.clone()));
        loaded
    });
    match cached {
        AssistantImagePreview::Unavailable => None,
        AssistantImagePreview::Loaded(texture) => Some(assistant_image_thumbnail(ui, &texture)),
    }
}

fn load_assistant_image_preview(ctx: &egui::Context, path: &Path) -> Option<egui::TextureHandle> {
    load_assistant_image_path(ctx, path, ASSISTANT_IMAGE_PREVIEW_EDGE)
}

fn load_assistant_image_path(
    ctx: &egui::Context,
    path: &Path,
    max_edge: u32,
) -> Option<egui::TextureHandle> {
    let metadata = fs::metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > ASSISTANT_IMAGE_PREVIEW_MAX_BYTES {
        return None;
    }
    let reader = image::ImageReader::open(path)
        .ok()?
        .with_guessed_format()
        .ok()?;
    decode_assistant_image_preview(ctx, path.display().to_string(), reader, max_edge)
}

fn load_assistant_image_preview_bytes(
    ctx: &egui::Context,
    name: &str,
    bytes: &[u8],
) -> Option<egui::TextureHandle> {
    load_assistant_image_bytes(ctx, name, bytes, ASSISTANT_IMAGE_PREVIEW_EDGE)
}

fn load_assistant_image_bytes(
    ctx: &egui::Context,
    name: &str,
    bytes: &[u8],
    max_edge: u32,
) -> Option<egui::TextureHandle> {
    if bytes.is_empty() || bytes.len() as u64 > ASSISTANT_IMAGE_PREVIEW_MAX_BYTES {
        return None;
    }
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?;
    decode_assistant_image_preview(ctx, name.to_owned(), reader, max_edge)
}

fn decode_assistant_image_preview<R: BufRead + Seek>(
    ctx: &egui::Context,
    name: String,
    mut reader: image::ImageReader<R>,
    max_edge: u32,
) -> Option<egui::TextureHandle> {
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    let image = if image.width() > max_edge || image.height() > max_edge {
        image.thumbnail(max_edge, max_edge)
    } else {
        image
    };
    let size = [image.width() as usize, image.height() as usize];
    let pixels = image.into_rgba8();
    Some(ctx.load_texture(
        name,
        egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_raw()),
        egui::TextureOptions::LINEAR,
    ))
}

fn agent_tool_status(status: Option<&str>) -> (&'static str, Color32) {
    match status.unwrap_or_default() {
        "Completed" => ("", theme::text().muted),
        "InProgress" | "Pending" => ("Running", theme::accent()),
        "Failed" => ("Failed", theme::ink(theme::semantic().danger)),
        "Cancelled" => ("Cancelled", theme::text().muted),
        _ => ("", theme::text().muted),
    }
}

fn paint_agent_disclosure(ui: &mut egui::Ui, openness: f32, response: &egui::Response) {
    let center = response.rect.center() + egui::vec2(0.0, 1.0);
    let box_rect = egui::Rect::from_center_size(center, egui::Vec2::splat(icons::GRID * 0.75));
    let color = ui.style().interact(response).fg_stroke.color;
    let shapes = icons::shapes(
        if openness > 0.5 {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        box_rect,
        color,
    );
    for shape in shapes {
        ui.painter().add(shape);
    }
}

fn paint_cursor_mark(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    let center = rect.center();
    let scale = rect.width().min(rect.height()) / 15.0;
    let top = center + egui::vec2(0.0, -7.0 * scale);
    let upper_right = center + egui::vec2(6.1 * scale, -3.5 * scale);
    let lower_right = center + egui::vec2(6.1 * scale, 3.5 * scale);
    let bottom = center + egui::vec2(0.0, 7.0 * scale);
    let lower_left = center + egui::vec2(-6.1 * scale, 3.5 * scale);
    let upper_left = center + egui::vec2(-6.1 * scale, -3.5 * scale);
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

fn paint_provider_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    icon: ProviderIcon,
    color: Color32,
) {
    match icon {
        ProviderIcon::CursorMark => paint_cursor_mark(painter, rect, color),
        ProviderIcon::OpenAiMark | ProviderIcon::AnthropicMark => {
            let (cache_key, name, bytes) = match icon {
                ProviderIcon::OpenAiMark => (
                    "provider_openai_texture",
                    "OpenAI logo",
                    &include_bytes!("../assets/icons/provider-openai.png")[..],
                ),
                ProviderIcon::AnthropicMark => (
                    "provider_anthropic_texture",
                    "Anthropic logo",
                    &include_bytes!("../assets/icons/provider-anthropic.png")[..],
                ),
                ProviderIcon::CursorMark => unreachable!(),
            };
            let ctx = painter.ctx();
            let cache_id = Id::new(cache_key);
            let texture = ctx.data_mut(|data| data.get_temp::<egui::TextureHandle>(cache_id));
            let texture = texture.or_else(|| {
                let pixels = image::load_from_memory(bytes).ok()?.into_rgba8();
                let size = [pixels.width() as usize, pixels.height() as usize];
                let texture = ctx.load_texture(
                    name,
                    egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_raw()),
                    egui::TextureOptions::LINEAR,
                );
                ctx.data_mut(|data| data.insert_temp(cache_id, texture.clone()));
                Some(texture)
            });
            if let Some(texture) = texture {
                let side = rect.width().min(rect.height());
                let rect = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
                painter.image(
                    texture.id(),
                    rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    color,
                );
            }
        }
    }
}

fn draw_agent_connecting(
    ui: &mut egui::Ui,
    provider: ProviderId,
    headline: &str,
    detail: &str,
    progress: Option<f32>,
) {
    draw_assistant_connecting(ui, headline, detail, progress, |painter, rect, color| {
        paint_provider_icon(painter, rect, provider_descriptor(provider).icon, color);
    });
}

/// The shared assistant-sidebar loading state: a breathing identity mark,
/// headline, status line, and determinate or sweeping progress bar.
fn draw_assistant_connecting(
    ui: &mut egui::Ui,
    headline: &str,
    detail: &str,
    progress: Option<f32>,
    paint_icon: impl Fn(&egui::Painter, egui::Rect, Color32),
) {
    let region = ui.max_rect();
    let block = egui::Rect::from_center_size(
        region.center(),
        egui::vec2(region.width().min(280.0), 126.0),
    );
    let time = ui.input(|input| input.time);
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("assistant_connecting")
            .max_rect(block),
        |ui| {
            ui.vertical_centered(|ui| {
                let pulse =
                    0.55 + 0.45 * (0.5 + 0.5 * (time * std::f64::consts::TAU / 2.4).sin()) as f32;
                let (mark, _) = ui.allocate_exact_size(egui::vec2(40.0, 44.0), Sense::hover());
                paint_icon(
                    ui.painter(),
                    egui::Rect::from_center_size(mark.center(), egui::Vec2::splat(40.0)),
                    theme::text().primary.gamma_multiply(pulse),
                );
                ui.add_space(theme::space::MEDIUM);
                ui.label(
                    RichText::new(headline)
                        .font(theme::typography::title())
                        .color(theme::text().primary),
                );
                ui.add_space(theme::space::TIGHT);
                ui.label(
                    RichText::new(detail)
                        .size(theme::typography::SMALL_SIZE)
                        .color(theme::text().muted),
                );
                ui.add_space(theme::space::LARGE);
                let (track, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width().min(220.0), 3.0),
                    Sense::hover(),
                );
                ui.painter()
                    .rect_filled(track, 2.0, theme::border::hairline_color());
                let fill = match progress {
                    Some(progress) => {
                        track.with_max_x(track.left() + track.width() * progress.clamp(0.02, 1.0))
                    }
                    None => {
                        let segment = track.width() * 0.35;
                        let travel = track.width() + segment;
                        let offset = ((time * 140.0) % f64::from(travel)) as f32;
                        egui::Rect::from_min_max(
                            egui::pos2(track.left() + offset - segment, track.top()),
                            egui::pos2(track.left() + offset, track.bottom()),
                        )
                        .intersect(track)
                    }
                };
                if fill.width() > 0.0 {
                    ui.painter().rect_filled(fill, 2.0, theme::accent());
                }
            });
        },
    );
    ui.ctx().request_repaint_after(Duration::from_millis(33));
}

fn agent_empty_state_rect(region: egui::Rect, agentic_mode: bool) -> egui::Rect {
    let width = region.width().min(if agentic_mode { 480.0 } else { 360.0 });
    egui::Rect::from_center_size(
        region.center(),
        egui::vec2(width, region.height().min(AGENT_EMPTY_STATE_HEIGHT)),
    )
}

fn draw_agent_empty_state(
    ui: &mut egui::Ui,
    id: Id,
    provider: ProviderId,
    project: &str,
    agentic_mode: bool,
) {
    let block = agent_empty_state_rect(ui.max_rect(), agentic_mode);
    let response = ui.interact(block, id, Sense::hover());
    response
        .widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "Start a task"));
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("agent_empty_state_content")
            .max_rect(block)
            .layout(Layout::top_down(Align::Center)),
        |ui| {
            let (mark, _) = ui.allocate_exact_size(egui::vec2(40.0, 44.0), Sense::hover());
            if agentic_mode {
                icons::paint(
                    ui.painter(),
                    Icon::Sparkle,
                    egui::Rect::from_center_size(
                        mark.center(),
                        egui::Vec2::splat(icons::GRID * 1.5),
                    ),
                    theme::accent(),
                );
            } else {
                paint_provider_icon(
                    ui.painter(),
                    egui::Rect::from_center_size(mark.center(), egui::Vec2::splat(40.0)),
                    provider_descriptor(provider).icon,
                    theme::text().primary,
                );
            }
            ui.add_space(theme::space::MEDIUM);
            ui.label(
                RichText::new(if agentic_mode {
                    format!("What should we work on in {project}?")
                } else {
                    "Start a task".to_owned()
                })
                .font(theme::typography::title())
                .color(theme::text().primary),
            );
        },
    );
}

/// One chat turn's identity line — a small mark, a strong name, and an
/// optional muted timestamp — shared by every assistant transcript so the
/// Agent and Devin panels read as the same product.
fn chat_identity(
    ui: &mut egui::Ui,
    name: &str,
    timestamp: Option<&str>,
    paint_mark: impl FnOnce(&egui::Painter, egui::Rect, Color32),
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        let (rect, _) = ui.allocate_exact_size(egui::vec2(17.0, 20.0), Sense::hover());
        paint_mark(ui.painter(), rect, theme::text().primary);
        ui.label(
            RichText::new(name)
                .size(theme::typography::BODY_SIZE)
                .strong()
                .color(theme::text().primary),
        );
        if let Some(timestamp) = timestamp.filter(|timestamp| !timestamp.is_empty()) {
            ui.label(
                RichText::new(timestamp)
                    .size(theme::typography::MICRO_SIZE)
                    .color(theme::text().muted),
            );
        }
    });
    ui.add_space(5.0);
}

/// The user-message bubble every transcript shares: input fill, strong
/// border, 12 px inset, 8 px corners.
fn chat_user_bubble(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(theme::surface().input)
        .stroke(egui::Stroke::new(1.0, theme::border::strong_color()))
        .inner_margin(egui::Margin::same(12))
        .corner_radius(8)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add_contents(ui);
        });
}

fn chat_user_message(
    ui: &mut egui::Ui,
    text: &str,
    search: Option<(&str, Option<usize>)>,
    add_attachments: impl FnOnce(&mut egui::Ui),
) {
    chat_user_bubble(ui, |ui| {
        if !text.is_empty() {
            let job = agent_text_job(
                text,
                ui.available_width(),
                theme::typography::body(),
                theme::text().primary,
                search,
            );
            ui.add(Label::new(job).wrap());
        }
        add_attachments(ui);
    });
}

fn draw_provider_identity(ui: &mut egui::Ui, provider: ProviderId) {
    let provider = provider_descriptor(provider);
    chat_identity(ui, provider.display_name, None, |painter, rect, color| {
        paint_provider_icon(painter, rect, provider.icon, color);
    });
}

/// A live provider-branded pulse that follows the latest transcript output.
fn draw_agent_working(ui: &mut egui::Ui, id: Id, provider: ProviderId) -> egui::Response {
    let provider = provider_descriptor(provider);
    let time = ui.input(|input| input.time);
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 36.0));
    let response = ui.interact(rect, id, Sense::hover());
    let center = egui::pos2(rect.left() + 14.0, rect.center().y);
    let pulse = (0.5 + 0.5 * (time * std::f64::consts::TAU / 1.8).sin()) as f32;
    ui.painter()
        .circle_filled(center, 11.0, theme::state::selected());
    ui.painter().circle_stroke(
        center,
        12.0 + pulse * 1.5,
        egui::Stroke::new(1.0, theme::accent().gamma_multiply(0.45 + pulse * 0.4)),
    );
    paint_provider_icon(
        ui.painter(),
        egui::Rect::from_center_size(center, egui::Vec2::splat(14.0)),
        provider.icon,
        theme::text().primary,
    );

    let label = format!("{} is working", provider.display_name);
    let galley =
        ui.painter()
            .layout_no_wrap(label, theme::typography::small(), theme::text().secondary);
    let text_pos = egui::pos2(rect.left() + 36.0, rect.center().y - galley.size().y * 0.5);
    let dots_left = text_pos.x + galley.size().x + 10.0;
    ui.painter()
        .galley(text_pos, galley, theme::text().secondary);
    for index in 0..3 {
        let wave = (0.5
            + 0.5 * ((time * 2.4 - f64::from(index) * 0.18) * std::f64::consts::TAU).sin())
            as f32;
        ui.painter().circle_filled(
            egui::pos2(dots_left + index as f32 * 7.0, rect.center().y - wave * 2.0),
            1.6,
            theme::accent().gamma_multiply(0.35 + wave * 0.65),
        );
    }
    ui.ctx().request_repaint_after(Duration::from_millis(33));
    response
}

fn draw_dense_agent_working(ui: &mut egui::Ui, id: Id) -> egui::Response {
    let time = ui.input(|input| input.time);
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 40.0));
    let response = ui.interact(rect, id, Sense::hover());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Label, ui.is_enabled(), "Working")
    });
    ui.painter().text(
        egui::pos2(rect.left() + 4.0, rect.center().y),
        Align2::LEFT_CENTER,
        "Working",
        theme::typography::body(),
        theme::text().secondary,
    );
    for index in 0..3 {
        let wave = (0.5
            + 0.5 * ((time * 2.4 - f64::from(index) * 0.18) * std::f64::consts::TAU).sin())
            as f32;
        ui.painter().circle_filled(
            egui::pos2(
                rect.left() + 65.0 + index as f32 * 7.0,
                rect.center().y - wave * 2.0,
            ),
            2.0,
            theme::accent().gamma_multiply(0.4 + wave * 0.6),
        );
    }
    ui.ctx().request_repaint_after(Duration::from_millis(33));
    response
}

fn draw_provider_selector_identity(
    ui: &mut egui::Ui,
    provider: ProviderId,
    enabled: bool,
    show_label: bool,
) -> egui::Response {
    let provider = provider_descriptor(provider);
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            let text_color = if ui.is_enabled() {
                theme::text().primary
            } else {
                theme::text_disabled()
            };
            let galley = ui.painter().layout_no_wrap(
                provider.display_name.to_owned(),
                theme::typography::body(),
                text_color,
            );
            let width = if show_label {
                41.0 + galley.size().x
            } else {
                39.0
            };
            let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 20.0), Sense::click());
            if response.hovered() || response.is_pointer_button_down_on() {
                ui.painter().rect_filled(
                    rect.expand2(egui::vec2(4.0, 2.0)),
                    5.0,
                    if response.is_pointer_button_down_on() {
                        theme::state::selected()
                    } else {
                        theme::state::hover()
                    },
                );
            }
            let icon = egui::Rect::from_center_size(
                egui::pos2(rect.left() + 7.5, rect.center().y),
                egui::vec2(15.0, 18.0),
            );
            paint_provider_icon(ui.painter(), icon, provider.icon, text_color);
            let text_pos = egui::pos2(rect.left() + 20.0, rect.center().y - galley.size().y * 0.5);
            let chevron_x = if show_label {
                let text_right = text_pos.x + galley.size().x;
                ui.painter().galley(text_pos, galley, text_color);
                text_right + 14.0
            } else {
                rect.left() + 28.0
            };
            icons::paint(
                ui.painter(),
                Icon::ChevronDown,
                egui::Rect::from_center_size(
                    egui::pos2(chevron_x, rect.center().y),
                    egui::Vec2::splat(icons::GRID),
                ),
                text_color,
            );
            response
        })
        .inner;
    let label = format!("Select ACP provider: {}", provider.display_name);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label.clone())
    });
    if show_label {
        response
    } else {
        response.on_hover_text(label)
    }
}

fn agent_session_selector_rect(
    ui: &egui::Ui,
    header: egui::Rect,
    title_x: f32,
    title: &str,
) -> egui::Rect {
    let title_width = ui
        .painter()
        .layout_no_wrap(
            title.to_owned(),
            theme::typography::body(),
            theme::text().primary,
        )
        .size()
        .x;
    let left = title_x - 4.0;
    let right = (left + title_width + 30.0).min(agent_new_session_rect(header).left() - 4.0);
    egui::Rect::from_min_max(
        egui::pos2(left, header.top() + 3.0),
        egui::pos2(right.max(left), header.bottom() - 2.0),
    )
}

fn draw_agent_session_selector(ui: &mut egui::Ui, rect: egui::Rect, title: &str) -> egui::Response {
    let response = ui.interact(rect, Id::new("agent_session_selector"), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Previous sessions: {title}"),
        )
    });
    if response.hovered() || response.is_pointer_button_down_on() {
        ui.painter().rect_filled(
            rect,
            5.0,
            if response.is_pointer_button_down_on() {
                theme::state::selected()
            } else {
                theme::state::hover()
            },
        );
    }
    let color = theme::text().primary;
    let chevron = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 9.0, rect.center().y),
        egui::Vec2::splat(icons::GRID),
    );
    let mut title_job = LayoutJob::single_section(
        title.to_owned(),
        TextFormat {
            font_id: theme::typography::body(),
            color,
            ..Default::default()
        },
    );
    title_job.wrap = egui::text::TextWrapping {
        max_width: (chevron.left() - rect.left() - 8.0).max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let title = ui.fonts_mut(|fonts| fonts.layout_job(title_job));
    ui.painter().galley(
        egui::pos2(rect.left() + 4.0, rect.center().y - title.size().y * 0.5),
        title,
        color,
    );
    icons::paint(ui.painter(), Icon::ChevronDown, chevron, color);
    if response.has_focus() {
        icons::focus_ring(ui.painter(), rect, theme::radius::CONTROL);
    }
    response
}

fn agent_tool_title(
    ui: &mut egui::Ui,
    id: Id,
    title: &str,
    width: f32,
    search: Option<(&str, Option<usize>)>,
) -> egui::Response {
    if search.is_some() {
        return ui
            .allocate_ui_with_layout(
                egui::vec2(width, 24.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_width(width);
                    ui.add(
                        Label::new(agent_text_job(
                            title,
                            width,
                            theme::typography::body(),
                            theme::text().primary,
                            search,
                        ))
                        .truncate()
                        .sense(Sense::click()),
                    )
                },
            )
            .inner;
    }
    let Some((action, path)) = title.split_once(' ').filter(|(action, path)| {
        !path.is_empty()
            && (action.eq_ignore_ascii_case("Read") || action.eq_ignore_ascii_case("Edit"))
    }) else {
        return ui
            .allocate_ui_with_layout(
                egui::vec2(width, 24.0),
                Layout::left_to_right(Align::Center),
                |ui| {
                    ui.set_width(width);
                    ui.add(
                        Label::new(
                            RichText::new(title)
                                .size(theme::typography::BODY_SIZE)
                                .color(theme::text().primary),
                        )
                        .truncate()
                        .sense(Sense::click()),
                    )
                },
            )
            .inner;
    };

    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 24.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), title)
    });
    let font = theme::typography::body();
    let action =
        ui.painter()
            .layout_no_wrap(format!("{action} "), font.clone(), theme::text().primary);
    let path = ui
        .painter()
        .layout_no_wrap(path.to_owned(), font, theme::text().primary);
    let y = rect.center().y - action.size().y * 0.5;
    ui.painter().with_clip_rect(rect).galley(
        egui::pos2(rect.left(), y),
        action.clone(),
        theme::text().primary,
    );
    let path_left = (rect.left() + action.size().x).min(rect.right());
    let path_rect =
        egui::Rect::from_min_max(egui::pos2(path_left, rect.top()), rect.right_bottom());
    let overflow = (path.size().x - path_rect.width()).max(0.0);
    let animation_id = id.with("path_marquee");
    if response.hovered() && overflow > 0.0 {
        let now = ui.input(|input| input.time);
        let started = ui.data_mut(|data| {
            data.get_temp::<f64>(animation_id).unwrap_or_else(|| {
                data.insert_temp(animation_id, now);
                now
            })
        });
        let pause = 0.6;
        let travel = f64::from(overflow) / 24.0;
        let phase = (now - started).rem_euclid(pause + travel + 0.8);
        let offset = if phase < pause {
            0.0
        } else if phase < pause + travel {
            ((phase - pause) * 24.0) as f32
        } else {
            overflow
        };
        ui.painter().with_clip_rect(path_rect).galley(
            egui::pos2(path_rect.left() - offset, y),
            path,
            theme::text().primary,
        );
        ui.ctx().request_repaint_after(Duration::from_millis(16));
    } else {
        ui.data_mut(|data| data.remove::<f64>(animation_id));
        let path = egui::WidgetText::from(
            RichText::new(path.text())
                .font(theme::typography::body())
                .color(theme::text().primary),
        )
        .into_galley(
            ui,
            Some(egui::TextWrapMode::Truncate),
            path_rect.width(),
            egui::FontSelection::Default,
        );
        ui.painter().with_clip_rect(path_rect).galley(
            egui::pos2(path_rect.left(), y),
            path,
            theme::text().primary,
        );
    }
    response
}

// Keeping the header's layout inputs explicit avoids a one-off parameter type.
#[expect(clippy::too_many_arguments)]
fn agent_collapsing_header(
    ui: &mut egui::Ui,
    id_salt: impl egui::AsIdSalt,
    title: &str,
    status: Option<&str>,
    change: Option<FileChange>,
    width: f32,
    search: Option<(&str, Option<usize>)>,
    has_body: bool,
    default_open: bool,
    add_body: impl FnOnce(&mut egui::Ui),
) {
    let id = ui.make_persistent_id(id_salt);
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    if has_body && search.is_some() {
        state.set_open(true);
    }
    let title_line = title
        .lines()
        .next()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .unwrap_or("Tool activity");
    let frame = egui::Frame::new()
        .fill(theme::surface().raised)
        .stroke(egui::Stroke::new(1.0, theme::border::hairline_color()))
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
                let failed = status == Some("Failed");
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    ui.spacing_mut().icon_width = 24.0;
                    if has_body
                        && state
                            .show_toggle_button(ui, paint_agent_disclosure)
                            .clicked()
                    {
                        ui.ctx().request_discard("tool card disclosure changed");
                    }
                    let status_width = if failed {
                        24.0
                    } else if label.is_empty() {
                        0.0
                    } else {
                        52.0
                    };
                    let counts = change.map(|change| {
                        let added = ui.painter().layout_no_wrap(
                            format!("+{}", change.added),
                            theme::typography::code_small(),
                            theme::ink(theme::semantic().success),
                        );
                        let removed = ui.painter().layout_no_wrap(
                            format!("−{}", change.removed),
                            theme::typography::code_small(),
                            theme::ink(theme::semantic().danger),
                        );
                        (added, removed)
                    });
                    let counts_width = counts.as_ref().map_or(0.0, |(added, removed)| {
                        added.size().x + theme::space::SMALL + removed.size().x
                    });
                    let trailing_items =
                        usize::from(counts.is_some()) + usize::from(status_width > 0.0);
                    let title_width = (ui.available_width()
                        - status_width
                        - counts_width
                        - trailing_items as f32 * ui.spacing().item_spacing.x)
                        .max(0.0);
                    let response = agent_tool_title(ui, id, title_line, title_width, search);
                    if has_body && response.clicked() {
                        state.toggle(ui);
                        ui.ctx().request_discard("tool card disclosure changed");
                    }
                    if let Some((added, removed)) = counts {
                        let (rect, _) =
                            ui.allocate_exact_size(egui::vec2(counts_width, 24.0), Sense::hover());
                        let y = rect.center().y - added.size().y * 0.5;
                        ui.painter().galley(
                            egui::pos2(rect.left(), y),
                            added.clone(),
                            theme::ink(theme::semantic().success),
                        );
                        ui.painter().galley(
                            egui::pos2(rect.left() + added.size().x + theme::space::SMALL, y),
                            removed,
                            theme::ink(theme::semantic().danger),
                        );
                    }
                    if failed {
                        let (rect, response) =
                            ui.allocate_exact_size(egui::vec2(24.0, 24.0), Sense::hover());
                        response.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Label,
                                ui.is_enabled(),
                                label,
                            )
                        });
                        icons::paint(
                            ui.painter(),
                            Icon::Error,
                            egui::Rect::from_center_size(
                                rect.center(),
                                egui::Vec2::splat(icons::GRID),
                            ),
                            color,
                        );
                    } else if !label.is_empty() {
                        ui.add_sized(
                            egui::vec2(status_width, 24.0),
                            Label::new(
                                RichText::new(label)
                                    .size(theme::typography::MICRO_SIZE)
                                    .color(color),
                            ),
                        );
                    }
                });
            });
        if has_body {
            state.show_body_unindented(ui, |ui| {
                ui.set_width((card_right - ui.cursor().left()).max(0.0));
                ui.painter().hline(
                    ui.available_rect_before_wrap().x_range(),
                    ui.cursor().top(),
                    egui::Stroke::new(1.0, theme::border::hairline_color()),
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
        }
    });
}

fn assistant_dense_disclosure_row(
    ui: &mut egui::Ui,
    id: Id,
    label: &str,
    change: Option<FileChange>,
    default_open: bool,
    force_open: bool,
) -> bool {
    let mut state = egui::collapsing_header::CollapsingState::load_with_default_open(
        ui.ctx(),
        id,
        default_open,
    );
    if force_open {
        state.set_open(true);
    }
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 40.0));
    let response = ui.interact(rect, id, Sense::click());
    if response.clicked() && !force_open {
        state.toggle(ui);
        ui.ctx().request_discard("dense agent disclosure changed");
    }
    let open = state.is_open();
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::CollapsingHeader,
            ui.is_enabled(),
            open,
            label,
        )
    });
    let label_color = if response.hovered() {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    let counts = change.map(|change| {
        let added = ui.painter().layout_no_wrap(
            format!("+{}", change.added),
            theme::typography::code_small(),
            theme::ink(theme::semantic().success),
        );
        let removed = ui.painter().layout_no_wrap(
            format!("−{}", change.removed),
            theme::typography::code_small(),
            theme::ink(theme::semantic().danger),
        );
        (added, removed)
    });
    let counts_width = counts.as_ref().map_or(0.0, |(added, removed)| {
        added.size().x + theme::space::SMALL + removed.size().x
    });
    let title_left = rect.left() + 4.0;
    let chevron_size = icons::GRID * 0.7;
    let title_gap = if counts_width > 0.0 {
        theme::space::SMALL
    } else {
        0.0
    };
    let trailing_width = title_gap + counts_width + theme::space::TIGHT + chevron_size;
    let title =
        ui.painter()
            .layout_no_wrap(label.to_owned(), theme::typography::strong(), label_color);
    let title_width = title
        .size()
        .x
        .min((rect.right() - title_left - trailing_width).max(0.0));
    ui.painter()
        .with_clip_rect(egui::Rect::from_min_max(
            egui::pos2(title_left, rect.top()),
            egui::pos2(title_left + title_width, rect.bottom()),
        ))
        .galley(
            egui::pos2(title_left, rect.center().y - title.size().y * 0.5),
            title,
            label_color,
        );
    let counts_left = title_left + title_width + title_gap;
    if let Some((added, removed)) = counts {
        let y = rect.center().y - added.size().y * 0.5;
        ui.painter().galley(
            egui::pos2(counts_left, y),
            added.clone(),
            theme::ink(theme::semantic().success),
        );
        ui.painter().galley(
            egui::pos2(counts_left + added.size().x + theme::space::SMALL, y),
            removed,
            theme::ink(theme::semantic().danger),
        );
    }
    let chevron_left = counts_left + counts_width + theme::space::TIGHT;
    icons::paint(
        ui.painter(),
        if open {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        },
        egui::Rect::from_center_size(
            egui::pos2(chevron_left + chevron_size * 0.5, rect.center().y),
            egui::Vec2::splat(chevron_size),
        ),
        label_color,
    );
    state.store(ui.ctx());
    open
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DenseAgentWorkCluster {
    label: String,
    change: Option<FileChange>,
    active: bool,
}

fn dense_agent_work_item(item: &TranscriptItem) -> bool {
    matches!(
        item,
        TranscriptItem::Thought(_) | TranscriptItem::Plan(_) | TranscriptItem::Tool(_)
    ) || matches!(
        item,
        TranscriptItem::Content {
            role: ContentRole::Thought,
            ..
        }
    )
}

fn dense_agent_gap_after_item(
    item_gap: f32,
    is_dense_work: bool,
    dense_work_open: bool,
    next_is_dense_work: bool,
) -> f32 {
    if is_dense_work && dense_work_open && next_is_dense_work {
        0.0
    } else {
        item_gap
    }
}

fn dense_agent_final_response_starts(
    transcript: &std::collections::VecDeque<TranscriptItem>,
    active: bool,
) -> Vec<bool> {
    let mut final_responses = vec![false; transcript.len()];
    let mut last_response_start = None;
    let mut previous_was_assistant = false;
    for (index, item) in transcript.iter().enumerate() {
        let is_user = matches!(item, TranscriptItem::User(_))
            || matches!(
                item,
                TranscriptItem::Content {
                    role: ContentRole::User,
                    ..
                }
            );
        let is_assistant = matches!(item, TranscriptItem::Assistant(_))
            || matches!(
                item,
                TranscriptItem::Content {
                    role: ContentRole::Assistant,
                    ..
                }
            );
        if is_user {
            if let Some(start) = last_response_start.take() {
                final_responses[start] = true;
            }
            previous_was_assistant = false;
        } else if is_assistant {
            if !previous_was_assistant {
                last_response_start = Some(index);
            }
            previous_was_assistant = true;
        } else {
            previous_was_assistant = false;
        }
    }
    if !active && let Some(start) = last_response_start {
        final_responses[start] = true;
    }
    final_responses
}

fn dense_agent_work_clusters(
    transcript: &std::collections::VecDeque<TranscriptItem>,
    tool_changes: &HashMap<String, FileChange>,
    active_work_start: Option<usize>,
) -> Vec<Option<DenseAgentWorkCluster>> {
    let mut clusters = vec![None; transcript.len()];
    let mut index = 0;
    while index < transcript.len() {
        if !dense_agent_work_item(&transcript[index]) {
            index += 1;
            continue;
        }
        let start = index;
        let mut edited_files = HashSet::new();
        let mut edits = 0_usize;
        let mut reads = 0_usize;
        let mut searches = 0_usize;
        let mut commands = 0_usize;
        let mut thoughts = 0_usize;
        let mut plans = 0_usize;
        let mut other_tools = 0_usize;
        let mut change = FileChange::default();
        while index < transcript.len() && dense_agent_work_item(&transcript[index]) {
            match &transcript[index] {
                TranscriptItem::Thought(_)
                | TranscriptItem::Content {
                    role: ContentRole::Thought,
                    ..
                } => thoughts += 1,
                TranscriptItem::Plan(_) => plans += 1,
                TranscriptItem::Tool(tool) => {
                    let title = tool.display_title();
                    let action = title
                        .split_whitespace()
                        .next()
                        .unwrap_or_default()
                        .trim_end_matches(':')
                        .to_ascii_lowercase();
                    let kind = tool
                        .kind
                        .as_deref()
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    if matches!(kind.as_str(), "edit" | "delete" | "move")
                        || matches!(action.as_str(), "edit" | "edited" | "write" | "patch")
                    {
                        edits += 1;
                        let path =
                            tool.paths
                                .first()
                                .map(|path| path.path.as_path())
                                .or_else(|| {
                                    title.split_once(char::is_whitespace).map(|(_, path)| {
                                        Path::new(path.trim_matches(['`', '\'', '"']))
                                    })
                                });
                        if let Some(path) = path.filter(|path| *path != Path::new("file")) {
                            edited_files.insert(
                                path.file_name()
                                    .unwrap_or(path.as_os_str())
                                    .to_string_lossy()
                                    .into_owned(),
                            );
                        }
                    } else if kind == "read" || matches!(action.as_str(), "read" | "opened") {
                        reads += tool.paths.len().max(1);
                    } else if kind == "search"
                        || matches!(
                            action.as_str(),
                            "grep" | "grepped" | "glob" | "find" | "searched"
                        )
                    {
                        searches += 1;
                    } else if kind == "execute"
                        || matches!(
                            action.as_str(),
                            "run" | "ran" | "exec" | "execute" | "shell" | "bash"
                        )
                    {
                        commands += 1;
                    } else {
                        other_tools += 1;
                    }
                    if let Some(tool_change) = tool_changes.get(&tool.id) {
                        change.added = change.added.saturating_add(tool_change.added);
                        change.removed = change.removed.saturating_add(tool_change.removed);
                    }
                }
                _ => {}
            }
            index += 1;
        }

        let mut parts = Vec::new();
        if edited_files.len() == 1 {
            parts.push(format!(
                "edited {}",
                edited_files.into_iter().next().unwrap()
            ));
        } else if !edited_files.is_empty() {
            parts.push(format!("edited {} files", edited_files.len()));
        } else if edits > 0 {
            parts.push(format!(
                "edited {edits} file{}",
                if edits == 1 { "" } else { "s" }
            ));
        }
        if reads > 0 || searches > 0 {
            let mut explored = Vec::new();
            if reads > 0 {
                explored.push(format!("{reads} file{}", if reads == 1 { "" } else { "s" }));
            }
            if searches > 0 {
                explored.push(format!(
                    "{searches} search{}",
                    if searches == 1 { "" } else { "es" }
                ));
            }
            parts.push(format!("explored {}", explored.join(", ")));
        }
        if commands > 0 {
            parts.push(format!(
                "ran {commands} command{}",
                if commands == 1 { "" } else { "s" }
            ));
        }
        if plans > 0 {
            parts.push(if plans == 1 {
                "updated plan".to_owned()
            } else {
                format!("updated {plans} plans")
            });
        }
        if thoughts > 0 && parts.is_empty() {
            parts.push(if thoughts == 1 {
                "thought".to_owned()
            } else {
                format!("thought through {thoughts} steps")
            });
        }
        if other_tools > 0 {
            parts.push(format!(
                "used {other_tools} tool{}",
                if other_tools == 1 { "" } else { "s" }
            ));
        }
        let mut label = if parts.is_empty() {
            "Worked".to_owned()
        } else {
            parts.join(", ")
        };
        label[..1].make_ascii_uppercase();
        clusters[start] = Some(DenseAgentWorkCluster {
            label,
            change: (change != FileChange::default()).then_some(change),
            active: active_work_start.is_some_and(|active_start| start >= active_start),
        });
    }
    clusters
}

#[expect(clippy::too_many_arguments)]
fn assistant_dense_tool(
    ui: &mut egui::Ui,
    id: Id,
    title: &str,
    status: Option<&str>,
    change: Option<FileChange>,
    search: Option<(&str, Option<usize>)>,
    has_body: bool,
    add_body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mut state =
        egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, false);
    if has_body && search.is_some() {
        state.set_open(true);
    }
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 28.0));
    let response = ui.interact(
        rect,
        id,
        if has_body {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    let toggled_open = response.clicked() && !state.is_open();
    if response.clicked() {
        state.toggle(ui);
        ui.ctx().request_discard("dense tool disclosure changed");
    }
    let open = state.is_open();
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            if has_body {
                egui::WidgetType::CollapsingHeader
            } else {
                egui::WidgetType::Label
            },
            ui.is_enabled(),
            open,
            title,
        )
    });
    let title_color = match status {
        Some("Failed") => theme::ink(theme::semantic().danger),
        _ if response.hovered() => theme::text().primary,
        Some("InProgress" | "Pending") => theme::text().secondary,
        _ => theme::text().muted,
    };
    if has_body {
        icons::paint(
            ui.painter(),
            if open {
                Icon::ChevronDown
            } else {
                Icon::ChevronRight
            },
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 10.0, rect.center().y),
                egui::Vec2::splat(icons::GRID * 0.7),
            ),
            title_color,
        );
    }
    let counts = change.map(|change| {
        let added = ui.painter().layout_no_wrap(
            format!("+{}", change.added),
            theme::typography::code_small(),
            theme::ink(theme::semantic().success),
        );
        let removed = ui.painter().layout_no_wrap(
            format!("−{}", change.removed),
            theme::typography::code_small(),
            theme::ink(theme::semantic().danger),
        );
        (added, removed)
    });
    let counts_width = counts.as_ref().map_or(0.0, |(added, removed)| {
        added.size().x + theme::space::SMALL + removed.size().x
    });
    let (status_label, status_color) = agent_tool_status(status);
    let failed = status == Some("Failed");
    let status = (!failed && !status_label.is_empty()).then(|| {
        ui.painter().layout_no_wrap(
            status_label.to_owned(),
            theme::typography::code_small(),
            status_color,
        )
    });
    let status_width = if failed {
        icons::GRID
    } else {
        status.as_ref().map_or(0.0, |status| status.size().x)
    };
    let trailing_gap = if status_width > 0.0 && counts_width > 0.0 {
        theme::space::SMALL
    } else {
        0.0
    };
    let title_left = rect.left() + if has_body { 24.0 } else { 4.0 };
    let title_width = (rect.right()
        - theme::space::SMALL
        - counts_width
        - trailing_gap
        - status_width
        - title_left)
        .max(0.0);
    let mut title = agent_text_job(
        title.lines().next().unwrap_or(title),
        title_width,
        theme::typography::body(),
        title_color,
        search,
    );
    title.wrap.max_rows = 1;
    title.wrap.overflow_character = Some('…');
    let title = ui.painter().layout_job(title);
    ui.painter()
        .with_clip_rect(egui::Rect::from_min_max(
            egui::pos2(title_left, rect.top()),
            egui::pos2(title_left + title_width, rect.bottom()),
        ))
        .galley(
            egui::pos2(title_left, rect.center().y - title.size().y * 0.5),
            title,
            title_color,
        );
    if let Some(status) = status {
        let x = rect.right() - theme::space::SMALL - counts_width - trailing_gap - status_width;
        ui.painter().galley(
            egui::pos2(x, rect.center().y - status.size().y * 0.5),
            status,
            status_color,
        );
    } else if failed {
        let x = rect.right() - theme::space::SMALL - counts_width - trailing_gap - status_width;
        icons::paint(
            ui.painter(),
            Icon::Error,
            egui::Rect::from_center_size(
                egui::pos2(x + status_width * 0.5, rect.center().y),
                egui::Vec2::splat(icons::GRID),
            ),
            status_color,
        );
    }
    if let Some((added, removed)) = counts {
        let x = rect.right() - theme::space::SMALL - counts_width;
        let y = rect.center().y - added.size().y * 0.5;
        ui.painter().galley(
            egui::pos2(x, y),
            added.clone(),
            theme::ink(theme::semantic().success),
        );
        ui.painter().galley(
            egui::pos2(x + added.size().x + theme::space::SMALL, y),
            removed,
            theme::ink(theme::semantic().danger),
        );
    }
    state.store(ui.ctx());
    if has_body && open {
        egui::Frame::new()
            .inner_margin(egui::Margin {
                left: 24,
                right: 4,
                top: 2,
                bottom: 6,
            })
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                add_body(ui);
            });
    }
    toggled_open
}

/// Measured with the active tab's face so a tab does not resize when it is
/// selected, which would shift every label to its right.
fn tab_width(ui: &egui::Ui, label: &str) -> f32 {
    let text = ui
        .painter()
        .layout_no_wrap(
            label.to_owned(),
            theme::typography::strong(),
            theme::text().primary,
        )
        .size()
        .x;
    (theme::space::MEDIUM + TAB_DOT + text + theme::space::SMALL + TAB_CLOSE + theme::space::SMALL)
        .clamp(TAB_MIN_WIDTH, TAB_MAX_WIDTH)
}

fn draw_agentic_diff_tabs(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    tabs: &[AgenticDiff],
    active: usize,
) -> (Option<usize>, Option<usize>) {
    let mut selected = None;
    let mut closed = None;
    ui.scope_builder(
        UiBuilder::new()
            .id_salt("agentic_diff_tabs")
            .max_rect(rect)
            .layout(Layout::left_to_right(Align::Center)),
        |ui| {
            ui.set_clip_rect(rect);
            ScrollArea::horizontal()
                .id_salt("agentic_diff_tabs_scroll")
                .max_width(rect.width())
                .max_height(rect.height())
                .auto_shrink([false, false])
                .content_margin(egui::Margin::ZERO)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                    for (index, panel) in tabs.iter().enumerate() {
                        let label = panel
                            .path
                            .file_name()
                            .unwrap_or(panel.path.as_os_str())
                            .to_string_lossy();
                        let (_, tab) = ui.allocate_space(egui::vec2(
                            tab_width(ui, label.as_ref()),
                            rect.height(),
                        ));
                        let is_active = index == active;
                        let response = ui.interact(
                            tab,
                            Id::new(("agentic_diff_tab", &panel.path)),
                            Sense::click(),
                        );
                        response.widget_info(|| {
                            egui::WidgetInfo::selected(
                                egui::WidgetType::SelectableLabel,
                                ui.is_enabled(),
                                is_active,
                                label.as_ref(),
                            )
                        });
                        if is_active || response.hovered() {
                            ui.painter().rect_filled(
                                tab,
                                0.0,
                                if is_active {
                                    theme::surface().input
                                } else {
                                    theme::state::hover()
                                },
                            );
                        }
                        if !is_active && index + 1 < tabs.len() && active != index + 1 {
                            ui.painter().vline(
                                tab.right() - 0.5,
                                tab.y_range().shrink(theme::space::SNUG),
                                theme::border::hairline(),
                            );
                        }
                        let close_rect = egui::Rect::from_center_size(
                            egui::pos2(tab.right() - theme::space::LARGE, tab.center().y),
                            egui::Vec2::splat(TAB_CLOSE),
                        );
                        let text_rect = egui::Rect::from_min_max(
                            egui::pos2(tab.left() + theme::space::MEDIUM, tab.top()),
                            egui::pos2(close_rect.left() - theme::space::SMALL, tab.bottom()),
                        );
                        let color = if is_active {
                            theme::text().primary
                        } else {
                            theme::text().muted
                        };
                        let galley = egui::WidgetText::from(
                            RichText::new(label.as_ref())
                                .font(if is_active {
                                    theme::typography::strong()
                                } else {
                                    theme::typography::small()
                                })
                                .color(color),
                        )
                        .into_galley(
                            ui,
                            Some(egui::TextWrapMode::Truncate),
                            text_rect.width(),
                            egui::FontSelection::Default,
                        );
                        ui.painter().galley(
                            egui::pos2(
                                text_rect.left(),
                                text_rect.center().y - galley.size().y * 0.5,
                            ),
                            galley,
                            color,
                        );
                        let close = ui
                            .interact(
                                close_rect,
                                Id::new(("agentic_diff_tab_close", &panel.path)),
                                Sense::click(),
                            )
                            .on_hover_text(format!("Close {label}"));
                        close.widget_info(|| {
                            egui::WidgetInfo::labeled(
                                egui::WidgetType::Button,
                                ui.is_enabled(),
                                format!("Close {label}"),
                            )
                        });
                        if is_active || response.hovered() || close.hovered() {
                            icons::paint_button(
                                ui.painter(),
                                Icon::Close,
                                close_rect,
                                &close,
                                ui.is_enabled(),
                                theme::text().secondary,
                            );
                        }
                        if close.clicked() {
                            closed = Some(index);
                        } else if response.clicked() {
                            selected = Some(index);
                        }
                        if is_active {
                            response.scroll_to_me(Some(Align::Center));
                        }
                    }
                });
        },
    );
    (selected, closed)
}

fn drag_label(path: &Path) -> Cow<'_, str> {
    path.file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy()
}

fn draw_dragged_pane_preview(painter: &egui::Painter, rect: egui::Rect, path: Option<&Path>) {
    let painter = painter.with_clip_rect(rect);
    let inset = 2.0_f32.min(rect.width().min(rect.height()).max(0.0) * 0.5);
    let rect = rect.shrink(inset);
    let header = rect.with_max_y((rect.top() + PANE_TAB_HEIGHT).min(rect.bottom()));
    painter.rect_filled(
        rect,
        theme::corner(theme::radius::ROW),
        editor_background().gamma_multiply(0.70),
    );
    painter.rect_filled(
        header,
        theme::corner(theme::radius::ROW),
        theme::surface().chrome.gamma_multiply(0.77),
    );
    painter.rect_stroke(
        rect,
        theme::corner(theme::radius::ROW),
        egui::Stroke::new(theme::stroke::FOCUS, theme::accent().gamma_multiply(0.75)),
        egui::StrokeKind::Inside,
    );
    if let Some(path) = path {
        painter.text(
            egui::pos2(header.left() + theme::space::MEDIUM, header.center().y),
            Align2::LEFT_CENTER,
            drag_label(path),
            theme::typography::small(),
            theme::text().primary.gamma_multiply(0.75),
        );
    }
}

pub(crate) fn draw_tab_drag_ghost(ctx: &egui::Context, label: &str) {
    let Some(pointer) = ctx.pointer_hover_pos() else {
        return;
    };
    let screen = ctx.content_rect();
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        Id::new("tab_drag_ghost"),
    ));
    let galley = painter.layout_no_wrap(
        label.to_owned(),
        theme::typography::small(),
        theme::text().primary,
    );
    let size = egui::vec2(
        (galley.size().x + 28.0)
            .clamp(100.0, 220.0)
            .min(screen.width()),
        30.0_f32.min(screen.height()),
    );
    let offset = egui::vec2(14.0, 14.0);
    let min = egui::pos2(
        (pointer.x + offset.x).clamp(screen.left(), (screen.right() - size.x).max(screen.left())),
        (pointer.y + offset.y).clamp(screen.top(), (screen.bottom() - size.y).max(screen.top())),
    );
    let rect = egui::Rect::from_min_size(min, size);
    crate::renderer::mark_retained(
        &painter,
        rect,
        TAB_DRAG_GHOST_PAINT_KEY,
        u64::from(rect.left().to_bits())
            ^ u64::from(rect.top().to_bits()).rotate_left(16)
            ^ u64::from(rect.width().to_bits()).rotate_left(32)
            ^ u64::from(rect.height().to_bits()).rotate_left(48),
    );
    painter.add(theme::shadow::popover().as_shape(rect, theme::corner(theme::radius::CONTROL)));
    painter.rect_filled(
        rect,
        theme::corner(theme::radius::CONTROL),
        theme::surface().raised.gamma_multiply(0.88),
    );
    painter.rect_stroke(
        rect,
        theme::corner(theme::radius::CONTROL),
        theme::border::strong(),
        egui::StrokeKind::Inside,
    );
    painter.rect_filled(
        egui::Rect::from_min_max(
            rect.left_top(),
            egui::pos2(rect.left() + theme::space::HAIR + 1.0, rect.bottom()),
        ),
        theme::corner(theme::radius::CONTROL),
        theme::accent(),
    );
    let clip_inset = 6.0_f32.min(rect.width().min(rect.height()).max(0.0) * 0.5);
    painter.with_clip_rect(rect.shrink(clip_inset)).galley(
        egui::pos2(rect.left() + 14.0, rect.center().y - galley.size().y * 0.5),
        galley,
        theme::text().primary,
    );
}

fn titlebar_drag_action(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    region: impl Hash + std::fmt::Debug,
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

#[cfg(target_os = "macos")]
fn macos_titlebar_controls(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    id: &'static str,
) -> Option<WindowAction> {
    let pointer = ui.ctx().pointer_hover_pos();
    let button_centers = [17.0, 37.0, 57.0].map(|x| egui::pos2(rect.left() + x, rect.center().y));
    let hovered = button_centers
        .iter()
        .position(|center| pointer.is_some_and(|pointer| pointer.distance(*center) <= 10.0));
    let focused = ui.input(|input| input.focused);
    let actions = [
        (WindowAction::Close, theme::traffic::CLOSE, Icon::Close),
        (
            WindowAction::Minimize,
            theme::traffic::MINIMIZE,
            Icon::Minus,
        ),
        (
            WindowAction::ToggleMaximize,
            theme::traffic::ZOOM,
            Icon::Plus,
        ),
    ];
    let mut selected = None;
    for (index, ((action, color, glyph), center)) in
        actions.into_iter().zip(button_centers).enumerate()
    {
        let button = egui::Rect::from_center_size(center, egui::vec2(18.0, 24.0));
        if ui
            .interact(
                button,
                Id::new((id, "titlebar_button", index)),
                Sense::click(),
            )
            .clicked()
        {
            selected = Some(action);
        }
        let fill = if focused {
            color
        } else {
            theme::traffic::unfocused()
        };
        ui.painter().circle_filled(center, 6.0, fill);
        if hovered == Some(index) {
            icons::paint(
                ui.painter(),
                glyph,
                egui::Rect::from_center_size(center, egui::Vec2::splat(icons::GRID * 0.5)),
                theme::traffic::glyph(),
            );
        }
    }
    selected
}

fn assistant_composer_height(text_height: f32, row_height: f32, sidebar_height: f32) -> f32 {
    let max_height =
        ASSISTANT_COMPOSER_MAX_HEIGHT.min((sidebar_height * 0.45).max(ASSISTANT_COMPOSER_HEIGHT));
    (ASSISTANT_COMPOSER_HEIGHT + (text_height - row_height * 3.0).max(0.0)).min(max_height)
}

fn draw_assistant_sidebar_surface(ui: &mut egui::Ui, rect: egui::Rect) {
    ui.style_mut()
        .text_styles
        .insert(egui::TextStyle::Body, theme::typography::title());
    ui.style_mut()
        .text_styles
        .insert(egui::TextStyle::Small, theme::typography::small());
    ui.style_mut()
        .text_styles
        .insert(egui::TextStyle::Button, theme::typography::body());
    ui.painter()
        .rect_filled(rect, 0.0, theme::state::sidebar_material());
}

fn assistant_sidebar_header(rect: egui::Rect) -> egui::Rect {
    rect.with_max_y((rect.top() + ASSISTANT_HEADER_HEIGHT).min(rect.bottom()))
}

fn paint_assistant_header_divider(painter: &egui::Painter, header: egui::Rect) {
    painter.hline(
        header.x_range(),
        header.bottom() - 0.5,
        egui::Stroke::new(1.0, theme::border::hairline_color()),
    );
}

fn measure_assistant_composer_height(
    ui: &mut egui::Ui,
    prompt: &str,
    prompt_width: f32,
    sidebar_height: f32,
    has_attachments: bool,
) -> f32 {
    let font_id = theme::typography::body();
    let (text_height, row_height) = ui.fonts_mut(|fonts| {
        (
            fonts
                .layout(
                    prompt.to_owned(),
                    font_id.clone(),
                    Color32::WHITE,
                    prompt_width.max(24.0),
                )
                .size()
                .y,
            fonts.row_height(&font_id),
        )
    });
    assistant_composer_height(text_height, row_height, sidebar_height)
        + if has_attachments {
            ASSISTANT_ATTACHMENT_ROW_HEIGHT
        } else {
            0.0
        }
}

fn split_agent_sidebar(
    rect: egui::Rect,
    composer_height: f32,
) -> (egui::Rect, egui::Rect, egui::Rect) {
    let header = assistant_sidebar_header(rect);
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
    let controls_right = header.right() - 3.0;
    #[cfg(not(target_os = "macos"))]
    let controls_right = header.right() - 3.0 * 46.0;
    egui::Rect::from_center_size(
        egui::pos2(controls_right - 14.0, header.center().y),
        egui::vec2(28.0, header.height()),
    )
}

fn agent_pane_button_rect(header: egui::Rect, _window_right: f32) -> egui::Rect {
    #[cfg(target_os = "macos")]
    let controls_right = header.right() - 3.0;
    #[cfg(not(target_os = "macos"))]
    let controls_right = if (header.right() - _window_right).abs() <= 0.5 {
        header.right() - 3.0 * 46.0
    } else {
        header.right() - 3.0
    };
    egui::Rect::from_center_size(
        egui::pos2(controls_right - 14.0, header.center().y),
        egui::vec2(28.0, header.height()),
    )
}

fn agent_pane_close_rect(
    header: egui::Rect,
    window_right: f32,
    has_open_button: bool,
) -> egui::Rect {
    agent_pane_button_rect(header, window_right)
        .translate(egui::vec2(if has_open_button { -28.0 } else { 0.0 }, 0.0))
}

fn agent_pane_drag_rect(
    header: egui::Rect,
    window_right: f32,
    has_open_button: bool,
) -> egui::Rect {
    agent_pane_close_rect(header, window_right, has_open_button).translate(egui::vec2(-28.0, 0.0))
}

fn draw_agent_pane_controls(
    ui: &mut egui::Ui,
    header: egui::Rect,
    pane: PaneId,
    title: &str,
    has_open_button: bool,
) -> (bool, bool) {
    let window_right = ui.ctx().content_rect().right();
    let close_rect = agent_pane_close_rect(header, window_right, has_open_button);
    let close = ui
        .interact(
            close_rect,
            Id::new(("agent_close_pane", pane.0)),
            Sense::click(),
        )
        .on_hover_text("Close pane");
    close.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Close pane")
    });
    let close_color = if close.hovered() {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    icons::paint(
        ui.painter(),
        Icon::Close,
        egui::Rect::from_center_size(close_rect.center(), egui::Vec2::splat(icons::GRID)),
        close_color,
    );

    let drag_rect = agent_pane_drag_rect(header, window_right, has_open_button);
    let drag = ui
        .interact(
            drag_rect,
            Id::new(("agent_pane_drag", pane.0)),
            Sense::click_and_drag(),
        )
        .on_hover_cursor(
            if ui
                .ctx()
                .is_being_dragged(Id::new(("agent_pane_drag", pane.0)))
            {
                CursorIcon::Grabbing
            } else {
                CursorIcon::Grab
            },
        )
        .on_hover_text(format!("Move {title} pane"));
    drag.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Move {title} pane"),
        )
    });
    let drag_color = if drag.hovered() || drag.dragged() {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    for x in [-2.5, 2.5] {
        for y in [-3.5, 0.0, 3.5] {
            ui.painter()
                .circle_filled(drag_rect.center() + egui::vec2(x, y), 1.0, drag_color);
        }
    }
    (close.clicked(), drag.drag_started() || drag.dragged())
}

fn devin_toggle_rect(header: egui::Rect, agent_open: bool) -> egui::Rect {
    let agent = agent_toggle_rect(header);
    if agent_open {
        agent
    } else {
        agent.translate(egui::vec2(-agent.width(), 0.0))
    }
}

fn file_tree_toggle_rect(titlebar: egui::Rect, _editor_header: egui::Rect) -> egui::Rect {
    #[cfg(target_os = "macos")]
    let controls_right = titlebar.left() + 72.0;
    #[cfg(not(target_os = "macos"))]
    let controls_right = _editor_header.left();
    egui::Rect::from_center_size(
        egui::pos2(controls_right + 16.0, titlebar.center().y),
        egui::vec2(32.0, titlebar.height()),
    )
}

fn terminal_toggle_rect(file_tree_button: egui::Rect) -> egui::Rect {
    file_tree_button.translate(egui::vec2(file_tree_button.width(), 0.0))
}

fn source_control_toggle_rect(terminal_button: egui::Rect) -> egui::Rect {
    terminal_button.translate(egui::vec2(terminal_button.width(), 0.0))
}

fn sidebar_settings_rect(sidebar: egui::Rect) -> egui::Rect {
    sidebar.with_min_y((sidebar.bottom() - SIDEBAR_SETTINGS_ROW_HEIGHT).max(sidebar.top()))
}

fn agentic_toggle_rect(preceding_button: egui::Rect, sidebar_right: Option<f32>) -> egui::Rect {
    let right = sidebar_right
        .unwrap_or_default()
        .max(preceding_button.right() + AGENTIC_MODE_TOGGLE_WIDTH);
    egui::Rect::from_center_size(
        egui::pos2(
            right - AGENTIC_MODE_TOGGLE_WIDTH * 0.5,
            preceding_button.center().y,
        ),
        egui::vec2(AGENTIC_MODE_TOGGLE_WIDTH, preceding_button.height()),
    )
}

fn draw_sidebar_toggle_icon(
    ui: &mut egui::Ui,
    button: egui::Rect,
    response: &egui::Response,
    open: bool,
) {
    let icon = egui::Rect::from_center_size(button.center(), egui::vec2(15.0, 12.0));
    let icon_color = if response.hovered() || open {
        theme::text().primary
    } else {
        theme::text().muted
    };
    ui.painter().rect_stroke(
        icon,
        2.0,
        egui::Stroke::new(1.2, icon_color),
        egui::StrokeKind::Inside,
    );
    let divider_x = icon.left() + 4.5;
    ui.painter().vline(
        divider_x,
        icon.y_range(),
        egui::Stroke::new(1.2, icon_color),
    );
    if open {
        let selected =
            egui::Rect::from_min_max(icon.left_top(), egui::pos2(divider_x, icon.bottom()));
        ui.painter()
            .rect_filled(selected, 1.0, theme::state::selected());
    }
}

fn agent_new_session_rect(header: egui::Rect) -> egui::Rect {
    let toggle = agent_toggle_rect(header);
    egui::Rect::from_center_size(
        egui::pos2(toggle.center().x - toggle.width(), header.center().y),
        egui::Vec2::splat(toggle.width()),
    )
}

/// Inset the composer so the prompt and footer share one rhythm: equal sides,
/// a little extra air under the toolbar so the selects don't sit on the edge.
fn assistant_composer_content(composer: egui::Rect) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(
            composer.left() + theme::space::MEDIUM,
            composer.top() + theme::space::MEDIUM,
        ),
        egui::pos2(
            composer.right() - theme::space::MEDIUM,
            composer.bottom() - (theme::space::LARGE - 5.0),
        ),
    )
}

fn agent_menu_rect(
    transcript: egui::Rect,
    anchor: egui::Rect,
    item_count: usize,
    row_height: f32,
    vertical_padding: f32,
) -> egui::Rect {
    let width = AGENT_MENU_WIDTH.min((transcript.width() - 12.0).max(1.0));
    let desired_height = 2.0 * vertical_padding + (item_count as f32 * row_height).min(280.0);
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
    row_height: f32,
    width: f32,
) -> egui::Rect {
    let width = width.min((transcript.width() - 12.0).max(1.0));
    let top = anchor.bottom() + 4.0;
    let height = (16.0 + item_count as f32 * row_height)
        .min(280.0)
        .min((transcript.bottom() - top - 8.0).max(1.0));
    let left = (anchor.right() - width).clamp(
        transcript.left() + 6.0,
        (transcript.right() - width - 6.0).max(transcript.left() + 6.0),
    );
    egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(width, height))
}

fn agent_provider_menu_rect(
    bounds: egui::Rect,
    anchor: egui::Rect,
    item_count: usize,
    row_height: f32,
) -> egui::Rect {
    let width = AGENT_PROVIDER_MENU_WIDTH.min((bounds.width() - 16.0).max(1.0));
    let top = anchor.bottom() + 6.0;
    let height = (16.0 + item_count as f32 * row_height)
        .min(280.0)
        .min((bounds.bottom() - top - 8.0).max(1.0));
    let left = anchor.left().clamp(
        bounds.left() + 8.0,
        (bounds.right() - width - 8.0).max(bounds.left() + 8.0),
    );
    egui::Rect::from_min_size(egui::pos2(left, top), egui::vec2(width, height))
}

fn slash_command_query(prompt: &str) -> Option<&str> {
    prompt
        .strip_prefix('/')
        .filter(|query| !query.chars().any(char::is_whitespace))
}

fn agent_mention_query(prompt: &str) -> Option<&str> {
    if prompt.chars().next_back().is_some_and(char::is_whitespace) {
        return None;
    }
    let token = prompt.split_whitespace().next_back()?;
    let query = token.strip_prefix('@')?;
    (token.len() == query.len() + 1).then_some(query)
}

fn remove_agent_mention(prompt: &mut String) {
    if agent_mention_query(prompt).is_some() {
        let start = prompt.rfind('@').unwrap_or(prompt.len());
        prompt.truncate(start);
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct AgentMentionEntry {
    path: PathBuf,
    relative: String,
    is_dir: bool,
}

fn collect_agent_mentions(root: &Path) -> Vec<AgentMentionEntry> {
    let mut found = Vec::new();
    let mut directories = vec![root.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
        entries.sort_by_key(std::fs::DirEntry::file_name);
        for entry in entries {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_symlink() {
                continue;
            }
            let is_dir = file_type.is_dir();
            if is_dir && crate::search::ignored_directory(&entry.file_name().to_string_lossy()) {
                continue;
            }
            if !is_dir && !file_type.is_file() {
                continue;
            }
            let path = entry.path();
            let Ok(relative) = path.strip_prefix(root) else {
                continue;
            };
            found.push(AgentMentionEntry {
                path: path.clone(),
                relative: relative
                    .components()
                    .map(|component| component.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
                is_dir,
            });
            if is_dir {
                directories.push(path);
            }
        }
    }
    found
}

fn agent_mention_matches(entries: &[AgentMentionEntry], query: &str) -> Vec<AgentMentionEntry> {
    let query = query.to_ascii_lowercase();
    let mut matches = entries
        .iter()
        .filter_map(|entry| {
            let relative = entry.relative.to_ascii_lowercase();
            let name = entry
                .path
                .file_name()
                .unwrap_or(entry.path.as_os_str())
                .to_string_lossy()
                .to_ascii_lowercase();
            let score = if query.is_empty() || name == query {
                0
            } else if name.starts_with(&query) {
                1
            } else if name.contains(&query) {
                2
            } else if relative.contains(&query) {
                3
            } else {
                return None;
            };
            Some((score, entry.relative.len(), entry))
        })
        .collect::<Vec<_>>();
    let by_score = |left: &(usize, usize, &AgentMentionEntry),
                    right: &(usize, usize, &AgentMentionEntry)| {
        (left.0, left.1, left.2.relative.as_str()).cmp(&(
            right.0,
            right.1,
            right.2.relative.as_str(),
        ))
    };
    if matches.len() > 10 {
        matches.select_nth_unstable_by(10, by_score);
        matches.truncate(10);
    }
    matches.sort_by(by_score);
    matches
        .into_iter()
        .map(|(_, _, entry)| entry.clone())
        .collect()
}

fn append_display_search_text(text: &mut String, content: &DisplayContent) {
    use std::fmt::Write as _;
    match content {
        DisplayContent::Image { mime_type, uri, .. } => {
            let _ = writeln!(text, "{mime_type}");
            if let Some(uri) = uri {
                let _ = writeln!(text, "{uri}");
            }
        }
        DisplayContent::Audio { mime_type, .. } => {
            let _ = writeln!(text, "{mime_type}");
        }
        DisplayContent::ResourceLink {
            name,
            title,
            uri,
            description,
            mime_type,
            ..
        } => {
            let _ = writeln!(text, "{name}\n{uri}");
            for value in [title, description, mime_type].into_iter().flatten() {
                let _ = writeln!(text, "{value}");
            }
        }
        DisplayContent::TextResource {
            uri,
            mime_type,
            text: content,
        } => {
            let _ = writeln!(text, "{uri}\n{content}");
            if let Some(mime_type) = mime_type {
                let _ = writeln!(text, "{mime_type}");
            }
        }
        DisplayContent::BlobResource { uri, mime_type, .. } => {
            let _ = writeln!(text, "{uri}");
            if let Some(mime_type) = mime_type {
                let _ = writeln!(text, "{mime_type}");
            }
        }
    }
}

fn agent_searchable_text(item: &TranscriptItem) -> Option<String> {
    use std::fmt::Write as _;
    let mut text = String::new();
    match item {
        TranscriptItem::Thought(_)
        | TranscriptItem::Content {
            role: ContentRole::Thought,
            ..
        } => return None,
        TranscriptItem::User(content)
        | TranscriptItem::Assistant(content)
        | TranscriptItem::Error(content) => text.push_str(content),
        TranscriptItem::Content { content, .. } => append_display_search_text(&mut text, content),
        TranscriptItem::Plan(items) => {
            for item in items {
                let _ = writeln!(text, "{} {}", item.status, item.content);
            }
        }
        TranscriptItem::Tool(tool) => {
            let title = tool.display_title();
            let _ = writeln!(text, "{title}");
            let action = title.split_whitespace().next();
            let title_includes_paths = action.is_some_and(|action| {
                action.eq_ignore_ascii_case("Read") || action.eq_ignore_ascii_case("Edit")
            });
            if !title_includes_paths {
                for path in &tool.paths {
                    let _ = writeln!(text, "{}", path.path.display());
                }
            }
            if let Some(detail) = &tool.detail {
                if detail.content.is_empty()
                    && let Some(value) = detail.output.as_ref().or(detail.input.as_ref())
                {
                    let _ = writeln!(text, "{value}");
                }
                for content in &detail.content {
                    match content {
                        ToolOutput::Text(content) => {
                            let _ = writeln!(text, "{content}");
                        }
                        ToolOutput::Log { label, text: log } => {
                            let _ = writeln!(text, "{label}\n{log}");
                        }
                        ToolOutput::Content(content) => {
                            append_display_search_text(&mut text, content)
                        }
                        ToolOutput::Diff {
                            path,
                            old_text,
                            new_text,
                        } => {
                            let _ = writeln!(text, "{}", path.display());
                            for line in &build_agent_diff(old_text.as_deref(), new_text).lines {
                                let _ = writeln!(text, "{}", line.text);
                            }
                        }
                        ToolOutput::Terminal(id) => {
                            let _ = writeln!(text, "Terminal {id}");
                        }
                        ToolOutput::Todo {
                            id,
                            content,
                            status,
                        } => {
                            let _ = writeln!(text, "{status} {content} {id}");
                        }
                        ToolOutput::Task {
                            description,
                            prompt,
                            subagent_type,
                            model,
                            agent_id,
                            agents,
                            path,
                            activity,
                            duration_ms,
                        } => {
                            let _ = writeln!(text, "{description}\n{prompt}\n{subagent_type}");
                            if let Some(model) = model {
                                let _ = writeln!(text, "{model}");
                            }
                            if let Some(agent_id) = agent_id {
                                let _ = writeln!(text, "{agent_id}");
                            }
                            for agent in agents {
                                let _ = writeln!(
                                    text,
                                    "{} {} {}",
                                    agent.status.as_deref().unwrap_or_default(),
                                    agent.id,
                                    agent.message.as_deref().unwrap_or_default()
                                );
                            }
                            if let Some(path) = path {
                                let _ = writeln!(text, "{path}");
                            }
                            if let Some(activity) = activity {
                                let _ = writeln!(text, "{activity}");
                            }
                            if let Some(duration_ms) = duration_ms {
                                let _ = writeln!(text, "{}", agent_task_duration(*duration_ms));
                            }
                        }
                        ToolOutput::GeneratedImage {
                            description,
                            file_path,
                            reference_image_paths,
                        } => {
                            let _ = writeln!(text, "{description}");
                            if let Some(path) = file_path {
                                let _ = writeln!(text, "{}", path.display());
                            }
                            for path in reference_image_paths {
                                let _ = writeln!(text, "{}", path.display());
                            }
                        }
                    }
                }
            }
        }
        TranscriptItem::Permission(card) => {
            let _ = writeln!(text, "{}", card.action);
            for option in &card.options {
                let _ = writeln!(text, "{} {}", option.name, option.kind);
            }
        }
        TranscriptItem::Interaction(card) => match &card.request.kind {
            InteractionKind::Questions { title, questions } => {
                let _ = writeln!(text, "{title}");
                for question in questions {
                    let _ = writeln!(text, "{}", question.prompt);
                    for option in &question.options {
                        let _ = writeln!(text, "{}", option.label);
                    }
                }
            }
            InteractionKind::Plan(plan) => {
                for value in [&plan.name, &plan.overview].into_iter().flatten() {
                    let _ = writeln!(text, "{value}");
                }
                let _ = writeln!(text, "{}", plan.plan);
                for item in plan
                    .todos
                    .iter()
                    .chain(plan.phases.iter().flat_map(|phase| phase.todos.iter()))
                {
                    let _ = writeln!(text, "{} {}", item.status, item.content);
                }
                for phase in &plan.phases {
                    let _ = writeln!(text, "{}", phase.name);
                }
            }
            InteractionKind::Url { title, url } => {
                let _ = writeln!(text, "{title}\n{url}");
            }
        },
    }
    Some(text)
}

fn agent_search_matches(
    transcript: &std::collections::VecDeque<TranscriptItem>,
    changed_paths: &HashMap<PathBuf, FileChange>,
    query: &str,
) -> Vec<usize> {
    if query.is_empty() {
        return Vec::new();
    }
    let mut matches = Vec::new();
    for (index, item) in transcript.iter().enumerate() {
        let count = agent_searchable_text(item)
            .map(|text| match_spans(&text, query).len())
            .unwrap_or(0);
        matches.extend(std::iter::repeat_n(index, count));
    }
    let changed_index = transcript.len();
    let mut paths = changed_paths.keys().collect::<Vec<_>>();
    paths.sort();
    for path in paths {
        let count = match_spans(&path.display().to_string(), query).len();
        matches.extend(std::iter::repeat_n(changed_index, count));
    }
    matches
}

fn paint_agent_search_item(ui: &mut egui::Ui, top: f32, selected: bool, scroll: bool) -> bool {
    let rect = egui::Rect::from_min_max(
        egui::pos2(ui.min_rect().left(), top),
        egui::pos2(ui.min_rect().right(), ui.cursor().top()),
    );
    if selected && scroll {
        ui.scroll_to_rect(rect, Some(Align::Center));
        true
    } else {
        false
    }
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

fn provider_selector_visible(available: &[crate::agent::provider::ProviderId]) -> bool {
    available.len() >= 2
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum AgentMenu {
    Providers,
    Sessions,
    Commands(String),
    Mentions(String),
    Permissions,
    Mode,
    Config(String),
}

#[derive(Clone, Copy)]
enum TreePromptAction {
    NewFile,
    NewFolder,
    Rename,
}

#[derive(Clone)]
struct TreePrompt {
    action: TreePromptAction,
    directory: PathBuf,
    original: Option<PathBuf>,
    name: String,
    focus: bool,
}

#[derive(Clone)]
struct TreeClipboard {
    path: PathBuf,
    cut: bool,
}

#[derive(Clone, Copy)]
enum TreeContextAction {
    Open,
    NewFile,
    NewFolder,
    Cut,
    Copy,
    Paste,
    Duplicate,
    Rename,
    Delete,
    CopyPath,
    CopyRelativePath,
    Reveal,
    OpenTerminal,
    Refresh,
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

    fn reload(&mut self) -> Result<(), String> {
        let mut children = HashMap::from([(self.root.clone(), read_directory(&self.root)?)]);
        self.expanded.retain(|path| path.is_dir());
        for path in &self.expanded {
            children.insert(path.clone(), read_directory(path)?);
        }
        self.children = children;
        self.refresh_visible();
        Ok(())
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
    appearance: u64,
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
    lsp_key: Option<(u64, u64)>,
    lsp_job: LayoutJob,
}

#[derive(Clone, PartialEq)]
struct GalleyKey {
    appearance: u64,
    revision: u64,
    syntax: String,
    find: Option<(String, usize)>,
    bracket_pair: Option<(std::ops::Range<usize>, std::ops::Range<usize>)>,
}

type MarkdownLayoutCache = Option<((u64, u32, u64), Arc<egui::Galley>)>;

#[derive(Clone)]
struct GitDiffTab {
    repository: PathBuf,
    path: PathBuf,
    area: DiffArea,
    old: Option<String>,
    new: String,
    generation: u64,
    unsaved_editor_changes: bool,
}

struct FileTab {
    buffer: Buffer,
    editor_surface: EditorSurface,
    highlight_cache: HighlightCache,
    pane: PaneId,
    markdown_preview: bool,
    markdown_layout: MarkdownLayoutCache,
    /// When set, the tab renders the agent-session diff (this baseline
    /// against the live buffer) instead of the editor. `None` inside means
    /// the agent created the file, so every line shows as added.
    agent_diff: Option<Option<String>>,
    git_diff: Option<GitDiffTab>,
    vim: VimState,
}

struct PaneFind {
    open: bool,
    query: String,
    focus: bool,
    matches: Vec<std::ops::Range<usize>>,
    match_revision: u64,
    match_query: String,
    selected: usize,
    scroll_to_match: bool,
}

struct AgentFind {
    open: bool,
    query: String,
    focus: bool,
    matches: Vec<usize>,
    selected: usize,
    scroll_to_match: bool,
    dirty: bool,
}

impl Default for AgentFind {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            focus: false,
            matches: Vec::new(),
            selected: 0,
            scroll_to_match: false,
            dirty: true,
        }
    }
}

struct AgentPaneRuntime {
    agent_menu: Option<AgentMenu>,
    agent_menu_popup: Option<egui::Rect>,
    agent_menu_scroll_y: f32,
    agent_follow_transcript: bool,
    agent_prompt_history_index: Option<usize>,
    agent_prompt_history_draft: String,
    agent_attachments: Vec<AssistantComposerAttachment>,
    agent_mentions: Option<Vec<AgentMentionEntry>>,
    agent_mention_matches: Vec<AgentMentionEntry>,
    agent_mention_selected: usize,
    agent_find: AgentFind,
    agent_transcript_heights: Vec<f32>,
    agent_transcript_heights_key: (u32, u64, bool, bool, u64),
    agent_transcript_rendered: usize,
    agent_drop_hovered: bool,
    agent_run_everything: Option<bool>,
    selected_provider: ProviderId,
    provider_agents: HashMap<ProviderId, AgentState>,
    provider_menu_anchor: Option<egui::Rect>,
    agent: AgentState,
    agent_controllers: HashMap<ProviderId, AgentController>,
    pending_agent_prompt: bool,
}

struct AgentPaneDrag {
    pane: PaneId,
    title: String,
}

impl AgentPaneRuntime {
    fn blank(selected_provider: ProviderId) -> Self {
        Self {
            agent_menu: None,
            agent_menu_popup: None,
            agent_menu_scroll_y: 0.0,
            agent_follow_transcript: true,
            agent_prompt_history_index: None,
            agent_prompt_history_draft: String::new(),
            agent_attachments: Vec::new(),
            agent_mentions: None,
            agent_mention_matches: Vec::new(),
            agent_mention_selected: 0,
            agent_find: AgentFind::default(),
            agent_transcript_heights: Vec::new(),
            agent_transcript_heights_key: (0, 0, false, false, 0),
            agent_transcript_rendered: 0,
            agent_drop_hovered: false,
            agent_run_everything: None,
            selected_provider,
            provider_agents: HashMap::new(),
            provider_menu_anchor: None,
            agent: AgentState::default(),
            agent_controllers: HashMap::new(),
            pending_agent_prompt: false,
        }
    }
}

impl Default for PaneFind {
    fn default() -> Self {
        Self {
            open: false,
            query: String::new(),
            focus: false,
            matches: Vec::new(),
            match_revision: u64::MAX,
            match_query: String::new(),
            selected: 0,
            scroll_to_match: false,
        }
    }
}

/// One changed-file tab in the agentic diff panel. Self-contained so it stays
/// renderable across session changes; the current text is re-read from disk
/// while the agent keeps editing the file.
struct AgenticDiff {
    path: PathBuf,
    baseline: Option<String>,
    text: String,
}

struct AssistantComposerAttachment {
    file: PromptAttachment,
    thumbnail: Option<egui::TextureHandle>,
}

#[derive(Clone, Copy, Default, Eq, PartialEq)]
enum AttachmentTarget {
    #[default]
    Agent,
    Devin,
}

enum SettingsAction {
    Back,
    OpenDevin,
    ConnectDevin,
    DisconnectDevin,
    Enabled(bool),
    Mode(PresetId, ServerMode),
    Apply(PresetId, String, String),
    Reset(PresetId),
    Rescan,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SettingsSection {
    Appearance,
    Devin,
    Keybindings,
    LanguageServers,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeybindingFilter {
    All,
    Bound,
    Unbound,
    Modified,
}

struct ShortcutRecorder {
    command: KeybindingCommand,
    strokes: Vec<BindingStroke>,
    logical_keys: Vec<String>,
    physical_keys: Vec<Option<String>>,
    scope: Scope,
    platform: Option<KeybindingPlatform>,
    replace_index: Option<usize>,
    disable_id: Option<String>,
    error: Option<String>,
    can_replace: bool,
}

struct NewProfileDraft {
    name: String,
    base: Option<String>,
    behavior: KeybindingBehavior,
}

enum KeybindingUiAction {
    Activate(String),
    Customize,
    Duplicate,
    ResetAll,
    DeleteToVsCode,
    Rename(String),
    Disable(String),
    Remove(usize),
    ResetCommand(KeybindingCommand),
}

enum VimOverlayKind {
    Search(VimSearchDirection),
    Ex,
}

struct VimOverlay {
    kind: VimOverlayKind,
    input: String,
    error: Option<String>,
    focus: bool,
}

#[derive(Clone, Copy)]
enum ClipboardRequest {
    EditorPaste,
    VimPaste { before: bool },
}

struct LspDiagnosticsState {
    revision: u64,
    stale: bool,
    generation: u64,
    diagnostics: Vec<crate::lsp::Diagnostic>,
    line_markers: HashMap<usize, crate::lsp::DiagnosticSeverity>,
}

struct LspCaret {
    tag: RequestTag,
    rect: egui::Rect,
    bounds: egui::Rect,
}

struct CompletionPopup {
    tag: RequestTag,
    items: Vec<CompletionItem>,
    selected: usize,
    anchor: egui::Rect,
    bounds: egui::Rect,
}

struct DefinitionChooser {
    locations: Vec<DefinitionLocation>,
    selected: usize,
}

enum LspFeatureSend {
    Sent,
    Retry,
    Waiting,
    Unsupported,
}

/// What a picker session is for: multi-selecting files to attach to the
/// agent, or walking to a folder to open as a project. One browser serves
/// both, so no path in the product ever reaches a native dialog.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FilePickerPurpose {
    AttachFiles,
    OpenProject,
}

/// What the picker dialog resolved to on this frame.
enum FilePickerOutcome {
    Dismissed,
    AttachFiles(Vec<PathBuf>),
    OpenDirectory(PathBuf),
}

struct WorkspaceFilePicker {
    purpose: FilePickerPurpose,
    directory: PathBuf,
    entries: Vec<TreeEntry>,
    selected: HashSet<PathBuf>,
    query: String,
    focus_search: bool,
    error: Option<String>,
    /// Dotfiles are noise in almost every browse; they stay hidden until the
    /// footer toggle brings them back.
    show_hidden: bool,
    /// The keyboard highlight in the entry list, an index into
    /// `visible_entries`. `None` means Enter confirms the dialog instead.
    cursor: Option<usize>,
    /// Recently opened project roots for the rail's Recent section; filled by
    /// the open-project flow, empty for attach-files.
    recent: Vec<PathBuf>,
    /// Directories the toolbar's Back button returns to, most recent last.
    history_back: Vec<PathBuf>,
    /// Directories Back stepped out of, so Forward can retrace them.
    history_forward: Vec<PathBuf>,
}

impl WorkspaceFilePicker {
    fn open(directory: PathBuf) -> Result<Self, String> {
        let entries = read_directory(&directory)?;
        Ok(Self::with_entries(directory, entries))
    }

    fn open_directories(directory: PathBuf) -> Result<Self, String> {
        let mut picker = Self::open(directory)?;
        picker.purpose = FilePickerPurpose::OpenProject;
        Ok(picker)
    }

    fn with_entries(directory: PathBuf, entries: Vec<TreeEntry>) -> Self {
        Self {
            purpose: FilePickerPurpose::AttachFiles,
            directory,
            entries,
            selected: HashSet::new(),
            query: String::new(),
            focus_search: true,
            error: None,
            show_hidden: false,
            cursor: None,
            recent: Vec::new(),
            history_back: Vec::new(),
            history_forward: Vec::new(),
        }
    }

    /// Moves into `directory` without touching history; navigation and the
    /// back/forward buttons manage their stacks around this one step.
    fn enter(&mut self, directory: PathBuf) -> Result<(), String> {
        let entries = match read_directory(&directory) {
            Ok(entries) => entries,
            Err(error) => {
                self.error = Some(error.clone());
                return Err(error);
            }
        };
        self.directory = directory;
        self.entries = entries;
        self.query.clear();
        self.focus_search = true;
        self.cursor = None;
        self.error = None;
        Ok(())
    }

    fn navigate(&mut self, directory: PathBuf) -> Result<(), String> {
        if directory == self.directory {
            self.reload();
            return Ok(());
        }
        let previous = self.directory.clone();
        self.enter(directory)?;
        self.history_back.push(previous);
        self.history_forward.clear();
        Ok(())
    }

    fn can_go_back(&self) -> bool {
        !self.history_back.is_empty()
    }

    fn can_go_forward(&self) -> bool {
        !self.history_forward.is_empty()
    }

    fn go_back(&mut self) {
        if let Some(target) = self.history_back.pop() {
            let previous = self.directory.clone();
            if self.enter(target).is_ok() {
                self.history_forward.push(previous);
            }
        }
    }

    fn go_forward(&mut self) {
        if let Some(target) = self.history_forward.pop() {
            let previous = self.directory.clone();
            if self.enter(target).is_ok() {
                self.history_back.push(previous);
            }
        }
    }

    /// Re-reads the current directory in place: the filter, the history, and
    /// the error state all survive a refresh that succeeds.
    fn reload(&mut self) {
        match read_directory(&self.directory) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(error) => self.error = Some(error),
        }
        self.cursor = None;
    }

    fn toggle(&mut self, path: PathBuf) {
        if !self.selected.remove(&path) {
            self.selected.insert(path);
        }
    }

    fn visible_entries(&self) -> Vec<TreeEntry> {
        let query = self.query.trim().to_lowercase();
        self.entries
            .iter()
            .filter(|entry| entry.is_dir || self.purpose == FilePickerPurpose::AttachFiles)
            .filter(|entry| self.show_hidden || !entry.name.to_string_lossy().starts_with('.'))
            .filter(|entry| {
                query.is_empty() || entry.name.to_string_lossy().to_lowercase().contains(&query)
            })
            .cloned()
            .collect()
    }

    /// Entries the hidden filter is currently keeping out of view, so the
    /// empty state can say "hidden" instead of pretending the folder is bare.
    fn hidden_entries(&self) -> usize {
        if self.show_hidden {
            return 0;
        }
        self.entries
            .iter()
            .filter(|entry| entry.is_dir || self.purpose == FilePickerPurpose::AttachFiles)
            .filter(|entry| entry.name.to_string_lossy().starts_with('.'))
            .count()
    }
}

fn stage_composer_files(
    ctx: &egui::Context,
    attachments: &mut Vec<AssistantComposerAttachment>,
    paths: impl IntoIterator<Item = PathBuf>,
    allow_directories: bool,
) -> Option<String> {
    let mut first_error = None;
    for path in paths {
        if attachments.len() >= MAX_PROMPT_ATTACHMENTS {
            return Some(format!("attach at most {MAX_PROMPT_ATTACHMENTS} items"));
        }
        let attachment = match PromptAttachment::from_path(path) {
            Ok(attachment) => attachment,
            Err(error) => {
                first_error.get_or_insert(error);
                continue;
            }
        };
        if attachment.is_directory() && !allow_directories {
            first_error.get_or_insert_with(|| "Only files can be attached here".into());
            continue;
        }
        if attachments
            .iter()
            .any(|attached| attached.file.path() == attachment.path())
        {
            continue;
        }
        let total = attachments
            .iter()
            .map(|attached| attached.file.byte_len())
            .sum::<u64>()
            .saturating_add(attachment.byte_len());
        if total > MAX_PROMPT_ATTACHMENT_TOTAL_BYTES {
            return Some(format!(
                "attached files must total no more than {} MiB",
                MAX_PROMPT_ATTACHMENT_TOTAL_BYTES / 1024 / 1024
            ));
        }
        let thumbnail = load_assistant_thumbnail(ctx, &attachment);
        attachments.push(AssistantComposerAttachment {
            file: attachment,
            thumbnail,
        });
    }
    first_error
}

struct AssistantComposer<'a> {
    panel: egui::Rect,
    prompt_id: Id,
    attach_id: Id,
    scroll_id: Id,
    hint: &'a str,
    attach_tooltip: &'a str,
    drop_hint: &'a str,
    enabled: bool,
    send_enabled: bool,
    active: bool,
    allow_active_send: bool,
    allow_directories: bool,
    handle_drop: bool,
    mouse_wheel: bool,
    focus: bool,
    radius: f32,
}

struct AssistantComposerOutput {
    input_changed: bool,
    submit: bool,
    send: bool,
    cancel: bool,
    open_file_picker: bool,
    input_rect: egui::Rect,
    controls_rect: egui::Rect,
    error: Option<String>,
}

impl AssistantComposer<'_> {
    fn stage_drop(
        ui: &egui::Ui,
        panel: egui::Rect,
        enabled: bool,
        attachments: &mut Vec<AssistantComposerAttachment>,
        drop_hovered: &mut bool,
        allow_directories: bool,
    ) -> Option<String> {
        let (hovered_files, dropped_files, pointer) = ui.input(|input| {
            (
                !input.raw.hovered_files.is_empty(),
                input
                    .raw
                    .dropped_files
                    .iter()
                    .filter_map(|file| file.path.clone())
                    .collect::<Vec<_>>(),
                input.pointer.hover_pos(),
            )
        });
        // External file drags do not always carry a pointer event on macOS.
        let pointer_over_composer = pointer.is_none_or(|pointer| panel.contains(pointer));
        if hovered_files {
            *drop_hovered = enabled && pointer_over_composer;
        }
        if dropped_files.is_empty() {
            if !hovered_files {
                *drop_hovered = false;
            }
            return None;
        }
        let dropped_over_composer = enabled && (pointer_over_composer || *drop_hovered);
        *drop_hovered = false;
        dropped_over_composer
            .then(|| stage_composer_files(ui.ctx(), attachments, dropped_files, allow_directories))
            .flatten()
    }

    fn show(
        self,
        ui: &mut egui::Ui,
        prompt: &mut String,
        attachments: &mut Vec<AssistantComposerAttachment>,
        drop_hovered: &mut bool,
    ) -> AssistantComposerOutput {
        let error = self
            .handle_drop
            .then(|| {
                Self::stage_drop(
                    ui,
                    self.panel,
                    self.enabled,
                    attachments,
                    drop_hovered,
                    self.allow_directories,
                )
            })
            .flatten();
        let content = assistant_composer_content(self.panel);
        let attachment_height = if attachments.is_empty() {
            0.0
        } else {
            ASSISTANT_ATTACHMENT_ROW_HEIGHT
        };
        if attachment_height > 0.0 {
            let attachments_rect = egui::Rect::from_min_max(
                content.left_top(),
                egui::pos2(content.right(), content.top() + attachment_height),
            );
            let mut remove = None;
            ui.scope_builder(
                UiBuilder::new()
                    .id_salt(self.scroll_id.with("attachments"))
                    .max_rect(attachments_rect)
                    .layout(Layout::left_to_right(Align::Center)),
                |ui| {
                    ScrollArea::horizontal()
                        .id_salt(self.scroll_id.with("attachment_scroll"))
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                for (index, attachment) in attachments.iter().enumerate() {
                                    if assistant_attachment_tile(ui, attachment).clicked() {
                                        remove = Some(index);
                                    }
                                }
                            });
                        });
                },
            );
            if let Some(index) = remove {
                attachments.remove(index);
                ui.ctx().request_repaint();
            }
        }
        let footer = egui::Rect::from_min_max(
            egui::pos2(
                content.left() - (theme::control::STANDARD - icons::GRID) * 0.5,
                content.bottom() - theme::control::STANDARD,
            ),
            content.right_bottom(),
        );
        let input_rect = egui::Rect::from_min_max(
            egui::pos2(content.left(), content.top() + attachment_height),
            egui::pos2(content.right(), footer.top() - theme::space::SMALL),
        );
        let mut input_changed = false;
        let mut submit = false;
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(self.scroll_id.with("region"))
                .max_rect(input_rect)
                .layout(Layout::top_down(Align::LEFT)),
            |ui| {
                ScrollArea::vertical()
                    .id_salt(self.scroll_id)
                    .max_height(input_rect.height())
                    .min_scrolled_height(0.0)
                    .auto_shrink([false, false])
                    .scroll_source(egui::scroll_area::ScrollSource {
                        mouse_wheel: self.mouse_wheel,
                        ..Default::default()
                    })
                    .content_margin(0)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        let input = ui.add_enabled(
                            self.enabled,
                            TextEdit::multiline(prompt)
                                .id(self.prompt_id)
                                .font(theme::typography::body())
                                .hint_text(
                                    RichText::new(self.hint)
                                        .size(theme::typography::BODY_SIZE)
                                        .color(theme::text().secondary),
                                )
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .return_key(egui::KeyboardShortcut::new(
                                    egui::Modifiers::SHIFT,
                                    Key::Enter,
                                ))
                                .frame(egui::Frame::NONE),
                        );
                        if self.focus {
                            input.request_focus();
                        }
                        input_changed = input.changed();
                        submit = input.has_focus()
                            && ui.input(|input| {
                                !input.modifiers.shift && input.key_pressed(Key::Enter)
                            });
                    });
            },
        );
        let controls_footer = footer.with_max_x(
            (footer.right() - theme::control::STANDARD - theme::space::SMALL).max(footer.left()),
        );
        let mut open_file_picker = false;
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(self.scroll_id.with("footer"))
                .max_rect(controls_footer.translate(egui::vec2(0.0, theme::space::SMALL)))
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                open_file_picker = ui
                    .add_enabled_ui(self.enabled, |ui| {
                        icons::button_with_id(
                            ui,
                            Some(self.attach_id),
                            Icon::Plus,
                            self.attach_tooltip,
                            theme::text().secondary,
                            egui::Vec2::splat(theme::control::STANDARD),
                        )
                    })
                    .inner
                    .clicked();
            },
        );
        let controls_rect = controls_footer
            .translate(egui::vec2(0.0, theme::space::SMALL))
            .with_min_x(controls_footer.left() + theme::control::STANDARD + theme::space::SMALL);
        let mut send = false;
        let mut cancel = false;
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(self.scroll_id.with("action"))
                .max_rect(footer)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                if self.active {
                    cancel = assistant_composer_action(
                        ui,
                        Icon::Stop,
                        "Stop",
                        theme::state::selected(),
                        theme::text().primary,
                        true,
                    )
                    .clicked();
                    if self.allow_active_send {
                        let (fill, color) = assistant_send_button_colors(self.send_enabled);
                        send = assistant_composer_action(
                            ui,
                            Icon::ArrowUp,
                            "Steer active turn (Enter)",
                            fill,
                            color,
                            self.send_enabled,
                        )
                        .clicked();
                    }
                } else {
                    let (fill, color) = assistant_send_button_colors(self.send_enabled);
                    send = assistant_composer_action(
                        ui,
                        Icon::ArrowUp,
                        "Send (Enter)",
                        fill,
                        color,
                        self.send_enabled,
                    )
                    .clicked();
                }
            },
        );
        if *drop_hovered {
            ui.painter().rect_filled(
                self.panel,
                self.radius,
                theme::surface().raised.gamma_multiply(0.93),
            );
            ui.painter().rect_stroke(
                self.panel.shrink(1.0),
                self.radius,
                egui::Stroke::new(1.5, theme::accent()),
                egui::StrokeKind::Inside,
            );
            ui.painter().text(
                self.panel.center(),
                Align2::CENTER_CENTER,
                self.drop_hint,
                theme::typography::body(),
                theme::text().primary,
            );
        }
        AssistantComposerOutput {
            input_changed,
            submit,
            send,
            cancel,
            open_file_picker,
            input_rect,
            controls_rect,
            error,
        }
    }
}

fn load_assistant_thumbnail(
    ctx: &egui::Context,
    attachment: &PromptAttachment,
) -> Option<egui::TextureHandle> {
    if !attachment.is_image() {
        return None;
    }
    let mut reader = image::ImageReader::open(attachment.path())
        .ok()?
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(16_384);
    limits.max_image_height = Some(16_384);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let pixels = reader
        .decode()
        .ok()?
        .resize_to_fill(96, 96, image::imageops::FilterType::Triangle)
        .into_rgba8();
    Some(ctx.load_texture(
        attachment.path().display().to_string(),
        egui::ColorImage::from_rgba_unmultiplied([96, 96], pixels.as_raw()),
        egui::TextureOptions::LINEAR,
    ))
}

fn assistant_attachment_tile(
    ui: &mut egui::Ui,
    attachment: &AssistantComposerAttachment,
) -> egui::Response {
    let size = egui::vec2(48.0, 48.0);
    let response = if let Some(thumbnail) = &attachment.thumbnail {
        ui.add(
            egui::Image::from_texture((thumbnail.id(), size))
                .fit_to_exact_size(size)
                .corner_radius(7)
                .sense(Sense::click()),
        )
    } else {
        let (rect, response) = ui.allocate_exact_size(size, Sense::click());
        ui.painter()
            .rect_filled(rect, 7.0, theme::state::selected());
        if attachment.file.is_directory() {
            icons::paint(
                ui.painter(),
                Icon::Folder,
                egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(icons::GRID * 1.25)),
                theme::text().secondary,
            );
        } else {
            let extension = attachment
                .file
                .path()
                .extension()
                .and_then(|extension| extension.to_str())
                .filter(|extension| !extension.is_empty())
                .map_or_else(|| "FILE".into(), |extension| extension.to_uppercase());
            ui.painter().text(
                rect.center() + egui::vec2(0.0, 1.0),
                Align2::CENTER_CENTER,
                extension.chars().take(5).collect::<String>(),
                theme::typography::micro(),
                theme::text().secondary,
            );
        }
        response
    };
    ui.painter().rect_stroke(
        response.rect,
        7.0,
        egui::Stroke::new(1.0, theme::state::selected()),
        egui::StrokeKind::Inside,
    );
    let close = response.rect.right_top() + egui::vec2(-8.0, 8.0);
    ui.painter()
        .circle_filled(close, 6.5, theme::surface().sunken.gamma_multiply(0.72));
    icons::paint(
        ui.painter(),
        Icon::Close,
        egui::Rect::from_center_size(close, egui::Vec2::splat(icons::GRID * 0.55)),
        theme::text().primary,
    );
    response
}

fn workspace_file_picker_row(
    ui: &mut egui::Ui,
    entry: &TreeEntry,
    selected: bool,
    highlighted: bool,
) -> egui::Response {
    let (id, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 30.0));
    let response = ui.interact(rect, id.with(&entry.path), Sense::click());
    let fill = if selected {
        theme::state::selected()
    } else if highlighted {
        theme::state::selected_focus()
    } else if response.hovered() {
        theme::state::hover()
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(rect, theme::corner(theme::radius::CONTROL), fill);
    }
    icons::paint(
        ui.painter(),
        if entry.is_dir {
            Icon::Folder
        } else {
            Icon::File
        },
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 20.0, rect.center().y),
            egui::Vec2::splat(icons::GRID * 0.95),
        ),
        if entry.is_dir {
            theme::text().secondary
        } else {
            theme::text().muted
        },
    );
    let mut name_right = rect.right() - theme::space::MEDIUM;
    if selected {
        icons::paint(
            ui.painter(),
            Icon::Check,
            egui::Rect::from_center_size(
                egui::pos2(rect.right() - 18.0, rect.center().y),
                egui::Vec2::splat(icons::GRID * 0.8),
            ),
            theme::accent(),
        );
        name_right = rect.right() - 32.0;
    }
    ui.painter()
        .with_clip_rect(rect.with_max_x(name_right))
        .text(
            egui::pos2(rect.left() + 36.0, rect.center().y),
            Align2::LEFT_CENTER,
            entry.name.to_string_lossy(),
            theme::typography::body(),
            theme::text().primary,
        );
    response
}

/// A rail shortcut: the Places and Recent rows share this one look.
fn workspace_file_picker_location_row(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    selected: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), theme::control::ROW),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            theme::corner(theme::radius::CONTROL),
            if selected {
                theme::state::selected()
            } else {
                theme::state::hover()
            },
        );
    }
    icons::paint(
        ui.painter(),
        icon,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 16.0, rect.center().y),
            egui::Vec2::splat(icons::GRID * 0.9),
        ),
        if selected {
            theme::accent()
        } else {
            theme::text().muted
        },
    );
    ui.painter()
        .with_clip_rect(rect.shrink2(egui::vec2(theme::space::TIGHT, 0.0)))
        .text(
            egui::pos2(rect.left() + 32.0, rect.center().y),
            Align2::LEFT_CENTER,
            label,
            theme::typography::small(),
            if selected {
                theme::text().primary
            } else {
                theme::text().secondary
            },
        );
    response
}

/// The trail the picker's toolbar renders: up to `limit` trailing components
/// of `directory` oldest-first, plus the ancestor hiding behind the leading
/// ellipsis when the path runs deeper than the trail shows.
fn picker_breadcrumb_segments(
    directory: &Path,
    limit: usize,
) -> (Option<PathBuf>, Vec<(String, PathBuf)>) {
    let mut segments = Vec::new();
    let mut cursor = Some(directory.to_path_buf());
    while let Some(path) = cursor {
        if segments.len() == limit {
            segments.reverse();
            return (Some(path), segments);
        }
        let label = path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        cursor = path.parent().map(Path::to_path_buf);
        segments.push((label, path));
    }
    segments.reverse();
    (None, segments)
}

fn file_picker_breadcrumb_chip(ui: &mut egui::Ui, label: &str, current: bool) -> egui::Response {
    let font = if current {
        theme::typography::small_strong()
    } else {
        theme::typography::small()
    };
    let color = if current {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    let galley =
        ui.painter()
            .layout_no_wrap(crate::dialog::middle_truncate(label, 24), font, color);
    let size = egui::vec2(galley.size().x + theme::space::SNUG * 2.0, 22.0);
    let sense = if current {
        Sense::hover()
    } else {
        Sense::click()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    if !current && response.hovered() {
        ui.painter().rect_filled(
            rect,
            theme::corner(theme::radius::ROW),
            theme::state::hover(),
        );
    }
    ui.painter().galley(
        egui::pos2(
            rect.left() + theme::space::SNUG,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        color,
    );
    response
}

fn file_picker_breadcrumb_separator(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(12.0, 22.0), Sense::hover());
    icons::paint(
        ui.painter(),
        Icon::ChevronRight,
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(icons::GRID * 0.55)),
        theme::text_disabled(),
    );
}

/// Paints the clickable directory trail inside the toolbar's address bar;
/// returns a directory when an ancestor segment is clicked.
fn file_picker_breadcrumbs(ui: &mut egui::Ui, directory: &Path) -> Option<PathBuf> {
    let (overflow, segments) = picker_breadcrumb_segments(directory, 4);
    let mut navigate = None;
    ui.spacing_mut().item_spacing.x = 0.0;
    if let Some(ancestor) = overflow {
        if file_picker_breadcrumb_chip(ui, "…", false).clicked() {
            navigate = Some(ancestor);
        }
        file_picker_breadcrumb_separator(ui);
    }
    let last = segments.len().saturating_sub(1);
    for (index, (label, path)) in segments.into_iter().enumerate() {
        let current = index == last;
        let chip = file_picker_breadcrumb_chip(ui, &label, current);
        if !current {
            if chip.clicked() {
                navigate = Some(path);
            }
            file_picker_breadcrumb_separator(ui);
        }
    }
    navigate
}

/// The footer's hidden-files toggle: a real checkbox, the control the
/// operating system's own dialogs put in this corner.
fn file_picker_check_row(ui: &mut egui::Ui, label: &str, checked: bool) -> egui::Response {
    let box_size = theme::space::LARGE;
    let font = theme::typography::small();
    let text_width = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), theme::text().secondary)
        .size()
        .x;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(
            box_size + theme::space::SNUG + text_width,
            theme::control::COMPACT,
        ),
        Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), checked, label)
    });
    let box_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + box_size * 0.5, rect.center().y),
        egui::Vec2::splat(box_size),
    );
    if checked {
        ui.painter()
            .rect_filled(box_rect, theme::corner(theme::radius::ROW), theme::accent());
        icons::paint(
            ui.painter(),
            Icon::Check,
            box_rect.shrink(3.0),
            theme::text().on_accent,
        );
    } else {
        ui.painter().rect_filled(
            box_rect,
            theme::corner(theme::radius::ROW),
            theme::surface().input,
        );
        ui.painter().rect_stroke(
            box_rect,
            theme::corner(theme::radius::ROW),
            if response.hovered() {
                theme::border::strong()
            } else {
                theme::border::hairline()
            },
            egui::StrokeKind::Inside,
        );
    }
    ui.painter().text(
        egui::pos2(box_rect.right() + theme::space::SNUG, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        font,
        if response.hovered() {
            theme::text().primary
        } else {
            theme::text().secondary
        },
    );
    response
}

impl FileTab {
    fn new(buffer: Buffer, pane: PaneId) -> Self {
        Self {
            buffer,
            editor_surface: EditorSurface::default(),
            highlight_cache: HighlightCache::default(),
            pane,
            markdown_preview: false,
            markdown_layout: None,
            agent_diff: None,
            git_diff: None,
            vim: VimState::default(),
        }
    }
}

pub struct EditorApp {
    tabs: Vec<FileTab>,
    active_tab: Option<usize>,
    active_pane: PaneId,
    pane_active_tabs: HashMap<PaneId, PathBuf>,
    pane_layout: PaneLayout,
    tab_drag: Option<PathBuf>,
    tab_drop: Option<TabDrop>,
    tree: TreeState,
    tree_surface: TreeSurface,
    recent_projects: Vec<PathBuf>,
    project_menu: bool,
    /// The previous window was in agent mode when the project switched, so the
    /// replacement starts its providers on the first frame that has a context.
    agent_boot_pending: bool,
    syntaxes: SyntaxManager,
    highlighter: Highlighter,
    search: SearchController,
    search_open: bool,
    search_query: String,
    search_selected: usize,
    focus_search: bool,
    pane_find: HashMap<PaneId, PaneFind>,
    bracket_pair: Option<(std::ops::Range<usize>, std::ops::Range<usize>)>,
    bracket_pair_key: Option<(u64, usize)>,
    sidebar: bool,
    sidebar_pane: SidebarPane,
    sidebar_width: f32,
    sidebar_dragging: bool,
    git_state: GitState,
    git_controller: Option<GitController>,
    git_refresh_at: Option<Instant>,
    git_discard: Option<GitDiscardRequest>,
    terminal_open: bool,
    terminal_height: f32,
    terminal_dragging: bool,
    terminal: TerminalPanel,
    agentic_mode: bool,
    agent_pane_layout: PaneLayout,
    active_agent_pane: PaneId,
    agent_pane_picker: Option<PaneId>,
    agent_pane_runtimes: HashMap<PaneId, AgentPaneRuntime>,
    agent_pane_close_requested: Option<PaneId>,
    agent_pane_drag: Option<AgentPaneDrag>,
    agent_pane_drop: Option<TabDrop>,
    agent_session_drag: Option<SessionChoice>,
    agent_session_drop: Option<TabDrop>,
    agentic_diffs: Vec<AgenticDiff>,
    active_agentic_diff: usize,
    agent_sidebar: bool,
    agent_sidebar_width: f32,
    agent_sidebar_dragging: bool,
    devin_sidebar: bool,
    devin_sidebar_width: f32,
    devin_sidebar_dragging: bool,
    devin_state: DevinState,
    devin_controller: Option<DevinController>,
    devin_api_key: String,
    devin_org_id: String,
    devin_view: DevinView,
    devin_scope: DevinScope,
    devin_filter: String,
    devin_server_filters: SessionFilters,
    devin_filter_tags: String,
    devin_resource_query: String,
    devin_resource_filter: String,
    devin_advanced: DevinAdvancedDraft,
    devin_resource_draft: DevinResourceDraft,
    devin_tags: String,
    devin_activity_query: String,
    devin_list_cursor: usize,
    devin_focus_list: bool,
    devin_focus_create: bool,
    devin_focus_detail: bool,
    devin_repository: String,
    devin_create_prompt: String,
    devin_creating: bool,
    devin_message: String,
    devin_attachments: Vec<AssistantComposerAttachment>,
    devin_drop_hovered: bool,
    devin_pending_message: Option<PendingDevinMessage>,
    devin_pending_lifecycle: Option<DevinLifecycle>,
    devin_confirm_terminate: bool,
    devin_confirm_disconnect: bool,
    devin_confirm_mutation: Option<ResourceMutation>,
    devin_confirm_org: Option<String>,
    agent_menu: Option<AgentMenu>,
    agent_menu_popup: Option<egui::Rect>,
    agent_menu_scroll_y: f32,
    agent_follow_transcript: bool,
    agent_prompt_history_index: Option<usize>,
    agent_prompt_history_draft: String,
    agent_attachments: Vec<AssistantComposerAttachment>,
    assistant_image_lightbox: Option<AssistantImageSource>,
    agent_mentions: Option<Vec<AgentMentionEntry>>,
    agent_mention_matches: Vec<AgentMentionEntry>,
    agent_mention_selected: usize,
    agent_find: AgentFind,
    /// Last measured height of each transcript item, so off-screen items can
    /// be culled into spacers instead of being laid out every frame. `NAN`
    /// means "not measured yet"; visible items re-measure every frame.
    agent_transcript_heights: Vec<f32>,
    /// The layout and provider-session identity that produced the cached heights.
    agent_transcript_heights_key: (u32, u64, bool, bool, u64),
    /// How many transcript items the last frame actually laid out (the rest
    /// were culled spacers); the culling tests key off this.
    agent_transcript_rendered: usize,
    agent_drop_hovered: bool,
    attachment_file_picker: Option<WorkspaceFilePicker>,
    attachment_picker_target: AttachmentTarget,
    project_folder_picker: Option<WorkspaceFilePicker>,
    agent_run_everything: Option<bool>,
    selected_provider: ProviderId,
    available_providers: Vec<ProviderId>,
    provider_agents: HashMap<ProviderId, AgentState>,
    provider_menu_anchor: Option<egui::Rect>,
    scrollbar_activity: crate::scrollbar::Activity,
    agent: AgentState,
    agent_controllers: HashMap<ProviderId, AgentController>,
    pending_agent_prompt: bool,
    focus_editor: bool,
    tree_focused: bool,
    scm_focused: bool,
    tree_prompt: Option<TreePrompt>,
    tree_delete: Option<PathBuf>,
    tree_clipboard: Option<TreeClipboard>,
    cursor: (usize, usize),
    pending: Option<PendingAction>,
    conflict: bool,
    save_as: Option<String>,
    error: Option<String>,
    toasts: crate::toast::Toasts,
    should_close: bool,
    window_action: Option<WindowAction>,
    settings_open: bool,
    settings_section: SettingsSection,
    settings_search: String,
    settings: Settings,
    settings_error: Option<String>,
    settings_drafts: HashMap<PresetId, (String, String)>,
    update_check_started: bool,
    update_available: Arc<std::sync::atomic::AtomicBool>,
    update_error: Arc<std::sync::Mutex<Option<String>>>,
    keybinding_resolver: Resolver,
    keybinding_filter: KeybindingFilter,
    keybinding_category: Option<String>,
    keybinding_vim_scope: Option<Scope>,
    confirm_profile_reset: bool,
    shortcut_recorder: Option<ShortcutRecorder>,
    new_profile: Option<NewProfileDraft>,
    rename_profile: Option<String>,
    vim_session: VimSession,
    vim_overlay: Option<VimOverlay>,
    clipboard_request: Option<ClipboardRequest>,
    lsp_controllers: HashMap<PresetId, LspController>,
    lsp_pending_controls: HashMap<PresetId, LspCommand>,
    lsp_status: HashMap<PresetId, ServerStatus>,
    lsp_detail: HashMap<PresetId, String>,
    lsp_open: HashMap<PathBuf, (PresetId, u64)>,
    lsp_pending_saves: HashSet<PathBuf>,
    lsp_sync_needed: bool,
    lsp_diagnostics: HashMap<PathBuf, LspDiagnosticsState>,
    lsp_generation: u64,
    lsp_scroll_to: Option<(PathBuf, usize)>,
    lsp_caret: Option<LspCaret>,
    lsp_completion: Option<CompletionPopup>,
    lsp_definitions: Option<DefinitionChooser>,
    lsp_pending_completion: Option<(RequestTag, Option<String>)>,
    lsp_pending_definition: Option<RequestTag>,
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
        let initial_pane = PaneId(0);
        let tabs = buffer
            .into_iter()
            .map(|buffer| FileTab::new(buffer, initial_pane))
            .collect::<Vec<_>>();
        let active_tab = (!tabs.is_empty()).then_some(0);
        let pane_active_tabs = tabs
            .first()
            .map(|tab| [(initial_pane, tab.buffer.path.clone())].into())
            .unwrap_or_default();
        let syntaxes = SyntaxManager::built_in()?;
        let search = SearchController::new(target.root.clone())?;
        #[cfg(not(test))]
        let (settings, settings_error) =
            match data_dir().map(|directory| directory.join("settings.json")) {
                Ok(path) => match settings::load(&path) {
                    Ok(settings) => (settings, None),
                    Err(error) => (Settings::default(), Some(error)),
                },
                Err(error) => (Settings::default(), Some(error)),
            };
        #[cfg(test)]
        let (settings, settings_error) = (Settings::default(), None);
        let keybinding_resolver = Resolver::new(
            settings.keybindings.effective_bindings()?,
            KeybindingPlatform::current(),
        )?;
        Ok(Self {
            tabs,
            active_tab,
            active_pane: initial_pane,
            pane_active_tabs,
            pane_layout: PaneLayout::default(),
            tab_drag: None,
            tab_drop: None,
            tree: TreeState::new(target.root, selected)?,
            tree_surface: TreeSurface::default(),
            recent_projects: data_dir()
                .map(|directory| crate::projects::load(&directory))
                .unwrap_or_default(),
            project_menu: false,
            agent_boot_pending: false,
            syntaxes,
            highlighter: Highlighter::new()?,
            search,
            search_open: false,
            search_query: String::new(),
            search_selected: 0,
            focus_search: false,
            pane_find: HashMap::new(),
            bracket_pair: None,
            bracket_pair_key: None,
            sidebar: true,
            sidebar_pane: SidebarPane::Files,
            sidebar_width: 248.0,
            sidebar_dragging: false,
            git_state: GitState::default(),
            git_controller: None,
            git_refresh_at: None,
            git_discard: None,
            terminal_open: false,
            terminal_height: TERMINAL_DEFAULT_HEIGHT,
            terminal_dragging: false,
            terminal: TerminalPanel::default(),
            agentic_mode: false,
            agent_pane_layout: PaneLayout::default(),
            active_agent_pane: PaneId(0),
            agent_pane_picker: None,
            agent_pane_runtimes: HashMap::new(),
            agent_pane_close_requested: None,
            agent_pane_drag: None,
            agent_pane_drop: None,
            agent_session_drag: None,
            agent_session_drop: None,
            agentic_diffs: Vec::new(),
            active_agentic_diff: 0,
            agent_sidebar: false,
            agent_sidebar_width: 440.0,
            agent_sidebar_dragging: false,
            devin_sidebar: false,
            devin_sidebar_width: 440.0,
            devin_sidebar_dragging: false,
            devin_state: DevinState::default(),
            devin_controller: None,
            devin_api_key: String::new(),
            devin_org_id: String::new(),
            devin_view: DevinView::default(),
            devin_scope: DevinScope::default(),
            devin_filter: String::new(),
            devin_server_filters: SessionFilters::default(),
            devin_filter_tags: String::new(),
            devin_resource_query: String::new(),
            devin_resource_filter: String::new(),
            devin_advanced: DevinAdvancedDraft::default(),
            devin_resource_draft: DevinResourceDraft::default(),
            devin_tags: String::new(),
            devin_activity_query: String::new(),
            devin_list_cursor: 0,
            devin_focus_list: false,
            devin_focus_create: false,
            devin_focus_detail: false,
            devin_repository: String::new(),
            devin_create_prompt: String::new(),
            devin_creating: false,
            devin_message: String::new(),
            devin_attachments: Vec::new(),
            devin_drop_hovered: false,
            devin_pending_message: None,
            devin_pending_lifecycle: None,
            devin_confirm_terminate: false,
            devin_confirm_disconnect: false,
            devin_confirm_mutation: None,
            devin_confirm_org: None,
            agent_menu: None,
            agent_menu_popup: None,
            agent_menu_scroll_y: 0.0,
            agent_follow_transcript: true,
            agent_prompt_history_index: None,
            agent_prompt_history_draft: String::new(),
            agent_attachments: Vec::new(),
            assistant_image_lightbox: None,
            agent_mentions: None,
            agent_mention_matches: Vec::new(),
            agent_mention_selected: 0,
            agent_find: AgentFind::default(),
            agent_transcript_heights: Vec::new(),
            agent_transcript_heights_key: (0, 0, false, false, 0),
            agent_transcript_rendered: 0,
            agent_drop_hovered: false,
            attachment_file_picker: None,
            attachment_picker_target: AttachmentTarget::Agent,
            project_folder_picker: None,
            agent_run_everything: None,
            selected_provider: ProviderId::Cursor,
            available_providers: Vec::new(),
            provider_agents: HashMap::new(),
            provider_menu_anchor: None,
            scrollbar_activity: crate::scrollbar::Activity::default(),
            agent: AgentState::default(),
            agent_controllers: HashMap::new(),
            pending_agent_prompt: false,
            focus_editor: target.file.is_some(),
            tree_focused: target.file.is_none(),
            scm_focused: false,
            tree_prompt: None,
            tree_delete: None,
            tree_clipboard: None,
            cursor: (1, 1),
            pending: None,
            conflict: false,
            save_as: None,
            error: None,
            toasts: crate::toast::Toasts::default(),
            should_close: false,
            window_action: None,
            settings_open: false,
            update_check_started: false,
            update_available: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            update_error: Arc::new(std::sync::Mutex::new(None)),
            settings_section: SettingsSection::LanguageServers,
            settings_search: String::new(),
            settings,
            settings_error,
            settings_drafts: HashMap::new(),
            keybinding_resolver,
            keybinding_filter: KeybindingFilter::All,
            keybinding_category: None,
            keybinding_vim_scope: None,
            confirm_profile_reset: false,
            shortcut_recorder: None,
            new_profile: None,
            rename_profile: None,
            vim_session: VimSession::default(),
            vim_overlay: None,
            clipboard_request: None,
            lsp_controllers: HashMap::new(),
            lsp_pending_controls: HashMap::new(),
            lsp_status: HashMap::new(),
            lsp_detail: HashMap::new(),
            lsp_open: HashMap::new(),
            lsp_pending_saves: HashSet::new(),
            lsp_sync_needed: true,
            lsp_diagnostics: HashMap::new(),
            lsp_generation: 0,
            lsp_scroll_to: None,
            lsp_caret: None,
            lsp_completion: None,
            lsp_definitions: None,
            lsp_pending_completion: None,
            lsp_pending_definition: None,
        })
    }

    /// Asks the release channel once per launch whether a newer build exists.
    /// The check runs off-thread and lights the sidebar's update button when
    /// it lands; tests never start it, so they stay off the network.
    fn start_update_check(&mut self, ctx: &egui::Context) {
        if self.update_check_started {
            return;
        }
        self.update_check_started = true;
        if cfg!(test) {
            return;
        }
        let available = Arc::clone(&self.update_available);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if crate::update::check_available().unwrap_or(false) {
                available.store(true, std::sync::atomic::Ordering::Relaxed);
                ctx.request_repaint();
            }
        });
    }

    /// Hands the update to a fresh `editur update` process, which asks this
    /// editor to quit once the download verifies, then installs the new
    /// build. If the updater fails instead, its message comes back as a toast.
    fn start_update(&mut self, ctx: &egui::Context) {
        let child = match crate::update::start_in_background() {
            Ok(child) => child,
            Err(error) => return self.show_error(error),
        };
        let slot = Arc::clone(&self.update_error);
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let Ok(output) = child.wait_with_output() else {
                return;
            };
            if output.status.success() {
                return;
            }
            let stderr = String::from_utf8_lossy(&output.stderr);
            let message = stderr
                .lines()
                .rev()
                .map(str::trim)
                .find(|line| !line.is_empty())
                .map_or_else(
                    || "the update did not finish".to_owned(),
                    |line| line.trim_start_matches("editur: ").to_owned(),
                );
            if let Ok(mut slot) = slot.lock() {
                *slot = Some(message);
            }
            ctx.request_repaint();
        });
    }

    pub fn ui(&mut self, root: &mut egui::Ui) {
        self.draw_ui(root);
        crate::renderer::end_retained(root.painter(), root.max_rect());
    }

    fn draw_ui(&mut self, root: &mut egui::Ui) {
        self.apply_appearance(root.ctx());
        theme::apply_to(root.style_mut());
        self.scrollbar_activity.style_egui(root);
        let ctx = root.ctx().clone();
        self.start_update_check(&ctx);
        if let Some(error) = self
            .update_error
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
        {
            self.show_error(error);
        }
        if self.agent_boot_pending {
            self.agent_boot_pending = false;
            self.open_agent(&ctx);
        }
        self.flush_git_refresh(&ctx);
        self.poll_git();
        let any_agent_active = self.poll_agent_panes(&ctx);
        self.poll_devin(&ctx);
        if any_agent_active {
            ctx.request_repaint_after(Duration::from_millis(500));
        }
        self.shortcuts(&ctx);
        self.poll_lsp(&ctx);
        if self.lsp_sync_needed {
            self.sync_lsp_documents(&ctx);
        }
        if self.settings_open {
            self.draw_settings(root, root.max_rect());
            self.draw_dialogs(&ctx);
            self.draw_error(&ctx);
            return;
        }
        let find_panes = self
            .pane_find
            .iter()
            .filter_map(|(pane, find)| find.open.then_some(*pane))
            .collect::<Vec<_>>();
        for pane in find_panes {
            self.refresh_find_matches(pane);
        }
        if self.search_open {
            self.search.poll(&self.search_query);
            let results = self.search.results();
            if search_needs_polling(&self.search_query, &results.query, results.complete) {
                ctx.request_repaint_after(Duration::from_millis(50));
            }
        }

        let window = root.max_rect();
        if self.agentic_mode {
            self.update_agentic_sidebar_resize(&ctx, window);
            self.draw_agentic_workspace(root, window);
            self.draw_dialogs(&ctx);
            self.draw_error(&ctx);
            return;
        }
        self.update_sidebar_resizes(&ctx, window);
        let agent_sidebar_at_frame_start = self.agent_sidebar;
        let devin_sidebar_at_frame_start = self.devin_sidebar;
        let (sidebar, editor_column, agent, devin) = split_workspace_with_devin(
            window,
            self.sidebar,
            self.sidebar_width,
            self.agent_sidebar,
            self.agent_sidebar_width,
            self.devin_sidebar,
            self.devin_sidebar_width,
        );
        let workspace = egui::Rect::from_min_max(editor_column.left_top(), window.right_bottom());
        self.update_terminal_resize(&ctx, workspace);
        let (workspace, terminal) =
            split_bottom_panel(workspace, self.terminal_open, self.terminal_height);
        let editor_column = editor_column.with_max_y(workspace.bottom());
        let agent = agent.with_max_y(workspace.bottom());
        let devin = devin.with_max_y(workspace.bottom());
        let editor = editor_column_content(editor_column);
        if let Some(sidebar) = sidebar {
            root.scope_builder(
                UiBuilder::new().id_salt("sidebar").max_rect(sidebar),
                |ui| self.draw_sidebar(ui),
            );
        }
        self.lsp_caret = None;
        let pane_rects = self.pane_layout.rects(editor);
        self.update_tab_drag(&ctx, &pane_rects);
        let pane_resize_handles = if self.tab_drag.is_none() {
            resize_dragged_pane_handle(
                &ctx,
                &mut self.pane_layout,
                editor,
                "pane_split_divider",
                false,
            )
        } else {
            Vec::new()
        };
        let mut pane_rects = self.pane_layout.rects(editor);
        let mut dragged_pane = None;
        if let (Some(path), Some(drop)) = (self.tab_drag.as_deref(), self.tab_drop)
            && let Some((preview, pane)) = self.tab_drag_preview(editor, path, drop)
        {
            pane_rects = preview;
            dragged_pane = Some(pane);
        }
        let dragged_path = self.tab_drag.clone();
        let dragged_source = dragged_path.as_deref().and_then(|path| {
            self.tabs
                .iter()
                .find(|tab| tab.buffer.path == path)
                .map(|tab| tab.pane)
        });
        let single_pane = pane_rects.len() == 1;
        let titlebar = window.with_max_y((window.top() + TITLEBAR_HEIGHT).min(window.bottom()));
        for (pane, rect) in pane_rects.iter().copied() {
            let (header, content) = pane_header_and_content(titlebar, editor, rect);
            let pane_bounds = egui::Rect::from_min_max(header.left_top(), rect.right_bottom());
            let pointer_interaction = ctx
                .pointer_hover_pos()
                .is_some_and(|pointer| pane_bounds.shrink(1.0).contains(pointer))
                && ctx.input(|input| {
                    input.pointer.any_pressed() || input.smooth_scroll_delta != egui::Vec2::ZERO
                });
            if pane != self.active_pane
                && pointer_interaction
                && let Some(index) = self
                    .pane_active_tabs
                    .get(&pane)
                    .and_then(|path| self.tabs.iter().position(|tab| &tab.buffer.path == path))
            {
                self.activate_tab(index);
            }
            let (content, findbar) = split_pane_content(
                content,
                self.pane_find.get(&pane).is_some_and(|find| find.open),
            );
            if header.top() >= editor.top() - 0.5 {
                root.scope_builder(
                    UiBuilder::new()
                        .id_salt(("editor_pane_header", pane.0))
                        .max_rect(header),
                    |ui| {
                        self.draw_pane_header(
                            ui,
                            header,
                            pane,
                            header.left(),
                            header.right(),
                            (dragged_pane == Some(pane))
                                .then_some(dragged_path.as_deref())
                                .flatten(),
                        );
                    },
                );
            }
            if dragged_pane == Some(pane) {
                if dragged_path
                    .as_deref()
                    .is_some_and(|path| self.tabs.iter().any(|tab| tab.buffer.path == path))
                {
                    root.scope_builder(
                        UiBuilder::new()
                            .id_salt(("editor_pane_preview", pane.0))
                            .max_rect(content),
                        |ui| {
                            self.draw_editor_pane(
                                ui,
                                pane,
                                editor,
                                false,
                                dragged_path.as_deref(),
                                true,
                            );
                        },
                    );
                } else {
                    draw_dragged_pane_preview(root.painter(), content, dragged_path.as_deref());
                }
                continue;
            }
            let path_override = (dragged_pane.is_some() && dragged_source == Some(pane))
                .then(|| {
                    self.tabs
                        .iter()
                        .find(|tab| {
                            tab.pane == pane
                                && dragged_path.as_deref() != Some(tab.buffer.path.as_path())
                        })
                        .map(|tab| tab.buffer.path.clone())
                })
                .flatten();
            root.scope_builder(
                UiBuilder::new()
                    .id_salt(("editor_pane", pane.0))
                    .max_rect(content),
                |ui| {
                    self.draw_editor_pane(
                        ui,
                        pane,
                        editor,
                        single_pane,
                        path_override.as_deref(),
                        false,
                    );
                },
            );
            if let Some(findbar) = findbar {
                root.scope_builder(
                    UiBuilder::new()
                        .id_salt(("file_search_bar", pane.0))
                        .max_rect(findbar),
                    |ui| self.draw_find(ui, pane),
                );
            }
        }
        if self.agent_sidebar {
            root.scope_builder(
                UiBuilder::new().id_salt("agent_sidebar").max_rect(agent),
                |ui| self.draw_agent_sidebar(ui),
            );
        }
        if self.devin_sidebar {
            root.scope_builder(
                UiBuilder::new().id_salt("devin_sidebar").max_rect(devin),
                |ui| self.draw_devin_sidebar(ui),
            );
        }
        if let Some(terminal) = terminal {
            let output = self.terminal.show(root, terminal, &self.tree.root);
            if output.empty {
                self.terminal_open = false;
            }
            if let Some(error) = output.error {
                self.show_error(error);
            }
            self.draw_terminal_resize(root, terminal);
        }
        self.draw_titlebar(
            root,
            titlebar,
            editor,
            &pane_rects,
            (agent_sidebar_at_frame_start, devin_sidebar_at_frame_start),
            dragged_pane,
        );
        if !self.terminal.focused(&ctx)
            && pane_rects.len() > 1
            && let Some((_, rect)) = pane_rects
                .iter()
                .find(|(pane, _)| *pane == self.active_pane)
        {
            let (header, _) = pane_header_and_content(titlebar, editor, *rect);
            let focus_rect = egui::Rect::from_min_max(header.left_top(), rect.right_bottom());
            root.painter().rect_stroke(
                focus_rect,
                pane_focus_corner_radius(focus_rect, window),
                egui::Stroke::new(1.0, theme::border::focus_color()),
                egui::StrokeKind::Inside,
            );
        }
        paint_pane_resize_handles(root, &pane_resize_handles, "pane_split_divider");
        if let Some(sidebar) = sidebar {
            let divider = egui::Rect::from_center_size(
                egui::pos2(sidebar.right(), sidebar.center().y),
                egui::vec2(5.0, sidebar.height()),
            );
            let pointer = ctx.pointer_hover_pos();
            let hovered = pointer.is_some_and(|pointer| divider.contains(pointer));
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
                resize_divider_stroke(&ctx, active),
            );
        }
        if self.agent_sidebar {
            let divider = egui::Rect::from_center_size(
                egui::pos2(agent.left(), agent.center().y),
                egui::vec2(5.0, agent.height()),
            );
            let pointer = ctx.pointer_hover_pos();
            let hovered = pointer.is_some_and(|pointer| divider.contains(pointer));
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
                resize_divider_stroke(&ctx, active),
            );
        }
        if self.devin_sidebar {
            let divider = egui::Rect::from_center_size(
                egui::pos2(devin.left(), devin.center().y),
                egui::vec2(5.0, devin.height()),
            );
            let pointer = ctx.pointer_hover_pos();
            let hovered = pointer.is_some_and(|pointer| divider.contains(pointer));
            if hovered || self.devin_sidebar_dragging {
                ctx.set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            let active = hovered || self.devin_sidebar_dragging;
            crate::renderer::mark_retained(
                root.painter(),
                divider,
                0x6000_0000_0000_0000,
                u64::from(divider.center().x.to_bits())
                    ^ u64::from(divider.height().to_bits()).rotate_left(32)
                    ^ ((active as u64) << 63),
            );
            root.painter().line_segment(
                [divider.center_top(), divider.center_bottom()],
                resize_divider_stroke(&ctx, active),
            );
        }
        if let Some(drop) = self.tab_drop.filter(|drop| drop.zone == DropZone::Center) {
            root.painter().rect_filled(
                drop.preview.shrink(4.0),
                5.0,
                theme::subtle(theme::accent()),
            );
            root.painter().rect_stroke(
                drop.preview.shrink(4.0),
                5.0,
                egui::Stroke::new(1.5, theme::accent()),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(path) = self.tab_drag.as_deref() {
            draw_tab_drag_ghost(&ctx, &drag_label(path));
            ctx.set_cursor_icon(CursorIcon::Grabbing);
        }
        if self.lsp_sync_needed {
            self.sync_lsp_documents(&ctx);
        }
        self.draw_lsp_popups(root);
        self.draw_search(root);
        self.draw_vim_overlay(&ctx);
        self.draw_dialogs(&ctx);
        self.draw_error(&ctx);
    }

    fn update_sidebar_resizes(&mut self, ctx: &egui::Context, window: egui::Rect) {
        let (sidebar, _, agent, devin) = split_workspace_with_devin(
            window,
            self.sidebar,
            self.sidebar_width,
            self.agent_sidebar,
            self.agent_sidebar_width,
            self.devin_sidebar,
            self.devin_sidebar_width,
        );
        let pointer = ctx.pointer_hover_pos();
        let (pressed, down) = ctx.input(|input| {
            (
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
            )
        });
        if let Some(sidebar) = sidebar {
            let divider = egui::Rect::from_center_size(
                egui::pos2(sidebar.right(), sidebar.center().y),
                egui::vec2(5.0, sidebar.height()),
            );
            if pressed && pointer.is_some_and(|pointer| divider.contains(pointer)) {
                self.sidebar_dragging = true;
            }
        }
        if self.agent_sidebar {
            let divider = egui::Rect::from_center_size(
                egui::pos2(agent.left(), agent.center().y),
                egui::vec2(5.0, agent.height()),
            );
            if pressed && pointer.is_some_and(|pointer| divider.contains(pointer)) {
                self.agent_sidebar_dragging = true;
            }
        }
        if self.devin_sidebar {
            let divider = egui::Rect::from_center_size(
                egui::pos2(devin.left(), devin.center().y),
                egui::vec2(5.0, devin.height()),
            );
            if pressed && pointer.is_some_and(|pointer| divider.contains(pointer)) {
                self.devin_sidebar_dragging = true;
            }
        }
        if !down {
            self.sidebar_dragging = false;
            self.agent_sidebar_dragging = false;
            self.devin_sidebar_dragging = false;
        } else if let Some(pointer) = pointer {
            if self.sidebar_dragging {
                self.sidebar_width = (pointer.x - window.left()).clamp(SIDEBAR_MIN_WIDTH, 500.0);
            }
            if self.agent_sidebar_dragging {
                let right = if self.devin_sidebar {
                    devin.left()
                } else {
                    window.right()
                };
                self.agent_sidebar_width = (right - pointer.x).clamp(320.0, 720.0);
            }
            if self.devin_sidebar_dragging {
                self.devin_sidebar_width = (window.right() - pointer.x).clamp(320.0, 720.0);
            }
        }
    }

    fn update_agentic_sidebar_resize(&mut self, ctx: &egui::Context, window: egui::Rect) {
        let (Some(sidebar), _) = split_agentic_workspace(window, self.sidebar, self.sidebar_width)
        else {
            self.sidebar_dragging = false;
            return;
        };
        let divider =
            egui::Rect::from_center_size(sidebar.right_center(), egui::vec2(5.0, sidebar.height()));
        let pointer = ctx.pointer_hover_pos();
        let (pressed, down) = ctx.input(|input| {
            (
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
            )
        });
        if pressed && pointer.is_some_and(|pointer| divider.contains(pointer)) {
            self.sidebar_dragging = true;
        }
        if !down {
            self.sidebar_dragging = false;
        } else if self.sidebar_dragging
            && let Some(pointer) = pointer
        {
            self.sidebar_width = pointer.x - window.left();
        }
    }

    fn draw_agentic_workspace(&mut self, root: &mut egui::Ui, window: egui::Rect) {
        let (sessions, agent_column) =
            split_agentic_workspace(window, self.sidebar, self.sidebar_width);
        self.update_terminal_resize(root.ctx(), agent_column);
        let (content, terminal) =
            split_bottom_panel(agent_column, self.terminal_open, self.terminal_height);
        let (agent, diff_panel) = split_agentic_diff(content, !self.agentic_diffs.is_empty());
        if let Some(sessions) = sessions {
            root.scope_builder(
                UiBuilder::new()
                    .id_salt("agentic_sessions")
                    .max_rect(sessions),
                |ui| self.draw_agentic_sessions(ui),
            );
        }
        let available_sessions = self.agent.sessions.clone().unwrap_or_default();
        let agent_panes = self.agent_pane_layout.rects(agent);
        self.update_agent_session_drag(root.ctx(), &agent_panes);
        let agent_panes = self.agent_pane_layout.rects(agent);
        let pane_drag_finished = self.update_agent_pane_drag(root.ctx(), &agent_panes);
        let agent_panes = self.agent_pane_layout.rects(agent);
        let selected_pane = self.active_agent_pane;
        let interacted_pane = (!pane_drag_finished)
            .then(|| {
                root.ctx().pointer_hover_pos().and_then(|pointer| {
                    root.ctx()
                        .input(|input| {
                            input.pointer.primary_pressed() || input.pointer.primary_released()
                        })
                        .then(|| {
                            agent_panes
                                .iter()
                                .find(|(_, rect)| rect.contains(pointer))
                                .map(|(pane, _)| *pane)
                        })
                        .flatten()
                })
            })
            .flatten();
        let pane_resize_handles =
            if self.agent_session_drag.is_none() && self.agent_pane_drag.is_none() {
                resize_dragged_pane_handle(
                    root.ctx(),
                    &mut self.agent_pane_layout,
                    agent,
                    "agent_pane_divider",
                    false,
                )
            } else {
                Vec::new()
            };
        for (pane, pane_rect) in agent_panes.iter().copied() {
            root.scope_builder(
                UiBuilder::new()
                    .id_salt(("agentic_canvas", pane.0))
                    .max_rect(pane_rect),
                |ui| {
                    ui.painter()
                        .rect_filled(ui.max_rect(), 0.0, editor_background());
                    if self.agent_pane_picker == Some(pane) {
                        self.draw_agent_session_picker(ui, pane_rect, pane, &available_sessions);
                    } else if self.activate_agent_session_pane(pane) {
                        self.draw_agent(ui, pane_rect);
                    }
                },
            );
        }
        if let Some(pane) = self.agent_pane_close_requested.take() {
            self.remove_agent_session_pane(pane);
        }
        let restore = interacted_pane
            .filter(|pane| self.agent_pane_picker != Some(*pane))
            .unwrap_or(selected_pane);
        self.activate_agent_session_pane(restore);
        paint_pane_resize_handles(root, &pane_resize_handles, "agent_pane_divider");
        if let Some(drop) = self.agent_session_drop {
            let preview = drop.preview.shrink(4.0);
            root.painter()
                .rect_filled(preview, 5.0, theme::subtle(theme::accent()));
            root.painter().rect_stroke(
                preview,
                5.0,
                egui::Stroke::new(1.5, theme::accent()),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(drop) = self.agent_pane_drop {
            let preview = drop.preview.shrink(4.0);
            root.painter()
                .rect_filled(preview, 5.0, theme::subtle(theme::accent()));
            root.painter().rect_stroke(
                preview,
                5.0,
                egui::Stroke::new(1.5, theme::accent()),
                egui::StrokeKind::Inside,
            );
        }
        if let Some(session) = &self.agent_session_drag {
            draw_tab_drag_ghost(
                root.ctx(),
                session
                    .title
                    .as_deref()
                    .filter(|title| !title.trim().is_empty())
                    .unwrap_or("Untitled session"),
            );
            root.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        if let Some(drag) = &self.agent_pane_drag {
            draw_tab_drag_ghost(root.ctx(), &drag.title);
            root.ctx().set_cursor_icon(CursorIcon::Grabbing);
        }
        if let Some(rect) = diff_panel {
            let mut selected_tab = None;
            let mut closed_tab = None;
            root.scope_builder(
                UiBuilder::new()
                    .id_salt("agentic_diff_panel")
                    .max_rect(rect),
                |ui| {
                    ui.painter()
                        .rect_filled(ui.max_rect(), 0.0, theme::surface().input);
                    let panel = self
                        .agentic_diffs
                        .get(self.active_agentic_diff)
                        .expect("the panel rect exists only while a diff is open");
                    let diff_id = Id::new(("agentic_diff", &panel.path));
                    let diff =
                        cached_agent_diff(ui, diff_id, panel.baseline.as_deref(), &panel.text);
                    // Diff tabs share the titlebar strip with the session
                    // title, so both columns wear one continuous header.
                    let strip = rect.with_max_y((rect.top() + TITLEBAR_HEIGHT).min(rect.bottom()));
                    ui.painter()
                        .rect_filled(strip, 0.0, theme::surface().chrome);
                    ui.painter().hline(
                        strip.x_range(),
                        strip.bottom() - 0.5,
                        egui::Stroke::new(1.0, theme::border::hairline_color()),
                    );
                    // On platforms with window controls in the top-right
                    // corner the header stops short of them.
                    #[cfg(target_os = "macos")]
                    let header_right = strip.right() - 14.0;
                    #[cfg(not(target_os = "macos"))]
                    let header_right = strip.right() - 3.0 * 46.0;
                    let header = egui::Rect::from_min_max(
                        strip.left_top(),
                        egui::pos2(header_right, strip.bottom()),
                    );
                    let summary_width = 160.0_f32.min((header.width() - TAB_MIN_WIDTH).max(0.0));
                    let tabs = header.with_max_x(header.right() - summary_width);
                    let summary = header
                        .with_min_x(tabs.right())
                        .shrink2(egui::vec2(theme::space::MEDIUM, 0.0));
                    (selected_tab, closed_tab) = draw_agentic_diff_tabs(
                        ui,
                        tabs,
                        &self.agentic_diffs,
                        self.active_agentic_diff,
                    );
                    ui.scope_builder(
                        UiBuilder::new()
                            .id_salt("agentic_diff_summary")
                            .max_rect(summary)
                            .layout(Layout::right_to_left(Align::Center)),
                        |ui| {
                            ui.label(
                                RichText::new(format!("+{}  −{}", diff.added, diff.removed))
                                    .monospace()
                                    .size(theme::typography::MICRO_SIZE)
                                    .color(theme::text().muted),
                            );
                            ui.label(
                                RichText::new(if panel.baseline.is_some() {
                                    "MODIFIED"
                                } else {
                                    "NEW FILE"
                                })
                                .size(theme::typography::MICRO_SIZE)
                                .strong()
                                .color(theme::accent()),
                            );
                        },
                    );
                    ui.scope_builder(
                        UiBuilder::new()
                            .id_salt("agentic_diff_content")
                            .max_rect(rect.with_min_y(strip.bottom().min(rect.bottom()))),
                        |ui| {
                            draw_agent_diff_body(
                                ui,
                                diff_id,
                                &panel.path,
                                &diff,
                                panel.baseline.as_deref(),
                                &panel.text,
                                &self.highlighter,
                                &self.syntaxes,
                            );
                        },
                    );
                },
            );
            root.painter().vline(
                rect.left(),
                rect.y_range(),
                egui::Stroke::new(1.0, theme::border::hairline_color()),
            );
            if let Some(index) = closed_tab {
                self.agentic_diffs.remove(index);
                if self.agentic_diffs.is_empty() {
                    self.active_agentic_diff = 0;
                } else if self.active_agentic_diff > index {
                    self.active_agentic_diff -= 1;
                } else {
                    self.active_agentic_diff =
                        self.active_agentic_diff.min(self.agentic_diffs.len() - 1);
                }
            } else if let Some(index) = selected_tab {
                self.active_agentic_diff = index;
            }
        }
        if let Some(terminal) = terminal {
            let output = self.terminal.show(root, terminal, &self.tree.root);
            if output.empty {
                self.terminal_open = false;
            }
            if let Some(error) = output.error {
                self.show_error(error);
            }
            self.draw_terminal_resize(root, terminal);
        }
        self.draw_agentic_titlebar(
            root,
            window.with_max_y((window.top() + TITLEBAR_HEIGHT).min(window.bottom())),
            agent,
            sessions,
        );
        if let Some(sessions) = sessions {
            let divider = egui::Rect::from_center_size(
                sessions.right_center(),
                egui::vec2(5.0, sessions.height()),
            );
            let hovered = root
                .ctx()
                .pointer_hover_pos()
                .is_some_and(|pointer| divider.contains(pointer));
            let active = hovered || self.sidebar_dragging;
            if active {
                root.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            root.painter().vline(
                sessions.right(),
                sessions.y_range(),
                resize_divider_stroke(root.ctx(), active),
            );
        }
    }

    fn open_agent_session_pane(&mut self) -> Option<PaneId> {
        if self.agent_pane_picker.is_some() {
            return None;
        }
        let pane = self
            .agent_pane_layout
            .split(self.active_agent_pane, DropZone::Right)?;
        self.agent_pane_picker = Some(pane);
        Some(pane)
    }

    fn remove_agent_session_pane(&mut self, pane: PaneId) -> bool {
        if !self.agent_pane_layout.remove(pane) {
            return false;
        }
        self.agent_pane_runtimes.remove(&pane);
        if self.agent_pane_picker == Some(pane) {
            self.agent_pane_picker = None;
        }
        if self.active_agent_pane != pane {
            return true;
        }
        let next = self
            .agent_pane_layout
            .panes()
            .into_iter()
            .find(|pane| self.agent_pane_picker != Some(*pane))
            .or(self.agent_pane_picker)
            .expect("a pane remains after removing a split pane");
        self.active_agent_pane = next;
        let runtime = self
            .agent_pane_runtimes
            .remove(&next)
            .unwrap_or_else(|| AgentPaneRuntime::blank(self.selected_provider));
        self.put_agent_pane_runtime(runtime);
        true
    }

    fn move_agent_session_pane(
        &mut self,
        source: PaneId,
        target: PaneId,
        zone: DropZone,
    ) -> Option<PaneId> {
        let panes = self.agent_pane_layout.panes();
        if source == target || !panes.contains(&source) || !panes.contains(&target) {
            return None;
        }
        let runtime = self.agent_pane_runtimes.remove(&source);
        self.agent_pane_layout.remove(source);
        let moved = self.agent_pane_layout.split(
            target,
            if zone == DropZone::Center {
                DropZone::Right
            } else {
                zone
            },
        )?;
        if self.agent_pane_picker == Some(source) {
            self.agent_pane_picker = Some(moved);
        } else if self.active_agent_pane == source {
            self.active_agent_pane = moved;
        } else if let Some(runtime) = runtime {
            self.agent_pane_runtimes.insert(moved, runtime);
        }
        Some(moved)
    }

    fn take_agent_pane_runtime(&mut self) -> AgentPaneRuntime {
        AgentPaneRuntime {
            agent_menu: self.agent_menu.take(),
            agent_menu_popup: self.agent_menu_popup.take(),
            agent_menu_scroll_y: std::mem::take(&mut self.agent_menu_scroll_y),
            agent_follow_transcript: std::mem::replace(&mut self.agent_follow_transcript, true),
            agent_prompt_history_index: self.agent_prompt_history_index.take(),
            agent_prompt_history_draft: std::mem::take(&mut self.agent_prompt_history_draft),
            agent_attachments: std::mem::take(&mut self.agent_attachments),
            agent_mentions: self.agent_mentions.take(),
            agent_mention_matches: std::mem::take(&mut self.agent_mention_matches),
            agent_mention_selected: std::mem::take(&mut self.agent_mention_selected),
            agent_find: std::mem::take(&mut self.agent_find),
            agent_transcript_heights: std::mem::take(&mut self.agent_transcript_heights),
            agent_transcript_heights_key: std::mem::replace(
                &mut self.agent_transcript_heights_key,
                (0, 0, false, false, 0),
            ),
            agent_transcript_rendered: std::mem::take(&mut self.agent_transcript_rendered),
            agent_drop_hovered: std::mem::take(&mut self.agent_drop_hovered),
            agent_run_everything: self.agent_run_everything.take(),
            selected_provider: std::mem::replace(&mut self.selected_provider, ProviderId::Cursor),
            provider_agents: std::mem::take(&mut self.provider_agents),
            provider_menu_anchor: self.provider_menu_anchor.take(),
            agent: std::mem::take(&mut self.agent),
            agent_controllers: std::mem::take(&mut self.agent_controllers),
            pending_agent_prompt: std::mem::take(&mut self.pending_agent_prompt),
        }
    }

    fn put_agent_pane_runtime(&mut self, runtime: AgentPaneRuntime) {
        self.agent_menu = runtime.agent_menu;
        self.agent_menu_popup = runtime.agent_menu_popup;
        self.agent_menu_scroll_y = runtime.agent_menu_scroll_y;
        self.agent_follow_transcript = runtime.agent_follow_transcript;
        self.agent_prompt_history_index = runtime.agent_prompt_history_index;
        self.agent_prompt_history_draft = runtime.agent_prompt_history_draft;
        self.agent_attachments = runtime.agent_attachments;
        self.agent_mentions = runtime.agent_mentions;
        self.agent_mention_matches = runtime.agent_mention_matches;
        self.agent_mention_selected = runtime.agent_mention_selected;
        self.agent_find = runtime.agent_find;
        self.agent_transcript_heights = runtime.agent_transcript_heights;
        self.agent_transcript_heights_key = runtime.agent_transcript_heights_key;
        self.agent_transcript_rendered = runtime.agent_transcript_rendered;
        self.agent_drop_hovered = runtime.agent_drop_hovered;
        self.agent_run_everything = runtime.agent_run_everything;
        self.selected_provider = runtime.selected_provider;
        self.provider_agents = runtime.provider_agents;
        self.provider_menu_anchor = runtime.provider_menu_anchor;
        self.agent = runtime.agent;
        self.agent_controllers = runtime.agent_controllers;
        self.pending_agent_prompt = runtime.pending_agent_prompt;
    }

    fn activate_agent_session_pane(&mut self, pane: PaneId) -> bool {
        if pane == self.active_agent_pane {
            return true;
        }
        let Some(runtime) = self.agent_pane_runtimes.remove(&pane) else {
            return false;
        };
        let previous_pane = std::mem::replace(&mut self.active_agent_pane, pane);
        let previous_runtime = self.take_agent_pane_runtime();
        self.put_agent_pane_runtime(runtime);
        self.agent_pane_runtimes
            .insert(previous_pane, previous_runtime);
        true
    }

    fn assign_agent_session_pane(&mut self, pane: PaneId, session_id: Option<String>) {
        if self.agent_pane_picker != Some(pane) {
            return;
        }
        let mut runtime = AgentPaneRuntime::blank(self.selected_provider);
        runtime.agent.session_id = session_id;
        self.agent_pane_picker = None;
        if self.active_agent_pane == pane {
            self.put_agent_pane_runtime(runtime);
        } else {
            self.agent_pane_runtimes.insert(pane, runtime);
            self.activate_agent_session_pane(pane);
        }
    }

    fn place_agent_session(
        &mut self,
        target: PaneId,
        zone: DropZone,
        session_id: String,
    ) -> Option<PaneId> {
        if self.agent_pane_picker == Some(target) {
            self.assign_agent_session_pane(target, Some(session_id));
            return Some(target);
        }
        let pane = self.agent_pane_layout.split(
            target,
            if zone == DropZone::Center {
                DropZone::Right
            } else {
                zone
            },
        )?;
        let mut runtime = AgentPaneRuntime::blank(self.selected_provider);
        runtime.agent.session_id = Some(session_id);
        self.agent_pane_runtimes.insert(pane, runtime);
        self.activate_agent_session_pane(pane);
        Some(pane)
    }

    fn update_agent_session_drag(&mut self, ctx: &egui::Context, panes: &[(PaneId, egui::Rect)]) {
        if self.agent_session_drag.is_none() {
            self.agent_session_drop = None;
            return;
        }
        let previous = self.agent_session_drop;
        self.agent_session_drop = ctx.pointer_hover_pos().and_then(|pointer| {
            panes.iter().find_map(|(target, rect)| {
                rect.contains(pointer).then(|| {
                    let zone = if self.agent_pane_picker == Some(*target) {
                        DropZone::Center
                    } else {
                        let previous = previous
                            .filter(|drop| drop.target == *target)
                            .map(|drop| drop.zone);
                        let zone = stable_tab_drop_zone(*rect, pointer, previous);
                        if zone == DropZone::Center {
                            DropZone::Right
                        } else {
                            zone
                        }
                    };
                    TabDrop {
                        target: *target,
                        zone,
                        preview: tab_drop_preview(*rect, zone),
                    }
                })
            })
        });
        if ctx.input(|input| input.pointer.primary_released()) {
            let session = self.agent_session_drag.take();
            let drop = self.agent_session_drop.take();
            if let (Some(session), Some(drop)) = (session, drop)
                && self
                    .place_agent_session(drop.target, drop.zone, session.id)
                    .is_some()
            {
                self.start_provider(self.selected_provider, ctx, false);
            }
        } else if !ctx.input(|input| input.pointer.primary_down()) {
            self.agent_session_drag = None;
            self.agent_session_drop = None;
        } else {
            ctx.request_repaint();
        }
    }

    fn update_agent_pane_drag(
        &mut self,
        ctx: &egui::Context,
        panes: &[(PaneId, egui::Rect)],
    ) -> bool {
        let Some(source) = self.agent_pane_drag.as_ref().map(|drag| drag.pane) else {
            self.agent_pane_drop = None;
            return false;
        };
        let previous = self.agent_pane_drop;
        self.agent_pane_drop = ctx.pointer_hover_pos().and_then(|pointer| {
            panes.iter().find_map(|(target, rect)| {
                (*target != source && rect.contains(pointer)).then(|| {
                    let previous = previous
                        .filter(|drop| drop.target == *target)
                        .map(|drop| drop.zone);
                    let zone = stable_tab_drop_zone(*rect, pointer, previous);
                    let zone = if zone == DropZone::Center {
                        DropZone::Right
                    } else {
                        zone
                    };
                    TabDrop {
                        target: *target,
                        zone,
                        preview: tab_drop_preview(*rect, zone),
                    }
                })
            })
        });
        if ctx.input(|input| input.pointer.primary_released()) {
            self.agent_pane_drag = None;
            if let Some(drop) = self.agent_pane_drop.take() {
                self.move_agent_session_pane(source, drop.target, drop.zone);
            }
            true
        } else if !ctx.input(|input| input.pointer.primary_down()) {
            self.agent_pane_drag = None;
            self.agent_pane_drop = None;
            false
        } else {
            ctx.request_repaint();
            false
        }
    }

    fn draw_agent_session_picker(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        pane: PaneId,
        sessions: &[SessionChoice],
    ) {
        let header = assistant_sidebar_header(rect);
        paint_assistant_header_divider(ui.painter(), header);
        ui.painter().text(
            egui::pos2(header.left() + 14.0, header.center().y),
            Align2::LEFT_CENTER,
            "Open session",
            theme::typography::body(),
            theme::text().primary,
        );
        if self.agent_pane_layout.panes().len() > 1 {
            let (close, dragging) =
                draw_agent_pane_controls(ui, header, pane, "Open session", false);
            if close {
                self.agent_pane_close_requested = Some(pane);
            }
            if dragging {
                self.agent_pane_drag = Some(AgentPaneDrag {
                    pane,
                    title: "Open session".into(),
                });
            }
        }
        let body = egui::Rect::from_min_max(header.left_bottom(), rect.right_bottom());
        let width = body.width().min(420.0);
        let content = egui::Rect::from_min_max(
            egui::pos2(
                body.center().x - width * 0.5,
                body.top() + theme::space::XWIDE,
            ),
            egui::pos2(body.center().x + width * 0.5, body.bottom()),
        )
        .shrink2(egui::vec2(theme::space::WIDE, 0.0));
        let mut selection = None;
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("agent_pane_picker", pane.0))
                .max_rect(content)
                .layout(Layout::top_down(Align::LEFT)),
            |ui| {
                ui.set_width(content.width());
                ui.label(
                    RichText::new("SESSION PANE")
                        .font(theme::typography::micro())
                        .color(theme::text().muted),
                );
                ui.add_space(theme::space::SMALL);
                let (new_rect, _) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), theme::control::PRIMARY),
                    Sense::hover(),
                );
                let response = ui.interact(
                    new_rect,
                    Id::new(("agent_pane_new", pane.0)),
                    Sense::click(),
                );
                ui.painter().rect_filled(
                    new_rect,
                    theme::corner(theme::radius::CONTROL),
                    if response.hovered() {
                        theme::state::hover()
                    } else {
                        theme::surface().raised
                    },
                );
                icons::paint(
                    ui.painter(),
                    Icon::Plus,
                    egui::Rect::from_center_size(
                        egui::pos2(new_rect.left() + 18.0, new_rect.center().y),
                        egui::Vec2::splat(icons::GRID),
                    ),
                    theme::accent(),
                );
                ui.painter().text(
                    egui::pos2(new_rect.left() + 34.0, new_rect.center().y),
                    Align2::LEFT_CENTER,
                    "New session",
                    theme::typography::strong(),
                    theme::text().primary,
                );
                if response.clicked() {
                    selection = Some(None);
                }
                ui.add_space(theme::space::XWIDE);
                ui.label(
                    RichText::new("RECENT SESSIONS")
                        .font(theme::typography::micro())
                        .color(theme::text().muted),
                );
                ui.add_space(theme::space::SMALL);
                if sessions.is_empty() {
                    ui.label(RichText::new("No previous sessions").small().weak());
                }
                ScrollArea::vertical()
                    .id_salt(("agent_pane_sessions", pane.0))
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = theme::space::HAIR;
                        for session in sessions {
                            let label = session
                                .title
                                .as_deref()
                                .filter(|title| !title.trim().is_empty())
                                .unwrap_or("Untitled session");
                            let (row, _) = ui.allocate_exact_size(
                                egui::vec2(ui.available_width(), theme::control::ROW),
                                Sense::hover(),
                            );
                            let response = ui.interact(
                                row,
                                Id::new(("agent_pane_session", pane.0, &session.id)),
                                Sense::click(),
                            );
                            if response.hovered() {
                                ui.painter().rect_filled(
                                    row,
                                    theme::corner(theme::radius::CONTROL),
                                    theme::state::hover(),
                                );
                            }
                            ui.painter().text(
                                egui::pos2(row.left() + theme::space::SMALL, row.center().y),
                                Align2::LEFT_CENTER,
                                label,
                                theme::typography::body(),
                                if response.hovered() {
                                    theme::text().primary
                                } else {
                                    theme::text().secondary
                                },
                            );
                            if response.clicked() {
                                selection = Some(Some(session.id.clone()));
                            }
                        }
                    });
                ui.add_space(theme::space::LARGE);
                ui.label(
                    RichText::new("You can also drag a session here from the sidebar.")
                        .small()
                        .color(theme::text().muted),
                );
            },
        );
        if let Some(session_id) = selection {
            let fresh_session = session_id.is_none();
            self.assign_agent_session_pane(pane, session_id);
            self.start_provider(self.selected_provider, ui.ctx(), fresh_session);
        }
    }

    fn draw_agentic_sessions(&mut self, ui: &mut egui::Ui) {
        let rect = ui.max_rect();
        draw_assistant_sidebar_surface(ui, rect);
        let settings = sidebar_settings_rect(rect);
        let content = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 14.0, rect.top() + TITLEBAR_HEIGHT + 14.0),
            egui::pos2(rect.right() - 14.0, settings.top() - 14.0),
        );
        let mut session_load = None;
        let mut session_remove = None;
        let mut new_session = false;
        let mut add_project = false;
        let mut switch_to = None;
        let mut sessions_top = content.top();
        ui.scope_builder(
            UiBuilder::new()
                .id_salt("agentic_session_content")
                .max_rect(content)
                .layout(Layout::top_down(Align::LEFT)),
            |ui| {
                ui.set_width(content.width());
                if provider_selector_visible(&self.available_providers) {
                    let response =
                        draw_provider_selector_identity(ui, self.selected_provider, true, true);
                    self.provider_menu_anchor = Some(response.rect);
                    if response.clicked() {
                        let menu = AgentMenu::Providers;
                        self.agent_menu = (self.agent_menu.as_ref() != Some(&menu)).then_some(menu);
                    }
                } else {
                    draw_provider_identity(ui, self.selected_provider);
                    self.provider_menu_anchor = None;
                }
                ui.add_space(theme::space::SMALL);
                add_project = agentic_section_header(
                    ui,
                    "Workspaces",
                    Some(("agentic_add_project", "Add workspace")),
                );
                ui.spacing_mut().item_spacing.y = theme::space::HAIR;
                if agentic_project_row(ui, &self.tree.root, true) {
                    switch_to = Some(self.tree.root.clone());
                }
                for root in &self.recent_projects {
                    if root == &self.tree.root {
                        continue;
                    }
                    if agentic_project_row(ui, root, false) {
                        switch_to = Some(root.clone());
                    }
                }
                ui.add_space(theme::space::LARGE);
                new_session = agentic_section_header(
                    ui,
                    "Sessions",
                    self.agent
                        .session_ready
                        .then_some(("agentic_new_session", "New session")),
                );
                sessions_top = ui.cursor().top();
            },
        );
        if sessions_top < content.bottom() {
            let sessions = egui::Rect::from_min_max(
                egui::pos2(content.left(), sessions_top),
                egui::pos2(rect.right(), content.bottom()),
            );
            ui.scope_builder(
                UiBuilder::new()
                    .id_salt("agentic_session_list_region")
                    .max_rect(sessions)
                    .layout(Layout::top_down(Align::LEFT)),
                |ui| {
                    ScrollArea::vertical()
                        .id_salt("agentic_session_list")
                        .auto_shrink([false, false])
                        .scroll_source(
                            egui::scroll_area::ScrollSource::SCROLL_BAR
                                | egui::scroll_area::ScrollSource::MOUSE_WHEEL,
                        )
                        .content_margin(egui::Margin {
                            left: 0,
                            right: 14,
                            top: 0,
                            bottom: 0,
                        })
                        .show(ui, |ui| match &self.agent.sessions {
                            _ if !self.agent.history_available => {
                                ui.horizontal(|ui| {
                                    ui.add_space(theme::space::SMALL);
                                    ui.label(
                                        RichText::new("Session history unavailable").small().weak(),
                                    );
                                });
                            }
                            None => {
                                ui.horizontal(|ui| {
                                    ui.add_space(theme::space::SMALL);
                                    ui.label(RichText::new("Loading sessions…").small().weak());
                                });
                            }
                            Some(sessions) if sessions.is_empty() => {
                                ui.horizontal(|ui| {
                                    ui.add_space(theme::space::SMALL);
                                    ui.label(RichText::new("No previous sessions").small().weak());
                                });
                            }
                            Some(sessions) => {
                                ui.spacing_mut().item_spacing.y = theme::space::HAIR;
                                for session in sessions {
                                    let selected = self.agent.session_id.as_deref()
                                        == Some(session.id.as_str());
                                    let (open, remove, drag) = agent_session_row(
                                        ui,
                                        session,
                                        self.selected_provider,
                                        selected,
                                        true,
                                    );
                                    if open {
                                        session_load = Some(session.id.clone());
                                    }
                                    if remove {
                                        session_remove = Some(session.id.clone());
                                    }
                                    if drag {
                                        self.agent_session_drag = Some(session.clone());
                                    }
                                }
                            }
                        });
                },
            );
        }
        let (open_settings, update) = self.draw_settings_row(ui, settings);
        if open_settings {
            self.execute_keybinding(KeybindingCommand::AppOpenSettings, None, ui.ctx());
        }
        if update {
            self.start_update(ui.ctx());
        }
        if let Some(session_id) = session_load
            && let Some(controller) = self.agent_controllers.get(&self.selected_provider)
        {
            let _ = controller.send(AgentCommand::LoadSession(session_id));
        }
        if let Some(session_id) = session_remove
            && let Some(controller) = self.agent_controllers.get(&self.selected_provider)
        {
            let _ = controller.send(AgentCommand::RemoveSession(session_id));
        }
        if new_session && let Some(controller) = self.agent_controllers.get(&self.selected_provider)
        {
            let _ = controller.send(AgentCommand::NewSession);
        }
        if let Some(root) = switch_to {
            self.switch_project(root);
        } else if add_project {
            self.add_project_via_dialog();
        }
    }

    fn draw_agentic_titlebar(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        agent: egui::Rect,
        sessions: Option<egui::Rect>,
    ) {
        crate::renderer::mark_retained(
            ui.painter(),
            rect,
            TITLEBAR_PAINT_KEY,
            ui.ctx().cumulative_frame_nr(),
        );
        let agent_header = egui::Rect::from_min_max(
            egui::pos2(agent.left(), rect.top()),
            egui::pos2(agent.right(), rect.bottom()),
        );
        let file_tree_button = file_tree_toggle_rect(rect, agent_header);
        let terminal_button = terminal_toggle_rect(file_tree_button);
        let source_control_button = source_control_toggle_rect(terminal_button);
        let agentic_button = agentic_toggle_rect(
            source_control_button,
            sessions.map(|sessions| sessions.right()),
        );
        let sidebar_drag_rect = egui::Rect::from_min_max(
            egui::pos2(
                if cfg!(target_os = "macos") {
                    source_control_button.right()
                } else {
                    rect.left()
                },
                rect.top(),
            ),
            egui::pos2(agentic_button.left(), rect.bottom()),
        );
        if let Some(action) = titlebar_drag_action(ui, sidebar_drag_rect, "agentic_sidebar") {
            self.window_action = Some(action);
        }
        for (pane, pane_rect) in self
            .agent_pane_layout
            .rects(agent)
            .into_iter()
            .filter(|(_, pane)| (pane.top() - agent.top()).abs() <= 0.5)
        {
            let header = egui::Rect::from_min_max(
                egui::pos2(pane_rect.left(), rect.top()),
                egui::pos2(pane_rect.right(), rect.bottom()),
            );
            let drag_left = if (pane_rect.left() - agent.left()).abs() <= 0.5 {
                agentic_button.right().max(source_control_button.right()) + 4.0
            } else {
                header.left() + 4.0
            };
            let drag_rect = egui::Rect::from_min_max(
                egui::pos2(drag_left.min(header.right()), header.top()),
                egui::pos2(
                    if self.agent_pane_layout.panes().len() > 1 {
                        agent_pane_drag_rect(
                            header,
                            rect.right(),
                            self.agent_pane_picker != Some(pane),
                        )
                        .left()
                    } else {
                        agent_pane_button_rect(header, rect.right()).left()
                    },
                    header.bottom(),
                ),
            );
            if let Some(action) = titlebar_drag_action(ui, drag_rect, ("agentic", pane.0)) {
                self.window_action = Some(action);
            }
        }
        if self.draw_file_tree_toggle(ui, file_tree_button) {
            self.execute_keybinding(KeybindingCommand::ViewToggleExplorer, None, ui.ctx());
            self.sidebar_dragging = false;
        }
        if self.draw_terminal_toggle(ui, terminal_button) {
            self.execute_keybinding(KeybindingCommand::ViewToggleTerminal, None, ui.ctx());
        }
        if self.draw_source_control_toggle(ui, source_control_button) {
            self.execute_keybinding(KeybindingCommand::ViewToggleSourceControl, None, ui.ctx());
            self.sidebar_dragging = false;
        }
        if self.draw_agentic_toggle(ui, agentic_button) {
            self.execute_keybinding(KeybindingCommand::AppToggleAgenticView, None, ui.ctx());
        }

        #[cfg(target_os = "macos")]
        self.draw_macos_titlebar_controls(ui, rect);
        #[cfg(not(target_os = "macos"))]
        self.draw_windows_titlebar_controls(ui, rect);
    }

    fn draw_pane_header(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        pane: PaneId,
        tabs_left: f32,
        controls_right: f32,
        preview_path: Option<&Path>,
    ) -> (f32, f32) {
        if preview_path.is_some() || self.tabs.iter().any(|tab| tab.pane == pane) {
            ui.painter().rect_filled(rect, 0.0, theme::surface().chrome);
            ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 0.5,
                egui::Stroke::new(1.0, theme::border::hairline_color()),
            );
        } else {
            ui.painter().rect_filled(rect, 0.0, editor_background());
        }
        let active = self
            .pane_active_tabs
            .get(&pane)
            .and_then(|path| self.tabs.iter().position(|tab| &tab.buffer.path == path))
            .or_else(|| self.tabs.iter().position(|tab| tab.pane == pane));
        let diagnostic_counts = active
            .and_then(|index| self.lsp_diagnostics.get(&self.tabs[index].buffer.path))
            .map(|state| {
                state
                    .diagnostics
                    .iter()
                    .fold((0, 0), |(errors, warnings), diagnostic| {
                        match diagnostic.severity {
                            crate::lsp::DiagnosticSeverity::Error => (errors + 1, warnings),
                            crate::lsp::DiagnosticSeverity::Warning => (errors, warnings + 1),
                            _ => (errors, warnings),
                        }
                    })
            })
            .filter(|counts| *counts != (0, 0));
        // The pane header is the product's status surface, so a count reads as
        // what it costs: errors in danger, warnings in warning, never as gray text.
        let mut status_left = controls_right;
        let mut pill = |ui: &mut egui::Ui, text: String, color: Color32, hint: String, id: Id| {
            let width = ui
                .painter()
                .layout_no_wrap(text.clone(), theme::typography::micro(), color)
                .size()
                .x
                + theme::space::MEDIUM;
            let rect = egui::Rect::from_min_max(
                egui::pos2(
                    (status_left - width).max(tabs_left),
                    rect.top() + theme::space::TIGHT,
                ),
                egui::pos2(status_left, rect.bottom() - theme::space::TIGHT),
            );
            if rect.width() < width {
                return;
            }
            let tint = theme::callout(color);
            ui.painter()
                .rect_filled(rect, theme::corner(theme::radius::ROW), tint.fill);
            ui.painter().text(
                rect.center(),
                Align2::CENTER_CENTER,
                text,
                theme::typography::micro(),
                tint.text,
            );
            let response = ui.interact(rect, id, Sense::hover());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::WidgetType::Label, true, hint.clone())
            });
            status_left = rect.left() - theme::space::TIGHT;
        };
        if let Some((errors, warnings)) = diagnostic_counts {
            if warnings > 0 {
                pill(
                    ui,
                    format!("{warnings}"),
                    theme::semantic().warning,
                    format!("{warnings} warnings"),
                    Id::new(("pane_warnings", pane.0)),
                );
            }
            if errors > 0 {
                pill(
                    ui,
                    format!("{errors}"),
                    theme::semantic().danger,
                    format!("{errors} errors"),
                    Id::new(("pane_errors", pane.0)),
                );
            }
        }
        let controls_right = status_left;
        let markdown =
            active.is_some_and(|index| markdown::is_markdown(&self.tabs[index].buffer.path));
        let preview = active.is_some_and(|index| self.tabs[index].markdown_preview);
        let preview_button = markdown.then(|| {
            egui::Rect::from_min_max(
                egui::pos2((controls_right - 66.0).max(tabs_left), rect.top()),
                egui::pos2(controls_right, rect.bottom()),
            )
        });
        if let Some(button) = preview_button {
            let response = ui.interact(
                button,
                Id::new(("markdown_preview_toggle", pane.0)),
                Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    if preview {
                        "Edit Markdown"
                    } else {
                        "Preview Markdown"
                    },
                )
            });
            if response.hovered() || preview {
                ui.painter().rect_filled(
                    button.shrink2(egui::vec2(3.0, 3.0)),
                    4.0,
                    theme::state::selected(),
                );
            }
            ui.painter().text(
                button.center(),
                Align2::CENTER_CENTER,
                if preview { "Edit" } else { "Preview" },
                theme::typography::small(),
                theme::text().secondary,
            );
            if response.clicked()
                && let Some(index) = active
            {
                self.activate_tab(index);
                self.execute_keybinding(
                    KeybindingCommand::ViewToggleMarkdownPreview,
                    None,
                    ui.ctx(),
                );
                self.focus_editor = preview;
                ui.ctx().request_repaint();
            }
        }

        let controls_start = preview_button.map_or(controls_right, |button| button.left());
        let tabs_right = (controls_start - 8.0).max(tabs_left);
        let hidden_path = self
            .tab_drop
            .filter(|drop| drop.zone != DropZone::Center)
            .and(self.tab_drag.as_deref())
            .filter(|path| {
                self.tabs
                    .iter()
                    .any(|tab| tab.pane == pane && tab.buffer.path == **path)
            });
        let strip_width = match preview_path {
            Some(path) => tab_width(ui, &drag_label(path)),
            None => self
                .tabs
                .iter()
                .filter(|tab| tab.pane == pane && hidden_path != Some(tab.buffer.path.as_path()))
                .map(|tab| tab_width(ui, &drag_label(&tab.buffer.path)))
                .sum(),
        };
        let tabs_used_right = (tabs_left + strip_width).min(tabs_right);
        if let Some(path) = preview_path {
            let tab = egui::Rect::from_min_max(
                egui::pos2(tabs_left, rect.top()),
                egui::pos2(tabs_used_right, rect.bottom()),
            );
            ui.painter()
                .rect_filled(tab, 0.0, theme::surface().input.gamma_multiply(0.62));
            ui.painter().hline(
                tab.x_range(),
                tab.bottom() - 1.0,
                egui::Stroke::new(2.0, theme::accent().gamma_multiply(0.62)),
            );
            ui.painter().text(
                egui::pos2(tab.left() + 12.0, tab.center().y),
                Align2::LEFT_CENTER,
                drag_label(path),
                theme::typography::small(),
                theme::text().primary.gamma_multiply(0.62),
            );
        } else if tabs_used_right > tabs_left {
            self.draw_file_tabs(
                ui,
                egui::Rect::from_min_max(
                    egui::pos2(tabs_left, rect.top()),
                    egui::pos2(tabs_used_right, rect.bottom()),
                ),
                pane,
            );
        }
        (tabs_used_right, controls_start)
    }

    fn draw_titlebar(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        editor: egui::Rect,
        panes: &[(PaneId, egui::Rect)],
        assistant_sidebars: (bool, bool),
        dragged_pane: Option<PaneId>,
    ) {
        let (agent_sidebar_open, devin_sidebar_open) = assistant_sidebars;
        let dragged_path = self.tab_drag.clone();
        crate::renderer::mark_retained(
            ui.painter(),
            rect,
            TITLEBAR_PAINT_KEY,
            ui.ctx().cumulative_frame_nr(),
        );
        let editor_header =
            egui::Rect::from_min_max(egui::pos2(editor.left(), rect.top()), editor.right_top());
        let file_tree_button = file_tree_toggle_rect(rect, editor_header);
        let terminal_button = terminal_toggle_rect(file_tree_button);
        let source_control_button = source_control_toggle_rect(terminal_button);
        let agentic_button = agentic_toggle_rect(
            source_control_button,
            self.sidebar.then_some(editor_header.left()),
        );
        #[cfg(target_os = "macos")]
        let controls_left = editor_header.right();
        #[cfg(not(target_os = "macos"))]
        let controls_left = editor_header.right().min(rect.right() - 3.0 * 46.0);
        let agent_button = (!agent_sidebar_open).then(|| agent_toggle_rect(editor_header));
        let devin_button =
            (!devin_sidebar_open).then(|| devin_toggle_rect(editor_header, agent_sidebar_open));
        #[cfg(target_os = "macos")]
        let first_tabs_left = if self.sidebar {
            if agentic_button.right() > editor_header.left() {
                agentic_button.right() + 4.0
            } else {
                editor_header.left()
            }
        } else {
            agentic_button.right() + 4.0
        };
        #[cfg(not(target_os = "macos"))]
        let first_tabs_left = source_control_button.right().max(agentic_button.right()) + 4.0;
        for (pane, pane_rect) in panes
            .iter()
            .copied()
            .filter(|(_, pane)| (pane.top() - editor.top()).abs() <= 0.5)
        {
            let header = egui::Rect::from_min_max(
                egui::pos2(pane_rect.left(), rect.top()),
                egui::pos2(pane_rect.right(), rect.bottom()),
            );
            let tabs_left = if (pane_rect.left() - editor.left()).abs() <= 0.5 {
                first_tabs_left.max(header.left())
            } else {
                header.left()
            };
            let controls_right = if (pane_rect.right() - editor.right()).abs() <= 0.5 {
                [agent_button, devin_button]
                    .into_iter()
                    .flatten()
                    .map(|button| button.left())
                    .fold(controls_left, f32::min)
            } else {
                header.right()
            };
            let preview_path = (dragged_pane == Some(pane))
                .then_some(dragged_path.as_deref())
                .flatten();
            let (tabs_used_right, controls_start) =
                self.draw_pane_header(ui, header, pane, tabs_left, controls_right, preview_path);
            let drag_rect = egui::Rect::from_min_max(
                egui::pos2(tabs_used_right.min(controls_start), header.top()),
                egui::pos2(controls_start, header.bottom()),
            );
            if let Some(action) = titlebar_drag_action(ui, drag_rect, ("editor", pane.0)) {
                self.window_action = Some(action);
            }
        }
        #[cfg(target_os = "macos")]
        let sidebar_drag_left = source_control_button.right();
        #[cfg(not(target_os = "macos"))]
        let sidebar_drag_left = rect.left();
        let sidebar_drag_right = (agentic_button.left() - 3.0).max(sidebar_drag_left);
        let sidebar_drag_rect = egui::Rect::from_min_max(
            egui::pos2(sidebar_drag_left, rect.top()),
            egui::pos2(sidebar_drag_right, rect.bottom()),
        );
        let agent_drag_rect = if agent_sidebar_open {
            let agent_header =
                egui::Rect::from_min_max(editor_header.right_top(), rect.right_bottom());
            let title = self.agent.title.as_deref().unwrap_or("Agent");
            let title_x = if provider_selector_visible(&self.available_providers) {
                agent_header.left() + 64.0
            } else {
                agent_header.left() + 14.0
            };
            let drag_left = if self.agent.session_ready && self.agent.history_available {
                agent_session_selector_rect(ui, agent_header, title_x, title).right() + 4.0
            } else {
                self.provider_menu_anchor
                    .map_or(agent_header.left() + 3.0, |anchor| anchor.right() + 4.0)
            };
            egui::Rect::from_min_max(
                egui::pos2(drag_left, agent_header.top()),
                egui::pos2(
                    agent_new_session_rect(agent_header).left(),
                    agent_header.bottom(),
                ),
            )
        } else {
            egui::Rect::NOTHING
        };
        for (region, drag_rect) in [("sidebar", sidebar_drag_rect), ("agent", agent_drag_rect)] {
            if let Some(action) = titlebar_drag_action(ui, drag_rect, region) {
                self.window_action = Some(action);
            }
        }

        if agent_button.is_some_and(|button| self.draw_agent_toggle(ui, button)) {
            self.execute_keybinding(KeybindingCommand::AppToggleAgentSidebar, None, ui.ctx());
            ui.ctx().request_repaint();
        }
        if devin_button.is_some_and(|button| self.draw_devin_toggle(ui, button)) {
            self.execute_keybinding(KeybindingCommand::AppToggleDevinSidebar, None, ui.ctx());
            ui.ctx().request_repaint();
        }
        if self.draw_file_tree_toggle(ui, file_tree_button) {
            self.execute_keybinding(KeybindingCommand::ViewToggleExplorer, None, ui.ctx());
            self.sidebar_dragging = false;
            ui.ctx().request_repaint();
        }
        if self.draw_terminal_toggle(ui, terminal_button) {
            self.execute_keybinding(KeybindingCommand::ViewToggleTerminal, None, ui.ctx());
        }
        if self.draw_source_control_toggle(ui, source_control_button) {
            self.execute_keybinding(KeybindingCommand::ViewToggleSourceControl, None, ui.ctx());
            self.sidebar_dragging = false;
            ui.ctx().request_repaint();
        }
        if self.draw_agentic_toggle(ui, agentic_button) {
            self.execute_keybinding(KeybindingCommand::AppToggleAgenticView, None, ui.ctx());
        }

        #[cfg(target_os = "macos")]
        self.draw_macos_titlebar_controls(ui, rect);
        #[cfg(not(target_os = "macos"))]
        self.draw_windows_titlebar_controls(ui, rect);
    }

    fn update_tab_drag(&mut self, ctx: &egui::Context, panes: &[(PaneId, egui::Rect)]) -> bool {
        let Some(path) = self.tab_drag.clone() else {
            self.tab_drop = None;
            return false;
        };
        let index = self.tabs.iter().position(|tab| tab.buffer.path == path);
        let pointer = ctx.pointer_hover_pos();
        let previous = self.tab_drop;
        self.tab_drop = pointer.and_then(|pointer| {
            panes.iter().find_map(|(target, rect)| {
                rect.contains(pointer).then(|| {
                    let previous = previous
                        .filter(|drop| drop.target == *target)
                        .map(|drop| drop.zone);
                    let zone = if pointer.y <= rect.top() + PANE_TAB_HEIGHT
                        && previous.unwrap_or(DropZone::Center) == DropZone::Center
                    {
                        DropZone::Center
                    } else {
                        stable_tab_drop_zone(*rect, pointer, previous)
                    };
                    TabDrop {
                        target: *target,
                        zone,
                        preview: tab_drop_preview(*rect, zone),
                    }
                })
            })
        });
        if index.is_some_and(|index| {
            self.tab_drop.is_some_and(|drop| {
                drop.zone != DropZone::Center
                    && drop.target == self.tabs[index].pane
                    && self
                        .tabs
                        .iter()
                        .filter(|tab| tab.pane == drop.target)
                        .count()
                        == 1
            })
        }) {
            self.tab_drop = None;
        }
        let released = ctx.input(|input| input.pointer.primary_released());
        let down = ctx.input(|input| input.pointer.primary_down());
        if released {
            let drop = self.tab_drop.take();
            self.tab_drag = None;
            if let Some(drop) = drop {
                self.drop_path(path, drop.target, drop.zone);
                return true;
            }
        } else if !down {
            self.tab_drag = None;
            self.tab_drop = None;
        } else {
            ctx.request_repaint();
        }
        false
    }

    fn tab_drag_preview(
        &self,
        available: egui::Rect,
        path: &Path,
        drop: TabDrop,
    ) -> Option<(Vec<(PaneId, egui::Rect)>, PaneId)> {
        if drop.zone == DropZone::Center {
            return None;
        }
        let source = self
            .tabs
            .iter()
            .find(|tab| tab.buffer.path == path)
            .map(|tab| tab.pane);
        if source == Some(drop.target)
            && self
                .tabs
                .iter()
                .filter(|tab| tab.pane == drop.target)
                .count()
                == 1
        {
            return None;
        }
        let mut layout = self.pane_layout.clone();
        let preview = layout.split(drop.target, drop.zone)?;
        if let Some(source) = source
            && self.tabs.iter().filter(|tab| tab.pane == source).count() == 1
        {
            layout.remove(source);
        }
        Some((layout.rects(available), preview))
    }

    fn draw_editor_pane(
        &mut self,
        ui: &mut egui::Ui,
        pane: PaneId,
        editor: egui::Rect,
        single_pane: bool,
        path_override: Option<&Path>,
        preview: bool,
    ) {
        let rect = ui.max_rect();
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("editor_surface", pane.0))
                .max_rect(rect),
            |ui| {
                if preview {
                    ui.multiply_opacity(0.62);
                }
                self.draw_editor(ui, pane, single_pane, path_override, preview);
            },
        );
        if rect.right() + 0.5 < editor.right() {
            ui.painter().vline(
                rect.right() - 0.5,
                rect.y_range(),
                egui::Stroke::new(1.0, theme::border::strong_color()),
            );
        }
        if rect.bottom() + 0.5 < editor.bottom() {
            ui.painter().hline(
                rect.x_range(),
                rect.bottom() - 0.5,
                egui::Stroke::new(1.0, theme::border::strong_color()),
            );
        }
    }

    fn draw_file_tabs(&mut self, ui: &mut egui::Ui, rect: egui::Rect, pane: PaneId) {
        let hidden_path = self
            .tab_drop
            .filter(|drop| drop.zone != DropZone::Center)
            .and(self.tab_drag.as_deref())
            .filter(|path| {
                self.tabs
                    .iter()
                    .any(|tab| tab.pane == pane && tab.buffer.path == **path)
            });
        let active = self
            .pane_active_tabs
            .get(&pane)
            .filter(|path| hidden_path != Some(path.as_path()))
            .and_then(|path| self.tabs.iter().position(|tab| &tab.buffer.path == path))
            .or_else(|| {
                self.tabs.iter().position(|tab| {
                    tab.pane == pane && hidden_path != Some(tab.buffer.path.as_path())
                })
            });
        let tabs = self
            .tabs
            .iter()
            .enumerate()
            .filter(|(_, tab)| tab.pane == pane && hidden_path != Some(tab.buffer.path.as_path()))
            .map(|(index, tab)| {
                let path = &tab.buffer.path;
                let label = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned();
                (
                    index,
                    label,
                    path.clone(),
                    path.display().to_string(),
                    tab.buffer.dirty,
                )
            })
            .collect::<Vec<_>>();
        let mut activate = None;
        let mut close = None;
        let mut reorder = None;
        ui.scope_builder(
            UiBuilder::new()
                .id_salt(("file_tabs", pane.0))
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.set_clip_rect(rect);
                ScrollArea::horizontal()
                    .id_salt(("file_tabs_scroll", pane.0))
                    .max_width(rect.width())
                    .max_height(rect.height())
                    .auto_shrink([false, false])
                    .content_margin(egui::Margin::ZERO)
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                        let widths = tabs
                            .iter()
                            .map(|(_, label, ..)| tab_width(ui, label))
                            .collect::<Vec<_>>();
                        for (position, (index, label, path, path_display, dirty)) in
                            tabs.iter().enumerate()
                        {
                            let (_, tab) =
                                ui.allocate_space(egui::vec2(widths[position], rect.height()));
                            let selected = active == Some(*index);
                            let response = ui.interact(
                                tab,
                                Id::new(("file_tab", path_display)),
                                Sense::click_and_drag(),
                            );
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
                                        theme::state::selected()
                                    } else if selected {
                                        // The active tab is the top edge of the
                                        // document, so it wears the document's fill.
                                        theme::surface().editor
                                    } else {
                                        theme::state::hover()
                                    },
                                );
                            }
                            // A divider only earns its place between two inactive
                            // tabs; beside the active one it competes with the bar.
                            let touches_active = selected
                                || active == Some(tabs[(position + 1).min(tabs.len() - 1)].0);
                            if !touches_active && position + 1 < tabs.len() {
                                ui.painter().vline(
                                    tab.right() - 0.5,
                                    tab.y_range().shrink(theme::space::SNUG),
                                    theme::border::hairline(),
                                );
                            }
                            let close_rect = egui::Rect::from_center_size(
                                egui::pos2(tab.right() - theme::space::LARGE, tab.center().y),
                                egui::Vec2::splat(TAB_CLOSE),
                            );
                            let text_rect = egui::Rect::from_min_max(
                                egui::pos2(tab.left() + theme::space::MEDIUM + TAB_DOT, tab.top()),
                                egui::pos2(close_rect.left() - theme::space::SMALL, tab.bottom()),
                            );
                            let text_color = if selected {
                                theme::text().primary
                            } else {
                                theme::text().muted
                            };
                            let galley = egui::WidgetText::from(
                                RichText::new(label)
                                    .font(if selected {
                                        theme::typography::strong()
                                    } else {
                                        theme::typography::small()
                                    })
                                    .color(text_color),
                            )
                            .into_galley(
                                ui,
                                Some(egui::TextWrapMode::Truncate),
                                text_rect.width(),
                                egui::FontSelection::Default,
                            );
                            let text_position = egui::pos2(
                                text_rect.left(),
                                text_rect.center().y - galley.size().y * 0.5,
                            );
                            ui.painter().galley(text_position, galley, text_color);
                            if *dirty {
                                ui.painter().circle_filled(
                                    egui::pos2(
                                        tab.left() + theme::space::MEDIUM + TAB_DOT * 0.5,
                                        tab.center().y,
                                    ),
                                    3.0,
                                    theme::ink(theme::semantic().warning),
                                );
                            }
                            let show_close = hovered || selected;
                            let close_response = ui
                                .interact(
                                    close_rect,
                                    Id::new(("file_tab_close", path_display)),
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
                                icons::paint_button(
                                    ui.painter(),
                                    Icon::Close,
                                    close_rect,
                                    &close_response,
                                    ui.is_enabled(),
                                    theme::text().secondary,
                                );
                            }
                            if close_response.clicked() {
                                close = Some(*index);
                            } else if response.clicked() {
                                activate = Some(*index);
                            }
                            if response.drag_started() {
                                self.tab_drag = Some(path.clone());
                                activate = Some(*index);
                            }
                            if dragging {
                                self.tab_drag = Some(path.clone());
                                ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
                                if let Some(pointer) =
                                    ui.input(|input| input.pointer.interact_pos())
                                    && rect.contains(pointer)
                                {
                                    let first_left =
                                        tab.left() - widths[..position].iter().sum::<f32>();
                                    let mut edge = first_left;
                                    let mut target_position = tabs.len() - 1;
                                    for (candidate, width) in widths.iter().enumerate() {
                                        edge += width;
                                        if pointer.x < edge {
                                            target_position = candidate;
                                            break;
                                        }
                                    }
                                    let target = tabs[target_position].0;
                                    if target != *index {
                                        reorder = Some((*index, target));
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
            "Close Agent"
        } else {
            "Open Agent"
        };
        let response = ui
            .interact(button, Id::new("agent_sidebar_toggle"), Sense::click())
            .on_hover_text(label);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        let color = if response.hovered() || self.agent_sidebar {
            theme::text().primary
        } else {
            theme::text().muted
        };
        icons::paint(
            ui.painter(),
            if self.agent_sidebar {
                Icon::Close
            } else {
                Icon::Robot
            },
            egui::Rect::from_center_size(button.center(), egui::Vec2::splat(icons::GRID)),
            color,
        );
        if self.agent.waiting_permission() {
            ui.painter().circle_filled(
                button.center() + egui::vec2(8.0, -7.0),
                3.0,
                theme::ink(theme::semantic().warning),
            );
        }
        response.clicked()
    }

    fn draw_devin_toggle(&self, ui: &mut egui::Ui, button: egui::Rect) -> bool {
        let label = if self.devin_sidebar {
            "Close Devin"
        } else {
            "Open Devin"
        };
        let response = ui
            .interact(button, Id::new("devin_sidebar_toggle"), Sense::click())
            .on_hover_text(label);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        devin_view::paint_devin_icon(
            ui.painter(),
            egui::Rect::from_center_size(button.center(), egui::Vec2::splat(icons::GRID)),
            if response.hovered() || self.devin_sidebar {
                theme::accent()
            } else {
                theme::text().muted
            },
        );
        response.clicked()
    }

    fn draw_file_tree_toggle(&self, ui: &mut egui::Ui, button: egui::Rect) -> bool {
        let active = self.sidebar && self.sidebar_pane == SidebarPane::Files;
        let label = if active {
            "Hide File Tree"
        } else {
            "Show File Tree"
        };
        let response = ui
            .interact(button, Id::new("file_tree_toggle"), Sense::click())
            .on_hover_text(label);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        draw_sidebar_toggle_icon(ui, button, &response, active);
        response.clicked()
    }

    fn draw_terminal_toggle(&self, ui: &mut egui::Ui, button: egui::Rect) -> bool {
        let label = if self.terminal_open {
            "Hide Terminal"
        } else {
            "Show Terminal"
        };
        let response = ui
            .interact(button, Id::new("terminal_toggle"), Sense::click())
            .on_hover_text(label);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        let color = if response.hovered() || self.terminal_open {
            theme::text().primary
        } else {
            theme::text().muted
        };
        icons::paint(
            ui.painter(),
            Icon::Terminal,
            egui::Rect::from_center_size(button.center(), egui::Vec2::splat(icons::GRID)),
            color,
        );
        response.clicked()
    }

    fn draw_source_control_toggle(&self, ui: &mut egui::Ui, button: egui::Rect) -> bool {
        let active = self.sidebar && self.sidebar_pane == SidebarPane::SourceControl;
        let label = if active {
            "Hide Source Control"
        } else {
            "Show Source Control"
        };
        let response = ui
            .interact(button, Id::new("source_control_toggle"), Sense::click())
            .on_hover_text(label);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
        });
        icons::paint(
            ui.painter(),
            Icon::SourceControl,
            egui::Rect::from_center_size(button.center(), egui::Vec2::splat(icons::GRID)),
            if response.hovered() || active {
                theme::text().primary
            } else {
                theme::text().muted
            },
        );
        response.clicked()
    }

    fn toggle_terminal(&mut self, ctx: &egui::Context) {
        if self.terminal_open {
            self.terminal.blur(ctx);
            self.terminal_open = false;
            self.terminal_dragging = false;
        } else {
            let root = self.tree.root.clone();
            match self.terminal.open(&root, ctx) {
                Ok(()) => self.terminal_open = true,
                Err(error) => self.show_error(error),
            }
        }
        ctx.request_repaint();
    }

    fn update_terminal_resize(&mut self, ctx: &egui::Context, bounds: egui::Rect) {
        if !self.terminal_open {
            self.terminal_dragging = false;
            return;
        }
        let (_, Some(terminal)) = split_bottom_panel(bounds, true, self.terminal_height) else {
            return;
        };
        let divider = egui::Rect::from_center_size(
            egui::pos2(terminal.center().x, terminal.top()),
            egui::vec2(terminal.width(), 7.0),
        );
        let pointer = ctx.pointer_hover_pos();
        let (pressed, down) = ctx.input(|input| {
            (
                input.pointer.primary_pressed(),
                input.pointer.primary_down(),
            )
        });
        if pressed && pointer.is_some_and(|pointer| divider.contains(pointer)) {
            self.terminal_dragging = true;
        }
        if !down {
            self.terminal_dragging = false;
        } else if self.terminal_dragging
            && let Some(pointer) = pointer
        {
            let max_height = (bounds.height() - WORKSPACE_MIN_HEIGHT).max(0.0);
            let min_height = TERMINAL_MIN_HEIGHT.min(max_height);
            self.terminal_height = (bounds.bottom() - pointer.y).clamp(min_height, max_height);
        }
    }

    fn draw_terminal_resize(&mut self, ui: &mut egui::Ui, terminal: egui::Rect) {
        let divider = egui::Rect::from_center_size(
            egui::pos2(terminal.center().x, terminal.top()),
            egui::vec2(terminal.width(), 7.0),
        );
        let pointer = ui.ctx().pointer_hover_pos();
        let hovered = pointer.is_some_and(|pointer| divider.contains(pointer));
        let active = hovered || self.terminal_dragging;
        if active {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        ui.painter().hline(
            terminal.x_range(),
            terminal.top(),
            resize_divider_stroke(ui.ctx(), active),
        );
    }

    fn draw_agentic_toggle(&self, ui: &mut egui::Ui, button: egui::Rect) -> bool {
        let (label, action) = if self.agentic_mode {
            ("IDE", "Switch to IDE")
        } else {
            ("Agent", "Switch to Agent")
        };
        let response = ui.interact(button, Id::new("agentic_mode_toggle"), Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), action)
        });
        let color = if response.hovered() {
            theme::text().primary
        } else {
            theme::text().secondary
        };
        ui.painter().text(
            egui::pos2(button.right() - 12.0, button.center().y),
            Align2::RIGHT_CENTER,
            label,
            theme::typography::small(),
            color,
        );
        response.clicked()
    }

    fn set_agentic_mode(&mut self, enabled: bool, ctx: &egui::Context) {
        self.agentic_mode = enabled;
        self.agent_menu = None;
        self.agent_menu_popup = None;
        self.attachment_file_picker = None;
        if !enabled {
            self.agent_find.open = false;
            self.agent_find.focus = false;
        }
        if enabled {
            if self.devin_sidebar {
                self.devin_sidebar = false;
                if let Some(controller) = self.devin_controller.as_ref() {
                    let _ = controller.send(DevinCommand::SetVisible(false));
                }
            }
            if let Some(controller) = self.agent_controllers.get(&self.selected_provider) {
                let _ = controller.send(AgentCommand::RefreshSessions);
            } else {
                self.open_agent(ctx);
            }
        }
        ctx.request_repaint();
    }

    #[cfg(target_os = "macos")]
    fn draw_macos_titlebar_controls(&mut self, ui: &mut egui::Ui, rect: egui::Rect) {
        if let Some(action) = macos_titlebar_controls(ui, rect, "editor") {
            self.window_action = Some(action);
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
                        theme::semantic().danger
                    } else {
                        theme::border::hairline_color()
                    },
                );
            }
            let center = button.center();
            let color = theme::text().secondary;
            icons::paint(
                ui.painter(),
                match index {
                    0 => Icon::Minus,
                    1 => Icon::Square,
                    _ => Icon::Close,
                },
                egui::Rect::from_center_size(center, egui::Vec2::splat(icons::GRID * 0.75)),
                color,
            );
            if response.clicked() {
                self.window_action = Some(action);
            }
        }
    }

    /// Errors that need a decision are dialogs; these need only to be seen.
    fn draw_error(&mut self, ctx: &egui::Context) {
        if let Some(error) = self.error.take() {
            self.toasts.push(Severity::Danger, error);
        }
        self.toasts.show(ctx);
    }

    fn take_window_action(&mut self) -> Option<WindowAction> {
        self.window_action.take()
    }

    fn draw_assistant_image_lightbox(&mut self, ctx: &egui::Context) {
        let Some(source) = self.assistant_image_lightbox.clone() else {
            return;
        };
        let cache_id = match &source {
            AssistantImageSource::Bytes(data) => Id::new((
                "agent_image_lightbox_bytes",
                data.as_ptr() as usize,
                data.len(),
            )),
            AssistantImageSource::Path(path) => Id::new(("agent_image_lightbox_path", path)),
        };
        let cached = ctx.data(|data| data.get_temp::<AssistantImagePreview>(cache_id));
        let preview = cached.unwrap_or_else(|| {
            let loaded = match &source {
                AssistantImageSource::Bytes(data) => load_assistant_image_bytes(
                    ctx,
                    &format!(
                        "agent_image_lightbox_{:x}_{}",
                        data.as_ptr() as usize,
                        data.len()
                    ),
                    data,
                    ASSISTANT_IMAGE_LIGHTBOX_EDGE,
                ),
                AssistantImageSource::Path(path) => {
                    load_assistant_image_path(ctx, path, ASSISTANT_IMAGE_LIGHTBOX_EDGE)
                }
            };
            let preview = loaded.map_or(
                AssistantImagePreview::Unavailable,
                AssistantImagePreview::Loaded,
            );
            ctx.data_mut(|data| data.insert_temp(cache_id, preview.clone()));
            preview
        });
        let AssistantImagePreview::Loaded(texture) = preview else {
            self.assistant_image_lightbox = None;
            return;
        };

        let screen = ctx.content_rect();
        let size = egui::vec2(
            (screen.width() * 0.9).min(1_600.0),
            (screen.height() * 0.9).min(1_000.0),
        );
        let mut close = ctx.input(|input| input.key_pressed(Key::Escape));
        let frame = egui::Frame::new()
            .fill(theme::surface().raised)
            .stroke(theme::border::strong())
            .corner_radius(theme::corner(theme::radius::DIALOG))
            .shadow(theme::shadow::dialog());
        let modal = egui::Modal::new(Id::new("assistant_image_lightbox"))
            .backdrop_color(theme::state::scrim())
            .frame(frame)
            .show(ctx, |ui| {
                let (_, full) = ui.allocate_space(size);
                let header = egui::Rect::from_min_size(full.min, egui::vec2(full.width(), 44.0));
                ui.painter()
                    .hline(header.x_range(), header.bottom(), theme::border::hairline());
                ui.painter().text(
                    egui::pos2(header.left() + theme::space::LARGE, header.center().y),
                    Align2::LEFT_CENTER,
                    "Image preview",
                    theme::typography::strong(),
                    theme::text().primary,
                );
                let close_rect = egui::Rect::from_center_size(
                    egui::pos2(header.right() - 22.0, header.center().y),
                    egui::Vec2::splat(40.0),
                );
                let close_response = ui
                    .interact(
                        close_rect,
                        Id::new("agent_image_lightbox_close"),
                        Sense::click(),
                    )
                    .on_hover_text("Close (Esc)");
                close_response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Close image preview")
                });
                if close_response.hovered() {
                    ui.painter().rect_filled(
                        close_rect,
                        theme::corner(theme::radius::CONTROL),
                        theme::state::hover(),
                    );
                }
                icons::paint(
                    ui.painter(),
                    Icon::Close,
                    egui::Rect::from_center_size(
                        close_rect.center(),
                        egui::Vec2::splat(icons::GRID * 0.85),
                    ),
                    if close_response.hovered() {
                        theme::text().primary
                    } else {
                        theme::text().muted
                    },
                );
                close |= close_response.clicked();

                let body = egui::Rect::from_min_max(
                    header.left_bottom() + egui::vec2(theme::space::LARGE, theme::space::LARGE),
                    full.right_bottom() - egui::vec2(theme::space::LARGE, theme::space::LARGE),
                );
                let image_size = texture.size_vec2();
                let scale = (body.width() / image_size.x).min(body.height() / image_size.y);
                let image_rect = egui::Rect::from_center_size(body.center(), image_size * scale);
                ui.painter().image(
                    texture.id(),
                    image_rect,
                    egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                    Color32::WHITE,
                );
            });
        close |= modal.backdrop_response.clicked();
        if close {
            self.assistant_image_lightbox = None;
        }
    }

    fn draw_dialogs(&mut self, ctx: &egui::Context) {
        self.draw_assistant_image_lightbox(ctx);
        self.draw_attachment_file_picker(ctx);
        self.draw_project_folder_picker(ctx);
        self.draw_git_discard_dialog(ctx);
        if self.devin_confirm_terminate {
            let title = self
                .devin_state
                .detail
                .as_ref()
                .map_or("Terminate Devin session", |detail| {
                    detail.summary.title.as_str()
                });
            let outcome = Dialog::new("devin_terminate_dialog", title)
                .severity(Severity::Danger)
                .body("Terminate this remote Devin session? Its work stops permanently and cannot be resumed. Closing or hiding the sidebar does not stop it.")
                .destructive("Terminate session")
                .show(ctx);
            match outcome {
                Outcome::Destructive => {
                    self.devin_confirm_terminate = false;
                    self.devin_pending_lifecycle = Some(DevinLifecycle::Terminate);
                    self.devin_state.busy = true;
                    self.send_devin(DevinCommand::TerminateConfirmed);
                }
                Outcome::Cancel | Outcome::Dismissed => self.devin_confirm_terminate = false,
                _ => {}
            }
        }
        if self.devin_confirm_disconnect {
            let outcome = Dialog::new("devin_disconnect_dialog", "Disconnect Devin")
                .body("Remove the stored Devin token from this machine? Remote sessions keep running.")
                .primary("Remove token")
                .show(ctx);
            match outcome {
                Outcome::Primary => {
                    self.devin_confirm_disconnect = false;
                    self.send_devin(DevinCommand::Disconnect);
                }
                Outcome::Cancel | Outcome::Dismissed => self.devin_confirm_disconnect = false,
                _ => {}
            }
        }
        if let Some(org_id) = self.devin_confirm_org.clone() {
            let outcome = Dialog::new("devin_org_switch_dialog", "Switch Devin organization")
                .body("Switch organizations? Remote sessions and resource caches will be cleared. Unsent local drafts and staged files stay in Editur.")
                .primary("Switch organization")
                .show(ctx);
            match outcome {
                Outcome::Primary => {
                    self.devin_confirm_org = None;
                    self.send_devin(DevinCommand::SelectOrganization(org_id));
                }
                Outcome::Cancel | Outcome::Dismissed => self.devin_confirm_org = None,
                _ => {}
            }
        }
        if let Some(mutation) = self.devin_confirm_mutation.clone() {
            let destructive = matches!(
                mutation,
                ResourceMutation::RemoveRepositoryIndex { .. }
                    | ResourceMutation::RemoveRepositoryBranch { .. }
                    | ResourceMutation::Knowledge {
                        action: CrudAction::Delete,
                        ..
                    }
                    | ResourceMutation::DismissKnowledgeSuggestion { .. }
                    | ResourceMutation::Playbook {
                        action: CrudAction::Delete,
                        ..
                    }
                    | ResourceMutation::Schedule {
                        action: CrudAction::Delete,
                        ..
                    }
                    | ResourceMutation::Automation {
                        action: CrudAction::Delete,
                        ..
                    }
                    | ResourceMutation::Blueprint {
                        action: CrudAction::Delete,
                        ..
                    }
                    | ResourceMutation::DeleteBlueprintFile { .. }
                    | ResourceMutation::CancelBuild { .. }
                    | ResourceMutation::DeleteSecret { .. }
            );
            let (title, body, button) = match &mutation {
                ResourceMutation::TriggerReview { .. } => (
                    "Trigger Devin Review",
                    "Start a remote review? This can incur Devin usage.",
                    "Trigger review",
                ),
                ResourceMutation::TriggerBuild => (
                    "Trigger snapshot build",
                    "Start a remote snapshot build using the current blueprints?",
                    "Trigger build",
                ),
                _ => (
                    "Confirm Devin change",
                    "Apply this remote organization change? Deleted resources may not be recoverable.",
                    "Apply change",
                ),
            };
            let dialog = Dialog::new("devin_resource_confirmation", title).body(body);
            let outcome = if destructive {
                dialog
                    .severity(Severity::Danger)
                    .destructive(button)
                    .show(ctx)
            } else {
                dialog.primary(button).show(ctx)
            };
            match outcome {
                Outcome::Primary | Outcome::Destructive => {
                    self.devin_confirm_mutation = None;
                    self.send_devin(DevinCommand::MutateResource(mutation));
                }
                Outcome::Cancel | Outcome::Dismissed => self.devin_confirm_mutation = None,
                _ => {}
            }
        }
        if self.tree_prompt.is_some() {
            let action = self.tree_prompt.as_ref().map(|prompt| prompt.action);
            let title = match action {
                Some(TreePromptAction::NewFile) => "New File",
                Some(TreePromptAction::NewFolder) => "New Folder",
                Some(TreePromptAction::Rename) => "Rename",
                None => "File Operation",
            };
            let mut submitted_by_return = false;
            let empty = self
                .tree_prompt
                .as_ref()
                .is_some_and(|prompt| prompt.name.trim().is_empty());
            let outcome = Dialog::new("tree_name_dialog", title)
                .primary(title)
                .primary_enabled(!empty)
                .show_with(ctx, |ui| {
                    ui.add_space(theme::space::TIGHT);
                    ui.label(
                        RichText::new("Name")
                            .font(theme::typography::small())
                            .color(theme::text().muted),
                    );
                    if let Some(prompt) = self.tree_prompt.as_mut() {
                        let response = ui.add(
                            TextEdit::singleline(&mut prompt.name)
                                .id(Id::new("tree_name_input"))
                                .desired_width(ui.available_width()),
                        );
                        if std::mem::take(&mut prompt.focus) {
                            response.request_focus();
                        }
                        submitted_by_return = response.lost_focus()
                            && ui.input(|input| input.key_pressed(Key::Enter));
                    }
                });
            if (outcome == Outcome::Primary || submitted_by_return) && !empty {
                match self.finish_tree_prompt() {
                    Ok((path, TreePromptAction::NewFile)) => {
                        self.refresh_tree(Some(path.clone()));
                        self.request(PendingAction::Open(path));
                    }
                    Ok((path, _)) => self.refresh_tree(Some(path)),
                    Err(error) => self.show_error(error),
                }
            } else if matches!(outcome, Outcome::Cancel | Outcome::Dismissed) {
                self.tree_prompt = None;
            }
        }
        if let Some(path) = self.tree_delete.clone() {
            let outcome = Dialog::new("tree_delete_dialog", "Delete")
                .severity(Severity::Danger)
                .body("This cannot be undone.")
                .path(&path)
                .destructive("Delete")
                .show(ctx);
            match outcome {
                Outcome::Destructive => {
                    self.tree_delete = None;
                    if let Err(error) = self.delete_tree_entry(&path) {
                        self.show_error(error);
                    }
                }
                Outcome::Cancel | Outcome::Dismissed => self.tree_delete = None,
                _ => {}
            }
        }
        if self.pending_agent_prompt {
            let provider = provider_descriptor(self.selected_provider).display_name;
            let buffer = self.buffer().map(|buffer| buffer.path.clone());
            let title = format!("Save before running {provider} Agent");
            let mut dialog = Dialog::new("agent_save_dialog", &title)
                .severity(Severity::Info)
                .body("The agent reads files from disk, so unsaved edits are invisible to it.")
                .primary("Save and Run");
            if let Some(path) = buffer.as_deref() {
                dialog = dialog.path(path);
            }
            match dialog.show(ctx) {
                Outcome::Primary => {
                    if self.save(None) {
                        self.pending_agent_prompt = false;
                        self.send_agent_prompt();
                    }
                }
                Outcome::Cancel | Outcome::Dismissed => self.pending_agent_prompt = false,
                _ => {}
            }
        }
        if self.pending.is_some() && !self.conflict && self.save_as.is_none() {
            let path = self.buffer().map(|buffer| buffer.path.clone());
            let mut dialog = Dialog::new("unsaved_dialog", "Unsaved changes")
                .severity(Severity::Warning)
                .body("Save your changes before continuing?")
                .destructive("Discard")
                .primary("Save");
            if let Some(path) = path.as_deref() {
                dialog = dialog.path(path);
            }
            match dialog.show(ctx) {
                Outcome::Primary => {
                    if self.save(None) {
                        self.finish_pending();
                    }
                }
                Outcome::Destructive => self.discard_pending(),
                Outcome::Cancel | Outcome::Dismissed => {
                    self.pending = None;
                    self.tree
                        .select(self.buffer().map(|buffer| buffer.path.clone()));
                }
                _ => {}
            }
        }
        if self.conflict {
            let path = self.buffer().map(|buffer| buffer.path.clone());
            let mut dialog = Dialog::new("conflict_dialog", "File changed on disk")
                .severity(Severity::Warning)
                .body("The file changed outside Editur and was not overwritten. Reloading discards the edits in this buffer.")
                .neutral("Save As…")
                .primary("Reload");
            if let Some(path) = path.as_deref() {
                dialog = dialog.path(path);
            }
            match dialog.show(ctx) {
                Outcome::Primary => {
                    if let Some(index) = self.active_tab {
                        let path = self.tabs[index].buffer.path.clone();
                        match load_buffer(&path) {
                            Ok(buffer) => {
                                let pane = self.tabs[index].pane;
                                self.tabs[index] = FileTab::new(buffer, pane);
                                self.conflict = false;
                                if self.pending.is_some() {
                                    self.finish_pending();
                                }
                            }
                            Err(error) => self.show_error(error),
                        }
                    }
                }
                Outcome::Neutral => {
                    let suggestion = self.buffer().map_or_else(String::new, |buffer| {
                        format!("{}.editur-copy", buffer.path.display())
                    });
                    self.save_as = Some(suggestion);
                    self.conflict = false;
                }
                Outcome::Cancel | Outcome::Dismissed => {
                    self.conflict = false;
                    self.pending = None;
                }
                _ => {}
            }
        }
        if self.save_as.is_some() {
            let destination = self.save_as.clone().unwrap_or_default();
            let trimmed = destination.trim();
            let parent_missing = (!trimmed.is_empty())
                .then(|| PathBuf::from(trimmed))
                .and_then(|path| path.parent().map(Path::to_path_buf))
                .is_some_and(|parent| !parent.as_os_str().is_empty() && !parent.is_dir());
            let outcome = Dialog::new("save_as_dialog", "Save As")
                .primary("Save")
                .primary_enabled(!trimmed.is_empty() && !parent_missing)
                .show_with(ctx, |ui| {
                    ui.add_space(theme::space::TIGHT);
                    ui.label(
                        RichText::new("Destination path")
                            .font(theme::typography::small())
                            .color(theme::text().muted),
                    );
                    if let Some(path) = self.save_as.as_mut() {
                        let response = ui.add(
                            TextEdit::singleline(path)
                                .id(Id::new("save_as_input"))
                                .font(theme::typography::code_small())
                                .desired_width(ui.available_width()),
                        );
                        if !response.has_focus() && ui.memory(|memory| memory.focused()).is_none() {
                            response.request_focus();
                        }
                    }
                    if parent_missing {
                        ui.label(
                            RichText::new("That folder does not exist.")
                                .font(theme::typography::small())
                                .color(theme::ink(theme::semantic().danger)),
                        );
                    }
                });
            match outcome {
                Outcome::Primary => {
                    if self.save(Some(PathBuf::from(trimmed))) {
                        self.save_as = None;
                        self.finish_pending();
                    }
                }
                Outcome::Cancel | Outcome::Dismissed => {
                    self.save_as = None;
                    self.conflict = true;
                }
                _ => {}
            }
        }
    }
}

fn completion_word_range(text: &str, cursor: usize) -> std::ops::Range<usize> {
    let (cursor, byte_cursor) = text.char_indices().nth(cursor).map_or_else(
        || (text.chars().count(), text.len()),
        |(byte, _)| (cursor, byte),
    );
    let is_word = |character: &char| character.is_alphanumeric() || *character == '_';
    let before = text[..byte_cursor]
        .chars()
        .rev()
        .take_while(is_word)
        .count();
    let after = text[byte_cursor..].chars().take_while(is_word).count();
    cursor - before..cursor + after
}

fn completion_kind_label(kind: Option<i32>) -> Option<&'static str> {
    Some(match kind? {
        1 => "Text",
        2 => "Method",
        3 => "Function",
        4 => "Constructor",
        5 => "Field",
        6 => "Variable",
        7 => "Class",
        8 => "Interface",
        9 => "Module",
        10 => "Property",
        11 => "Unit",
        12 => "Value",
        13 => "Enum",
        14 => "Keyword",
        16 => "Color",
        17 => "File",
        18 => "Reference",
        19 => "Folder",
        20 => "Enum member",
        21 => "Constant",
        22 => "Struct",
        23 => "Event",
        24 => "Operator",
        25 => "Type parameter",
        _ => return None,
    })
}

fn popup_position(desired: egui::Pos2, size: egui::Vec2, bounds: egui::Rect) -> egui::Pos2 {
    egui::pos2(
        desired
            .x
            .clamp(bounds.left(), (bounds.right() - size.x).max(bounds.left())),
        desired
            .y
            .clamp(bounds.top(), (bounds.bottom() - size.y).max(bounds.top())),
    )
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
    selectable_content_row(ui, selected, min_height, |ui| {
        ui.add(Label::new(job).wrap());
    })
}

fn search_hit(results: &SearchResults, index: usize) -> Option<&SearchHit> {
    results.files.get(index).or_else(|| {
        results
            .contents
            .get(index.saturating_sub(results.files.len()))
    })
}

/// A composer selector: a segmented chip rather than bare text with a chevron,
/// so the thing that opens a menu looks like a control.
fn agent_selector_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    segment(ui, label, false, Some(Icon::ChevronDown))
}

fn agent_config_selector_button(
    ui: &mut egui::Ui,
    id: Option<Id>,
    label: &str,
    fast: bool,
    max_width: f32,
) -> egui::Response {
    let font = theme::typography::small();
    let color = theme::text().secondary;
    let natural_text = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font.clone(), color);
    let icon_size = icons::GRID * 0.75;
    let icon_width = icon_size + theme::space::TIGHT;
    let natural_width = natural_text.size().x
        + theme::space::MEDIUM
        + icon_width
        + if fast { icon_width } else { 0.0 };
    let width = natural_width
        .min(max_width)
        .min(ui.available_width())
        .max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, theme::control::COMPACT), Sense::click());
    let response = id.map_or(response, |id| ui.interact(rect, id, Sense::click()));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if response.hovered() {
        ui.painter().rect_filled(
            rect,
            theme::corner(theme::radius::CONTROL),
            theme::state::hover(),
        );
    }
    let color = if response.hovered() {
        theme::text().primary
    } else {
        color
    };
    let chevron = egui::Rect::from_center_size(
        egui::pos2(
            rect.right() - theme::space::SNUG - icon_size * 0.5,
            rect.center().y,
        ),
        egui::Vec2::splat(icon_size),
    );
    let bolt = fast.then(|| chevron.translate(egui::vec2(-icon_width, 0.0)));
    let text_right = bolt.map_or(chevron.left(), |bolt| bolt.left());
    let mut job = LayoutJob::single_section(
        label.to_owned(),
        TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (text_right - rect.left() - theme::space::SNUG - theme::space::TIGHT).max(1.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let text = ui.fonts_mut(|fonts| fonts.layout_job(job));
    ui.painter().galley(
        egui::pos2(
            rect.left() + theme::space::SNUG,
            rect.center().y - text.size().y * 0.5,
        ),
        text,
        color,
    );
    if let Some(bolt) = bolt {
        icons::paint(
            ui.painter(),
            Icon::Bolt,
            bolt,
            if ui.is_enabled() {
                theme::accent()
            } else {
                theme::text_disabled()
            },
        );
    }
    icons::paint(ui.painter(), Icon::ChevronDown, chevron, color);
    response
}

fn assistant_send_button_colors(ready: bool) -> (Color32, Color32) {
    if ready {
        (theme::accent(), theme::text().on_accent)
    } else {
        (theme::state::hover(), theme::text_disabled())
    }
}

/// The floating composer sits just below the editor canvas in the surface stack.
fn agentic_composer_fill() -> Color32 {
    theme::mix(theme::surface().editor, theme::surface().input, 0.7)
}

/// Send and stop: one solid 32 px control, filled by state rather than drawn
/// as a bare glyph, because it is the panel's primary action.
fn assistant_composer_action(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    fill: Color32,
    color: Color32,
    enabled: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::Vec2::splat(theme::control::STANDARD), Sense::click());
    let response = response.on_hover_text(label);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let fill = if enabled && response.hovered() {
        theme::composite(theme::state::hover(), fill)
    } else {
        fill
    };
    ui.painter()
        .rect_filled(rect, theme::corner(theme::radius::CONTROL), fill);
    icons::paint(
        ui.painter(),
        icon,
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(icons::GRID)),
        color,
    );
    if response.has_focus() {
        icons::focus_ring(ui.painter(), rect, theme::radius::CONTROL);
    }
    response
}

fn is_model_config(id: &str, name: &str) -> bool {
    id.eq_ignore_ascii_case("model") || name.eq_ignore_ascii_case("model")
}

fn is_thinking_config(id: &str, name: &str) -> bool {
    let id = id.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    matches!(
        id.as_str(),
        "thinking" | "reasoning" | "reasoning_effort" | "effort" | "thought_level"
    ) || ["thinking", "reasoning", "thought", "effort"]
        .iter()
        .any(|term| name.contains(term))
}

fn is_effort_config(id: &str, name: &str) -> bool {
    id.to_ascii_lowercase().contains("effort") || name.to_ascii_lowercase().contains("effort")
}

fn is_fast_config(option: &ConfigChoice) -> bool {
    let named = [option.id.as_str(), option.name.as_str()]
        .iter()
        .any(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "fast" | "fast-mode" | "fast mode" | "speed"
            )
        });
    named
        || (!is_model_config(&option.id, &option.name)
            && !is_thinking_config(&option.id, &option.name)
            && !option.id.eq_ignore_ascii_case("mode")
            && !option.name.eq_ignore_ascii_case("mode")
            && option.options.len() == 2
            && option.options.iter().any(|value| {
                value.id.eq_ignore_ascii_case("fast") || value.name.eq_ignore_ascii_case("fast")
            }))
}

fn fast_mode_config(options: &[ConfigChoice]) -> Option<(&ConfigChoice, bool, ConfigValue)> {
    options.iter().find_map(|option| {
        if !is_fast_config(option) {
            return None;
        }
        match &option.value {
            ConfigValue::Boolean(enabled) => {
                Some((option, *enabled, ConfigValue::Boolean(!enabled)))
            }
            ConfigValue::Select(current) => {
                let fast = option.options.iter().find(|value| {
                    [value.id.as_str(), value.name.as_str()]
                        .iter()
                        .any(|value| {
                            matches!(
                                value.to_ascii_lowercase().as_str(),
                                "fast" | "true" | "on" | "enabled"
                            )
                        })
                })?;
                let enabled = current == &fast.id;
                let next = if enabled {
                    option.options.iter().find(|value| value.id != fast.id)?
                } else {
                    fast
                };
                Some((option, enabled, ConfigValue::Select(next.id.clone())))
            }
        }
    })
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

fn selected_config_name(option: &ConfigChoice) -> Option<Cow<'_, str>> {
    let ConfigValue::Select(current) = &option.value else {
        return None;
    };
    let value = option.options.iter().find(|value| value.id == *current);
    Some(if is_model_config(&option.id, &option.name) {
        value.map_or_else(
            || model_display_name(current, current),
            |value| model_display_name(&value.id, &value.name),
        )
    } else {
        value.map_or(Cow::Borrowed(current.as_str()), |value| {
            Cow::Borrowed(value.name.as_str())
        })
    })
}

fn provider_menu_option(
    ui: &mut egui::Ui,
    provider: &ProviderDescriptor,
    packaged: bool,
    selected: bool,
) -> egui::Response {
    let unavailable = provider
        .unavailable_reason
        .or_else(|| (!packaged).then_some("Unavailable in this build"));
    let accessibility = unavailable.map_or_else(
        || format!("{}: {}", provider.display_name, provider.description),
        |status| {
            format!(
                "{}: {}. {status}",
                provider.display_name, provider.description
            )
        },
    );
    let row = selectable_row(ui, &accessibility, selected, AGENT_PROVIDER_ROW_HEIGHT);
    let rect = row.rect;
    let name_color = row.foreground;
    let response = row.response;
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 20.0, rect.center().y),
        egui::vec2(18.0, 18.0),
    );
    paint_provider_icon(ui.painter(), icon_rect, provider.icon, name_color);
    ui.painter().text(
        egui::pos2(rect.left() + 39.0, rect.center().y),
        Align2::LEFT_CENTER,
        provider.display_name,
        theme::typography::body(),
        name_color,
    );
    response
}

fn agent_menu_option(
    ui: &mut egui::Ui,
    label: &str,
    selected: bool,
    row_height: f32,
) -> egui::Response {
    let row = selectable_row(ui, label, selected, row_height);
    ui.painter().text(
        egui::pos2(row.rect.left() + 9.0, row.rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::typography::small(),
        row.foreground,
    );
    row.response
}

fn agent_menu_section_label(ui: &mut egui::Ui, label: &str, row_height: f32) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), row_height), Sense::hover());
    ui.painter().hline(
        rect.shrink2(egui::vec2(9.0, 0.0)).x_range(),
        rect.bottom(),
        theme::border::hairline(),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 9.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::typography::small_strong(),
        theme::text().primary,
    );
}

fn agent_mention_row(
    ui: &mut egui::Ui,
    entry: &AgentMentionEntry,
    selected: bool,
) -> egui::Response {
    let response = selectable_content_row(ui, selected, 20.0, |ui| {
        ui.horizontal(|ui| {
            let (_, icon) = ui.allocate_space(egui::Vec2::splat(icons::GRID));
            icons::paint(
                ui.painter(),
                if entry.is_dir {
                    Icon::Folder
                } else {
                    Icon::File
                },
                icon,
                if selected {
                    theme::text().primary
                } else {
                    theme::text().secondary
                },
            );
            ui.add(
                Label::new(
                    RichText::new(&entry.relative)
                        .monospace()
                        .size(theme::typography::SMALL_SIZE),
                )
                .truncate(),
            );
        });
    });
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            entry.relative.clone(),
        )
    });
    response
}

fn agent_toggle_row(ui: &mut egui::Ui, label: &str, enabled: bool, height: f32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), enabled, label)
    });
    if response.hovered() {
        ui.painter().rect_filled(rect, 5.0, theme::state::hover());
    }
    ui.painter().text(
        egui::pos2(rect.left() + 9.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::typography::small(),
        if response.hovered() {
            theme::text().primary
        } else {
            theme::text().secondary
        },
    );
    let track = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 19.0, rect.center().y),
        egui::vec2(26.0, 14.0),
    );
    ui.painter().rect_filled(
        track,
        7.0,
        if enabled {
            theme::accent()
        } else {
            theme::border::strong_color()
        },
    );
    ui.painter().circle_filled(
        egui::pos2(
            if enabled {
                track.right() - 7.0
            } else {
                track.left() + 7.0
            },
            track.center().y,
        ),
        5.0,
        if enabled {
            theme::text().on_accent
        } else {
            theme::text().secondary
        },
    );
    response
}

/// A muted section label for the agentic rail with an optional "+" action on
/// the right. Returns whether the action was clicked.
fn agentic_section_header(
    ui: &mut egui::Ui,
    label: &str,
    action: Option<(&'static str, &'static str)>,
) -> bool {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(
            ui.available_width(),
            theme::control::COMPACT + theme::space::TIGHT,
        ),
        Sense::hover(),
    );
    ui.painter().text(
        egui::pos2(rect.left() + theme::space::SMALL, rect.center().y),
        Align2::LEFT_CENTER,
        label.to_ascii_uppercase(),
        theme::typography::micro(),
        theme::text().muted,
    );
    let Some((id, hover)) = action else {
        return false;
    };
    let button = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 12.0, rect.center().y),
        egui::Vec2::splat(20.0),
    );
    let response = ui
        .interact(button, Id::new(id), Sense::click())
        .on_hover_text(hover);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), hover)
    });
    if response.hovered() {
        ui.painter().rect_filled(button, 4.0, theme::state::hover());
    }
    let color = if response.hovered() {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    ui.painter().hline(
        (button.center().x - 5.0)..=(button.center().x + 5.0),
        button.center().y,
        egui::Stroke::new(1.3, color),
    );
    ui.painter().vline(
        button.center().x,
        (button.center().y - 5.0)..=(button.center().y + 5.0),
        egui::Stroke::new(1.3, color),
    );
    response.clicked()
}

/// One project in the agentic rail. The open project is marked selected;
/// clicking any row asks the app to switch to that root.
fn agentic_project_row(ui: &mut egui::Ui, root: &Path, selected: bool) -> bool {
    let name = root
        .file_name()
        .unwrap_or(root.as_os_str())
        .to_string_lossy();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(
            ui.available_width(),
            theme::control::ROW + theme::space::TIGHT,
        ),
        Sense::hover(),
    );
    let response = ui.interact(rect, Id::new(("agentic_project", root)), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            ui.is_enabled(),
            format!("Open project {name}"),
        )
    });
    if selected {
        ui.painter().rect_filled(
            rect,
            theme::corner(theme::radius::CONTROL),
            theme::state::selected(),
        );
    } else if response.hovered() {
        ui.painter().rect_filled(
            rect,
            theme::corner(theme::radius::CONTROL),
            theme::state::hover(),
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + theme::space::MEDIUM, rect.center().y),
        Align2::LEFT_CENTER,
        name,
        if selected {
            theme::typography::small_strong()
        } else {
            theme::typography::small()
        },
        if selected || response.hovered() {
            theme::text().primary
        } else {
            theme::text().secondary
        },
    );
    response.clicked()
}

fn agent_session_row(
    ui: &mut egui::Ui,
    session: &SessionChoice,
    provider: ProviderId,
    selected: bool,
    compact: bool,
) -> (bool, bool, bool) {
    let label = session
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("Untitled session");
    let row_height = if compact {
        theme::control::ROW + theme::space::TIGHT
    } else {
        AGENT_SESSION_ROW_HEIGHT
    };
    let (_, row) = ui.allocate_space(egui::vec2(ui.available_width(), row_height));
    let remove = egui::Rect::from_min_max(
        egui::pos2(row.right() - row_height, row.top()),
        row.right_bottom(),
    );
    let open = row.with_max_x(remove.left());
    let open_response = ui.interact(
        open,
        Id::new(("agent_session_open", &session.id)),
        Sense::click_and_drag(),
    );
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
    if selected {
        ui.painter().rect_filled(
            row,
            theme::corner(theme::radius::CONTROL),
            theme::state::selected(),
        );
    } else if open_response.hovered() {
        ui.painter().rect_filled(
            row,
            theme::corner(theme::radius::CONTROL),
            theme::state::hover(),
        );
    }
    if remove_response.hovered() {
        ui.painter().rect_filled(
            remove,
            theme::corner(theme::radius::ROW),
            theme::callout(theme::semantic().danger).fill,
        );
    }
    let color = if selected || open_response.hovered() {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    let font = if compact && selected {
        theme::typography::strong()
    } else {
        theme::typography::body()
    };
    let galley = ui.painter().layout_no_wrap(label.to_owned(), font, color);
    let text_padding = theme::space::SMALL;
    let origin_width = if session.started_in_editur { 0.0 } else { 18.0 };
    if !session.started_in_editur {
        let descriptor = provider_descriptor(provider);
        let icon = egui::Rect::from_center_size(
            egui::pos2(open.left() + text_padding + 6.0, open.center().y),
            egui::vec2(12.0, 14.0),
        );
        let origin_label = format!("Started in {}", descriptor.display_name);
        ui.interact(
            icon,
            Id::new(("agent_session_origin", &session.id, provider.as_str())),
            Sense::hover(),
        )
        .widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Label,
                ui.is_enabled(),
                origin_label.clone(),
            )
        });
        paint_provider_icon(ui.painter(), icon, descriptor.icon, theme::text().muted);
    }
    ui.painter()
        .with_clip_rect(open.shrink2(egui::vec2(theme::space::TIGHT, 0.0)))
        .galley(
            egui::pos2(
                open.left() + text_padding + origin_width,
                open.center().y - galley.size().y * 0.5,
            ),
            galley,
            color,
        );
    if !compact || open_response.hovered() || remove_response.hovered() {
        ui.painter().text(
            remove.center(),
            Align2::CENTER_CENTER,
            "×",
            theme::typography::title(),
            if remove_response.hovered() {
                theme::ink(theme::semantic().danger)
            } else {
                theme::text().muted
            },
        );
    }
    (
        open_response.clicked(),
        remove_response.clicked(),
        open_response.drag_started() || open_response.dragged(),
    )
}

fn plain_text_job(text: &str, wrap_width: f32) -> LayoutJob {
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: theme::typography::code_editor(),
            color: theme::syntax().foreground,
            ..TextFormat::default()
        },
    );
    job
}

fn draw_markdown_preview(
    ui: &mut egui::Ui,
    source: &str,
    revision: u64,
    cache: &mut MarkdownLayoutCache,
    pane: PaneId,
) {
    let rect = ui.available_rect_before_wrap();
    ui.painter().rect_filled(rect, 0.0, editor_background());
    let content_width = (rect.width() - 64.0).clamp(1.0, 860.0);
    let side = ((rect.width() - content_width) * 0.5).max(0.0);
    let key = (
        revision,
        content_width.round().to_bits(),
        theme::paint_appearance(ui.pixels_per_point()),
    );
    if cache.as_ref().is_none_or(|(current, _)| *current != key) {
        let job = markdown::layout(source, content_width);
        *cache = Some((key, ui.fonts_mut(|fonts| fonts.layout_job(job))));
    }
    let galley = Arc::clone(&cache.as_ref().expect("Markdown layout was cached").1);
    ScrollArea::vertical()
        .id_salt(("markdown_preview", pane.0))
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

/// A group only announces itself when it has something in it, and it always
/// says how much, so the reader can tell a short list from a truncated one.
fn search_group_header(ui: &mut egui::Ui, label: &str, count: usize) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(label)
                .font(theme::typography::micro())
                .color(theme::text().muted),
        );
        ui.label(
            RichText::new(count.to_string())
                .font(theme::typography::micro())
                .color(theme::text_disabled()),
        );
    });
}

/// The filename result, with the part the query matched picked out — the same
/// treatment the content results already get.
fn file_result_job(relative: &str, query: &str, wrap_width: f32) -> LayoutJob {
    let font_id = theme::typography::code_small();
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    let normal = TextFormat {
        font_id: font_id.clone(),
        color: theme::text().secondary,
        ..TextFormat::default()
    };
    let highlighted = TextFormat {
        font_id,
        color: theme::accent(),
        background: theme::state::selected(),
        ..TextFormat::default()
    };
    job.append("  ", 0.0, normal.clone());
    let mut cursor = 0;
    for span in match_spans(relative, query) {
        job.append(&relative[cursor..span.start], 0.0, normal.clone());
        job.append(&relative[span.clone()], 0.0, highlighted.clone());
        cursor = span.end;
    }
    job.append(&relative[cursor..], 0.0, normal);
    job
}

fn content_result_job(hit: &SearchHit, query: &str, wrap_width: f32) -> LayoutJob {
    let font_id = theme::typography::code_small();
    let mut job = LayoutJob::default();
    job.wrap.max_width = wrap_width;
    job.wrap.break_anywhere = true;
    job.append(
        &format!("  {}:{}\n", hit.relative, hit.line.unwrap_or(1)),
        0.0,
        TextFormat {
            font_id: font_id.clone(),
            color: theme::text().secondary,
            ..TextFormat::default()
        },
    );
    let normal = TextFormat {
        font_id: font_id.clone(),
        color: theme::text().secondary,
        ..TextFormat::default()
    };
    let highlighted = TextFormat {
        font_id,
        color: theme::accent(),
        background: theme::state::selected(),
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
                if current_match == active {
                    format.background = theme::state::find::active_fill();
                    format.color = theme::state::find::active_ink();
                } else {
                    format.background = theme::state::find::match_fill();
                }
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
            section.format.background = theme::state::selected();
            section.format.underline = egui::Stroke::new(1.0, theme::accent());
        }
    }
    highlighted
}

fn diagnostic_highlighted_job(
    base: &LayoutJob,
    diagnostics: &[crate::lsp::Diagnostic],
    stale: bool,
) -> LayoutJob {
    let mut events = diagnostics
        .iter()
        .filter(|diagnostic| {
            !diagnostic.range.is_empty()
                && diagnostic.range.end <= base.text.len()
                && base.text.is_char_boundary(diagnostic.range.start)
                && base.text.is_char_boundary(diagnostic.range.end)
        })
        .flat_map(|diagnostic| {
            [
                (diagnostic.range.start, true, diagnostic.severity),
                (diagnostic.range.end, false, diagnostic.severity),
            ]
        })
        .collect::<Vec<_>>();
    if events.is_empty() {
        return base.clone();
    }
    events.sort_unstable_by_key(|event| event.0);
    let mut active = [0_usize; 4];
    let mut spans = Vec::new();
    let mut cursor = 0;
    let mut index = 0;
    while index < events.len() {
        let position = events[index].0;
        if cursor < position
            && let Some(severity) = active_diagnostic_severity(active)
        {
            spans.push((cursor..position, severity));
        }
        while index < events.len() && events[index].0 == position {
            let slot = diagnostic_severity_index(events[index].2);
            if events[index].1 {
                active[slot] += 1;
            } else {
                active[slot] = active[slot].saturating_sub(1);
            }
            index += 1;
        }
        cursor = position;
    }

    let mut highlighted = base.clone();
    highlighted.text.clear();
    highlighted.sections.clear();
    let mut span_index = 0;
    for section in &base.sections {
        let section_start = section.byte_range.start.0;
        let section_end = section.byte_range.end.0;
        while span_index < spans.len() && spans[span_index].0.end <= section_start {
            span_index += 1;
        }
        let mut current_span = span_index;
        let mut section_cursor = section_start;
        let mut leading_space = section.leading_space;
        while current_span < spans.len() && spans[current_span].0.start < section_end {
            let start = spans[current_span].0.start.max(section_start);
            let end = spans[current_span].0.end.min(section_end);
            if section_cursor < start {
                highlighted.append(
                    &base.text[section_cursor..start],
                    leading_space,
                    section.format.clone(),
                );
                leading_space = 0.0;
            }
            if start < end {
                let mut format = section.format.clone();
                let color = diagnostic_color(spans[current_span].1);
                format.underline = egui::Stroke::new(
                    1.2,
                    if stale {
                        color.gamma_multiply(0.55)
                    } else {
                        color
                    },
                );
                highlighted.append(&base.text[start..end], leading_space, format);
                leading_space = 0.0;
                section_cursor = end;
            }
            current_span += 1;
        }
        if section_cursor < section_end {
            highlighted.append(
                &base.text[section_cursor..section_end],
                leading_space,
                section.format.clone(),
            );
        }
        while span_index < spans.len() && spans[span_index].0.end <= section_end {
            span_index += 1;
        }
    }
    highlighted
}

const fn diagnostic_severity_index(severity: crate::lsp::DiagnosticSeverity) -> usize {
    match severity {
        crate::lsp::DiagnosticSeverity::Error => 0,
        crate::lsp::DiagnosticSeverity::Warning => 1,
        crate::lsp::DiagnosticSeverity::Information => 2,
        crate::lsp::DiagnosticSeverity::Hint => 3,
    }
}

fn active_diagnostic_severity(active: [usize; 4]) -> Option<crate::lsp::DiagnosticSeverity> {
    [
        crate::lsp::DiagnosticSeverity::Error,
        crate::lsp::DiagnosticSeverity::Warning,
        crate::lsp::DiagnosticSeverity::Information,
        crate::lsp::DiagnosticSeverity::Hint,
    ]
    .into_iter()
    .zip(active)
    .find_map(|(severity, count)| (count > 0).then_some(severity))
}

fn diagnostic_color(severity: crate::lsp::DiagnosticSeverity) -> Color32 {
    match severity {
        crate::lsp::DiagnosticSeverity::Error => theme::semantic().danger,
        crate::lsp::DiagnosticSeverity::Warning => theme::semantic().warning,
        crate::lsp::DiagnosticSeverity::Information | crate::lsp::DiagnosticSeverity::Hint => {
            theme::semantic().info
        }
    }
}

fn presentation_job<'a>(
    base: &'a LayoutJob,
    bracket_overlay: Option<&'a LayoutJob>,
) -> &'a LayoutJob {
    bracket_overlay.unwrap_or(base)
}

/// Non-overlapping, ASCII-case-insensitive matches. Compares byte windows in
/// place: this runs for every diff row during a search, so the two lowercase
/// String copies the obvious version makes are too expensive.
fn match_spans(text: &str, query: &str) -> Vec<std::ops::Range<usize>> {
    let mut spans = Vec::new();
    if query.is_empty() || query.len() > text.len() {
        return spans;
    }
    let text = text.as_bytes();
    let query = query.as_bytes();
    let mut cursor = 0;
    while cursor + query.len() <= text.len() {
        if text[cursor..cursor + query.len()].eq_ignore_ascii_case(query) {
            spans.push(cursor..cursor + query.len());
            cursor += query.len();
        } else {
            cursor += 1;
        }
    }
    spans
}

fn truncate_lines(value: &str, lines: usize) -> String {
    value.lines().take(lines).collect::<Vec<_>>().join("\n")
}

fn settings_preset_matches(preset: &crate::lsp::Preset, query: &str) -> bool {
    query.is_empty()
        || preset.language.to_ascii_lowercase().contains(query)
        || preset.name.to_ascii_lowercase().contains(query)
        || preset.id.as_str().contains(query)
}

#[cfg(test)]
mod tests;
