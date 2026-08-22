use egui::{Align, Align2, Color32, Id, Layout, RichText, Sense, UiBuilder};

use crate::{
    components::{segment, selectable_row},
    icons::{self, Icon},
    lsp::PresetId,
    settings::{ServerMode, UI_SCALE_MAX_PERCENT, UI_SCALE_MIN_PERCENT, UI_SCALE_STEP_PERCENT},
    theme,
};

/// Bare text that brightens on hover: no fill, no padding, just the label
/// (and room for a leading icon) sitting flush with its neighbours.
pub(super) fn settings_quiet_button(
    ui: &mut egui::Ui,
    id: impl egui::AsId,
    label: &str,
    icon_space: f32,
) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::typography::small_strong(),
        theme::text().muted,
    );
    let (_, rect) = ui.allocate_space(egui::vec2(
        galley.size().x + icon_space,
        theme::control::ROW,
    ));
    let response = ui.interact(rect, Id::new(id), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    ui.painter().text(
        egui::pos2(rect.left() + icon_space, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::typography::small_strong(),
        if response.hovered() {
            theme::text().primary
        } else {
            theme::text().muted
        },
    );
    response
}

/// A rail section: the same rounded selected fill and weight shift a session
/// row uses in the main sidebar, so the two rails read as one family.
pub(super) fn settings_navigation_row(
    ui: &mut egui::Ui,
    id: impl egui::AsId,
    label: &str,
    selected: bool,
) -> egui::Response {
    let (_, rect) = ui.allocate_space(egui::vec2(
        ui.available_width(),
        theme::control::ROW + theme::space::TIGHT,
    ));
    let response = ui.interact(rect, Id::new(id), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    if selected {
        ui.painter().rect_filled(
            rect,
            theme::corner(theme::radius::CONTROL),
            theme::state::selected(),
        );
    } else if response.hovered() {
        ui.painter().rect_filled(
            rect,
            theme::corner(theme::radius::CONTROL),
            theme::state::hover(),
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + theme::space::SMALL, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        if selected {
            theme::typography::strong()
        } else {
            theme::typography::body()
        },
        if selected || response.hovered() {
            theme::text().primary
        } else {
            theme::text().secondary
        },
    );
    response
}

pub(super) fn settings_ui_scale_slider(
    ui: &mut egui::Ui,
    value: &mut u16,
) -> (egui::Response, bool) {
    ui.spacing_mut().slider_width = 190.0;
    let mut preview = *value;
    let response = ui.add(
        egui::Slider::new(&mut preview, UI_SCALE_MIN_PERCENT..=UI_SCALE_MAX_PERCENT)
            .step_by(f64::from(UI_SCALE_STEP_PERCENT))
            .suffix("%"),
    );
    let changed = preview != *value && !response.is_pointer_button_down_on();
    if changed {
        *value = preview;
    }
    (response, changed)
}

pub(super) fn settings_page_header(ui: &mut egui::Ui, title: &str, subtitle: &str) {
    ui.label(
        RichText::new(title)
            .size(theme::typography::DISPLAY_SIZE)
            .strong()
            .color(theme::text().primary),
    );
    ui.add_space(theme::space::SNUG);
    ui.label(
        RichText::new(subtitle)
            .font(theme::typography::small())
            .color(theme::text().muted),
    );
    ui.add_space(theme::space::LARGE);
    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter()
        .hline(line.x_range(), line.center().y, theme::border::hairline());
    ui.add_space(theme::space::XWIDE);
}

pub(super) fn settings_section_label(ui: &mut egui::Ui, label: &str) {
    ui.label(
        RichText::new(label.to_ascii_uppercase())
            .size(theme::typography::MICRO_SIZE)
            .strong()
            .color(theme::text().muted),
    );
    ui.add_space(theme::space::SMALL);
}

pub(super) fn settings_card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    settings_card_margin(ui, egui::Margin::symmetric(16, 8), add);
}

pub(super) fn settings_stack_card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    settings_card_margin(ui, egui::Margin::symmetric(16, 12), add);
}

fn settings_card_margin(ui: &mut egui::Ui, margin: egui::Margin, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(settings_card_fill())
        .stroke(egui::Stroke::new(1.0, theme::border::hairline_color()))
        .corner_radius(theme::radius::CARD)
        .inner_margin(margin)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

pub(super) fn settings_card_fill() -> Color32 {
    theme::settings().card
}

/// Provider mark, title, and one-line description above a settings card.
pub(super) fn settings_identity_header(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    paint_mark: impl FnOnce(&egui::Painter, egui::Rect, Color32),
) {
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 36.0));
    let mark = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 16.0, rect.center().y),
        egui::Vec2::splat(32.0),
    );
    ui.painter().rect_filled(
        mark,
        theme::corner(theme::radius::CONTROL),
        theme::state::hover(),
    );
    paint_mark(ui.painter(), mark.shrink(7.0), theme::text().secondary);
    ui.painter().text(
        egui::pos2(mark.right() + theme::space::SMALL, rect.center().y - 8.0),
        Align2::LEFT_CENTER,
        title,
        theme::typography::title(),
        theme::text().primary,
    );
    ui.painter().text(
        egui::pos2(mark.right() + theme::space::SMALL, rect.center().y + 9.0),
        Align2::LEFT_CENTER,
        detail,
        theme::typography::small(),
        theme::text().muted,
    );
    ui.add_space(theme::space::MEDIUM);
}

