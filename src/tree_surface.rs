use egui::{Color32, Rect, Response, Sense, Stroke, Ui};
use std::{collections::HashMap, ops::Range, path::Path, sync::Arc};

use crate::{
    icons::{self, Icon},
    renderer::mark_retained,
    theme,
    tree::TreeEntry,
};

/// The indent step, and the column the guide for that depth lives in.
const INDENT: f32 = 16.0;
/// The chevron sits to the left of its own indent column so it never lands on
/// the guide it is meant to replace.
const CHEVRON_INSET: f32 = 6.0;

fn row_height() -> f32 {
    theme::control::row()
}

#[derive(Clone)]
pub struct TreeRow {
    pub entry: TreeEntry,
    pub label: String,
    pub depth: usize,
    pub directory: bool,
    pub expanded: bool,
    pub revision: u64,
}

/// A label galley depends on the row's state, not only on its text: a selected
/// row is brighter, and a cache that ignores that paints yesterday's color.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct LabelKey {
    revision: u64,
    width: u32,
    selected: bool,
    hovered: bool,
    appearance: u64,
}

#[derive(Default)]
pub struct TreeSurface {
    scroll_y: f32,
    scrollbar: crate::scrollbar::State,
    hovered: Option<usize>,
    labels: HashMap<LabelKey, Arc<egui::Galley>>,
}

pub struct TreeOutput {
    pub response: Response,
    pub clicked: Option<usize>,
    pub drag_started: Option<usize>,
    pub context_requested: Option<usize>,
    /// The project name at the top was clicked; the app opens the switcher.
    pub root_clicked: bool,
}

