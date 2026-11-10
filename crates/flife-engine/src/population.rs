//! Live organism storage (plan §7.7, §8.3, §9.1, INV-01, INV-06, INV-07,
//! Appendix C.7).
//!
//! [`Population`] keeps the hot state of every live organism as a structure
//! of arrays: one column per field, indexed by storage slot. Slots are
//! physical. Everything scientific uses stable identity: RNG addresses,
//! lineage, commands, inspection, events and iteration order. It goes
//! through an ordered identity-to-slot index that every change updates in
//! the same transaction.
//!
//! Two arrangements are provided, and a world must not be able to tell them
//! apart (C.7):
//! - [`Arrangement::Compact`]: removing organisms closes the gaps and keeps
//!   the survivors in their relative slot order; newborns are appended.
//! - [`Arrangement::FreeList`]: a removed organism's slot is freed with its
//!   stale contents left in place, and a newborn takes the lowest free slot.
//!
//! Identity is never a slot (INV-07). The store admits identities in
//! strictly increasing order only. So a dead identity, or any identity at or
//! below the highest ever admitted, can never be admitted again. A reused
//! slot never carries the old organism's identity or randomness (addressed
//! by identity), and never its controller state: admission writes every
//! column, and a newborn's held output is invalid and unreadable.
//!
//! [`Population::commit`] applies one tick's deaths and births together
//! (INV-06). It checks everything first, and only then changes anything:
//! - each death names a distinct live organism, with that organism's genome,
//!   lineage and generation;
//! - each birth is a new identity, in ascending order, with its own genome;
//! - the result fits the capacity.
//!
//! [`Population::commit_tick`] commits a whole ordered tick (FL-013): every
//! live organism either survives, with its new state, or dies, and the
//! dying genomes' quota charges are returned in the same transaction.
//!
//! A refused commit leaves the population exactly as it was. Columns are
//! reserved to capacity up front, so applying a checked commit cannot fail
//! or allocate. Every write is checked as well: a stored organism has
//! finite state and strictly positive energy (INV-01), and a nonfinite one
//! is refused by identity rather than stored.
//!
//! [`Population::digest`] hashes the canonical scientific state in identity
//! order, excluding slots, for comparing arrangements and runs.
//!
//! The population owns the genome arena (FL-026, plan §9.2): each live
//! genome holds an arena slot of its topology, taken when it is admitted or
//! born and freed when it dies, by which backends keep its packed
//! parameters ([`Population::arena_slot`]). Arena slots are physical too.
//! An organism keeps its arena slot when the compact arrangement moves it.

use std::collections::{BTreeMap, BTreeSet};

use flife_core::clock::Tick;
use flife_core::ids::{Generation, GenomeId, LineageId, OrganismId};
use flife_core::memory::{capacity_bytes, total};
use flife_core::{CoreError, Sha256Digest, ValidatedConfig};
use flife_genome::{Genome, GenomeArena, GenomeQuota, MutationRecord};
use flife_neural::{ACTION_WIDTH, ArenaSlot};

use crate::lifecycle::Death;
use crate::reproduction::{Birth, Newborn};

/// The canonical digest's schema line.
pub const POPULATION_DIGEST_SCHEMA: &str = "frankenlife.population-state.v1";

/// A controller's held raw outputs (§6.6). An invalid output cannot be read,
/// so a stale one can never be decoded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeldOutput {
    evaluated_at: Option<Tick>,
    outputs: [f32; ACTION_WIDTH],
}

impl HeldOutput {
    /// No held output: the next advance forces an evaluation.
    pub const INVALID: Self = Self {
        evaluated_at: None,
        outputs: [0.0; ACTION_WIDTH],
    };

    /// `outputs`, evaluated at `tick`.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite output.
    pub fn evaluated(tick: Tick, outputs: [f32; ACTION_WIDTH]) -> Result<Self, CoreError> {
        if let Some(index) = outputs.iter().position(|value| !value.is_finite()) {
            return Err(CoreError::NumericalInvalid {
                quantity: "held controller output",
                detail: format!("output {index} is {}", outputs[index]),
            });
        }
        Ok(Self {
            evaluated_at: Some(tick),
            outputs,
        })
    }

    /// Whether an output is held.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.evaluated_at.is_some()
    }

    /// The tick it was evaluated at.
    #[must_use]
    pub const fn evaluated_at(&self) -> Option<Tick> {
        self.evaluated_at
    }

    /// The held outputs, if any.
    #[must_use]
    pub const fn outputs(&self) -> Option<&[f32; ACTION_WIDTH]> {
        if self.evaluated_at.is_some() {
            Some(&self.outputs)
        } else {
            None
        }
    }
}

