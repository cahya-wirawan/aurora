use crate::error::TestkitError;

/// An in-memory `Rgba8` image — the format every golden/actual/diff PNG
/// this crate reads or writes uses, regardless of what precision the
/// pipeline that produced `rgba` actually renders in. Converting from
/// e.g. `aurora-tile`'s `f16` tiles to `Rgba8` (promotion/quantization)
/// is the caller's own job — this crate stays deliberately dependency-free
/// of the rest of the workspace (see this crate's own `lib.rs` doc
/// comment), so it has no way to do that conversion itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// `width * height * 4` bytes, row-major, no padding.
    pub rgba: Vec<u8>,
}

impl Image {
    /// # Errors
    ///
    /// Returns [`TestkitError::WrongBufferLength`] if `rgba.len()` isn't
    /// `width * height * 4`.
    pub fn new(width: u32, height: u32, rgba: Vec<u8>) -> Result<Self, TestkitError> {
        let expected_len = (width as usize) * (height as usize) * 4;
        if rgba.len() != expected_len {
            return Err(TestkitError::WrongBufferLength {
                width,
                height,
                expected_len,
                actual_len: rgba.len(),
            });
        }
        Ok(Self {
            width,
            height,
            rgba,
        })
    }
}
