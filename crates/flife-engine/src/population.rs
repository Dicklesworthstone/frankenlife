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

use std::collections::{BTreeMap, BTreeSet, btree_map};
use std::iter::Peekable;

use flife_core::clock::Tick;
use flife_core::ids::{Generation, GenomeId, LineageId, OrganismId};
use flife_core::memory::{capacity_bytes, total};
use flife_core::{CoreError, Sha256Digest, V2Feature, ValidatedConfig};
use flife_genome::{Genome, GenomeArena, GenomeQuota, MutationRecord};
use flife_neural::{ACTION_WIDTH, ArenaSlot, MAX_ACTION_WIDTH, MAXIMUM_RECURRENT_STATE_WIDTH};

use crate::emission::INITIAL_SECRETORY_READINESS;
use crate::lifecycle::Death;
use crate::reproduction::{Birth, Newborn};

/// The canonical digest's schema line.
pub const POPULATION_DIGEST_SCHEMA: &str = "frankenlife.population-state.v1";

/// A controller's held raw outputs (§6.6) and, for
/// `independent-recurrent-mlp-v1` / `epigenetic-recurrence` (FL-054, V2-01,
/// V2-12), its bounded internal recurrent state `h ∈ [-1, 1]^S`. An invalid
/// output cannot be read, so a stale one can never be decoded.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeldOutput {
    evaluated_at: Option<Tick>,
    output_len: u8,
    outputs: [f32; MAX_ACTION_WIDTH],
    state_len: u8,
    state: [f32; MAXIMUM_RECURRENT_STATE_WIDTH],
}

impl HeldOutput {
    /// No held output and zero internal state: the next advance forces an
    /// evaluation.
    pub const INVALID: Self = Self {
        evaluated_at: None,
        output_len: 0,
        outputs: [0.0; MAX_ACTION_WIDTH],
        state_len: 0,
        state: [0.0; MAXIMUM_RECURRENT_STATE_WIDTH],
    };

