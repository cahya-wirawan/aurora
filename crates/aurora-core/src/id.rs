//! A generic, type-tagged ID shared by every crate that needs one (layers,
//! render-graph nodes, history entries, ...) without each crate inventing
//! its own newtype.
//!
//! Process-local and monotonic, not globally unique — collaboration
//! (FR-022) is deferred past v1.0, so there is no cross-process identity
//! requirement to design for yet.

use std::fmt;
use std::marker::PhantomData;

/// An ID tagged with the type it identifies, e.g. `Id<Layer>` vs.
/// `Id<Node>` — distinct types the compiler will not let you mix up, even
/// though both are a `u64` underneath.
///
/// `Clone`/`Copy`/`PartialEq`/`Eq`/`Hash`/`Debug` are hand-implemented
/// rather than derived: `#[derive]` on a type generic over `T` generates a
/// `T: Trait` bound even when `T` is never actually stored (only named via
/// `PhantomData`), which would wrongly force every marker type (e.g. a
/// future `Layer`) to implement traits it has no reason to.
pub struct Id<T> {
    value: u64,
    marker: PhantomData<fn() -> T>,
}

impl<T> Id<T> {
    /// Reconstructs an ID from a raw value, e.g. when loading a saved
    /// document. Does not coordinate with any live [`IdGenerator`] — the
    /// caller is responsible for not creating collisions when mixing
    /// loaded and freshly generated IDs.
    #[must_use]
    pub const fn from_raw(value: u64) -> Self {
        Self {
            value,
            marker: PhantomData,
        }
    }

    #[must_use]
    pub const fn to_raw(self) -> u64 {
        self.value
    }
}

impl<T> Clone for Id<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for Id<T> {}

impl<T> PartialEq for Id<T> {
    fn eq(&self, other: &Self) -> bool {
        self.value == other.value
    }
}

impl<T> Eq for Id<T> {}

impl<T> std::hash::Hash for Id<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.value.hash(state);
    }
}

impl<T> fmt::Debug for Id<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Id").field(&self.value).finish()
    }
}

/// Hand-implemented, not derived, for the same reason as the traits
/// above: `#[derive(Serialize)]` on a type generic over `T` would add a
/// `T: Serialize` bound even though `T` is never actually serialized
/// (only named via `PhantomData`) — wrongly forcing every marker type to
/// implement a trait it has no reason to. Serializes as the plain `u64`
/// it is underneath; the type tag exists only at compile time, so there
/// is nothing else to carry across a serialization boundary. First real
/// consumer: `aurora_doc::History::save_journal` (ADR 0009).
impl<T> serde::Serialize for Id<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_u64(self.value)
    }
}

/// Hand-written for the same reason `Serialize` above is.
impl<'de, T> serde::Deserialize<'de> for Id<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = u64::deserialize(deserializer)?;
        Ok(Self::from_raw(value))
    }
}

/// Generates process-local, monotonically increasing [`Id`]s for one
/// marker type `T`.
///
/// **Not thread-safe** — this is a plain counter, deliberately: nothing in
/// the project yet generates IDs from more than one thread. Wrap one in a
/// `Mutex` if and when concurrent generation is actually needed, rather
/// than paying for atomics no caller uses today.
pub struct IdGenerator<T> {
    next: u64,
    marker: PhantomData<fn() -> T>,
}

impl<T> Default for IdGenerator<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T> IdGenerator<T> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next: 0,
            marker: PhantomData,
        }
    }

    /// Returns the next unused ID. Saturates rather than wraps at
    /// `u64::MAX` — realistically unreachable in one editing session, but
    /// wrapping back to a reused ID would be a silent correctness bug, and
    /// this crate denies `panic`.
    pub fn next_id(&mut self) -> Id<T> {
        let id = Id::from_raw(self.next);
        self.next = self.next.saturating_add(1);
        id
    }
}

impl<T> fmt::Debug for IdGenerator<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IdGenerator")
            .field("next", &self.next)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::{Id, IdGenerator};

    struct Layer;
    struct Node;

    #[test]
    fn generator_increments() {
        let mut generator: IdGenerator<Layer> = IdGenerator::new();
        let a = generator.next_id();
        let b = generator.next_id();
        assert_ne!(a, b);
        assert_eq!(a.to_raw(), 0);
        assert_eq!(b.to_raw(), 1);
    }

    #[test]
    fn from_raw_round_trips() {
        let id: Id<Layer> = Id::from_raw(42);
        assert_eq!(id.to_raw(), 42);
    }

    #[test]
    fn generator_saturates_instead_of_wrapping() {
        let mut generator: IdGenerator<Node> = IdGenerator::new();
        generator.next = u64::MAX - 1;
        let a = generator.next_id();
        let b = generator.next_id();
        assert_eq!(a.to_raw(), u64::MAX - 1);
        assert_eq!(b.to_raw(), u64::MAX);
        let c = generator.next_id();
        assert_eq!(c.to_raw(), u64::MAX, "must saturate, not wrap to 0");
    }
}
