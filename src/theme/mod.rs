//! The design tokens. No module outside `src/theme/` may construct a color, a
//! radius, a font size, a spacing value, a stroke width, or a duration.

pub(crate) mod color;
pub(crate) mod metrics;
pub(crate) mod motion;
pub(crate) mod state;
pub(crate) mod typography;

#[cfg(test)]
pub(crate) use color::contrast_ratio;
#[allow(unused_imports)] // public token surface; consumers arrive over time
pub(crate) use color::{
    accent, ansi, composite, ink, mix, semantic, set_light, subtle, surface, syntax, text,
    text_disabled, traffic,
};
pub(crate) use metrics::{
    Density, chrome, control, corner, density, radius, set_density, shadow, space, stroke,
};
pub(crate) use state::{border, callout, diff, editor};

use egui::{Context, Stroke, Style, style::WidgetVisuals};

pub(crate) fn apply(context: &Context) {
    install_fonts(context);
    context.all_styles_mut(apply_to);
}

fn install_fonts(context: &Context) {
    context.set_fonts(typography::font_definitions());
    context.data_mut(|data| data.insert_temp(egui::Id::new("theme_fonts_installed"), true));
}

/// One number that changes whenever anything the retained caches baked in has
/// moved: the palette, the editor's metrics, or the density.
pub(crate) fn appearance() -> u64 {
    let light = u64::from(!color::palette().dark);
    let compact = u64::from(density() == Density::Compact);
    let size = u64::from(typography::code_size().to_bits());
    let ratio = u64::from(typography::code_ratio().to_bits());
    light | compact << 1 | size << 8 | ratio.rotate_left(40)
}

#[cfg(test)]
pub(crate) fn test_context() -> Context {
    let context = Context::default();
    apply(&context);
    context
}

pub(crate) fn apply_to(style: &mut Style) {
    // Motion is explicit and quantized in `theme::motion`; nothing may rely on
    // egui's implicit per-widget animation, which the retained renderer cannot
    // fold into a revision.
    style.animation_time = 0.0;
    style.text_styles = typography::text_styles();
    style.spacing.item_spacing = egui::vec2(space::SMALL, space::TIGHT);
    style.spacing.button_padding = egui::vec2(space::MEDIUM, space::SNUG);
    style.spacing.menu_margin = egui::Margin::symmetric(space::TIGHT as i8, space::TIGHT as i8);

    let surface = surface();
    let text = text();
    let visuals = &mut style.visuals;
    visuals.dark_mode = color::palette().dark;
    visuals.weak_text_color = Some(text.muted);
    visuals.selection.bg_fill = editor::selection();
    visuals.selection.stroke = Stroke::new(stroke::DIVIDER, text.primary);
    visuals.hyperlink_color = accent();
    visuals.faint_bg_color = state::hover();
    visuals.extreme_bg_color = surface.input;
    visuals.text_edit_bg_color = Some(surface.input);
    visuals.code_bg_color = state::selected();
    visuals.window_corner_radius = corner(radius::DIALOG);
    visuals.window_fill = surface.raised;
    visuals.window_stroke = border::strong();
    visuals.window_shadow = shadow::dialog();
    visuals.popup_shadow = shadow::popover();
    visuals.menu_corner_radius = corner(radius::CARD);
    visuals.panel_fill = surface.chrome;
    visuals.text_cursor.stroke = Stroke::new(stroke::CARET, accent());

    style_widget(
        &mut visuals.widgets.noninteractive,
        surface.chrome,
        surface.chrome,
        border::hairline_color(),
        text.secondary,
    );
    style_widget(
        &mut visuals.widgets.inactive,
        surface.input,
        state::selected(),
        border::hairline_color(),
        text.secondary,
    );
    style_widget(
        &mut visuals.widgets.hovered,
        state::hover(),
        state::hover(),
        border::strong_color(),
        text.primary,
    );
    style_widget(
        &mut visuals.widgets.active,
        state::press(),
        state::press(),
        border::focus_color(),
        text.primary,
    );
    style_widget(
        &mut visuals.widgets.open,
        state::selected(),
        state::selected(),
        border::strong_color(),
        text.primary,
    );
}

fn style_widget(
    widget: &mut WidgetVisuals,
    background: egui::Color32,
    weak_background: egui::Color32,
    border: egui::Color32,
    foreground: egui::Color32,
) {
    widget.bg_fill = background;
    widget.weak_bg_fill = weak_background;
    widget.bg_stroke = Stroke::new(stroke::DIVIDER, border);
    widget.corner_radius = corner(radius::CONTROL);
    widget.fg_stroke = Stroke::new(stroke::DIVIDER, foreground);
}

#[cfg(test)]
mod tests {
    use super::{
        accent, apply, color, editor, radius, set_light, space, surface, text, typography,
    };

    /// The one test that keeps this plan from unwinding: a color built outside
    /// the theme module is a color that will drift away from every other color.
    #[test]
    fn no_module_outside_the_theme_constructs_a_color() {
        let sources = [
            ("src/app.rs", include_str!("../app.rs")),
            ("src/components.rs", include_str!("../components.rs")),
            (
                "src/editor_surface.rs",
                include_str!("../editor_surface.rs"),
            ),
            ("src/icons.rs", include_str!("../icons.rs")),
            ("src/markdown.rs", include_str!("../markdown.rs")),
            ("src/scrollbar.rs", include_str!("../scrollbar.rs")),
            ("src/syntax.rs", include_str!("../syntax.rs")),
            ("src/terminal.rs", include_str!("../terminal.rs")),
            ("src/tree_surface.rs", include_str!("../tree_surface.rs")),
        ];

        let mut offenders = Vec::new();
        for (name, source) in sources {
            for (number, line) in source.lines().enumerate() {
                if line.contains("Color32::from_") {
                    offenders.push(format!("{name}:{}: {}", number + 1, line.trim()));
                }
            }
        }

        assert!(
            offenders.is_empty(),
            "colors have to come from src/theme/:\n{}",
            offenders.join("\n")
        );
    }

    #[test]
    fn theme_switching_does_not_require_a_restart() {
        let context = super::test_context();
        assert!(color::palette().dark);
        set_light(true);
        apply(&context);
        assert!(!color::palette().dark);
        assert_eq!(
            context.style_of(context.theme()).visuals.window_fill,
            surface().raised
        );
        set_light(false);
        apply(&context);
        assert!(color::palette().dark);
    }

    #[test]
    fn apply_wires_the_token_layer_into_every_egui_style() {
        let context = egui::Context::default();

        apply(&context);

        let style = context.style_of(context.theme());
        assert_eq!(style.visuals.panel_fill, surface().chrome);
        assert_eq!(style.visuals.window_fill, surface().raised);
        assert_eq!(style.visuals.text_edit_bg_color, Some(surface().input));
        assert_eq!(style.visuals.selection.bg_fill, editor::selection());
        assert_eq!(
            style.visuals.selection.stroke.color,
            text().primary,
            "selected code must stay readable rather than turning cyan"
        );
        assert_eq!(style.visuals.text_cursor.stroke.color, accent());
        assert_eq!(
            style.visuals.window_corner_radius,
            super::corner(radius::DIALOG)
        );
        assert_eq!(
            style.text_styles[&egui::TextStyle::Body],
            typography::body()
        );
        assert!(
            space::SCALE.contains(&style.spacing.item_spacing.x),
            "even egui's own spacing has to sit on the grid"
        );
        assert_eq!(
            style.animation_time, 0.0,
            "implicit egui animation cannot be folded into a retained revision"
        );
    }
}
