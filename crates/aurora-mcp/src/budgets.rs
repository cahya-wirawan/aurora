//! The limits that bound what one request, one reply and one file may cost.
//! Every one is an engineering default, not a design decision; the ones a
//! caller may reasonably want different are in [`Limits`].

/// Longest accepted path, in bytes.
pub const MAX_PATH_BYTES: usize = 4096;
/// Longest accepted request line (JSON-RPC message), in bytes.
pub const MAX_REQUEST_BYTES: usize = 1 << 20;
/// Longest accepted tool reply text, in bytes.
pub const MAX_RESPONSE_BYTES: usize = 4 << 20;
/// Largest PNG a preview may return, in bytes.
pub const MAX_PREVIEW_PNG_BYTES: usize = 6 << 20;

/// Limits one server runs under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest file `doc_open` reads, in bytes. A PSD of this size is
    /// copied once into the server's private scratch directory first.
    pub max_open_bytes: u64,
    /// Longest side of a preview; a request above it is refused.
    pub max_preview_side: u32,
    /// The side a preview has when none is asked for.
    pub default_preview_side: u32,
    /// Most steps in one `command_batch`.
    pub max_batch_steps: usize,
    /// Largest exported file, in bytes.
    pub max_export_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_open_bytes: 2 << 30,
            max_preview_side: 2048,
            default_preview_side: 1024,
            max_batch_steps: 32,
            max_export_bytes: 4 << 30,
        }
    }
}
