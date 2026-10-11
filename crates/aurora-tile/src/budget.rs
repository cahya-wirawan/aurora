//! The resident-tile budget, sized from physical RAM (0.185.0, ADR 0011).
//!
//! Until 0.184.0 the app's only store held a fixed 256 tiles (128 MiB).
//! A large template's working set is many times that, and every full
//! composite then paged most of it in from the scratch disk again. The
//! design owner's decision (Cahya, 2026-10-11) is that the budget is a
//! **share of system RAM**. This module holds that policy:
//!
//! - [`budget_bytes_for_ram`]: a quarter of physical RAM, clamped to
//!   [[`FLOOR_BYTES`], [`CEILING_BYTES`]], and the floor when the RAM is
//!   unknown. A pure function, so every case is a unit test.
//! - [`parse_override_mb`]: the `AURORA_TILE_BUDGET_MB` override, parsed
//!   defensively and clamped to the same range.
//! - [`physical_ram_bytes`]: the detection, per OS.
//! - [`ResidencyPool`]: one budget shared by every store that joins it.
//!   Each member's share is `max(floor, total / members)`, re-read on
//!   every admission, so a store that leaves grows the others at once.
//!   A member that admits nothing (a parked document) only shrinks when
//!   its owner calls [`crate::TileStore::enforce_budget`].
//!
//! The budget is a cap on what may be resident, never a preload
//! (invariant §7.3.1): nothing here reads a tile.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::SAMPLES;

/// Bytes one resident tile holds: 256 × 256 texels × 4 channels × 2
/// bytes (`f16`), 512 KiB. Every byte figure here is converted to tiles
/// through this, so a future tile-size change cannot leave the budget
/// counted in the old unit.
pub const TILE_BYTES: u64 = (SAMPLES * std::mem::size_of::<half::f16>()) as u64;

/// The share of physical RAM the tile budget takes: one quarter
/// (`ram / RAM_SHARE_DIVISOR`). The other three quarters are for the OS,
/// the GPU driver's own staging, the drop-shadow cache (256 MiB), the
/// undo journal and every other application the user has open.
pub const RAM_SHARE_DIVISOR: u64 = 4;

/// The smallest budget: 128 MiB, the fixed budget every release up to
/// 0.184.0 used (256 tiles). Also what a store gets when the RAM cannot
/// be detected, and the least any one member of a pool is cut to.
pub const FLOOR_BYTES: u64 = 128 << 20;

/// The largest budget: 8 GiB (16,384 tiles). A quarter of a 32 GiB
/// machine; past that a bigger cache buys nothing a single document can
/// use on screen, and a workstation's RAM is better left to other work.
pub const CEILING_BYTES: u64 = 8 << 30;

/// The most tiles one call evicts to bring an over-budget store back
/// inside its share (0.185.0 review RB-1): [`crate::TileStore::enforce_budget`]
/// callers pass it, and an admission evicts at most this many before it
/// admits. Each eviction encodes one 512 KiB tile on the caller's thread,
/// so a store that a newly joined pool member left thousands of tiles
/// over its share converges over several event-loop turns instead of
/// freezing one. 32 is an estimate of about one frame of encoding, not a
/// measurement.
pub const EVICTION_BURST: usize = 32;

/// The environment variable that overrides the RAM-derived budget, in
/// MiB (base 2). Read by [`system_budget_bytes`]; see
/// [`parse_override_mb`] for what it accepts.
pub const OVERRIDE_ENV: &str = "AURORA_TILE_BUDGET_MB";

/// The budget, in bytes, for a machine with `ram` bytes of physical
/// memory: `ram / 4`, clamped to [[`FLOOR_BYTES`], [`CEILING_BYTES`]].
/// `None` (detection failed) and `Some(0)` give the floor.
#[must_use]
pub const fn budget_bytes_for_ram(ram: Option<u64>) -> u64 {
    let share = match ram {
        Some(ram) => ram / RAM_SHARE_DIVISOR,
        None => FLOOR_BYTES,
    };
    clamp_bytes(share)
}