/// One live organism's state, as a row.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Organism {
    /// Its stable identity.
    pub id: OrganismId,
    /// Its genome's identity.
    pub genome: GenomeId,
    /// Its parent; none for a founder.
    pub parent: Option<OrganismId>,
    /// Its founder lineage.
    pub lineage: LineageId,
    /// Its generation.
    pub generation: Generation,
    /// The first committed tick it exists in.
    pub birth_tick: Tick,
    /// Its wrapped position.
    pub position: [f64; 2],
    /// Its velocity.
    pub velocity: [f64; 2],
    /// Its energy, strictly positive.
    pub energy: f64,
    /// `(E_next − E_previous)/Δt` from its last committed tick (§8.2).
    pub energy_change_rate: f64,
    /// Its held controller output.
    pub held: HeldOutput,
    /// Its stable controller phase.
    pub phase: u32,
    /// The first attempt tick at which it may reproduce.
    pub cooldown_deadline: Tick,
}

impl Organism {
    /// The row a staged newborn commits as: zero velocity and history, and
    /// no held output.
    #[must_use]
    pub const fn newborn(newborn: &Newborn) -> Self {
        Self {
            id: newborn.id,
            genome: newborn.genome,
            parent: Some(newborn.parent),
            lineage: newborn.lineage,
            generation: newborn.generation,
            birth_tick: newborn.birth_tick,
            position: newborn.position,
            velocity: newborn.velocity,
            energy: newborn.energy,
            energy_change_rate: newborn.energy_change_rate,
            held: HeldOutput::INVALID,
            phase: newborn.phase,
            cooldown_deadline: newborn.cooldown_deadline,
        }
    }

    /// INV-01: finite state and strictly positive energy.
    fn check(&self) -> Result<(), CoreError> {
        let finite = self
            .position
            .iter()
            .chain(&self.velocity)
            .chain([&self.energy_change_rate])
            .all(|value| value.is_finite());
        if !finite {
            return Err(CoreError::NumericalInvalid {
                quantity: "organism state",
                detail: format!(
                    "{}: position {:?}, velocity {:?}, energy change rate {}",
                    self.id, self.position, self.velocity, self.energy_change_rate
                ),
            });
        }
        if !(self.energy.is_finite() && self.energy > 0.0) {
            return Err(CoreError::NumericalInvalid {
                quantity: "organism energy",
                detail: format!(
                    "{}: {} is not finite and strictly positive",
                    self.id, self.energy
                ),
            });
        }
        Ok(())
    }

    /// The fields fixed for an organism's life.
    fn lineage_key(
        &self,
    ) -> (
        GenomeId,
        Option<OrganismId>,
        LineageId,
        Generation,
        Tick,
        u32,
    ) {
        (
            self.genome,
            self.parent,
            self.lineage,
            self.generation,
            self.birth_tick,
            self.phase,
        )
    }
}

/// An organism entering the population, with its sealed genome.
#[derive(Debug)]
pub struct Entrant {
    /// Its row.
    pub organism: Organism,
    /// Its genome; its identity must be the row's.
    pub genome: Genome,
}

impl Entrant {
    /// A staged birth's newborn, and how its genome was made.
    #[must_use]
    pub fn born(birth: Birth) -> (Self, MutationRecord) {
        let Birth {
            newborn,
            genome,
            record,
            ..
        } = birth;
        (
            Self {
                organism: Organism::newborn(&newborn),
                genome: *genome,
            },
            record,
        )
    }
}

/// A dead organism removed from the population: the death, its last
/// committed row and its genome. The caller retires the genome's quota
/// charge (`flife_genome::GenomeQuota::retire`); readers holding its shared
/// parameters keep them.
#[derive(Debug)]
pub struct Collected {
    /// The death.
    pub death: Death,
    /// Its last committed state.
    pub organism: Organism,
    /// Its genome.
    pub genome: Genome,
}

/// How slots are reused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Arrangement {
    /// Close gaps, keeping survivors' relative order; append newborns.
    Compact,
    /// Free a dead organism's slot in place; fill the lowest free slot first.
    FreeList,
}

