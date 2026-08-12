use crate::theme;
use egui::{Id, Rect, Sense, Ui, pos2};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::time::Duration;

pub(crate) const WIDTH: f32 = 10.0;
pub(crate) const HOLD_SECONDS: f64 = 0.25;
pub(crate) const FADE_SECONDS: f64 = 0.18;

#[derive(Default)]
pub(crate) struct Activity {
    last_active: Option<f64>,
}

#[derive(Default)]
pub(crate) struct State {
    drag_offset: Option<f32>,
    activity: Activity,
}

impl Activity {
    fn opacity(&mut self, ui: &Ui, active: bool) -> f32 {
        let now = ui.input(|input| input.time);
        if active {
            self.last_active = Some(now);
        }
        let opacity = opacity_at(self.last_active, now);
        if opacity > 0.0 && !active {
            ui.ctx().request_repaint_after(Duration::from_millis(16));
        }
        opacity
    }

    pub(crate) fn style_egui(&mut self, ui: &mut Ui) {
        let scrolling = ui.input(|input| input.smooth_scroll_delta != egui::Vec2::ZERO);
        let opacity = self.opacity(ui, scrolling);
        let scroll = &mut ui.spacing_mut().scroll;
        scroll.floating = true;
        scroll.floating_allocated_width = 0.0;
        scroll.dormant_background_opacity = 0.0;
        scroll.dormant_handle_opacity = 0.0;
        scroll.active_background_opacity = 0.0;
        scroll.active_handle_opacity = opacity * 0.75;
        scroll.interact_background_opacity = 0.15;
        scroll.interact_handle_opacity = 1.0;
    }
}