/// Clamps `bytes` to [[`FLOOR_BYTES`], [`CEILING_BYTES`]].
#[must_use]
pub const fn clamp_bytes(bytes: u64) -> u64 {
    if bytes < FLOOR_BYTES {
        FLOOR_BYTES
    } else if bytes > CEILING_BYTES {
        CEILING_BYTES
    } else {
        bytes
    }
}

/// Parses an [`OVERRIDE_ENV`] value: a whole number of MiB, surrounding
/// whitespace ignored, clamped to [[`FLOOR_BYTES`], [`CEILING_BYTES`]]
/// and returned in bytes. Anything else — empty, a sign, a fraction, a
/// unit suffix, text — is `None`, so the RAM-derived budget stands. A
/// number too large for `u64` is clamped to the ceiling like any other
/// large value rather than rejected.
#[must_use]
pub fn parse_override_mb(value: &str) -> Option<u64> {
    let digits = value.trim();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    // All ASCII digits: the only parse failure left is overflow.
    let mib = digits.parse::<u64>().unwrap_or(u64::MAX);
    Some(clamp_bytes(mib.saturating_mul(1 << 20)))
}

/// The budget in bytes from a detected `ram` and an optional override
/// value: the override when it parses, else [`budget_bytes_for_ram`].
#[must_use]
pub fn resolve_budget_bytes(ram: Option<u64>, override_value: Option<&str>) -> u64 {
    override_value
        .and_then(parse_override_mb)
        .unwrap_or_else(|| budget_bytes_for_ram(ram))
}

/// [`resolve_budget_bytes`] for this machine and process:
/// [`physical_ram_bytes`] and the [`OVERRIDE_ENV`] variable (a value
/// that is not valid Unicode is ignored).
#[must_use]
pub fn system_budget_bytes() -> u64 {
    let value = std::env::var(OVERRIDE_ENV).ok();
    resolve_budget_bytes(physical_ram_bytes(), value.as_deref())
}

/// How many whole tiles `bytes` holds, at least one.
#[must_use]
pub fn tiles_for_bytes(bytes: u64) -> NonZeroUsize {
    let tiles = usize::try_from(bytes / TILE_BYTES).unwrap_or(usize::MAX);
    NonZeroUsize::new(tiles).unwrap_or(NonZeroUsize::MIN)
}

/// One member's share of a pool of `total` tiles split `members` ways:
/// `max(floor, total / members)`, with no members counted as one. The
/// floor wins, so with very many members the pool's sum can exceed
/// `total` by up to `members × floor` — each document keeps a working
/// set rather than thrashing on a handful of tiles.
#[must_use]
pub fn share_tiles(total: NonZeroUsize, floor: NonZeroUsize, members: usize) -> NonZeroUsize {
    let split = total.get() / members.max(1);
    NonZeroUsize::new(split.max(floor.get())).unwrap_or(floor)
}

/// The total physical memory of this machine in bytes, or `None` when
/// it cannot be read. Linux reads `/proc/meminfo`'s `MemTotal` (no
/// `unsafe`), macOS `sysctl hw.memsize`, Windows
/// `GlobalMemoryStatusEx`; any other OS reports `None`. A container's
/// cgroup memory limit is not consulted: the host's RAM is reported.
#[must_use]
pub fn physical_ram_bytes() -> Option<u64> {
    detect_ram().filter(|&bytes| bytes > 0)
}

#[cfg(target_os = "linux")]
fn detect_ram() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/meminfo").ok()?;
    parse_meminfo_total(&text)
}

/// `MemTotal` from a `/proc/meminfo` text, in bytes (`kB` there is KiB).
/// `None` when the line is missing or malformed, or its unit is not
/// `kB`.
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo_total(text: &str) -> Option<u64> {
    let line = text.lines().find(|line| line.starts_with("MemTotal:"))?;
    let mut fields = line.trim_start_matches("MemTotal:").split_whitespace();
    let kib: u64 = fields.next()?.parse().ok()?;
    match (fields.next(), fields.next()) {
        (Some("kB"), None) => kib.checked_mul(1024),
        _ => None,
    }
}

