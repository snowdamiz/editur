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
        strong_color_for(color::palette())
    }

    pub(super) fn strong_color_for(palette: color::Palette) -> Color32 {
        if palette.dark {
            Color32::from_white_alpha(36)
        } else {
            Color32::from_black_alpha(112)
        }
    }

    /// Keyboard focus ring, focused pane.
    pub(crate) fn focus_color() -> Color32 {
        focus_color_for(color::palette())
    }

    pub(super) fn focus_color_for(palette: color::Palette) -> Color32 {
        if palette.dark {
            palette.accent.gamma_multiply(0.55)
        } else {
            palette.accent
        }
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

/// The diff colors. Dark mode uses a deep tint; light mode uses the same pale
/// semantic wash as callouts so green and red never become muddy blocks.
pub(crate) mod diff {
    use egui::Color32;

    use super::super::color;

    pub(super) fn wash_for(palette: color::Palette, semantic: Color32) -> Color32 {
        if palette.dark {
            color::mix(
                color::mix(palette.surface.editor, palette.surface.sunken, 0.35),
                semantic,
                0.25,
            )
        } else {
            color::composite(color::subtle(semantic), palette.surface.editor)
        }
    }

    fn wash(semantic: Color32) -> Color32 {
        wash_for(color::palette(), semantic)
    }

    /// A syntax color on a tinted row: pushed toward the page the same way
    /// ink is, because hues tuned for the editor surface lose their contrast
    /// once a green or red wash sits underneath them.
    pub(crate) fn code(color: Color32) -> Color32 {
        color::ink(color)
    }

    pub(crate) fn added() -> Color32 {
        wash(color::semantic().success)
    }

    pub(crate) fn removed() -> Color32 {
        wash(color::semantic().danger)
    }

    pub(crate) fn added_ink() -> Color32 {
        color::ink(color::semantic().success)
    }

    pub(crate) fn removed_ink() -> Color32 {
        color::ink(color::semantic().danger)
    }

    /// Gutter numbers on tinted rows: the row's ink pulled partway toward the
    /// wash, so numbers stay legible on green/red without competing with the
    /// code beside them.
    pub(crate) fn added_number() -> Color32 {
        color::mix(added(), added_ink(), 0.65)
    }

    pub(crate) fn removed_number() -> Color32 {
        color::mix(removed(), removed_ink(), 0.65)
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
    use super::super::color;
    use super::{border, diff, fill, hover, selected_focus};

    #[test]
    fn light_focus_ring_clears_non_text_contrast() {
        let rendered = color::composite(
            border::focus_color_for(color::LIGHT),
            color::LIGHT.surface.raised,
        );

        assert!(
            color::contrast_ratio(rendered, color::LIGHT.surface.raised) >= 3.0,
            "focus ring is not distinguishable on a light surface: {rendered:?}"
        );
    }

    #[test]
    fn light_strong_borders_clear_non_text_contrast() {
        let rendered = color::composite(
            border::strong_color_for(color::LIGHT),
            color::LIGHT.surface.raised,
        );

        assert!(
            color::contrast_ratio(rendered, color::LIGHT.surface.raised) >= 3.0,
            "control border is not distinguishable on a light surface: {rendered:?}"
        );
    }

    #[test]
    fn light_diff_rows_use_pale_semantic_washes() {
        let added = diff::wash_for(color::LIGHT, color::LIGHT.semantic.success);
        let removed = diff::wash_for(color::LIGHT, color::LIGHT.semantic.danger);

        assert!(
            added.r() >= 200
                && added.g() >= added.r() + 10
                && removed.g() >= 200
                && removed.r() >= removed.g() + 15,
            "light semantic rows are muddy instead of pale: {added:?}, {removed:?}"
        );
    }

    #[test]
    fn diff_rows_stay_dark_without_losing_their_green_and_red_hues() {
        let editor = color::surface().editor;
        let added = diff::added();
        let removed = diff::removed();

        assert!(
            added.g() >= editor.g() + 28 && added.g() <= editor.g() + 45,
            "the added row is too washed out or too bright: {added:?} over {editor:?}"
        );
        assert!(
            removed.r() >= editor.r() + 35 && removed.r() <= editor.r() + 55,
            "the removed row is too washed out or too bright: {removed:?} over {editor:?}"
        );
        assert!(
            added.g() >= added.r() + 15 && removed.r() >= removed.g() + 25,
            "diff rows lost their semantic hues: {added:?}, {removed:?}"
        );
    }

    #[test]
    fn lifted_code_keeps_every_syntax_role_legible_on_the_diff_washes() {
        let syntax = color::syntax();
        // A comment recedes to 3:1 like it does on the document; every other
        // role holds the same 4:1 bar it clears on the editor surface.
        let roles = [
            (syntax.comment, 3.0),
            (syntax.foreground, 4.0),
            (syntax.string, 4.0),
            (syntax.keyword, 4.0),
            (syntax.declared_type, 4.0),
            (syntax.macro_name, 4.0),
            (syntax.escape, 4.0),
            (syntax.link, 4.0),
        ];
        for wash in [diff::added(), diff::removed()] {
            for (role, minimum) in roles {
                let ratio = color::contrast_ratio(diff::code(role), wash);
                assert!(
                    ratio >= minimum,
                    "{role:?} code is only {ratio:.2}:1 on the {wash:?} wash"
                );
            }
        }
    }

    #[test]
    fn gutter_numbers_stay_legible_on_the_diff_washes() {
        let contrast = |text: egui::Color32, fill: egui::Color32| {
            (i32::from(text.r()) - i32::from(fill.r())).abs()
                + (i32::from(text.g()) - i32::from(fill.g())).abs()
                + (i32::from(text.b()) - i32::from(fill.b())).abs()
        };

        assert!(
            contrast(diff::added_number(), diff::added()) >= 120,
            "added-row numbers vanish into the wash: {:?} on {:?}",
            diff::added_number(),
            diff::added()
        );
        assert!(
            contrast(diff::removed_number(), diff::removed()) >= 120,
            "removed-row numbers vanish into the wash: {:?} on {:?}",
            diff::removed_number(),
            diff::removed()
        );
    }

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
