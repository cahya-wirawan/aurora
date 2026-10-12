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
//!   Each member's share is re-read on every admission, so a store that
//!   leaves grows the others at once. A member that admits nothing (a
//!   parked document) only shrinks when its owner calls
//!   [`crate::TileStore::enforce_budget`].
//! - [`pool_shares`] (0.187.0): how the pool splits. One *group* of
//!   members (the active document's store and its background
//!   compositor's mirror) is active; it gets three quarters of the total
//!   and the parked members split the last quarter, each floored. With
//!   no active group, or no parked member, the split is equal
//!   ([`share_tiles`]).
//!
//! The budget is a cap on what may be resident, never a preload
//! (invariant §7.3.1): nothing here reads a tile.

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

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

/// The parked members of a pool that has an active group together get
/// `total / PARKED_RESERVE_DIVISOR` (a quarter), and the active group
/// the rest (0.187.0, design-owner decision "the active document gets a
/// bigger share", Cahya, 2026-10-11; the quarter is engineering's
/// choice, ADR 0011's 0.187.0 addendum). A quarter of this machine's
/// 16,384 tiles is 4,096, more than the Nyepi template's ~2,900-tile
/// working set, so one parked large document stays warm for switching
/// back; the active document's three quarters never shrink as more
/// documents are parked, only the floors of very many parked ones can
/// eat into them.
pub const PARKED_RESERVE_DIVISOR: usize = 4;

/// Each member's share of a pool, by role ([`pool_shares`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolShares {
    /// What each member of the active group may hold, in tiles.
    pub active: NonZeroUsize,
    /// What each parked member may hold, in tiles.
    pub parked: NonZeroUsize,
}

/// How a pool of `total` tiles splits between `active` members (the
/// active group: the active document's store and its mirror) and
/// `parked` members (every other store: parked documents, a pending
/// open), never cutting anyone below `floor` (0.187.0):
///
/// - With no active member or no parked member, everyone gets the equal
///   split [`share_tiles`] of `active + parked` members — exactly the
///   0.185.0/0.186.0 rule.
/// - Otherwise the parked members split `total / PARKED_RESERVE_DIVISOR`
///   (a quarter), each at least `floor`; the active group gets what the
///   parked shares leave — at least three quarters, less only when the
///   parked floors do not fit in the quarter — split evenly, each at
///   least `floor`.
///
/// So the sum of every member's share is at most `max(total, members ×
/// floor)`: within the total whenever the floors fit in it, and only the
/// floors themselves past it (never more than one floor past it while
/// `(members − 1) × floor ≤ total`). An active member never gets less
/// than the equal split would give it, and a parked one never less than
/// `floor`.
///
/// These bounds assume a small active group — at most three members
/// (a document, its mirror and one mirror the worker has not yet
/// released), which is all the app creates. A larger active group can
/// break them: with total 100, floor 10, nine active and one parked the
/// sum is 115.
#[must_use]
pub fn pool_shares(
    total: NonZeroUsize,
    floor: NonZeroUsize,
    active: usize,
    parked: usize,
) -> PoolShares {
    if active == 0 || parked == 0 {
        let equal = share_tiles(total, floor, active.saturating_add(parked));
        return PoolShares {
            active: equal,
            parked: equal,
        };
    }
    let reserve = total.get() / PARKED_RESERVE_DIVISOR;
    let parked_each = (reserve / parked).max(floor.get());
    let active_set = total
        .get()
        .saturating_sub(parked_each.saturating_mul(parked));
    let active_each = (active_set / active).max(floor.get());
    PoolShares {
        active: NonZeroUsize::new(active_each).unwrap_or(floor),
        parked: NonZeroUsize::new(parked_each).unwrap_or(floor),
    }
}

/// The share of the tile budget the pixel-history memory budget takes:
/// one quarter (`tile_budget / HISTORY_SHARE_DIVISOR`, 0.210.0, ADR 0011's
/// 0.210.0 addendum). The history holds *compressed* tile copies, so a
/// quarter of the tile budget keeps many more edits than the resident
/// tiles it sits beside; whatever does not fit is spilled to the
/// document's scratch directory, never dropped.
pub const HISTORY_SHARE_DIVISOR: u64 = 4;