pub(super) fn settings_card_action(
    ui: &mut egui::Ui,
    id: impl egui::AsId,
    icon: Icon,
    label: &str,
) -> egui::Response {
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), theme::control::PRIMARY));
    let response = ui.interact(rect, Id::new(id), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let hovered = response.hovered();
    if hovered {
        ui.painter().rect_filled(
            rect,
            theme::corner(theme::radius::CONTROL),
            theme::state::hover(),
        );
    }
    let color = if hovered {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(rect.left() + icons::GRID * 0.5, rect.center().y),
        egui::Vec2::splat(icons::GRID),
    );
    icons::paint(ui.painter(), icon, icon_rect, color);
    ui.painter().text(
        egui::pos2(icon_rect.right() + theme::space::SNUG, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::typography::body(),
        color,
    );
    response
}

pub(super) fn settings_row(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    settings_named_row(ui, title, detail, theme::typography::body(), control);
}

pub(super) fn settings_account_row(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    settings_named_row(ui, title, detail, theme::typography::strong(), control);
}

fn settings_named_row(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    title_font: egui::FontId,
    control: impl FnOnce(&mut egui::Ui),
) {
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 56.0));
    ui.painter().text(
        egui::pos2(rect.left(), rect.center().y - 10.0),
        Align2::LEFT_CENTER,
        title,
        title_font,
        theme::text().primary,
    );
    ui.painter().text(
        egui::pos2(rect.left(), rect.center().y + 10.0),
        Align2::LEFT_CENTER,
        detail,
        theme::typography::micro(),
        theme::text().muted,
    );
    let controls = egui::Rect::from_min_max(
        egui::pos2(rect.left() + rect.width() * 0.42, rect.top()),
        rect.right_bottom(),
    );
    ui.scope_builder(UiBuilder::new().max_rect(controls), |ui| {
        ui.set_width(ui.available_width());
        ui.with_layout(Layout::right_to_left(Align::Center), control);
    });
}

pub(super) fn settings_choice_row<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    value: &mut T,
    choices: impl IntoIterator<Item = (T, &'static str)>,
) -> bool {
    let mut dirty = false;
    let choices = choices.into_iter().collect::<Vec<_>>();
    settings_row(ui, title, detail, |ui| {
        for (candidate, label) in choices.into_iter().rev() {
            let selected = *value == candidate;
            let response = segment(ui, label, selected, None);
            if response.clicked() && !selected {
                *value = candidate;
                dirty = true;
            }
        }
    });
    dirty
}

