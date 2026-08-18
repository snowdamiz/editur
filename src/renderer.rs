use egui::{PaintCallback, Painter, Rect, TexturesDelta, epaint::Primitive};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RetainedPaint {
    pub key: u64,
    pub revision: u64,
}

struct RetainedPaintEnd;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RetainedUpload {
    pub revision: u64,
    pub vertex_offset: usize,
    pub vertex_bytes: usize,
    pub index_offset: usize,
    pub index_bytes: usize,
}

pub(crate) fn mark_retained(painter: &Painter, rect: Rect, key: u64, revision: u64) {
    let revision =
        revision ^ crate::theme::paint_appearance(painter.pixels_per_point()).rotate_left(23);
    painter.add(PaintCallback {
        rect,
        callback: Arc::new(RetainedPaint { key, revision }),
    });
}

pub(crate) fn end_retained(painter: &Painter, rect: Rect) {
    painter.add(PaintCallback {
        rect,
        callback: Arc::new(RetainedPaintEnd),
    });
}

pub(crate) fn retained_paint(primitive: &Primitive) -> Result<Option<RetainedPaint>, String> {
    let Primitive::Callback(callback) = primitive else {
        return Ok(None);
    };
    if let Some(paint) = callback.callback.downcast_ref::<RetainedPaint>() {
        Ok(Some(*paint))
    } else if callback
        .callback
        .downcast_ref::<RetainedPaintEnd>()
        .is_some()
    {
        Ok(None)
    } else {
        Err("unsupported egui paint callback".to_owned())
    }
}

fn retained_for_mesh(
    retained: &mut Option<(RetainedPaint, Rect)>,
    mesh_clip_rect: Rect,
) -> Option<RetainedPaint> {
    retained
        .take()
        .filter(|(_, marker_clip_rect)| *marker_clip_rect == mesh_clip_rect)
        .map(|(paint, _)| paint)
}

pub(crate) fn upload_required(current: Option<&RetainedUpload>, next: RetainedUpload) -> bool {
    current != Some(&next)
}

pub(crate) fn retain_active_uploads(
    retained: &mut HashMap<u64, RetainedUpload>,
    active: &HashSet<u64>,
) {
    retained.retain(|key, _| active.contains(key));
}

pub(crate) fn invalidate_retained_uploads_on_texture_replace(
    retained: &mut HashMap<u64, RetainedUpload>,
    textures: &TexturesDelta,
) {
    if textures.set.iter().any(|(_, delta)| delta.pos.is_none()) {
        retained.clear();
    }
}

fn choose_adapter(adapters: &[(&str, bool, bool)], requested: Option<&str>) -> Option<usize> {
    if let Some(requested) = requested {
        let requested = requested.to_ascii_lowercase();
        return adapters.iter().position(|(name, _, headless)| {
            !headless && name.to_ascii_lowercase().contains(&requested)
        });
    }

    adapters
        .iter()
        .position(|(_, low_power, headless)| *low_power && !headless)
        .or_else(|| adapters.iter().position(|(_, _, headless)| !headless))
}

fn buffer_capacity(current: usize, needed: usize) -> usize {
    if needed <= current {
        current
    } else {
        needed.checked_next_power_of_two().unwrap_or(needed)
    }
}

#[cfg(target_os = "macos")]
mod metal;
#[cfg(target_os = "macos")]
pub use metal::Renderer;

#[cfg(target_os = "linux")]
mod vulkan;
#[cfg(target_os = "linux")]
pub use vulkan::Renderer;

#[cfg(target_os = "windows")]
mod d3d12;
#[cfg(target_os = "windows")]
pub use d3d12::Renderer;

#[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
compile_error!("editur supports macOS, Windows, and Linux");

#[cfg(test)]
mod tests {
    use super::{
        RetainedUpload, buffer_capacity, choose_adapter,
        invalidate_retained_uploads_on_texture_replace, retain_active_uploads, retained_for_mesh,
        upload_required,
    };
    use egui::{
        Color32, ColorImage, RawInput, Rect, TextureId, TextureOptions, TexturesDelta, Vec2,
        epaint::ImageDelta, pos2,
    };
    use std::collections::{HashMap, HashSet};

    #[test]
    fn upload_buffers_grow_geometrically_and_never_shrink() {
        assert_eq!(buffer_capacity(1024, 1025), 2048);
        assert_eq!(buffer_capacity(2048, 64), 2048);
    }

