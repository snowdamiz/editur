//! Two weights, one six-step scale. Hierarchy comes from weight and size
//! together, never from gray level alone.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

use egui::{FontData, FontDefinitions, FontFamily, FontId, TextStyle};

const UI_REGULAR: &str = "Inter";
const UI_SEMIBOLD: &str = "Inter SemiBold";
const CODE: &str = "JetBrains Mono";

/// The named family that gives the interface a real weight axis. egui does not
/// apply variable-font axes, so semibold has to arrive as its own face.
pub(crate) const STRONG_FAMILY: &str = "ui-strong";

pub(crate) fn strong_family() -> FontFamily {
    FontFamily::Name(Arc::from(STRONG_FAMILY))
}

pub(crate) const MICRO_SIZE: f32 = 11.0;
pub(crate) const SMALL_SIZE: f32 = 12.0;
pub(crate) const BODY_SIZE: f32 = 13.0;
pub(crate) const TITLE_SIZE: f32 = 15.0;
pub(crate) const DISPLAY_SIZE: f32 = 20.0;

/// Section labels and badges: uppercase, semibold, with +6% tracking.
pub(crate) fn micro() -> FontId {
    FontId::new(MICRO_SIZE, strong_family())
}

/// Tab labels, tree rows, metadata, hints.
pub(crate) fn small() -> FontId {
    FontId::proportional(SMALL_SIZE)
}

/// A tree directory or any small label that carries weight.
pub(crate) fn small_strong() -> FontId {
    FontId::new(SMALL_SIZE, strong_family())
}

/// Default UI text: menus, buttons, the agent transcript.
pub(crate) fn body() -> FontId {
    FontId::proportional(BODY_SIZE)
}

/// Active tab, selected row, field labels.
pub(crate) fn strong() -> FontId {
    FontId::new(BODY_SIZE, strong_family())
}

/// Dialog and section titles.
pub(crate) fn title() -> FontId {
    FontId::new(TITLE_SIZE, strong_family())
}

/// Settings page titles and the empty-state headline.
pub(crate) fn display() -> FontId {
    FontId::new(DISPLAY_SIZE, strong_family())
}

/// Editor text. 14 px by default, with a 20 px line box (1.43).
pub(crate) const CODE_SIZE: f32 = 14.0;
pub(crate) const CODE_SIZE_RANGE: std::ops::RangeInclusive<f32> = 10.0..=24.0;
pub(crate) const CODE_LINE_RATIO: f32 = 1.43;

/// The editor's own metrics are user settings, so they are runtime values. Like
/// the palette they belong to one window on one thread.
static CODE_SIZE_BITS: AtomicU32 = AtomicU32::new(0);
static CODE_RATIO_BITS: AtomicU32 = AtomicU32::new(0);
static CODE_FAMILY: Mutex<Option<String>> = Mutex::new(None);

pub(crate) fn set_code_metrics(size: f32, ratio: f32) {
    CODE_SIZE_BITS.store(
        size.clamp(*CODE_SIZE_RANGE.start(), *CODE_SIZE_RANGE.end())
            .to_bits(),
        Ordering::Relaxed,
    );
    CODE_RATIO_BITS.store(ratio.clamp(1.0, 2.0).to_bits(), Ordering::Relaxed);
}

/// The user's chosen code face, or `None` for the bundled one.
pub(crate) fn set_code_family(family: Option<String>) {
    *CODE_FAMILY.lock().expect("code family") = family;
}

pub(crate) fn code_size() -> f32 {
    match CODE_SIZE_BITS.load(Ordering::Relaxed) {
        0 => CODE_SIZE,
        bits => f32::from_bits(bits),
    }
}

pub(crate) fn code_ratio() -> f32 {
    match CODE_RATIO_BITS.load(Ordering::Relaxed) {
        0 => CODE_LINE_RATIO,
        bits => f32::from_bits(bits),
    }
}

/// The editor's line box. Whole pixels, because a fractional line box makes
/// every row land on a different subpixel and the retained cache thrash.
pub(crate) fn code_line() -> f32 {
    (code_size() * code_ratio()).round()
}

/// The editor's own font.
pub(crate) fn code_editor() -> FontId {
    FontId::monospace(code_size())
}

/// Line numbers, inline code, monospace paths in dialogs.
pub(crate) fn code_small() -> FontId {
    FontId::monospace(SMALL_SIZE)
}

/// The six roles, for the tests that keep the scale honest.
#[cfg(test)]
pub(crate) fn scale() -> [FontId; 6] {
    [micro(), small(), body(), strong(), title(), display()]
}

pub(crate) fn font_definitions() -> FontDefinitions {
    let mut definitions = FontDefinitions::default();
    let mut install = |name: &str, bytes: &'static [u8]| {
        definitions
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    install(
        UI_REGULAR,
        include_bytes!("../../assets/fonts/InterVariable.ttf"),
    );
    install(
        UI_SEMIBOLD,
        include_bytes!("../../assets/fonts/Inter-SemiBold.ttf"),
    );
    install(
        CODE,
        include_bytes!("../../assets/fonts/JetBrainsMono-Regular.ttf"),
    );
    definitions
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, UI_REGULAR.to_owned());
    definitions
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, CODE.to_owned());
    definitions.families.insert(
        strong_family(),
        vec![UI_SEMIBOLD.to_owned(), UI_REGULAR.to_owned()],
    );
    definitions
}

pub(crate) fn text_styles() -> std::collections::BTreeMap<TextStyle, FontId> {
    [
        (TextStyle::Small, small()),
        (TextStyle::Body, body()),
        (TextStyle::Button, body()),
        (TextStyle::Heading, display()),
        (TextStyle::Monospace, code_small()),
    ]
    .into()
}

#[cfg(test)]
mod tests {
    use super::{
        CODE, STRONG_FAMILY, UI_REGULAR, body, font_definitions, scale, strong, strong_family,
    };
    use egui::FontFamily;

    #[test]
    fn the_interface_ships_a_second_weight_and_a_modern_code_face() {
        let definitions = font_definitions();

        assert_eq!(
            definitions.families[&FontFamily::Proportional]
                .first()
                .map(String::as_str),
            Some(UI_REGULAR)
        );
        assert_eq!(
            definitions.families[&FontFamily::Monospace]
                .first()
                .map(String::as_str),
            Some(CODE),
            "the bundled Hack default must be replaced"
        );
        assert_eq!(
            definitions.families[&strong_family()]
                .first()
                .map(String::as_str),
            Some("Inter SemiBold")
        );
        assert_ne!(
            strong().family,
            body().family,
            "strong text has to resolve to a different face, not just a lighter gray"
        );
        assert_eq!(strong().family, FontFamily::Name(STRONG_FAMILY.into()));
    }

    #[test]
    fn the_scale_has_no_duplicate_steps_within_a_weight() {
        let mut roles: Vec<_> = scale()
            .iter()
            .map(|font| (format!("{:?}", font.family), font.size.to_bits()))
            .collect();
        let before = roles.len();
        roles.sort_unstable();
        roles.dedup();

        assert_eq!(roles.len(), before, "two roles resolve to the same font");
    }
}
