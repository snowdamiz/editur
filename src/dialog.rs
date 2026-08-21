//! One modal component. Every dialog in the product is a `Dialog`, so the
//! frame, the scrim, the spacing, the button order, and the keyboard are
//! decided once instead of at seven call sites.

use std::borrow::Cow;

use egui::{Align, Context, Id, Key, Layout, Modifiers, Response, RichText, Ui};

use crate::{
    icons::{self, Icon},
    theme,
};

/// What the dialog is about. The glyph and its tint are the only difference a
/// severity makes; nothing else about the anatomy moves.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(crate) enum Severity {
    #[default]
    Neutral,
    Info,
    Warning,
    Danger,
}

impl Severity {
    fn glyph(self) -> Option<(Icon, egui::Color32)> {
        let semantic = theme::semantic();
        match self {
            Self::Neutral => None,
            Self::Info => Some((Icon::Info, semantic.info)),
            Self::Warning => Some((Icon::Warning, semantic.warning)),
            Self::Danger => Some((Icon::Error, semantic.danger)),
        }
    }
}

/// How the dialog ended. `Open` means it is still on screen, which is what
/// removes the `let mut save_clicked = false` dance from every call site.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Outcome {
    Open,
    Primary,
    Neutral,
    Destructive,
    Cancel,
    /// Esc, or a click on the scrim of a dialog with nothing destructive in it.
    Dismissed,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Variant {
    Primary,
    Neutral,
    Destructive,
}

/// The fixed anatomy, from §9 of the design plan.
const MIN_WIDTH: f32 = 380.0;
const MAX_WIDTH: f32 = 520.0;
const ACTION_WIDTH: f32 = 84.0;

pub(crate) struct Dialog<'a> {
    id: &'static str,
    title: &'a str,
    severity: Severity,
    body: Option<Cow<'a, str>>,
    path: Option<String>,
    primary: Option<&'a str>,
    primary_enabled: bool,
    neutral: Option<&'a str>,
    destructive: Option<&'a str>,
    cancel: Option<&'a str>,
    keyboard: bool,
}

impl<'a> Dialog<'a> {
    pub(crate) fn new(id: &'static str, title: &'a str) -> Self {
        Self {
            id,
            title,
            severity: Severity::Neutral,
            body: None,
            path: None,
            primary: None,
            primary_enabled: true,
            neutral: None,
            destructive: None,
            cancel: Some("Cancel"),
            keyboard: true,
        }
    }

    pub(crate) fn severity(mut self, severity: Severity) -> Self {
        self.severity = severity;
        self
    }

    pub(crate) fn body(mut self, body: impl Into<Cow<'a, str>>) -> Self {
        self.body = Some(body.into());
        self
    }

    /// A file path, which renders monospace and middle-truncates so both the
    /// directory and the filename survive.
    pub(crate) fn path(mut self, path: &std::path::Path) -> Self {
        self.path = Some(middle_truncate(&path.display().to_string(), 64));
        self
    }

    pub(crate) fn primary(mut self, label: &'a str) -> Self {
        self.primary = Some(label);
        self
    }

    /// A primary action the dialog is not yet ready to run, such as Save with
    /// an empty path.
    pub(crate) fn primary_enabled(mut self, enabled: bool) -> Self {
        self.primary_enabled = enabled;
        self
    }

    pub(crate) fn neutral(mut self, label: &'a str) -> Self {
        self.neutral = Some(label);
        self
    }

    pub(crate) fn destructive(mut self, label: &'a str) -> Self {
        self.destructive = Some(label);
        self
    }

    /// For the one dialog that is itself a key capture field, and so cannot
    /// hand Enter and Esc to the buttons.
    pub(crate) fn without_keyboard(mut self) -> Self {
        self.keyboard = false;
        self
    }

    pub(crate) fn show(self, ctx: &Context) -> Outcome {
        self.show_with(ctx, |_| {})
    }

