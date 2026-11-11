use crate::{InstallError, RawPen};
use std::marker::PhantomData;
use std::rc::Rc;

/// Placeholder for non-Windows builds: cannot be created.
#[derive(Debug)]
pub struct PenTracker {
    _not_send: PhantomData<Rc<()>>,
}

impl PenTracker {
    /// Always [`InstallError::Unsupported`].
    ///
    /// # Errors
    /// Always.
    pub fn install(_hwnd: isize) -> Result<Self, InstallError> {
        Err(InstallError::Unsupported)
    }

    /// Never has info.
    #[must_use]
    pub fn pen_info(&self, _pointer_id: u64) -> Option<RawPen> {
        None
    }

    /// No-op.
    pub fn forget(&self, _pointer_id: u64) {}
}