#[cfg(target_os = "macos")]
fn detect_ram() -> Option<u64> {
    let mut bytes: u64 = 0;
    let mut len = std::mem::size_of::<u64>();
    // SAFETY: `hw.memsize` is a 64-bit integer sysctl. `oldp` points at
    // a live, writable `u64` and `oldlenp` at its size, so the kernel
    // writes at most `len` bytes into it; `newp` is null with length 0,
    // so nothing is set. Both pointers outlive the call.
    let status = unsafe {
        libc::sysctlbyname(
            c"hw.memsize".as_ptr(),
            (&raw mut bytes).cast::<libc::c_void>(),
            &raw mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (status == 0 && len == std::mem::size_of::<u64>()).then_some(bytes)
}

#[cfg(windows)]
fn detect_ram() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    let mut status = MEMORYSTATUSEX {
        dwLength: u32::try_from(std::mem::size_of::<MEMORYSTATUSEX>()).ok()?,
        ..MEMORYSTATUSEX::default()
    };
    // SAFETY: `status` is a live, writable `MEMORYSTATUSEX` whose
    // `dwLength` is set to its own size, as the API requires; the call
    // only writes into it.
    let ok = unsafe { GlobalMemoryStatusEx(&raw mut status) };
    (ok != 0).then_some(status.ullTotalPhys)
}

#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
const fn detect_ram() -> Option<u64> {
    None
}

/// One resident-tile budget shared by every store that joins it
/// ([`crate::TileStore::new_shared`]). The app keeps one per process, so
/// every open document — active, parked, or still opening — draws on the
/// same total.
///
/// Each member's budget is [`Self::share`], `max(floor, total /
/// members)`, read again whenever the member admits a tile, so:
///
/// - a store joining shrinks every other member's share at once, and the
///   active store evicts down to it on its next admission; a store that
///   admits nothing (a parked document) evicts only when its owner calls
///   [`crate::TileStore::enforce_budget`];
/// - a store dropped (a closed document) leaves, and every remaining
///   member's share grows at once. Growth needs no eviction.
///
/// Eviction always goes through the store's own path: a dirty tile is
/// encoded and written to scratch, never dropped.
#[derive(Debug)]
pub struct ResidencyPool {
    total: NonZeroUsize,
    floor: NonZeroUsize,
    members: AtomicUsize,
}

impl ResidencyPool {
    /// A pool of `total` tiles whose members are never cut below `floor`
    /// tiles (`floor` is capped at `total`).
    #[must_use]
    pub fn new(total: NonZeroUsize, floor: NonZeroUsize) -> Arc<Self> {
        Arc::new(Self {
            total,
            floor: floor.min(total),
            members: AtomicUsize::new(0),
        })
    }

    /// A pool of `total_bytes`, with the member floor at
    /// [`FLOOR_BYTES`], both converted through [`tiles_for_bytes`].
    #[must_use]
    pub fn from_bytes(total_bytes: u64) -> Arc<Self> {
        Self::new(tiles_for_bytes(total_bytes), tiles_for_bytes(FLOOR_BYTES))
    }

    /// [`Self::from_bytes`] of [`system_budget_bytes`].
    #[must_use]
    pub fn from_system() -> Arc<Self> {
        Self::from_bytes(system_budget_bytes())
    }

    /// The pool's whole budget, in tiles.
    #[must_use]
    pub const fn total_tiles(&self) -> NonZeroUsize {
        self.total
    }

    /// The least any member is cut to, in tiles.
    #[must_use]
    pub const fn floor_tiles(&self) -> NonZeroUsize {
        self.floor
    }

    /// How many stores are members now.
    #[must_use]
    pub fn members(&self) -> usize {
        self.members.load(Ordering::Relaxed)
    }

    /// One member's budget now, in tiles ([`share_tiles`]).
    #[must_use]
    pub fn share(&self) -> NonZeroUsize {
        share_tiles(self.total, self.floor, self.members())
    }

    pub(crate) fn join(self: &Arc<Self>) -> PoolMembership {
        self.members.fetch_add(1, Ordering::Relaxed);
        PoolMembership(Arc::clone(self))
    }
}

/// A store's place in a [`ResidencyPool`]; leaving is its `Drop`, so a
/// store counts exactly as long as it exists.
#[derive(Debug)]
pub(crate) struct PoolMembership(Arc<ResidencyPool>);

impl PoolMembership {
    pub(crate) fn share(&self) -> NonZeroUsize {
        self.0.share()
    }

    pub(crate) fn pool(&self) -> Arc<ResidencyPool> {
        Arc::clone(&self.0)
    }
}

impl Drop for PoolMembership {
    fn drop(&mut self) {
        // Saturating: a membership only exists after its own increment,
        // so this never underflows, but a wrap would be worse than a
        // stuck count.
        let _ = self
            .0
            .members
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
                Some(n.saturating_sub(1))
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1 << 30;
    const MIB: u64 = 1 << 20;

    fn nz(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap_or(NonZeroUsize::MIN)
    }

    #[test]
    fn a_tile_is_512_kib_and_the_floor_is_the_old_256_tile_budget() {
        assert_eq!(TILE_BYTES, 512 * 1024);
        assert_eq!(tiles_for_bytes(FLOOR_BYTES).get(), 256);
        assert_eq!(tiles_for_bytes(CEILING_BYTES).get(), 16_384);
    }

    #[test]
    fn the_budget_is_a_quarter_of_ram_between_the_floor_and_the_ceiling() {
        let cases = [
            (4 * GIB, GIB),
            (8 * GIB, 2 * GIB),
            (16 * GIB, 4 * GIB),
            (32 * GIB, 8 * GIB),
            (128 * GIB, CEILING_BYTES),
        ];
        for (ram, want) in cases {
            assert_eq!(budget_bytes_for_ram(Some(ram)), want, "ram {ram}");
        }
        assert_eq!(
            tiles_for_bytes(budget_bytes_for_ram(Some(4 * GIB))).get(),
            2048
        );
        assert_eq!(
            tiles_for_bytes(budget_bytes_for_ram(Some(16 * GIB))).get(),
            8192
        );
    }

    #[test]
    fn unknown_and_absurd_ram_values_stay_inside_the_floor_and_ceiling() {
        assert_eq!(budget_bytes_for_ram(None), FLOOR_BYTES);
        assert_eq!(budget_bytes_for_ram(Some(0)), FLOOR_BYTES);
        assert_eq!(budget_bytes_for_ram(Some(1)), FLOOR_BYTES);
        assert_eq!(budget_bytes_for_ram(Some(256 * MIB)), FLOOR_BYTES);
        assert_eq!(budget_bytes_for_ram(Some(u64::MAX)), CEILING_BYTES);
        assert_eq!(budget_bytes_for_ram(Some(1 << 50)), CEILING_BYTES);
        // Just inside each end.
        assert_eq!(
            budget_bytes_for_ram(Some(4 * FLOOR_BYTES + 4)),
            FLOOR_BYTES + 1
        );
        assert_eq!(
            budget_bytes_for_ram(Some(4 * CEILING_BYTES - 4)),
            CEILING_BYTES - 1
        );
    }

    #[test]
    fn the_override_is_whole_mib_clamped_and_anything_else_is_ignored() {
        assert_eq!(parse_override_mb("1024"), Some(GIB));
        assert_eq!(parse_override_mb("  2048\n"), Some(2 * GIB));
        assert_eq!(parse_override_mb("1"), Some(FLOOR_BYTES));
        assert_eq!(parse_override_mb("0"), Some(FLOOR_BYTES));
        assert_eq!(parse_override_mb("1000000"), Some(CEILING_BYTES));
        assert_eq!(
            parse_override_mb("99999999999999999999999999"),
            Some(CEILING_BYTES)
        );
        assert_eq!(
            parse_override_mb(&u64::MAX.to_string()),
            Some(CEILING_BYTES)
        );
        for bad in [
            "",
            "   ",
            "-1",
            "+512",
            "1.5",
            "512MB",
            "1e3",
            "lots",
            "0x100",
            "５１２",
        ] {
            assert_eq!(parse_override_mb(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn an_override_that_parses_wins_and_one_that_does_not_falls_back_to_ram() {
        let ram = Some(16 * GIB);
        assert_eq!(resolve_budget_bytes(ram, None), 4 * GIB);
        assert_eq!(resolve_budget_bytes(ram, Some("512")), 512 * MIB);
        assert_eq!(resolve_budget_bytes(ram, Some("nonsense")), 4 * GIB);
        assert_eq!(resolve_budget_bytes(None, Some("")), FLOOR_BYTES);
        assert_eq!(resolve_budget_bytes(None, Some("100000")), CEILING_BYTES);
    }

    #[test]
    fn meminfo_parsing_reads_memtotal_in_kib_and_rejects_anything_else() {
        let text = "MemFree:  10 kB\nMemTotal:       65536000 kB\nSwap: 1 kB\n";
        assert_eq!(parse_meminfo_total(text), Some(65_536_000 * 1024));
        assert_eq!(parse_meminfo_total("MemFree: 1 kB\n"), None);
        assert_eq!(parse_meminfo_total("MemTotal: lots kB\n"), None);
        assert_eq!(parse_meminfo_total("MemTotal: 12 MB\n"), None);
        assert_eq!(parse_meminfo_total("MemTotal: 12\n"), None);
        assert_eq!(parse_meminfo_total("MemTotal: 12 kB extra\n"), None);
        assert_eq!(
            parse_meminfo_total(&format!("MemTotal: {} kB\n", u64::MAX)),
            None
        );
    }

    #[test]
    fn this_machine_reports_some_ram_and_a_budget_inside_the_range() {
        // Linux only reads a file; a sandbox without `/proc` gives
        // `None`, which is the floor, and still inside the range.
        let budget = system_budget_bytes();
        assert!((FLOOR_BYTES..=CEILING_BYTES).contains(&budget));
        if cfg!(any(target_os = "linux", target_os = "macos", windows)) {
            println!("physical RAM {:?}, budget {budget}", physical_ram_bytes());
        }
    }

    #[test]
    fn shares_split_the_total_and_never_go_below_the_floor() {
        let (total, floor) = (nz(1000), nz(100));
        assert_eq!(share_tiles(total, floor, 0).get(), 1000);
        assert_eq!(share_tiles(total, floor, 1).get(), 1000);
        assert_eq!(share_tiles(total, floor, 2).get(), 500);
        assert_eq!(share_tiles(total, floor, 3).get(), 333);
        assert_eq!(share_tiles(total, floor, 10).get(), 100);
        assert_eq!(share_tiles(total, floor, 11).get(), 100);
        assert_eq!(share_tiles(total, floor, usize::MAX).get(), 100);
    }

    #[test]
    fn joining_and_leaving_a_pool_moves_every_members_share() {
        let pool = ResidencyPool::new(nz(1000), nz(100));
        assert_eq!(pool.share().get(), 1000);
        let a = pool.join();
        assert_eq!(a.share().get(), 1000);
        let b = pool.join();
        assert_eq!((a.share().get(), b.share().get()), (500, 500));
        drop(b);
        assert_eq!((pool.members(), a.share().get()), (1, 1000));
        drop(a);
        assert_eq!(pool.members(), 0);
        // The floor is capped at the total.
        assert_eq!(ResidencyPool::new(nz(10), nz(100)).floor_tiles().get(), 10);
    }
}