    /// No held output (next advance forces first evaluation, Appendix C.13),
    /// carrying inherited bounded epigenetic state `state ∈ [-1, 1]^S` (V2-01,
    /// V2-12).
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] if `state.len() > MAXIMUM_RECURRENT_STATE_WIDTH`;
    /// [`CoreError::NumericalInvalid`] if any entry lies outside `[-1, 1]` or is
    /// nonfinite.
    pub fn newborn_with_state(state: &[f32]) -> Result<Self, CoreError> {
        if state.len() > MAXIMUM_RECURRENT_STATE_WIDTH {
            return Err(CoreError::LayoutMismatch {
                buffer: "recurrent controller state",
                detail: format!(
                    "expected at most {MAXIMUM_RECURRENT_STATE_WIDTH} state values, got {}",
                    state.len()
                ),
            });
        }
        let mut stored = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
        for (index, (slot, &value)) in stored.iter_mut().zip(state).enumerate() {
            if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "recurrent controller state",
                    detail: format!("state[{index}] is {value}, outside [-1, 1]"),
                });
            }
            *slot = value;
        }
        let state_len = u8::try_from(state.len()).map_err(|_| CoreError::Overflow {
            quantity: "recurrent controller state width",
        })?;
        Ok(Self {
            evaluated_at: None,
            output_len: 0,
            outputs: [0.0; MAX_ACTION_WIDTH],
            state_len,
            state: stored,
        })
    }

    /// Baseline 36-element `outputs`, evaluated at `tick` by a stateless
    /// controller.
    ///
    /// # Errors
    /// [`CoreError::NumericalInvalid`] for a nonfinite output.
    pub fn evaluated(tick: Tick, outputs: [f32; ACTION_WIDTH]) -> Result<Self, CoreError> {
        Self::evaluated_slice(tick, &outputs)
    }

    /// Variable-width `outputs` (`ACTION_WIDTH..=MAX_ACTION_WIDTH`), evaluated
    /// at `tick` by a stateless controller (V2-01).
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a length outside
    /// `ACTION_WIDTH..=MAX_ACTION_WIDTH`; [`CoreError::NumericalInvalid`] for
    /// a nonfinite output.
    pub fn evaluated_slice(tick: Tick, outputs: &[f32]) -> Result<Self, CoreError> {
        if outputs.len() < ACTION_WIDTH || outputs.len() > MAX_ACTION_WIDTH {
            return Err(CoreError::LayoutMismatch {
                buffer: "held controller output",
                detail: format!(
                    "expected {ACTION_WIDTH}..={MAX_ACTION_WIDTH} outputs, got {}",
                    outputs.len()
                ),
            });
        }
        if let Some(index) = outputs.iter().position(|value| !value.is_finite()) {
            return Err(CoreError::NumericalInvalid {
                quantity: "held controller output",
                detail: format!("output {index} is {}", outputs[index]),
            });
        }
        let mut stored = [0.0_f32; MAX_ACTION_WIDTH];
        stored[..outputs.len()].copy_from_slice(outputs);
        let output_len = u8::try_from(outputs.len()).map_err(|_| CoreError::Overflow {
            quantity: "held controller output width",
        })?;
        Ok(Self {
            evaluated_at: Some(tick),
            output_len,
            outputs: stored,
            state_len: 0,
            state: [0.0; MAXIMUM_RECURRENT_STATE_WIDTH],
        })
    }

    /// Baseline 36-element `outputs` and bounded recurrent `state ∈ [-1, 1]^S`,
    /// evaluated at `tick` by `independent-recurrent-mlp-v1` (FL-054).
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for a state length outside
    /// `1..=MAXIMUM_RECURRENT_STATE_WIDTH`; [`CoreError::NumericalInvalid`]
    /// for a nonfinite output or a state entry outside `[-1, 1]`.
    pub fn evaluated_recurrent(
        tick: Tick,
        outputs: [f32; ACTION_WIDTH],
        state: &[f32],
    ) -> Result<Self, CoreError> {
        Self::evaluated_recurrent_slice(tick, &outputs, state)
    }

    /// Variable-width `outputs` (`ACTION_WIDTH..=MAX_ACTION_WIDTH`) and bounded
    /// recurrent `state ∈ [-1, 1]^S`, evaluated at `tick` (FL-054, V2-01).
    ///
    /// # Errors
    /// [`CoreError::LayoutMismatch`] for lengths outside their bounds;
    /// [`CoreError::NumericalInvalid`] for a nonfinite output or a state entry
    /// outside `[-1, 1]`.
    pub fn evaluated_recurrent_slice(
        tick: Tick,
        outputs: &[f32],
        state: &[f32],
    ) -> Result<Self, CoreError> {
        if state.is_empty() || state.len() > MAXIMUM_RECURRENT_STATE_WIDTH {
            return Err(CoreError::LayoutMismatch {
                buffer: "recurrent controller state",
                detail: format!(
                    "expected 1..={MAXIMUM_RECURRENT_STATE_WIDTH} state values, got {}",
                    state.len()
                ),
            });
        }
        let mut base = Self::evaluated_slice(tick, outputs)?;
        let mut stored = [0.0_f32; MAXIMUM_RECURRENT_STATE_WIDTH];
        for (index, (slot, &value)) in stored.iter_mut().zip(state).enumerate() {
            if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
                return Err(CoreError::NumericalInvalid {
                    quantity: "recurrent controller state",
                    detail: format!("state[{index}] is {value}, outside [-1, 1]"),
                });
            }
            *slot = value;
        }
        base.state_len = u8::try_from(state.len()).map_err(|_| CoreError::Overflow {
            quantity: "recurrent controller state width",
        })?;
        base.state = stored;
        Ok(base)
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

    /// The held action outputs (`ACTION_WIDTH..=MAX_ACTION_WIDTH`), if any.
    #[must_use]
    pub fn outputs(&self) -> Option<&[f32]> {
        if self.evaluated_at.is_some() {
            Some(&self.outputs[..usize::from(self.output_len)])
        } else {
            None
        }
    }

    /// The held recurrent internal state slice (empty for stateless
    /// controllers or unevaluated organisms without inherited state).
    #[must_use]
    pub fn recurrent_state(&self) -> &[f32] {
        &self.state[..usize::from(self.state_len)]
    }

    /// Fixed-width array holding the recurrent state in its leading lanes
    /// (zero-filled for unevaluated organisms without inherited state and
    /// unused tail lanes).
    #[must_use]
    pub const fn recurrent_state_array(&self) -> [f32; MAXIMUM_RECURRENT_STATE_WIDTH] {
        self.state
    }
}

/// Default unit apical-basal polarity vector when `V2-08` `pcp-polarity` is
/// disabled (`[1.0, 0.0]`).
pub const DEFAULT_POLARITY: [f64; 2] = [1.0, 0.0];

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
    /// Dimensionless secretory readiness `r_i ∈ [0, 1]` for `V2-04`
    /// `excitable-relay` (`vesicle-readiness-v1`). Holds zero energy (`INV-03`).
    pub secretory_readiness: f32,
    /// The first tick at which it may fire a burst under `V2-04`
    /// `excitable-relay` (`tick-cooldown-v1`).
    pub emission_cooldown_deadline: Tick,
    /// Persistent unit apical-basal polarity axis `p_i ∈ S^1` for `V2-08`
    /// (`pcp-polarity`). Holds [`DEFAULT_POLARITY`] (`0 B` storage in
    /// [`Population`]) when `pcp-polarity` is disabled.
    pub polarity: [f64; 2],
    /// Kin-weighted mean neighbor primary recurrent state `h̄_0 ∈ [-1, 1]`
    /// from the previous tick's contact Sweep 1 (`V2-12`, `notch-delta`).
    /// Holds `0.0` (`0 B` storage in [`Population`]) when `notch-delta` is
    /// disabled.
    pub last_neighbor_h0: f32,
}

