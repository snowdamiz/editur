use super::*;

pub fn launch(target: OpenTarget, started: Instant) -> Result<(), String> {
    if launch_in_current_process(
        std::io::stdin().is_terminal(),
        std::io::stdout().is_terminal(),
        std::io::stderr().is_terminal(),
        cfg!(target_os = "macos") && std::env::var_os("__CFBundleIdentifier").is_some(),
    ) {
        return run(target, started);
    }
    if open_running(&target)? {
        return Ok(());
    }
    let executable = std::env::current_exe()
        .map_err(|error| format!("cannot locate the Editur executable: {error}"))?;
    let path = target.file.as_ref().unwrap_or(&target.root);
    let mut command = Command::new(executable);
    #[cfg(target_os = "macos")]
    command
        .env_remove("__CFBundleIdentifier")
        .env_remove("XPC_SERVICE_NAME");
    command
        .arg("--resident")
        .arg(path)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    detach(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("cannot start the editor resident: {error}"))
}

pub(super) const fn should_show_project_chooser(
    path_provided: bool,
    macos_bundle_launch: bool,
) -> bool {
    !path_provided && macos_bundle_launch
}

pub fn should_choose_project(path_provided: bool) -> bool {
    should_show_project_chooser(
        path_provided,
        cfg!(target_os = "macos") && std::env::var_os("__CFBundleIdentifier").is_some(),
    )
}

pub fn choose_project(started: Instant) -> Result<(), String> {
    let mut event_loop = EventLoop::<()>::with_user_event();
    #[cfg(target_os = "macos")]
    winit::platform::macos::EventLoopBuilderExtMacOS::with_default_menu(&mut event_loop, false);
    let event_loop = event_loop
        .build()
        .map_err(|error| format!("cannot create project chooser event loop: {error}"))?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut chooser = ProjectChooserShell::new(event_loop.create_proxy());
    #[cfg(target_os = "macos")]
    chooser.open_before_launch(&event_loop)?;
    event_loop
        .run_app(&mut chooser)
        .map_err(|error| format!("project chooser event loop failed: {error}"))?;
    let selected = chooser.selected.take();
    let fatal = chooser.fatal.take();
    drop(chooser);
    if let Some(error) = fatal {
        return Err(error);
    }
    let Some(path) = selected else {
        return Ok(());
    };
    let target = resolve_target(&path, Some(&path))?;
    launch(target, started)
}

struct ProjectChooserShell {
    window: Option<Window>,
    renderer: Option<Renderer>,
    egui: Option<egui_winit::State>,
    picker: Option<AgentFilePicker>,
    selected: Option<PathBuf>,
    error: Option<String>,
    fatal: Option<String>,
    repaint_at: Option<Instant>,
    event_proxy: EventLoopProxy<()>,
}

impl ProjectChooserShell {
    fn new(event_proxy: EventLoopProxy<()>) -> Self {
        Self {
            window: None,
            renderer: None,
            egui: None,
            picker: None,
            selected: None,
            error: None,
            fatal: None,
            repaint_at: None,
            event_proxy,
        }
    }

