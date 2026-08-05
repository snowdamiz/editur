use egui::{Color32, FontId, Rect, Response, Sense, Stroke, Ui};
use std::{collections::HashMap, ops::Range, sync::Arc};

use crate::renderer::mark_retained;
use crate::tree::TreeEntry;

const ROW_HEIGHT: f32 = 22.0;

#[derive(Clone)]
pub struct TreeRow {
    pub entry: TreeEntry,
    pub label: String,
    pub depth: usize,
    pub directory: bool,
    pub expanded: bool,
    pub revision: u64,
}

#[derive(Default)]
pub struct TreeSurface {
    scroll_y: f32,
    scroll_drag_offset: Option<f32>,
    hovered: Option<usize>,
    labels: HashMap<u64, Arc<egui::Galley>>,
}

pub struct TreeOutput {
    pub response: Response,
    pub clicked: Option<usize>,
}

impl TreeSurface {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        rows: &[TreeRow],
        selected: Option<usize>,
        scroll_to_selected: bool,
    ) -> TreeOutput {
        let (id, rect) = ui.allocate_space(ui.available_size());
        let content = rect.with_max_x(rect.right() - crate::scrollbar::WIDTH);
        let response = ui.interact(content, id.with("tree"), Sense::click());
        let pointer = ui
            .input(|input| input.pointer.hover_pos())
            .filter(|pointer| content.contains(*pointer));
        if ui.input(|input| {
            input
                .pointer
                .hover_pos()
                .is_some_and(|pointer| rect.contains(pointer))
        }) {
            self.scroll_y -= ui.input(|input| input.smooth_scroll_delta.y);
        }
        if scroll_to_selected && let Some(selected) = selected {
            self.scroll_to_row(selected, rows.len(), rect.height());
        }
        self.clamp_scroll(rows.len(), rect.height());
        self.hovered = pointer.and_then(|pointer| self.row_at(pointer.y, rect, rows.len()));
        let clicked = response.clicked().then_some(self.hovered).flatten();

        let painter = ui.painter_at(rect);
        mark_retained(
            &painter,
            rect,
            0x4000_0000_0000_0000,
            u64::from(rect.width().to_bits()) << 32 | u64::from(rect.height().to_bits()),
        );
        painter.rect_filled(rect, 0.0, Color32::from_rgb(26, 27, 31));
        for index in self.visible_rows(rows.len(), rect.height()) {
            let row = &rows[index];
            let top = rect.top() + index as f32 * ROW_HEIGHT - self.scroll_y;
            let row_rect = Rect::from_min_size(
                egui::pos2(rect.left(), top),
                egui::vec2(content.width(), ROW_HEIGHT),
            );
            let fill = if selected == Some(index) {
                Color32::from_rgb(17, 103, 139)
            } else if self.hovered == Some(index) {
                Color32::from_rgb(38, 40, 46)
            } else {
                Color32::TRANSPARENT
            };
            let revision = row.revision
                ^ u64::from(top.to_bits())
                ^ (row.expanded as u64) << 33
                ^ (selected == Some(index)) as u64
                ^ ((self.hovered == Some(index)) as u64) << 1;
            mark_retained(
                &painter,
                rect,
                0x5000_0000_0000_0000 | index as u64,
                revision,
            );
            painter.rect_filled(row_rect, 3.0, fill);
            let marker_center = egui::pos2(
                rect.left() + 11.0 + row.depth as f32 * 14.0,
                row_rect.center().y,
            );
            if row.directory {
                let stroke = Stroke::new(1.2, Color32::from_rgb(160, 165, 177));
                if row.expanded {
                    painter.line_segment(
                        [
                            marker_center + egui::vec2(-3.5, -1.5),
                            marker_center + egui::vec2(0.0, 2.0),
                        ],
                        stroke,
                    );
                    painter.line_segment(
                        [
                            marker_center + egui::vec2(0.0, 2.0),
                            marker_center + egui::vec2(3.5, -1.5),
                        ],
                        stroke,
                    );
                } else {
                    painter.line_segment(
                        [
                            marker_center + egui::vec2(-1.5, -3.5),
                            marker_center + egui::vec2(2.0, 0.0),
                        ],
                        stroke,
                    );
                    painter.line_segment(
                        [
                            marker_center + egui::vec2(2.0, 0.0),
                            marker_center + egui::vec2(-1.5, 3.5),
                        ],
                        stroke,
                    );
                }
            }
            let label = self.labels.entry(row.revision).or_insert_with(|| {
                ui.fonts_mut(|fonts| {
                    fonts.layout_no_wrap(
                        row.label.clone(),
                        FontId::proportional(14.0),
                        Color32::from_rgb(190, 193, 202),
                    )
                })
            });
            painter.galley(
                egui::pos2(
                    marker_center.x + 9.0,
                    row_rect.center().y - label.size().y * 0.5,
                ),
                Arc::clone(label),
                Color32::from_rgb(190, 193, 202),
            );
        }
        if crate::scrollbar::show(
            ui,
            id.with("scrollbar"),
            rect,
            rows.len() as f32 * ROW_HEIGHT,
            &mut self.scroll_y,
            &mut self.scroll_drag_offset,
        ) {
            self.clamp_scroll(rows.len(), rect.height());
            ui.ctx().request_repaint();
        }

        TreeOutput { response, clicked }
    }

    pub fn visible_rows(&self, total: usize, viewport_height: f32) -> Range<usize> {
        let start = (self.scroll_y / ROW_HEIGHT).floor() as usize;
        let visible = (viewport_height / ROW_HEIGHT).ceil() as usize + 1;
        start.min(total)..(start + visible).min(total)
    }

    pub fn scroll_to_row(&mut self, row: usize, total: usize, viewport_height: f32) {
        let top = row as f32 * ROW_HEIGHT;
        let bottom = top + ROW_HEIGHT;
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
            .then_some((document_y / ROW_HEIGHT).floor() as usize)
            .filter(|index| *index < total)
    }

    fn clamp_scroll(&mut self, total: usize, viewport_height: f32) {
        self.scroll_y = self
            .scroll_y
            .clamp(0.0, (total as f32 * ROW_HEIGHT - viewport_height).max(0.0));
    }
}

#[cfg(test)]
mod tests {
    use super::{TreeRow, TreeSurface};
    use crate::tree::TreeEntry;
    use egui::{Event, RawInput, Rect, Vec2, pos2};
    use std::{ffi::OsString, path::PathBuf};

    #[test]
    fn retained_tree_limits_work_to_visible_rows_and_keeps_selection_in_view() {
        let mut surface = TreeSurface::default();
        surface.scroll_to_row(50, 100, 220.0);

        let visible = surface.visible_rows(100, 220.0);
        assert!(visible.contains(&50));
        assert!(visible.len() <= 13);
    }

    #[test]
    fn pointer_movement_updates_the_retained_hover_row_immediately() {
        let context = egui::Context::default();
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
                events: vec![Event::PointerMoved(pos2(100.0, 11.0))],
                ..RawInput::default()
            },
            |ui| {
                surface.show(ui, &rows, None, false);
            },
        );

        assert_eq!(surface.hovered, Some(0));
    }
}
