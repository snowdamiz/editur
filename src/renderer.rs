use egui::{PaintCallback, Painter, Rect, epaint::Primitive};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RetainedPaint {
    pub key: u64,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct RetainedUpload {
    pub revision: u64,
    pub vertex_offset: usize,
    pub vertex_bytes: usize,
    pub index_offset: usize,
    pub index_bytes: usize,
}

pub(crate) fn mark_retained(painter: &Painter, rect: Rect, key: u64, revision: u64) {
    painter.add(PaintCallback {
        rect,
        callback: Arc::new(RetainedPaint { key, revision }),
    });
}

pub(crate) fn retained_paint(primitive: &Primitive) -> Result<Option<RetainedPaint>, String> {
    let Primitive::Callback(callback) = primitive else {
        return Ok(None);
    };
    callback
        .callback
        .downcast_ref::<RetainedPaint>()
        .copied()
        .map(Some)
        .ok_or_else(|| "unsupported egui paint callback".to_owned())
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
        RetainedUpload, buffer_capacity, choose_adapter, retain_active_uploads, upload_required,
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
}