impl TreeSurface {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        root: &Path,
        rows: &[TreeRow],
        selected: Option<usize>,
        scroll_to_selected: bool,
    ) -> TreeOutput {
        let (id, full) = ui.allocate_space(ui.available_size());
        ui.painter().rect_filled(full, 0.0, theme::surface().chrome);
        let header = full.with_max_y((full.top() + theme::control::ROW).min(full.bottom()));
        let root_clicked = self.draw_root_header(ui, id, header, root);
        let rect = full.with_min_y(header.bottom());
        let content = rect;
        let response = ui.interact(content, id.with("tree"), Sense::click_and_drag());
        let accepts_pointer = !response.context_menu_opened();
        let pointer = accepts_pointer
            .then(|| ui.input(|input| input.pointer.hover_pos()))
            .flatten()
            .filter(|pointer| content.contains(*pointer));
        let scrolling = accepts_pointer
            && ui.input(|input| {
                input
                    .pointer
                    .hover_pos()
                    .is_some_and(|pointer| rect.contains(pointer))
            })
            && ui.input(|input| input.smooth_scroll_delta.y != 0.0);
        if scrolling {
            self.scroll_y -= ui.input(|input| input.smooth_scroll_delta.y);
        }
        if scroll_to_selected && let Some(selected) = selected {
            self.scroll_to_row(selected, rows.len(), rect.height());
        }
        self.clamp_scroll(rows.len(), rect.height());
        self.hovered = pointer.and_then(|pointer| self.row_at(pointer.y, rect, rows.len()));
        let clicked = (accepts_pointer && response.clicked())
            .then_some(self.hovered)
            .flatten();
        let context_requested = (accepts_pointer && response.secondary_clicked())
            .then_some(self.hovered)
            .flatten();
        let drag_started = (accepts_pointer && response.drag_started())
            .then(|| {
                ui.input(|input| input.pointer.press_origin())
                    .filter(|pointer| content.contains(*pointer))
                    .and_then(|pointer| self.row_at(pointer.y, rect, rows.len()))
            })
            .flatten()
            .filter(|index| !rows[*index].directory);

        let painter = ui.painter_at(rect);
        mark_retained(
            &painter,
            rect,
            0x4000_0000_0000_0000,
            u64::from(rect.width().to_bits()) << 32 | u64::from(rect.height().to_bits()),
        );
        let row_height = row_height();
        let mut ancestor_on_screen = Vec::new();
        let mut truncated_hover = None;
        for index in self.visible_rows(rows.len(), rect.height()) {
            let row = &rows[index];
            let top = rect.top() + index as f32 * row_height - self.scroll_y;
            let row_rect = Rect::from_min_size(
                egui::pos2(rect.left() + theme::space::TIGHT, top + 1.0),
                egui::vec2(
                    (content.width() - theme::space::SMALL).max(0.0),
                    row_height - 2.0,
                ),
            );
            let is_selected = selected == Some(index);
            let is_hovered = self.hovered == Some(index);
            let revision = row.revision
                ^ u64::from(top.to_bits())
                ^ u64::from(rect.width().to_bits()).rotate_left(32)
                ^ (row.expanded as u64) << 33
                ^ is_selected as u64
                ^ (is_hovered as u64) << 1;
            mark_retained(
                &painter,
                rect,
                0x5000_0000_0000_0000 | index as u64,
                revision,
            );
            let fill = theme::state::fill(is_selected, true, is_hovered, false);
            if fill != Color32::TRANSPARENT {
                painter.rect_filled(row_rect, theme::corner(theme::radius::ROW), fill);
            }
            ancestor_on_screen.resize(row.depth + 1, false);
            for (depth, on_screen) in ancestor_on_screen.iter().take(row.depth).enumerate() {
                if !on_screen {
                    continue;
                }
                painter.vline(
                    column(rect, depth),
                    row_rect.y_range(),
                    Stroke::new(theme::stroke::DIVIDER, theme::editor::indent_guide()),
                );
            }
            ancestor_on_screen[row.depth] = true;
            ancestor_on_screen.truncate(row.depth + 1);
            let content_left = column(rect, row.depth);
            let glyph_color = if is_selected {
                theme::accent()
            } else if row.directory {
                theme::text().muted
            } else {
                theme::text_disabled()
            };
            if row.directory {
                icons::paint(
                    &painter,
                    if row.expanded {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    },
                    Rect::from_center_size(
                        egui::pos2(content_left - CHEVRON_INSET, row_rect.center().y),
                        egui::Vec2::splat(icons::GRID * 0.75),
                    ),
                    theme::text().muted,
                );
            }
            icons::paint(
                &painter,
                if row.directory {
                    Icon::Folder
                } else {
                    Icon::File
                },
                Rect::from_min_size(
                    egui::pos2(content_left, row_rect.center().y - icons::GRID * 0.5),
                    egui::Vec2::splat(icons::GRID),
                ),
                glyph_color,
            );
            let label_left = content_left + icons::GRID + theme::space::SNUG;
            let available = (row_rect.right() - label_left - theme::space::SNUG).max(0.0);
            let key = LabelKey {
                revision: row.revision,
                width: available.to_bits(),
                selected: is_selected,
                hovered: is_hovered,
                appearance: theme::paint_appearance(ui.pixels_per_point()),
            };
            let label = self.labels.entry(key).or_insert_with(|| {
                let color = if is_selected || is_hovered {
                    theme::text().primary
                } else {
                    theme::text().secondary
                };
                let mut job = egui::text::LayoutJob::single_section(
                    row.label.clone(),
                    egui::TextFormat {
                        font_id: if row.directory {
                            theme::typography::small_strong()
                        } else {
                            theme::typography::small()
                        },
                        color,
                        ..Default::default()
                    },
                );
                job.wrap = egui::text::TextWrapping {
                    max_width: available,
                    max_rows: 1,
                    break_anywhere: true,
                    overflow_character: Some('…'),
                };
                ui.fonts_mut(|fonts| fonts.layout_job(job))
            });
            if is_hovered && label.elided {
                truncated_hover = Some(row.entry.path.clone());
            }
            painter.galley(
                egui::pos2(label_left, row_rect.center().y - label.size().y * 0.5),
                Arc::clone(label),
                theme::text().primary,
            );
        }
        if let Some(path) = truncated_hover {
            response.clone().on_hover_text(path.display().to_string());
        }
        if crate::scrollbar::show(
            ui,
            id.with("scrollbar"),
            rect,
            rows.len() as f32 * row_height,
            &mut self.scroll_y,
            &mut self.scrollbar,
            scrolling,
        ) {
            self.clamp_scroll(rows.len(), rect.height());
            ui.ctx().request_repaint();
        }

        TreeOutput {
            response,
            clicked,
            drag_started,
            context_requested,
            root_clicked,
        }
    }

    /// The first thing in the window that says which project is open. Clicking
    /// it is how the editor switches projects, so it reads as a button on
    /// hover.
    fn draw_root_header(&self, ui: &mut Ui, id: egui::Id, header: Rect, root: &Path) -> bool {
        let response = ui
            .interact(header, id.with("tree_root"), Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), "Switch project")
        });
        let name = root
            .file_name()
            .unwrap_or(root.as_os_str())
            .to_string_lossy();
        let painter = ui.painter_at(header);
        if response.hovered() {
            painter.rect_filled(header, 0.0, theme::state::hover());
        }
        let left = header.left() + theme::space::MEDIUM;
        icons::paint(
            &painter,
            Icon::Folder,
            Rect::from_min_size(
                egui::pos2(left, header.center().y - icons::GRID * 0.5),
                egui::Vec2::splat(icons::GRID),
            ),
            theme::text().muted,
        );
        let label_left = left + icons::GRID + theme::space::SNUG;
        painter.text(
            egui::pos2(label_left, header.center().y),
            egui::Align2::LEFT_CENTER,
            name.as_ref(),
            theme::typography::strong(),
            theme::text().primary,
        );
        if response.hovered() {
            let name_width = painter
                .layout_no_wrap(
                    name.into_owned(),
                    theme::typography::strong(),
                    theme::text().primary,
                )
                .size()
                .x;
            icons::paint(
                &painter,
                Icon::ChevronDown,
                Rect::from_center_size(
                    egui::pos2(
                        label_left + name_width + theme::space::SNUG + icons::GRID * 0.5,
                        header.center().y,
                    ),
                    egui::Vec2::splat(icons::GRID * 0.75),
                ),
                theme::text().muted,
            );
        }
        painter.hline(header.x_range(), header.bottom(), theme::border::hairline());
        response.on_hover_text(root.display().to_string()).clicked()
    }

    pub fn visible_rows(&self, total: usize, viewport_height: f32) -> Range<usize> {
        let row_height = row_height();
        let start = (self.scroll_y / row_height).floor() as usize;
        let visible = (viewport_height / row_height).ceil() as usize + 1;
        start.min(total)..(start + visible).min(total)
    }

    pub fn scroll_to_row(&mut self, row: usize, total: usize, viewport_height: f32) {
        let top = row as f32 * row_height();
        let bottom = top + row_height();
        if top < self.scroll_y {
            self.scroll_y = top;
        } else if bottom > self.scroll_y + viewport_height {
            self.scroll_y = bottom - viewport_height;
        }
        self.clamp_scroll(total, viewport_height);
    }

    fn row_at(&self, pointer_y: f32, rect: Rect, total: usize) -> Option<usize> {
        let document_y = pointer_y - rect.top() + self.scroll_y;
        (document_y >= 0.0)
            .then_some((document_y / row_height()).floor() as usize)
            .filter(|index| *index < total)
    }

    fn clamp_scroll(&mut self, total: usize, viewport_height: f32) {
        self.scroll_y = self.scroll_y.clamp(
            0.0,
            (total as f32 * row_height() - viewport_height).max(0.0),
        );
    }
}