pub(super) fn settings_switch_row(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    enabled: bool,
) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 56.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), enabled, title)
    });
    ui.painter().text(
        egui::pos2(rect.left(), rect.center().y - 10.0),
        Align2::LEFT_CENTER,
        title,
        theme::typography::body(),
        theme::text().primary,
    );
    ui.painter().text(
        egui::pos2(rect.left(), rect.center().y + 10.0),
        Align2::LEFT_CENTER,
        detail,
        theme::typography::micro(),
        theme::text().muted,
    );
    let track = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 18.0, rect.center().y),
        egui::vec2(36.0, 20.0),
    );
    ui.painter().rect_filled(
        track,
        10.0,
        if enabled {
            theme::accent()
        } else {
            theme::border::strong_color()
        },
    );
    ui.painter().circle_filled(
        egui::pos2(
            if enabled {
                track.right() - 9.0
            } else {
                track.left() + 9.0
            },
            track.center().y,
        ),
        7.0,
        if enabled {
            theme::text().on_accent
        } else {
            theme::text().secondary
        },
    );
    response
}

pub(super) fn settings_secondary_button(
    ui: &mut egui::Ui,
    id: impl egui::AsId,
    label: &str,
) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::typography::small(),
        theme::text().secondary,
    );
    let (_, rect) = ui.allocate_space(egui::vec2(
        (galley.size().x + theme::space::MEDIUM * 2.0).max(64.0),
        theme::control::ROW,
    ));
    let response = ui.interact(rect, Id::new(id), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let hovered = response.hovered();
    let fill = if hovered {
        theme::composite(theme::state::hover(), theme::settings().control)
    } else {
        theme::settings().control
    };
    ui.painter().rect(
        rect,
        theme::corner(theme::radius::CONTROL),
        fill,
        egui::Stroke::new(
            1.0,
            if hovered {
                theme::border::strong_color()
            } else {
                theme::border::hairline_color()
            },
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::typography::small(),
        if hovered {
            theme::text().primary
        } else {
            theme::text().secondary
        },
    );
    response
}

pub(super) fn settings_danger_button(
    ui: &mut egui::Ui,
    id: impl egui::AsId,
    label: &str,
) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::typography::small(),
        theme::ink(theme::semantic().danger),
    );
    let (_, rect) = ui.allocate_space(egui::vec2(
        (galley.size().x + theme::space::MEDIUM * 2.0).max(64.0),
        theme::control::ROW,
    ));
    let response = ui.interact(rect, Id::new(id), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let hovered = response.hovered();
    ui.painter().rect(
        rect,
        theme::corner(theme::radius::CONTROL),
        if hovered {
            theme::callout(theme::semantic().danger).fill
        } else {
            theme::settings().control
        },
        egui::Stroke::new(
            1.0,
            if hovered {
                theme::ink(theme::semantic().danger)
            } else {
                theme::border::hairline_color()
            },
        ),
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::typography::small(),
        theme::ink(theme::semantic().danger),
    );
    response
}

pub(super) fn settings_primary_button(
    ui: &mut egui::Ui,
    id: impl egui::AsId,
    label: &str,
) -> egui::Response {
    let enabled = ui.is_enabled();
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::typography::small(),
        theme::text().on_accent,
    );
    let (_, rect) = ui.allocate_space(egui::vec2(
        (galley.size().x + theme::space::MEDIUM * 2.0).max(76.0),
        theme::control::ROW,
    ));
    let response = ui.interact(rect, Id::new(id), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, label));
    let (fill, stroke, text) = if !enabled {
        (
            theme::settings().control,
            egui::Stroke::new(1.0, theme::border::hairline_color()),
            theme::text_disabled(),
        )
    } else if response.hovered() {
        (
            theme::composite(theme::state::hover(), theme::accent()),
            egui::Stroke::NONE,
            theme::text().on_accent,
        )
    } else {
        (theme::accent(), egui::Stroke::NONE, theme::text().on_accent)
    };
    ui.painter().rect(
        rect,
        theme::corner(theme::radius::CONTROL),
        fill,
        stroke,
        egui::StrokeKind::Inside,
    );
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::typography::small(),
        text,
    );
    response
}