impl Organism {
    /// The row a staged newborn commits as: zero velocity and history, and
    /// an invalid held output (`evaluated_at == None`, carrying optional
    /// inherited epigenetic state under `V2-12`).
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
            held: newborn.held,
            phase: newborn.phase,
            cooldown_deadline: newborn.cooldown_deadline,
            secretory_readiness: INITIAL_SECRETORY_READINESS,
            emission_cooldown_deadline: Tick::new(0),
            polarity: newborn.polarity,
            last_neighbor_h0: 0.0,
        }
    }

    /// INV-01: finite state and strictly positive energy.
    fn check(&self) -> Result<(), CoreError> {
        let finite = self.position[0].is_finite()
            && self.position[1].is_finite()
            && self.velocity[0].is_finite()
            && self.velocity[1].is_finite()
            && self.polarity[0].is_finite()
            && self.polarity[1].is_finite()
            && self.energy_change_rate.is_finite();
        if !finite {
            return Err(CoreError::NumericalInvalid {
                quantity: "organism state",
                detail: format!(
                    "{}: position {:?}, velocity {:?}, polarity {:?}, energy change rate {}",
                    self.id, self.position, self.velocity, self.polarity, self.energy_change_rate
                ),
            });
        }
        let pol_sq = self.polarity[0] * self.polarity[0] + self.polarity[1] * self.polarity[1];
        if !(pol_sq.is_finite() && (pol_sq - 1.0).abs() <= 1.0e-6) {
            return Err(CoreError::NumericalInvalid {
                quantity: "organism polarity",
                detail: format!(
                    "{}: polarity {:?} is not a unit vector",
                    self.id, self.polarity
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
        if !self.secretory_readiness.is_finite()
            || !(0.0_f32..=1.0_f32).contains(&self.secretory_readiness)
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "organism secretory readiness",
                detail: format!(
                    "{}: {} is outside [0, 1]",
                    self.id, self.secretory_readiness
                ),
            });
        }
        if !self.last_neighbor_h0.is_finite()
            || !(-1.0_f32..=1.0_f32).contains(&self.last_neighbor_h0)
        {
            return Err(CoreError::NumericalInvalid {
                quantity: "organism notch-delta neighbor state",
                detail: format!(
                    "{}: last_neighbor_h0 {} is outside [-1, 1]",
                    self.id, self.last_neighbor_h0
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

/// A validated tick commit returned by [`Population::check_tick_staged`] and
/// consumed by [`Population::apply_checked_tick`] without re-scanning every
/// survivor while the caller holds exclusive access to the population.
#[derive(Debug)]
pub(crate) struct CheckedTick {
    dying: BTreeMap<OrganismId, Death>,
}

impl Population {
    /// Validate a whole tick commit and return the checked deaths map so
    /// [`Population::apply_checked_tick`] can commit it without re-checking
    /// every survivor a second time.
    pub(crate) fn check_tick_staged(
        &self,
        tick: Tick,
        survivors: &[Organism],
        deaths: &[Death],
        births: &[Entrant],
        quota: &GenomeQuota,
    ) -> Result<CheckedTick, CoreError> {
        self.check_tick_staged_on(tick, survivors, deaths, births, quota, 1)
    }

    /// Parallel counterpart to [`Population::check_tick_staged`] using up to
    /// `workers` threads; returns bit-identical validation results and exact
    /// canonical first-error reporting.
    pub(crate) fn check_tick_staged_on(
        &self,
        tick: Tick,
        survivors: &[Organism],
        deaths: &[Death],
        births: &[Entrant],
        quota: &GenomeQuota,
        workers: usize,
    ) -> Result<CheckedTick, CoreError> {
        let dying = self.check_whole_tick_on(tick, survivors, deaths, births, quota, workers)?;
        Ok(CheckedTick { dying })
    }

    /// Apply a tick commit already validated against this exact population
    /// by [`Population::check_tick_staged`].
    pub(crate) fn apply_checked_tick(
        &mut self,
        checked: CheckedTick,
        survivors: &[Organism],
        births: &mut Vec<Entrant>,
        quota: &mut GenomeQuota,
        released: Option<&mut Vec<ArenaSlot>>,
    ) -> Result<(), CoreError> {
        self.reserve_arena(births)?;
        if self.is_compact_dense() && checked.dying.is_empty() && survivors.len() == self.len() {
            for (slot, organism) in survivors.iter().enumerate() {
                self.write(slot, organism);
            }
        } else {
            let index = core::mem::take(&mut self.index);
            let mut live = index.iter();
            for organism in survivors {
                if let Some(slot) = next_slot(&mut live, organism.id) {
                    self.write(slot, organism);
                }
            }
            self.index = index;
            for collected in self.remove(checked.dying, released) {
                quota.retire(collected.genome)?;
            }
        }
        for entrant in births.drain(..) {
            self.insert(entrant);
        }
        Ok(())
    }
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
    /// Whether `V2-04` excitable-relay columns are active and allocated.
    /// When `false` (`field-ecology-v1*` or `excitable-relay` disabled), the
    /// two columns below stay empty (0 bytes) and reads return default values.
    tracks_excitable: bool,
    secretory_readiness: Vec<f32>,
    emission_cooldown_deadlines: Vec<Tick>,
    /// Whether `V2-08` PCP polarity column is active and allocated.
    /// When `false` (`field-ecology-v1*` or `pcp-polarity` disabled),
    /// `polarities` stays empty (0 bytes) and reads return [`DEFAULT_POLARITY`].
    tracks_polarity: bool,
    polarities: Vec<[f64; 2]>,
    /// Whether `V2-12` Notch-Delta neighbor state column is active and allocated.
    /// When `false`, `last_neighbor_h0` stays empty (0 bytes) and reads return `0.0`.
    tracks_notch_delta: bool,
    last_neighbor_h0: Vec<f32>,
    /// Whether `V2-13` `cadherin-4d` traits are active and hashed into
    /// [`Population::digest`] under tag byte `7`. Stored inside each organism's
    /// [`Genome`] `Traits` (`0 B` extra SoA column allocation).
    tracks_cadherin: bool,
    genomes: Vec<Option<Genome>>,
    /// Each slot's genome's arena slot, beside `genomes`.
    arena_slots: Vec<Option<ArenaSlot>>,
    arena: GenomeArena,
    index: BTreeMap<OrganismId, usize>,
    free: BTreeSet<usize>,
    latest: Option<OrganismId>,
}

/// Cursor strategy behind [`Ascending`]: direct array cursor under dense
/// [`Arrangement::Compact`], falling back to the ordered identity `BTreeMap`
/// under [`Arrangement::FreeList`].
#[derive(Debug)]
enum AscendingCursor<'a> {
    Compact {
        cursor: usize,
    },
    Tree(Peekable<btree_map::Range<'a, OrganismId, usize>>),
}

/// One ascending walk of a population's identity index (FL-034), for a
/// caller visiting organisms in identity order: [`Ascending::next`] gives
/// what [`Population::genome`] and [`Population::arena_slot`] give, read
/// from the organism's one slot, without a search per organism.
#[derive(Debug)]
pub struct Ascending<'a> {
    population: &'a Population,
    cursor: AscendingCursor<'a>,
}

impl<'a> Ascending<'a> {
    /// `id`'s genome and its genome's arena slot, for an `id` above every
    /// identity this walk found before: the entries below `id` are passed,
    /// and `id`'s is taken. `(None, None)` when `id` is not alive or not
    /// above the last identity found; the walk then keeps the entries at
    /// and after `id`.
    pub fn next(&mut self, id: OrganismId) -> (Option<&'a Genome>, Option<ArenaSlot>) {
        let population = self.population;
        match &mut self.cursor {
            AscendingCursor::Compact { cursor } => {
                let occupants = &population.occupants;
                while *cursor < occupants.len() {
                    match occupants[*cursor] {
                        Some(entry) if entry < id => *cursor += 1,
                        Some(entry) if entry == id => {
                            let slot = *cursor;
                            *cursor += 1;
                            return (
                                population.genomes.get(slot).and_then(Option::as_ref),
                                population.arena_slots.get(slot).copied().flatten(),
                            );
                        }
                        _ => return (None, None),
                    }
                }
                (None, None)
            }
            AscendingCursor::Tree(live) => {
                while live.next_if(|&(&entry, _)| entry < id).is_some() {}
                live.next_if(|&(&entry, _)| entry == id)
                    .map_or((None, None), |(_, &slot)| {
                        (
                            population.genomes.get(slot).and_then(Option::as_ref),
                            population.arena_slots.get(slot).copied().flatten(),
                        )
                    })
            }
        }
    }
}

/// `id`'s slot, from an ascending walk of the index that has not passed
/// `id`: the entries below `id` are skipped and the one at or after it is
/// taken. `None` when `id` is not alive.
fn next_slot(live: &mut btree_map::Iter<'_, OrganismId, usize>, id: OrganismId) -> Option<usize> {
    live.find(|&(&entry, _)| entry >= id)
        .and_then(|(&entry, &slot)| (entry == id).then_some(slot))
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
        Self::new_with_excitable(arrangement, capacity, false)
    }

    /// An empty population of at most `capacity` organisms, optionally
    /// allocating the `V2-04` excitable-relay columns (`secretory_readiness`
    /// and `emission_cooldown_deadlines`) up front (`INV-15`).
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses a column.
    pub fn new_with_excitable(
        arrangement: Arrangement,
        capacity: usize,
        tracks_excitable: bool,
    ) -> Result<Self, CoreError> {
        Self::new_with_v2(arrangement, capacity, tracks_excitable, false)
    }

    /// An empty population of at most `capacity` organisms, conditionally
    /// allocating `V2-04` excitable-relay and/or `V2-08` polarity SoA columns
    /// up front (`INV-15`; `0 B` for any disabled column).
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses a column.
    pub fn new_with_v2(
        arrangement: Arrangement,
        capacity: usize,
        tracks_excitable: bool,
        tracks_polarity: bool,
    ) -> Result<Self, CoreError> {
        Self::new_with_v2_columns(
            arrangement,
            capacity,
            tracks_excitable,
            tracks_polarity,
            false,
        )
    }

    /// An empty population of at most `capacity` organisms, conditionally
    /// allocating `V2-04` excitable-relay, `V2-08` polarity, and/or `V2-12`
    /// Notch-Delta SoA columns up front (`INV-15`; `0 B` for any disabled
    /// column).
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses a column.
    pub fn new_with_v2_columns(
        arrangement: Arrangement,
        capacity: usize,
        tracks_excitable: bool,
        tracks_polarity: bool,
        tracks_notch_delta: bool,
    ) -> Result<Self, CoreError> {
        Self::new_with_v2_features(
            arrangement,
            capacity,
            tracks_excitable,
            tracks_polarity,
            tracks_notch_delta,
            false,
        )
    }

    /// An empty population of at most `capacity` organisms, conditionally
    /// allocating `V2-04` excitable-relay, `V2-08` polarity, and `V2-12`
    /// Notch-Delta SoA columns up front (`INV-15`; `0 B` for any disabled
    /// column) and optionally hashing `V2-13` `cadherin-4d` traits under
    /// [`Population::digest`] tag byte `7`.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] when the host refuses a column.
    pub fn new_with_v2_features(
        arrangement: Arrangement,
        capacity: usize,
        tracks_excitable: bool,
        tracks_polarity: bool,
        tracks_notch_delta: bool,
        tracks_cadherin: bool,
    ) -> Result<Self, CoreError> {
        let (secretory_readiness, emission_cooldown_deadlines) = if tracks_excitable {
            (reserve(capacity)?, reserve(capacity)?)
        } else {
            (Vec::new(), Vec::new())
        };
        let polarities = if tracks_polarity {
            reserve(capacity)?
        } else {
            Vec::new()
        };
        let last_neighbor_h0 = if tracks_notch_delta {
            reserve(capacity)?
        } else {
            Vec::new()
        };
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
            tracks_excitable,
            secretory_readiness,
            emission_cooldown_deadlines,
            tracks_polarity,
            polarities,
            tracks_notch_delta,
            last_neighbor_h0,
            tracks_cadherin,
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
        let tracks_excitable = config
            .config()
            .v2
            .as_ref()
            .is_some_and(|v2| v2.is_enabled(V2Feature::ExcitableRelay));
        let tracks_polarity = config
            .config()
            .v2
            .as_ref()
            .is_some_and(|v2| v2.is_enabled(V2Feature::PcpPolarity));
        let tracks_notch_delta = config
            .config()
            .v2
            .as_ref()
            .is_some_and(|v2| v2.is_enabled(V2Feature::NotchDelta));
        let tracks_cadherin = config
            .config()
            .v2
            .as_ref()
            .is_some_and(|v2| v2.is_enabled(V2Feature::Cadherin4d));
        Self::new_with_v2_features(
            arrangement,
            capacity,
            tracks_excitable,
            tracks_polarity,
            tracks_notch_delta,
            tracks_cadherin,
        )
    }

    /// Whether this population stores live `V2-08` PCP polarity vectors.
    #[must_use]
    pub const fn tracks_polarity(&self) -> bool {
        self.tracks_polarity
    }

    /// Whether this population stores live `V2-12` Notch-Delta neighbor states.
    #[must_use]
    pub const fn tracks_notch_delta(&self) -> bool {
        self.tracks_notch_delta
    }

    /// Whether this population hashes `V2-13` `cadherin-4d` traits in its
    /// canonical digest.
    #[must_use]
    pub const fn tracks_cadherin(&self) -> bool {
        self.tracks_cadherin
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
            capacity_bytes(&self.secretory_readiness),
            capacity_bytes(&self.emission_cooldown_deadlines),
            capacity_bytes(&self.polarities),
            capacity_bytes(&self.last_neighbor_h0),
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

    /// Every live organism with its genome, in identity order: what
    /// [`Population::iter`] and [`Population::genome`] give, read from the
    /// one slot without a second search.
    pub fn iter_with_genomes(&self) -> impl Iterator<Item = (Organism, Option<&Genome>)> + '_ {
        self.index.values().map(|&slot| {
            let genome = self.genomes.get(slot).and_then(Option::as_ref);
            (self.read(slot), genome)
        })
    }

    /// `id`'s genome's arena slot.
    #[must_use]
    pub fn arena_slot(&self, id: OrganismId) -> Option<ArenaSlot> {
        self.index
            .get(&id)
            .and_then(|&slot| self.arena_slots.get(slot).copied().flatten())
    }

    /// Whether physical slots `0..self.len()` are densely occupied in exact
    /// ascending identity order (guaranteed invariant of [`Arrangement::Compact`]).
    #[inline]
    fn is_compact_dense(&self) -> bool {
        self.arrangement == Arrangement::Compact && self.occupants.len() == self.index.len()
    }

    /// Direct column read of the six immutable lineage fields at `slot` without
    /// materializing the full [`Organism`] row or copying [`HeldOutput`].
    #[inline]
    fn lineage_key_at(
        &self,
        slot: usize,
    ) -> (
        GenomeId,
        Option<OrganismId>,
        LineageId,
        Generation,
        Tick,
        u32,
    ) {
        (
            self.genome_ids[slot],
            self.parents[slot],
            self.lineages[slot],
            self.generations[slot],
            self.birth_ticks[slot],
            self.phases[slot],
        )
    }

    /// `Σ E` over every live organism in ascending identity order, compensated,
    /// reading only the energy column without materializing [`Organism`] rows.
    #[must_use]
    pub fn energy_total(&self) -> f64 {
        let mut total = flife_core::CompensatedSum::ZERO;
        if self.is_compact_dense() {
            for &energy in &self.energies {
                total.add(energy);
            }
        } else {
            for &slot in self.index.values() {
                total.add(self.energies[slot]);
            }
        }
        total.value()
    }

    /// Copy every live organism into `rows` in ascending identity order, using
    /// up to `workers` threads on the dense [`Arrangement::Compact`] fast path.
    ///
    /// # Errors
    /// [`CoreError::AllocationFailed`] if `rows` cannot hold `self.len()`, or
    /// [`CoreError::StateConflict`] if the worker pool fails.
    pub fn copy_rows_into_on(
        &self,
        rows: &mut Vec<Organism>,
        workers: usize,
    ) -> Result<(), CoreError> {
        let len = self.len();
        rows.clear();
        if len == 0 {
            return Ok(());
        }
        if self.is_compact_dense() && len >= 1_024 && workers > 1 {
            rows.try_reserve(len)
                .map_err(|_| CoreError::AllocationFailed {
                    buffer: "organism rows",
                    bytes: u64::try_from(len.saturating_mul(size_of::<Organism>()))
                        .unwrap_or(u64::MAX),
                })?;
            let placeholder = self.read(0);
            rows.resize(len, placeholder);
            flife_core::pool::parallel_chunks_mut(
                workers,
                "copy organism rows",
                rows.as_mut_slice(),
                512,
                |start, chunk| {
                    for (offset, dst) in chunk.iter_mut().enumerate() {
                        *dst = self.read(start + offset);
                    }
                },
            )?;
            return Ok(());
        }
        rows.extend(self.iter());
        Ok(())
    }

    /// A walk from the lowest identity that gives [`Population::genome`]
    /// and [`Population::arena_slot`] for ascending identities ([`Ascending`]).
    #[must_use]
    pub fn ascending(&self) -> Ascending<'_> {
        let cursor = if self.is_compact_dense() {
            AscendingCursor::Compact { cursor: 0 }
        } else {
            AscendingCursor::Tree(self.index.range(..).peekable())
        };
        Ascending {
            population: self,
            cursor,
        }
    }

    /// A walk starting at the first live identity at or above `first`, for a
    /// worker lane visiting a contiguous slice of identity-ordered organisms
    /// without per-organism tree searches.
    #[must_use]
    pub fn ascending_from(&self, first: OrganismId) -> Ascending<'_> {
        let cursor = if self.is_compact_dense() {
            let cursor = self
                .occupants
                .partition_point(|slot| slot.is_some_and(|id| id < first));
            AscendingCursor::Compact { cursor }
        } else {
            AscendingCursor::Tree(self.index.range(first..).peekable())
        };
        Ascending {
            population: self,
            cursor,
        }
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

    /// Admit a single organism outside a tick without staging a temporary
    /// `Vec<Entrant>` (used by streaming checkpoint restore to avoid holding
    /// every entrant beside the population; FL-043).
    ///
    /// # Errors
    /// As [`Population::admit`]. Nothing changes on error.
    pub fn admit_one(&mut self, entrant: Entrant) -> Result<(), CoreError> {
        self.check_entrants(std::slice::from_ref(&entrant), self.len())?;
        self.arena.reserve_one(entrant.genome.topology())?;
        self.insert(entrant);
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
        if self.lineage_key_at(slot) != organism.lineage_key() {
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

        let collected = self.remove(dying, None);
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
        let checked = self.check_tick_staged(tick, survivors, deaths, births, quota)?;
        self.apply_checked_tick(checked, survivors, births, quota, None)
    }

    fn check_whole_tick(
        &self,
        tick: Tick,
        survivors: &[Organism],
        deaths: &[Death],
        births: &[Entrant],
        quota: &GenomeQuota,
    ) -> Result<BTreeMap<OrganismId, Death>, CoreError> {
        self.check_whole_tick_on(tick, survivors, deaths, births, quota, 1)
    }

    fn check_whole_tick_on(
        &self,
        tick: Tick,
        survivors: &[Organism],
        deaths: &[Death],
        births: &[Entrant],
        quota: &GenomeQuota,
        workers: usize,
    ) -> Result<BTreeMap<OrganismId, Death>, CoreError> {
        let dying = self.check_deaths(tick, deaths)?;
        let conflict = |detail: String| CoreError::StateConflict {
            operation: "commit tick",
            detail,
        };
        let mut fast_validated = false;
        if self.is_compact_dense() && survivors.len() >= 1_024 && workers > 1 {
            if dying.is_empty() && survivors.len() == self.len() {
                let tile_count = workers
                    .saturating_mul(4)
                    .min(64)
                    .min(survivors.len().div_ceil(512))
                    .max(1);
                let tile_len = survivors.len().div_ceil(tile_count);
                let mut tiles: Vec<(usize, &[Organism])> = survivors
                    .chunks(tile_len)
                    .enumerate()
                    .map(|(idx, chunk)| (idx * tile_len, chunk))
                    .collect();
                let ok = flife_core::pool::try_parallel_tasks(
                    workers,
                    "check tick survivors",
                    &mut tiles,
                    |_, (start, chunk)| {
                        for (offset, organism) in chunk.iter().enumerate() {
                            let slot = *start + offset;
                            if self.occupants[slot] != Some(organism.id) {
                                return Err(conflict(format!("{} slot mismatch", organism.id)));
                            }
                            if self.lineage_key_at(slot) != organism.lineage_key() {
                                return Err(conflict(format!(
                                    "{} cannot change its genome, ancestry, birth tick or phase",
                                    organism.id
                                )));
                            }
                            organism.check()?;
                        }
                        Ok(())
                    },
                )
                .is_ok();
                if ok {
                    fast_validated = true;
                }
            } else if survivors.len() + dying.len() == self.len() {
                // Dense compact scan mapping survivors to slots without BTreeMap
                // lookups, then parallel INV-01 check across workers.
                let mut slot = 0_usize;
                let mut all_matched = true;
                let n = self.occupants.len();
                for organism in survivors {
                    while slot < n {
                        match self.occupants[slot] {
                            Some(entry) if entry < organism.id => slot += 1,
                            _ => break,
                        }
                    }
                    if slot >= n
                        || self.occupants[slot] != Some(organism.id)
                        || dying.contains_key(&organism.id)
                        || self.lineage_key_at(slot) != organism.lineage_key()
                    {
                        all_matched = false;
                        break;
                    }
                    slot += 1;
                }
                if all_matched {
                    let tile_count = workers
                        .saturating_mul(4)
                        .min(64)
                        .min(survivors.len().div_ceil(512))
                        .max(1);
                    let tile_len = survivors.len().div_ceil(tile_count);
                    let mut tiles: Vec<&[Organism]> = survivors.chunks(tile_len).collect();
                    let ok = flife_core::pool::try_parallel_tasks(
                        workers,
                        "check tick survivors",
                        &mut tiles,
                        |_, chunk| {
                            for organism in *chunk {
                                organism.check()?;
                            }
                            Ok(())
                        },
                    )
                    .is_ok();
                    if ok {
                        fast_validated = true;
                    }
                }
            }
        }
        if !fast_validated {
            let mut previous: Option<OrganismId> = None;
            let mut live = self.index.iter();
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
                // The survivors so far ascend and each matched its own entry,
                // so the walk stands just past the last one's entry and this
                // one's, if it is alive, lies ahead: one ordered walk of the
                // index, not a search per survivor.
                let Some(slot) = next_slot(&mut live, organism.id) else {
                    return Err(conflict(format!("{} is not alive", organism.id)));
                };
                if self.lineage_key_at(slot) != organism.lineage_key() {
                    return Err(conflict(format!(
                        "{} cannot change its genome, ancestry, birth tick or phase",
                        organism.id
                    )));
                }
                organism.check()?;
            }
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
        for (organism, genome) in self.iter_with_genomes() {
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
                    if outputs.len() != ACTION_WIDTH {
                        bytes.push(outputs.len() as u8);
                    }
                    for value in outputs {
                        bytes.extend_from_slice(&value.to_bits().to_le_bytes());
                    }
                    let recurrent = organism.held.recurrent_state();
                    if !recurrent.is_empty() {
                        bytes.push(recurrent.len() as u8);
                        for value in recurrent {
                            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
                        }
                    }
                }
                _ => {
                    let recurrent = organism.held.recurrent_state();
                    if recurrent.is_empty() {
                        bytes.push(0);
                    } else {
                        bytes.push(2);
                        bytes.push(recurrent.len() as u8);
                        for value in recurrent {
                            bytes.extend_from_slice(&value.to_bits().to_le_bytes());
                        }
                    }
                }
            }
            bytes.extend_from_slice(&organism.phase.to_le_bytes());
            bytes.extend_from_slice(&organism.cooldown_deadline.get().to_le_bytes());
            if self.tracks_excitable {
                bytes.push(4);
                bytes.extend_from_slice(&organism.secretory_readiness.to_bits().to_le_bytes());
                bytes.extend_from_slice(&organism.emission_cooldown_deadline.get().to_le_bytes());
            }
            if self.tracks_polarity {
                bytes.push(5);
                bytes.extend_from_slice(&organism.polarity[0].to_bits().to_le_bytes());
                bytes.extend_from_slice(&organism.polarity[1].to_bits().to_le_bytes());
                if let Some(pol) = genome.and_then(|g| g.traits().polarity_traits()) {
                    bytes.push(1);
                    bytes.extend_from_slice(&pol.polarity_strength().to_bits().to_le_bytes());
                    bytes.extend_from_slice(&pol.wedge_curvature().to_bits().to_le_bytes());
                } else {
                    bytes.push(0);
                }
            }
            if self.tracks_notch_delta {
                bytes.push(6);
                bytes.extend_from_slice(&organism.last_neighbor_h0.to_bits().to_le_bytes());
            }
            if self.tracks_cadherin {
                bytes.push(7);
                if let Some(cad) = genome.and_then(|g| g.traits().cadherin_traits()) {
                    bytes.push(1);
                    for component in cad.tag() {
                        bytes.extend_from_slice(&component.to_bits().to_le_bytes());
                    }
                    bytes.extend_from_slice(&cad.cohesiveness().to_bits().to_le_bytes());
                } else {
                    bytes.push(0);
                }
            }
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

    /// Remove the checked `dying` organisms, optionally recording each retired
    /// arena slot in `released`.
    fn remove(
        &mut self,
        dying: BTreeMap<OrganismId, Death>,
        mut released: Option<&mut Vec<ArenaSlot>>,
    ) -> Vec<Collected> {
        let mut collected = Vec::with_capacity(dying.len());
        match self.arrangement {
            Arrangement::FreeList => {
                for (id, death) in dying {
                    let Some(slot) = self.index.remove(&id) else {
                        continue;
                    };
                    let organism = self.read(slot);
                    self.occupants[slot] = None;
                    if let Some(arena_slot) = self.release_arena(slot)
                        && let Some(out) = released.as_deref_mut()
                    {
                        out.push(arena_slot);
                    }
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
                        if let Some(arena_slot) = self.release_arena(slot)
                            && let Some(out) = released.as_deref_mut()
                        {
                            out.push(arena_slot);
                        }
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
        let (secretory_readiness, emission_cooldown_deadline) = if self.tracks_excitable {
            (
                self.secretory_readiness[slot],
                self.emission_cooldown_deadlines[slot],
            )
        } else {
            (INITIAL_SECRETORY_READINESS, Tick::new(0))
        };
        let polarity = if self.tracks_polarity {
            self.polarities[slot]
        } else {
            DEFAULT_POLARITY
        };
        let last_neighbor_h0 = if self.tracks_notch_delta {
            self.last_neighbor_h0[slot]
        } else {
            0.0
        };
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
            secretory_readiness,
            emission_cooldown_deadline,
            polarity,
            last_neighbor_h0,
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
        if self.tracks_excitable {
            self.secretory_readiness[slot] = organism.secretory_readiness;
            self.emission_cooldown_deadlines[slot] = organism.emission_cooldown_deadline;
        }
        if self.tracks_polarity {
            self.polarities[slot] = organism.polarity;
        }
        if self.tracks_notch_delta {
            self.last_neighbor_h0[slot] = organism.last_neighbor_h0;
        }
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
        if self.tracks_excitable {
            self.secretory_readiness.push(organism.secretory_readiness);
            self.emission_cooldown_deadlines
                .push(organism.emission_cooldown_deadline);
        }
        if self.tracks_polarity {
            self.polarities.push(organism.polarity);
        }
        if self.tracks_notch_delta {
            self.last_neighbor_h0.push(organism.last_neighbor_h0);
        }
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
    fn release_arena(&mut self, slot: usize) -> Option<ArenaSlot> {
        if let Some(arena_slot) = self.arena_slots[slot].take() {
            // Issued by `insert` and freed only here, so it is live.
            let released = self.arena.release(arena_slot);
            debug_assert!(released.is_ok(), "{released:?}");
            Some(arena_slot)
        } else {
            None
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
        if self.tracks_excitable {
            self.secretory_readiness.truncate(length);
            self.emission_cooldown_deadlines.truncate(length);
        }
        if self.tracks_polarity {
            self.polarities.truncate(length);
        }
        if self.tracks_notch_delta {
            self.last_neighbor_h0.truncate(length);
        }
        self.genomes.truncate(length);
        self.arena_slots.truncate(length);
    }
}
