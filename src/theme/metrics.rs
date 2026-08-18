//! The single physical rhythm: one spacing grid, four radii, one icon stroke,
//! four control heights, three chrome rows, two shadows.

use std::sync::atomic::{AtomicBool, Ordering};

use egui::CornerRadius;

/// The 4 px grid. Every `add_space`, margin, and inset uses one of these.
pub(crate) mod space {
    pub(crate) const HAIR: f32 = 2.0;
    pub(crate) const TIGHT: f32 = 4.0;
    pub(crate) const SNUG: f32 = 6.0;
    pub(crate) const SMALL: f32 = 8.0;
    pub(crate) const MEDIUM: f32 = 12.0;
    pub(crate) const LARGE: f32 = 16.0;
    pub(crate) const WIDE: f32 = 20.0;
    pub(crate) const XWIDE: f32 = 24.0;

    #[cfg(test)]
    pub(crate) const SCALE: [f32; 8] = [HAIR, TIGHT, SNUG, SMALL, MEDIUM, LARGE, WIDE, XWIDE];
}

pub(crate) mod radius {
    /// Rows, chips, tags.
    pub(crate) const ROW: u8 = 4;
    /// Buttons, inputs, icon-button hover.
    pub(crate) const CONTROL: u8 = 6;
    /// Cards, menus, popovers.
    pub(crate) const CARD: u8 = 10;
    /// Dialogs and the command palette.
    pub(crate) const DIALOG: u8 = 14;
    /// A platform value rather than a scale step: this is what sits correctly
    /// inside the macOS window mask.
    pub(crate) const WINDOW: u8 = 12;

    #[cfg(test)]
    pub(crate) const SCALE: [u8; 4] = [ROW, CONTROL, CARD, DIALOG];
}

pub(crate) mod stroke {
    pub(crate) const DIVIDER: f32 = 1.0;
    pub(crate) const ICON: f32 = 1.25;
    pub(crate) const CARET: f32 = 1.5;
    pub(crate) const FOCUS: f32 = 2.0;
}

pub(crate) mod control {
    /// Compact icon button.
    pub(crate) const COMPACT: f32 = 24.0;
    /// List row, tree row.
    pub(crate) const ROW: f32 = 28.0;
    /// Button, input, tab.
    pub(crate) const STANDARD: f32 = 32.0;
    /// Primary action, composer send.
    pub(crate) const PRIMARY: f32 = 36.0;

    #[cfg(test)]
    pub(crate) const SCALE: [f32; 4] = [COMPACT, ROW, STANDARD, PRIMARY];

    /// The list-row height the density setting asks for.
    pub(crate) fn row() -> f32 {
        match super::density() {
            super::Density::Comfortable => ROW,
            super::Density::Compact => COMPACT,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Density {
    #[default]
    Comfortable,
    Compact,
}

/// Like the palette, density is one value for one window on one thread, so it
/// stays a token call rather than a parameter threaded through every row.
static COMPACT_DENSITY: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_density(density: Density) {
    COMPACT_DENSITY.store(density == Density::Compact, Ordering::Relaxed);
}

pub(crate) fn density() -> Density {
    if COMPACT_DENSITY.load(Ordering::Relaxed) {
        Density::Compact
    } else {
        Density::Comfortable
    }
}

pub(crate) mod chrome {
    /// The titlebar strip, sized to fit the macOS traffic lights.
    pub(crate) const TITLEBAR: f32 = 34.0;
    /// Secondary headers: pane header, terminal header.
    pub(crate) const HEADER: f32 = 30.0;
    /// The find bar, which carries a full-height input.
    pub(crate) const FIND: f32 = 34.0;
}

pub(crate) fn corner(radius: u8) -> CornerRadius {
    CornerRadius::same(radius)
}

pub(crate) mod shadow {
    use egui::{Color32, Shadow};

    pub(crate) fn popover() -> Shadow {
        Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(115),
        }
    }

    pub(crate) fn dialog() -> Shadow {
        Shadow {
            offset: [0, 16],
            blur: 40,
            spread: 0,
            color: Color32::from_black_alpha(140),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{control, radius, space, stroke};

    #[test]
    fn every_scale_is_strictly_increasing_with_no_duplicate_steps() {
        assert!(space::SCALE.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(radius::SCALE.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(control::SCALE.windows(2).all(|pair| pair[0] < pair[1]));
        assert!(
            space::SCALE
                .iter()
                .all(|step| (step / 2.0).fract() == 0.0 || *step == space::SNUG),
            "spacing must stay on the grid"
        );
        const { assert!(stroke::DIVIDER < stroke::ICON && stroke::ICON < stroke::FOCUS) };
    }
}
