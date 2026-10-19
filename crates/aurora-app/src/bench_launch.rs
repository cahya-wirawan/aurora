//! Environment-gated launch options for outside-in measurement.
//!
//! Stage 2 of `spike/photocraft-compare` drives the live window with
//! synthetic input and times its presented frames from outside the
//! process. Two things it needs have no user-facing route that a script can
//! reach: opening a file without the native file dialog, and choosing the
//! swapchain's present mode (the default, `AutoVsync`, caps every frame
//! rate at the display's refresh, so two apps would both read 60 FPS).
//!
//! * `AURORA_OPEN` ([`OPEN_ENV`]): a path opened once, after the window is
//!   created, through the same `App::open_file` the File > Open command and
//!   a dropped file use.
//! * `AURORA_PRESENT_MODE` ([`PRESENT_MODE_ENV`]): `fifo` (or `vsync`),
//!   `autovsync`, or `novsync` (also `immediate`, `mailbox`,
//!   `autonovsync`), which is wgpu's `AutoNoVsync`: Immediate, else
//!   Mailbox, else Fifo, so it never asks for a mode the surface lacks.
//!   Any other value is ignored with a warning.
//!
//! Unset or empty, neither changes anything: the app behaves exactly as
//! it does without them. Neither is a setting: nothing is persisted.

use std::path::PathBuf;

/// The variable naming a file to open at startup.
pub(crate) const OPEN_ENV: &str = "AURORA_OPEN";

/// The variable choosing the swapchain present mode.
pub(crate) const PRESENT_MODE_ENV: &str = "AURORA_PRESENT_MODE";

/// The present mode `value` names, or `None` for an empty or unknown one
/// (trimmed, case-insensitive).
pub(crate) fn parse_present_mode(value: &str) -> Option<wgpu::PresentMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "fifo" | "vsync" => Some(wgpu::PresentMode::Fifo),
        "autovsync" => Some(wgpu::PresentMode::AutoVsync),
        "novsync" | "immediate" | "mailbox" | "autonovsync" => Some(wgpu::PresentMode::AutoNoVsync),
        _ => None,
    }
}

/// The present mode `AURORA_PRESENT_MODE` asks for, if any. An unknown
/// value is logged and ignored.
pub(crate) fn present_mode_from_env() -> Option<wgpu::PresentMode> {
    let value = std::env::var(PRESENT_MODE_ENV).ok()?;
    if value.trim().is_empty() {
        return None;
    }
    let mode = parse_present_mode(&value);
    if mode.is_none() {
        tracing::warn!(
            value,
            "{PRESENT_MODE_ENV} is not fifo, autovsync or novsync; ignored"
        );
    }
    mode
}

/// Reconfigures `surface` with the present mode `AURORA_PRESENT_MODE`
/// asks for; unset, empty or unknown, it leaves the surface untouched.
pub(crate) fn apply_present_mode_from_env(
    surface: &mut aurora_gpu::GpuSurface<'_>,
    device: &wgpu::Device,
) {
    if let Some(mode) = present_mode_from_env() {
        surface.set_present_mode(device, mode);
        tracing::info!(?mode, "present mode set by {PRESENT_MODE_ENV}");
    }
}

/// The path `value` names, or `None` when it is empty or only whitespace.
pub(crate) fn parse_open_path(value: &std::ffi::OsStr) -> Option<PathBuf> {
    let text = value.to_string_lossy();
    if text.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

/// The file `AURORA_OPEN` names, if any.
pub(crate) fn open_path_from_env() -> Option<PathBuf> {
    parse_open_path(&std::env::var_os(OPEN_ENV)?)
}

#[cfg(test)]
mod tests {
    use super::{parse_open_path, parse_present_mode};
    use std::ffi::OsStr;
    use std::path::PathBuf;

    #[test]
    fn present_mode_spellings_map_to_the_three_safe_modes() {
        for (value, mode) in [
            ("fifo", wgpu::PresentMode::Fifo),
            ("VSync", wgpu::PresentMode::Fifo),
            (" autovsync ", wgpu::PresentMode::AutoVsync),
            ("novsync", wgpu::PresentMode::AutoNoVsync),
            ("immediate", wgpu::PresentMode::AutoNoVsync),
            ("mailbox", wgpu::PresentMode::AutoNoVsync),
            ("AutoNoVsync", wgpu::PresentMode::AutoNoVsync),
        ] {
            assert_eq!(parse_present_mode(value), Some(mode), "{value:?}");
        }
    }

    #[test]
    fn unknown_or_empty_present_modes_change_nothing() {
        for value in ["", "  ", "fast", "fiforelaxed", "0"] {
            assert_eq!(parse_present_mode(value), None, "{value:?}");
        }
    }

    #[test]
    fn open_path_is_kept_verbatim_unless_blank() {
        assert_eq!(parse_open_path(OsStr::new("")), None);
        assert_eq!(parse_open_path(OsStr::new("  ")), None);
        assert_eq!(
            parse_open_path(OsStr::new("/data/a b.psd")),
            Some(PathBuf::from("/data/a b.psd"))
        );
    }
}
