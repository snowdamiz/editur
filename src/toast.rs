//! Non-blocking failures. A save error, an LSP crash, or an agent transport
//! drop needs to be seen, not answered, so it arrives here instead of in a
//! modal that stops the person typing.

use egui::{Align2, Context, Id, Rect, Sense, Ui, UiBuilder};
use std::time::Duration;

use crate::{
    dialog::Severity,
    icons::{self, Icon},
    theme,
};

/// How long a toast lives once nothing is holding it open.
const LIFETIME: f32 = 6.0;
/// Older toasts collapse into a count rather than filling the corner.
const VISIBLE: usize = 3;
const WIDTH: f32 = 320.0;
const RAIL: f32 = 3.0;

pub(crate) struct Toast {
    id: u64,
    severity: Severity,
    message: String,
    action: Option<String>,
    remaining: f32,
}

#[derive(Default)]
pub(crate) struct Toasts {
    items: Vec<Toast>,
    next_id: u64,
    /// Wall-clock time of the last frame. Toast lifetimes are measured against
    /// the clock rather than against a frame count, so a slow frame does not
    /// shorten them.
    last: Option<f64>,
}

impl Toasts {
    pub(crate) fn push(&mut self, severity: Severity, message: impl Into<String>) -> u64 {
        self.push_with_action(severity, message, None::<String>)
    }

    pub(crate) fn push_with_action(
        &mut self,
        severity: Severity,
        message: impl Into<String>,
        action: Option<impl Into<String>>,
    ) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(Toast {
            id,
            severity,
            message: message.into(),
            action: action.map(Into::into),
            remaining: LIFETIME,
        });
        id
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// Draws the stack and returns the message of a toast whose action was
    /// clicked, if any.
    pub(crate) fn show(&mut self, ctx: &Context) -> Option<String> {
        if self.items.is_empty() {
            return None;
        }
        let now = ctx.input(|input| input.time);
        let delta = (now - self.last.unwrap_or(now)).clamp(0.0, 0.5) as f32;
        self.last = Some(now);
        let screen = ctx.content_rect();
        let mut clicked = None;
        let mut dismissed = Vec::new();
        let mut repaint_after: Option<f32> = None;
        let hidden = self.items.len().saturating_sub(VISIBLE);
        let mut cursor = screen.bottom() - theme::space::LARGE;
        let pointer = ctx.pointer_hover_pos();

        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            Id::new("toasts"),
        ));
        if hidden > 0 {
            let label = format!("{hidden} more");
            let galley =
                painter.layout_no_wrap(label, theme::typography::small(), theme::text().muted);
            let position = egui::pos2(
                screen.right() - theme::space::LARGE - galley.size().x,
                cursor - galley.size().y,
            );
            painter.galley(position, galley, theme::text().muted);
            cursor = position.y - theme::space::SMALL;
        }

        for toast in self.items.iter_mut().rev().take(VISIBLE) {
            let height = toast_height(ctx, toast);
            let rect = Rect::from_min_size(
                egui::pos2(
                    screen.right() - theme::space::LARGE - WIDTH,
                    cursor - height,
                ),
                egui::vec2(WIDTH, height),
            );
            cursor = rect.top() - theme::space::SMALL;

            let ui = Ui::new(
                ctx.clone(),
                Id::new(("toast", toast.id)),
                UiBuilder::new()
                    .layer_id(egui::LayerId::new(
                        egui::Order::Foreground,
                        Id::new(("toast", toast.id)),
                    ))
                    .max_rect(rect)
                    .sense(Sense::click()),
            );
            // A floating overlay's hover is a geometric fact, and reading it
            // that way keeps the toast out of egui's interaction ordering,
            // where a foreground layer would otherwise steal hover from the
            // dialog or menu it is standing in front of.
            let held = pointer.is_some_and(|position| rect.contains(position));
            if !held {
                toast.remaining -= delta;
            }
            if toast.remaining <= 0.0 {
                dismissed.push(toast.id);
            } else if !held {
                let delay = toast.remaining.min(0.5);
                repaint_after = Some(repaint_after.map_or(delay, |current| current.min(delay)));
            }

            let painter = ui.painter();
            painter
                .add(theme::shadow::popover().as_shape(rect, theme::corner(theme::radius::CARD)));
            painter.rect_filled(
                rect,
                theme::corner(theme::radius::CARD),
                theme::surface().raised,
            );
            painter.rect_stroke(
                rect,
                theme::corner(theme::radius::CARD),
                theme::border::strong(),
                egui::StrokeKind::Inside,
            );
            painter.rect_filled(
                Rect::from_min_size(rect.left_top(), egui::vec2(RAIL, rect.height())),
                theme::corner(theme::radius::CARD),
                severity_color(toast.severity),
            );
            let glyph = Rect::from_min_size(
                egui::pos2(
                    rect.left() + theme::space::MEDIUM,
                    rect.top() + theme::space::MEDIUM,
                ),
                egui::Vec2::splat(icons::GRID),
            );
            icons::paint(
                painter,
                severity_icon(toast.severity),
                glyph,
                severity_color(toast.severity),
            );

            let text_left = glyph.right() + theme::space::SMALL;
            let text_width = rect.right() - theme::space::MEDIUM - text_left;
            let galley = painter.layout(
                toast.message.clone(),
                theme::typography::body(),
                theme::text().primary,
                text_width,
            );
            painter.galley(
                egui::pos2(text_left, rect.top() + theme::space::MEDIUM),
                galley.clone(),
                theme::text().primary,
            );

            if let Some(action) = toast.action.as_deref() {
                let size = egui::vec2(72.0, theme::control::COMPACT);
                let button = Rect::from_min_size(
                    egui::pos2(
                        rect.right() - theme::space::MEDIUM - size.x,
                        rect.bottom() - theme::space::MEDIUM - size.y,
                    ),
                    size,
                );
                let action_response =
                    ui.interact(button, Id::new(("toast_action", toast.id)), Sense::click());
                let painter = ui.painter();
                painter.rect_filled(
                    button,
                    theme::corner(theme::radius::CONTROL),
                    if action_response.hovered() {
                        theme::state::hover()
                    } else {
                        theme::state::selected()
                    },
                );
                painter.text(
                    button.center(),
                    Align2::CENTER_CENTER,
                    action,
                    theme::typography::small(),
                    theme::text().primary,
                );
                if action_response.clicked() {
                    clicked = Some(toast.message.clone());
                    dismissed.push(toast.id);
                }
            }
        }

        self.items.retain(|toast| !dismissed.contains(&toast.id));
        if !self.items.is_empty()
            && let Some(delay) = repaint_after
        {
            ctx.request_repaint_after(Duration::from_secs_f32(delay));
        }
        clicked
    }
}

