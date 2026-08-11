//! Motion is feedback, not decoration. Nothing here costs a frame when idle,
//! and every animated value is quantized so the retained renderer sees a
//! bounded number of distinct frames rather than a new float every frame.

use egui::{Color32, Context, Id, emath::easing};

/// Hover and press fills, icon color, tab activation.
pub(crate) const FAST: f32 = 0.090;
/// Dialog, palette, and menu appearance.
pub(crate) const BASE: f32 = 0.140;
/// Sidebar and agent panel width, the agentic-mode transition.
pub(crate) const SLOW: f32 = 0.200;

/// An animated value is snapped to this many steps before it is used, so an
/// in-flight animation cannot invalidate the retained cache on every frame
/// with a difference nobody can see.
const STEPS: f32 = 64.0;

fn reduced_id() -> Id {
    Id::new("editur-reduced-motion")
}

pub(crate) fn set_reduced(ctx: &Context, reduced: bool) {
    ctx.data_mut(|data| data.insert_temp(reduced_id(), reduced));
}

pub(crate) fn reduced(ctx: &Context) -> bool {
    ctx.data(|data| data.get_temp::<bool>(reduced_id()).unwrap_or(false))
}

fn quantize(value: f32) -> f32 {
    (value * STEPS).round() / STEPS
}

/// Drives `target` over `duration`, returning quantized 0.0..=1.0 progress.
pub(crate) fn animate(ctx: &Context, id: Id, target: bool, duration: f32) -> f32 {
    if reduced(ctx) {
        return f32::from(u8::from(target));
    }
    let easing = if duration >= SLOW {
        easing::cubic_in_out
    } else {
        easing::cubic_out
    };
    quantize(ctx.animate_bool_with_time_and_easing(id, target, duration, easing))
}

/// Interpolates a fill between absent and `overlay` along `progress`.
pub(crate) fn fade(overlay: Color32, progress: f32) -> Color32 {
    if progress <= 0.0 {
        Color32::TRANSPARENT
    } else if progress >= 1.0 {
        overlay
    } else {
        overlay.gamma_multiply(progress)
    }
}

#[cfg(test)]
mod tests {
    use super::{BASE, FAST, animate, set_reduced};
    use egui::{Id, RawInput, Rect, Vec2, ViewportId, pos2};

    fn frame(context: &egui::Context, time: f64, target: bool, duration: f32) -> (f32, bool) {
        let mut value = 0.0;
        let output = context.run_ui(
            RawInput {
                screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(100.0))),
                time: Some(time),
                ..RawInput::default()
            },
            |ui| value = animate(ui.ctx(), Id::new("motion"), target, duration),
        );
        let repaints = output.viewport_output[&ViewportId::ROOT]
            .repaint_delay
            .is_zero();
        (value, repaints)
    }

    /// egui asks for a repaint on its own opening frames, so a test can only
    /// speak about idleness once the context has gone quiet by itself.
    fn settle(context: &egui::Context, duration: f32) -> f64 {
        let mut time = 0.0;
        for _ in 0..16 {
            if !frame(context, time, false, duration).1 {
                return time;
            }
            time += f64::from(duration);
        }
        panic!("the context never went idle on its own");
    }

    #[test]
    fn an_animation_settles_inside_its_stated_duration_and_then_stops_repainting() {
        let context = egui::Context::default();
        let idle = settle(&context, FAST);

        // The flip frame is always the frame that handled the input which
        // caused it, so egui leaves scheduling the next one to that input and
        // only starts asking for time once the value is actually in flight.
        let (start, _) = frame(&context, idle, true, FAST);
        let (midway, midway_repaints) = frame(&context, idle + f64::from(FAST) / 2.0, true, FAST);

        assert_eq!(start, 0.0, "an animation has to begin where it was");
        assert!(
            midway > 0.0 && midway < 1.0,
            "expected an in-flight value, got {midway}"
        );
        assert!(
            midway_repaints,
            "an in-flight animation has to keep painting"
        );

        let mut time = idle + f64::from(FAST) * 1.5;
        let mut settled = 0.0;
        for _ in 0..8 {
            let (value, repaints) = frame(&context, time, true, FAST);
            settled = value;
            if !repaints {
                assert_eq!(settled, 1.0, "the animation idled before reaching target");
                return;
            }
            time += f64::from(FAST);
        }
        panic!("a settled animation must let the app idle, stuck at {settled}");
    }

    #[test]
    fn reduced_motion_jumps_straight_to_the_target_without_scheduling_a_frame() {
        let context = egui::Context::default();
        let idle = settle(&context, BASE);
        set_reduced(&context, true);

        let (value, repaints) = frame(&context, idle, true, BASE);
        let (midway, midway_repaints) = frame(&context, idle + f64::from(BASE) / 2.0, true, BASE);

        assert_eq!((value, midway), (1.0, 1.0));
        assert!(
            !repaints && !midway_repaints,
            "reduced motion must not cost a frame"
        );
    }
}