pub(super) fn settings_combo_box(
    ui: &mut egui::Ui,
    id: impl egui::AsIdSalt,
    width: f32,
    selected: &str,
    add: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = egui::vec2(11.0, 7.0);
        ui.spacing_mut().interact_size.y = theme::control::STANDARD;
        let control = theme::settings().control;
        let visuals = &mut ui.style_mut().visuals.widgets;
        visuals.inactive.weak_bg_fill = control;
        visuals.inactive.bg_stroke = egui::Stroke::new(1.0, theme::border::hairline_color());
        visuals.hovered.weak_bg_fill = theme::composite(theme::state::hover(), control);
        visuals.hovered.bg_stroke = egui::Stroke::new(1.0, theme::border::strong_color());
        visuals.active.weak_bg_fill = theme::composite(theme::state::selected(), control);
        visuals.active.bg_stroke = egui::Stroke::new(1.0, theme::border::strong_color());
        visuals.open.weak_bg_fill = theme::composite(theme::state::selected(), control);
        visuals.open.bg_stroke = egui::Stroke::new(1.0, theme::accent());
        visuals.inactive.corner_radius = 6.into();
        visuals.hovered.corner_radius = 6.into();
        visuals.active.corner_radius = 6.into();
        visuals.open.corner_radius = 6.into();
        egui::ComboBox::from_id_salt(id)
            .width(width)
            .selected_text(
                RichText::new(selected)
                    .color(theme::text().primary)
                    .size(theme::typography::SMALL_SIZE),
            )
            .icon(|ui, rect, visuals, open| {
                icons::paint(
                    ui.painter(),
                    if open {
                        Icon::ChevronUp
                    } else {
                        Icon::ChevronDown
                    },
                    egui::Rect::from_center_size(
                        rect.center(),
                        egui::Vec2::splat(icons::GRID * 0.75),
                    ),
                    visuals.fg_stroke.color,
                );
            })
            .popup_style(egui::style::StyleModifier::new(|style| {
                style.spacing.item_spacing.y = 2.0;
                style.visuals.window_fill = theme::surface().raised;
                style.visuals.window_stroke = egui::Stroke::new(1.0, theme::border::strong_color());
                style.visuals.menu_corner_radius = theme::corner(theme::radius::CARD);
            }))
            .show_ui(ui, add)
            .response
    })
    .inner
}

pub(super) fn settings_combo_choice(ui: &mut egui::Ui, label: &str, selected: bool) -> bool {
    let row = selectable_row(ui, label, selected, 34.0);
    ui.painter().text(
        egui::pos2(row.rect.left() + 9.0, row.rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::typography::small(),
        row.foreground,
    );
    row.response.clicked()
}

pub(super) fn settings_mode_combo(
    ui: &mut egui::Ui,
    preset: PresetId,
    mode: &mut ServerMode,
) -> egui::Response {
    let label = match mode {
        ServerMode::Auto => "Auto",
        ServerMode::Custom => "Custom",
        ServerMode::Off => "Off",
    };
    settings_combo_box(ui, ("server_mode", preset.as_str()), 120.0, label, |ui| {
        for (candidate, label) in [
            (ServerMode::Auto, "Auto"),
            (ServerMode::Custom, "Custom"),
            (ServerMode::Off, "Off"),
        ] {
            if settings_combo_choice(ui, label, *mode == candidate) {
                *mode = candidate;
            }
        }
    })
}