/// The smallest pixel-history memory budget: 32 MiB (a quarter of
/// [`FLOOR_BYTES`]).
pub const HISTORY_FLOOR_BYTES: u64 = FLOOR_BYTES / HISTORY_SHARE_DIVISOR;

/// The largest pixel-history memory budget: 2 GiB (a quarter of
/// [`CEILING_BYTES`]).
pub const HISTORY_CEILING_BYTES: u64 = CEILING_BYTES / HISTORY_SHARE_DIVISOR;

/// The smallest value the history override accepts: 1 MiB, so a tester
/// can force spilling with a small document. (The tile override's floor
/// is much higher because a tile store needs a working set; the history
/// only needs to stay correct when everything spills.)
pub const HISTORY_OVERRIDE_MIN_BYTES: u64 = 1 << 20;

/// The environment variable that overrides the derived pixel-history
/// memory budget, in MiB (base 2), read by [`system_history_budget_bytes`].
pub const HISTORY_OVERRIDE_ENV: &str = "AURORA_HISTORY_BUDGET_MB";

/// The pixel-history memory budget for a tile budget of `tile_budget`
/// bytes: a quarter of it, clamped to [[`HISTORY_FLOOR_BYTES`],
/// [`HISTORY_CEILING_BYTES`]].
#[must_use]
pub const fn history_budget_for_tile_budget(tile_budget: u64) -> u64 {
    let share = tile_budget / HISTORY_SHARE_DIVISOR;
    if share < HISTORY_FLOOR_BYTES {
        HISTORY_FLOOR_BYTES
    } else if share > HISTORY_CEILING_BYTES {
        HISTORY_CEILING_BYTES
    } else {
        share
    }
}

/// Parses a [`HISTORY_OVERRIDE_ENV`] value: whole MiB, whitespace
/// ignored, clamped to [[`HISTORY_OVERRIDE_MIN_BYTES`],
/// [`CEILING_BYTES`]]; anything else is `None` (the derived budget
/// stands).
#[must_use]
pub fn parse_history_override_mb(value: &str) -> Option<u64> {
    let digits = value.trim();
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mib = digits.parse::<u64>().unwrap_or(u64::MAX);
    Some(
        mib.saturating_mul(1 << 20)
            .clamp(HISTORY_OVERRIDE_MIN_BYTES, CEILING_BYTES),
    )
}

/// The history budget from a tile budget and an optional override value:
/// the override when it parses, else [`history_budget_for_tile_budget`].
#[must_use]
pub fn resolve_history_budget_bytes(tile_budget: u64, override_value: Option<&str>) -> u64 {
    override_value
        .and_then(parse_history_override_mb)
        .unwrap_or_else(|| history_budget_for_tile_budget(tile_budget))
}

/// [`resolve_history_budget_bytes`] for this machine and process: the
/// [`system_budget_bytes`] tile budget and [`HISTORY_OVERRIDE_ENV`].
#[must_use]
pub fn system_history_budget_bytes() -> u64 {
    let value = std::env::var(HISTORY_OVERRIDE_ENV).ok();
    resolve_history_budget_bytes(system_budget_bytes(), value.as_deref())
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

/// Which members of a [`ResidencyPool`] share one role (0.187.0): a
/// document's store and its background compositor's mirror are one
/// group, so making the document active makes both active. Ids are
/// unique for the whole process, so a group from one pool can never name
/// another pool's members.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PoolGroup(u64);

impl PoolGroup {
    fn next() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        Self(NEXT.fetch_add(1, Ordering::Relaxed))
    }
}