/// The live organisms (§9.1).
#[derive(Debug)]
pub struct Population {
    arrangement: Arrangement,
    capacity: usize,
    occupants: Vec<Option<OrganismId>>,
    genome_ids: Vec<GenomeId>,
    parents: Vec<Option<OrganismId>>,
    lineages: Vec<LineageId>,
    generations: Vec<Generation>,
    birth_ticks: Vec<Tick>,
    positions: Vec<[f64; 2]>,
    velocities: Vec<[f64; 2]>,
    energies: Vec<f64>,
    energy_change_rates: Vec<f64>,
    held: Vec<HeldOutput>,
    phases: Vec<u32>,
    cooldown_deadlines: Vec<Tick>,
    genomes: Vec<Option<Genome>>,
    /// Each slot's genome's arena slot, beside `genomes`.
    arena_slots: Vec<Option<ArenaSlot>>,
    arena: GenomeArena,
    index: BTreeMap<OrganismId, usize>,
    free: BTreeSet<usize>,
    latest: Option<OrganismId>,
}

/// Every birth in `births` is at `tick`.
fn check_birth_ticks(tick: Tick, births: &[Entrant]) -> Result<(), CoreError> {
    for entrant in births {
        if entrant.organism.birth_tick != tick {
            return Err(CoreError::StateConflict {
                operation: "commit births",
                detail: format!(
                    "{} is born at tick {}, not {}",
                    entrant.organism.id,
                    entrant.organism.birth_tick.get(),
                    tick.get()
                ),
            });
        }
    }
    Ok(())
}

/// The host bytes each live organism is charged beyond its columns and its
/// genome's parameters (FL-043, plan §19.4). An ordered map's or set's
/// nodes are at least half full, so an entry takes at most twice its key and
/// value, with its share of the links counted as two more words: the
/// identity index's entry and the free list's. A genome's shared parameters
/// carry two reference counts and a vector header beside the values the
/// genome quota charges.
pub const PER_ORGANISM_BYTES: u64 = {
    const WORD: u64 = size_of::<usize>() as u64;
    let index = 2 * (size_of::<OrganismId>() as u64 + WORD) + 2 * WORD;
    let free = 2 * WORD + 2 * WORD;
    let header = 2 * WORD + size_of::<Vec<f32>>() as u64;
    index + free + header
};

fn reserve<T>(capacity: usize) -> Result<Vec<T>, CoreError> {
    let mut column = Vec::new();
    column
        .try_reserve_exact(capacity)
        .map_err(|_| CoreError::AllocationFailed {
            buffer: "population column",
            bytes: u64::try_from(capacity.saturating_mul(size_of::<T>())).unwrap_or(u64::MAX),
        })?;
    Ok(column)
}

impl Population {
    /// An empty population of at most `capacity` organisms, its columns
    /// reserved up front.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses a column.
    pub fn new(arrangement: Arrangement, capacity: usize) -> Result<Self, CoreError> {
        Ok(Self {
            arrangement,
            capacity,
            occupants: reserve(capacity)?,
            genome_ids: reserve(capacity)?,
            parents: reserve(capacity)?,
            lineages: reserve(capacity)?,
            generations: reserve(capacity)?,
            birth_ticks: reserve(capacity)?,
            positions: reserve(capacity)?,
            velocities: reserve(capacity)?,
            energies: reserve(capacity)?,
            energy_change_rates: reserve(capacity)?,
            held: reserve(capacity)?,
            phases: reserve(capacity)?,
            cooldown_deadlines: reserve(capacity)?,
            genomes: reserve(capacity)?,
            arena_slots: reserve(capacity)?,
            arena: GenomeArena::new(),
            index: BTreeMap::new(),
            free: BTreeSet::new(),
            latest: None,
        })
    }

    /// An empty population at the configuration's `population.maximum_count`.
    ///
    /// # Errors
    /// As [`Population::new`].
    pub fn from_config(
        arrangement: Arrangement,
        config: &ValidatedConfig,
    ) -> Result<Self, CoreError> {
        let maximum = config.config().population.maximum_count;
        let capacity = usize::try_from(maximum).map_err(|_| CoreError::Overflow {
            quantity: "population.maximum_count",
        })?;
        Self::new(arrangement, capacity)
    }

    /// The arrangement.
    #[must_use]
    pub const fn arrangement(&self) -> Arrangement {
        self.arrangement
    }

    /// The most organisms it can hold.
    #[must_use]
    pub const fn capacity(&self) -> usize {
        self.capacity
    }

