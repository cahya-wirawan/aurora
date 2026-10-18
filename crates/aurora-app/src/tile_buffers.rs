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
pub(crate) fn zeroed() -> Vec<half::f16> {
    match take() {
        Some(mut buffer) => {
            buffer.clear();
            buffer.resize(aurora_tile::SAMPLES, half::f16::from_bits(0));
            buffer
        }
        None => aurora_render::transparent_tile(),
    }
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
