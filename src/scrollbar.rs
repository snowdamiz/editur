use egui::{Color32, Id, Rect, Sense, Ui, pos2};
use std::hash::{DefaultHasher, Hash, Hasher};

pub(crate) const WIDTH: f32 = 10.0;

#[derive(Clone, Copy, Debug)]
struct Geometry {
    track: Rect,
    thumb: Rect,
    max_scroll: f32,
}

fn geometry(viewport: Rect, content_height: f32, scroll_y: f32) -> Option<Geometry> {
    let viewport_height = viewport.height();
    if content_height <= viewport_height || viewport_height <= 0.0 {
        return None;
    }
    let track = Rect::from_min_max(
        pos2(viewport.right() - WIDTH + 2.0, viewport.top() + 2.0),
        pos2(viewport.right() - 2.0, viewport.bottom() - 2.0),
    );
    let max_scroll = content_height - viewport_height;
    let thumb_height =
        (track.height() * viewport_height / content_height).clamp(24.0, track.height());
    let travel = track.height() - thumb_height;
    let top = track.top() + scroll_y.clamp(0.0, max_scroll) / max_scroll * travel;
    Some(Geometry {
        track,
        thumb: Rect::from_min_max(
            pos2(track.left(), top),
            pos2(track.right(), top + thumb_height),
        ),
        max_scroll,
    })
}

fn geometry_revision(layout: Geometry, active: bool) -> u64 {
    let mut hasher = DefaultHasher::new();
    [
        layout.track.min.x,
        layout.track.min.y,
        layout.track.max.x,
        layout.track.max.y,
        layout.thumb.min.x,
        layout.thumb.min.y,
        layout.thumb.max.x,
        layout.thumb.max.y,
    ]
    .map(f32::to_bits)
    .hash(&mut hasher);
    active.hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn show(
    ui: &mut Ui,
    id: Id,
    viewport: Rect,
    content_height: f32,
    scroll_y: &mut f32,
    drag_offset: &mut Option<f32>,
) -> bool {
    let Some(mut layout) = geometry(viewport, content_height, *scroll_y) else {
        *drag_offset = None;
        return false;
    };
    let response = ui.interact(layout.track.expand(2.0), id, Sense::click_and_drag());
    let pointer = response.interact_pointer_pos();
    if (response.drag_started() || response.clicked())
        && let Some(pointer) = pointer
    {
        *drag_offset = Some(if layout.thumb.contains(pointer) {
            pointer.y - layout.thumb.top()
        } else {
            layout.thumb.height() * 0.5
        });
    }
    let mut changed = false;
    if (response.dragged() || response.clicked())
        && let (Some(pointer), Some(offset)) = (pointer, *drag_offset)
    {
        let travel = layout.track.height() - layout.thumb.height();
        let ratio = ((pointer.y - layout.track.top() - offset) / travel).clamp(0.0, 1.0);
        let next = ratio * layout.max_scroll;
        changed = (*scroll_y - next).abs() > f32::EPSILON;
        *scroll_y = next;
        layout = geometry(viewport, content_height, *scroll_y).expect("scrollbar remains visible");
    }
    if !ui.input(|input| input.pointer.primary_down()) {
        *drag_offset = None;
    }

    let painter = ui.painter_at(viewport);
    let active = response.hovered() || response.dragged();
    crate::renderer::mark_retained(
        &painter,
        viewport,
        id.value(),
        geometry_revision(layout, active),
    );
    painter.rect_filled(layout.track, 3.0, Color32::from_black_alpha(32));
    painter.rect_filled(
        layout.thumb,
        3.0,
        if active {
            Color32::from_rgb(112, 116, 128)
        } else {
            Color32::from_rgb(78, 82, 94)
        },
    );
    changed
}

#[cfg(test)]
mod tests {
    use super::geometry;
    use egui::{Id, RawInput, Rect, Vec2, pos2};

    fn retained_scrollbar(viewport: Rect) -> crate::renderer::RetainedPaint {
        let context = egui::Context::default();
        let mut scroll_y = 100.0;
        let mut drag_offset = None;
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(400.0))),
                ..RawInput::default()
            },
            |ui| {
                super::show(
                    ui,
                    Id::new("scrollbar"),
                    viewport,
                    1_000.0,
                    &mut scroll_y,
                    &mut drag_offset,
                );
            },
        );
        context
            .tessellate(output.shapes, output.pixels_per_point)
            .iter()
            .find_map(|primitive| crate::renderer::retained_paint(&primitive.primitive).unwrap())
            .unwrap()
    }

    #[test]
    fn scrollbar_thumb_tracks_the_visible_fraction_and_scroll_position() {
        let viewport = Rect::from_min_max(pos2(0.0, 100.0), pos2(500.0, 500.0));
        let top = geometry(viewport, 1_600.0, 0.0).unwrap();
        let middle = geometry(viewport, 1_600.0, 600.0).unwrap();
        let bottom = geometry(viewport, 1_600.0, 1_200.0).unwrap();

        assert_eq!(top.thumb.top(), top.track.top());
        assert_eq!(middle.thumb.center().y, middle.track.center().y);
        assert_eq!(bottom.thumb.bottom(), bottom.track.bottom());
        assert_eq!(bottom.max_scroll, 1_200.0);
    }

    #[test]
    fn retained_scrollbar_geometry_changes_when_its_viewport_moves() {
        let first = retained_scrollbar(Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 200.0)));
        let moved = retained_scrollbar(Rect::from_min_max(pos2(50.0, 0.0), pos2(150.0, 200.0)));

        assert_eq!(first.key, moved.key);
        assert_ne!(first.revision, moved.revision);
    }
}