/// One resident-tile budget shared by every store that joins it
/// ([`crate::TileStore::new_shared`]). The app keeps one per process, so
/// every open document — active, parked, or still opening — draws on the
/// same total.
///
/// Every member belongs to a [`PoolGroup`]; at most one group is
/// *active* at a time ([`Self::activate`]), and the shares are
/// [`pool_shares`] of the active group's member count and everyone
/// else's. Each member's share is read again whenever it admits a tile,
/// so:
///
/// - a store joining, or the active group moving, shrinks some members'
///   shares at once; the active store evicts down to its share on its
///   next admission, and a store that admits nothing (a parked document)
///   evicts only when its owner calls
///   [`crate::TileStore::enforce_budget`];
/// - a store dropped (a closed document) leaves, and every remaining
///   member's share grows or stays. Growth needs no eviction.
///
/// Eviction always goes through the store's own path: a dirty tile is
/// encoded and written to scratch, never dropped.
#[derive(Debug)]
pub struct ResidencyPool {
    total: NonZeroUsize,
    floor: NonZeroUsize,
    state: Mutex<PoolState>,
}

/// Who is in a pool: members per group and the active group. Behind one
/// lock so a share is never computed from a half-moved active group.
#[derive(Debug, Default)]
struct PoolState {
    groups: HashMap<PoolGroup, usize>,
    members: usize,
    active: Option<PoolGroup>,
}

impl PoolState {
    fn active_members(&self) -> usize {
        self.active
            .and_then(|group| self.groups.get(&group).copied())
            .unwrap_or(0)
    }
}