    /// Bytes its columns hold, reserved for its capacity (FL-043).
    fn column_bytes(&self) -> u64 {
        total([
            capacity_bytes(&self.occupants),
            capacity_bytes(&self.genome_ids),
            capacity_bytes(&self.parents),
            capacity_bytes(&self.lineages),
            capacity_bytes(&self.generations),
            capacity_bytes(&self.birth_ticks),
            capacity_bytes(&self.positions),
            capacity_bytes(&self.velocities),
            capacity_bytes(&self.energies),
            capacity_bytes(&self.energy_change_rates),
            capacity_bytes(&self.held),
            capacity_bytes(&self.phases),
            capacity_bytes(&self.cooldown_deadlines),
            capacity_bytes(&self.genomes),
            capacity_bytes(&self.arena_slots),
        ])
    }

    /// Host bytes it holds now apart from its genomes' parameters (FL-043):
    /// its columns, its genome arena, and [`PER_ORGANISM_BYTES`] for each
    /// live organism.
    #[must_use]
    pub fn allocated_bytes(&self) -> u64 {
        total([
            self.column_bytes(),
            self.arena.allocated_bytes(),
            (self.len() as u64).saturating_mul(PER_ORGANISM_BYTES),
        ])
    }

    /// The most host bytes it can hold apart from its genomes' parameters,
    /// with genomes of at most `topologies` catalog members: its columns,
    /// its arena at [`GenomeArena::bound_bytes`], and
    /// [`PER_ORGANISM_BYTES`] for every organism it can hold (FL-043).
    #[must_use]
    pub fn bound_bytes(&self, topologies: usize) -> u64 {
        total([
            self.column_bytes(),
            GenomeArena::bound_bytes(topologies, self.capacity),
            (self.capacity as u64).saturating_mul(PER_ORGANISM_BYTES),
        ])
    }

    /// The number of live organisms.
    #[must_use]
    pub fn len(&self) -> usize {
        self.index.len()
    }

    /// Whether no organism is alive.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// The highest identity ever admitted. No identity at or below it can be
    /// admitted again.
    #[must_use]
    pub const fn latest(&self) -> Option<OrganismId> {
        self.latest
    }

    /// Whether `id` is alive.
    #[must_use]
    pub fn contains(&self, id: OrganismId) -> bool {
        self.index.contains_key(&id)
    }

    /// The slot `id` occupies. Slots are physical; nothing scientific may
    /// depend on them.
    #[must_use]
    pub fn slot(&self, id: OrganismId) -> Option<usize> {
        self.index.get(&id).copied()
    }

    /// The occupant of every physical slot, `None` for a free one.
    #[must_use]
    pub fn slots(&self) -> &[Option<OrganismId>] {
        &self.occupants
    }

