//! Host allocation accounting for the peak-memory preflight (FL-043; plan
//! §19.3, §19.4). A buffer is charged what its allocation holds, its
//! capacity times its element size, not what it currently uses.
//!
//! [`MemoryAdmission`] is the preflight's decision, which the engine's
//! `World::admit_memory` makes for every host: the command line's runs and
//! renders and the Studio.

use serde::Serialize;

/// The heap the runtime, the session, its records, the store's database and
/// the device driver hold apart from the world, its backend, its observers
/// and its checkpoints: a declared allowance.
pub const RESERVE_BYTES: u64 = 64 << 20;

/// The peak-memory preflight's decision (plan §19.4), reported in a run's
/// start record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MemoryAdmission {
    /// `[execution] maximum_total_bytes`.
    pub budget_bytes: u64,
    /// The fixed logical genome quota in parameters of the run's weight
    /// dtype: `maximum_logical_genome_bytes` over four for `f32`, over two
    /// for `f16`.
    pub logical_parameters: u64,
    /// `maximum_logical_genome_bytes`.
    pub logical_genome_bytes: u64,
    /// The allocated genome quota, `maximum_allocated_genome_bytes`.
    pub genomes: u64,
    /// The population, the sources and their paths.
    pub state: u64,
    /// The fields, the field step's scratch and the sensor pyramid.
    pub field: u64,
    /// The tick's scratch, the batched path's lists included.
    pub scratch: u64,
    /// What a step holds while it runs: births and their staging.
    pub births: u64,
    /// What observers hold (render views, encoding, or the metrics tracker);
    /// zero when none is active.
    pub render: u64,
    /// The checkpoint admission's peak, or zero without a run directory.
    pub checkpoint: u64,
    /// A transplant's package (FL-042), held beside the host's world: its
    /// restored world, the container it was read from, and what grafting
    /// it makes before the host's parts are released. Zero for every other
    /// host.
    pub package: u64,
    /// A restored world's container, held beside the world while it was
    /// built: a phase of its own, before the run's, and not among
    /// [`MemoryAdmission::fixed_bytes`] (see
    /// [`MemoryAdmission::restoring_bytes`]). Zero for a world not restored.
    pub restore: u64,
    /// [`RESERVE_BYTES`].
    pub reserve: u64,
    /// The controller backend's budget: what the other terms leave, or zero
    /// for the CPU reference.
    pub backend: u64,
    /// The larger phase: the run's ([`MemoryAdmission::fixed_bytes`] and the
    /// backend) or the restore's ([`MemoryAdmission::restoring_bytes`]). At
    /// most `budget_bytes`.
    pub peak_bytes: u64,
}

impl MemoryAdmission {
    /// Every term of the run's phase but the backend's.
    #[must_use]
    pub fn fixed_bytes(&self) -> u64 {
        total([
            self.genomes,
            self.state,
            self.field,
            self.scratch,
            self.births,
            self.render,
            self.checkpoint,
            self.package,
            self.reserve,
        ])
    }

    /// The restore's phase: the world as its terms bound it (the genome
    /// quota, state, field and scratch), the container it was read from and
    /// the reserve. Nothing steps, observes or checkpoints while a world is
    /// restored, and no backend is installed yet.
    #[must_use]
    pub fn restoring_bytes(&self) -> u64 {
        total([
            self.genomes,
            self.state,
            self.field,
            self.scratch,
            self.restore,
            self.reserve,
        ])
    }
}

/// The bytes `vector`'s allocation holds: its capacity times its element
/// size, saturating at `u64::MAX`.
#[must_use]
pub fn capacity_bytes<T>(vector: &Vec<T>) -> u64 {
    bytes_for::<T>(vector.capacity())
}

/// The bytes `count` values of `T` take, saturating at `u64::MAX`.
#[must_use]
pub const fn bytes_for<T>(count: usize) -> u64 {
    (count as u64).saturating_mul(size_of::<T>() as u64)
}

/// The sum of `terms`, saturating at `u64::MAX`.
#[must_use]
pub fn total(terms: impl IntoIterator<Item = u64>) -> u64 {
    terms.into_iter().fold(0, u64::saturating_add)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vector_is_charged_its_capacity_not_its_length() {
        let mut vector: Vec<u64> = Vec::with_capacity(10);
        vector.push(1);
        assert_eq!(capacity_bytes(&vector), 80);
        assert_eq!(bytes_for::<[f32; 3]>(5), 60);
        assert_eq!(bytes_for::<u64>(usize::MAX), u64::MAX);
        assert_eq!(total([1, 2, u64::MAX]), u64::MAX);
        assert_eq!(total([1, 2, 3]), 6);
    }

    #[test]
    fn the_restore_is_a_phase_of_its_own() {
        let admission = MemoryAdmission {
            budget_bytes: 0,
            logical_parameters: 0,
            logical_genome_bytes: 0,
            genomes: 1,
            state: 2,
            field: 4,
            scratch: 8,
            births: 16,
            render: 32,
            checkpoint: 64,
            package: 128,
            restore: 256,
            reserve: 512,
            backend: 1_024,
            peak_bytes: 0,
        };
        assert_eq!(
            admission.fixed_bytes(),
            1 + 2 + 4 + 8 + 16 + 32 + 64 + 128 + 512
        );
        assert_eq!(admission.restoring_bytes(), 1 + 2 + 4 + 8 + 256 + 512);
    }
}
