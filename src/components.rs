use egui::{Align2, Color32, Rect, Response, Sense, Ui, Vec2};

use crate::{
    icons::{self, Icon},
    theme,
};

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
        ui.painter()
            .rect_filled(rect, theme::corner(theme::radius::ROW), fill);
    }
    let foreground = if !ui.is_enabled() {
        theme::text_disabled()
    } else if selected || response.hovered() {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    if selected {
        let box_rect = Rect::from_center_size(
            egui::pos2(rect.right() - theme::space::MEDIUM, rect.center().y),
            Vec2::splat(icons::GRID),
        );
        icons::paint(ui.painter(), Icon::Check, box_rect, theme::accent());
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
        .inner_margin(egui::Margin::symmetric(
            theme::space::SMALL as i8,
            theme::space::SNUG as i8,
        ))
        .corner_radius(theme::corner(theme::radius::CONTROL))
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
        ui.painter().rect_filled(
            rect,
            theme::corner(theme::radius::CONTROL),
            theme::state::hover(),
        );
    }
    let color = if !ui.is_enabled() {
        theme::text_disabled()
    } else if response.hovered() {
        theme::text().primary
    } else {
        idle_color
    };
    paint(ui.painter(), rect, color);
    response.on_hover_text(label)
}

pub(crate) fn chevron_icon_button(ui: &mut Ui, upward: bool, label: &str) -> Response {
    let icon = if upward {
        Icon::ChevronUp
    } else {
        Icon::ChevronDown
    };
    icons::button_sized(
        ui,
        icon,
        label,
        theme::text().secondary,
        egui::vec2(theme::control::STANDARD, theme::control::COMPACT + 2.0),
    )
}

pub(crate) fn close_icon_button(ui: &mut Ui) -> Response {
    icons::button_sized(
        ui,
        Icon::Close,
        "Close (Esc)",
        theme::text().secondary,
        egui::vec2(theme::control::STANDARD, theme::control::COMPACT + 2.0),
    )
}

/// A key cap, a mode, a provider: one small piece of state rendered as a solid
/// token rather than as bare text.
pub(crate) fn chip(ui: &mut Ui, label: &str) -> Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::typography::code_small(),
        theme::text().primary,
    );
    let size = egui::vec2(
        galley.size().x + theme::space::MEDIUM,
        theme::control::COMPACT,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    ui.painter().rect_filled(
        rect,
        theme::corner(theme::radius::ROW),
        theme::state::selected(),
    );
    ui.painter().galley(
        egui::pos2(
            rect.center().x - galley.size().x * 0.5,
            rect.center().y - galley.size().y * 0.5,
        ),
        galley,
        theme::text().primary,
    );
    response
}

/// A segmented control cell, for the agent panel's provider and mode pickers.
pub(crate) fn segment(
    ui: &mut Ui,
    label: &str,
    selected: bool,
    trailing: Option<Icon>,
) -> Response {
    let font = theme::typography::small();
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, theme::text().secondary);
    let glyph = if trailing.is_some() {
        icons::GRID * 0.75 + theme::space::TIGHT
    } else {
        0.0
    };
    let size = egui::vec2(
        galley.size().x + glyph + theme::space::MEDIUM,
        theme::control::COMPACT,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), selected, label)
    });
    let fill = if selected {
        theme::state::selected()
    } else if response.hovered() {
        theme::state::hover()
    } else {
        Color32::TRANSPARENT
    };
    if fill != Color32::TRANSPARENT {
        ui.painter()
            .rect_filled(rect, theme::corner(theme::radius::CONTROL), fill);
    }
    let color = if selected || response.hovered() {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    ui.painter().text(
        egui::pos2(rect.left() + theme::space::SNUG, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::typography::small(),
        color,
    );
    if let Some(icon) = trailing {
        let box_rect = Rect::from_center_size(
            egui::pos2(
                rect.right() - theme::space::SNUG - icons::GRID * 0.375,
                rect.center().y,
            ),
            Vec2::splat(icons::GRID * 0.75),
        );
        icons::paint(ui.painter(), icon, box_rect, color);
    }
    response
}

fn selection_fill(selected: bool, hovered: bool) -> Color32 {
    if selected {
        theme::state::selected()
    } else if hovered {
        theme::state::hover()
    } else {
        Color32::TRANSPARENT
    }
}
