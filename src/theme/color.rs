//! The palette. Every color in the product resolves through the active
//! `Palette`, so a second theme is a data change rather than a refactor.

#[cfg(test)]
use std::cell::Cell;
#[cfg(not(test))]
use std::sync::atomic::{AtomicBool, Ordering};

use egui::Color32;

#[derive(Clone, Copy)]
pub(crate) struct Surfaces {
    /// Window backdrop, split gutters, the empty pane well.
    pub(crate) sunken: Color32,
    /// Titlebar, tab strip, file tree, agent panel, pane and terminal headers.
    pub(crate) chrome: Color32,
    /// Editor body, terminal body, markdown preview.
    pub(crate) editor: Color32,
    /// Menus, dialogs, palette, settings cards, agent tool rows.
    pub(crate) raised: Color32,
    /// Text fields and wells inside raised surfaces.
    pub(crate) input: Color32,
}

#[derive(Clone, Copy)]
pub(crate) struct TextRoles {
    /// Code, active tab, dialog titles, selected rows.
    pub(crate) primary: Color32,
    /// Default UI text, inactive tabs, file names.
    pub(crate) secondary: Color32,
    /// Line numbers, hints, metadata, placeholders.
    pub(crate) muted: Color32,
    /// Text on a solid accent fill.
    pub(crate) on_accent: Color32,
}

#[derive(Clone, Copy)]
pub(crate) struct Semantics {
    pub(crate) danger: Color32,
    pub(crate) warning: Color32,
    pub(crate) success: Color32,
    pub(crate) info: Color32,
}

#[derive(Clone, Copy)]
pub(crate) struct SyntaxRoles {
    pub(crate) foreground: Color32,
    pub(crate) comment: Color32,
    pub(crate) string: Color32,
    pub(crate) keyword: Color32,
    pub(crate) declared_type: Color32,
    pub(crate) macro_name: Color32,
    pub(crate) escape: Color32,
    pub(crate) link: Color32,
}

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    pub(crate) dark: bool,
    pub(crate) accent: Color32,
    pub(crate) surface: Surfaces,
    pub(crate) text: TextRoles,
    pub(crate) semantic: Semantics,
    pub(crate) syntax: SyntaxRoles,
}

const CYAN: Color32 = Color32::from_rgb(94, 210, 224);

pub(crate) const DARK: Palette = Palette {
    dark: true,
    accent: CYAN,
    surface: Surfaces {
        sunken: Color32::from_rgb(12, 12, 14),
        chrome: Color32::from_rgb(18, 18, 21),
        editor: Color32::from_rgb(27, 27, 31),
        raised: Color32::from_rgb(33, 33, 38),
        input: Color32::from_rgb(21, 21, 25),
    },
    text: TextRoles {
        primary: Color32::from_rgb(233, 234, 238),
        secondary: Color32::from_rgb(166, 168, 176),
        // Six values above the drafted 136 so that line numbers and menu hints
        // clear 4.5:1 on `raised` too, not only on the document.
        muted: Color32::from_rgb(142, 144, 152),
        on_accent: Color32::from_rgb(8, 26, 30),
    },
    semantic: Semantics {
        danger: Color32::from_rgb(232, 106, 102),
        warning: Color32::from_rgb(224, 176, 84),
        success: Color32::from_rgb(118, 199, 150),
        info: Color32::from_rgb(108, 158, 214),
    },
    syntax: SyntaxRoles {
        foreground: Color32::from_rgb(233, 234, 238),
        comment: Color32::from_rgb(112, 116, 128),
        string: Color32::from_rgb(150, 200, 138),
        keyword: Color32::from_rgb(199, 133, 224),
        declared_type: Color32::from_rgb(112, 172, 232),
        macro_name: Color32::from_rgb(228, 190, 128),
        escape: CYAN,
        link: Color32::from_rgb(126, 198, 210),
    },
};

