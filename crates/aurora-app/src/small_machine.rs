//! Small-machine bounds (0.192.0, ADR 0012's "Memory" addendum).
//!
//! Design owner's decision (Cahya, 2026-10-11): the background
//! compositor's mirror keeps sharing the document's residency pool, and a
//! machine with **at most [`SMALL_MACHINE_MAX_CORES`] cores or at most
//! [`SMALL_MACHINE_MAX_RAM_BYTES`] of RAM** gets a smaller drop-shadow
//! coverage cache ([`SHADOW_CACHE_BYTES_SMALL`], against
//! [`SHADOW_CACHE_BYTES_LARGE`]) and **no overlap** of a PSD layer's tile
//! encode with the next layer's decode
//! (`prepared_pixels::PreparedPsd::stream`).
//!
//! These bounds change speed and memory only, never a pixel: a cache hit
//! returns the value a miss would compute, and the overlap only decides
//! which thread encodes a layer's tiles (the tiles are the same, in the
//! same order). The pool shares (ADR 0011) and the thread counts
//! ([`crate::background_composite::composite_threads`]) are untouched.
//!
//! The decision is [`small_machine`], a pure function of the core count
//! and the detected RAM; [`is_small_machine`] reads the real system once.
//! Detection that fails counts as small (the safe side): an unreadable
//! core count is one core, an unreadable RAM size is `None`.
//!
//! **Override.** `AURORA_SMALL_MACHINE` ([`SMALL_MACHINE_ENV`]) forces
//! either way: `1`, `true`, `on`, `yes` (any value that is not one of the
//! falsy spellings below) force small; `0`, `false`, `off`, `no` force
//! large. Unset, empty or whitespace-only means *detect* — unlike
//! `AURORA_REQUIRE_GPU`, whose empty value means off, an empty override
//! must not silently force a large machine on a small one (CI sets an
//! empty variable when a `${{ }}` expression is empty). Parsing is
//! [`override_from_value`]; spellings are compared trimmed and
//! ASCII-lowercased.

use std::sync::OnceLock;

/// A machine with this many cores or fewer is small.
pub(crate) const SMALL_MACHINE_MAX_CORES: usize = 4;

/// A machine with this much RAM or less is small: 8 GiB. (An "8 GiB"
/// machine reports a little under that, so it is small; a 16 GiB one,
/// which reports about 15.6 GiB, is not.)
pub(crate) const SMALL_MACHINE_MAX_RAM_BYTES: u64 = 8 << 30;

/// The environment variable that forces small or large.
pub(crate) const SMALL_MACHINE_ENV: &str = "AURORA_SMALL_MACHINE";

/// The drop-shadow/stroke/glow coverage cache's cap on a large machine
/// (the cap since 0.182.0): 1,024 tiles.
pub(crate) const SHADOW_CACHE_BYTES_LARGE: usize = 256 << 20;

/// The same cap on a small machine: 256 tiles. Both the background
/// worker's shared cache and each other thread's own use it.
pub(crate) const SHADOW_CACHE_BYTES_SMALL: usize = 64 << 20;

/// The spellings of "off" for [`SMALL_MACHINE_ENV`].
const FALSY_VALUES: [&str; 4] = ["0", "false", "off", "no"];

/// Whether a machine with `cores` available cores and `ram` bytes of
/// physical RAM (`None`: detection failed) is small: `cores` at most
/// [`SMALL_MACHINE_MAX_CORES`], or `ram` at most
/// [`SMALL_MACHINE_MAX_RAM_BYTES`], or `ram` unknown.
#[must_use]
pub(crate) fn small_machine(cores: usize, ram: Option<u64>) -> bool {
    cores <= SMALL_MACHINE_MAX_CORES || ram.is_none_or(|ram| ram <= SMALL_MACHINE_MAX_RAM_BYTES)
}