fn toast_height(ctx: &Context, toast: &Toast) -> f32 {
    let text_width = WIDTH - theme::space::MEDIUM * 2.0 - icons::GRID - theme::space::SMALL;
    let lines = ctx.fonts_mut(|fonts| {
        fonts
            .layout(
                toast.message.clone(),
                theme::typography::body(),
                theme::text().primary,
                text_width,
            )
            .size()
            .y
    });
    let action = if toast.action.is_some() {
        theme::control::COMPACT + theme::space::SMALL
    } else {
        0.0
    };
    (lines + action + theme::space::MEDIUM * 2.0).max(theme::control::PRIMARY + theme::space::SMALL)
}

fn severity_color(severity: Severity) -> egui::Color32 {
    let semantic = theme::semantic();
    match severity {
        Severity::Neutral => theme::text().muted,
        Severity::Info => semantic.info,
        Severity::Warning => semantic.warning,
        Severity::Danger => semantic.danger,
    }
}

fn severity_icon(severity: Severity) -> Icon {
    match severity {
        Severity::Neutral | Severity::Info => Icon::Info,
        Severity::Warning => Icon::Warning,
        Severity::Danger => Icon::Error,
    }
}

#[cfg(test)]
mod tests {
    use super::{LIFETIME, Toasts};
    use crate::{dialog::Severity, theme};
    use egui::{Event, RawInput, Rect, Vec2, ViewportId, pos2};
    use std::time::Duration;

    fn frames(
        toasts: &mut Toasts,
        context: &egui::Context,
        seconds: f64,
        pointer: Option<egui::Pos2>,
    ) {
        let step = 0.25;
        let mut elapsed = 0.0;
        while elapsed < seconds {
            let _ = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(900.0, 600.0))),
                    time: Some(elapsed),
                    events: pointer.map(Event::PointerMoved).into_iter().collect(),
                    ..RawInput::default()
                },
                |ui| {
                    toasts.show(ui.ctx());
                },
            );
            elapsed += step;
        }
    }

    #[test]
    fn a_toast_expires_on_its_own_and_a_hovered_one_waits() {
        let context = theme::test_context();
        let mut toasts = Toasts::default();
        toasts.push(Severity::Danger, "cannot save main.rs: permission denied");

        frames(&mut toasts, &context, f64::from(LIFETIME) + 1.0, None);
        assert!(toasts.is_empty(), "a toast has to clear itself");

        let mut toasts = Toasts::default();
        toasts.push(Severity::Danger, "cannot save main.rs: permission denied");
        frames(
            &mut toasts,
            &context,
            f64::from(LIFETIME) + 1.0,
            Some(pos2(700.0, 565.0)),
        );
        assert_eq!(
            toasts.len(),
            1,
            "a toast the pointer is reading must not vanish mid-sentence"
        );
    }

    #[test]
    fn a_visible_toast_schedules_expiry_without_spinning_repaints() {
        let context = theme::test_context();
        for time in 0..16 {
            let output = context.run_ui(
                RawInput {
                    time: Some(f64::from(time)),
                    ..RawInput::default()
                },
                |_| {},
            );
            if output.viewport_output[&ViewportId::ROOT].repaint_delay == Duration::MAX {
                break;
            }
        }
        let mut toasts = Toasts::default();
        toasts.push(Severity::Info, "saved");
        let output = context.run_ui(RawInput::default(), |ui| {
            toasts.show(ui.ctx());
        });
        let delay = output.viewport_output[&ViewportId::ROOT].repaint_delay;

        assert!(
            delay > Duration::ZERO,
            "toast requested an immediate repaint"
        );
        assert!(
            delay <= Duration::from_millis(500),
            "toast expiry was not scheduled"
        );
    }
}
