//! Genome arena slots and shard geometry (plan §9.2, FL-026).
//!
//! Every live genome has an [`ArenaSlot`]: its catalog member (the topology
//! bucket), a slot index in that bucket, and the generation of the slot's
//! current occupant. The population assigns slots (`flife_genome`'s
//! `GenomeArena`), lowest free slot first. A slot's generation grows each
//! time it is reused, so while a backend serves one world a slot and its
//! generation name one immutable parameter set. A backend may keep a packed
//! copy of it by them, in slabs it lays out with a [`ShardGeometry`] chosen
//! for its own buffer limits.
//!
//! Every count, offset and length here is a checked `u64` (§9.2). At four
//! billion parameters a byte offset already exceeds 32 bits, and slot
//! indexes and element offsets can too. Converting a [`Placement`] to a
//! device's narrower words is the backend's own checked step.

use flife_core::CoreError;

/// A live genome's place in its topology's arena: a name, not memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ArenaSlot {
    topology: u32,
    slot: u64,
    generation: u32,
}

impl ArenaSlot {
    /// Slot `slot` of catalog member `topology`, held by its `generation`th
    /// occupant (the first is 1).
    #[must_use]
    pub const fn new(topology: u32, slot: u64, generation: u32) -> Self {
        Self {
            topology,
            slot,
            generation,
        }
    }

    /// The catalog member, in catalog order: the topology bucket.
    #[must_use]
    pub const fn topology(self) -> u32 {
        self.topology
    }

    /// The slot's index in its bucket.
    #[must_use]
    pub const fn slot(self) -> u64 {
        self.slot
    }

    /// Which occupant of the slot this is.
    #[must_use]
    pub const fn generation(self) -> u32 {
        self.generation
    }
}

/// Where a slot's parameters sit in a backend's slabs: shard `shard`,
/// elements `offset..offset + length` of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// The shard.
    pub shard: u64,
    /// The slot's index within its shard.
    pub slot_in_shard: u64,
    /// The first element, counted from the shard's start.
    pub offset: u64,
    /// The elements: the topology's parameter count.
    pub length: u64,
}

/// Fixed-size slots of one topology's parameters, `slots_per_shard` of them
/// to a shard. Slot `s` is in shard `s / slots_per_shard`, at element
/// `(s mod slots_per_shard) · parameters` of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShardGeometry {
    parameters: u64,
    slots_per_shard: u64,
    shard_elements: u64,
}

impl ShardGeometry {
    /// Shards of `slots_per_shard` slots of `parameters` elements each.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for zero parameters or slots;
    /// [`CoreError::Overflow`] for a shard of more than `u64::MAX` elements.
    pub fn new(parameters: u64, slots_per_shard: u64) -> Result<Self, CoreError> {
        if parameters == 0 || slots_per_shard == 0 {
            return Err(CoreError::NumericalInvalid {
                quantity: "arena shard geometry",
                detail: format!("{slots_per_shard} slots of {parameters} parameters hold nothing"),
            });
        }
        let shard_elements =
            parameters
                .checked_mul(slots_per_shard)
                .ok_or(CoreError::Overflow {
                    quantity: "arena shard elements",
                })?;
        Ok(Self {
            parameters,
            slots_per_shard,
            shard_elements,
        })
    }

    /// As many slots of `parameters` as fit in a shard of at most
    /// `maximum_elements`, and at most `maximum_slots`.
    ///
    /// # Errors
    /// [`CoreError::BudgetExhausted`] when not one slot fits;
    /// [`CoreError::NumericalInvalid`] for zero parameters or slots.
    pub fn fitting(
        parameters: u64,
        maximum_elements: u64,
        maximum_slots: u64,
    ) -> Result<Self, CoreError> {
        let fit = maximum_elements
            .checked_div(parameters)
            .unwrap_or(0)
            .min(maximum_slots);
        if fit == 0 && parameters > maximum_elements {
            return Err(CoreError::BudgetExhausted {
                budget: "arena shard elements",
                required: parameters,
                available: maximum_elements,
            });
        }
        Self::new(parameters, fit)
    }