    /// The live identities, ascending.
    pub fn ids(&self) -> impl Iterator<Item = OrganismId> + '_ {
        self.index.keys().copied()
    }

    /// `id`'s state.
    #[must_use]
    pub fn get(&self, id: OrganismId) -> Option<Organism> {
        self.index.get(&id).map(|&slot| self.read(slot))
    }

    /// Every live organism, in identity order.
    pub fn iter(&self) -> impl Iterator<Item = Organism> + '_ {
        self.index.values().map(|&slot| self.read(slot))
    }

    /// `id`'s genome's arena slot.
    #[must_use]
    pub fn arena_slot(&self, id: OrganismId) -> Option<ArenaSlot> {
        self.index
            .get(&id)
            .and_then(|&slot| self.arena_slots.get(slot).copied().flatten())
    }

    /// The genome arena: which slots live genomes hold.
    #[must_use]
    pub const fn arena(&self) -> &GenomeArena {
        &self.arena
    }

    /// `id`'s genome.
    #[must_use]
    pub fn genome(&self, id: OrganismId) -> Option<&Genome> {
        self.index
            .get(&id)
            .and_then(|&slot| self.genomes.get(slot))
            .and_then(Option::as_ref)
    }

    /// Admit organisms outside a tick, such as founders, in order. On
    /// success `entrants` is drained.
    ///
    /// # Errors
    /// As [`Population::commit`]'s births. Nothing changes on error, and
    /// `entrants` keeps every genome so the caller can return its charge.
    pub fn admit(&mut self, entrants: &mut Vec<Entrant>) -> Result<(), CoreError> {
        self.check_entrants(entrants, self.len())?;
        self.reserve_arena(entrants)?;
        for entrant in entrants.drain(..) {
            self.insert(entrant);
        }
        Ok(())
    }

    /// Replace a live organism's changing state: position, velocity, energy,
    /// energy change rate, held output and cooldown deadline.
    ///
    /// # Errors
    /// - [`CoreError::StateConflict`] when `organism` is not alive, or its
    ///   genome, parent, lineage, generation, birth tick or phase differs
    ///   from the stored organism's.
    /// - [`CoreError::NumericalInvalid`] for nonfinite state or energy that
    ///   is not strictly positive (INV-01), naming the organism.
    ///
    /// Nothing changes on error.
    pub fn update(&mut self, organism: &Organism) -> Result<(), CoreError> {
        let Some(&slot) = self.index.get(&organism.id) else {
            return Err(CoreError::StateConflict {
                operation: "update organism",
                detail: format!("{} is not alive", organism.id),
            });
        };
        if self.read(slot).lineage_key() != organism.lineage_key() {
            return Err(CoreError::StateConflict {
                operation: "update organism",
                detail: format!(
                    "{} cannot change its genome, ancestry, birth tick or phase",
                    organism.id
                ),
            });
        }
        organism.check()?;
        self.write(slot, organism);
        Ok(())
    }

    /// Commit one tick's deaths and births together (INV-06, INV-07).
    /// Deaths are removed in identity order, then births are admitted in
    /// identity order. Returns the collected deaths in identity order; on
    /// success `births` is drained.
    ///
    /// # Errors
    /// - [`CoreError::StateConflict`] for a death at another tick, of an
    ///   organism that is not alive, twice in one commit, or whose genome,
    ///   lineage or generation is not the stored organism's; and for a birth
    ///   at another tick, with an identity at or below one already admitted
    ///   or out of order, or with another identity's genome.
    /// - [`CoreError::BudgetExhausted`] when the result would exceed the
    ///   capacity.
    /// - [`CoreError::NumericalInvalid`] for a newborn that fails INV-01.
    ///
    /// Nothing changes on error, and `births` keeps every genome so the
    /// caller can return its charge.
    pub fn commit(
        &mut self,
        tick: Tick,
        deaths: &[Death],
        births: &mut Vec<Entrant>,
    ) -> Result<Vec<Collected>, CoreError> {
        let dying = self.check_deaths(tick, deaths)?;
        check_birth_ticks(tick, births)?;
        self.check_entrants(births, self.len() - dying.len())?;
        self.reserve_arena(births)?;

        let collected = self.remove(dying);
        for entrant in births.drain(..) {
            self.insert(entrant);
        }
        Ok(collected)
    }

    /// Check a whole tick's commit (§8.1 phase 10) without changing
    /// anything: every live organism either survives, with its new state in
    /// `survivors`, or dies in `deaths`; the births are as for
    /// [`Population::commit`]; and `quota` holds the charges of the dying
    /// organisms' genomes.
    ///
    /// # Errors
    /// As [`Population::commit_tick`].
    pub fn check_tick(
        &self,
        tick: Tick,
        survivors: &[Organism],
        deaths: &[Death],
        births: &[Entrant],
        quota: &GenomeQuota,
    ) -> Result<(), CoreError> {
        self.check_whole_tick(tick, survivors, deaths, births, quota)
            .map(|_| ())
    }

    /// Commit one whole tick together: the survivors' new state, then the
    /// deaths, whose genome charges `quota` takes back, then the births
    /// (INV-01, INV-06, INV-07). On success `births` is drained.
    ///
    /// # Errors
    /// - [`CoreError::StateConflict`] for survivors that are not in strictly
    ///   ascending identity order, not alive, also dying, or changing a
    ///   field fixed for life, and for a live organism that neither survives
    ///   nor dies; for a quota that does not hold the dying genomes'
    ///   charges; and as [`Population::commit`] for the deaths and births.
    /// - [`CoreError::NumericalInvalid`] for a survivor or newborn that fails
    ///   INV-01, naming it.
    /// - [`CoreError::BudgetExhausted`] when the result would exceed the
    ///   capacity.
    ///
    /// Nothing changes on error, neither here nor in `quota`, and `births`
    /// keeps every genome.
    pub fn commit_tick(
        &mut self,
        tick: Tick,
        survivors: &[Organism],
        deaths: &[Death],
        births: &mut Vec<Entrant>,
        quota: &mut GenomeQuota,
    ) -> Result<(), CoreError> {
        let dying = self.check_whole_tick(tick, survivors, deaths, births, quota)?;
        self.reserve_arena(births)?;
        for organism in survivors {
            if let Some(&slot) = self.index.get(&organism.id) {
                self.write(slot, organism);
            }
        }
        for collected in self.remove(dying) {
            // Checked above: the quota holds every one of these charges.
            quota.retire(collected.genome)?;
        }
        for entrant in births.drain(..) {
            self.insert(entrant);
        }
        Ok(())
    }

    fn check_whole_tick(
        &self,
        tick: Tick,
        survivors: &[Organism],
        deaths: &[Death],
        births: &[Entrant],
        quota: &GenomeQuota,
    ) -> Result<BTreeMap<OrganismId, Death>, CoreError> {
        let dying = self.check_deaths(tick, deaths)?;
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "commit tick",
            detail,
        };
        let mut previous: Option<OrganismId> = None;
        for organism in survivors {
            if previous.is_some_and(|previous| organism.id <= previous) {
                return Err(conflict(format!(
                    "{} is not after the survivor before it; survivors are given once, in ascending identity order",
                    organism.id
                )));
            }
            previous = Some(organism.id);
            if dying.contains_key(&organism.id) {
                return Err(conflict(format!("{} both survives and dies", organism.id)));
            }
            let Some(&slot) = self.index.get(&organism.id) else {
                return Err(conflict(format!("{} is not alive", organism.id)));
            };
            if self.read(slot).lineage_key() != organism.lineage_key() {
                return Err(conflict(format!(
                    "{} cannot change its genome, ancestry, birth tick or phase",
                    organism.id
                )));
            }
            organism.check()?;
        }
        if survivors.len() + dying.len() != self.len() {
            return Err(conflict(format!(
                "{} survivors and {} deaths do not account for {} live organisms",
                survivors.len(),
                dying.len(),
                self.len()
            )));
        }
        check_birth_ticks(tick, births)?;
        self.check_entrants(births, survivors.len())?;
        let (mut logical, mut allocated) = (0_u64, 0_u64);
        for id in dying.keys() {
            let genome = self
                .index
                .get(id)
                .and_then(|&slot| self.genomes.get(slot))
                .and_then(Option::as_ref)
                .ok_or_else(|| conflict(format!("{id} has no genome")))?;
            logical = logical
                .checked_add(genome.logical_bytes())
                .ok_or(CoreError::Overflow {
                    quantity: "retired logical genome bytes",
                })?;
            allocated =
                allocated
                    .checked_add(genome.allocated_bytes())
                    .ok_or(CoreError::Overflow {
                        quantity: "retired allocated genome bytes",
                    })?;
        }
        let count = u64::try_from(dying.len()).unwrap_or(u64::MAX);
        if logical > quota.logical_bytes()
            || allocated > quota.allocated_bytes()
            || count > quota.live()
        {
            return Err(conflict(format!(
                "the quota holds {} logical and {} allocated bytes for {} genomes, \
                 less than the {logical}, {allocated} and {count} the deaths return",
                quota.logical_bytes(),
                quota.allocated_bytes(),
                quota.live()
            )));
        }
        Ok(dying)
    }

    /// Check `deaths` against the live organisms: each at `tick`, alive,
    /// once, and naming the stored genome, lineage and generation.
    fn check_deaths(
        &self,
        tick: Tick,
        deaths: &[Death],
    ) -> Result<BTreeMap<OrganismId, Death>, CoreError> {
        let mut dying = BTreeMap::new();
        for &death in deaths {
            let conflict = |detail: String| CoreError::StateConflict {
                operation: "commit deaths",
                detail,
            };
            if death.tick != tick {
                return Err(conflict(format!(
                    "{} died at tick {}, not {}",
                    death.id,
                    death.tick.get(),
                    tick.get()
                )));
            }
            let Some(&slot) = self.index.get(&death.id) else {
                return Err(conflict(format!(
                    "{} is not alive; a death is committed once",
                    death.id
                )));
            };
            let stored = self.read(slot);
            if (stored.genome, stored.lineage, stored.generation)
                != (death.genome, death.lineage, death.generation)
            {
                return Err(conflict(format!(
                    "the death of {} names genome {}, lineage {} and generation {}, \
                     but it has genome {}, lineage {} and generation {}",
                    death.id,
                    death.genome,
                    death.lineage,
                    death.generation.get(),
                    stored.genome,
                    stored.lineage,
                    stored.generation.get()
                )));
            }
            if dying.insert(death.id, death).is_some() {
                return Err(conflict(format!("{} dies twice in one commit", death.id)));
            }
        }
        Ok(dying)
    }

    /// The canonical scientific state, in identity order, without slots.
    #[must_use]
    pub fn digest(&self) -> Sha256Digest {
        let mut bytes = Vec::with_capacity(64 + self.len() * 256);
        bytes.extend_from_slice(POPULATION_DIGEST_SCHEMA.as_bytes());
        bytes.push(b'\n');
        bytes.extend_from_slice(&(self.len() as u64).to_le_bytes());
        for organism in self.iter() {
            bytes.extend_from_slice(&organism.id.get().to_le_bytes());
            bytes.extend_from_slice(&organism.genome.get().to_le_bytes());
            match organism.parent {
                Some(parent) => {
                    bytes.push(1);
                    bytes.extend_from_slice(&parent.get().to_le_bytes());
                }
                None => bytes.push(0),
            }
            bytes.extend_from_slice(&organism.lineage.get().to_le_bytes());
            bytes.extend_from_slice(&organism.generation.get().to_le_bytes());
            bytes.extend_from_slice(&organism.birth_tick.get().to_le_bytes());
            for value in organism
                .position
                .iter()
                .chain(&organism.velocity)
                .chain([&organism.energy, &organism.energy_change_rate])
            {
                bytes.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            match (organism.held.evaluated_at(), organism.held.outputs()) {
                (Some(tick), Some(outputs)) => {
                    bytes.push(1);
                    bytes.extend_from_slice(&tick.get().to_le_bytes());
                    for value in outputs {
                        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
                    }
                }
                _ => bytes.push(0),
            }
            bytes.extend_from_slice(&organism.phase.to_le_bytes());
            bytes.extend_from_slice(&organism.cooldown_deadline.get().to_le_bytes());
        }
        Sha256Digest::of(&bytes)
    }

    /// Check `entrants` against the population as it will be with
    /// `remaining` organisms.
    fn check_entrants(&self, entrants: &[Entrant], remaining: usize) -> Result<(), CoreError> {
        let mut latest = self.latest;
        for entrant in entrants {
            let organism = &entrant.organism;
            if latest.is_some_and(|latest| organism.id <= latest) {
                return Err(CoreError::StateConflict {
                    operation: "admit organism",
                    detail: format!(
                        "{} is not after {}; identities are admitted once, in ascending order",
                        organism.id,
                        latest.map_or_else(String::new, |id| id.to_string())
                    ),
                });
            }
            if entrant.genome.id() != organism.genome {
                return Err(CoreError::StateConflict {
                    operation: "admit organism",
                    detail: format!(
                        "{} names genome {} but brings genome {}",
                        organism.id,
                        organism.genome,
                        entrant.genome.id()
                    ),
                });
            }
            organism.check()?;
            latest = Some(organism.id);
        }
        let total = remaining.saturating_add(entrants.len());
        if total > self.capacity {
            return Err(CoreError::BudgetExhausted {
                budget: "live population",
                required: u64::try_from(total).unwrap_or(u64::MAX),
                available: u64::try_from(self.capacity).unwrap_or(u64::MAX),
            });
        }
        Ok(())
    }

    /// Remove the checked `dying` organisms.
    fn remove(&mut self, dying: BTreeMap<OrganismId, Death>) -> Vec<Collected> {
        let mut collected = Vec::with_capacity(dying.len());
        match self.arrangement {
            Arrangement::FreeList => {
                for (id, death) in dying {
                    let Some(slot) = self.index.remove(&id) else {
                        continue;
                    };
                    let organism = self.read(slot);
                    self.occupants[slot] = None;
                    self.release_arena(slot);
                    if let Some(genome) = self.genomes[slot].take() {
                        collected.push(Collected {
                            death,
                            organism,
                            genome,
                        });
                    }
                    self.free.insert(slot);
                }
            }
            Arrangement::Compact => {
                let mut dying = dying;
                let mut kept = 0;
                for slot in 0..self.occupants.len() {
                    let Some(id) = self.occupants[slot] else {
                        continue;
                    };
                    if let Some(death) = dying.remove(&id) {
                        self.index.remove(&id);
                        let organism = self.read(slot);
                        self.release_arena(slot);
                        if let Some(genome) = self.genomes[slot].take() {
                            collected.push(Collected {
                                death,
                                organism,
                                genome,
                            });
                        }
                        continue;
                    }
                    if kept != slot {
                        self.relocate(slot, kept);
                        self.index.insert(id, kept);
                    }
                    kept += 1;
                }
                self.truncate(kept);
                collected.sort_by_key(|entry| entry.death.id);
            }
        }
        collected
    }

    /// Admit a checked entrant.
    fn insert(&mut self, entrant: Entrant) {
        let Entrant { organism, genome } = entrant;
        let arena_slot = Some(self.arena.insert(genome.topology()));
        let slot = match self.arrangement {
            Arrangement::FreeList => self.free.pop_first(),
            Arrangement::Compact => None,
        };
        let slot = if let Some(slot) = slot {
            self.write(slot, &organism);
            self.genomes[slot] = Some(genome);
            self.arena_slots[slot] = arena_slot;
            slot
        } else {
            self.push(&organism);
            self.genomes.push(Some(genome));
            self.arena_slots.push(arena_slot);
            self.occupants.len() - 1
        };
        self.occupants[slot] = Some(organism.id);
        self.index.insert(organism.id, slot);
        self.latest = Some(organism.id);
    }

    fn read(&self, slot: usize) -> Organism {
        Organism {
            id: self.occupants[slot].unwrap_or(OrganismId::new(u64::MAX)),
            genome: self.genome_ids[slot],
            parent: self.parents[slot],
            lineage: self.lineages[slot],
            generation: self.generations[slot],
            birth_tick: self.birth_ticks[slot],
            position: self.positions[slot],
            velocity: self.velocities[slot],
            energy: self.energies[slot],
            energy_change_rate: self.energy_change_rates[slot],
            held: self.held[slot],
            phase: self.phases[slot],
            cooldown_deadline: self.cooldown_deadlines[slot],
        }
    }

    /// Write every column of `slot`.
    fn write(&mut self, slot: usize, organism: &Organism) {
        self.occupants[slot] = Some(organism.id);
        self.genome_ids[slot] = organism.genome;
        self.parents[slot] = organism.parent;
        self.lineages[slot] = organism.lineage;
        self.generations[slot] = organism.generation;
        self.birth_ticks[slot] = organism.birth_tick;
        self.positions[slot] = organism.position;
        self.velocities[slot] = organism.velocity;
        self.energies[slot] = organism.energy;
        self.energy_change_rates[slot] = organism.energy_change_rate;
        self.held[slot] = organism.held;
        self.phases[slot] = organism.phase;
        self.cooldown_deadlines[slot] = organism.cooldown_deadline;
    }

    /// Append a slot, within the reserved capacity.
    fn push(&mut self, organism: &Organism) {
        self.occupants.push(Some(organism.id));
        self.genome_ids.push(organism.genome);
        self.parents.push(organism.parent);
        self.lineages.push(organism.lineage);
        self.generations.push(organism.generation);
        self.birth_ticks.push(organism.birth_tick);
        self.positions.push(organism.position);
        self.velocities.push(organism.velocity);
        self.energies.push(organism.energy);
        self.energy_change_rates.push(organism.energy_change_rate);
        self.held.push(organism.held);
        self.phases.push(organism.phase);
        self.cooldown_deadlines.push(organism.cooldown_deadline);
    }

    /// Move slot `from` down to the free slot `to`.
    fn relocate(&mut self, from: usize, to: usize) {
        let organism = self.read(from);
        self.write(to, &organism);
        self.genomes.swap(from, to);
        self.arena_slots.swap(from, to);
        self.occupants[from] = None;
    }

    /// Room in the arena for `entrants`' genomes, before anything changes.
    fn reserve_arena(&mut self, entrants: &[Entrant]) -> Result<(), CoreError> {
        self.arena
            .reserve(entrants.iter().map(|entrant| entrant.genome.topology()))
    }

    /// Free the arena slot of the genome in population slot `slot`.
    fn release_arena(&mut self, slot: usize) {
        if let Some(arena_slot) = self.arena_slots[slot].take() {
            // Issued by `insert` and freed only here, so it is live.
            let released = self.arena.release(arena_slot);
            debug_assert!(released.is_ok(), "{released:?}");
        }
    }

    fn truncate(&mut self, length: usize) {
        self.occupants.truncate(length);
        self.genome_ids.truncate(length);
        self.parents.truncate(length);
        self.lineages.truncate(length);
        self.generations.truncate(length);
        self.birth_ticks.truncate(length);
        self.positions.truncate(length);
        self.velocities.truncate(length);
        self.energies.truncate(length);
        self.energy_change_rates.truncate(length);
        self.held.truncate(length);
        self.phases.truncate(length);
        self.cooldown_deadlines.truncate(length);
        self.genomes.truncate(length);
        self.arena_slots.truncate(length);
    }
}
