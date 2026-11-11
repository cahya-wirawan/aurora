//! The Windows reader. All `unsafe` is here.

use crate::InstallError;
use crate::raw::{PenMap, RawPen};
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows_sys::Win32::UI::Input::Pointer::{GetPointerPenInfo, GetPointerType, POINTER_PEN_INFO};
use windows_sys::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    PT_PEN, WM_POINTERDOWN, WM_POINTERUP, WM_POINTERUPDATE,
};

/// Subclass id; unique to this crate within the window.
const SUBCLASS_ID: usize = 0x4155_5231; // "AUR1"

#[derive(Debug, Default)]
struct Shared {
    map: RefCell<PenMap>,
}

/// Records pen info for the pointers of one window. See the crate docs.
#[derive(Debug)]
pub struct PenTracker {
    hwnd: HWND,
    shared: Rc<Shared>,
    /// Tied to the window's thread: not `Send`, not `Sync`.
    _not_send: PhantomData<Rc<()>>,
}

impl PenTracker {
    /// Subclasses the window. Call on the window's thread.
    ///
    /// # Errors
    /// [`InstallError::NullWindow`] or [`InstallError::SubclassFailed`].
    pub fn install(hwnd: isize) -> Result<Self, InstallError> {
        let hwnd = hwnd as HWND;
        if hwnd.is_null() {
            return Err(InstallError::NullWindow);
        }
        let shared = Rc::new(Shared::default());
        let data = Rc::as_ptr(&shared) as usize;
        // SAFETY: `hwnd` is non-null; the subclass proc has the required
        // signature; `data` points at the `Shared` inside `shared`, which
        // this tracker keeps alive until `Drop` has removed the subclass, so
        // the proc never sees a dangling pointer. SetWindowSubclass fails
        // (returns 0) if called from another thread than the window's.
        let ok = unsafe { SetWindowSubclass(hwnd, Some(subclass_proc), SUBCLASS_ID, data) };
        if ok == 0 {
            return Err(InstallError::SubclassFailed);
        }
        Ok(Self {
            hwnd,
            shared,
            _not_send: PhantomData,
        })
    }

    /// The latest pen info for a winit `Touch` id, if that contact is a pen.
    #[must_use]
    pub fn pen_info(&self, pointer_id: u64) -> Option<RawPen> {
        self.shared.map.try_borrow().ok()?.get(pointer_id)
    }

    /// Forgets a contact (call on its release or cancel).
    pub fn forget(&self, pointer_id: u64) {
        if let Ok(mut map) = self.shared.map.try_borrow_mut() {
            map.forget(pointer_id);
        }
    }
}

impl Drop for PenTracker {
    fn drop(&mut self) {
        // SAFETY: removes the subclass this tracker installed with the same
        // (proc, id) pair, on the window's thread; if the window is already
        // gone the call just fails, which is ignored. After it returns the
        // proc can no longer run, so `shared` may be freed.
        unsafe {
            RemoveWindowSubclass(self.hwnd, Some(subclass_proc), SUBCLASS_ID);
        }
    }
}

/// Records pen info for a pointer message, without touching the message.
fn observe(shared: &Shared, msg: u32, wparam: WPARAM) {
    let id = (wparam & 0xFFFF) as u32;
    let mut kind = 0i32;
    // SAFETY: `kind` is a valid out pointer for the call's duration.
    let typed = unsafe { GetPointerType(id, &raw mut kind) };
    let is_pen = typed != 0 && kind == PT_PEN;
    let Ok(mut map) = shared.map.try_borrow_mut() else {
        return;
    };
    if !is_pen {
        // A finger or mouse reusing an id must not inherit a stale pen.
        if msg == WM_POINTERDOWN {
            map.forget(u64::from(id));
        }
        return;
    }
    let mut info = POINTER_PEN_INFO::default();
    // SAFETY: `info` is a valid, writable POINTER_PEN_INFO.
    if unsafe { GetPointerPenInfo(id, &raw mut info) } == 0 {
        return;
    }
    map.insert(
        u64::from(id),
        RawPen::from_raw(
            info.penMask,
            info.penFlags,
            info.pressure,
            info.rotation,
            info.tiltX,
            info.tiltY,
        ),
    );
}

/// The subclass procedure. Never panics, never consumes a message: every
/// message goes on to `DefSubclassProc` (the next procedure, winit's).
unsafe extern "system" fn subclass_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    data: usize,
) -> LRESULT {
    if matches!(msg, WM_POINTERDOWN | WM_POINTERUPDATE | WM_POINTERUP) {
        // SAFETY: `data` is the address of the `Shared` held by the live
        // tracker (see `install`); the proc runs on the window thread, the
        // only thread that touches it, and only a shared reference is made.
        if let Some(shared) = unsafe { (data as *const Shared).as_ref() } {
            observe(shared, msg, wparam);
        }
    }
    // SAFETY: forwards the original arguments unchanged.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}
