//! Pen tilt, rotation and pen-ness on Windows (task 171).
//!
//! winit 0.30 reports a Windows pen as `WindowEvent::Touch` with pressure
//! only: it calls `GetPointerPenInfo` but keeps just the pressure, and a pen
//! cannot be told from a finger. [`PenTracker`] subclasses the window
//! (`SetWindowSubclass`), watches `WM_POINTERDOWN/UPDATE/UP`, records the pen
//! info per pointer id and forwards every message untouched. The caller looks
//! the info up by the `Touch` id winit reports (winit uses the pointer id).
//!
//! Off Windows [`PenTracker::install`] returns [`InstallError::Unsupported`].
//! The numeric conversion and the bounded map ([`raw`]) are portable.
//!
//! Threading: a tracker must be created, used and dropped on the thread that
//! owns the window, and dropped before the window is destroyed.

pub mod raw;

pub use raw::{PenMap, RawPen};

#[cfg(windows)]
mod win;
#[cfg(windows)]
pub use win::PenTracker;

#[cfg(not(windows))]
mod stub;
#[cfg(not(windows))]
pub use stub::PenTracker;

/// Why a tracker could not be installed.
#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    /// Not a Windows build.
    #[error("pen tracking is only available on Windows")]
    Unsupported,
    /// The window handle was null.
    #[error("null window handle")]
    NullWindow,
    /// `SetWindowSubclass` failed (wrong thread or a bad window).
    #[error("SetWindowSubclass failed")]
    SubclassFailed,
}