    #[cfg(target_os = "macos")]
    #[allow(deprecated)]
    fn open_before_launch(&mut self, event_loop: &EventLoop<()>) -> Result<(), String> {
        let attributes = project_chooser_window_attributes()?;
        let window_started = Instant::now();
        let window = create_macos_window_without_native_title(
            |attributes| event_loop.create_window(attributes),
            attributes,
        )
        .map_err(|error| format!("cannot create project chooser window: {error}"))?;
        if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
            eprintln!(
                "editur: project chooser window created before launch in {:.2?}",
                window_started.elapsed()
            );
        }
        self.install_window(window, event_loop)?;
        self.draw_before_launch()
    }

    fn install_window(
        &mut self,
        window: Window,
        display_target: &dyn winit::raw_window_handle::HasDisplayHandle,
    ) -> Result<(), String> {
        let renderer = Renderer::new(&window)?;
        let context = egui::Context::default();
        theme::apply(&context);
        disable_transient_egui_debug_overlays(&context);
        let event_proxy = self.event_proxy.clone();
        install_repaint_wake(&context, move || {
            let _ = event_proxy.send_event(());
        });
        let state = egui_winit::State::new(
            context,
            ViewportId::ROOT,
            display_target,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        self.egui = Some(state);
        self.renderer = Some(renderer);
        self.window = Some(window);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn draw_before_launch(&mut self) -> Result<(), String> {
        let error = self.error.as_deref();
        let (window, renderer, state) = (
            self.window.as_ref().expect("chooser window is installed"),
            self.renderer
                .as_mut()
                .expect("chooser renderer is installed"),
            self.egui.as_mut().expect("chooser egui state is installed"),
        );
        let input = state.take_egui_input(window);
        let context = state.egui_ctx().clone();
        let output = context.run_ui(input, |root| {
            let _ = project_chooser_ui(root, error);
        });
        state.handle_platform_output(window, output.platform_output);
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        renderer
            .render(output.pixels_per_point, &primitives, &output.textures_delta)
            .map(|_| ())
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        self.fatal = Some(error);
        event_loop.exit();
    }

    fn select(&mut self, event_loop: &ActiveEventLoop, path: PathBuf) {
        if path.is_dir() {
            self.selected = Some(path);
            event_loop.exit();
        } else {
            self.error = Some("Choose a folder containing your project.".into());
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }

    fn resize(&mut self, _event_loop: &ActiveEventLoop, size: winit::dpi::PhysicalSize<u32>) {
        let (Some(_window), Some(renderer)) = (self.window.as_ref(), self.renderer.as_mut()) else {
            return;
        };
        #[cfg(target_os = "macos")]
        if let Err(error) = renderer.resize(_window, size) {
            self.fail(_event_loop, error);
            return;
        }
        #[cfg(not(target_os = "macos"))]
        renderer.resize(size);
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        let (Some(window), Some(renderer), Some(state)) = (
            self.window.as_ref(),
            self.renderer.as_mut(),
            self.egui.as_mut(),
        ) else {
            return;
        };
        let input = state.take_egui_input(window);
        let context = state.egui_ctx().clone();
        let error = self.error.as_deref();
        let mut picker = self.picker.as_mut();
        let mut chooser_action = (false, None);
        let mut picker_outcome = None;
        let output = context.run_ui(input, |root| {
            chooser_action = project_chooser_ui(root, error);
            if let Some(picker) = picker.as_deref_mut() {
                picker_outcome = EditorApp::file_picker_dialog(root.ctx(), picker, None, 0);
            }
        });
        let (browse, window_action) = chooser_action;
        state.handle_platform_output_with_event_loop(window, event_loop, output.platform_output);
        let textures_updated = !output.textures_delta.set.is_empty();
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        #[cfg(target_os = "linux")]
        window.pre_present_notify();
        #[cfg(target_os = "macos")]
        let rendered =
            renderer.render(output.pixels_per_point, &primitives, &output.textures_delta);
        #[cfg(not(target_os = "macos"))]
        let rendered = renderer
            .render(output.pixels_per_point, &primitives, &output.textures_delta)
            .map(|()| true);
        match rendered {
            Ok(true) => {}
            Ok(false) => {
                window.request_redraw();
                return;
            }
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        }

        if let Some(action) = window_action {
            match action {
                WindowAction::Close => {
                    event_loop.exit();
                    return;
                }
                WindowAction::Minimize => window.set_minimized(true),
                WindowAction::ToggleMaximize => {
                    if let Err(error) = toggle_window_maximize(window) {
                        self.error = Some(error);
                        window.request_redraw();
                    }
                }
                WindowAction::Drag => {
                    if let Err(error) = window.drag_window() {
                        self.error = Some(format!("cannot drag window: {error}"));
                        window.request_redraw();
                    }
                }
            }
        }

        if let Some(outcome) = picker_outcome {
            match outcome {
                FilePickerOutcome::Dismissed => {
                    self.picker = None;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
                }
                FilePickerOutcome::OpenDirectory(path) => {
                    self.picker = None;
                    self.select(event_loop, path);
                }
                FilePickerOutcome::AttachFiles(_) => {}
            }
            return;
        }

        if browse && self.picker.is_none() {
            let start = directories::UserDirs::new()
                .map(|directories| directories.home_dir().to_path_buf())
                .or_else(|| std::env::current_dir().ok())
                .unwrap_or_else(|| PathBuf::from("/"));
            match AgentFilePicker::open_directories(start) {
                Ok(picker) => self.picker = Some(picker),
                Err(error) => self.error = Some(error),
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        let delay = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .map_or(Duration::MAX, |output| output.repaint_delay);
        let delay = repaint_delay_after_texture_update(delay, textures_updated);
        if let Some(repaint_at) = repaint_deadline(delay, Instant::now()) {
            self.repaint_at = Some(repaint_at);
            event_loop.set_control_flow(if delay.is_zero() {
                ControlFlow::Poll
            } else {
                ControlFlow::WaitUntil(repaint_at)
            });
        } else {
            self.repaint_at = None;
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

impl ApplicationHandler<()> for ProjectChooserShell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            #[cfg(target_os = "macos")]
            activate_macos_application();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        let attributes = match project_chooser_window_attributes() {
            Ok(attributes) => attributes,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        let display = opening_display(event_loop, attributes.position);
        let attributes = fit_startup_window_attributes(attributes, display, false);
        let window_started = Instant::now();
        #[cfg(target_os = "macos")]
        let window = create_macos_window_without_native_title(
            |attributes| event_loop.create_window(attributes),
            attributes,
        );
        #[cfg(not(target_os = "macos"))]
        let window = event_loop.create_window(attributes);
        let window = match window {
            Ok(window) => window,
            Err(error) => {
                self.fail(
                    event_loop,
                    format!("cannot create project chooser window: {error}"),
                );
                return;
            }
        };
        if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
            eprintln!(
                "editur: project chooser window created in {:.2?}",
                window_started.elapsed()
            );
        }
        if let Err(error) = self.install_window(window, event_loop) {
            self.fail(event_loop, error);
            return;
        }
        #[cfg(target_os = "macos")]
        {
            activate_macos_application();
            if let Some(window) = &self.window {
                window.focus_window();
            }
        }
        self.redraw(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if window.id() != window_id {
            return;
        }
        let repaint = !matches!(event, WindowEvent::RedrawRequested)
            && self
                .egui
                .as_mut()
                .is_some_and(|state| state.on_window_event(window, &event).repaint);
        if repaint {
            window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::DroppedFile(path) => self.select(event_loop, path),
            WindowEvent::Resized(size) => self.resize(event_loop, size),
            WindowEvent::ScaleFactorChanged { .. } => {
                self.resize(event_loop, window.inner_size());
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self
            .repaint_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            self.repaint_at = None;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        } else if let Some(deadline) = self.repaint_at {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, (): ()) {
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
}

pub(super) fn project_chooser_window_attributes() -> Result<winit::window::WindowAttributes, String>
{
    let (pixels, width, height) = application_icon_rgba();
    let icon = WindowIcon::from_rgba(pixels.to_vec(), width, height)
        .map_err(|error| format!("cannot load application icon: {error}"))?;
    let attributes = Window::default_attributes()
        .with_title("Editur")
        .with_inner_size(LogicalSize::new(760, 520))
        .with_min_inner_size(LogicalSize::new(560, 420))
        .with_window_icon(Some(icon));
    #[cfg(target_os = "macos")]
    let attributes = attributes.with_decorations(false).with_transparent(true);
    Ok(attributes)
}

pub(super) fn project_chooser_ui(
    ui: &mut egui::Ui,
    error: Option<&str>,
) -> (bool, Option<WindowAction>) {
    let screen = ui.max_rect();
    ui.painter()
        .rect_filled(screen, 0.0, theme::surface().chrome);
    #[cfg(target_os = "macos")]
    let window_action = {
        let titlebar = screen.with_max_y((screen.top() + TITLEBAR_HEIGHT).min(screen.bottom()));
        ui.painter()
            .rect_filled(titlebar, 0.0, theme::surface().chrome);
        ui.painter().hline(
            titlebar.x_range(),
            titlebar.bottom() - 0.5,
            egui::Stroke::new(1.0, theme::border::hairline_color()),
        );
        ui.painter().text(
            titlebar.center(),
            Align2::CENTER_CENTER,
            "Choose a project",
            theme::typography::small(),
            theme::text().secondary,
        );
        let mut action = macos_titlebar_controls(ui, titlebar, "project_chooser");
        let drag = titlebar.with_min_x(titlebar.left() + 72.0);
        if action.is_none() {
            action = titlebar_drag_action(ui, drag, "project_chooser");
        }
        action
    };
    #[cfg(not(target_os = "macos"))]
    let window_action = None;
    let width = (screen.width() - 64.0).clamp(420.0, 540.0);
    let content = egui::Rect::from_center_size(
        screen.center() - egui::vec2(0.0, 12.0),
        egui::vec2(width, 330.0),
    );
    let mut browse = false;
    ui.scope_builder(UiBuilder::new().max_rect(content), |ui| {
        ui.vertical_centered(|ui| {
            let (logo, _) = ui.allocate_exact_size(egui::vec2(48.0, 48.0), Sense::hover());
            paint_editur_mark(ui.painter(), logo);
            ui.add_space(13.0);
            ui.label(
                RichText::new("EDITUR")
                    .size(theme::typography::DISPLAY_SIZE)
                    .strong()
                    .color(theme::text().primary),
            );
            ui.add_space(5.0);
            ui.label(
                RichText::new("Choose where you want to work")
                    .size(theme::typography::BODY_SIZE)
                    .color(theme::text().muted),
            );
            ui.add_space(28.0);
            let (card, response) = ui.allocate_exact_size(egui::vec2(width, 86.0), Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::labeled(
                    egui::WidgetType::Button,
                    ui.is_enabled(),
                    "Open project folder",
                )
            });
            let hovered = response.hovered();
            ui.painter().rect_filled(
                card,
                10.0,
                if hovered {
                    theme::state::hover()
                } else {
                    theme::surface().raised
                },
            );
            ui.painter().rect_stroke(
                card,
                10.0,
                egui::Stroke::new(
                    1.0,
                    if hovered {
                        theme::accent()
                    } else {
                        theme::border::hairline_color()
                    },
                ),
                egui::StrokeKind::Inside,
            );
            let folder = egui::Rect::from_center_size(
                egui::pos2(card.left() + 47.0, card.center().y + 2.0),
                egui::vec2(24.0, 18.0),
            );
            paint_project_folder(
                ui.painter(),
                folder,
                if hovered {
                    theme::accent()
                } else {
                    theme::text().secondary
                },
            );
            ui.painter().text(
                egui::pos2(card.left() + 82.0, card.center().y - 10.0),
                Align2::LEFT_CENTER,
                "Open project",
                theme::typography::title(),
                theme::text().primary,
            );
            ui.painter().text(
                egui::pos2(card.left() + 82.0, card.center().y + 13.0),
                Align2::LEFT_CENTER,
                "Select an existing folder",
                theme::typography::small(),
                theme::text().muted,
            );
            icons::paint(
                ui.painter(),
                Icon::ChevronRight,
                egui::Rect::from_center_size(
                    egui::pos2(card.right() - 28.0, card.center().y),
                    egui::Vec2::splat(icons::GRID * 0.75),
                ),
                if hovered {
                    theme::accent()
                } else {
                    theme::text().muted
                },
            );
            if response.clicked() {
                browse = true;
            }
            ui.add_space(17.0);
            ui.label(
                RichText::new("or drop a project folder anywhere in this window")
                    .size(theme::typography::MICRO_SIZE)
                    .color(theme::text_disabled()),
            );
            if let Some(error) = error {
                ui.add_space(13.0);
                ui.label(
                    RichText::new(error)
                        .size(theme::typography::SMALL_SIZE)
                        .color(theme::ink(theme::semantic().danger)),
                );
            }
        });
    });
    (browse, window_action)
}

/// The empty editor says nothing at all: only the product's bare mark, large
/// and gray, sitting in the middle of the pane like a watermark.
pub(super) fn draw_editor_empty_state(ui: &mut egui::Ui) {
    let rect = ui.max_rect();
    let side = (rect.width().min(rect.height()) * 0.26).clamp(56.0, 128.0);
    paint_editur_glyph(
        ui.painter(),
        egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(side)),
        editor_watermark_color(),
    );
}

/// Dimmer than disabled text: the watermark is texture, not content.
pub(super) fn editor_watermark_color() -> Color32 {
    theme::text().muted.gamma_multiply(0.35)
}

/// The icon's bare mark — three slanted strips, the middle one split — traced
/// from the shipped icon so the watermark keeps its exact geometry without the
/// tile behind it. Coordinates live in the mark's own 576 x 555 unit box.
pub(super) fn paint_editur_glyph(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    const BOX: egui::Vec2 = egui::vec2(576.0, 555.0);
    let scale = (rect.width() / BOX.x).min(rect.height() / BOX.y);
    let origin = rect.center() - BOX * scale * 0.5;
    let at = move |x: f32, y: f32| origin + egui::vec2(x, y) * scale;
    // The large curved corners on the left. Flattened here because the mark is
    // painted as convex polygons; endpoints belong to the caller's outline.
    let curve = |points: &mut Vec<egui::Pos2>,
                 from: (f32, f32),
                 control_a: (f32, f32),
                 control_b: (f32, f32),
                 to: (f32, f32)| {
        const SEGMENTS: usize = 16;
        for step in 1..=SEGMENTS {
            let t = step as f32 / SEGMENTS as f32;
            let rest = 1.0 - t;
            let blend = |a: f32, b: f32, c: f32, d: f32| {
                rest * rest * rest * a
                    + 3.0 * rest * rest * t * b
                    + 3.0 * rest * t * t * c
                    + t * t * t * d
            };
            points.push(at(
                blend(from.0, control_a.0, control_b.0, to.0),
                blend(from.1, control_a.1, control_b.1, to.1),
            ));
        }
    };
    let mut top = vec![
        at(124.0, 0.0),
        at(576.0, 0.0),
        at(495.0, 144.0),
        at(0.0, 144.0),
    ];
    curve(
        &mut top,
        (0.0, 144.0),
        (0.0, 49.0),
        (89.0, 0.0),
        (124.0, 0.0),
    );
    top.pop();
    let middle_left = vec![
        at(0.0, 205.0),
        at(235.0, 205.0),
        at(235.0, 349.0),
        at(0.0, 349.0),
    ];
    let middle_right = vec![
        at(282.0, 205.0),
        at(576.0, 205.0),
        at(495.0, 349.0),
        at(282.0, 349.0),
    ];
    let mut bottom = vec![
        at(0.0, 411.0),
        at(576.0, 411.0),
        at(495.0, 555.0),
        at(124.0, 555.0),
    ];
    curve(
        &mut bottom,
        (124.0, 555.0),
        (89.0, 555.0),
        (0.0, 506.0),
        (0.0, 411.0),
    );
    bottom.pop();
    for strip in [top, middle_left, middle_right, bottom] {
        painter.add(egui::Shape::convex_polygon(
            strip,
            color,
            egui::Stroke::NONE,
        ));
    }
}

/// The product mark is the shipped icon rather than a redrawn approximation,
/// so the chooser, the empty editor, and the dock all show the same logo.
pub(super) fn paint_editur_mark(painter: &egui::Painter, rect: egui::Rect) {
    let ctx = painter.ctx();
    let cache_id = Id::new("editur_mark_texture");
    let texture = ctx.data_mut(|data| data.get_temp::<egui::TextureHandle>(cache_id));
    let texture = texture.or_else(|| {
        let bytes = &include_bytes!("../../assets/icons/editur-128.png")[..];
        let pixels = image::load_from_memory(bytes).ok()?.into_rgba8();
        let size = [pixels.width() as usize, pixels.height() as usize];
        let texture = ctx.load_texture(
            "Editur logo",
            egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_raw()),
            egui::TextureOptions::LINEAR,
        );
        ctx.data_mut(|data| data.insert_temp(cache_id, texture.clone()));
        Some(texture)
    });
    if let Some(texture) = texture {
        let side = rect.width().min(rect.height());
        let rect = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
        painter.image(
            texture.id(),
            rect,
            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
            Color32::WHITE,
        );
    }
}

pub(super) fn paint_project_folder(painter: &egui::Painter, rect: egui::Rect, color: Color32) {
    icons::paint(painter, Icon::Folder, rect, color);
}

#[doc(hidden)]
pub fn quit_running() -> Result<(), String> {
    if crate::instance::quit_running()? {
        Ok(())
    } else {
        Err("save or discard changes in the running editor before restarting".into())
    }
}

pub(super) const fn launch_in_current_process(
    stdin_terminal: bool,
    stdout_terminal: bool,
    stderr_terminal: bool,
    macos_bundle_launch: bool,
) -> bool {
    !(macos_bundle_launch || stdin_terminal || stdout_terminal || stderr_terminal)
}

#[cfg(unix)]
pub(super) fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
pub(super) fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

pub fn run(target: OpenTarget, started: Instant) -> Result<(), String> {
    let listener = match claim(&target)? {
        Claim::Primary(listener) => listener,
        Claim::Forwarded => return Ok(()),
    };
    let mut event_loop = EventLoop::<InstanceEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    winit::platform::macos::EventLoopBuilderExtMacOS::with_default_menu(&mut event_loop, false);
    let event_loop = event_loop
        .build()
        .map_err(|error| format!("cannot create event loop: {error}"))?;
    let event_proxy = event_loop.create_proxy();
    spawn_listener(listener, event_proxy.clone())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let editor_started = Instant::now();
    let editor = EditorApp::new(target)?;
    if let Ok(directory) = data_dir() {
        let _ = crate::projects::remember(&directory, &editor.tree.root);
    }
    if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
        eprintln!(
            "editur: editor state initialized in {:.2?}",
            editor_started.elapsed()
        );
    }
    let mut shell = Shell::new(editor, started, event_proxy);
    event_loop
        .run_app(&mut shell)
        .map_err(|error| format!("window event loop failed: {error}"))?;
    shell.fatal.map_or(Ok(()), Err)
}

struct Shell {
    editor: EditorApp,
    window: Option<Window>,
    renderer: Option<Renderer>,
    egui: Option<egui_winit::State>,
    repaint_at: Option<Instant>,
    resize_at: Option<Instant>,
    pending_maximize: bool,
    pending_resize: Option<winit::dpi::PhysicalSize<u32>>,
    fatal: Option<String>,
    clipboard: Option<arboard::Clipboard>,
    modifiers: ModifiersState,
    started: Instant,
    first_frame_logged: bool,
    event_proxy: EventLoopProxy<InstanceEvent>,
}

impl Shell {
    fn new(
        editor: EditorApp,
        started: Instant,
        event_proxy: EventLoopProxy<InstanceEvent>,
    ) -> Self {
        Self {
            editor,
            window: None,
            renderer: None,
            egui: None,
            repaint_at: None,
            resize_at: None,
            pending_maximize: false,
            pending_resize: None,
            fatal: None,
            clipboard: None,
            modifiers: ModifiersState::default(),
            started,
            first_frame_logged: false,
            event_proxy,
        }
    }

    fn fail(&mut self, event_loop: &ActiveEventLoop, error: String) {
        self.fatal = Some(error);
        event_loop.exit();
    }

    fn redraw(&mut self, event_loop: &ActiveEventLoop) {
        if let Some(deadline) = self.resize_at {
            if Instant::now() < deadline {
                event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
                return;
            }
            self.resize_at = None;
        }
        let (Some(window), Some(renderer), Some(state)) = (
            self.window.as_ref(),
            self.renderer.as_mut(),
            self.egui.as_mut(),
        ) else {
            return;
        };
        if let Some(size) = self.pending_resize.take() {
            #[cfg(target_os = "macos")]
            if let Err(error) = renderer.resize(window, size) {
                self.fail(event_loop, error);
                return;
            }
            #[cfg(not(target_os = "macos"))]
            renderer.resize(size);
        }
        let input = state.take_egui_input(window);
        let context = state.egui_ctx().clone();
        let output = context.run_ui(input, |root| self.editor.ui(root));
        if let Some(request) = self.editor.take_clipboard_request() {
            match system_clipboard(&mut self.clipboard)
                .and_then(|clipboard| clipboard.get_text().map_err(|error| error.to_string()))
            {
                Ok(text) => {
                    self.editor.receive_clipboard(request, &text, &context);
                    window.request_redraw();
                }
                Err(error) => self
                    .editor
                    .show_error(format!("cannot paste from system clipboard: {error}")),
            }
        }
        let mut maximize_requested = false;
        if let Some(action) = self.editor.take_window_action() {
            match action {
                WindowAction::Close => self.editor.request_close(),
                WindowAction::Minimize => window.set_minimized(true),
                WindowAction::ToggleMaximize => {
                    self.pending_maximize = true;
                    maximize_requested = true;
                }
                WindowAction::Drag => {
                    if let Err(error) = window.drag_window() {
                        self.editor
                            .show_error(format!("cannot drag window: {error}"));
                    }
                }
            }
        }
        for command in &output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command
                && let Err(error) = system_clipboard(&mut self.clipboard).and_then(|clipboard| {
                    clipboard.set_text(text).map_err(|error| error.to_string())
                })
            {
                self.editor
                    .show_error(format!("cannot copy to system clipboard: {error}"));
            }
        }
        state.handle_platform_output_with_event_loop(window, event_loop, output.platform_output);
        let textures_updated = !output.textures_delta.set.is_empty();
        if skip_transition_render(maximize_requested, !output.textures_delta.is_empty()) {
            window.request_redraw();
            return;
        }
        let primitives = context.tessellate(output.shapes, output.pixels_per_point);
        #[cfg(target_os = "linux")]
        window.pre_present_notify();
        #[cfg(target_os = "macos")]
        let rendered =
            renderer.render(output.pixels_per_point, &primitives, &output.textures_delta);
        #[cfg(not(target_os = "macos"))]
        let rendered = renderer
            .render(output.pixels_per_point, &primitives, &output.textures_delta)
            .map(|()| true);
        let rendered = match rendered {
            Ok(rendered) => rendered,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        if !rendered {
            window.request_redraw();
            return;
        }
        if !self.first_frame_logged {
            window.focus_window();
            if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
                eprintln!(
                    "editur: first editable frame in {:.2?}",
                    self.started.elapsed()
                );
            }
            self.first_frame_logged = true;
        }
        if self.editor.should_close {
            event_loop.exit();
            return;
        }
        let delay = output
            .viewport_output
            .get(&ViewportId::ROOT)
            .map_or(Duration::MAX, |output| output.repaint_delay);
        let delay = repaint_delay_after_texture_update(delay, textures_updated);
        let now = Instant::now();
        if let Some(repaint_at) = repaint_deadline(delay, now) {
            self.repaint_at = Some(repaint_at);
            if delay.is_zero() {
                event_loop.set_control_flow(ControlFlow::Poll);
            } else {
                event_loop.set_control_flow(ControlFlow::WaitUntil(repaint_at));
            }
        } else {
            self.repaint_at = None;
            event_loop.set_control_flow(ControlFlow::Wait);
        }
    }
}

impl ApplicationHandler<InstanceEvent> for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let (pixels, width, height) = application_icon_rgba();
        let icon = match WindowIcon::from_rgba(pixels.to_vec(), width, height) {
            Ok(icon) => icon,
            Err(error) => {
                self.fail(event_loop, format!("cannot load application icon: {error}"));
                return;
            }
        };
        let attributes = Window::default_attributes()
            .with_title("Editur")
            .with_inner_size(LogicalSize::new(1180, 760))
            .with_min_inner_size(LogicalSize::new(520, 320))
            .with_window_icon(Some(icon))
            .with_decorations(false);
        let geometry = data_dir()
            .ok()
            .and_then(|directory| load_window_geometry(&directory.join(WINDOW_GEOMETRY_FILE)));
        let restored = geometry.is_some();
        let display = opening_display(
            event_loop,
            geometry
                .and_then(|geometry| geometry.position)
                .map(|(x, y)| winit::dpi::PhysicalPosition::new(x, y).into()),
        );
        let attributes = match geometry {
            Some(geometry) => geometry.apply(
                attributes,
                display.map_or(1.0, |display| display.scale_factor),
            ),
            None => attributes,
        };
        let attributes = fit_startup_window_attributes(attributes, display, restored);
        #[cfg(target_os = "macos")]
        let attributes = attributes.with_transparent(true);
        let window_started = Instant::now();
        #[cfg(target_os = "macos")]
        let window = create_macos_window_without_native_title(
            |attributes| event_loop.create_window(attributes),
            attributes,
        );
        #[cfg(not(target_os = "macos"))]
        let window = event_loop.create_window(attributes);
        let window = match window {
            Ok(window) => window,
            Err(error) => {
                self.fail(event_loop, format!("cannot create window: {error}"));
                return;
            }
        };
        let window_time = window_started.elapsed();
        let renderer_started = Instant::now();
        let renderer = match Renderer::new(&window) {
            Ok(renderer) => renderer,
            Err(error) => {
                self.fail(event_loop, error);
                return;
            }
        };
        let renderer_time = renderer_started.elapsed();
        if std::env::var("EDITUR_LOG").as_deref() == Ok("debug") {
            eprintln!(
                "editur: {} adapter: {}",
                renderer.backend_name(),
                renderer.adapter_name()
            );
            eprintln!("editur: window created in {window_time:.2?}");
            eprintln!("editur: renderer initialized in {renderer_time:.2?}");
        }
        let context = egui::Context::default();
        theme::apply(&context);
        disable_transient_egui_debug_overlays(&context);
        let event_proxy = self.event_proxy.clone();
        install_repaint_wake(&context, move || {
            let _ = event_proxy.send_event(InstanceEvent::Wake);
        });
        self.editor.warm_providers(&context);
        let state = egui_winit::State::new(
            context,
            ViewportId::ROOT,
            event_loop,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );
        #[cfg(target_os = "macos")]
        {
            activate_macos_application();
            window.focus_window();
        }
        self.egui = Some(state);
        self.renderer = Some(renderer);
        self.window = Some(window);
        self.redraw(event_loop);
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(window) = self.window.as_ref() else {
            return;
        };
        if window.id() != window_id {
            return;
        }
        if let WindowEvent::ModifiersChanged(modifiers) = &event {
            self.modifiers = modifiers.state();
        }
        if let WindowEvent::KeyboardInput { event: key, .. } = &event
            && key.state.is_pressed()
            && key.physical_key == PhysicalKey::Code(KeyCode::KeyV)
            && (self.modifiers.control_key() || self.modifiers.super_key())
        {
            match system_clipboard(&mut self.clipboard)
                .and_then(|clipboard| clipboard.get_text().map_err(|error| error.to_string()))
            {
                Ok(text) => {
                    if let Some(state) = &mut self.egui {
                        state.set_clipboard_text(text);
                    }
                }
                Err(error) => self
                    .editor
                    .show_error(format!("cannot paste from system clipboard: {error}")),
            }
        }
        let deferred_resize = self.resize_at.is_some()
            && matches!(
                &event,
                WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. }
            );
        let egui_repaint = !matches!(event, WindowEvent::RedrawRequested)
            && self
                .egui
                .as_mut()
                .is_some_and(|state| state.on_window_event(window, &event).repaint);
        if egui_repaint && !deferred_resize {
            window.request_redraw();
        }
        match event {
            WindowEvent::CloseRequested => {
                self.editor.request_close();
                if self.editor.should_close {
                    event_loop.exit();
                } else {
                    window.request_redraw();
                }
            }
            WindowEvent::Resized(size) => {
                if self.resize_at.is_some() {
                    defer_resize(
                        &mut self.pending_resize,
                        &mut self.resize_at,
                        size,
                        Instant::now(),
                    );
                    event_loop.set_control_flow(ControlFlow::WaitUntil(
                        self.resize_at.expect("deferred resize has a deadline"),
                    ));
                } else {
                    queue_resize(&mut self.pending_resize, size);
                    window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { .. } => {
                let size = window.inner_size();
                if self.resize_at.is_some() {
                    defer_resize(
                        &mut self.pending_resize,
                        &mut self.resize_at,
                        size,
                        Instant::now(),
                    );
                    event_loop.set_control_flow(ControlFlow::WaitUntil(
                        self.resize_at.expect("deferred resize has a deadline"),
                    ));
                } else {
                    queue_resize(&mut self.pending_resize, size);
                    window.request_redraw();
                }
            }
            WindowEvent::Focused(true) => {
                self.editor.reconcile_open_buffer();
                self.editor.schedule_git_refresh();
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => self.redraw(event_loop),
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if std::mem::take(&mut self.pending_maximize) {
            if let Some(window) = &self.window
                && let Err(error) = toggle_window_maximize(window)
            {
                self.fail(event_loop, error);
                return;
            }
            self.resize_at = Some(Instant::now() + RESIZE_SETTLE_DELAY);
        }

        let now = Instant::now();
        let resize_due = self.resize_at.is_some_and(|deadline| now >= deadline);
        let repaint_due = self.repaint_at.is_some_and(|deadline| now >= deadline);
        if resize_due {
            self.resize_at = None;
        }
        if repaint_due {
            self.repaint_at = None;
        }
        if resize_due || repaint_due {
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        } else if let Some(deadline) = [self.resize_at, self.repaint_at]
            .into_iter()
            .flatten()
            .min()
        {
            event_loop.set_control_flow(ControlFlow::WaitUntil(deadline));
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: InstanceEvent) {
        match event {
            InstanceEvent::Open(target, reply) => {
                self.editor.request_target(target);
                if let Some(window) = &self.window {
                    window.set_visible(true);
                    #[cfg(target_os = "macos")]
                    activate_macos_application();
                    window.focus_window();
                    window.request_redraw();
                }
                let _ = reply.send(true);
            }
            InstanceEvent::Quit(reply) => {
                let clean = self.editor.tabs.iter().all(|tab| !tab.buffer.dirty);
                let _ = reply.send(clean);
                if !clean {
                    self.editor
                        .show_error("Save or discard changes before updating Editur.".into());
                    if let Some(window) = &self.window {
                        window.set_visible(true);
                        window.focus_window();
                        window.request_redraw();
                    }
                }
            }
            InstanceEvent::Wake => {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            InstanceEvent::Exit => event_loop.exit(),
        }
    }

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        let Some(window) = &self.window else {
            return;
        };
        let position = window
            .outer_position()
            .ok()
            .map(|position| (position.x, position.y));
        let size = window.inner_size();
        let result = data_dir().and_then(|directory| {
            save_window_geometry(
                &directory.join(WINDOW_GEOMETRY_FILE),
                WindowGeometry {
                    position,
                    size: (size.width, size.height),
                },
            )
        });
        if let Err(error) = result
            && std::env::var("EDITUR_LOG").as_deref() == Ok("debug")
        {
            eprintln!("editur: {error}");
        }
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub(super) fn create_macos_window_without_native_title(
    create_window: impl FnOnce(winit::window::WindowAttributes) -> Result<Window, winit::error::OsError>,
    attributes: winit::window::WindowAttributes,
) -> Result<Window, winit::error::OsError> {
    use objc::{
        class,
        runtime::{self, Imp, Method, Object, Sel},
        sel, sel_impl,
    };

    unsafe extern "C" fn ignore_title(_: *mut Object, _: Sel, _: *mut Object) {}

    struct RestoreMethod {
        method: *mut Method,
        implementation: Imp,
    }

    impl Drop for RestoreMethod {
        fn drop(&mut self) {
            unsafe {
                runtime::method_setImplementation(self.method, self.implementation);
            }
        }
    }

    unsafe {
        // macOS 26 blocks for roughly two seconds when winit sets a title on a borderless window.
        // Editur draws its own titlebar, so suppress only that synchronous creation-time call.
        let method = runtime::class_getInstanceMethod(class!(NSWindow), sel!(setTitle:));
        if method.is_null() {
            return create_window(attributes);
        }
        let replacement: Imp = std::mem::transmute(
            ignore_title as unsafe extern "C" fn(*mut Object, Sel, *mut Object),
        );
        let restore = RestoreMethod {
            method: method.cast_mut(),
            implementation: runtime::method_setImplementation(method.cast_mut(), replacement),
        };
        let window = create_window(attributes);
        drop(restore);
        window
    }
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub(super) fn toggle_window_maximize(window: &Window) -> Result<(), String> {
    use objc::{msg_send, runtime::Object, sel, sel_impl};
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    let handle = window
        .window_handle()
        .map_err(|error| format!("cannot obtain the AppKit window handle: {error}"))?;
    let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
        return Err("winit did not provide an AppKit window handle".to_owned());
    };
    let view = handle.ns_view.as_ptr().cast::<Object>();
    unsafe {
        let native_window: *mut Object = msg_send![view, window];
        if native_window.is_null() {
            return Err("cannot obtain the AppKit window".to_owned());
        }
        let _: () = msg_send![native_window, zoom: std::ptr::null_mut::<Object>()];
    }
    Ok(())
}

#[cfg(not(target_os = "macos"))]
pub(super) fn toggle_window_maximize(window: &Window) -> Result<(), String> {
    window.set_maximized(!window.is_maximized());
    Ok(())
}

#[cfg(target_os = "macos")]
#[allow(unexpected_cfgs)]
pub(super) fn activate_macos_application() {
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};

    unsafe {
        let application: *mut Object = msg_send![class!(NSApplication), sharedApplication];
        let _: objc::runtime::BOOL = msg_send![application, setActivationPolicy: 0_isize];
        let modern: objc::runtime::BOOL =
            msg_send![application, respondsToSelector: sel!(activate)];
        if modern == objc::runtime::YES {
            let _: () = msg_send![application, activate];
        } else {
            let _: () = msg_send![application, activateIgnoringOtherApps: objc::runtime::YES];
        }
    }
}