/// What a [`SMALL_MACHINE_ENV`] value forces: `None` (unset, empty or
/// whitespace) means detect, `Some(false)` forces large, `Some(true)`
/// forces small.
#[must_use]
pub(crate) fn override_from_value(value: Option<&str>) -> Option<bool> {
    let value = value?.trim().to_ascii_lowercase();
    if value.is_empty() {
        return None;
    }
    Some(!FALSY_VALUES.contains(&value.as_str()))
}

/// [`small_machine`] with the override applied: `forced` (from
/// [`override_from_value`]) wins when present.
#[must_use]
pub(crate) fn resolve(cores: usize, ram: Option<u64>, forced: Option<bool>) -> bool {
    forced.unwrap_or_else(|| small_machine(cores, ram))
}

/// The cap for a machine of the given size.
#[must_use]
pub(crate) fn shadow_cache_bytes_for(small: bool) -> usize {
    if small {
        SHADOW_CACHE_BYTES_SMALL
    } else {
        SHADOW_CACHE_BYTES_LARGE
    }
}

/// Whether a machine of the given size may overlap a layer's encode with
/// the next layer's decode.
#[must_use]
pub(crate) fn overlap_allowed_for(small: bool) -> bool {
    !small
}

#[cfg(test)]
thread_local! {
    /// Test-only: this thread's forced answer, ahead of the override and
    /// the detection (`None`: none).
    static FORCED: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

/// Test-only: restores the previous forced answer on drop.
#[cfg(test)]
#[must_use = "the force ends when the guard drops"]
pub(crate) struct ForcedGuard(Option<bool>);

#[cfg(test)]
impl Drop for ForcedGuard {
    fn drop(&mut self) {
        FORCED.with(|forced| forced.set(self.0));
    }
}

/// Test-only: makes [`is_small_machine`] answer `small` **on this thread**
/// until the guard drops, whatever the machine or environment. Code that
/// runs on other threads must be handed the answer by a caller on this one
/// (the worker's shadow cap and the open's overlap flag are).
#[cfg(test)]
pub(crate) fn force_small_machine(small: bool) -> ForcedGuard {
    ForcedGuard(FORCED.with(|forced| forced.replace(Some(small))))
}

/// The core count from `available_parallelism`'s answer: an unreadable
/// one is a single core, which is small (the safe side).
#[must_use]
pub(crate) fn cores_or_one(parallelism: std::io::Result<std::num::NonZeroUsize>) -> usize {
    parallelism.map_or(1, std::num::NonZeroUsize::get)
}

/// Reads this machine once: cores, RAM and the override.
fn detect() -> bool {
    let cores = cores_or_one(std::thread::available_parallelism());
    let ram = aurora_tile::physical_ram_bytes();
    let value = std::env::var(SMALL_MACHINE_ENV).ok();
    let forced = override_from_value(value.as_deref());
    let small = resolve(cores, ram, forced);
    tracing::info!(
        cores,
        ?ram,
        override_value = ?value,
        small,
        shadow_cache_mib = shadow_cache_bytes_for(small) >> 20,
        encode_overlap = overlap_allowed_for(small),
        "small-machine bounds"
    );
    small
}

/// The machine's answer, read once per process.
static SMALL: OnceLock<bool> = OnceLock::new();

/// Whether this machine is small, read once per process (tests can force
/// an answer on their own thread, `force_small_machine`).
#[must_use]
pub(crate) fn is_small_machine() -> bool {
    #[cfg(test)]
    if let Some(forced) = FORCED.with(std::cell::Cell::get) {
        return forced;
    }
    *SMALL.get_or_init(detect)
}

/// The drop-shadow coverage cache cap for this machine.
#[must_use]
pub(crate) fn shadow_cache_bytes() -> usize {
    shadow_cache_bytes_for(is_small_machine())
}

/// Whether the streaming open may overlap encode and decode on this
/// machine.
#[must_use]
pub(crate) fn overlap_allowed() -> bool {
    overlap_allowed_for(is_small_machine())
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;

    #[test]
    fn the_thresholds_are_cahyas() {
        assert_eq!(SMALL_MACHINE_MAX_CORES, 4);
        assert_eq!(SMALL_MACHINE_MAX_RAM_BYTES, 8 * GIB);
        assert_eq!(SHADOW_CACHE_BYTES_LARGE, 256 << 20);
        assert_eq!(SHADOW_CACHE_BYTES_SMALL, 64 << 20);
    }

    #[test]
    fn four_cores_is_small_and_five_is_not() {
        let big = Some(64 * GIB);
        assert!(small_machine(1, big));
        assert!(small_machine(4, big));
        assert!(!small_machine(5, big));
        assert!(!small_machine(64, big));
    }

    #[test]
    fn exactly_eight_gib_is_small_and_one_byte_more_is_not() {
        assert!(small_machine(16, Some(8 * GIB - 1)));
        assert!(small_machine(16, Some(8 * GIB)));
        assert!(!small_machine(16, Some(8 * GIB + 1)));
        assert!(!small_machine(16, Some(62 * GIB)));
        assert!(small_machine(16, Some(0)));
    }

    #[test]
    fn failed_ram_detection_is_small_even_with_many_cores() {
        assert!(small_machine(64, None));
        assert!(small_machine(4, None));
    }

    #[test]
    fn unreadable_parallelism_is_one_core_which_is_small() {
        let failed = Err(std::io::Error::other("no parallelism"));
        assert_eq!(cores_or_one(failed), 1);
        assert!(small_machine(
            cores_or_one(Err(std::io::Error::other("x"))),
            Some(64 * GIB)
        ));
        let Some(eight) = std::num::NonZeroUsize::new(8) else {
            unreachable!("8 is non-zero");
        };
        assert_eq!(cores_or_one(Ok(eight)), 8);
    }

    #[test]
    fn either_limit_alone_makes_a_machine_small() {
        // Few cores, lots of RAM; many cores, little RAM.
        assert!(small_machine(2, Some(128 * GIB)));
        assert!(small_machine(32, Some(4 * GIB)));
        assert!(!small_machine(8, Some(16 * GIB)));
    }

    #[test]
    fn the_override_parses_like_require_gpu_with_empty_meaning_detect() {
        assert_eq!(override_from_value(None), None);
        assert_eq!(override_from_value(Some("")), None);
        assert_eq!(override_from_value(Some("   ")), None);
        for off in ["0", "false", "FALSE", " Off ", "no", "No"] {
            assert_eq!(override_from_value(Some(off)), Some(false), "{off:?}");
        }
        for on in ["1", "true", "True", "on", "yes", "2", "small", " 1 "] {
            assert_eq!(override_from_value(Some(on)), Some(true), "{on:?}");
        }
    }

    #[test]
    fn the_override_wins_over_detection_either_way() {
        assert!(resolve(64, Some(64 * GIB), Some(true)));
        assert!(!resolve(1, None, Some(false)));
        assert!(resolve(1, None, None));
        assert!(!resolve(64, Some(64 * GIB), None));
    }

    #[test]
    fn a_small_machine_gets_the_small_cache_and_no_overlap() {
        assert_eq!(shadow_cache_bytes_for(true), SHADOW_CACHE_BYTES_SMALL);
        assert_eq!(shadow_cache_bytes_for(false), SHADOW_CACHE_BYTES_LARGE);
        assert!(!overlap_allowed_for(true));
        assert!(overlap_allowed_for(false));
    }

    #[test]
    fn a_test_can_force_either_answer_and_the_force_ends() {
        let before = is_small_machine();
        {
            let _small = force_small_machine(true);
            assert!(is_small_machine());
            assert_eq!(shadow_cache_bytes(), SHADOW_CACHE_BYTES_SMALL);
            assert!(!overlap_allowed());
            let _large = force_small_machine(false);
            assert!(!is_small_machine());
            assert_eq!(shadow_cache_bytes(), SHADOW_CACHE_BYTES_LARGE);
            assert!(overlap_allowed());
        }
        assert_eq!(is_small_machine(), before);
    }
}