pub(crate) const LIGHT: Palette = Palette {
    dark: false,
    accent: Color32::from_rgb(17, 120, 136),
    surface: Surfaces {
        sunken: Color32::from_rgb(210, 210, 215),
        chrome: Color32::from_rgb(226, 226, 231),
        editor: Color32::from_rgb(243, 243, 246),
        raised: Color32::from_rgb(255, 255, 255),
        input: Color32::from_rgb(236, 236, 240),
    },
    text: TextRoles {
        primary: Color32::from_rgb(18, 19, 23),
        secondary: Color32::from_rgb(66, 69, 77),
        muted: Color32::from_rgb(86, 89, 98),
        on_accent: Color32::from_rgb(247, 253, 254),
    },
    semantic: Semantics {
        danger: Color32::from_rgb(178, 38, 38),
        warning: Color32::from_rgb(128, 88, 6),
        success: Color32::from_rgb(20, 108, 66),
        info: Color32::from_rgb(28, 88, 158),
    },
    syntax: SyntaxRoles {
        foreground: Color32::from_rgb(18, 19, 23),
        comment: Color32::from_rgb(112, 116, 126),
        string: Color32::from_rgb(24, 106, 52),
        keyword: Color32::from_rgb(140, 38, 162),
        declared_type: Color32::from_rgb(22, 88, 164),
        macro_name: Color32::from_rgb(128, 84, 10),
        escape: Color32::from_rgb(17, 120, 136),
        link: Color32::from_rgb(20, 102, 120),
    },
};

