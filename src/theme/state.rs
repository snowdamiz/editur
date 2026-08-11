//! Interaction states expressed as alpha overlays rather than absolute fills,
//! so the same gesture reads identically on every surface it lands on.

use egui::Color32;

use super::color;

/// The ink an overlay is made of: white on a dark palette, black on a light
/// one, so "brighter on hover" becomes "more contrast on hover" in both.
fn overlay(alpha: u8) -> Color32 {
    if color::palette().dark {
        Color32::from_white_alpha(alpha)
    } else {
        Color32::from_black_alpha(alpha)
    }
}

/// Pointer over any row, tab, or icon button.
pub(crate) fn hover() -> Color32 {
    overlay(15)
}

/// Active press.
pub(crate) fn press() -> Color32 {
    overlay(26)
}

/// Selected row in an unfocused list.
pub(crate) fn selected() -> Color32 {
    overlay(31)
}

/// Selected row in the focused list. Accent is reserved for exactly this.
pub(crate) fn selected_focus() -> Color32 {
    color::accent().gamma_multiply(0.16)
}

pub(crate) fn fill(selected_row: bool, focused: bool, hovered: bool, pressed: bool) -> Color32 {
    if selected_row {
        if focused {
            selected_focus()
        } else {
            selected()
        }
    } else if pressed {
        press()
    } else if hovered {
        hover()
    } else {
        Color32::TRANSPARENT
    }
}

/// Dims the whole window behind a modal so it reads as modal.
pub(crate) fn scrim() -> Color32 {
    color::palette().surface.sunken.gamma_multiply(0.45)
}

pub(crate) mod border {
    use egui::{Color32, Stroke};

    use super::super::{color, metrics::stroke};

    /// Chrome-to-content dividers, tab separators.
    pub(crate) fn hairline_color() -> Color32 {
        super::overlay(20)
    }

    /// Dialog, menu, and palette outlines.
    pub(crate) fn strong_color() -> Color32 {
        super::overlay(36)
    }

    /// Keyboard focus ring, focused pane.
    pub(crate) fn focus_color() -> Color32 {
        color::accent().gamma_multiply(0.55)
    }

    pub(crate) fn hairline() -> Stroke {
        Stroke::new(stroke::DIVIDER, hairline_color())
    }

    pub(crate) fn strong() -> Stroke {
        Stroke::new(stroke::DIVIDER, strong_color())
    }

    pub(crate) fn focus_ring() -> Stroke {
        Stroke::new(stroke::FOCUS, focus_color())
    }
}

/// A wash that darkens whatever it lands on in both themes, for the one case
/// where "recessed" has to read the same on a light surface as on a dark one.
pub(crate) fn shade(alpha: u8) -> Color32 {
    Color32::from_black_alpha(alpha)
}

/// The overlay scrollbars. Grays derived from the chrome so the bar belongs to
/// the surface it floats over rather than to a fixed gray ramp.
pub(crate) mod scrollbar {
    use egui::Color32;

    use super::super::color;

    pub(crate) fn track() -> Color32 {
        super::shade(32)
    }

    pub(crate) fn thumb() -> Color32 {
        color::mix(color::surface().chrome, color::text().muted, 0.5)
    }

    pub(crate) fn thumb_active() -> Color32 {
        color::mix(color::surface().chrome, color::text().muted, 0.75)
    }
}

/// A tinted block — a diff row, an error banner, a status pill — expressed as
/// one recipe so every semantic color produces the same shape of surface.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct Callout {
    pub(crate) fill: Color32,
    pub(crate) border: Color32,
    pub(crate) text: Color32,
}

/// `color` is the semantic hue; the block is a 16% wash of it, a 35% border,
/// and text pushed toward the page so it stays readable on the wash.
pub(crate) fn callout(color: Color32) -> Callout {
    Callout {
        fill: color::composite(color::subtle(color), color::surface().raised),
        border: color.gamma_multiply(0.35),
        text: color::ink(color),
    }
}

/// The diff colors, which are the callout recipe applied to success and danger.
pub(crate) mod diff {
    use egui::Color32;

    use super::super::color;

    pub(crate) fn added() -> Color32 {
        color::composite(
            color::subtle(color::semantic().success),
            color::surface().editor,
        )
    }

    pub(crate) fn removed() -> Color32 {
        color::composite(
            color::subtle(color::semantic().danger),
            color::surface().editor,
        )
    }

    pub(crate) fn added_ink() -> Color32 {
        color::ink(color::semantic().success)
    }

    pub(crate) fn removed_ink() -> Color32 {
        color::ink(color::semantic().danger)
    }
}

/// Reading affordances that only the document surface needs.
pub(crate) mod editor {
    use egui::Color32;

    use super::super::color;

    /// Replaces the 1.1:1 fill that made selecting a paragraph invisible.
    pub(crate) fn selection() -> Color32 {
        color::accent().gamma_multiply(0.22)
    }

    /// The same selection in an unfocused pane.
    pub(crate) fn selection_inactive() -> Color32 {
        color::accent().gamma_multiply(0.11)
    }

    /// Replaces the invisible 2.4% white.
    pub(crate) fn line_active() -> Color32 {
        super::overlay(13)
    }

    pub(crate) fn indent_guide() -> Color32 {
        super::overlay(15)
    }

    /// The guide for the block that encloses the caret.
    pub(crate) fn indent_guide_active() -> Color32 {
        color::accent().gamma_multiply(0.30)
    }
}

#[cfg(test)]
mod tests {
    use super::{fill, hover, selected_focus};

    #[test]
    fn a_focused_list_marks_its_selection_with_accent_and_an_unfocused_one_does_not() {
        let focused = fill(true, true, false, false);
        let unfocused = fill(true, false, false, false);

        assert_eq!(focused, selected_focus());
        assert!(
            focused.b() > focused.r(),
            "the focused fill must read as cyan"
        );
        assert_eq!(unfocused.r(), unfocused.b(), "an unfocused fill is neutral");
        assert_eq!(fill(false, false, true, false), hover());
    }
}