    /// Elements per slot: the topology's parameter count.
    #[must_use]
    pub const fn parameters(&self) -> u64 {
        self.parameters
    }

    /// Slots per shard.
    #[must_use]
    pub const fn slots_per_shard(&self) -> u64 {
        self.slots_per_shard
    }

    /// Elements per shard.
    #[must_use]
    pub const fn shard_elements(&self) -> u64 {
        self.shard_elements
    }

    /// Where slot `slot` sits. It never overflows: the offset is below
    /// [`ShardGeometry::shard_elements`], which construction checked.
    #[must_use]
    pub const fn locate(&self, slot: u64) -> Placement {
        let slot_in_shard = slot % self.slots_per_shard;
        Placement {
            shard: slot / self.slots_per_shard,
            slot_in_shard,
            offset: slot_in_shard * self.parameters,
            length: self.parameters,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slots_far_past_32_bits_locate_exactly() {
        // The preset's 9,988-parameter topology (104-64-32-36), in shards of 1,024.
        let geometry = ShardGeometry::new(9_988, 1_024).expect("a geometry");
        for slot in [
            0,
            1_023,
            1_024,
            u64::from(u32::MAX),
            u64::from(u32::MAX) + 1,
            1 << 40,
            u64::MAX,
        ] {
            let at = geometry.locate(slot);
            assert_eq!(at.shard, slot / 1_024, "slot {slot}");
            assert_eq!(at.slot_in_shard, slot % 1_024, "slot {slot}");
            assert_eq!(at.offset, (slot % 1_024) * 9_988, "slot {slot}");
            assert_eq!(at.length, 9_988);
            assert!(at.offset + at.length <= geometry.shard_elements());
            // The slot's elements, counted from the first shard, past 2^32.
            let global = u128::from(at.shard) * u128::from(geometry.shard_elements())
                + u128::from(at.offset);
            assert_eq!(global, u128::from(slot) * 9_988, "slot {slot}");
        }
    }

    #[test]
    fn a_shard_can_hold_more_than_32_bits_of_elements() {
        // 2^20 parameters, 2^13 to a shard: 2^33 elements, the last slot's
        // offset past every 32-bit word.
        let geometry = ShardGeometry::new(1 << 20, 1 << 13).expect("a geometry");
        assert_eq!(geometry.shard_elements(), 1 << 33);
        let last = geometry.locate((1 << 13) - 1);
        assert_eq!(last.shard, 0);
        assert_eq!(last.offset, (1 << 33) - (1 << 20));
        assert!(last.offset > u64::from(u32::MAX));
        assert_eq!(geometry.locate(1 << 13).shard, 1);
    }

    #[test]
    fn geometry_refuses_what_it_cannot_hold() {
        assert!(matches!(
            ShardGeometry::new(1 << 33, 1 << 31),
            Err(CoreError::Overflow { .. })
        ));
        assert!(matches!(
            ShardGeometry::new(0, 1),
            Err(CoreError::NumericalInvalid { .. })
        ));
        assert!(matches!(
            ShardGeometry::new(1, 0),
            Err(CoreError::NumericalInvalid { .. })
        ));
        assert!(matches!(
            ShardGeometry::fitting(9_988, 9_987, 1_024),
            Err(CoreError::BudgetExhausted {
                required: 9_988,
                available: 9_987,
                ..
            })
        ));
    }

    #[test]
    fn fitting_takes_the_tighter_bound() {
        let by_elements = ShardGeometry::fitting(9_988, 10 * 9_988 + 7, 1_024).expect("fits");
        assert_eq!(by_elements.slots_per_shard(), 10);
        let by_slots = ShardGeometry::fitting(9_988, u64::MAX, 1_024).expect("fits");
        assert_eq!(by_slots.slots_per_shard(), 1_024);
        // A 32-bit device word bounds the shard, not the slot count.
        let device = ShardGeometry::fitting(1 << 20, u64::from(u32::MAX), u64::MAX).expect("fits");
        assert_eq!(device.slots_per_shard(), (1 << 12) - 1);
        assert!(device.shard_elements() <= u64::from(u32::MAX));
    }
}
