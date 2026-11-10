//! The population's genome arena (plan §9.2, FL-026): one bucket per catalog
//! member, each a run of fixed-size slots that live genomes of that
//! topology hold.
//!
//! A genome takes the lowest free slot of its topology's bucket when it
//! enters the population and frees it when it leaves. Each occupant of a
//! slot has a higher generation than the one before, so an
//! [`ArenaSlot`] names one genome for as long as it lives and never a later
//! one. Backends keep each genome's packed parameters by its slot (in
//! shards, `flife_neural::ShardGeometry`), so a genome is packed once while
//! it lives, a newborn overwrites a dead genome's place, and a backend's
//! slabs follow the peak live count of each topology, not the batch.
//!
//! Slots are physical, like population slots: nothing scientific depends
//! on them, checkpoints do not record them, and a restored population
//! assigns them afresh in identity order.
//!
//! A slot whose generation reaches `u32::MAX` is retired when it is freed,
//! so a generation is never reused. Slot indexes are `u64`.
//!
//! [`GenomeArena::reserve`] makes room for a commit's genomes before the
//! commit changes anything, so that the commit's inserts and releases do
//! not allocate.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use flife_core::CoreError;
use flife_core::memory::{bytes_for, capacity_bytes, total};
use flife_neural::ArenaSlot;

use crate::catalog::TopologyId;

/// One slot's current generation, and whether a genome holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlotState {
    generation: u32,
    live: bool,
}

/// One topology's slots.
#[derive(Debug, Default)]
struct Bucket {
    slots: Vec<SlotState>,
    /// Free slots that may be reused, lowest first. Its capacity is kept at
    /// least `slots.len()`, so freeing never allocates.
    free: BinaryHeap<Reverse<u64>>,
    live: u64,
}

/// Every catalog member's bucket of slots.
#[derive(Debug, Default)]
pub struct GenomeArena {
    buckets: Vec<Bucket>,
}

fn grow<T>(vector: &mut Vec<T>, additional: usize) -> Result<(), CoreError> {
    vector
        .try_reserve(additional)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "genome arena",
            bytes: u64::try_from(additional.saturating_mul(size_of::<T>())).unwrap_or(u64::MAX),
        })
}

