use std::sync::Arc;

use egui::{
    Color32, Context, CornerRadius, FontData, FontDefinitions, FontFamily, Stroke, Style,
    style::WidgetVisuals,
};

pub(crate) const CANVAS: Color32 = Color32::from_rgb(20, 20, 22);
pub(crate) const SURFACE: Color32 = Color32::from_rgb(24, 24, 26);
pub(crate) const SURFACE_RAISED: Color32 = Color32::from_rgb(28, 28, 31);
pub(crate) const SURFACE_INPUT: Color32 = Color32::from_rgb(31, 31, 34);
pub(crate) const SURFACE_HOVER: Color32 = Color32::from_rgb(35, 35, 39);
pub(crate) const SURFACE_SELECTED: Color32 = Color32::from_rgb(39, 39, 43);
pub(crate) const BORDER_SUBTLE: Color32 = Color32::from_rgb(48, 48, 52);
pub(crate) const BORDER_STRONG: Color32 = Color32::from_rgb(56, 56, 60);

pub(crate) const TEXT_PRIMARY: Color32 = Color32::from_rgb(230, 230, 234);
pub(crate) const TEXT_SECONDARY: Color32 = Color32::from_rgb(184, 184, 188);
pub(crate) const TEXT_MUTED: Color32 = Color32::from_rgb(145, 145, 149);
pub(crate) const TEXT_DISABLED: Color32 = Color32::from_rgb(126, 126, 130);

pub(crate) const ACCENT: Color32 = Color32::from_rgb(94, 210, 224);
pub(crate) const ACCENT_INK: Color32 = Color32::from_rgb(9, 28, 32);

pub(crate) fn apply(context: &Context) {
    context.set_fonts(font_definitions());
    context.all_styles_mut(apply_to);
}

fn font_definitions() -> FontDefinitions {
    let mut definitions = FontDefinitions::default();
    definitions.font_data.insert(
        "Inter".to_owned(),
        Arc::new(FontData::from_static(include_bytes!(
            "../assets/fonts/InterVariable.ttf"
        ))),
    );
    definitions
        .families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "Inter".to_owned());
    definitions
}

pub(crate) fn apply_to(style: &mut Style) {
    style.animation_time = 0.0;
    let visuals = &mut style.visuals;
    visuals.dark_mode = true;
    visuals.weak_text_color = Some(TEXT_MUTED);
    visuals.selection.bg_fill = SURFACE_SELECTED;
    visuals.selection.stroke = Stroke::new(1.0, ACCENT);
    visuals.hyperlink_color = ACCENT;
    visuals.faint_bg_color = SURFACE_HOVER;
    visuals.extreme_bg_color = SURFACE_INPUT;
    visuals.text_edit_bg_color = Some(SURFACE_INPUT);
    visuals.code_bg_color = SURFACE_SELECTED;
    visuals.window_corner_radius = CornerRadius::same(12);
    visuals.window_fill = SURFACE_RAISED;
    visuals.window_stroke = Stroke::new(1.0, BORDER_SUBTLE);
    visuals.menu_corner_radius = CornerRadius::same(8);
    visuals.panel_fill = CANVAS;
    visuals.text_cursor.stroke = Stroke::new(1.5, ACCENT);

    style_widget(
        &mut visuals.widgets.noninteractive,
        SURFACE,
        SURFACE,
        BORDER_SUBTLE,
        TEXT_SECONDARY,
    );
    style_widget(
        &mut visuals.widgets.inactive,
        SURFACE_INPUT,
        SURFACE_SELECTED,
        BORDER_SUBTLE,
        TEXT_SECONDARY,
    );
    style_widget(
        &mut visuals.widgets.hovered,
        SURFACE_HOVER,
        SURFACE_HOVER,
        BORDER_STRONG,
        TEXT_PRIMARY,
    );
    style_widget(
        &mut visuals.widgets.active,
        SURFACE_SELECTED,
        SURFACE_SELECTED,
        ACCENT,
        TEXT_PRIMARY,
    );
    style_widget(
        &mut visuals.widgets.open,
        SURFACE_SELECTED,
        SURFACE_SELECTED,
        BORDER_STRONG,
        TEXT_PRIMARY,
    );
}

fn style_widget(
    widget: &mut WidgetVisuals,
    background: Color32,
    weak_background: Color32,
    border: Color32,
    foreground: Color32,
) {
    widget.bg_fill = background;
    widget.weak_bg_fill = weak_background;
    widget.bg_stroke = Stroke::new(1.0, border);
    widget.corner_radius = CornerRadius::same(6);
    widget.fg_stroke = Stroke::new(1.0, foreground);
}

#[cfg(test)]
mod tests {
    use super::{
        ACCENT, BORDER_SUBTLE, CANVAS, SURFACE_INPUT, SURFACE_RAISED, apply, font_definitions,
    };

    #[test]
    fn apply_uses_the_shared_palette_for_egui_widgets() {
        let context = egui::Context::default();

        apply(&context);

        let style = context.style_of(context.theme());
        assert_eq!(
            (
                style.visuals.panel_fill,
                style.visuals.window_fill,
                style.visuals.text_edit_bg_color,
                style.visuals.selection.stroke.color,
                style.visuals.window_stroke.color,
            ),
            (
                CANVAS,
                SURFACE_RAISED,
                Some(SURFACE_INPUT),
                ACCENT,
                BORDER_SUBTLE,
            )
        );
    }

    #[test]
    fn font_definitions_use_inter_for_ui_without_replacing_code_font() {
        let definitions = font_definitions();

        assert_eq!(
            (
                definitions.families[&egui::FontFamily::Proportional]
                    .first()
                    .map(String::as_str),
                definitions.families[&egui::FontFamily::Monospace]
                    .first()
                    .map(String::as_str),
            ),
            (Some("Inter"), Some("Hack"))
        );
    }
}