/// The x of the indent guide for `depth`, which is also where that depth's
/// content starts. The base offset leaves the depth-0 disclosure chevron a
/// full gutter instead of pressing it against the sidebar border.
fn column(rect: Rect, depth: usize) -> f32 {
    rect.left() + theme::space::XWIDE + depth as f32 * INDENT
}

#[cfg(test)]
mod tests {
    use super::{TreeRow, TreeSurface, theme};
    use crate::{renderer::retained_paint, tree::TreeEntry};
    use egui::{Event, RawInput, Rect, Shape, Vec2, pos2};
    use std::{
        ffi::OsString,
        path::{Path, PathBuf},
    };

    #[test]
    fn retained_tree_limits_work_to_visible_rows_and_keeps_selection_in_view() {
        let mut surface = TreeSurface::default();
        surface.scroll_to_row(50, 100, 220.0);

        let visible = surface.visible_rows(100, 220.0);
        assert!(visible.contains(&50));
        assert!(visible.len() <= 13);
    }

    #[test]
    fn selected_row_retention_changes_during_sidebar_resize() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows = [TreeRow {
            entry: TreeEntry {
                name: OsString::from("main.rs"),
                path: PathBuf::from("main.rs"),
                is_dir: false,
                is_symlink: false,
            },
            label: "main.rs".into(),
            depth: 0,
            directory: false,
            expanded: false,
            revision: 1,
        }];
        let draw = |surface: &mut TreeSurface, width| {
            let output = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(width, 80.0))),
                    ..RawInput::default()
                },
                |ui| {
                    surface.show(ui, Path::new("/tmp/project"), &rows, Some(0), false);
                },
            );
            context
                .tessellate(output.shapes, output.pixels_per_point)
                .iter()
                .find_map(|primitive| {
                    retained_paint(&primitive.primitive)
                        .ok()
                        .flatten()
                        .filter(|paint| paint.key == 0x5000_0000_0000_0000)
                })
                .expect("retained selected row")
                .revision
        };

        assert_ne!(draw(&mut surface, 200.0), draw(&mut surface, 320.0));
    }

    #[test]
    fn pointer_movement_updates_the_retained_hover_row_immediately() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows = [TreeRow {
            entry: TreeEntry {
                name: OsString::from("main.rs"),
                path: PathBuf::from("main.rs"),
                is_dir: false,
                is_symlink: false,
            },
            label: "main.rs".into(),
            depth: 0,
            directory: false,
            expanded: false,
            revision: 1,
        }];
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
                events: vec![Event::PointerMoved(pos2(100.0, 42.0))],
                ..RawInput::default()
            },
            |ui| {
                surface.show(ui, Path::new("/tmp/project"), &rows, None, false);
            },
        );

        assert_eq!(surface.hovered, Some(0));
    }

    #[test]
    fn theme_switch_rebuilds_a_previously_hovered_label() {
        let _flag = theme::PALETTE_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        theme::set_light(false);
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows = [TreeRow {
            entry: TreeEntry {
                name: OsString::from("main.rs"),
                path: PathBuf::from("main.rs"),
                is_dir: false,
                is_symlink: false,
            },
            label: "main.rs".into(),
            depth: 0,
            directory: false,
            expanded: false,
            revision: 1,
        }];
        let mut draw = || {
            let _ = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0))),
                    events: vec![Event::PointerMoved(pos2(100.0, 42.0))],
                    ..RawInput::default()
                },
                |ui| {
                    surface.show(ui, Path::new("/tmp/project"), &rows, None, false);
                },
            );
        };

        draw();
        theme::set_light(true);
        theme::apply(&context);
        draw();
        theme::set_light(false);
        theme::apply(&context);

        assert_eq!(surface.labels.len(), 2);
    }

    #[test]
    fn file_rows_can_start_a_drag() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows = ["main.rs", "lib.rs"].map(|name| TreeRow {
            entry: TreeEntry {
                name: OsString::from(name),
                path: PathBuf::from(name),
                is_dir: false,
                is_symlink: false,
            },
            label: name.into(),
            depth: 0,
            directory: false,
            expanded: false,
            revision: 1,
        });
        let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0));
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                ..RawInput::default()
            },
            |ui| {
                surface.show(ui, Path::new("/tmp/project"), &rows, None, false);
            },
        );
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events: vec![
                    Event::PointerMoved(pos2(100.0, 42.0)),
                    Event::PointerButton {
                        pos: pos2(100.0, 42.0),
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                ..RawInput::default()
            },
            |ui| {
                surface.show(ui, Path::new("/tmp/project"), &rows, None, false);
            },
        );
        let mut drag_started = None;
        let _ = context.run_ui(
            RawInput {
                screen_rect: Some(screen),
                events: vec![Event::PointerMoved(pos2(100.0, 70.0))],
                ..RawInput::default()
            },
            |ui| {
                drag_started = surface
                    .show(ui, Path::new("/tmp/project"), &rows, None, false)
                    .drag_started;
            },
        );

        assert_eq!(surface.hovered, Some(1));
        assert_eq!(drag_started, Some(0));
    }

    #[test]
    fn right_click_reports_the_target_row() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows = [TreeRow {
            entry: TreeEntry {
                name: OsString::from("main.rs"),
                path: PathBuf::from("main.rs"),
                is_dir: false,
                is_symlink: false,
            },
            label: "main.rs".into(),
            depth: 0,
            directory: false,
            expanded: false,
            revision: 1,
        }];
        let screen = Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0)));
        let mut draw = |events| {
            let mut requested = None;
            let _ = context.run_ui(
                RawInput {
                    screen_rect: screen,
                    events,
                    ..RawInput::default()
                },
                |ui| {
                    requested = surface
                        .show(ui, Path::new("/tmp/project"), &rows, None, false)
                        .context_requested
                },
            );
            requested
        };

        draw(Vec::new());
        draw(vec![
            Event::PointerMoved(pos2(100.0, 42.0)),
            Event::PointerButton {
                pos: pos2(100.0, 42.0),
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);

        assert_eq!(
            draw(vec![Event::PointerButton {
                pos: pos2(100.0, 42.0),
                button: egui::PointerButton::Secondary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]),
            Some(0)
        );
    }

    #[test]
    fn clicking_the_root_header_reports_a_project_switch_request() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows: [TreeRow; 0] = [];
        let screen = Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0)));
        let header = pos2(100.0, 10.0);
        let mut draw = |events| {
            let mut root_clicked = false;
            let _ = context.run_ui(
                RawInput {
                    screen_rect: screen,
                    events,
                    ..RawInput::default()
                },
                |ui| {
                    root_clicked = surface
                        .show(ui, Path::new("/tmp/project"), &rows, None, false)
                        .root_clicked
                },
            );
            root_clicked
        };

        draw(Vec::new());
        draw(vec![
            Event::PointerMoved(header),
            Event::PointerButton {
                pos: header,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);

        assert!(draw(vec![Event::PointerButton {
            pos: header,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]));
    }

    #[test]
    fn open_context_menu_blocks_tree_hover_underneath() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows = ["main.rs", "lib.rs"].map(|name| TreeRow {
            entry: TreeEntry {
                name: OsString::from(name),
                path: PathBuf::from(name),
                is_dir: false,
                is_symlink: false,
            },
            label: name.into(),
            depth: 0,
            directory: false,
            expanded: false,
            revision: 1,
        });
        let screen = Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0)));
        let mut draw = |events| {
            let mut menu_open = false;
            let _ = context.run_ui(
                RawInput {
                    screen_rect: screen,
                    events,
                    ..RawInput::default()
                },
                |ui| {
                    let output = surface.show(ui, Path::new("/tmp/project"), &rows, None, false);
                    menu_open = output.response.context_menu_opened();
                    output.response.context_menu(|ui| {
                        let _ = ui.button("Action");
                    });
                },
            );
            menu_open
        };

        draw(Vec::new());
        draw(vec![
            Event::PointerMoved(pos2(100.0, 42.0)),
            Event::PointerButton {
                pos: pos2(100.0, 42.0),
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        draw(vec![Event::PointerButton {
            pos: pos2(100.0, 42.0),
            button: egui::PointerButton::Secondary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        }]);

        assert!(draw(vec![Event::PointerMoved(pos2(100.0, 70.0))]));
        assert_eq!(surface.hovered, None);
    }

    fn directory_row(name: &str, depth: usize) -> TreeRow {
        TreeRow {
            entry: TreeEntry {
                name: OsString::from(name),
                path: PathBuf::from(name),
                is_dir: true,
                is_symlink: false,
            },
            label: name.into(),
            depth,
            directory: true,
            expanded: true,
            revision: depth as u64 + 1,
        }
    }

    #[test]
    fn directory_row_glyphs_and_label_share_one_vertical_center() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows = [directory_row("src", 0)];
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(200.0, 80.0))),
                ..RawInput::default()
            },
            |ui| {
                surface.show(ui, Path::new("/tmp/project"), &rows, None, false);
            },
        );
        let mut glyphs = Rect::NOTHING;
        let mut label_center = None;
        for clipped in output.shapes {
            let bounds = clipped.shape.visual_bounding_rect();
            match &clipped.shape {
                // Below the root header, which paints a folder in the same ink.
                Shape::Path(path)
                    if path.stroke.color == egui::epaint::ColorMode::Solid(theme::text().muted)
                        && bounds.top() > theme::control::ROW =>
                {
                    glyphs = glyphs.union(bounds);
                }
                Shape::Text(text) if text.galley.text() == "src" => {
                    label_center = Some(text.pos.y + text.galley.size().y * 0.5);
                }
                _ => {}
            }
        }
        let label_center = label_center.expect("tree label");

        assert!(
            (glyphs.center().y - label_center).abs() < 0.6,
            "glyphs center on {} but the label on {label_center}",
            glyphs.center().y
        );
    }

    #[test]
    fn the_disclosure_chevron_never_lands_on_the_indent_guide_column() {
        for depth in 0..4 {
            let rect = Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0));
            let guide = super::column(rect, depth);
            let chevron = guide - super::CHEVRON_INSET;
            assert!(
                (guide - chevron).abs() > theme::stroke::DIVIDER + theme::stroke::ICON,
                "depth {depth} draws its chevron on top of its own guide"
            );
        }
    }

    #[test]
    fn the_top_level_chevron_keeps_a_breathing_gutter_from_the_left_border() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(200.0));
        let chevron_center = super::column(rect, 0) - super::CHEVRON_INSET;
        let chevron_left = chevron_center - crate::icons::GRID * 0.75 * 0.5;

        assert!(
            chevron_left - rect.left() >= theme::space::MEDIUM,
            "the depth-0 chevron glyph starts {}px from the border, wants at least {}px",
            chevron_left - rect.left(),
            theme::space::MEDIUM
        );
    }

    #[test]
    fn a_guide_is_only_drawn_while_the_parent_it_belongs_to_is_on_screen() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows = [
            directory_row("src", 0),
            directory_row("agent", 1),
            directory_row("controller", 2),
        ];
        // Small enough that scrolling can actually push the root out of view.
        let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(200.0, 68.0));
        let guides = |surface: &mut TreeSurface| {
            let output = context.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..RawInput::default()
                },
                |ui| {
                    surface.show(ui, Path::new("/tmp/project"), &rows, None, false);
                },
            );
            output
                .shapes
                .iter()
                .filter(|clipped| {
                    matches!(&clipped.shape, Shape::LineSegment { stroke, .. }
                        if stroke.color == theme::editor::indent_guide())
                })
                .count()
        };

        assert_eq!(guides(&mut surface), 3, "one for src, two for agent");
        surface.scroll_to_row(2, rows.len(), 40.0);
        assert!(
            guides(&mut surface) < 3,
            "a guide outlived the parent that justified it"
        );
    }

    #[test]
    fn a_long_name_truncates_to_the_panel_and_offers_the_full_path() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let long = "a_very_long_source_file_name_that_cannot_possibly_fit.rs";
        let rows = [TreeRow {
            entry: TreeEntry {
                name: OsString::from(long),
                path: PathBuf::from(long),
                is_dir: false,
                is_symlink: false,
            },
            label: long.into(),
            depth: 0,
            directory: false,
            expanded: false,
            revision: 1,
        }];
        let width = 180.0;
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(width, 80.0))),
                ..RawInput::default()
            },
            |ui| {
                surface.show(ui, Path::new("/tmp/project"), &rows, None, false);
            },
        );
        let label = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                Shape::Text(text) if text.galley.text() == long => Some(text.clone()),
                _ => None,
            })
            .expect("the row label");

        assert!(
            label.galley.elided,
            "a name wider than the panel hard-clipped"
        );
        assert!(label.galley.size().x <= width);
        assert!(
            label.galley.rows[0]
                .glyphs
                .last()
                .is_some_and(|glyph| glyph.chr == '…'),
            "truncation has to be visible as an ellipsis"
        );
    }

    #[test]
    fn a_selected_row_relayouts_its_label_instead_of_reusing_the_unselected_color() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows = [TreeRow {
            entry: TreeEntry {
                name: OsString::from("main.rs"),
                path: PathBuf::from("main.rs"),
                is_dir: false,
                is_symlink: false,
            },
            label: "main.rs".into(),
            depth: 0,
            directory: false,
            expanded: false,
            revision: 1,
        }];
        let screen = Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(200.0, 80.0));
        let mut draw = |selected| {
            let _ = context.run_ui(
                RawInput {
                    screen_rect: Some(screen),
                    ..RawInput::default()
                },
                |ui| {
                    surface.show(ui, Path::new("/tmp/project"), &rows, selected, false);
                },
            );
        };

        draw(None);
        draw(Some(0));
        let colors: Vec<_> = surface
            .labels
            .values()
            .map(|galley| galley.job.sections[0].format.color)
            .collect();

        assert_eq!(surface.labels.len(), 2, "one galley served both states");
        assert!(colors.contains(&theme::text().primary));
        assert!(colors.contains(&theme::text().secondary));
    }

    #[test]
    fn the_tree_names_the_open_project_above_its_rows() {
        let context = theme::test_context();
        let mut surface = TreeSurface::default();
        let rows = [directory_row("src", 0)];
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(220.0, 120.0))),
                ..RawInput::default()
            },
            |ui| {
                surface.show(ui, Path::new("/tmp/editur"), &rows, None, false);
            },
        );
        let text = |expected: &str| {
            output
                .shapes
                .iter()
                .find_map(|clipped| match &clipped.shape {
                    Shape::Text(text) if text.galley.text() == expected => Some(text.pos.y),
                    _ => None,
                })
        };

        assert!(
            text("editur").expect("project name") < text("src").expect("first row"),
            "the project header has to sit above the rows it describes"
        );
    }
}
