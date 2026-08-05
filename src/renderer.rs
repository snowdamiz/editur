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
    use super::{buffer_capacity, choose_adapter};

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
}