fn opacity_at(last_active: Option<f64>, now: f64) -> f32 {
    let Some(elapsed) = last_active.map(|last| (now - last).max(0.0)) else {
        return 0.0;
    };
    if elapsed <= HOLD_SECONDS {
        1.0
    } else if elapsed >= HOLD_SECONDS + FADE_SECONDS {
        0.0
    } else {
        let opacity = 1.0 - (elapsed - HOLD_SECONDS) / FADE_SECONDS;
        if opacity <= f32::EPSILON as f64 {
            0.0
        } else {
            opacity as f32
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Geometry {
    track: Rect,
    thumb: Rect,
    max_scroll: f32,
}

#[cfg(test)]
fn geometry(viewport: Rect, content_length: f32, scroll: f32) -> Option<Geometry> {
    axis_geometry(viewport, content_length, scroll, false)
}

fn axis_geometry(
    viewport: Rect,
    content_length: f32,
    scroll: f32,
    horizontal: bool,
) -> Option<Geometry> {
    let viewport_length = if horizontal {
        viewport.width()
    } else {
        viewport.height()
    };
    if content_length <= viewport_length || viewport_length <= 0.0 {
        return None;
    }
    let track = if horizontal {
        Rect::from_min_max(
            pos2(viewport.left() + 2.0, viewport.bottom() - WIDTH + 2.0),
            pos2(viewport.right() - 2.0, viewport.bottom() - 2.0),
        )
    } else {
        Rect::from_min_max(
            pos2(viewport.right() - WIDTH + 2.0, viewport.top() + 2.0),
            pos2(viewport.right() - 2.0, viewport.bottom() - 2.0),
        )
    };
    let max_scroll = content_length - viewport_length;
    let track_length = if horizontal {
        track.width()
    } else {
        track.height()
    };
    let thumb_length = (track_length * viewport_length / content_length).clamp(24.0, track_length);
    let travel = track_length - thumb_length;
    let track_start = if horizontal {
        track.left()
    } else {
        track.top()
    };
    let start = track_start + scroll.clamp(0.0, max_scroll) / max_scroll * travel;
    let thumb = if horizontal {
        Rect::from_min_max(
            pos2(start, track.top()),
            pos2(start + thumb_length, track.bottom()),
        )
    } else {
        Rect::from_min_max(
            pos2(track.left(), start),
            pos2(track.right(), start + thumb_length),
        )
    };
    Some(Geometry {
        track,
        thumb,
        max_scroll,
    })
}

fn geometry_revision(layout: Geometry, active: bool, opacity: f32) -> u64 {
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
    opacity.to_bits().hash(&mut hasher);
    hasher.finish()
}

pub(crate) fn show(
    ui: &mut Ui,
    id: Id,
    viewport: Rect,
    content_height: f32,
    scroll_y: &mut f32,
    state: &mut State,
    scrolling: bool,
) -> bool {
    show_axis(
        ui,
        id,
        viewport,
        content_height,
        scroll_y,
        state,
        scrolling,
        false,
    )
}

pub(crate) fn show_horizontal(
    ui: &mut Ui,
    id: Id,
    viewport: Rect,
    content_width: f32,
    scroll_x: &mut f32,
    state: &mut State,
    scrolling: bool,
) -> bool {
    show_axis(
        ui,
        id,
        viewport,
        content_width,
        scroll_x,
        state,
        scrolling,
        true,
    )
}

// The shared renderer keeps vertical and horizontal axis state explicit.
#[expect(clippy::too_many_arguments)]
fn show_axis(
    ui: &mut Ui,
    id: Id,
    viewport: Rect,
    content_length: f32,
    scroll: &mut f32,
    state: &mut State,
    scrolling: bool,
    horizontal: bool,
) -> bool {
    let Some(mut layout) = axis_geometry(viewport, content_length, *scroll, horizontal) else {
        state.drag_offset = None;
        return false;
    };
    let response = ui.interact(layout.track.expand(2.0), id, Sense::click_and_drag());
    let pointer = response.interact_pointer_pos();
    if (response.drag_started() || response.clicked())
        && let Some(pointer) = pointer
    {
        state.drag_offset = Some(if layout.thumb.contains(pointer) {
            if horizontal {
                pointer.x - layout.thumb.left()
            } else {
                pointer.y - layout.thumb.top()
            }
        } else {
            (if horizontal {
                layout.thumb.width()
            } else {
                layout.thumb.height()
            }) * 0.5
        });
    }
    let mut changed = false;
    if (response.dragged() || response.clicked())
        && let (Some(pointer), Some(offset)) = (pointer, state.drag_offset)
    {
        let (pointer, track_start, travel) = if horizontal {
            (
                pointer.x,
                layout.track.left(),
                layout.track.width() - layout.thumb.width(),
            )
        } else {
            (
                pointer.y,
                layout.track.top(),
                layout.track.height() - layout.thumb.height(),
            )
        };
        let ratio = ((pointer - track_start - offset) / travel).clamp(0.0, 1.0);
        let next = ratio * layout.max_scroll;
        changed = (*scroll - next).abs() > f32::EPSILON;
        *scroll = next;
        layout = axis_geometry(viewport, content_length, *scroll, horizontal)
            .expect("scrollbar remains visible");
    }
    if !ui.input(|input| input.pointer.primary_down()) {
        state.drag_offset = None;
    }

    let active = response.hovered() || response.dragged();
    let opacity = state.activity.opacity(ui, scrolling || active);
    if opacity <= 0.0 {
        return changed;
    }
    let painter = ui.painter_at(viewport);
    crate::renderer::mark_retained(
        &painter,
        viewport,
        id.value(),
        geometry_revision(layout, active, opacity),
    );
    painter.rect_filled(
        layout.track,
        theme::corner(theme::radius::ROW),
        theme::state::scrollbar::track().gamma_multiply(opacity),
    );
    painter.rect_filled(
        layout.thumb,
        theme::corner(theme::radius::ROW),
        if active {
            theme::state::scrollbar::thumb_active()
        } else {
            theme::state::scrollbar::thumb()
        }
        .gamma_multiply(opacity),
    );
    changed
}

#[cfg(test)]
mod tests {
    use super::{FADE_SECONDS, HOLD_SECONDS, axis_geometry, geometry, opacity_at};
    use egui::{Id, RawInput, Rect, Vec2, pos2};

    fn retained_scrollbar(viewport: Rect) -> crate::renderer::RetainedPaint {
        let context = crate::theme::test_context();
        let mut scroll_y = 100.0;
        let mut state = super::State::default();
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
                    &mut state,
                    true,
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

        let left = axis_geometry(viewport, 1_600.0, 0.0, true).unwrap();
        let right = axis_geometry(viewport, 1_600.0, 1_100.0, true).unwrap();
        assert_eq!(left.thumb.left(), left.track.left());
        assert_eq!(right.thumb.right(), right.track.right());
        assert_eq!(right.max_scroll, 1_100.0);
    }

    #[test]
    fn retained_scrollbar_geometry_changes_when_its_viewport_moves() {
        let first = retained_scrollbar(Rect::from_min_max(pos2(0.0, 0.0), pos2(100.0, 200.0)));
        let moved = retained_scrollbar(Rect::from_min_max(pos2(50.0, 0.0), pos2(150.0, 200.0)));

        assert_eq!(first.key, moved.key);
        assert_ne!(first.revision, moved.revision);
    }

    #[test]
    fn scrollbar_is_hidden_until_activity_then_fades_out() {
        assert_eq!(opacity_at(None, 10.0), 0.0);
        assert_eq!(opacity_at(Some(10.0), 10.0), 1.0);
        assert_eq!(opacity_at(Some(10.0), 10.0 + HOLD_SECONDS), 1.0);
        assert_eq!(
            opacity_at(Some(10.0), 10.0 + HOLD_SECONDS + FADE_SECONDS),
            0.0
        );
    }
}