    /// `content` renders between the body and the actions: a text field, a list,
    /// a chip row. It is the only part of a dialog a call site controls.
    pub(crate) fn show_with(self, ctx: &Context, content: impl FnOnce(&mut Ui)) -> Outcome {
        let frame = egui::Frame::new()
            .fill(theme::surface().input)
            .stroke(theme::border::strong())
            .corner_radius(theme::corner(theme::radius::DIALOG))
            .shadow(theme::shadow::dialog())
            .inner_margin(theme::space::WIDE as i8);
        let destructive_present = self.destructive.is_some();
        let keyboard = self.keyboard;
        let first_frame = ctx.data_mut(|data| {
            data.get_temp_mut_or_insert_with(self.opened_id(), || true)
                .to_owned()
        });
        ctx.data_mut(|data| data.insert_temp(self.opened_id(), false));

        let appear =
            theme::motion::animate(ctx, Id::new((self.id, "appear")), true, theme::motion::BASE);
        let modal = egui::Modal::new(Id::new(self.id))
            .backdrop_color(theme::motion::fade(theme::state::scrim(), appear))
            .frame(frame)
            .show(ctx, |ui| self.contents(ui, content, first_frame));

        let mut outcome = modal.inner;
        if outcome == Outcome::Open && modal.backdrop_response.clicked() && !destructive_present {
            outcome = Outcome::Dismissed;
        }
        if outcome == Outcome::Open
            && keyboard
            && !first_frame
            && modal.is_top_modal
            && !modal.any_popup_open
            && ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape))
        {
            outcome = Outcome::Dismissed;
        }
        if outcome != Outcome::Open {
            // The next time this dialog opens it is a first frame again, so it
            // takes focus again.
            ctx.data_mut(|data| data.remove::<bool>(self.opened_id()));
        }
        outcome
    }

    fn opened_id(&self) -> Id {
        Id::new((self.id, "opened"))
    }

    fn contents(&self, ui: &mut Ui, content: impl FnOnce(&mut Ui), first_frame: bool) -> Outcome {
        ui.visuals_mut().text_edit_bg_color = Some(theme::surface().raised);
        ui.set_min_width(MIN_WIDTH - theme::space::WIDE * 2.0);
        ui.set_max_width(MAX_WIDTH - theme::space::WIDE * 2.0);
        ui.spacing_mut().item_spacing = egui::vec2(theme::space::SMALL, theme::space::SMALL);

        ui.horizontal(|ui| {
            if let Some((icon, tint)) = self.severity.glyph() {
                let (rect, _) = ui.allocate_exact_size(
                    egui::Vec2::splat(theme::space::WIDE),
                    egui::Sense::hover(),
                );
                icons::paint(ui.painter(), icon, rect, tint);
            }
            ui.add(
                egui::Label::new(
                    RichText::new(self.title)
                        .font(theme::typography::title())
                        .color(theme::text().primary),
                )
                .truncate(),
            );
        });

        if self.body.is_some() || self.path.is_some() {
            ui.add_space(theme::space::TIGHT);
        }
        if let Some(body) = self.body.as_deref() {
            ui.label(
                RichText::new(body)
                    .font(theme::typography::body())
                    .color(theme::text().secondary),
            );
        }
        if let Some(path) = self.path.as_deref() {
            ui.label(
                RichText::new(path)
                    .font(theme::typography::code_small())
                    .color(theme::text().muted),
            );
        }

        content(ui);

        ui.add_space(theme::space::MEDIUM);
        let mut outcome = Outcome::Open;
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), theme::control::STANDARD),
            Layout::right_to_left(Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = theme::space::SMALL;
                if let Some(label) = self.primary {
                    let response = ui.add_enabled_ui(self.primary_enabled, |ui| {
                        action(ui, label, Variant::Primary)
                    });
                    if first_frame
                        && self.primary_enabled
                        && ui.memory(|memory| memory.focused()).is_none()
                    {
                        response.inner.request_focus();
                    }
                    if response.inner.clicked() {
                        outcome = Outcome::Primary;
                    }
                }
                if let Some(label) = self.cancel
                    && action(ui, label, Variant::Neutral).clicked()
                {
                    outcome = Outcome::Cancel;
                }
                if let Some(label) = self.neutral
                    && action(ui, label, Variant::Neutral).clicked()
                {
                    outcome = Outcome::Neutral;
                }
                if let Some(label) = self.destructive
                    && action(ui, label, Variant::Destructive).clicked()
                {
                    outcome = Outcome::Destructive;
                }
            },
        );

        // The keystroke that opened a dialog must not also answer it, so the
        // Enter binding waits until the second frame.
        let enter_bound = self.keyboard && !first_frame && self.primary_enabled;
        if outcome == Outcome::Open
            && enter_bound
            && self.primary.is_some()
            && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter))
        {
            outcome = Outcome::Primary;
        }
        outcome
    }
}

