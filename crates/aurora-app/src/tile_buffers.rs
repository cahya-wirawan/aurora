//! A small per-thread pool of tile-sized `f16` buffers for the CPU
//! composite (60 FPS round 9).
//!
//! Compositing one tile of a two-layer document allocates about three
//! 512 KiB buffers (the accumulator and two layer copies) and frees them
//! when the tile is installed. 512 KiB is above glibc's initial mmap
//! threshold, so whether those come from the heap or from fresh `mmap`
//! pages (faulted in and zeroed by the kernel, then unmapped again) depends
//! on malloc's dynamic thresholds and on which arena the thread uses: on
//! Linux the round-8 build took 363,000 minor page faults where round 7
//! took 12,000 for the same 400 dab recomposites, and its dab recomposite
//! was 20-30 % slower (`dab_timing_tests`); with the thresholds pinned
//! (`MALLOC_MMAP_THRESHOLD_` and `MALLOC_TRIM_THRESHOLD_`) both builds
//! took about 10,000. Keeping a few buffers per thread takes that out of
//! the allocator's hands.
//!
//! **Only Linux (glibc) reuses a buffer for [`zeroed`]** (round 9 review).
//! Clearing a recycled buffer is a 512 KiB `memset`, about 15 µs. On the
//! Apple M2 Pro that made the plain-photo CPU recomposite about 6 % slower
//! than main (24 MP sequential 83.2-83.9 -> 88.0-88.5 ms, 60 MP 207-208 ->
//! 220-221), because a fresh `calloc` there costs less than that: the VM
//! hands out zero-filled pages. On Linux the same change measured equal to
//! main on that document (24 MP 189-210 against 190-223 ms, 60 MP 472-475
//! against 472-503, noisy), since glibc's `calloc` clears a reused heap
//! chunk itself; there it is the page faults (above) that the pool
//! removes. Elsewhere [`zeroed`] is `transparent_tile` as on main, and
//! only [`copy_of`] (which overwrites the whole buffer either way) reuses.
//!
//! A buffer handed out has exactly the content the allocation it replaces
//! had (all `+0.0` bits for [`zeroed`], a copy of the source for
//! [`copy_of`]), so nothing a composite computes changes. A thread keeps at
//! most [`KEEP`] buffers (2 MiB).

use std::cell::RefCell;

/// The most buffers a thread keeps.
const KEEP: usize = 4;

thread_local! {
    static FREE: RefCell<Vec<Vec<half::f16>>> = const { RefCell::new(Vec::new()) };
}

fn take() -> Option<Vec<half::f16>> {
    FREE.with(|free| free.try_borrow_mut().ok().and_then(|mut free| free.pop()))
}

/// A tile-sized buffer of `+0.0` (bit pattern `0x0000`), what
/// `aurora_render::transparent_tile` returns.
/// Reuses a kept buffer on Linux (glibc) only; see the module docs.
pub(crate) fn zeroed() -> Vec<half::f16> {
    #[cfg(all(target_os = "linux", target_env = "gnu"))]
    if let Some(mut buffer) = take() {
        buffer.clear();
        buffer.resize(aurora_tile::SAMPLES, half::f16::from_bits(0));
        return buffer;
    }
    aurora_render::transparent_tile()
}

/// A copy of `source` (`source.to_vec()`).
pub(crate) fn copy_of(source: &[half::f16]) -> Vec<half::f16> {
    match take() {
        Some(mut buffer) => {
            buffer.clear();
            buffer.extend_from_slice(source);
            buffer
        }
        None => source.to_vec(),
    }
}

/// Gives `buffer` back for reuse on this thread (dropped when the thread
/// already keeps [`KEEP`], or when it is not tile-sized).
pub(crate) fn recycle(buffer: Vec<half::f16>) {
    if buffer.capacity() < aurora_tile::SAMPLES
        || buffer.capacity() > aurora_tile::SAMPLES.saturating_mul(2)
    {
        return;
    }
    FREE.with(|free| {
        if let Ok(mut free) = free.try_borrow_mut()
            && free.len() < KEEP
        {
            free.push(buffer);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{KEEP, copy_of, recycle, zeroed};

    #[test]
    fn a_recycled_buffer_comes_back_zeroed_or_copied() {
        let mut dirty = vec![half::f16::from_bits(0x7E01); aurora_tile::SAMPLES];
        dirty.truncate(17);
        dirty.reserve(aurora_tile::SAMPLES);
        recycle(dirty);
        let zero = zeroed();
        assert_eq!(zero.len(), aurora_tile::SAMPLES);
        assert!(zero.iter().all(|v| v.to_bits() == 0));
        assert_eq!(zero, aurora_render::transparent_tile());
        recycle(zero);
        let source: Vec<_> = (0..aurora_tile::SAMPLES)
            .map(|i| half::f16::from_bits(u16::try_from(i % 65_536).unwrap_or(0)))
            .collect();
        let copy = copy_of(&source);
        assert!(
            copy.iter()
                .zip(&source)
                .all(|(a, b)| a.to_bits() == b.to_bits())
        );
        assert_eq!(copy.len(), source.len());
    }

    /// On Linux (glibc) [`zeroed`] reuses a kept buffer (the dab's
    /// page-fault fix); a kept buffer always serves [`copy_of`].
    #[test]
    fn kept_buffers_are_reused_where_intended() {
        super::FREE.with(|free| free.borrow_mut().clear());
        let buffer = vec![half::f16::ONE; aurora_tile::SAMPLES];
        let address = buffer.as_ptr();
        recycle(buffer);
        let copy = copy_of(&[half::f16::ZERO; 8]);
        assert_eq!(copy.as_ptr(), address, "copy_of reuses a kept buffer");
        recycle(copy);
        let zero = zeroed();
        #[cfg(all(target_os = "linux", target_env = "gnu"))]
        assert_eq!(
            zero.as_ptr(),
            address,
            "zeroed reuses a kept buffer on glibc"
        );
        assert!(zero.iter().all(|v| v.to_bits() == 0));
        super::FREE.with(|free| free.borrow_mut().clear());
    }

    #[test]
    fn a_thread_keeps_at_most_keep_buffers() {
        for _ in 0..KEEP + 3 {
            recycle(vec![half::f16::ZERO; aurora_tile::SAMPLES]);
        }
        let kept = super::FREE.with(|free| free.borrow().len());
        assert_eq!(kept, KEEP);
        // A buffer that is not tile-sized is not kept.
        super::FREE.with(|free| free.borrow_mut().clear());
        recycle(vec![half::f16::ZERO; 16]);
        assert_eq!(super::FREE.with(|free| free.borrow().len()), 0);
    }
}