    #[test]
    fn explicit_adapter_wins_then_low_power_is_preferred() {
        let adapters = [
            ("Discrete GPU", false, false),
            ("Integrated GPU", true, false),
            ("Headless GPU", true, true),
        ];

        assert_eq!(choose_adapter(&adapters, Some("discrete")), Some(0));
        assert_eq!(choose_adapter(&adapters, None), Some(1));
        assert_eq!(choose_adapter(&[("Headless", true, true)], None), None);
    }

    #[test]
    fn retained_uploads_change_only_when_geometry_or_offsets_change() {
        let upload = RetainedUpload {
            revision: 7,
            vertex_offset: 16,
            vertex_bytes: 32,
            index_offset: 8,
            index_bytes: 12,
        };

        assert!(upload_required(None, upload));
        assert!(!upload_required(Some(&upload), upload));
        assert!(upload_required(
            Some(&upload),
            RetainedUpload {
                revision: 8,
                ..upload
            }
        ));
    }

    #[test]
    fn retained_paint_changes_revision_with_the_theme() {
        let _flag = crate::theme::PALETTE_TEST_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        crate::theme::set_light(false);
        let context = crate::theme::test_context();
        let draw = || {
            let output = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(100.0))),
                    ..RawInput::default()
                },
                |ui| {
                    super::mark_retained(ui.painter(), ui.max_rect(), 1, 7);
                },
            );
            context
                .tessellate(output.shapes, output.pixels_per_point)
                .iter()
                .find_map(|primitive| super::retained_paint(&primitive.primitive).unwrap())
                .unwrap()
                .revision
        };

        let dark = draw();
        crate::theme::set_light(true);
        crate::theme::apply(&context);
        let light = draw();
        crate::theme::set_light(false);
        crate::theme::apply(&context);

        assert_ne!(dark, light);
    }

    #[test]
    fn retained_paint_changes_revision_with_pixels_per_point() {
        let context = crate::theme::test_context();
        let draw = || {
            let output = context.run_ui(
                RawInput {
                    screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), Vec2::splat(100.0))),
                    ..RawInput::default()
                },
                |ui| super::mark_retained(ui.painter(), ui.max_rect(), 1, 7),
            );
            context
                .tessellate(output.shapes, output.pixels_per_point)
                .iter()
                .find_map(|primitive| super::retained_paint(&primitive.primitive).unwrap())
                .unwrap()
                .revision
        };

        let regular = draw();
        context.set_pixels_per_point(1.5);
        let scaled = draw();

        assert_ne!(regular, scaled);
    }

    #[test]
    fn retained_marker_does_not_cross_into_an_adjacent_panel() {
        let marker_clip = Rect::from_min_max(pos2(0.0, 0.0), pos2(240.0, 700.0));
        let mesh_clip = Rect::from_min_max(pos2(240.0, 0.0), pos2(1_000.0, 700.0));
        let mut retained = Some((
            super::RetainedPaint {
                key: 1,
                revision: 1,
            },
            marker_clip,
        ));

        assert_eq!(retained_for_mesh(&mut retained, mesh_clip), None);
    }

    #[test]
    fn overwritten_retained_ranges_are_not_reused_when_a_line_returns() {
        let upload = RetainedUpload {
            revision: 7,
            vertex_offset: 16,
            vertex_bytes: 32,
            index_offset: 8,
            index_bytes: 12,
        };
        let mut retained = HashMap::from([(1, upload), (2, upload)]);

        retain_active_uploads(&mut retained, &HashSet::from([2]));

        assert!(!retained.contains_key(&1));
        assert!(upload_required(retained.get(&1), upload));
    }

    #[test]
    fn replacing_a_texture_invalidates_retained_meshes() {
        let upload = RetainedUpload {
            revision: 7,
            vertex_offset: 16,
            vertex_bytes: 32,
            index_offset: 8,
            index_bytes: 12,
        };
        let mut retained = HashMap::from([(1, upload)]);
        let textures = TexturesDelta {
            set: vec![(
                TextureId::default(),
                ImageDelta::full(
                    ColorImage::filled([2, 2], Color32::WHITE),
                    TextureOptions::LINEAR,
                ),
            )],
            free: Vec::new(),
        };

        invalidate_retained_uploads_on_texture_replace(&mut retained, &textures);

        assert!(retained.is_empty());
    }
}
