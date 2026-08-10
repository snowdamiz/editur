use egui::{Align, Align2, Color32, Id, Layout, Rect, Response, RichText, Sense, Ui, Vec2};

use crate::theme::{
    ACCENT, ACCENT_INK, BORDER_STRONG, SURFACE_HOVER, SURFACE_RAISED, SURFACE_SELECTED, TEXT_MUTED,
    TEXT_PRIMARY, TEXT_SECONDARY,
};

pub(crate) fn dialog_frame(ctx: &egui::Context) -> egui::Frame {
    egui::Frame::window(&ctx.style_of(ctx.theme()))
        .fill(SURFACE_RAISED)
        .stroke(egui::Stroke::new(1.0, Color32::from_white_alpha(24)))
        .inner_margin(18)
        .corner_radius(12)
        .shadow(egui::Shadow {
            offset: [0, 10],
            blur: 32,
            spread: 2,
            color: Color32::from_black_alpha(170),
        })
}

pub(crate) fn dialog_window<'a>(
    ctx: &egui::Context,
    title: impl egui::IntoAtoms<'a>,
    id: &'static str,
) -> egui::Window<'a> {
    egui::Window::new(title)
        .id(Id::new(id))
        .anchor(Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .title_bar(false)
        .fade_in(false)
        .collapsible(false)
        .resizable(false)
        .movable(false)
        .frame(dialog_frame(ctx))
}

pub(crate) fn begin_dialog(ui: &mut Ui, title: &str) {
    ui.set_min_width(340.0);
    ui.spacing_mut().item_spacing = egui::vec2(8.0, 10.0);
    ui.label(RichText::new(title).size(16.0).strong().color(TEXT_PRIMARY));
}

pub(crate) fn dialog_button(label: &'static str, primary: bool) -> egui::Button<'static> {
    let (text, fill) = if primary {
        (ACCENT_INK, ACCENT)
    } else {
        (TEXT_PRIMARY, SURFACE_SELECTED)
    };
    egui::Button::new(RichText::new(label).color(text).strong())
        .fill(fill)
        .stroke(egui::Stroke::NONE)
        .corner_radius(6)
        .min_size(egui::vec2(72.0, 40.0))
}

pub(crate) fn dialog_actions<R>(ui: &mut Ui, add_contents: impl FnOnce(&mut Ui) -> R) -> R {
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), 40.0),
        Layout::right_to_left(Align::Center),
        add_contents,
    )
    .inner
}

pub(crate) struct SelectableRow {
    pub(crate) rect: Rect,
    pub(crate) response: Response,
    pub(crate) foreground: Color32,
}

pub(crate) fn selectable_row(
    ui: &mut Ui,
    label: &str,
    selected: bool,
    height: f32,
) -> SelectableRow {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    let fill = selection_fill(selected, response.hovered());
    if fill != Color32::TRANSPARENT {
        ui.painter().rect_filled(rect, 5.0, fill);
    }
    let foreground = if !ui.is_enabled() {
        TEXT_MUTED
    } else if selected || response.hovered() {
        TEXT_PRIMARY
    } else {
        TEXT_SECONDARY
    };
    if selected {
        paint_checkmark(ui, egui::pos2(rect.right() - 13.0, rect.center().y));
    }
    SelectableRow {
        rect,
        response,
        foreground,
    }
}

pub(crate) fn selectable_content_row(
    ui: &mut Ui,
    selected: bool,
    min_height: f32,
    add_contents: impl FnOnce(&mut Ui),
) -> Response {
    let mut row = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(9, 6))
        .corner_radius(6)
        .begin(ui);
    row.content_ui
        .set_min_width(row.content_ui.available_width());
    row.content_ui.set_min_height(min_height);
    add_contents(&mut row.content_ui);
    let response = row.allocate_space(ui).interact(Sense::click());
    row.frame.fill = selection_fill(selected, response.hovered());
    row.paint(ui);
    response
}

pub(crate) fn icon_button(
    ui: &mut Ui,
    label: &str,
    size: Vec2,
    idle_color: Color32,
    paint: impl FnOnce(&egui::Painter, Rect, Color32),
) -> Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, 5.0, Color32::from_white_alpha(14));
    }
    let color = if !ui.is_enabled() {
        BORDER_STRONG
    } else if response.hovered() {
        TEXT_PRIMARY
    } else {
        idle_color
    };
    paint(ui.painter(), rect, color);
    response.on_hover_text(label)
}

pub(crate) fn chevron_icon_button(ui: &mut Ui, upward: bool, label: &str) -> Response {
    icon_button(
        ui,
        label,
        egui::vec2(30.0, 26.0),
        TEXT_SECONDARY,
        |painter, rect, color| {
            let center = rect.center();
            let direction = if upward { -1.0 } else { 1.0 };
            let tip = center + egui::vec2(0.0, 3.0 * direction);
            let stroke = egui::Stroke::new(1.5, color);
            painter.line_segment([center + egui::vec2(-4.0, -2.0 * direction), tip], stroke);
            painter.line_segment([tip, center + egui::vec2(4.0, -2.0 * direction)], stroke);
        },
    )
}

pub(crate) fn close_icon_button(ui: &mut Ui) -> Response {
    icon_button(
        ui,
        "Close (Esc)",
        egui::vec2(30.0, 26.0),
        TEXT_SECONDARY,
        |painter, rect, color| {
            let center = rect.center();
            let stroke = egui::Stroke::new(1.5, color);
            painter.line_segment(
                [
                    center + egui::vec2(-3.5, -3.5),
                    center + egui::vec2(3.5, 3.5),
                ],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-3.5, 3.5),
                    center + egui::vec2(3.5, -3.5),
                ],
                stroke,
            );
        },
    )
}

fn selection_fill(selected: bool, hovered: bool) -> Color32 {
    if selected {
        SURFACE_SELECTED
    } else if hovered {
        SURFACE_HOVER
    } else {
        Color32::TRANSPARENT
    }
}

fn paint_checkmark(ui: &Ui, center: egui::Pos2) {
    let stroke = egui::Stroke::new(1.4, ACCENT);
    ui.painter().line_segment(
        [
            center + egui::vec2(-4.0, 0.0),
            center + egui::vec2(-1.0, 3.0),
        ],
        stroke,
    );
    ui.painter().line_segment(
        [
            center + egui::vec2(-1.0, 3.0),
            center + egui::vec2(4.0, -3.0),
        ],
        stroke,
    );
}
