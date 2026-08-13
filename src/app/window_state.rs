use std::{fs, path::Path, time::Duration, time::Instant};

use winit::event_loop::ActiveEventLoop;

#[cfg(target_os = "macos")]
use super::TITLEBAR_HEIGHT;

pub(super) const RESIZE_SETTLE_DELAY: Duration = Duration::from_millis(50);
pub(super) const WINDOW_GEOMETRY_FILE: &str = "window.json";
const MAX_WINDOW_GEOMETRY_BYTES: u64 = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WindowGeometry {
    pub(super) position: Option<(i32, i32)>,
    pub(super) size: (u32, u32),
}

#[derive(Clone, Copy)]
pub(super) struct DisplayBounds {
    pub(super) position: winit::dpi::PhysicalPosition<i32>,
    pub(super) size: winit::dpi::PhysicalSize<u32>,
    pub(super) scale_factor: f64,
}

pub(super) fn startup_display_bounds(display: DisplayBounds) -> DisplayBounds {
    #[cfg(target_os = "macos")]
    let display = {
        let top = ((f64::from(TITLEBAR_HEIGHT) * display.scale_factor).round() as u32)
            .min(display.size.height);
        DisplayBounds {
            position: winit::dpi::PhysicalPosition::new(
                display.position.x,
                display.position.y.saturating_add_unsigned(top),
            ),
            size: winit::dpi::PhysicalSize::new(display.size.width, display.size.height - top),
            ..display
        }
    };
    display
}

impl WindowGeometry {
    pub(super) fn apply(
        self,
        attributes: winit::window::WindowAttributes,
        scale_factor: f64,
    ) -> winit::window::WindowAttributes {
        let size =
            winit::dpi::PhysicalSize::new(self.size.0, self.size.1).to_logical::<f64>(scale_factor);
        let attributes = attributes.with_inner_size(size);
        if let Some((x, y)) = self.position {
            attributes.with_position(
                winit::dpi::PhysicalPosition::new(x, y).to_logical::<f64>(scale_factor),
            )
        } else {
            attributes
        }
    }
}

pub(super) fn fit_window_attributes_to_display(
    mut attributes: winit::window::WindowAttributes,
    display: DisplayBounds,
) -> winit::window::WindowAttributes {
    let fitted_size = attributes.inner_size.map(|size| {
        let size = size.to_physical::<u32>(display.scale_factor);
        winit::dpi::PhysicalSize::new(
            size.width.min(display.size.width),
            size.height.min(display.size.height),
        )
    });
    if let Some(size) = fitted_size {
        attributes.inner_size = Some(size.into());
        if let Some(minimum) = attributes.min_inner_size {
            let minimum = minimum.to_physical::<u32>(display.scale_factor);
            attributes.min_inner_size = Some(
                winit::dpi::PhysicalSize::new(
                    minimum.width.min(size.width),
                    minimum.height.min(size.height),
                )
                .into(),
            );
        }
        if let Some(position) = attributes.position {
            let position = position.to_physical::<i32>(display.scale_factor);
            let left = i64::from(display.position.x);
            let top = i64::from(display.position.y);
            let right = left + i64::from(display.size.width.saturating_sub(size.width));
            let bottom = top + i64::from(display.size.height.saturating_sub(size.height));
            attributes.position = Some(
                winit::dpi::PhysicalPosition::new(
                    i64::from(position.x).clamp(left, right) as i32,
                    i64::from(position.y).clamp(top, bottom) as i32,
                )
                .into(),
            );
        }
    }
    attributes
}

pub(super) fn fit_startup_window_attributes(
    attributes: winit::window::WindowAttributes,
    display: Option<DisplayBounds>,
    restored: bool,
) -> winit::window::WindowAttributes {
    if restored {
        attributes
    } else if let Some(display) = display {
        fit_window_attributes_to_display(attributes, display)
    } else {
        attributes
    }
}

pub(super) fn opening_display(
    event_loop: &ActiveEventLoop,
    requested: Option<winit::dpi::Position>,
) -> Option<DisplayBounds> {
    let bounds = |monitor: winit::monitor::MonitorHandle| DisplayBounds {
        position: monitor.position(),
        size: monitor.size(),
        scale_factor: monitor.scale_factor(),
    };
    requested
        .and_then(|position| {
            event_loop.available_monitors().find_map(|monitor| {
                let display = bounds(monitor);
                let position = position.to_physical::<i32>(display.scale_factor);
                let x = i64::from(position.x) - i64::from(display.position.x);
                let y = i64::from(position.y) - i64::from(display.position.y);
                (x >= 0
                    && y >= 0
                    && x < i64::from(display.size.width)
                    && y < i64::from(display.size.height))
                .then_some(display)
            })
        })
        .or_else(|| event_loop.primary_monitor().map(bounds))
        .or_else(|| event_loop.available_monitors().next().map(bounds))
        .map(startup_display_bounds)
}

pub(super) fn load_window_geometry(path: &Path) -> Option<WindowGeometry> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() || metadata.len() > MAX_WINDOW_GEOMETRY_BYTES {
        return None;
    }
    serde_json::from_slice(&fs::read(path).ok()?).ok()
}

pub(super) fn save_window_geometry(path: &Path, geometry: WindowGeometry) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "window geometry path has no parent".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create application data directory: {error}"))?;
    let bytes = serde_json::to_vec(&geometry)
        .map_err(|error| format!("cannot encode window geometry: {error}"))?;
    fs::write(path, bytes).map_err(|error| format!("cannot save window geometry: {error}"))
}

pub(super) fn repaint_deadline(delay: Duration, now: Instant) -> Option<Instant> {
    (delay != Duration::MAX).then(|| now + delay)
}

pub(super) fn queue_resize(
    pending: &mut Option<winit::dpi::PhysicalSize<u32>>,
    size: winit::dpi::PhysicalSize<u32>,
) {
    *pending = Some(size);
}

pub(super) fn defer_resize(
    pending: &mut Option<winit::dpi::PhysicalSize<u32>>,
    redraw_at: &mut Option<Instant>,
    size: winit::dpi::PhysicalSize<u32>,
    now: Instant,
) {
    queue_resize(pending, size);
    *redraw_at = Some(now + RESIZE_SETTLE_DELAY);
}

pub(super) fn repaint_delay_after_texture_update(
    delay: Duration,
    textures_updated: bool,
) -> Duration {
    if textures_updated {
        Duration::ZERO
    } else {
        delay
    }
}

pub(super) const fn skip_transition_render(
    maximize_requested: bool,
    textures_changed: bool,
) -> bool {
    maximize_requested && !textures_changed
}

pub(super) fn install_repaint_wake(
    context: &egui::Context,
    wake: impl Fn() + Send + Sync + 'static,
) {
    context.set_request_repaint_callback(move |info| {
        if info.delay.is_zero() {
            wake();
        }
    });
}

pub(super) fn disable_transient_egui_debug_overlays(context: &egui::Context) {
    #[cfg(debug_assertions)]
    context.all_styles_mut(|style| style.debug.warn_if_rect_changes_id = false);
    #[cfg(not(debug_assertions))]
    let _ = context;
}

pub(super) fn system_clipboard(
    clipboard: &mut Option<arboard::Clipboard>,
) -> Result<&mut arboard::Clipboard, String> {
    if clipboard.is_none() {
        *clipboard = Some(
            arboard::Clipboard::new()
                .map_err(|error| format!("system clipboard is unavailable: {error}"))?,
        );
    }
    Ok(clipboard.as_mut().expect("clipboard was initialized"))
}