/// One action button. Height, width floor, radius, and focus ring are the same
/// for all three variants so the row reads as one control group.
fn action(ui: &mut Ui, label: &str, variant: Variant) -> Response {
    let (fill, foreground, stroke) = match variant {
        Variant::Primary => (theme::accent(), theme::text().on_accent, egui::Stroke::NONE),
        Variant::Neutral => (
            theme::state::selected(),
            theme::text().primary,
            egui::Stroke::NONE,
        ),
        Variant::Destructive => {
            let callout = theme::callout(theme::semantic().danger);
            (
                callout.fill,
                callout.text,
                egui::Stroke::new(theme::stroke::DIVIDER, callout.border),
            )
        }
    };
    let response = ui.add(
        egui::Button::new(
            RichText::new(label)
                .font(theme::typography::strong())
                .color(foreground),
        )
        .fill(fill)
        .stroke(stroke)
        .corner_radius(theme::corner(theme::radius::CONTROL))
        .min_size(egui::vec2(ACTION_WIDTH, theme::control::STANDARD)),
    );
    if response.has_focus() {
        icons::focus_ring(ui.painter(), response.rect, theme::radius::CONTROL);
    }
    response
}

/// Keeps the head and the tail of a path, which is where the meaning is.
pub(crate) fn middle_truncate(text: &str, budget: usize) -> String {
    let characters = text.chars().count();
    if characters <= budget {
        return text.to_owned();
    }
    let tail = (budget * 2 / 3).min(characters);
    let head = budget.saturating_sub(tail);
    let start: String = text.chars().take(head).collect();
    let end: String = text.chars().skip(characters - tail).collect();
    format!("{start}…{end}")
}

#[cfg(test)]
mod tests {
    use super::{Dialog, Outcome, Severity, middle_truncate};
    use crate::theme;
    use egui::{Event, Key, Modifiers, RawInput, Rect, Shape, Vec2, pos2};