/// The window renders one palette at a time on one thread; a flag is enough,
/// and it keeps every token a plain call instead of a threaded parameter.
#[cfg(not(test))]
static LIGHT_ACTIVE: AtomicBool = AtomicBool::new(false);
#[cfg(test)]
std::thread_local! {
    static LIGHT_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn set_light(light: bool) {
    #[cfg(not(test))]
    LIGHT_ACTIVE.store(light, Ordering::Relaxed);
    #[cfg(test)]
    LIGHT_ACTIVE.set(light);
}

pub(crate) fn palette() -> Palette {
    #[cfg(not(test))]
    let light = LIGHT_ACTIVE.load(Ordering::Relaxed);
    #[cfg(test)]
    let light = LIGHT_ACTIVE.get();
    if light { LIGHT } else { DARK }
}

pub(crate) fn surface() -> Surfaces {
    palette().surface
}

pub(crate) fn text() -> TextRoles {
    palette().text
}

pub(crate) fn accent() -> Color32 {
    palette().accent
}

pub(crate) fn semantic() -> Semantics {
    palette().semantic
}

pub(crate) fn syntax() -> SyntaxRoles {
    palette().syntax
}

/// Disabled state is `text.muted` at 55%, not a fourth gray.
pub(crate) fn text_disabled() -> Color32 {
    text().muted.gamma_multiply(0.55)
}

/// The 16% companion of a semantic color, for diff and diagnostic backgrounds.
pub(crate) fn subtle(color: Color32) -> Color32 {
    color.gamma_multiply(0.16)
}

/// A semantic color pushed far enough toward the page to read as body text on
/// top of `subtle(color)`.
pub(crate) fn ink(color: Color32) -> Color32 {
    ink_for(&palette(), color)
}

fn ink_for(palette: &Palette, color: Color32) -> Color32 {
    if palette.dark {
        mix(color, Color32::WHITE, 0.35)
    } else {
        mix(color, palette.text.primary, 0.35)
    }
}

/// The macOS window buttons. These are platform values rather than product
/// ones: they have to match the system's own fills, so they do not move with
/// the palette, only their unfocused gray does.
#[cfg(target_os = "macos")]
pub(crate) mod traffic {
    use egui::Color32;

    pub(crate) const CLOSE: Color32 = Color32::from_rgb(255, 95, 87);
    pub(crate) const MINIMIZE: Color32 = Color32::from_rgb(254, 188, 46);
    pub(crate) const ZOOM: Color32 = Color32::from_rgb(40, 200, 64);

    /// The system paints every button the same gray while the window is not the
    /// key window.
    pub(crate) fn unfocused() -> Color32 {
        super::mix(
            super::palette().surface.chrome,
            super::palette().text.muted,
            0.55,
        )
    }

    /// The glyph the system draws inside a hovered button.
    pub(crate) fn glyph() -> Color32 {
        Color32::from_black_alpha(150)
    }
}

/// The terminal's 16-color table, tuned to the chrome's white point.
pub(crate) fn ansi() -> [Color32; 16] {
    ansi_for(&palette())
}

fn ansi_for(palette: &Palette) -> [Color32; 16] {
    let Semantics {
        danger,
        warning,
        success,
        info,
    } = palette.semantic;
    let magenta = palette.syntax.keyword;
    let (black, bright_black, toward) = if palette.dark {
        (
            Color32::from_rgb(48, 48, 54),
            Color32::from_rgb(104, 106, 114),
            Color32::WHITE,
        )
    } else {
        (
            Color32::from_rgb(60, 62, 68),
            Color32::from_rgb(112, 116, 126),
            palette.text.primary,
        )
    };
    let bright = |color| mix(color, toward, 0.25);
    [
        black,
        danger,
        success,
        warning,
        info,
        magenta,
        palette.accent,
        palette.text.secondary,
        bright_black,
        bright(danger),
        bright(success),
        bright(warning),
        bright(info),
        bright(magenta),
        bright(palette.accent),
        palette.text.primary,
    ]
}

/// A color a program asked for by number rather than by role: the terminal's
/// truecolor and 256-color escapes. Nothing in the product's own chrome may
/// call this, which is why it is the theme module that owns it.
pub(crate) fn literal(red: u8, green: u8, blue: u8) -> Color32 {
    Color32::from_rgb(red, green, blue)
}

pub(crate) fn literal_gray(value: u8) -> Color32 {
    Color32::from_gray(value)
}

/// A color no palette contains, so a test can find its own mesh in the
/// tessellated output without asserting against a product token.
#[cfg(test)]
pub(crate) fn sentinel() -> Color32 {
    Color32::from_rgb(7, 19, 23)
}

/// Blends `factor` of `toward` into `color`. Both must be opaque.
pub(crate) fn mix(color: Color32, toward: Color32, factor: f32) -> Color32 {
    let factor = factor.clamp(0.0, 1.0);
    let channel = |from: u8, to: u8| {
        (f32::from(from) + (f32::from(to) - f32::from(from)) * factor).round() as u8
    };
    Color32::from_rgb(
        channel(color.r(), toward.r()),
        channel(color.g(), toward.g()),
        channel(color.b(), toward.b()),
    )
}

/// Lays a premultiplied translucent `overlay` over an opaque `base`.
pub(crate) fn composite(overlay: Color32, base: Color32) -> Color32 {
    let remaining = 1.0 - f32::from(overlay.a()) / 255.0;
    let channel = |over: u8, under: u8| {
        (f32::from(over) + f32::from(under) * remaining)
            .round()
            .min(255.0) as u8
    };
    Color32::from_rgb(
        channel(overlay.r(), base.r()),
        channel(overlay.g(), base.g()),
        channel(overlay.b(), base.b()),
    )
}

#[cfg(test)]
fn relative_luminance(color: Color32) -> f32 {
    let channel = |value: u8| {
        let value = f32::from(value) / 255.0;
        if value <= 0.040_45 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
}

/// WCAG contrast ratio between two opaque colors, from 1.0 to 21.0.
#[cfg(test)]
pub(crate) fn contrast_ratio(first: Color32, second: Color32) -> f32 {
    let (first, second) = (relative_luminance(first), relative_luminance(second));
    let (lighter, darker) = if first >= second {
        (first, second)
    } else {
        (second, first)
    };
    (lighter + 0.05) / (darker + 0.05)
}

#[cfg(test)]
mod tests {
    use super::{DARK, LIGHT, Palette, composite, contrast_ratio, palette, set_light, subtle};
    use crate::theme::state;
    use egui::Color32;
    use std::sync::{Arc, Barrier};

    fn palettes() -> [Palette; 2] {
        [DARK, LIGHT]
    }

    #[test]
    fn parallel_tests_keep_their_palette_local() {
        let _flag = crate::theme::PALETTE_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        set_light(false);
        let ready = Arc::new(Barrier::new(2));
        let release = Arc::new(Barrier::new(2));
        let thread = std::thread::spawn({
            let ready = Arc::clone(&ready);
            let release = Arc::clone(&release);
            move || {
                set_light(true);
                ready.wait();
                release.wait();
            }
        });

        ready.wait();
        let main_stayed_dark = palette().dark;
        release.wait();
        thread.join().unwrap();
        set_light(false);

        assert!(main_stayed_dark);
    }

    fn ramp(palette: &Palette) -> [Color32; 4] {
        [
            palette.surface.sunken,
            palette.surface.chrome,
            palette.surface.editor,
            palette.surface.raised,
        ]
    }

    #[test]
    fn every_elevation_ramp_rises_in_perceptible_steps() {
        for palette in palettes() {
            for pair in ramp(&palette).windows(2) {
                let step = i16::from(pair[1].r()) - i16::from(pair[0].r());
                assert!(
                    step >= 6,
                    "{:?} to {:?} steps only {step} per channel",
                    pair[0],
                    pair[1]
                );
            }
            assert!(
                palette.surface.input.r() < palette.surface.raised.r(),
                "a well has to sit below the surface it is cut into"
            );
        }
    }

    #[test]
    fn every_text_role_stays_legible_on_every_surface_it_is_allowed_on() {
        for palette in palettes() {
            let minimums = [
                (palette.text.primary, 12.0),
                (palette.text.secondary, 6.0),
                (palette.text.muted, 4.5),
            ];
            for (color, minimum) in minimums {
                for surface in [
                    palette.surface.sunken,
                    palette.surface.chrome,
                    palette.surface.editor,
                    palette.surface.raised,
                    palette.surface.input,
                ] {
                    let ratio = contrast_ratio(color, surface);
                    assert!(
                        ratio >= minimum,
                        "{color:?} on {surface:?} is only {ratio:.2}:1, wanted {minimum}:1"
                    );
                }
            }
            assert!(
                contrast_ratio(palette.text.on_accent, palette.accent) >= 4.5,
                "a primary action's own label has to clear AA"
            );
        }
    }

    #[test]
    fn every_syntax_role_stays_legible_on_the_document_surface() {
        for palette in palettes() {
            let syntax = palette.syntax;
            // A comment is meant to recede, so it answers to the 3:1 bar the
            // rest of the roles clear at 4:1.
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
            for (color, minimum) in roles {
                let ratio = contrast_ratio(color, palette.surface.editor);
                assert!(
                    ratio >= minimum,
                    "{color:?} code is only {ratio:.2}:1 against the document"
                );
            }
        }
    }

    #[test]
    fn semantic_colors_read_against_their_own_subtle_backgrounds() {
        for palette in palettes() {
            let semantic = palette.semantic;
            for color in [
                semantic.danger,
                semantic.warning,
                semantic.success,
                semantic.info,
            ] {
                let background = composite(subtle(color), palette.surface.editor);
                let ratio = contrast_ratio(super::ink_for(&palette, color), background);
                assert!(
                    ratio >= 4.0,
                    "{color:?} ink is only {ratio:.2}:1 on its own subtle fill"
                );
            }
        }
    }

    #[test]
    fn the_terminal_palette_separates_every_slot_it_is_meant_to() {
        for palette in palettes() {
            let table = super::ansi_for(&palette);
            for (index, color) in table.iter().enumerate() {
                let ratio = contrast_ratio(*color, palette.surface.editor);
                // Slot 0 is the terminal's own black and is used as a fill.
                let minimum = if index == 0 { 1.0 } else { 2.5 };
                assert!(
                    ratio >= minimum,
                    "ANSI {index} is only {ratio:.2}:1 against the terminal body"
                );
            }
            assert_eq!(table.len(), 16);
        }
    }

    #[test]
    fn state_overlays_lighten_every_surface_by_a_matching_amount() {
        let delta = |overlay: Color32, base: Color32| {
            i16::from(composite(overlay, base).r()) - i16::from(base.r())
        };
        let chrome = DARK.surface.chrome;
        let raised = DARK.surface.raised;
        for overlay in [state::hover(), state::press(), state::selected()] {
            let over_chrome = delta(overlay, chrome);
            let over_raised = delta(overlay, raised);
            assert!(over_chrome > 0 && over_raised > 0, "{overlay:?} darkened");
            assert!(
                (over_chrome - over_raised).abs() <= 2,
                "{overlay:?} shifts chrome by {over_chrome} but raised by {over_raised}"
            );
        }
        let steps =
            [state::hover(), state::press(), state::selected()].map(|over| delta(over, chrome));
        assert!(
            steps[0] < steps[1] && steps[1] < steps[2],
            "hover, press, and selected must be increasingly visible: {steps:?}"
        );
    }

    #[test]
    fn editor_selection_and_active_line_survive_on_screen() {
        let editor = DARK.surface.editor;
        let selection = composite(state::editor::selection(), editor);
        let against_background = contrast_ratio(selection, editor);
        assert!(
            against_background >= 1.5,
            "selection is only {against_background:.2}:1 against the document, \
             barely past the 1.10:1 that made it invisible"
        );
        assert!(
            contrast_ratio(DARK.text.primary, selection) >= 4.5,
            "code stops being readable once it is selected"
        );
        assert!(
            selection.b() > selection.r() + 8,
            "selection must stay chromatic so it cannot be read as a hover"
        );
        assert!(
            contrast_ratio(
                composite(state::editor::selection_inactive(), editor),
                editor
            ) < against_background,
            "an unfocused pane must dim its selection"
        );
        assert!(
            composite(state::editor::line_active(), editor).r() >= editor.r() + 8,
            "the active line still does not survive on screen"
        );
    }
}