impl ResidencyPool {
    /// A pool of `total` tiles whose members are never cut below `floor`
    /// tiles (`floor` is capped at `total`).
    #[must_use]
    pub fn new(total: NonZeroUsize, floor: NonZeroUsize) -> Arc<Self> {
        Arc::new(Self {
            total,
            floor: floor.min(total),
            state: Mutex::new(PoolState::default()),
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

    /// The membership state. A panic while it was held cannot leave it
    /// half-updated (every update is a few integer writes that cannot
    /// panic), so a poisoned lock is simply taken over.
    fn state(&self) -> MutexGuard<'_, PoolState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
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
        self.state().members
    }

    /// How many members belong to the active group now (0 when no group
    /// is active).
    #[must_use]
    pub fn active_members(&self) -> usize {
        self.state().active_members()
    }

    /// The active group, if one is set and still has members.
    #[must_use]
    pub fn active_group(&self) -> Option<PoolGroup> {
        let state = self.state();
        state
            .active
            .filter(|group| state.groups.contains_key(group))
    }

    /// Makes `group` the active one (0.187.0), parking whichever was
    /// active before: there is only ever one active group. Shares move
    /// at once; nothing is evicted here.
    pub fn activate(&self, group: PoolGroup) {
        self.state().active = Some(group);
    }

    /// Parks `group` if it is the active one, leaving no group active
    /// (every member then gets the equal split). Returns whether it was
    /// active.
    pub fn deactivate(&self, group: PoolGroup) -> bool {
        let mut state = self.state();
        let was = state.active == Some(group);
        if was {
            state.active = None;
        }
        was
    }

    /// Each role's share now ([`pool_shares`]).
    #[must_use]
    pub fn shares(&self) -> PoolShares {
        let state = self.state();
        let active = state.active_members();
        pool_shares(
            self.total,
            self.floor,
            active,
            state.members.saturating_sub(active),
        )
    }

    /// A parked member's share now, in tiles — every member's, while no
    /// group is active (the 0.185.0 meaning: `max(floor, total /
    /// members)`). [`Self::shares`] has the active group's too.
    #[must_use]
    pub fn share(&self) -> NonZeroUsize {
        self.shares().parked
    }

    /// The share a member of `group` may hold now, in tiles.
    #[must_use]
    pub fn share_for(&self, group: PoolGroup) -> NonZeroUsize {
        let state = self.state();
        let active = state.active_members();
        let shares = pool_shares(
            self.total,
            self.floor,
            active,
            state.members.saturating_sub(active),
        );
        if active > 0 && state.active == Some(group) {
            shares.active
        } else {
            shares.parked
        }
    }

    /// Joins as the only member of a new group.
    pub(crate) fn join(self: &Arc<Self>) -> PoolMembership {
        self.join_group(PoolGroup::next())
    }

    /// Joins `group` (0.187.0: the mirror joins its document's group).
    pub(crate) fn join_group(self: &Arc<Self>, group: PoolGroup) -> PoolMembership {
        let mut state = self.state();
        *state.groups.entry(group).or_insert(0) += 1;
        state.members = state.members.saturating_add(1);
        drop(state);
        PoolMembership {
            pool: Arc::clone(self),
            group,
        }
    }

    fn leave(&self, group: PoolGroup) {
        let mut state = self.state();
        // Saturating: a membership only exists after its own increment,
        // so this never underflows, but a wrap would be worse than a
        // stuck count.
        state.members = state.members.saturating_sub(1);
        let emptied = match state.groups.get_mut(&group) {
            Some(count) => {
                *count = count.saturating_sub(1);
                *count == 0
            }
            None => false,
        };
        if emptied {
            state.groups.remove(&group);
            // An active group whose last member left (its document
            // closed) is active no longer.
            if state.active == Some(group) {
                state.active = None;
            }
        }
    }
}

/// A store's place in a [`ResidencyPool`]; leaving is its `Drop`, so a
/// store counts exactly as long as it exists.
#[derive(Debug)]
pub(crate) struct PoolMembership {
    pool: Arc<ResidencyPool>,
    group: PoolGroup,
}

impl PoolMembership {
    pub(crate) fn share(&self) -> NonZeroUsize {
        self.pool.share_for(self.group)
    }

    pub(crate) fn pool(&self) -> Arc<ResidencyPool> {
        Arc::clone(&self.pool)
    }

    pub(crate) const fn group(&self) -> PoolGroup {
        self.group
    }
}

impl Drop for PoolMembership {
    fn drop(&mut self) {
        self.pool.leave(self.group);
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

    /// The sum of every member's share for `active` + `parked` members.
    fn shares_sum(shares: PoolShares, active: usize, parked: usize) -> usize {
        shares.active.get() * active + shares.parked.get() * parked
    }

    /// 0.187.0: the worked cases of the weighting, at this machine's
    /// 16,384-tile pool with the 256-tile floor.
    #[test]
    fn the_active_group_gets_three_quarters_and_the_parked_split_a_quarter() {
        let (total, floor) = (nz(16_384), nz(256));
        let at = |active, parked| {
            let shares = pool_shares(total, floor, active, parked);
            (shares.active.get(), shares.parked.get())
        };
        // One document alone: everything; with its mirror: half each.
        assert_eq!(at(1, 0), (16_384, 16_384));
        assert_eq!(at(2, 0), (8_192, 8_192));
        // One parked document gets a quarter (4,096 tiles, more than
        // Nyepi's ~2,900-tile working set), the active one the rest.
        assert_eq!(at(1, 1), (12_288, 4_096));
        assert_eq!(at(2, 1), (6_144, 4_096));
        // More parked documents split the quarter; the active group's
        // three quarters do not move.
        assert_eq!(at(1, 4), (12_288, 1_024));
        assert_eq!(at(2, 16), (6_144, 256));
        // Past 16 parked the floors no longer fit in the quarter and come
        // out of the active group's share, never below its own floor.
        assert_eq!(at(1, 20), (16_384 - 20 * 256, 256));
        assert_eq!(at(2, 63), (256, 256));
        assert_eq!(at(1, 200), (256, 256));
        // No active group: the 0.186.0 equal split.
        assert_eq!(at(0, 3), (5_461, 5_461));
        assert_eq!(at(0, 0), (16_384, 16_384));
    }

    /// 0.187.0: over every small pool, floor and member count, with and
    /// without a mirror in the active group: nobody is below the floor;
    /// the sum of shares is within `max(total, members × floor)`, so
    /// within the total plus one floor whenever the other members' floors
    /// fit in the total; an active member never gets less than the
    /// 0.186.0 equal split; and an active member gets strictly more than
    /// a parked one whenever the total leaves room for that.
    #[test]
    fn pool_shares_keep_the_floor_and_the_bound_for_every_member_count() {
        let mut checked = 0_usize;
        for total in 1..=160 {
            for floor in 1..=total {
                for active in 0..=3 {
                    for parked in 0..=12 {
                        let shares = pool_shares(nz(total), nz(floor), active, parked);
                        let members = active + parked;
                        let case = format!("total {total} floor {floor} a {active} p {parked}");
                        assert!(shares.active.get() >= floor, "{case}");
                        assert!(shares.parked.get() >= floor, "{case}");
                        let sum = shares_sum(shares, active, parked);
                        assert!(sum <= total.max(members * floor), "{case}: sum {sum}");
                        if members.saturating_sub(1) * floor <= total {
                            assert!(sum <= total + floor, "{case}: sum {sum}");
                        }
                        if active > 0 {
                            let equal = share_tiles(nz(total), nz(floor), members).get();
                            assert!(shares.active.get() >= equal, "{case}: below equal");
                        }
                        if active > 0
                            && active <= 2
                            && parked > 0
                            && active * (floor + 1) + parked * floor <= total
                        {
                            assert!(
                                shares.active.get() > shares.parked.get(),
                                "{case}: {shares:?}"
                            );
                        }
                        checked += 1;
                    }
                }
            }
        }
        assert!(checked > 600_000, "{checked}");
    }

    #[test]
    fn activating_a_group_moves_the_bigger_share_and_there_is_only_one() {
        let pool = ResidencyPool::new(nz(1000), nz(100));
        let a = pool.join();
        let b = pool.join();
        // Nothing active yet: the equal split.
        assert_eq!((a.share().get(), b.share().get()), (500, 500));
        assert_eq!((pool.active_group(), pool.active_members()), (None, 0));
        pool.activate(a.group());
        assert_eq!((a.share().get(), b.share().get()), (750, 250));
        assert_eq!(pool.active_members(), 1);
        // A switch: b active, a parked, at once.
        pool.activate(b.group());
        assert_eq!((a.share().get(), b.share().get()), (250, 750));
        assert_eq!(pool.active_group(), Some(b.group()));
        // Parking a group that is not the active one changes nothing.
        assert!(!pool.deactivate(a.group()));
        assert_eq!(pool.active_group(), Some(b.group()));
        assert!(pool.deactivate(b.group()));
        assert_eq!((a.share().get(), b.share().get()), (500, 500));
    }

    /// 0.187.0: a mirror joins its document's group, so the two split the
    /// active share; a mirror joined as a group of its own would be
    /// parked.
    #[test]
    fn a_mirror_in_the_documents_group_splits_the_active_share() {
        let pool = ResidencyPool::new(nz(1000), nz(100));
        let parked = pool.join();
        let document = pool.join();
        pool.activate(document.group());
        let mirror = pool.join_group(document.group());
        assert_eq!(pool.active_members(), 2);
        assert_eq!(
            (
                document.share().get(),
                mirror.share().get(),
                parked.share().get()
            ),
            (375, 375, 250)
        );
        let sum = document.share().get() + mirror.share().get() + parked.share().get();
        assert!(sum <= 1000);
        // The mirror released: the document has the whole active share.
        drop(mirror);
        assert_eq!((document.share().get(), parked.share().get()), (750, 250));
    }

    /// 0.187.0: the active group's last member leaving (its document
    /// closed) leaves no group active, so the rest split equally until
    /// another is activated.
    #[test]
    fn closing_the_active_document_leaves_no_group_active() {
        let pool = ResidencyPool::new(nz(900), nz(100));
        let a = pool.join();
        let b = pool.join();
        let c = pool.join();
        pool.activate(c.group());
        drop(c);
        assert_eq!(pool.active_group(), None);
        assert_eq!((a.share().get(), b.share().get()), (450, 450));
        pool.activate(b.group());
        assert_eq!((a.share().get(), b.share().get()), (225, 675));
        assert_eq!(pool.members(), 2);
    }
}
