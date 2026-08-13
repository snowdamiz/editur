use egui::{Align, Align2, Color32, Id, Layout, RichText, Sense, UiBuilder};

use crate::{
    components::{segment, selectable_row},
    icons::{self, Icon},
    lsp::PresetId,
    settings::{ServerMode, UI_SCALE_MAX_PERCENT, UI_SCALE_MIN_PERCENT, UI_SCALE_STEP_PERCENT},
    theme,
};

pub(super) fn settings_quiet_button(
    ui: &mut egui::Ui,
    id: impl egui::AsId,
    label: &str,
    icon_space: f32,
) -> egui::Response {
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::typography::body(),
        theme::text().secondary,
    );
    let (_, rect) = ui.allocate_space(egui::vec2(
        (galley.size().x + icon_space + 20.0).max(40.0),
        40.0,
    ));
    let response = ui.interact(rect, Id::new(id), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    let color = if response.hovered() {
        theme::text().primary
    } else {
        theme::text().secondary
    };
    ui.painter().text(
        egui::pos2(rect.left() + 10.0 + icon_space, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::typography::body(),
        color,
    );
    response
}

pub(super) fn settings_navigation_row(
    ui: &mut egui::Ui,
    id: impl egui::AsId,
    label: &str,
    selected: bool,
) -> egui::Response {
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 40.0));
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
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 1.0, rect.center().y),
                egui::vec2(2.0, 18.0),
            ),
            1.0,
            theme::accent(),
        );
    }
    ui.painter().text(
        egui::pos2(rect.left() + 12.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        theme::typography::body(),
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
    ui.add_space(6.0);
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
    egui::Frame::new()
        .fill(settings_card_fill())
        .stroke(egui::Stroke::new(1.0, theme::border::hairline_color()))
        .corner_radius(theme::radius::CARD)
        .inner_margin(egui::Margin::symmetric(18, 8))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

pub(super) fn settings_card_fill() -> Color32 {
    theme::settings().card
}

pub(super) fn settings_row(
    ui: &mut egui::Ui,
    title: &str,
    detail: &str,
    control: impl FnOnce(&mut egui::Ui),
) {
    let (_, rect) = ui.allocate_space(egui::vec2(ui.available_width(), 56.0));
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
    let (_, rect) = ui.allocate_space(egui::vec2((galley.size().x + 22.0).max(64.0), 30.0));
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
    let (_, rect) = ui.allocate_space(egui::vec2((galley.size().x + 22.0).max(64.0), 30.0));
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
    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        theme::typography::small(),
        theme::text().on_accent,
    );
    let (_, rect) = ui.allocate_space(egui::vec2((galley.size().x + 24.0).max(88.0), 40.0));
    let response = ui.interact(rect, Id::new(id), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    ui.painter().rect_filled(rect, 6.0, theme::accent());
    ui.painter().text(
        rect.center(),
        Align2::CENTER_CENTER,
        label,
        theme::typography::small(),
        theme::text().on_accent,
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
        ui.spacing_mut().interact_size.y = 34.0;
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