    /// egui measures an anchored area on its first pass, so a dialog is only
    /// on screen from the second frame; every test here runs a settling frame
    /// first, exactly like the running product does.
    fn run(
        context: &egui::Context,
        events: Vec<Event>,
        build: fn(&egui::Context) -> Outcome,
    ) -> (Outcome, egui::FullOutput) {
        let frame = |events: Vec<Event>| {
            let mut outcome = Outcome::Open;
            let output = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::new(900.0, 600.0))),
                    events,
                    ..RawInput::default()
                },
                |ui| outcome = build(ui.ctx()),
            );
            (outcome, output)
        };
        frame(Vec::new());
        frame(events)
    }

    fn unsaved(ctx: &egui::Context) -> Outcome {
        Dialog::new("unsaved_dialog", "Unsaved changes")
            .severity(Severity::Warning)
            .body("Save your changes before continuing?")
            .destructive("Discard")
            .primary("Save")
            .show(ctx)
    }

    #[test]
    fn a_dialog_titles_itself_once_and_binds_enter_to_its_primary_action() {
        let context = theme::test_context();

        let (_, output) = run(&context, Vec::new(), unsaved);
        fn titles(shape: &Shape, found: &mut usize) {
            match shape {
                Shape::Text(text) if text.galley.text() == "Unsaved changes" => *found += 1,
                Shape::Vec(shapes) => shapes.iter().for_each(|shape| titles(shape, found)),
                _ => {}
            }
        }
        let mut found = 0;
        for shape in &output.shapes {
            titles(&shape.shape, &mut found);
        }
        assert_eq!(found, 1, "a dialog draws its title exactly once");

        let (outcome, _) = run(
            &context,
            vec![Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            unsaved,
        );
        assert_eq!(outcome, Outcome::Primary);

        let (outcome, _) = run(
            &context,
            vec![Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
            unsaved,
        );
        assert_eq!(outcome, Outcome::Dismissed);
    }

    #[test]
    fn every_action_shares_one_height_and_one_width_floor() {
        let context = theme::test_context();

        let (_, output) = run(&context, Vec::new(), |ctx| {
            Dialog::new("conflict_dialog", "File changed on disk")
                .severity(Severity::Warning)
                .body("Reloading drops the edits in this buffer.")
                .neutral("Save As…")
                .primary("Reload")
                .show(ctx)
        });

        fn buttons(shape: &Shape, found: &mut Vec<Rect>) {
            match shape {
                Shape::Rect(rect)
                    if rect.rect.height() == theme::control::STANDARD
                        && rect.fill != egui::Color32::TRANSPARENT =>
                {
                    found.push(rect.rect);
                }
                Shape::Vec(shapes) => shapes.iter().for_each(|shape| buttons(shape, found)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for shape in &output.shapes {
            buttons(&shape.shape, &mut found);
        }
        assert_eq!(found.len(), 3, "reload, cancel, and save-as: {found:?}");
        assert!(
            found
                .iter()
                .all(|rect| rect.width() >= super::ACTION_WIDTH - 0.5),
            "an action narrower than the floor breaks the row: {found:?}"
        );
    }

    #[test]
    fn dialogs_match_the_file_picker_surface_and_lift_text_inputs() {
        let context = theme::test_context();
        let (_, output) = run(&context, Vec::new(), |ctx| {
            let mut name = "Account 2".to_owned();
            Dialog::new("field_dialog", "Add account")
                .primary("Add")
                .show_with(ctx, |ui| {
                    ui.add(egui::TextEdit::singleline(&mut name).desired_width(320.0));
                })
        });

        fn rectangles(shape: &Shape, found: &mut Vec<(Rect, egui::Color32)>) {
            match shape {
                Shape::Rect(rect) => found.push((rect.rect, rect.fill)),
                Shape::Vec(shapes) => shapes.iter().for_each(|shape| rectangles(shape, found)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for shape in &output.shapes {
            rectangles(&shape.shape, &mut found);
        }
        let dialog_fill = found
            .iter()
            .filter(|(rect, fill)| {
                rect.width() >= super::MIN_WIDTH - 1.0
                    && (*fill == theme::surface().input || *fill == theme::surface().raised)
            })
            .max_by(|(left, _), (right, _)| left.area().total_cmp(&right.area()))
            .map(|(_, fill)| *fill);
        assert_eq!(dialog_fill, Some(theme::surface().input));
        assert!(found.iter().any(|(rect, fill)| {
            *fill == theme::surface().raised && rect.width() > 250.0 && rect.height() < 50.0
        }));
    }

    #[test]
    fn a_scrim_click_cancels_a_safe_dialog_and_never_a_destructive_one() {
        fn click(context: &egui::Context, build: fn(&egui::Context) -> Outcome) -> Outcome {
            let corner = pos2(40.0, 40.0);
            let button = |pressed| Event::PointerButton {
                pos: corner,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            // Hover resolves a frame after the pointer moves, so the press has
            // to arrive after the move has been seen.
            run(context, vec![Event::PointerMoved(corner)], build);
            run(context, vec![button(true), button(false)], build).0
        }

        let context = theme::test_context();
        assert_eq!(
            click(&context, |ctx| Dialog::new("save_as_dialog", "Save As")
                .primary("Save")
                .show(ctx)),
            Outcome::Dismissed
        );

        let context = theme::test_context();
        assert_eq!(
            click(&context, unsaved),
            Outcome::Open,
            "a stray click must not discard a buffer"
        );
    }

    #[test]
    fn a_path_keeps_its_directory_and_its_filename() {
        let long = "/Users/example/Documents/projects/editur/src/very/deep/module/file.rs";

        let truncated = middle_truncate(long, 40);

        assert!(truncated.chars().count() <= 41);
        assert!(truncated.starts_with("/Users/"));
        assert!(truncated.ends_with("file.rs"));
    }
}