impl GenomeArena {
    /// An arena with no slots.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            buckets: Vec::new(),
        }
    }

    /// Make room for one genome of each of `topologies`, so that inserting
    /// them, and freeing any slot, does not allocate. Nothing changes that a
    /// reader can see.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses the room.
    pub fn reserve(
        &mut self,
        topologies: impl IntoIterator<Item = TopologyId>,
    ) -> Result<(), CoreError> {
        let mut wanted: Vec<usize> = Vec::new();
        for topology in topologies {
            let index = topology.index();
            if wanted.len() <= index {
                let more = index + 1 - wanted.len();
                grow(&mut wanted, more)?;
                wanted.resize(index + 1, 0);
            }
            if let Some(count) = wanted.get_mut(index) {
                *count = count.saturating_add(1);
            }
        }
        if self.buckets.len() < wanted.len() {
            let more = wanted.len() - self.buckets.len();
            grow(&mut self.buckets, more)?;
            self.buckets.resize_with(wanted.len(), Bucket::default);
        }
        for (bucket, &count) in self.buckets.iter_mut().zip(&wanted) {
            let fresh = count.saturating_sub(bucket.free.len());
            grow(&mut bucket.slots, fresh)?;
            let total = bucket.slots.len().saturating_add(fresh);
            let more = total.saturating_sub(bucket.free.len());
            bucket
                .free
                .try_reserve(more)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "genome arena",
                    bytes: u64::try_from(more.saturating_mul(size_of::<u64>())).unwrap_or(u64::MAX),
                })?;
        }
        Ok(())
    }

    /// Give a genome of `topology` the lowest free slot of its bucket, or a
    /// new one. It allocates only beyond what [`GenomeArena::reserve`] made
    /// room for.
    pub fn insert(&mut self, topology: TopologyId) -> ArenaSlot {
        let index = topology.index();
        if self.buckets.len() <= index {
            self.buckets.resize_with(index + 1, Bucket::default);
        }
        // A member index is a `u8`.
        let member = index as u32;
        let bucket = &mut self.buckets[index];
        bucket.live += 1;
        if let Some(Reverse(slot)) = bucket.free.pop()
            && let Some(state) = usize::try_from(slot)
                .ok()
                .and_then(|at| bucket.slots.get_mut(at))
        {
            // Freed slots at `u32::MAX` are retired, never queued.
            state.generation += 1;
            state.live = true;
            return ArenaSlot::new(member, slot, state.generation);
        }
        let slot = bucket.slots.len() as u64;
        bucket.slots.push(SlotState {
            generation: 1,
            live: true,
        });
        if bucket.free.capacity() < bucket.slots.len() {
            bucket.free.reserve(bucket.slots.len() - bucket.free.len());
        }
        ArenaSlot::new(member, slot, 1)
    }

    /// Free `slot` once its genome has left the population. A slot at the
    /// last generation is retired instead of freed.
    ///
    /// # Errors
    /// [`CoreError::StateConflict`] for a slot that is not live at that
    /// generation: never issued, already freed, or a stale name for a
    /// later occupant. Nothing changes.
    pub fn release(&mut self, slot: ArenaSlot) -> Result<(), CoreError> {
        let state = usize::try_from(slot.topology())
            .ok()
            .and_then(|index| self.buckets.get_mut(index))
            .and_then(|bucket| {
                let at = usize::try_from(slot.slot()).ok()?;
                let state = *bucket.slots.get(at)?;
                Some((bucket, at, state))
            });
        match state {
            Some((bucket, at, state)) if state.live && state.generation == slot.generation() => {
                if let Some(entry) = bucket.slots.get_mut(at) {
                    entry.live = false;
                }
                bucket.live -= 1;
                if state.generation < u32::MAX {
                    bucket.free.push(Reverse(slot.slot()));
                }
                Ok(())
            }
            _ => Err(CoreError::StateConflict {
                operation: "release genome arena slot",
                detail: format!(
                    "slot {} of member {} at generation {} is not live",
                    slot.slot(),
                    slot.topology(),
                    slot.generation()
                ),
            }),
        }
    }

    /// Whether `slot` is live at its generation.
    #[must_use]
    pub fn is_live(&self, slot: ArenaSlot) -> bool {
        usize::try_from(slot.topology())
            .ok()
            .and_then(|index| self.buckets.get(index))
            .and_then(|bucket| bucket.slots.get(usize::try_from(slot.slot()).ok()?))
            .is_some_and(|state| state.live && state.generation == slot.generation())
    }

    /// Live genomes of `topology`.
    #[must_use]
    pub fn live(&self, topology: TopologyId) -> u64 {
        self.buckets
            .get(topology.index())
            .map_or(0, |bucket| bucket.live)
    }

    /// Slots `topology`'s bucket has ever issued: one more than its highest
    /// slot index.
    #[must_use]
    pub fn slots(&self, topology: TopologyId) -> u64 {
        self.buckets
            .get(topology.index())
            .map_or(0, |bucket| bucket.slots.len() as u64)
    }

    /// Bytes its buckets hold: each slot list's and free heap's capacity
    /// times its element size (FL-043).
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        total(
            std::iter::once(capacity_bytes(&self.buckets)).chain(self.buckets.iter().map(
                |bucket| {
                    capacity_bytes(&bucket.slots)
                        .saturating_add(bytes_for::<Reverse<u64>>(bucket.free.capacity()))
                },
            )),
        )
    }

    /// The most bytes an arena of `topologies` buckets holds while at most
    /// `live` genomes are live at once. A bucket issues a new slot only when
    /// none is free, so it never has more than `live` slots; a list that
    /// grows by doubling holds at most twice its length, and three times
    /// while it moves.
    #[must_use]
    pub const fn bound_bytes(topologies: usize, live: usize) -> u64 {
        let per_slot = bytes_for::<SlotState>(live).saturating_add(bytes_for::<Reverse<u64>>(live));
        bytes_for::<Bucket>(topologies)
            .saturating_add(per_slot.saturating_mul(3).saturating_mul(topologies as u64))
    }

    /// Set a slot's generation, to reach the last one without four billion
    /// reuses.
    #[cfg(test)]
    fn force_generation(&mut self, slot: ArenaSlot, generation: u32) -> ArenaSlot {
        let at = usize::try_from(slot.slot()).unwrap_or(usize::MAX);
        if let Some(state) = usize::try_from(slot.topology())
            .ok()
            .and_then(|index| self.buckets.get_mut(index))
            .and_then(|bucket| bucket.slots.get_mut(at))
        {
            state.generation = generation;
        }
        ArenaSlot::new(slot.topology(), slot.slot(), generation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::TopologyCatalog;

    fn members() -> Vec<TopologyId> {
        let catalog = TopologyCatalog::dense_104_36_v1(u64::MAX, &[64, 32]).expect("a catalog");
        catalog.ids().collect()
    }

    #[test]
    fn slots_are_lowest_free_first_per_topology_with_growing_generations() {
        let [a, b, ..] = members()[..] else {
            panic!("two members")
        };
        let mut arena = GenomeArena::new();
        let first: Vec<ArenaSlot> = (0..4).map(|_| arena.insert(a)).collect();
        let other = arena.insert(b);
        assert_eq!(
            first.iter().map(|slot| slot.slot()).collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert!(first.iter().all(|slot| slot.generation() == 1));
        assert_eq!((other.topology(), other.slot()), (1, 0), "its own bucket");
        arena.release(first[2]).expect("live");
        arena.release(first[1]).expect("live");
        let reused = arena.insert(a);
        assert_eq!((reused.slot(), reused.generation()), (1, 2));
        assert_eq!(arena.insert(a).slot(), 2);
        assert_eq!(arena.insert(a).slot(), 4, "none free: a new slot");
        assert_eq!((arena.live(a), arena.slots(a)), (5, 5));
        assert_eq!((arena.live(b), arena.slots(b)), (1, 1));
    }

    #[test]
    fn stale_and_freed_names_are_refused() {
        let a = members()[0];
        let mut arena = GenomeArena::new();
        let old = arena.insert(a);
        arena.release(old).expect("live");
        assert!(!arena.is_live(old));
        assert!(matches!(
            arena.release(old),
            Err(CoreError::StateConflict { .. })
        ));
        let new = arena.insert(a);
        assert_eq!(new.slot(), old.slot());
        assert!(arena.is_live(new) && !arena.is_live(old));
        assert!(
            arena.release(old).is_err(),
            "a stale name does not free its successor"
        );
        assert!(arena.is_live(new));
        assert!(
            arena.release(ArenaSlot::new(0, 9, 1)).is_err(),
            "never issued"
        );
        assert!(arena.release(ArenaSlot::new(4, 0, 1)).is_err(), "no bucket");
        assert_eq!(arena.live(a), 1);
    }

    #[test]
    fn the_last_generation_retires_its_slot() {
        let a = members()[0];
        let mut arena = GenomeArena::new();
        let zero = arena.insert(a);
        let one = arena.insert(a);
        let last = arena.force_generation(zero, u32::MAX);
        arena.release(last).expect("live at its last generation");
        arena.release(one).expect("live");
        let next = arena.insert(a);
        assert_eq!(
            (next.slot(), next.generation()),
            (1, 2),
            "slot 0 is retired, not reused at a wrapped generation"
        );
        assert_eq!(arena.insert(a).slot(), 2);
    }

    #[test]
    fn reserving_makes_inserts_and_releases_allocation_free() {
        let ids = members();
        let mut arena = GenomeArena::new();
        arena
            .reserve(std::iter::repeat_n(ids[3], 64))
            .expect("room");
        let capacity = (
            arena.buckets[3].slots.capacity(),
            arena.buckets[3].free.capacity(),
        );
        assert!(capacity.0 >= 64 && capacity.1 >= 64);
        let slots: Vec<ArenaSlot> = (0..64).map(|_| arena.insert(ids[3])).collect();
        for slot in slots {
            arena.release(slot).expect("live");
        }
        assert_eq!(
            (
                arena.buckets[3].slots.capacity(),
                arena.buckets[3].free.capacity()
            ),
            capacity
        );
        assert_eq!(arena.live(ids[3]), 0);
        assert_eq!(arena.slots(ids[0]), 0);
    }
}
